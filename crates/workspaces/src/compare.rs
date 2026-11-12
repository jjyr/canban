//! General Git reference discovery and arbitrary comparison for the Diff & Review workbench.
use super::Ops;
use anyhow::{Context, Result, ensure};
use canban_types::Project;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct ParsedFile {
    filename: String,
    status: String,
    additions: usize,
    deletions: usize,
    patch: String,
}

fn validate_ref_arg(value: &str) -> Result<()> {
    let trimmed = value.trim();
    ensure!(!trimmed.is_empty(), "Git reference cannot be empty");
    ensure!(
        !trimmed.starts_with('-'),
        "Invalid Git reference: {trimmed}"
    );
    ensure!(
        !trimmed.chars().any(char::is_control),
        "Invalid characters in Git reference"
    );
    Ok(())
}

pub fn split_unified_patch(raw: &str) -> (Vec<Value>, usize, usize) {
    let mut files: Vec<ParsedFile> = Vec::new();
    let mut current: Option<ParsedFile> = None;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(prev) = current.take()
                && !prev.filename.is_empty()
            {
                files.push(prev);
            }
            let filename = parse_diff_git_header(rest);
            current = Some(ParsedFile {
                filename,
                status: "modified".into(),
                ..Default::default()
            });
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if line.starts_with("new file mode ") {
            file.status = "added".into();
            continue;
        }
        if line.starts_with("deleted file mode ") {
            file.status = "removed".into();
            continue;
        }
        if line.starts_with("rename from ") {
            file.status = "renamed".into();
            continue;
        }
        if let Some(to) = line.strip_prefix("rename to ") {
            file.status = "renamed".into();
            file.filename = unquote_git_path(to.trim());
            continue;
        }
        if let Some(plus) = line.strip_prefix("+++ ") {
            let clean = plus.trim();
            if clean != "/dev/null" {
                let name = unquote_git_path(clean);
                let name = name.strip_prefix("b/").unwrap_or(&name).to_owned();
                if !name.is_empty() {
                    file.filename = name;
                }
            }
            continue;
        }
        if let Some(minus) = line.strip_prefix("--- ") {
            let clean = minus.trim();
            if clean == "/dev/null" && file.status == "modified" {
                file.status = "added".into();
            } else if file.filename.is_empty() && clean != "/dev/null" {
                let name = unquote_git_path(clean);
                file.filename = name.strip_prefix("a/").unwrap_or(&name).to_owned();
            }
            continue;
        }
        if line.starts_with("index ")
            || line.starts_with("similarity index ")
            || line.starts_with("dissimilarity index ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
        {
            continue;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            if file.patch.is_empty() {
                file.patch.push_str(line);
            }
            continue;
        }
        if !file.patch.is_empty() || line.starts_with("@@") {
            if !file.patch.is_empty() {
                file.patch.push('\n');
            }
            file.patch.push_str(line);
            if line.starts_with('+') && !line.starts_with("+++") {
                file.additions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                file.deletions += 1;
            }
        }
    }
    if let Some(prev) = current.take()
        && !prev.filename.is_empty()
    {
        files.push(prev);
    }

    let mut total_add = 0usize;
    let mut total_del = 0usize;
    let out = files
        .into_iter()
        .map(|f| {
            total_add += f.additions;
            total_del += f.deletions;
            json!({
                "filename": f.filename,
                "status": f.status,
                "additions": f.additions,
                "deletions": f.deletions,
                "patch": f.patch,
            })
        })
        .collect();
    (out, total_add, total_del)
}

fn parse_diff_git_header(rest: &str) -> String {
    let trimmed = rest.trim();
    if let Some((_, right)) = trimmed.rsplit_once(" b/") {
        return unquote_git_path(right);
    }
    if let Some(part) = trimmed.split_whitespace().last() {
        let clean = unquote_git_path(part);
        return clean.strip_prefix("b/").unwrap_or(&clean).to_owned();
    }
    trimmed.to_owned()
}

fn unquote_git_path(path: &str) -> String {
    let s = path.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
            .replace("\\t", "\t")
            .replace("\\n", "\n")
    } else {
        s.to_owned()
    }
}

fn zero_separated_names(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

impl Ops {
    fn resolve_default_branch(&self, dir: &Path, project: &Project, current: &str) -> String {
        if let Some(t) = project
            .target_branch
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            return t.trim().to_owned();
        }
        if let Some(b) = project
            .workflow
            .workspace
            .base_branch
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            return b.trim().to_owned();
        }
        if let Ok(out) = self.git(
            dir,
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        ) && out.status.success()
        {
            let sym = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if let Some(short) = sym.strip_prefix("refs/remotes/origin/")
                && !short.is_empty()
            {
                return short.to_owned();
            }
        }
        for candidate in ["main", "master", "trunk", "develop"] {
            if self
                .git(
                    dir,
                    &[
                        "show-ref",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{candidate}"),
                    ],
                )
                .is_ok_and(|o| o.status.success())
            {
                return candidate.to_owned();
            }
        }
        if !current.is_empty() {
            current.to_owned()
        } else {
            "HEAD".into()
        }
    }

    pub fn git_refs(&self, project: &Project) -> Result<Value> {
        let dir = Path::new(&project.directory);
        ensure!(
            self.host.is_dir(dir),
            "Project directory is not available: {}",
            project.directory
        );
        let has_head = self
            .git(dir, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success();
        if !has_head {
            return Ok(json!({
                "current_branch": self.current_branch(&project.directory).unwrap_or_default(),
                "default_branch": project.target_branch.clone().unwrap_or_else(|| "main".into()),
                "head_sha": "",
                "dirty": false,
                "dirty_count": 0,
                "staged_count": 0,
                "branches": [],
                "tags": [],
                "commits": [],
            }));
        }
        let current_branch = self.current_branch(&project.directory).unwrap_or_default();
        let default_branch = self.resolve_default_branch(dir, project, &current_branch);
        let head_sha = self
            .checked(dir, &["rev-parse", "HEAD"])
            .unwrap_or_default();

        let status_out = self
            .git(
                dir,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            )?
            .stdout;
        let mut dirty_count = 0usize;
        let mut staged_count = 0usize;
        for entry in status_out.split(|b| *b == 0).filter(|e| e.len() >= 3) {
            dirty_count += 1;
            let idx = entry[0] as char;
            if idx != ' ' && idx != '?' && idx != '!' {
                staged_count += 1;
            }
        }

        let mut branches = Vec::new();
        if let Ok(raw) = self.checked(
            dir,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--count=45",
                "--format=%(refname:short)%09%(objectname)%09%(objectname:short)%09%(committerdate:relative)%09%(contents:subject)",
                "refs/heads",
            ],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                if parts.len() >= 5 {
                    let name = parts[0];
                    branches.push(json!({
                        "name": name,
                        "kind": "local",
                        "sha": parts[1],
                        "short_sha": parts[2],
                        "relative_time": parts[3],
                        "subject": parts[4],
                        "is_current": name == current_branch,
                        "is_default": name == default_branch,
                    }));
                }
            }
        }
        if let Ok(raw) = self.checked(
            dir,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--count=25",
                "--format=%(refname:short)%09%(objectname)%09%(objectname:short)%09%(committerdate:relative)%09%(contents:subject)",
                "refs/remotes",
            ],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                if parts.len() >= 5 {
                    let name = parts[0];
                    if name.ends_with("/HEAD") || name == "origin" {
                        continue;
                    }
                    branches.push(json!({
                        "name": name,
                        "kind": "remote",
                        "sha": parts[1],
                        "short_sha": parts[2],
                        "relative_time": parts[3],
                        "subject": parts[4],
                        "is_current": false,
                        "is_default": false,
                    }));
                }
            }
        }

        let mut tags = Vec::new();
        if let Ok(raw) = self.checked(
            dir,
            &[
                "for-each-ref",
                "--sort=-creatordate",
                "--count=25",
                "--format=%(refname:short)%09%(objectname:short)%09%(creatordate:relative)%09%(contents:subject)",
                "refs/tags",
            ],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(4, '\t').collect();
                if parts.len() >= 4 {
                    tags.push(json!({
                        "name": parts[0],
                        "short_sha": parts[1],
                        "relative_time": parts[2],
                        "subject": parts[3],
                    }));
                }
            }
        }

        let mut commits = Vec::new();
        if let Ok(raw) = self.checked(
            dir,
            &["log", "-n", "40", "--format=%H%x09%h%x09%cr%x09%an%x09%s"],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                if parts.len() >= 5 {
                    commits.push(json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "relative_time": parts[2],
                        "author": parts[3],
                        "subject": parts[4],
                    }));
                }
            }
        }

        Ok(json!({
            "current_branch": current_branch,
            "default_branch": default_branch,
            "head_sha": head_sha,
            "dirty": dirty_count > 0,
            "dirty_count": dirty_count,
            "staged_count": staged_count,
            "branches": branches,
            "tags": tags,
            "commits": commits,
        }))
    }

    pub fn git_compare(
        &self,
        project: &Project,
        base: &str,
        compare: &str,
        cwd_override: Option<&str>,
        three_dot: bool,
    ) -> Result<Value> {
        let dir = Path::new(cwd_override.unwrap_or(&project.directory));
        ensure!(
            self.host.is_dir(dir),
            "Comparison directory does not exist: {}",
            dir.display()
        );
        validate_ref_arg(base)?;
        validate_ref_arg(compare)?;

        let base_trimmed = base.trim();
        let compare_trimmed = compare.trim();

        // Resolve base commit (supporting EMPTY_TREE or `sha~1` on root commits gracefully).
        let base_sha = if base_trimmed == EMPTY_TREE {
            EMPTY_TREE.to_owned()
        } else if let Some(stem) = base_trimmed
            .strip_suffix("~1")
            .or_else(|| base_trimmed.strip_suffix('^'))
        {
            validate_ref_arg(stem)?;
            let parent_spec = format!("{stem}^");
            if let Ok(out) = self.git(
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("{parent_spec}^{{commit}}"),
                ],
            ) && out.status.success()
            {
                String::from_utf8_lossy(&out.stdout).trim().to_owned()
            } else {
                // Verify stem is at least a valid commit (root commit case).
                self.checked(
                    dir,
                    &["rev-parse", "--verify", &format!("{stem}^{{commit}}")],
                )?;
                EMPTY_TREE.to_owned()
            }
        } else {
            self.checked(
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("{base_trimmed}^{{commit}}"),
                ],
            )
            .with_context(|| format!("Unknown base reference '{base_trimmed}'"))?
        };

        let mut raw_patch = Vec::<u8>::new();
        let mut commits = Vec::<Value>::new();
        let compare_sha: String;
        let effective_base_sha: String;

        if compare_trimmed == "WORKDIR" {
            let head_sha = self
                .checked(dir, &["rev-parse", "--verify", "HEAD^{commit}"])
                .unwrap_or_else(|_| base_sha.clone());
            compare_sha = head_sha.clone();
            effective_base_sha = if three_dot && base_sha != EMPTY_TREE && head_sha != base_sha {
                self.checked(dir, &["merge-base", &base_sha, &head_sha])
                    .unwrap_or_else(|_| base_sha.clone())
            } else {
                base_sha.clone()
            };

            let diff_out = self.git(
                dir,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-M",
                    &effective_base_sha,
                    "--",
                ],
            )?;
            ensure!(
                diff_out.status.success(),
                "git diff failed: {}",
                String::from_utf8_lossy(&diff_out.stderr).trim()
            );
            raw_patch.extend_from_slice(&diff_out.stdout);

            // Include untracked files so Working tree matches `review_preview`.
            let untracked_out =
                self.git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
            if untracked_out.status.success() {
                for rel in zero_separated_names(&untracked_out.stdout) {
                    if raw_patch.len() > MAX_PATCH_BYTES {
                        break;
                    }
                    let out = self.git(
                        dir,
                        &[
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--no-index",
                            "--",
                            "/dev/null",
                            &rel,
                        ],
                    )?;
                    if out.status.code() == Some(1) || out.status.success() {
                        raw_patch.extend_from_slice(&out.stdout);
                    }
                }
            }
            if effective_base_sha != EMPTY_TREE && effective_base_sha != head_sha {
                commits = self.log_between(dir, &effective_base_sha, &head_sha);
            }
        } else if compare_trimmed == "STAGED" {
            compare_sha = self
                .checked(dir, &["rev-parse", "--verify", "HEAD^{commit}"])
                .unwrap_or_else(|_| base_sha.clone());
            effective_base_sha = base_sha.clone();
            let diff_out = self.git(
                dir,
                &[
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-M",
                    &effective_base_sha,
                    "--",
                ],
            )?;
            ensure!(
                diff_out.status.success(),
                "git diff --cached failed: {}",
                String::from_utf8_lossy(&diff_out.stderr).trim()
            );
            raw_patch.extend_from_slice(&diff_out.stdout);
        } else {
            compare_sha = self
                .checked(
                    dir,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("{compare_trimmed}^{{commit}}"),
                    ],
                )
                .with_context(|| format!("Unknown compare reference '{compare_trimmed}'"))?;
            effective_base_sha = if three_dot && base_sha != EMPTY_TREE && base_sha != compare_sha {
                self.checked(dir, &["merge-base", &base_sha, &compare_sha])
                    .unwrap_or_else(|_| base_sha.clone())
            } else {
                base_sha.clone()
            };
            let diff_out = self.git(
                dir,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-M",
                    &effective_base_sha,
                    &compare_sha,
                    "--",
                ],
            )?;
            ensure!(
                diff_out.status.success(),
                "git diff failed: {}",
                String::from_utf8_lossy(&diff_out.stderr).trim()
            );
            raw_patch.extend_from_slice(&diff_out.stdout);
            if effective_base_sha != EMPTY_TREE && effective_base_sha != compare_sha {
                commits = self.log_between(dir, &effective_base_sha, &compare_sha);
            } else if effective_base_sha == EMPTY_TREE {
                commits = self.log_single(dir, &compare_sha);
            }
        }

        let truncated = raw_patch.len() > MAX_PATCH_BYTES;
        let capped = String::from_utf8_lossy(&raw_patch[..raw_patch.len().min(MAX_PATCH_BYTES)]);
        let (files, total_additions, total_deletions) = split_unified_patch(&capped);

        Ok(json!({
            "base_ref": base_trimmed,
            "compare_ref": compare_trimmed,
            "base_sha": effective_base_sha,
            "compare_sha": compare_sha,
            "three_dot": three_dot,
            "files": files,
            "commits": commits,
            "total_additions": total_additions,
            "total_deletions": total_deletions,
            "truncated": truncated,
            "cwd": dir.to_string_lossy(),
        }))
    }

    fn log_between(&self, dir: &Path, from: &str, to: &str) -> Vec<Value> {
        let range = format!("{from}..{to}");
        let Ok(raw) = self.checked(
            dir,
            &[
                "log",
                "-n",
                "50",
                "--format=%H%x09%h%x09%cr%x09%an%x09%s",
                &range,
            ],
        ) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                (parts.len() >= 5).then(|| {
                    json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "relative_time": parts[2],
                        "author": parts[3],
                        "subject": parts[4],
                    })
                })
            })
            .collect()
    }

    fn log_single(&self, dir: &Path, sha: &str) -> Vec<Value> {
        let Ok(raw) = self.checked(
            dir,
            &[
                "log",
                "-n",
                "1",
                "--format=%H%x09%h%x09%cr%x09%an%x09%s",
                sha,
            ],
        ) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                (parts.len() >= 5).then(|| {
                    json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "relative_time": parts[2],
                        "author": parts[3],
                        "subject": parts[4],
                    })
                })
            })
            .collect()
    }

    /// Seeds `refs/canban/cards/<card_id>` from `refs/pull/<pr_number>/head` on the configured remote
    /// so `Ops::prepare` checks out the exact PR branch head when creating a local Card worktree.
    pub fn fetch_pr_for_card(
        &self,
        project: &Project,
        pr_number: u64,
        card_id: &str,
    ) -> Result<String> {
        ensure!(pr_number > 0, "Invalid PR number");
        validate_ref_arg(card_id)?;
        let source = Path::new(&project.directory);
        let remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.trim()
        };
        validate_ref_arg(remote)?;
        let keep = format!("refs/canban/cards/{card_id}");
        let refspec = format!("+refs/pull/{pr_number}/head:{keep}");
        self.checked(source, &["fetch", "--", remote, &refspec])?;
        self.checked(source, &["rev-parse", "--verify", &keep])
    }

    /// Creates an isolated worktree under `<home>/worktrees/review/<slug>` for ad-hoc review Agent runs.
    pub fn prepare_review_worktree(
        &self,
        home: &Path,
        project: &Project,
        target_ref: &str,
        run_id: &str,
    ) -> Result<PathBuf> {
        validate_ref_arg(run_id)?;
        let source = Path::new(&project.directory);
        let resolved = if matches!(target_ref.trim(), "" | "WORKDIR" | "STAGED") {
            "HEAD".to_owned()
        } else {
            validate_ref_arg(target_ref)?;
            target_ref.trim().to_owned()
        };
        let sha = self.checked(
            source,
            &["rev-parse", "--verify", &format!("{resolved}^{{commit}}")],
        )?;
        let dest = home.join("worktrees").join("review").join(format!(
            "{}-{run_id}",
            &project.id[..project.id.len().min(8)]
        ));
        if let Some(parent) = dest.parent() {
            self.host.mkdir(parent)?;
        }
        self.checked(
            source,
            &[
                "worktree",
                "add",
                "--detach",
                dest.to_str().context("Invalid review worktree path")?,
                &sha,
            ],
        )?;
        Ok(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) {
        let out = Ops::local().checked(dir, args).unwrap();
        let _ = out;
    }

    fn make_repo() -> (tempfile::TempDir, Project) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Reviewer"),
            ("user.email", "reviewer@example.invalid"),
            ("core.hooksPath", "/dev/null"),
            ("commit.gpgsign", "false"),
        ] {
            git(&repo, &["config", k, v]);
        }
        fs::write(repo.join("README.md"), "# Hello\nline 2\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "initial commit"]);

        let project = Project {
            github: None,
            delivery_mode: Default::default(),
            id: "proj1".into(),
            name: "Demo".into(),
            location: None,
            directory: repo.to_string_lossy().into_owned(),
            workflow: Default::default(),
            target_branch: Some("main".into()),
            created_at: 0,
        };
        (tmp, project)
    }

    #[test]
    fn lists_refs_and_compares_branches_commits_and_working_tree() {
        let (tmp, project) = make_repo();
        let repo = Path::new(&project.directory);
        let root_sha = Ops::local().checked(repo, &["rev-parse", "HEAD"]).unwrap();

        // Root commit diff (`sha~1` -> `sha`) works cleanly via empty tree fallback.
        let root_cmp = Ops::local()
            .git_compare(&project, &format!("{root_sha}~1"), &root_sha, None, false)
            .unwrap();
        assert_eq!(root_cmp["files"].as_array().unwrap().len(), 1);
        assert_eq!(root_cmp["files"][0]["filename"], "README.md");
        assert_eq!(root_cmp["files"][0]["status"], "added");

        // Feature branch with modification + new file.
        git(repo, &["switch", "-c", "feat/review-ui"]);
        fs::write(repo.join("README.md"), "# Hello\nline 2 updated\nline 3\n").unwrap();
        fs::write(repo.join("src_view.rs"), "pub fn render() {}\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "feat: add review view"]);

        // Add untracked file to test WORKDIR diff.
        fs::write(repo.join("notes.txt"), "draft note\n").unwrap();

        let refs = Ops::local().git_refs(&project).unwrap();
        assert_eq!(refs["current_branch"], "feat/review-ui");
        assert_eq!(refs["default_branch"], "main");
        assert_eq!(refs["dirty"], true);
        assert!(
            refs["branches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == "feat/review-ui" && b["is_current"] == true)
        );
        assert!(
            refs["branches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == "main" && b["is_default"] == true)
        );

        let branch_cmp = Ops::local()
            .git_compare(&project, "main", "feat/review-ui", None, true)
            .unwrap();
        assert_eq!(branch_cmp["files"].as_array().unwrap().len(), 2);
        assert_eq!(branch_cmp["commits"].as_array().unwrap().len(), 1);
        assert_eq!(branch_cmp["total_additions"], 3);
        assert_eq!(branch_cmp["total_deletions"], 1);

        let workdir_cmp = Ops::local()
            .git_compare(&project, "HEAD", "WORKDIR", None, false)
            .unwrap();
        assert_eq!(workdir_cmp["files"].as_array().unwrap().len(), 1);
        assert_eq!(workdir_cmp["files"][0]["filename"], "notes.txt");
        assert_eq!(workdir_cmp["files"][0]["status"], "added");

        // Detached review worktree creation.
        let home = tmp.path().join("home");
        let wt = Ops::local()
            .prepare_review_worktree(&home, &project, "feat/review-ui", "r1")
            .unwrap();
        assert!(wt.join("src_view.rs").exists());
    }
}
