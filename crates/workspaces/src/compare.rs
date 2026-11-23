//! General Git reference discovery and arbitrary comparison for the Git workbench.
use super::Ops;
use anyhow::{Context, Result, ensure};
use canban_types::Project;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct ParsedFile {
    filename: String,
    status: String,
    additions: usize,
    deletions: usize,
    patch: String,
}

pub(crate) fn validate_ref_arg(value: &str) -> Result<()> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrRemoteSource {
    /// Fetch from a configured local Git remote (e.g. `origin` or `upstream`).
    Named(String),
    /// Fetch directly from candidate repository URLs when no local remote points
    /// to the PR's repository (e.g. `origin` is a fork without an `upstream` remote).
    Urls(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCheckoutSpec {
    pub pr_number: u64,
    pub head_ref: Option<String>,
    pub same_repo: bool,
    pub remote: Option<PrRemoteSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrTrackingTarget {
    pub remote: String,
    pub merge_ref: String,
    pub fetch_refspec: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
}

pub(crate) fn parse_worktree_list_porcelain_z(raw: &str) -> Vec<GitWorktree> {
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut bare = false;
    let mut prunable = false;

    let flush = |out: &mut Vec<GitWorktree>,
                 path: &mut Option<String>,
                 branch: &mut Option<String>,
                 bare: &mut bool,
                 prunable: &mut bool| {
        if let Some(p) = path.take()
            && !p.is_empty()
            && !*bare
            && !*prunable
        {
            out.push(GitWorktree {
                path: p,
                branch: branch.take(),
            });
        } else {
            *branch = None;
        }
        *bare = false;
        *prunable = false;
    };

    for field in raw.split('\0') {
        if field.is_empty() {
            flush(&mut out, &mut path, &mut branch, &mut bare, &mut prunable);
            continue;
        }
        if let Some(p) = field.strip_prefix("worktree ") {
            path = Some(p.to_owned());
        } else if let Some(b) = field.strip_prefix("branch ") {
            branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
        } else if field == "bare" {
            bare = true;
        } else if field == "prunable" || field.starts_with("prunable ") {
            prunable = true;
        }
    }
    flush(&mut out, &mut path, &mut branch, &mut bare, &mut prunable);
    out
}

impl Ops {
    /// Lists non-bare, non-prunable Git worktrees registered for `source`.
    pub fn list_git_worktrees(&self, source: &Path) -> Vec<GitWorktree> {
        let Ok(raw) = self.checked(source, &["worktree", "list", "--porcelain", "-z"]) else {
            return Vec::new();
        };
        parse_worktree_list_porcelain_z(&raw)
    }

    fn resolve_default_branch(
        &self,
        dir: &Path,
        project: &Project,
        current: &str,
        preferred_remotes: &[&str],
        github_default_branch: Option<&str>,
    ) -> String {
        if let Some(detected) = self.detect_repo_default_branch(dir, preferred_remotes) {
            return detected;
        }
        if let Some(gh) = github_default_branch
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return gh.to_owned();
        }
        if let Some(t) = project
            .target_branch
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return t.to_owned();
        }
        if let Some(b) = project
            .workflow
            .workspace
            .base_branch
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return b.to_owned();
        }
        if !current.is_empty() {
            current.to_owned()
        } else {
            "HEAD".into()
        }
    }

    pub fn git_refs(&self, project: &Project) -> Result<Value> {
        self.git_refs_in_with_context(project, None, None, None)
    }

    pub fn git_refs_in(&self, project: &Project, cwd_override: Option<&str>) -> Result<Value> {
        self.git_refs_in_with_context(project, cwd_override, None, None)
    }

    pub fn git_refs_in_with_context(
        &self,
        project: &Project,
        cwd_override: Option<&str>,
        preferred_remote: Option<&str>,
        github_default_branch: Option<&str>,
    ) -> Result<Value> {
        let dir_str = cwd_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&project.directory);
        let dir = Path::new(dir_str);
        ensure!(
            self.host.is_dir(dir),
            "Repository directory is not available: {}",
            dir.display()
        );
        let remote_name = if project.workflow.workspace.remote.trim().is_empty()
            || project.workflow.workspace.remote.trim().starts_with('-')
        {
            "origin".to_owned()
        } else {
            project.workflow.workspace.remote.trim().to_owned()
        };
        let has_remote = self.has_remote(dir, &remote_name);
        let sync_remote = preferred_remote
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('-'))
            .unwrap_or(&remote_name)
            .to_owned();
        let has_sync_remote = self.has_remote(dir, &sync_remote);
        let preferred_remotes = [sync_remote.as_str(), remote_name.as_str()];

        let has_head = self
            .git(dir, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success();
        let current_branch = self.current_branch(dir_str).unwrap_or_default();
        if !has_head {
            let status_files = self.status_files(dir).unwrap_or_default();
            let staged_count = status_files.iter().filter(|f| f["staged"] == true).count();
            let unstaged_count = status_files
                .iter()
                .filter(|f| f["unstaged"] == true || f["untracked"] == true)
                .count();
            let default_branch = self
                .detect_repo_default_branch(dir, &preferred_remotes)
                .or_else(|| {
                    github_default_branch
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    project
                        .target_branch
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| (!current_branch.is_empty()).then(|| current_branch.clone()))
                .unwrap_or_else(|| "main".into());
            return Ok(json!({
                "cwd": dir_str,
                "current_branch": current_branch,
                "default_branch": default_branch,
                "head_sha": "",
                "upstream": "",
                "ahead": 0,
                "behind": 0,
                "has_remote": has_remote,
                "remote_name": remote_name,
                "sync_remote": sync_remote,
                "has_sync_remote": has_sync_remote,
                "dirty": !status_files.is_empty(),
                "dirty_count": status_files.len(),
                "staged_count": staged_count,
                "unstaged_count": unstaged_count,
                "status_files": status_files,
                "last_commit_subject": "",
                "last_commit_body": "",
                "branches": [],
                "tags": [],
                "commits": [],
            }));
        }

        let default_branch = self.resolve_default_branch(
            dir,
            project,
            &current_branch,
            &preferred_remotes,
            github_default_branch,
        );
        let head_sha = self
            .checked(dir, &["rev-parse", "HEAD"])
            .unwrap_or_default();

        let status_files = self.status_files(dir).unwrap_or_default();
        let dirty_count = status_files.len();
        let staged_count = status_files.iter().filter(|f| f["staged"] == true).count();
        let unstaged_count = status_files
            .iter()
            .filter(|f| f["unstaged"] == true || f["untracked"] == true)
            .count();

        let upstream = if !current_branch.is_empty() {
            self.checked(
                dir,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )
            .unwrap_or_default()
        } else {
            String::new()
        };
        let (ahead, behind) = if !upstream.is_empty() {
            self.checked(
                dir,
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{upstream}...HEAD"),
                ],
            )
            .ok()
            .and_then(|s| {
                let mut it = s.split_whitespace();
                let beh = it.next()?.parse::<u64>().ok()?;
                let ahd = it.next()?.parse::<u64>().ok()?;
                Some((ahd, beh))
            })
            .unwrap_or((0, 0))
        } else {
            (0, 0)
        };

        let (last_commit_subject, last_commit_body) = self
            .checked(dir, &["log", "-n", "1", "--format=%s%x00%b", "HEAD"])
            .ok()
            .and_then(|raw| {
                let (subj, body) = raw.split_once('\0')?;
                Some((subj.trim().to_owned(), body.trim().to_owned()))
            })
            .unwrap_or_default();

        let mut branches = Vec::new();
        if let Ok(raw) = self.checked(
            dir,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--count=100",
                "--format=%(refname:short)%09%(objectname)%09%(objectname:short)%09%(committerdate:relative)%09%(upstream:short)%09%(upstream:track)%09%(contents:subject)",
                "refs/heads",
            ],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(7, '\t').collect();
                if parts.len() >= 7 {
                    let name = parts[0];
                    let (br_ahead, br_behind, br_gone) =
                        super::git_client::parse_upstream_track(parts[5]);
                    branches.push(json!({
                        "name": name,
                        "kind": "local",
                        "sha": parts[1],
                        "short_sha": parts[2],
                        "relative_time": parts[3],
                        "upstream": parts[4],
                        "ahead": br_ahead,
                        "behind": br_behind,
                        "gone": br_gone,
                        "subject": parts[6],
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
                "--count=100",
                "--format=%(refname:short)%09%(objectname)%09%(objectname:short)%09%(committerdate:relative)%09%(contents:subject)",
                "refs/remotes",
            ],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                if parts.len() >= 5 {
                    let name = parts[0];
                    if name.ends_with("/HEAD") || !name.contains('/') {
                        continue;
                    }
                    branches.push(json!({
                        "name": name,
                        "kind": "remote",
                        "sha": parts[1],
                        "short_sha": parts[2],
                        "relative_time": parts[3],
                        "upstream": "",
                        "ahead": 0,
                        "behind": 0,
                        "gone": false,
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
            &[
                "log",
                "-n",
                "40",
                "--format=%H%x09%h%x09%P%x09%cr%x09%an%x09%s",
            ],
        ) {
            for line in raw.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.splitn(6, '\t').collect();
                if parts.len() >= 6 {
                    let parents: Vec<&str> = parts[2].split_whitespace().collect();
                    commits.push(json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "parents": parents,
                        "relative_time": parts[3],
                        "author": parts[4],
                        "subject": parts[5],
                    }));
                }
            }
        }

        Ok(json!({
            "cwd": dir_str,
            "current_branch": current_branch,
            "default_branch": default_branch,
            "head_sha": head_sha,
            "upstream": upstream,
            "ahead": ahead,
            "behind": behind,
            "has_remote": has_remote,
            "remote_name": remote_name,
            "sync_remote": sync_remote,
            "has_sync_remote": has_sync_remote,
            "dirty": dirty_count > 0,
            "dirty_count": dirty_count,
            "staged_count": staged_count,
            "unstaged_count": unstaged_count,
            "status_files": status_files,
            "last_commit_subject": last_commit_subject,
            "last_commit_body": last_commit_body,
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

        // Keep index and working-directory patches separate. A net HEAD -> working
        // tree diff can hide staged edits that were subsequently undone on disk.
        if compare_trimmed == "UNCOMMITTED" {
            let head = if self
                .git(dir, &["rev-parse", "--verify", "HEAD"])?
                .status
                .success()
            {
                "HEAD"
            } else {
                EMPTY_TREE
            };
            let mut staged = self.git_compare(project, head, "STAGED", cwd_override, false)?;
            let unstaged = self.git_compare(project, head, "UNSTAGED", cwd_override, false)?;
            let mut files = Vec::new();
            for (section, data) in [("staged", &staged), ("unstaged", &unstaged)] {
                for file in data["files"].as_array().into_iter().flatten() {
                    let mut file = file.clone();
                    file["section"] = json!(section);
                    files.push(file);
                }
            }
            for field in ["total_additions", "total_deletions"] {
                staged[field] = json!(
                    staged[field].as_u64().unwrap_or(0) + unstaged[field].as_u64().unwrap_or(0)
                );
            }
            staged["files"] = json!(files);
            staged["compare_ref"] = json!("UNCOMMITTED");
            staged["truncated"] =
                json!(staged["truncated"] == true || unstaged["truncated"] == true);
            return Ok(staged);
        }

        // Resolve base commit (supporting EMPTY_TREE, `sha~1`/`sha^`/`sha^1` on root commits,
        // or `sha^N` for merge commit parent selection).
        let (base_sha, single_parent_idx) = if base_trimmed == EMPTY_TREE {
            (EMPTY_TREE.to_owned(), Some(0usize))
        } else if let Some(stem) = base_trimmed
            .strip_suffix("~1")
            .or_else(|| base_trimmed.strip_suffix("^1"))
            .or_else(|| base_trimmed.strip_suffix('^'))
        {
            validate_ref_arg(stem)?;
            let parent_spec = format!("{stem}^1");
            let resolved = if let Ok(out) = self.git(
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
            };
            let is_single = stem == compare_trimmed;
            (resolved, is_single.then_some(0usize))
        } else if let Some((stem, idx_str)) = base_trimmed.rsplit_once('^')
            && !stem.is_empty()
            && let Ok(p_num) = idx_str.parse::<usize>()
            && p_num >= 2
        {
            validate_ref_arg(stem)?;
            let resolved = self
                .checked(
                    dir,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("{stem}^{p_num}^{{commit}}"),
                    ],
                )
                .with_context(|| format!("Unknown parent reference '{base_trimmed}'"))?;
            let is_single = stem == compare_trimmed;
            (resolved, is_single.then_some(p_num - 1))
        } else {
            let resolved = self
                .checked(
                    dir,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("{base_trimmed}^{{commit}}"),
                    ],
                )
                .with_context(|| format!("Unknown base reference '{base_trimmed}'"))?;
            (resolved, None)
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
        } else if compare_trimmed == "UNSTAGED" {
            compare_sha = self
                .checked(dir, &["rev-parse", "--verify", "HEAD^{commit}"])
                .unwrap_or_else(|_| base_sha.clone());
            effective_base_sha = base_sha.clone();
            let diff_out =
                self.git(dir, &["diff", "--no-ext-diff", "--no-textconv", "-M", "--"])?;
            ensure!(
                diff_out.status.success(),
                "git diff failed: {}",
                String::from_utf8_lossy(&diff_out.stderr).trim()
            );
            raw_patch.extend_from_slice(&diff_out.stdout);

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

        let is_workdir_mode = matches!(compare_trimmed, "WORKDIR" | "STAGED" | "UNSTAGED");
        let status_files = is_workdir_mode.then(|| self.status_files(dir).unwrap_or_default());
        let commit_detail = if !is_workdir_mode && let Some(p_idx) = single_parent_idx {
            self.commit_metadata(dir, &compare_sha)
                .ok()
                .map(|mut detail| {
                    detail["parent_index"] = json!(p_idx);
                    detail
                })
        } else {
            None
        };

        let mut resp = json!({
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
        });
        if let Some(sf) = status_files {
            resp["status_files"] = json!(sf);
        }
        if let Some(cd) = commit_detail {
            resp["commit_detail"] = cd;
        }
        Ok(resp)
    }

    fn log_between(&self, dir: &Path, from: &str, to: &str) -> Vec<Value> {
        let range = format!("{from}..{to}");
        let Ok(raw) = self.checked(
            dir,
            &[
                "log",
                "-n",
                "50",
                "--format=%H%x09%h%x09%P%x09%cr%x09%an%x09%s",
                &range,
            ],
        ) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(6, '\t').collect();
                (parts.len() >= 6).then(|| {
                    let parents: Vec<&str> = parts[2].split_whitespace().collect();
                    json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "parents": parents,
                        "relative_time": parts[3],
                        "author": parts[4],
                        "subject": parts[5],
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
                "--format=%H%x09%h%x09%P%x09%cr%x09%an%x09%s",
                sha,
            ],
        ) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(6, '\t').collect();
                (parts.len() >= 6).then(|| {
                    let parents: Vec<&str> = parts[2].split_whitespace().collect();
                    json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "parents": parents,
                        "relative_time": parts[3],
                        "author": parts[4],
                        "subject": parts[5],
                    })
                })
            })
            .collect()
    }

    /// Seeds `refs/canban/cards/<card_id>` from either the PR's same-repo branch or
    /// `refs/pull/<pr_number>/head` (for fork PRs / fallback) and returns the upstream
    /// tracking target for the Card branch.
    pub fn seed_pr_for_card(
        &self,
        project: &Project,
        card_id: &str,
        spec: &PrCheckoutSpec,
    ) -> Result<Option<PrTrackingTarget>> {
        ensure!(spec.pr_number > 0, "Invalid PR number");
        validate_ref_arg(card_id)?;
        let source = Path::new(&project.directory);
        let keep = format!("refs/canban/cards/{card_id}");
        if let Some(PrRemoteSource::Urls(urls)) = &spec.remote {
            self.fetch_pr_from_urls(source, urls, spec.pr_number, &keep)?;
            return Ok(None);
        }
        let default_remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.trim()
        };
        let remote = match &spec.remote {
            Some(PrRemoteSource::Named(name)) if !name.trim().is_empty() => name.trim(),
            _ => default_remote,
        };
        validate_ref_arg(remote)?;
        if !self
            .git(source, &["remote", "get-url", remote])?
            .status
            .success()
        {
            return Ok(None);
        }
        if spec.same_repo
            && let Some(head_ref) = spec
                .head_ref
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            && self
                .git(source, &["check-ref-format", "--branch", head_ref])?
                .status
                .success()
        {
            let remote_ref = format!("refs/remotes/{remote}/{head_ref}");
            let fetch_spec = format!("+refs/heads/{head_ref}:{remote_ref}");
            if self
                .git(source, &["fetch", "--", remote, &fetch_spec])?
                .status
                .success()
                && let Ok(sha) = self.checked(
                    source,
                    &["rev-parse", "--verify", &format!("{remote_ref}^{{commit}}")],
                )
            {
                self.checked(source, &["update-ref", &keep, &sha])?;
                return Ok(Some(PrTrackingTarget {
                    remote: remote.to_owned(),
                    merge_ref: format!("refs/heads/{head_ref}"),
                    fetch_refspec: None,
                }));
            }
        }
        let pr_remote_ref = format!("refs/remotes/{remote}/pr/{}", spec.pr_number);
        let fetch_spec = format!("+refs/pull/{}/head:{pr_remote_ref}", spec.pr_number);
        self.checked(source, &["fetch", "--", remote, &fetch_spec])?;
        let sha = self.checked(
            source,
            &[
                "rev-parse",
                "--verify",
                &format!("{pr_remote_ref}^{{commit}}"),
            ],
        )?;
        self.checked(source, &["update-ref", &keep, &sha])?;
        Ok(Some(PrTrackingTarget {
            remote: remote.to_owned(),
            merge_ref: format!("refs/pull/{}/head", spec.pr_number),
            fetch_refspec: Some(fetch_spec),
        }))
    }

    fn fetch_pr_from_urls(
        &self,
        source: &Path,
        urls: &[String],
        pr_number: u64,
        keep: &str,
    ) -> Result<String> {
        ensure!(
            !urls.is_empty(),
            "No Git remote URLs available for PR fetch"
        );
        let refspec = format!("+refs/pull/{pr_number}/head:{keep}");
        let mut last_err = None;
        for raw_url in urls {
            let url = raw_url.trim();
            if url.is_empty() {
                continue;
            }
            validate_ref_arg(url)?;
            match self.checked(source, &["fetch", "--", url, &refspec]) {
                Ok(_) => return self.checked(source, &["rev-parse", "--verify", keep]),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("No valid Git remote URLs for PR fetch")))
    }

    /// Configures Git upstream tracking on a Card branch for its PR source when not already set.
    pub fn configure_pr_tracking(
        &self,
        dir: &Path,
        branch: &str,
        target: &PrTrackingTarget,
    ) -> Result<()> {
        validate_ref_arg(branch)?;
        validate_ref_arg(&target.remote)?;
        validate_ref_arg(&target.merge_ref)?;
        let remote_key = format!("branch.{branch}.remote");
        if self
            .git(dir, &["config", "--get", &remote_key])?
            .status
            .success()
        {
            return Ok(());
        }
        if let Some(refspec) = &target.fetch_refspec {
            let fetch_key = format!("remote.{}.fetch", target.remote);
            let existing = self
                .checked(dir, &["config", "--get-all", &fetch_key])
                .unwrap_or_default();
            if !existing.lines().any(|l| l.trim() == refspec) {
                self.checked(dir, &["config", "--add", &fetch_key, refspec])?;
            }
        }
        self.checked(dir, &["config", &remote_key, &target.remote])?;
        self.checked(
            dir,
            &[
                "config",
                &format!("branch.{branch}.merge"),
                &target.merge_ref,
            ],
        )?;
        Ok(())
    }

    /// Seeds `refs/canban/cards/<card_id>` from `refs/pull/<pr_number>/head` on the configured
    /// or resolved PR remote so `Ops::prepare` checks out the exact PR branch head when creating
    /// a local Card worktree.
    pub fn fetch_pr_for_card(
        &self,
        project: &Project,
        pr_number: u64,
        card_id: &str,
        remote_override: Option<&PrRemoteSource>,
    ) -> Result<String> {
        ensure!(pr_number > 0, "Invalid PR number");
        validate_ref_arg(card_id)?;
        let source = Path::new(&project.directory);
        let keep = format!("refs/canban/cards/{card_id}");
        if let Some(PrRemoteSource::Urls(urls)) = remote_override {
            return self.fetch_pr_from_urls(source, urls, pr_number, &keep);
        }
        let default_remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.trim()
        };
        let remote = match remote_override {
            Some(PrRemoteSource::Named(name)) if !name.trim().is_empty() => name.trim(),
            _ => default_remote,
        };
        validate_ref_arg(remote)?;
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
        let resolved = if matches!(target_ref.trim(), "" | "WORKDIR" | "STAGED" | "UNSTAGED") {
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

    /// Reads text lines from a file in the checkout (`workdir`, `index`, or `rev`) for diff context expansion.
    pub fn git_file_lines(&self, req: FileLinesRequest<'_>) -> Result<Value> {
        let FileLinesRequest {
            project,
            cwd_override,
            path,
            source,
            rev,
            start_line,
            end_line,
        } = req;
        let rel = path.trim();
        ensure!(!rel.is_empty(), "File path cannot be empty");
        ensure!(
            !rel.starts_with('/') && !rel.starts_with('-') && !rel.contains('\0'),
            "Invalid relative file path: {rel}"
        );
        let rel_path = Path::new(rel);
        ensure!(!rel_path.is_absolute(), "File path must be relative");
        for comp in rel_path.components() {
            ensure!(
                !matches!(
                    comp,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                ),
                "Invalid path component in '{rel}'"
            );
        }

        let dir = Path::new(cwd_override.unwrap_or(&project.directory));
        ensure!(
            self.host.is_dir(dir),
            "Comparison directory does not exist: {}",
            dir.display()
        );

        const MAX_CONTEXT_FILE_BYTES: usize = 4 * 1024 * 1024;

        let bytes = match source {
            "workdir" => {
                let target = dir.join(rel_path);
                let canon_dir = dir
                    .canonicalize()
                    .with_context(|| format!("Invalid directory {}", dir.display()))?;
                let canon_file = target
                    .canonicalize()
                    .with_context(|| format!("File not found: {rel}"))?;
                ensure!(
                    canon_file.starts_with(&canon_dir),
                    "File path escapes checkout directory"
                );
                let meta = std::fs::metadata(&canon_file)?;
                ensure!(
                    meta.len() <= MAX_CONTEXT_FILE_BYTES as u64,
                    "File exceeds 4 MiB context limit"
                );
                std::fs::read(&canon_file)?
            }
            "index" => {
                let spec = format!(":{rel}");
                let out = self.git(dir, &["show", &spec])?;
                ensure!(
                    out.status.success(),
                    "git show failed for index file '{rel}': {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                ensure!(
                    out.stdout.len() <= MAX_CONTEXT_FILE_BYTES,
                    "File exceeds 4 MiB context limit"
                );
                out.stdout
            }
            "rev" => {
                let r = rev
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .context("Missing Git revision")?;
                validate_ref_arg(r)?;
                let spec = format!("{r}:{rel}");
                let out = self.git(dir, &["show", &spec])?;
                ensure!(
                    out.status.success(),
                    "git show failed for '{spec}': {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                ensure!(
                    out.stdout.len() <= MAX_CONTEXT_FILE_BYTES,
                    "File exceeds 4 MiB context limit"
                );
                out.stdout
            }
            other => anyhow::bail!("Unsupported file source: {other}"),
        };

        slice_context_file_bytes(rel, &bytes, start_line, end_line)
    }
}

/// Request parameters for [`Ops::git_file_lines`].
pub struct FileLinesRequest<'a> {
    pub project: &'a Project,
    pub cwd_override: Option<&'a str>,
    pub path: &'a str,
    pub source: &'a str,
    pub rev: Option<&'a str>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

/// Validates size/binary safety on raw file bytes and returns 1-based line slices for diff context expansion.
pub fn slice_context_file_bytes(
    path: &str,
    bytes: &[u8],
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<Value> {
    const MAX_CONTEXT_FILE_BYTES: usize = 4 * 1024 * 1024;
    const BINARY_SNIFF_LEN: usize = 8192;
    ensure!(
        bytes.len() <= MAX_CONTEXT_FILE_BYTES,
        "File exceeds 4 MiB context limit"
    );
    let sniff = bytes.len().min(BINARY_SNIFF_LEN);
    ensure!(
        !bytes[..sniff].contains(&0),
        "Binary file cannot be expanded"
    );

    let text = String::from_utf8_lossy(bytes);
    let all_lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let total_lines = all_lines.len();

    let (actual_start, lines) = match (start_line, end_line) {
        (Some(s), Some(e)) if s >= 1 && e >= s => {
            let lo = s.saturating_sub(1).min(total_lines);
            let hi = e.min(total_lines);
            (lo + 1, all_lines[lo..hi].to_vec())
        }
        (Some(s), None) if s >= 1 => {
            let lo = s.saturating_sub(1).min(total_lines);
            (lo + 1, all_lines[lo..].to_vec())
        }
        _ => (1, all_lines),
    };

    Ok(json!({
        "path": path,
        "total_lines": total_lines,
        "start_line": actual_start,
        "lines": lines,
        "truncated": false,
    }))
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
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            id: "proj1".into(),
            name: "Demo".into(),
            directory: repo.to_string_lossy().into_owned(),
            workflow: Default::default(),
            target_branch: Some("main".into()),
            created_at: 0,
        };
        (tmp, project)
    }

    #[test]
    fn uncommitted_keeps_staged_and_unstaged_patches_even_when_they_cancel() {
        let (_tmp, project) = make_repo();
        let repo = Path::new(&project.directory);
        let original = fs::read_to_string(repo.join("README.md")).unwrap();
        fs::write(repo.join("README.md"), "staged replacement\n").unwrap();
        git(repo, &["add", "README.md"]);
        fs::write(repo.join("README.md"), &original).unwrap();
        fs::write(repo.join("new.txt"), "untracked content\n").unwrap();
        let ops = Ops::local();
        let net = ops
            .git_compare(&project, "HEAD", "WORKDIR", None, false)
            .unwrap();
        assert!(
            !net["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["filename"] == "README.md")
        );
        let data = ops
            .git_compare(&project, "HEAD", "UNCOMMITTED", None, false)
            .unwrap();
        let files = data["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        let index = files
            .iter()
            .find(|f| f["filename"] == "README.md" && f["section"] == "staged")
            .unwrap();
        let disk = files
            .iter()
            .find(|f| f["filename"] == "README.md" && f["section"] == "unstaged")
            .unwrap();
        assert!(
            index["patch"]
                .as_str()
                .unwrap()
                .contains("+staged replacement")
        );
        assert!(
            disk["patch"]
                .as_str()
                .unwrap()
                .contains("-staged replacement")
        );
        assert!(files.iter().any(|f| f["filename"] == "new.txt"
            && f["section"] == "unstaged"
            && f["status"] == "added"));
        // A clean checkout stays a valid empty Uncommitted comparison.
        git(repo, &["reset", "--hard", "HEAD"]);
        fs::remove_file(repo.join("new.txt")).unwrap();
        assert!(
            ops.git_compare(&project, "HEAD", "UNCOMMITTED", None, false)
                .unwrap()["files"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn uncommitted_works_before_the_first_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-b", "main"]);
        fs::write(repo.join("staged.txt"), "index\n").unwrap();
        git(repo, &["add", "staged.txt"]);
        fs::write(repo.join("new.txt"), "disk\n").unwrap();
        let mut project = make_repo().1;
        project.directory = repo.to_string_lossy().into_owned();
        let data = Ops::local()
            .git_compare(&project, "HEAD", "UNCOMMITTED", None, false)
            .unwrap();
        assert_eq!(data["files"].as_array().unwrap().len(), 2);
        assert_eq!(data["files"][0]["section"], "staged");
        assert_eq!(data["files"][1]["section"], "unstaged");
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

    #[test]
    fn seed_pr_and_configure_tracking_handle_same_repo_and_fork_prs() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("origin.git");
        let source = tmp.path().join("source");
        let peer = tmp.path().join("peer");
        let home = tmp.path().join("home");
        fs::create_dir_all(&remote).unwrap();
        fs::create_dir_all(&source).unwrap();
        git(&remote, &["init", "--bare", "-b", "main"]);
        git(&source, &["init", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Tester"),
            ("user.email", "tester@example.invalid"),
            ("core.hooksPath", "/dev/null"),
            ("commit.gpgsign", "false"),
        ] {
            git(&source, &["config", k, v]);
        }
        fs::write(source.join("README.md"), "root\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "root"]);
        git(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&source, &["push", "-u", "origin", "main"]);

        // Peer creates a same-repo PR branch `feat/pr-branch` and a fork PR ref `refs/pull/42/head`.
        git(
            tmp.path(),
            &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
        );
        for (k, v) in [
            ("user.name", "Peer"),
            ("user.email", "peer@example.invalid"),
            ("core.hooksPath", "/dev/null"),
            ("commit.gpgsign", "false"),
        ] {
            git(&peer, &["config", k, v]);
        }
        git(&peer, &["switch", "-c", "feat/pr-branch"]);
        fs::write(peer.join("pr_file.txt"), "from same-repo pr\n").unwrap();
        git(&peer, &["add", "."]);
        git(&peer, &["commit", "-m", "feat: pr commit"]);
        git(&peer, &["push", "origin", "feat/pr-branch"]);

        // Also publish a fork PR commit directly to refs/pull/42/head on the bare remote.
        git(&peer, &["switch", "main"]);
        git(&peer, &["switch", "-c", "fork/patch"]);
        fs::write(peer.join("fork_file.txt"), "from fork\n").unwrap();
        git(&peer, &["add", "."]);
        git(&peer, &["commit", "-m", "feat: fork pr"]);
        git(&peer, &["push", "origin", "HEAD:refs/pull/42/head"]);

        let mut workflow = canban_types::Workflow::default();
        workflow.workspace.sync_base = false;
        let project = Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            id: "projpr123".into(),
            name: "PR Demo".into(),
            directory: source.to_string_lossy().into_owned(),
            workflow,
            target_branch: Some("main".into()),
            created_at: 0,
        };
        let mut card_same = canban_types::Card {
            schedule_source: None,
            source_trust: None,
            hold_auto_merge: false,
            github_links: Vec::new(),
            created_by: String::new(),
            content: Default::default(),
            removed_at: None,
            pull_request: None,
            pull_request_history: Vec::new(),
            delivery_mode: None,
            workspace: None,
            workspace_history: Vec::new(),
            id: "card-same".into(),
            project_id: project.id.clone(),
            number: 10,
            board_order: None,
            title: "Same repo PR".into(),
            body: String::new(),
            context_notes: None,
            stage: "develop".into(),
            revision: 1,
            attention: None,
            archived_at: None,
            archive: None,
            archive_error: None,
            worktree: None,
            branch: None,
            base_commit: None,
            human_gate: None,
            delivery: None,
            resolution: None,
            created_at: 0,
        };
        let ops = Ops::local();
        let target_same = ops
            .seed_pr_for_card(
                &project,
                &card_same.id,
                &PrCheckoutSpec {
                    pr_number: 11,
                    head_ref: Some("feat/pr-branch".into()),
                    same_repo: true,
                    remote: None,
                },
            )
            .unwrap()
            .expect("same-repo tracking target");
        let wt_same = ops.prepare(&home, &project, &card_same).unwrap();
        assert!(wt_same.join("pr_file.txt").exists());
        card_same.worktree = Some(wt_same.to_string_lossy().into_owned());
        let info_same = ops.info(&project, &card_same).unwrap();
        let br_same = info_same.branch.clone().unwrap();
        ops.configure_pr_tracking(&wt_same, &br_same, &target_same)
            .unwrap();
        let info_same = ops.info(&project, &card_same).unwrap();
        assert_eq!(info_same.upstream.as_deref(), Some("origin/feat/pr-branch"));

        // Peer pushes a second commit to feat/pr-branch -> git_pull fast-forwards Card worktree.
        git(&peer, &["switch", "feat/pr-branch"]);
        fs::write(peer.join("pr_file.txt"), "from same-repo pr v2\n").unwrap();
        git(&peer, &["add", "."]);
        git(&peer, &["commit", "-m", "feat: pr commit 2"]);
        git(&peer, &["push", "origin", "feat/pr-branch"]);
        let pull_res = ops.git_pull(&project, &wt_same, None, None).unwrap();
        assert_eq!(pull_res["fast_forwarded"], true);
        assert_eq!(
            fs::read_to_string(wt_same.join("pr_file.txt")).unwrap(),
            "from same-repo pr v2\n"
        );

        // Cross-repo fork PR: tracks refs/pull/42/head and rejects direct push.
        let mut card_fork = card_same.clone();
        card_fork.id = "card-fork".into();
        card_fork.number = 11;
        card_fork.worktree = None;
        card_fork.branch = None;
        let target_fork = ops
            .seed_pr_for_card(
                &project,
                &card_fork.id,
                &PrCheckoutSpec {
                    pr_number: 42,
                    head_ref: Some("fork/patch".into()),
                    same_repo: false,
                    remote: None,
                },
            )
            .unwrap()
            .expect("fork tracking target");
        let wt_fork = ops.prepare(&home, &project, &card_fork).unwrap();
        assert!(wt_fork.join("fork_file.txt").exists());
        card_fork.worktree = Some(wt_fork.to_string_lossy().into_owned());
        let info_fork = ops.info(&project, &card_fork).unwrap();
        let br_fork = info_fork.branch.clone().unwrap();
        ops.configure_pr_tracking(&wt_fork, &br_fork, &target_fork)
            .unwrap();
        let info_fork = ops.info(&project, &card_fork).unwrap();
        assert_eq!(info_fork.upstream.as_deref(), Some("origin/pr/42"));
        let push_err = ops.git_push(&project, &wt_fork, None, false).unwrap_err();
        assert!(push_err.to_string().contains("read-only GitHub PR ref"));
    }

    #[test]
    fn git_file_lines_reads_workdir_index_and_rev_and_rejects_traversal_and_binary() {
        let (_tmp, project) = make_repo();
        let repo = Path::new(&project.directory);
        let ops = Ops::local();

        fs::write(repo.join("notes.txt"), "l1\nl2\nl3\nl4\n").unwrap();
        git(repo, &["add", "notes.txt"]);
        git(repo, &["commit", "-m", "add notes"]);
        let head_sha = ops.checked(repo, &["rev-parse", "HEAD"]).unwrap();

        fs::write(repo.join("notes.txt"), "l1\nl2-index\nl3\nl4\n").unwrap();
        git(repo, &["add", "notes.txt"]);
        fs::write(repo.join("notes.txt"), "l1\nl2-workdir\nl3\nl4\nl5\n").unwrap();

        let from_rev = ops
            .git_file_lines(FileLinesRequest {
                project: &project,
                cwd_override: None,
                path: "notes.txt",
                source: "rev",
                rev: Some(&head_sha),
                start_line: None,
                end_line: None,
            })
            .unwrap();
        assert_eq!(from_rev["total_lines"], 4);
        assert_eq!(from_rev["lines"][1], "l2");

        let from_index = ops
            .git_file_lines(FileLinesRequest {
                project: &project,
                cwd_override: None,
                path: "notes.txt",
                source: "index",
                rev: None,
                start_line: Some(2),
                end_line: Some(3),
            })
            .unwrap();
        assert_eq!(from_index["total_lines"], 4);
        assert_eq!(from_index["start_line"], 2);
        assert_eq!(from_index["lines"], json!(["l2-index", "l3"]));

        let from_workdir = ops
            .git_file_lines(FileLinesRequest {
                project: &project,
                cwd_override: None,
                path: "notes.txt",
                source: "workdir",
                rev: None,
                start_line: None,
                end_line: None,
            })
            .unwrap();
        assert_eq!(from_workdir["total_lines"], 5);
        assert_eq!(from_workdir["lines"][1], "l2-workdir");
        assert_eq!(from_workdir["lines"][4], "l5");

        assert!(
            ops.git_file_lines(FileLinesRequest {
                project: &project,
                cwd_override: None,
                path: "../etc/passwd",
                source: "workdir",
                rev: None,
                start_line: None,
                end_line: None,
            })
            .is_err()
        );
        fs::write(repo.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        assert!(
            ops.git_file_lines(FileLinesRequest {
                project: &project,
                cwd_override: None,
                path: "bin.dat",
                source: "workdir",
                rev: None,
                start_line: None,
                end_line: None,
            })
            .is_err()
        );
    }

    #[test]
    fn seed_and_fetch_pr_uses_explicit_upstream_remote_or_url_fallback() {
        let (dir, mut project) = make_repo();
        let repo = PathBuf::from(&project.directory);
        let fork_remote = dir.path().join("fork.git");
        let upstream_remote = dir.path().join("upstream.git");
        let peer = dir.path().join("peer");
        let home = dir.path().join("home");
        fs::create_dir_all(&fork_remote).unwrap();
        fs::create_dir_all(&upstream_remote).unwrap();
        git(&fork_remote, &["init", "--bare", "-b", "main"]);
        git(&upstream_remote, &["init", "--bare", "-b", "main"]);

        git(
            &repo,
            &["remote", "add", "origin", fork_remote.to_str().unwrap()],
        );
        git(
            &repo,
            &[
                "remote",
                "add",
                "upstream",
                upstream_remote.to_str().unwrap(),
            ],
        );
        git(&repo, &["push", "-u", "origin", "main"]);
        git(&repo, &["push", "upstream", "main"]);

        // Publish PR #1758 only on `upstream`, NOT on `origin` (fork).
        git(
            dir.path(),
            &[
                "clone",
                upstream_remote.to_str().unwrap(),
                peer.to_str().unwrap(),
            ],
        );
        for (k, v) in [
            ("user.name", "Peer"),
            ("user.email", "peer@example.invalid"),
            ("core.hooksPath", "/dev/null"),
            ("commit.gpgsign", "false"),
        ] {
            git(&peer, &["config", k, v]);
        }
        git(&peer, &["switch", "-c", "pr-1758"]);
        fs::write(peer.join("pr1758.txt"), "upstream pr 1758\n").unwrap();
        git(&peer, &["add", "."]);
        git(&peer, &["commit", "-m", "feat: pr 1758"]);
        git(&peer, &["push", "origin", "HEAD:refs/pull/1758/head"]);

        project.workflow.workspace.remote = "origin".into();
        let ops = Ops::local();

        // Fetching from `origin` (default) fails because the fork does not have PR #1758.
        assert!(
            ops.seed_pr_for_card(
                &project,
                "card-fail",
                &PrCheckoutSpec {
                    pr_number: 1758,
                    head_ref: None,
                    same_repo: false,
                    remote: None,
                },
            )
            .is_err()
        );

        // Specifying `Named("upstream")` fetches from upstream and tracks `upstream/pr/1758`.
        let mut card = canban_types::Card {
            schedule_source: None,
            source_trust: None,
            hold_auto_merge: false,
            github_links: Vec::new(),
            created_by: String::new(),
            content: Default::default(),
            removed_at: None,
            pull_request: None,
            pull_request_history: Vec::new(),
            delivery_mode: None,
            workspace: None,
            workspace_history: Vec::new(),
            id: "card-upstream".into(),
            project_id: project.id.clone(),
            number: 102,
            board_order: None,
            title: "Upstream PR 1758".into(),
            body: String::new(),
            context_notes: None,
            stage: "develop".into(),
            revision: 1,
            attention: None,
            archived_at: None,
            archive: None,
            archive_error: None,
            worktree: None,
            branch: None,
            base_commit: None,
            human_gate: None,
            delivery: None,
            resolution: None,
            created_at: 0,
        };
        let target = ops
            .seed_pr_for_card(
                &project,
                &card.id,
                &PrCheckoutSpec {
                    pr_number: 1758,
                    head_ref: None,
                    same_repo: false,
                    remote: Some(PrRemoteSource::Named("upstream".into())),
                },
            )
            .unwrap()
            .expect("upstream tracking target");
        assert_eq!(target.remote, "upstream");
        let wt = ops.prepare(&home, &project, &card).unwrap();
        assert_eq!(
            fs::read_to_string(wt.join("pr1758.txt")).unwrap(),
            "upstream pr 1758\n"
        );
        card.worktree = Some(wt.to_string_lossy().into_owned());
        let info = ops.info(&project, &card).unwrap();
        let br = info.branch.clone().unwrap();
        ops.configure_pr_tracking(&wt, &br, &target).unwrap();
        let info = ops.info(&project, &card).unwrap();
        assert_eq!(info.upstream.as_deref(), Some("upstream/pr/1758"));

        // `fetch_pr_for_card` with `Named("upstream")` also succeeds.
        let sha = ops
            .fetch_pr_for_card(
                &project,
                1758,
                "card-fetch-upstream",
                Some(&PrRemoteSource::Named("upstream".into())),
            )
            .unwrap();
        assert!(!sha.is_empty());

        // Direct URL fallback (`PrRemoteSource::Urls`) skips broken URL and succeeds on valid URL.
        let url_target = ops
            .seed_pr_for_card(
                &project,
                "card-url-fallback",
                &PrCheckoutSpec {
                    pr_number: 1758,
                    head_ref: None,
                    same_repo: false,
                    remote: Some(PrRemoteSource::Urls(vec![
                        dir.path()
                            .join("missing.git")
                            .to_string_lossy()
                            .into_owned(),
                        upstream_remote.to_string_lossy().into_owned(),
                    ])),
                },
            )
            .unwrap();
        assert!(url_target.is_none());
        let url_sha = ops
            .fetch_pr_for_card(
                &project,
                1758,
                "card-fetch-url",
                Some(&PrRemoteSource::Urls(vec![
                    upstream_remote.to_string_lossy().into_owned(),
                ])),
            )
            .unwrap();
        assert_eq!(url_sha, sha);
    }

    #[test]
    fn parse_worktree_list_filters_bare_and_prunable_entries() {
        let raw = concat!(
            "worktree /repo\0HEAD 1111111\0branch refs/heads/main\0\0",
            "worktree /repo/.worktrees/fix-overpay\0HEAD 2222222\0branch refs/heads/fix/overpay\0\0",
            "worktree /repo/.worktrees/detached\0HEAD 3333333\0detached\0\0",
            "worktree /tmp/gone\0HEAD 4444444\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0",
            "worktree /repo/bare.git\0bare\0\0",
        );
        let list = parse_worktree_list_porcelain_z(raw);
        assert_eq!(
            list,
            vec![
                GitWorktree {
                    path: "/repo".into(),
                    branch: Some("main".into()),
                },
                GitWorktree {
                    path: "/repo/.worktrees/fix-overpay".into(),
                    branch: Some("fix/overpay".into()),
                },
                GitWorktree {
                    path: "/repo/.worktrees/detached".into(),
                    branch: None,
                },
            ]
        );
    }

    #[test]
    fn git_refs_in_with_context_prefers_detected_default_branch_and_reports_sync_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let upstream_bare = tmp.path().join("upstream.git");
        let fork_bare = tmp.path().join("fork.git");
        let seed = tmp.path().join("seed");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&seed).unwrap();
        git(
            &seed,
            &[
                "init",
                "--bare",
                "-b",
                "develop",
                upstream_bare.to_str().unwrap(),
            ],
        );
        git(
            &seed,
            &[
                "init",
                "--bare",
                "-b",
                "develop",
                fork_bare.to_str().unwrap(),
            ],
        );
        git(&seed, &["init", "-b", "develop"]);
        for (k, v) in [
            ("user.name", "Reviewer"),
            ("user.email", "reviewer@example.invalid"),
            ("core.hooksPath", "/dev/null"),
            ("commit.gpgsign", "false"),
        ] {
            git(&seed, &["config", k, v]);
        }
        fs::write(seed.join("README.md"), "# Fiber\n").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-m", "init develop"]);
        git(
            &seed,
            &["remote", "add", "upstream", upstream_bare.to_str().unwrap()],
        );
        git(&seed, &["push", "upstream", "develop"]);
        git(
            &seed,
            &["remote", "add", "origin", fork_bare.to_str().unwrap()],
        );
        git(&seed, &["push", "origin", "develop"]);
        git(
            tmp.path(),
            &[
                "clone",
                "-b",
                "develop",
                fork_bare.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        git(
            &repo,
            &["remote", "add", "upstream", upstream_bare.to_str().unwrap()],
        );
        git(&repo, &["fetch", "upstream"]);
        git(&repo, &["remote", "set-head", "upstream", "develop"]);
        git(&repo, &["switch", "-c", "lsp-support"]);

        let project = Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            id: "proj-fiber".into(),
            name: "Fiber".into(),
            directory: repo.to_string_lossy().into_owned(),
            workflow: Default::default(),
            target_branch: Some("lsp-support".into()),
            created_at: 0,
        };

        let refs = Ops::local()
            .git_refs_in_with_context(&project, None, Some("upstream"), None)
            .unwrap();
        assert_eq!(refs["current_branch"], "lsp-support");
        assert_eq!(refs["default_branch"], "develop");
        assert_eq!(refs["remote_name"], "origin");
        assert_eq!(refs["sync_remote"], "upstream");
        assert_eq!(refs["has_sync_remote"], true);
        assert!(
            refs["branches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == "develop" && b["is_default"] == true)
        );
        assert!(
            refs["branches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == "upstream/develop" && b["kind"] == "remote")
        );
    }
}
