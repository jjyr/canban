use crate::Ops;
use crate::compare::{EMPTY_TREE, validate_ref_arg};
use anyhow::{Context, Result, bail, ensure};
use canban_types::Project;
use serde_json::{Value, json};
use std::path::{Component, Path};

/// Parses Git's `%(upstream:track)` string (e.g. `"[ahead 2, behind 1]"`, `"[gone]"`)
/// into `(ahead, behind, gone)`.
pub(crate) fn parse_upstream_track(track: &str) -> (u64, u64, bool) {
    let trimmed = track.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    if inner.is_empty() {
        return (0, 0, false);
    }
    let mut ahead = 0u64;
    let mut behind = 0u64;
    let mut gone = false;
    for part in inner.split(',') {
        let p = part.trim();
        if p == "gone" {
            gone = true;
        } else if let Some(n) = p.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = p.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind, gone)
}

fn validate_rel_paths(paths: &[String]) -> Result<Vec<&str>> {
    ensure!(!paths.is_empty(), "No file paths provided");
    let mut out = Vec::with_capacity(paths.len());
    for raw in paths {
        let p = raw.trim();
        ensure!(!p.is_empty(), "File path is empty");
        ensure!(
            !p.starts_with('/') && !p.starts_with('-'),
            "Invalid relative file path: {p}"
        );
        ensure!(!p.contains('\0'), "File path contains NUL byte");
        let rel = Path::new(p);
        ensure!(!rel.is_absolute(), "File path must be relative: {p}");
        for comp in rel.components() {
            if matches!(
                comp,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            ) {
                bail!("Invalid file path component in '{p}'");
            }
        }
        out.push(p);
    }
    Ok(out)
}

impl Ops {
    /// Parses `git status --porcelain=v1 -z --untracked-files=all` into structured file entries.
    pub fn status_files(&self, dir: &Path) -> Result<Vec<Value>> {
        let out = self.git(
            dir,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !out.status.success() {
            return Ok(Vec::new());
        }
        let raw = &out.stdout;
        let mut entries = Vec::new();
        let mut parts = raw.split(|&b| b == 0).filter(|s| !s.is_empty());
        while let Some(rec) = parts.next() {
            if rec.len() < 4 || rec[2] != b' ' {
                continue;
            }
            let x = rec[0] as char;
            let y = rec[1] as char;
            let path = String::from_utf8_lossy(&rec[3..]).into_owned();
            let orig_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
                parts
                    .next()
                    .map(|orig| String::from_utf8_lossy(orig).into_owned())
            } else {
                None
            };
            if x == '!' && y == '!' {
                continue;
            }
            let untracked = x == '?' && y == '?';
            let conflicted = matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'));
            let staged = x != ' ' && x != '?' && x != '!';
            let unstaged = y != ' ' && y != '?' && y != '!';
            entries.push(json!({
                "path": path,
                "orig_path": orig_path,
                "index_code": x.to_string(),
                "worktree_code": y.to_string(),
                "staged": staged,
                "unstaged": unstaged,
                "untracked": untracked,
                "conflicted": conflicted,
            }));
        }
        Ok(entries)
    }

    /// Paginated commit log for any branch, tag, or commit reference, with optional filter query.
    pub fn git_log(
        &self,
        dir: &Path,
        rev: Option<&str>,
        skip: usize,
        limit: usize,
        query: Option<&str>,
    ) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Repository directory does not exist: {}",
            dir.display()
        );
        let rev_str = rev
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("HEAD");
        validate_ref_arg(rev_str)?;

        let capped = limit.clamp(1, 100);
        let q = query.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
        ensure!(!q.contains('\0'), "Query contains NUL byte");

        let rev_exists = self
            .git(
                dir,
                &["rev-parse", "--verify", &format!("{rev_str}^{{commit}}")],
            )?
            .status
            .success();
        if !rev_exists {
            return Ok(json!({
                "rev": rev_str,
                "commits": [],
                "skip": skip,
                "limit": capped,
                "has_more": false,
            }));
        }

        let skip_arg = format!("--skip={skip}");
        let count_arg = format!("{}", capped + 1);
        let grep_arg = format!("--grep={q}");
        let mut args = vec![
            "log",
            &skip_arg,
            "-n",
            &count_arg,
            "--format=%H%x09%h%x09%P%x09%cr%x09%cI%x09%an%x09%ae%x09%s",
        ];
        if !q.is_empty() {
            args.push("-i");
            args.push("--fixed-strings");
            args.push(&grep_arg);
        }
        args.push(rev_str);
        args.push("--");

        let raw = self.checked(dir, &args).unwrap_or_default();
        let mut commits: Vec<Value> = raw
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(8, '\t').collect();
                (parts.len() >= 8).then(|| {
                    let parents: Vec<&str> = parts[2].split_whitespace().collect();
                    json!({
                        "sha": parts[0],
                        "short_sha": parts[1],
                        "parents": parents,
                        "relative_time": parts[3],
                        "date_iso": parts[4],
                        "author": parts[5],
                        "email": parts[6],
                        "subject": parts[7],
                    })
                })
            })
            .collect();

        // Also support matching by commit SHA prefix when `--grep` found nothing on page 0.
        if commits.is_empty()
            && skip == 0
            && (4..=40).contains(&q.len())
            && q.chars().all(|c| c.is_ascii_hexdigit())
            && let Ok(sha) =
                self.checked(dir, &["rev-parse", "--verify", &format!("{q}^{{commit}}")])
            && self
                .git(dir, &["merge-base", "--is-ancestor", &sha, rev_str])
                .map(|o| o.status.success())
                .unwrap_or(false)
            && let Ok(single) = self.checked(
                dir,
                &[
                    "log",
                    "-n",
                    "1",
                    "--format=%H%x09%h%x09%P%x09%cr%x09%cI%x09%an%x09%ae%x09%s",
                    &sha,
                ],
            )
            && let Some(line) = single.lines().find(|l| !l.is_empty())
        {
            let parts: Vec<&str> = line.splitn(8, '\t').collect();
            if parts.len() >= 8 {
                let parents: Vec<&str> = parts[2].split_whitespace().collect();
                commits.push(json!({
                    "sha": parts[0],
                    "short_sha": parts[1],
                    "parents": parents,
                    "relative_time": parts[3],
                    "date_iso": parts[4],
                    "author": parts[5],
                    "email": parts[6],
                    "subject": parts[7],
                }));
            }
        }

        let has_more = commits.len() > capped;
        if has_more {
            commits.truncate(capped);
        }

        Ok(json!({
            "rev": rev_str,
            "commits": commits,
            "skip": skip,
            "limit": capped,
            "has_more": has_more,
        }))
    }

    /// Reads full metadata (author, email, timestamps, subject, body, and parent summaries) for one commit.
    pub fn commit_metadata(&self, dir: &Path, commit_ref: &str) -> Result<Value> {
        validate_ref_arg(commit_ref)?;
        let sha = self
            .checked(
                dir,
                &["rev-parse", "--verify", &format!("{commit_ref}^{{commit}}")],
            )
            .with_context(|| format!("Unknown commit '{commit_ref}'"))?;

        let header = self.checked(
            dir,
            &[
                "show",
                "-s",
                "--format=%H%x09%h%x09%P%x09%an%x09%ae%x09%cI%x09%cr%x09%s",
                &sha,
            ],
        )?;
        let parts: Vec<&str> = header.splitn(8, '\t').collect();
        ensure!(parts.len() >= 8, "Unexpected git show output for {sha}");

        let body_out = self.git(dir, &["show", "-s", "--format=%b", &sha])?;
        let body = String::from_utf8_lossy(&body_out.stdout)
            .trim_end()
            .to_owned();

        let parent_shas: Vec<&str> = parts[2].split_whitespace().collect();
        let mut parents = Vec::with_capacity(parent_shas.len());
        for (idx, p_sha) in parent_shas.iter().enumerate() {
            let summary = self
                .checked(dir, &["show", "-s", "--format=%h%x09%s", p_sha])
                .unwrap_or_default();
            let mut sp = summary.splitn(2, '\t');
            let short = sp.next().unwrap_or(&p_sha[..p_sha.len().min(7)]).to_owned();
            let subj = sp.next().unwrap_or("").to_owned();
            parents.push(json!({
                "index": idx,
                "sha": p_sha,
                "short_sha": short,
                "subject": subj,
            }));
        }

        Ok(json!({
            "sha": parts[0],
            "short_sha": parts[1],
            "author": parts[3],
            "email": parts[4],
            "date_iso": parts[5],
            "relative_time": parts[6],
            "subject": parts[7],
            "body": body,
            "parents": parents,
        }))
    }

    /// Inspects a single commit's full metadata and unified diff against `parent_index` (default `0`).
    pub fn git_commit_detail(
        &self,
        project: &Project,
        cwd_override: Option<&str>,
        commit_ref: &str,
        parent_index: usize,
    ) -> Result<Value> {
        let dir = Path::new(cwd_override.unwrap_or(&project.directory));
        ensure!(
            self.host.is_dir(dir),
            "Repository directory does not exist: {}",
            dir.display()
        );
        let mut meta = self.commit_metadata(dir, commit_ref)?;
        let sha = meta["sha"].as_str().unwrap_or(commit_ref).to_owned();
        let parent_count = meta["parents"].as_array().map(|a| a.len()).unwrap_or(0);
        let base_spec = if parent_count == 0 {
            EMPTY_TREE.to_owned()
        } else {
            ensure!(
                parent_index < parent_count,
                "Parent index {parent_index} out of range (commit has {parent_count} parent(s))"
            );
            format!("{sha}^{}", parent_index + 1)
        };
        meta["parent_index"] = json!(parent_index);

        let mut cmp = self.git_compare(project, &base_spec, &sha, cwd_override, false)?;
        cmp["commit_detail"] = meta;
        Ok(cmp)
    }

    /// Stages specific relative paths (`git add -A -- <paths>`) or the entire checkout (`all = true`).
    pub fn git_stage(&self, dir: &Path, paths: &[String], all: bool) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        if all {
            self.r_checked(dir, &["add", "-A", "--", "."])?;
        } else {
            let rels = validate_rel_paths(paths)?;
            let mut args = vec!["add", "-A", "--"];
            args.extend(rels);
            self.r_checked(dir, &args)?;
        }
        let status = self.status_files(dir)?;
        Ok(json!({ "status_files": status }))
    }

    /// Unstages specific relative paths or all staged paths, handling unborn `HEAD` repositories cleanly.
    pub fn git_unstage(&self, dir: &Path, paths: &[String], all: bool) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        let has_head = self
            .git(dir, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success();
        if all {
            if has_head {
                self.r_checked(dir, &["reset", "-q", "HEAD", "--", "."])?;
            } else {
                self.r_checked(
                    dir,
                    &["rm", "-q", "--cached", "-r", "--ignore-unmatch", "--", "."],
                )?;
            }
        } else {
            let rels = validate_rel_paths(paths)?;
            if has_head {
                let mut args = vec!["reset", "-q", "HEAD", "--"];
                args.extend(rels);
                self.r_checked(dir, &args)?;
            } else {
                let mut args = vec!["rm", "-q", "--cached", "-r", "--ignore-unmatch", "--"];
                args.extend(rels);
                self.r_checked(dir, &args)?;
            }
        }
        let status = self.status_files(dir)?;
        Ok(json!({ "status_files": status }))
    }

    /// Discards local staged, unstaged, and untracked changes for explicit relative file paths.
    pub fn git_discard(&self, dir: &Path, paths: &[String]) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        let rels = validate_rel_paths(paths)?;
        let has_head = self
            .git(dir, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success();
        let current_status = self.status_files(dir)?;

        // First unstage all target paths so newly added files (`A `) revert to untracked (`??`)
        // and renamed files split back cleanly before restoring from HEAD + cleaning untracked.
        let mut reset_paths: Vec<String> = Vec::new();
        for p in &rels {
            reset_paths.push((*p).to_owned());
            if let Some(orig) = current_status
                .iter()
                .find(|s| s["path"].as_str() == Some(*p))
                .and_then(|s| s["orig_path"].as_str())
            {
                reset_paths.push(orig.to_owned());
            }
        }

        if has_head {
            let mut reset_args = vec!["reset", "-q", "HEAD", "--"];
            for rp in &reset_paths {
                reset_args.push(rp.as_str());
            }
            let _ = self.git(dir, &reset_args)?;
        } else {
            let mut rm_args = vec!["rm", "-q", "--cached", "-r", "--ignore-unmatch", "--"];
            for rp in &reset_paths {
                rm_args.push(rp.as_str());
            }
            let _ = self.git(dir, &rm_args)?;
        }

        let mut in_head = Vec::<&str>::new();
        let mut not_in_head = Vec::<&str>::new();
        for rp in &reset_paths {
            let exists_in_head = has_head
                && self
                    .git(dir, &["cat-file", "-e", &format!("HEAD:{rp}")])
                    .map(|o| o.status.success())
                    .unwrap_or(false);
            if exists_in_head {
                in_head.push(rp.as_str());
            } else {
                not_in_head.push(rp.as_str());
            }
        }

        if !in_head.is_empty() {
            let mut co_args = vec!["checkout", "HEAD", "--"];
            co_args.extend(in_head);
            self.r_checked(dir, &co_args)?;
        }
        if !not_in_head.is_empty() {
            let mut clean_args = vec!["clean", "-fd", "--"];
            clean_args.extend(not_in_head);
            self.r_checked(dir, &clean_args)?;
        }

        let status = self.status_files(dir)?;
        Ok(json!({ "status_files": status }))
    }

    /// Creates a commit (or amends `HEAD`), optionally staging all modified/untracked files first.
    pub fn git_commit(
        &self,
        dir: &Path,
        message: &str,
        amend: bool,
        stage_all: bool,
    ) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        let msg = message.trim();
        ensure!(!msg.is_empty(), "Commit message cannot be empty");
        ensure!(!msg.contains('\0'), "Commit message contains NUL byte");

        if stage_all {
            self.r_checked(dir, &["add", "-A", "--", "."])?;
        }

        let has_head = self
            .git(dir, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success();
        if amend {
            ensure!(has_head, "Cannot amend in a repository with no commits");
        } else {
            let has_staged = if has_head {
                !self
                    .git(dir, &["diff", "--cached", "--quiet"])?
                    .status
                    .success()
            } else {
                !self
                    .checked(dir, &["ls-files", "--cached"])
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            };
            ensure!(has_staged, "No staged changes to commit; stage files first");
        }

        if amend {
            self.r_checked(dir, &["commit", "--amend", "-m", msg])?;
        } else {
            self.r_checked(dir, &["commit", "-m", msg])?;
        }

        let sha = self.checked(dir, &["rev-parse", "HEAD"])?;
        let short_sha = self.checked(dir, &["rev-parse", "--short", "HEAD"])?;
        let subject = self
            .checked(dir, &["show", "-s", "--format=%s", "HEAD"])
            .unwrap_or_default();
        let branch = self
            .checked(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .filter(|s| !s.is_empty());

        Ok(json!({
            "sha": sha,
            "short_sha": short_sha,
            "branch": branch,
            "subject": subject,
            "amended": amend,
        }))
    }

    fn resolve_remote_name(
        &self,
        project: &Project,
        dir: &Path,
        remote: Option<&str>,
    ) -> Result<String> {
        if let Some(r) = remote.map(str::trim).filter(|s| !s.is_empty()) {
            validate_ref_arg(r)?;
            return Ok(r.to_owned());
        }
        if let Ok(branch) = self.checked(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            && !branch.is_empty()
            && let Ok(cfg_remote) = self.checked(
                dir,
                &["config", "--get", &format!("branch.{branch}.remote")],
            )
            && !cfg_remote.trim().is_empty()
        {
            let r = cfg_remote.trim().to_owned();
            validate_ref_arg(&r)?;
            return Ok(r);
        }
        let default_remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.trim()
        };
        if self
            .git(dir, &["remote", "get-url", default_remote])?
            .status
            .success()
        {
            return Ok(default_remote.to_owned());
        }
        if let Ok(remotes) = self.checked(dir, &["remote"])
            && let Some(first) = remotes.lines().map(str::trim).find(|s| !s.is_empty())
        {
            validate_ref_arg(first)?;
            return Ok(first.to_owned());
        }
        bail!("No Git remote is configured for this repository");
    }

    /// Runs `git fetch` (with optional `--prune`) against the resolved remote.
    pub fn git_fetch(
        &self,
        project: &Project,
        dir: &Path,
        remote: Option<&str>,
        prune: bool,
    ) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        let remote_name = self.resolve_remote_name(project, dir, remote)?;
        ensure!(
            self.git(dir, &["remote", "get-url", &remote_name])?
                .status
                .success(),
            "Git remote '{remote_name}' is not configured"
        );
        if prune {
            self.checked(dir, &["fetch", "--prune", "--", &remote_name])?;
        } else {
            self.checked(dir, &["fetch", "--", &remote_name])?;
        }
        Ok(json!({
            "remote": remote_name,
            "pruned": prune,
        }))
    }

    /// Safely fast-forwards the current branch from its remote counterpart (`git pull --ff-only`).
    /// Never silently creates merge commits or rebases diverged history.
    pub fn git_pull(
        &self,
        project: &Project,
        dir: &Path,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        let current_branch = self
            .checked(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .filter(|s| !s.is_empty())
            .context("Checkout is on a detached HEAD; switch to a local branch before pulling")?;

        let remote_name = self.resolve_remote_name(project, dir, remote)?;
        ensure!(
            self.git(dir, &["remote", "get-url", &remote_name])?
                .status
                .success(),
            "Git remote '{remote_name}' is not configured"
        );

        let remote_branch = if let Some(b) = branch.map(str::trim).filter(|s| !s.is_empty()) {
            validate_ref_arg(b)?;
            b.to_owned()
        } else if remote.map(str::trim).filter(|s| !s.is_empty()).is_none()
            && let Ok(merge_ref) = self.checked(
                dir,
                &["config", "--get", &format!("branch.{current_branch}.merge")],
            )
            && !merge_ref.trim().is_empty()
        {
            merge_ref
                .trim()
                .strip_prefix("refs/heads/")
                .unwrap_or(merge_ref.trim())
                .to_owned()
        } else {
            current_branch.clone()
        };
        validate_ref_arg(&remote_branch)?;

        let old_head = self
            .checked(dir, &["rev-parse", "HEAD"])
            .unwrap_or_default();
        let out = self.git(
            dir,
            &["pull", "--ff-only", "--", &remote_name, &remote_branch],
        )?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            bail!("Fast-forward pull from {remote_name}/{remote_branch} failed: {detail}");
        }
        if remote_branch.starts_with("refs/pull/") {
            let _ = self.git(dir, &["fetch", "--", &remote_name]);
        }
        let new_head = self.checked(dir, &["rev-parse", "HEAD"])?;

        Ok(json!({
            "branch": current_branch,
            "remote": remote_name,
            "remote_branch": remote_branch,
            "old_head": old_head,
            "new_head": new_head,
            "fast_forwarded": old_head != new_head,
        }))
    }

    /// Pushes the current local branch to the remote without force-pushing, automatically setting upstream
    /// when `set_upstream` is requested or no upstream tracking branch is configured yet.
    pub fn git_push(
        &self,
        project: &Project,
        dir: &Path,
        remote: Option<&str>,
        set_upstream: bool,
    ) -> Result<Value> {
        ensure!(
            self.host.is_dir(dir),
            "Checkout directory does not exist: {}",
            dir.display()
        );
        let current_branch = self
            .checked(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .filter(|s| !s.is_empty())
            .context("Checkout is on a detached HEAD; switch to a local branch before pushing")?;

        let remote_name = self.resolve_remote_name(project, dir, remote)?;
        ensure!(
            self.git(dir, &["remote", "get-url", "--push", &remote_name])?
                .status
                .success(),
            "Git remote '{remote_name}' is not configured"
        );

        let has_upstream = self
            .git(
                dir,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )?
            .status
            .success();

        let target_remote_branch = if remote.map(str::trim).filter(|s| !s.is_empty()).is_none()
            && let Ok(merge_ref) = self.checked(
                dir,
                &["config", "--get", &format!("branch.{current_branch}.merge")],
            )
            && !merge_ref.trim().is_empty()
        {
            let trimmed = merge_ref.trim();
            if trimmed.starts_with("refs/pull/") {
                bail!(
                    "Branch '{current_branch}' tracks a read-only GitHub PR ref ({trimmed}) from a fork and cannot be pushed directly to '{remote_name}'"
                );
            }
            trimmed
                .strip_prefix("refs/heads/")
                .unwrap_or(trimmed)
                .to_owned()
        } else {
            current_branch.clone()
        };
        validate_ref_arg(&target_remote_branch)?;

        let refspec = format!("HEAD:refs/heads/{target_remote_branch}");
        let configure_upstream = set_upstream || !has_upstream;
        if configure_upstream {
            self.checked(
                dir,
                &[
                    "push",
                    "--porcelain",
                    "--set-upstream",
                    &remote_name,
                    &refspec,
                ],
            )?;
        } else {
            self.checked(dir, &["push", "--porcelain", &remote_name, &refspec])?;
        }

        let head_sha = self.checked(dir, &["rev-parse", "HEAD"])?;
        Ok(json!({
            "branch": current_branch,
            "remote": remote_name,
            "remote_branch": target_remote_branch,
            "head_sha": head_sha,
            "set_upstream": configure_upstream,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) -> String {
        Ops::local().checked(dir, args).unwrap()
    }

    fn setup_repo_with_remote() -> (tempfile::TempDir, Project, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("origin.git");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&remote).unwrap();
        fs::create_dir_all(&repo).unwrap();
        git(&remote, &["init", "--bare", "-b", "main"]);
        git(&repo, &["init", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Tester"),
            ("user.email", "tester@example.invalid"),
            ("core.hooksPath", "/dev/null"),
            ("commit.gpgsign", "false"),
        ] {
            git(&repo, &["config", k, v]);
        }
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        fs::write(repo.join("README.md"), "# Canban\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "chore: root commit"]);
        git(&repo, &["push", "-u", "origin", "main"]);

        let project = Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Vec::new(),
            id: "proj-git-client".into(),
            name: "Git Client Demo".into(),
            location: None,
            directory: repo.to_string_lossy().into_owned(),
            workflow: Default::default(),
            target_branch: Some("main".into()),
            created_at: 0,
        };
        (tmp, project, remote)
    }

    #[test]
    fn parses_upstream_track_badges() {
        assert_eq!(parse_upstream_track(""), (0, 0, false));
        assert_eq!(parse_upstream_track("[ahead 3]"), (3, 0, false));
        assert_eq!(parse_upstream_track("[behind 2]"), (0, 2, false));
        assert_eq!(parse_upstream_track("[ahead 4, behind 1]"), (4, 1, false));
        assert_eq!(parse_upstream_track("[gone]"), (0, 0, true));
    }

    #[test]
    fn stages_unstages_discards_and_commits_files() {
        let (_tmp, project, _remote) = setup_repo_with_remote();
        let ops = Ops::local();
        let repo = Path::new(&project.directory);

        // Modify tracked file + create two untracked files.
        fs::write(repo.join("README.md"), "# Canban\nupdated\n").unwrap();
        fs::write(repo.join("new_staged.txt"), "hello staged\n").unwrap();
        fs::write(repo.join("scratch.txt"), "throwaway\n").unwrap();

        let status = ops.status_files(repo).unwrap();
        assert_eq!(status.len(), 3);

        // Stage one tracked + one untracked file.
        ops.git_stage(
            repo,
            &["README.md".to_owned(), "new_staged.txt".to_owned()],
            false,
        )
        .unwrap();
        let refs = ops.git_refs(&project).unwrap();
        assert_eq!(refs["staged_count"], 2);
        assert_eq!(refs["unstaged_count"], 1);

        // Unstage README.md only.
        ops.git_unstage(repo, &["README.md".to_owned()], false)
            .unwrap();
        let refs = ops.git_refs(&project).unwrap();
        assert_eq!(refs["staged_count"], 1);
        assert_eq!(refs["unstaged_count"], 2);

        // Discard both a staged new file (`new_staged.txt`), modified tracked file (`README.md`),
        // and untracked file (`scratch.txt`).
        ops.git_discard(
            repo,
            &[
                "new_staged.txt".to_owned(),
                "README.md".to_owned(),
                "scratch.txt".to_owned(),
            ],
        )
        .unwrap();
        assert!(!repo.join("new_staged.txt").exists());
        assert!(!repo.join("scratch.txt").exists());
        assert_eq!(
            fs::read_to_string(repo.join("README.md")).unwrap(),
            "# Canban\n"
        );
        assert!(ops.status_files(repo).unwrap().is_empty());

        // Stage & commit + amend.
        fs::write(repo.join("feature.rs"), "pub fn run() {}\n").unwrap();
        let res = ops
            .git_commit(repo, "feat: add feature\n\ndetailed body line", false, true)
            .unwrap();
        assert_eq!(res["subject"], "feat: add feature");

        let detail = ops
            .git_commit_detail(&project, None, res["sha"].as_str().unwrap(), 0)
            .unwrap();
        assert_eq!(detail["commit_detail"]["subject"], "feat: add feature");
        assert_eq!(detail["commit_detail"]["body"], "detailed body line");
        assert_eq!(detail["files"].as_array().unwrap().len(), 1);

        let amended = ops
            .git_commit(repo, "feat: add feature (amended)", true, false)
            .unwrap();
        assert_eq!(amended["subject"], "feat: add feature (amended)");
        assert_ne!(amended["sha"], res["sha"]);
    }

    #[test]
    fn logs_branches_inspects_merge_parents_and_syncs_remote() {
        let (tmp, project, remote) = setup_repo_with_remote();
        let ops = Ops::local();
        let repo = Path::new(&project.directory);

        // Create a feature branch, push with auto upstream, then merge `--no-ff` on main.
        git(repo, &["switch", "-c", "feat/sync"]);
        fs::write(repo.join("sync.rs"), "pub const V: u32 = 1;\n").unwrap();
        ops.git_commit(repo, "feat: add sync module", false, true)
            .unwrap();
        let pushed = ops.git_push(&project, repo, None, false).unwrap();
        assert_eq!(pushed["set_upstream"], true);
        assert_eq!(pushed["branch"], "feat/sync");

        git(repo, &["switch", "main"]);
        fs::write(repo.join("main_only.rs"), "pub const M: u32 = 1;\n").unwrap();
        ops.git_commit(repo, "chore: main side commit", false, true)
            .unwrap();
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "feat/sync",
                "-m",
                "Merge branch 'feat/sync'",
            ],
        );
        let merge_sha = git(repo, &["rev-parse", "HEAD"]);

        // Log pagination & search.
        let page1 = ops.git_log(repo, Some("main"), 0, 2, None).unwrap();
        assert_eq!(page1["commits"].as_array().unwrap().len(), 2);
        assert_eq!(page1["has_more"], true);

        let filtered = ops
            .git_log(repo, Some("feat/sync"), 0, 20, Some("sync module"))
            .unwrap();
        assert_eq!(filtered["commits"].as_array().unwrap().len(), 1);

        // Merge commit parent 0 vs parent 1 diff inspection.
        let p0 = ops
            .git_commit_detail(&project, None, &merge_sha, 0)
            .unwrap();
        assert_eq!(p0["commit_detail"]["parents"].as_array().unwrap().len(), 2);
        assert_eq!(p0["files"][0]["filename"], "sync.rs");

        let p1 = ops
            .git_commit_detail(&project, None, &merge_sha, 1)
            .unwrap();
        assert_eq!(p1["files"][0]["filename"], "main_only.rs");

        // Push main, then simulate collaborator pushing to origin/main and test fetch + pull --ff-only.
        ops.git_push(&project, repo, None, false).unwrap();

        let peer = tmp.path().join("peer");
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
        fs::write(peer.join("remote_update.txt"), "from peer\n").unwrap();
        git(&peer, &["add", "."]);
        git(&peer, &["commit", "-m", "feat: peer commit"]);
        git(&peer, &["push", "origin", "main"]);

        ops.git_fetch(&project, repo, None, true).unwrap();
        let refs_before_pull = ops.git_refs(&project).unwrap();
        assert_eq!(refs_before_pull["behind"], 1);

        let pull_res = ops.git_pull(&project, repo, None, None).unwrap();
        assert_eq!(pull_res["fast_forwarded"], true);
        assert!(repo.join("remote_update.txt").exists());

        // Divergent history must reject `--ff-only` cleanly without merging.
        fs::write(peer.join("peer2.txt"), "peer 2\n").unwrap();
        git(&peer, &["add", "."]);
        git(&peer, &["commit", "-m", "feat: peer divergent"]);
        git(&peer, &["push", "origin", "main"]);

        fs::write(repo.join("local2.txt"), "local 2\n").unwrap();
        ops.git_commit(repo, "feat: local divergent", false, true)
            .unwrap();
        let local_head = git(repo, &["rev-parse", "HEAD"]);

        let err = ops.git_pull(&project, repo, None, None).unwrap_err();
        assert!(err.to_string().contains("Fast-forward pull"));
        assert_eq!(git(repo, &["rev-parse", "HEAD"]), local_head);
    }
}
