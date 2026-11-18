use anyhow::{Context, Result, ensure};
use canban_types::{Card, Project, WorkspaceInfo};
use std::path::{Path, PathBuf};
pub struct Ops {
    pub host: canban_herdr::host::Host,
}
impl Ops {
    pub fn local() -> Self {
        Self {
            host: canban_herdr::host::Host::local(),
        }
    }
    fn git(&self, dir: &Path, args: &[&str]) -> Result<std::process::Output> {
        let mut argv = vec!["git", "-C", dir.to_str().context("Invalid path")?];
        argv.extend(args);
        self.host.run_env(
            Path::new("/"),
            &[("GIT_EDITOR", "true"), ("GIT_SEQUENCE_EDITOR", "true")],
            &argv,
            None,
        )
    }
    fn checked(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let out = self.git(dir, args)?;
        ensure!(
            out.status.success(),
            "git {}: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8(out.stdout)?.trim_end_matches('\n').into())
    }
    pub fn branch_merged(&self, dir: &Path, branch: &str, base: &str) -> bool {
        self.git(dir, &["merge-base", "--is-ancestor", branch, base])
            .is_ok_and(|out| out.status.success())
    }
}
mod names;

pub fn card_branch(project: &Project, card: &Card) -> String {
    card.branch
        .clone()
        .unwrap_or_else(|| format!("{}/{}", names::namespace(project), card.number))
}
fn managed_card_path(home: &Path, project: &Project, card: &Card, path: &Path) -> bool {
    let worktrees = home.join("worktrees");
    let root = worktrees.join(&project.id);
    if path == root.join(card.number.to_string())
        || path == root.join(&card.id)
        || names::managed_path(home, card, path)
    {
        return true;
    }
    // Pre-migration managed worktrees could live under worktrees/<32-hex-project-id>/<number|card_id>.
    if let Ok(rel) = path.strip_prefix(&worktrees)
        && let Some(dir) = rel.parent().and_then(|p| p.to_str())
        && dir.len() == 32
        && dir.chars().all(|c| c.is_ascii_hexdigit())
        && dir
            .to_ascii_lowercase()
            .starts_with(&project.id.to_ascii_lowercase())
        && let Some(leaf) = rel.file_name().and_then(|l| l.to_str())
    {
        return leaf == card.number.to_string() || leaf == card.id;
    }
    false
}
impl Ops {
    fn sync_base(&self, source: &Path, base: &str, remote: &str) -> Result<()> {
        ensure!(
            !remote.is_empty() && !remote.starts_with('-'),
            "Invalid remote name"
        );
        self.checked(source, &["check-ref-format", "--branch", base])?;
        let local = format!("refs/heads/{base}");
        let tracking = format!("refs/remotes/{remote}/{base}");
        self.checked(
            source,
            &[
                "fetch",
                "--",
                remote,
                &format!("+refs/heads/{base}:{tracking}"),
            ],
        )?;
        let old = self.checked(source, &["rev-parse", "--verify", &local])?;
        let latest = self.checked(source, &["rev-parse", "--verify", &tracking])?;
        if self
            .git(source, &["merge-base", "--is-ancestor", &latest, &old])?
            .status
            .success()
        {
            return Ok(());
        }
        ensure!(
            self.git(source, &["merge-base", "--is-ancestor", &old, &latest])?
                .status
                .success(),
            "Base branch and remote have diverged; review them before continuing"
        );
        // A checked-out base must be updated through that checkout, never update-ref.
        let worktrees = self.checked(source, &["worktree", "list", "--porcelain"])?;
        for record in worktrees.split("\n\n") {
            if record.lines().any(|l| l == format!("branch {local}")) {
                let path = record
                    .lines()
                    .find_map(|l| l.strip_prefix("worktree "))
                    .context("Base checkout path missing")?;
                let cwd = Path::new(path);
                let base_status = self.r_checked(
                    cwd,
                    &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                )?;
                if !base_status.is_empty() {
                    return Err(review::DeliveryError::MainCheckoutBusy {
                        reason: format!(
                            "Main repository checkout ({}) has uncommitted local changes ({}); commit or stash them before synchronizing",
                            cwd.display(),
                            review::summarize_porcelain_z(&base_status, 4)
                        ),
                    }
                    .into());
                }
                ensure!(
                    self.checked(cwd, &["rev-parse", "HEAD"])? == old,
                    "Base changed during synchronization; retry after inspection"
                );
                self.checked(
                    cwd,
                    &["merge", "--ff-only", "--no-overwrite-ignore", &latest],
                )?;
                return Ok(());
            }
        }
        self.checked(source, &["update-ref", &local, &latest, &old])?;
        Ok(())
    }
    /// Best-effort fast-forward of the Project target branch when its checkout is clean.
    pub fn sync_target(&self, project: &Project, target_branch: Option<&str>) -> Result<()> {
        let source = Path::new(&project.directory);
        let base = target_branch
            .filter(|b| !b.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| project.workflow.workspace.base_branch.clone())
            .or_else(|| project.target_branch.clone())
            .or_else(|| self.current_branch(&project.directory))
            .context("Choose a base_branch for the workspace")?;
        let remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.as_str()
        };
        self.sync_base(source, &base, remote)
    }
    /// Returns true if the Project checkout has a configured push URL for its workspace remote.
    pub fn has_push_remote(&self, project: &Project) -> bool {
        let source = Path::new(&project.directory);
        let remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.as_str()
        };
        if remote.starts_with('-') {
            return false;
        }
        self.git(source, &["remote", "get-url", "--push", remote])
            .is_ok_and(|out| out.status.success())
    }
    /// Lists Git remote `(name, url)` pairs configured on `directory`, placing
    /// `preferred_remote` (default `"origin"`) first when present. Returns empty
    /// when `directory` does not exist or is not inside a Git repository.
    pub fn remote_urls(&self, directory: &str, preferred_remote: &str) -> Vec<(String, String)> {
        let dir = Path::new(directory.trim());
        if directory.trim().is_empty() || !dir.is_absolute() || !self.host.is_dir(dir) {
            return Vec::new();
        }
        if !self
            .git(dir, &["rev-parse", "--git-dir"])
            .is_ok_and(|out| out.status.success())
        {
            return Vec::new();
        }
        let preferred = if preferred_remote.trim().is_empty() || preferred_remote.starts_with('-') {
            "origin"
        } else {
            preferred_remote.trim()
        };
        let mut names = vec![preferred.to_owned()];
        if let Ok(list) = self.checked(dir, &["remote"]) {
            for line in list
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('-'))
            {
                if !names.iter().any(|existing| existing == line) {
                    names.push(line.to_owned());
                }
            }
        }
        let mut pairs = Vec::new();
        for remote in names {
            for args in [
                vec!["remote", "get-url", remote.as_str()],
                vec!["remote", "get-url", "--push", remote.as_str()],
            ] {
                if let Ok(url) = self.checked(dir, &args) {
                    let trimmed = url.trim();
                    if !trimmed.is_empty()
                        && !pairs.iter().any(|(r, u)| r == &remote && u == trimmed)
                    {
                        pairs.push((remote.clone(), trimmed.to_owned()));
                    }
                }
            }
        }
        pairs
    }
    /// Pushes the merged local `target_branch` to the configured upstream Git remote (`origin/<target_branch>`).
    /// Returns `Ok(Some(remote_name))` when pushed, or `Ok(None)` when no Git remote is configured.
    pub fn push_target_branch(
        &self,
        project: &Project,
        target_branch: &str,
    ) -> Result<Option<String>> {
        let source = Path::new(&project.directory);
        let remote = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.as_str()
        };
        ensure!(
            !remote.is_empty() && !remote.starts_with('-'),
            "Invalid remote name"
        );
        self.checked(source, &["check-ref-format", "--branch", target_branch])?;
        if !self.has_push_remote(project) {
            return Ok(None);
        }
        self.checked(
            source,
            &[
                "push",
                "--porcelain",
                remote,
                &format!("refs/heads/{target_branch}:refs/heads/{target_branch}"),
            ],
        )?;
        Ok(Some(remote.to_owned()))
    }
    /// Resume always returns to the saved location. A removed managed Card
    /// checkout can be reconstructed from its retained branch/commit, without
    /// fetching a new base or falling back to a different directory.
    pub fn resume(&self, home: &Path, project: &Project, card: &Card) -> Result<PathBuf> {
        let path = PathBuf::from(
            card.worktree
                .as_ref()
                .context("Original working directory is missing")?,
        );
        if self.host.is_dir(&path) {
            return Ok(path);
        }
        ensure!(
            managed_card_path(home, project, card, &path),
            "Original working directory is missing; only a managed Card worktree can be restored"
        );
        let source = Path::new(&project.directory);
        let branch = card
            .branch
            .as_ref()
            .context("Original Card branch is unavailable; inspect before resuming")?;
        self.checked(source, &["check-ref-format", "--branch", branch])?;
        let retained = format!("refs/canban/cards/{}", card.id);
        let branch_ref = format!("refs/heads/{branch}");
        ensure!(
            self.git(
                source,
                &["rev-parse", "--verify", &format!("{retained}^{{commit}}")]
            )?
            .status
            .success()
                || self
                    .git(
                        source,
                        &["rev-parse", "--verify", &format!("{branch_ref}^{{commit}}")]
                    )?
                    .status
                    .success(),
            "Original Card branch and retained commit are unavailable; Resume cannot start from a fresh base"
        );
        let mut saved = project.clone();
        saved.workflow.workspace.create_worktree = true;
        saved.workflow.workspace.sync_base = false;
        let actual = self.prepare(home, &saved, card)?;
        ensure!(
            actual == path,
            "Resume must restore the original Card directory"
        );
        Ok(actual)
    }
    pub fn prepare(&self, home: &Path, project: &Project, card: &Card) -> Result<PathBuf> {
        let source = Path::new(&project.directory);
        let branch = card_branch(project, card);
        if let Some(path) = &card.worktree {
            let p = PathBuf::from(path);
            if self.host.is_dir(&p) {
                if self.host.canonical(&p)? != self.host.canonical(source)?
                    && !self.in_merge_or_rebase(&p)?
                    && self.current_branch(path).is_none()
                    && !self
                        .git(
                            source,
                            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                        )?
                        .status
                        .success()
                {
                    // Adopt legacy detached checkouts at their current HEAD, preserving edits.
                    self.checked(&p, &["switch", "-c", &branch])?;
                }
                return Ok(p);
            }
            ensure!(
                managed_card_path(home, project, card, &p),
                "Saved working directory is missing: {}",
                p.display()
            );
        }
        if !project.workflow.workspace.create_worktree {
            return Ok(source.into());
        }
        if !self
            .git(source, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success()
        {
            ensure!(
                !project.workflow.workspace.sync_base,
                "Initialize a Git repository with a base commit, or disable worktree/sync_base in the template"
            );
            return Ok(source.into());
        }
        let keep = format!("refs/canban/cards/{}", card.id);
        if card.branch.is_none()
            && card.worktree.is_none()
            && !self
                .git(source, &["rev-parse", "--verify", &keep])?
                .status
                .success()
        {
            let start = self.workspace_start(source, project, &keep)?;
            return self.create_named(home, project, card, &start);
        }
        let target = card
            .worktree
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                home.join("worktrees")
                    .join(&project.id)
                    .join(card.number.to_string())
            });
        self.host.mkdir(target.parent().unwrap())?;
        let branch_ref = format!("refs/heads/{branch}");
        if self
            .git(source, &["rev-parse", "--verify", &branch_ref])?
            .status
            .success()
        {
            ensure!(
                card.worktree.is_some()
                    || self
                        .git(source, &["rev-parse", "--verify", &keep])?
                        .status
                        .success(),
                "Card branch already exists without a saved binding; inspect it before retrying"
            );
            self.checked(
                source,
                &["worktree", "add", target.to_str().unwrap(), &branch],
            )?;
        } else {
            let start = self.workspace_start(source, project, &keep)?;
            self.checked(
                source,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    target.to_str().unwrap(),
                    &start,
                ],
            )?;
        }
        Ok(target)
    }
    fn workspace_start(&self, source: &Path, project: &Project, keep: &str) -> Result<String> {
        if self
            .git(source, &["rev-parse", "--verify", keep])?
            .status
            .success()
        {
            return Ok(keep.into());
        }
        let base = project
            .workflow
            .workspace
            .base_branch
            .clone()
            .or_else(|| project.target_branch.clone())
            .or_else(|| self.current_branch(&project.directory))
            .context("Choose a base_branch for the workspace")?;
        self.checked(source, &["check-ref-format", "--branch", &base])?;
        if project.workflow.workspace.sync_base {
            self.sync_base(source, &base, &project.workflow.workspace.remote)?;
        }
        Ok(format!("refs/heads/{base}"))
    }
    /// Read-only projection for the Card's workspace panel.
    pub fn info(&self, project: &Project, card: &Card) -> Result<WorkspaceInfo> {
        let mut value = WorkspaceInfo {
            path: card.worktree.clone(),
            branch: card.branch.clone(),
            target_branch: card
                .workspace
                .as_ref()
                .and_then(|w| w.target_branch.clone())
                .or_else(|| project.target_branch.clone())
                .or_else(|| project.workflow.workspace.base_branch.clone()),
            upstream: None,
            base_commit: card.base_commit.clone(),
            state: "not_created".into(),
            dirty: false,
        };
        if let Some(path) = &card.worktree {
            let cwd = Path::new(path);
            value.state = if !self.host.is_dir(cwd) {
                "missing"
            } else if !self
                .git(cwd, &["rev-parse", "--verify", "HEAD"])?
                .status
                .success()
            {
                "directory"
            } else {
                "available"
            }
            .into();
            if value.state == "available" {
                value.branch = self
                    .current_branch(path)
                    .or_else(|| self.rebase_branch(cwd));
                if value.branch.is_some() {
                    value.upstream = self
                        .checked(
                            cwd,
                            &[
                                "rev-parse",
                                "--abbrev-ref",
                                "--symbolic-full-name",
                                "@{upstream}",
                            ],
                        )
                        .ok()
                        .filter(|s| !s.is_empty());
                }
                value.base_commit = card
                    .base_commit
                    .clone()
                    .or_else(|| self.checked(cwd, &["rev-parse", "HEAD"]).ok());
                value.dirty = !self
                    .checked(cwd, &["status", "--porcelain", "--untracked-files=all"])?
                    .is_empty();
                if !self
                    .checked(cwd, &["diff", "--name-only", "--diff-filter=U"])?
                    .is_empty()
                    || self.in_merge_or_rebase(cwd)?
                {
                    value.state = "conflict".into();
                }
            }
        }
        Ok(value)
    }
    pub fn archive(&self, home: &Path, project: &Project, card: &Card) -> Result<Option<String>> {
        let Some(path) = &card.worktree else {
            return Ok(None);
        };
        let p = Path::new(path);
        if !p.starts_with(home.join("worktrees")) || !self.host.exists(p) {
            return Ok(None);
        }
        let status = self.git(p, &["status", "--porcelain"])?;
        if !status.status.success() || !status.stdout.is_empty() {
            return Ok(Some(
                "Managed worktree retained: it contains changes or could not be checked".into(),
            ));
        }
        // Retain commits not reachable from the source HEAD before removing the checkout.
        let keep = format!("refs/canban/cards/{}", card.id);
        let sha = self.git(p, &["rev-parse", "HEAD"])?;
        let sha = String::from_utf8(sha.stdout)?;
        let out = self.git(
            Path::new(&project.directory),
            &["update-ref", &keep, sha.trim()],
        )?;
        ensure!(out.status.success(), "Could not retain Card commit");
        let out = self.git(
            Path::new(&project.directory),
            &["worktree", "remove", p.to_str().unwrap()],
        )?;
        ensure!(
            out.status.success(),
            "Could not remove clean worktree: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(None)
    }
    /// Create or attach a standalone Canban-managed branch worktree under `<home>/worktrees`.
    pub fn create_branch_worktree(
        &self,
        home: &Path,
        project: &Project,
        branch: &str,
        base_branch: Option<&str>,
        sync_base: bool,
    ) -> Result<(PathBuf, String, String)> {
        let source = Path::new(&project.directory);
        let branch = branch.trim();
        ensure!(!branch.is_empty(), "Branch name is required");
        ensure!(
            self.git(source, &["rev-parse", "--verify", "HEAD"])?
                .status
                .success(),
            "Initialize a Git repository with at least one commit before creating a worktree"
        );
        self.checked(source, &["check-ref-format", "--branch", branch])?;
        let base = base_branch
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_owned)
            .or_else(|| project.workflow.workspace.base_branch.clone())
            .or_else(|| project.target_branch.clone())
            .or_else(|| self.current_branch(&project.directory))
            .context("Choose a base branch for the worktree")?;
        self.checked(source, &["check-ref-format", "--branch", &base])?;
        if sync_base {
            let remote = if project.workflow.workspace.remote.trim().is_empty() {
                "origin"
            } else {
                project.workflow.workspace.remote.as_str()
            };
            if self.has_push_remote(project) {
                self.sync_base(source, &base, remote)?;
            }
        }
        let start_ref = format!("refs/heads/{base}");
        let base_commit = self.checked(source, &["rev-parse", "--verify", &start_ref])?;
        let target = self.create_standalone(home, project, branch, &start_ref)?;
        Ok((target, base, base_commit))
    }
    /// Inspect clean/dirty status and current branch of a standalone worktree.
    pub fn worktree_status(&self, path: &Path) -> Result<(String, bool, Option<String>)> {
        if !self.host.is_dir(path) {
            return Ok(("missing".into(), false, None));
        }
        if !self
            .git(path, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success()
        {
            return Ok(("directory".into(), false, None));
        }
        let branch = self
            .current_branch(path.to_str().unwrap_or_default())
            .or_else(|| self.rebase_branch(path));
        let dirty = !self
            .checked(path, &["status", "--porcelain", "--untracked-files=all"])?
            .is_empty();
        Ok(("available".into(), dirty, branch))
    }
    /// Remove a standalone managed worktree directory from Git.
    pub fn remove_worktree(
        &self,
        home: &Path,
        source: &Path,
        path: &Path,
        force: bool,
    ) -> Result<()> {
        ensure!(
            path.starts_with(home.join("worktrees")),
            "Refusing to remove an unmanaged directory outside worktrees: {}",
            path.display()
        );
        if !self.host.exists(path) {
            let _ = self.git(source, &["worktree", "prune"]);
            return Ok(());
        }
        if !force {
            let dirty = !self
                .checked(path, &["status", "--porcelain", "--untracked-files=all"])?
                .is_empty();
            ensure!(
                !dirty,
                "Worktree contains uncommitted changes; confirm force deletion or commit/stash changes first"
            );
        }
        let path_str = path.to_str().context("Invalid worktree path")?;
        let args: Vec<&str> = if force {
            vec!["worktree", "remove", "--force", path_str]
        } else {
            vec!["worktree", "remove", path_str]
        };
        self.checked(source, &args)?;
        Ok(())
    }
}
pub fn prepare(home: &Path, project: &Project, card: &Card) -> Result<PathBuf> {
    Ops::local().prepare(home, project, card)
}
pub fn info(project: &Project, card: &Card) -> Result<WorkspaceInfo> {
    Ops::local().info(project, card)
}
pub fn archive(home: &Path, project: &Project, card: &Card) -> Result<Option<String>> {
    Ops::local().archive(home, project, card)
}
#[cfg(test)]
fn checked(dir: &Path, args: &[&str]) -> Result<String> {
    Ops::local().checked(dir, args)
}
#[cfg(test)]
fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ops::local().git(dir, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    #[test]
    fn worktree_reuses_retains_changes_and_preserves_commits_on_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init"]);
        std::fs::write(source.join("seed.txt"), "seed").unwrap();
        run(&["add", "seed.txt"]);
        run(&[
            "-c",
            "user.name=Acceptance",
            "-c",
            "user.email=acceptance@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-m",
            "seed",
        ]);
        let mut workflow = canban_types::Workflow::default();
        workflow.workspace.sync_base = false;
        let project = Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            id: "project".into(),
            name: "Test".into(),
            location: None,
            directory: source.to_str().unwrap().into(),
            workflow,
            target_branch: None,
            created_at: 0,
        };
        let mut card = Card {
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
            id: "card".into(),
            project_id: project.id.clone(),
            number: 1,
            board_order: None,
            title: "Task".into(),
            body: String::new(),
            context_notes: None,
            stage: "develop".into(),
            revision: 1,
            attention: None,
            archived_at: None,
            archive: None,
            archive_error: None,
            workspace: None,
            workspace_history: Vec::new(),
            worktree: None,
            branch: None,
            base_commit: None,
            human_gate: None,
            delivery: None,
            created_at: 0,
        };
        let home = dir.path().join("home");
        let path = prepare(&home, &project, &card).unwrap();
        card.worktree = Some(path.to_string_lossy().into_owned());
        card.branch = review::current_branch(path.to_str().unwrap());
        assert_eq!(prepare(&home, &project, &card).unwrap(), path);
        std::fs::write(path.join("change.txt"), "keep me").unwrap();
        assert!(archive(&home, &project, &card).unwrap().is_some());
        assert!(path.exists());
        let commit = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["add", "change.txt"])
            .status()
            .unwrap();
        assert!(commit.success());
        let commit = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args([
                "-c",
                "user.name=Acceptance",
                "-c",
                "user.email=acceptance@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-m",
                "task",
            ])
            .output()
            .unwrap();
        assert!(commit.status.success());
        assert!(archive(&home, &project, &card).unwrap().is_none());
        assert!(!path.exists());
        run(&["rev-parse", "--verify", "refs/canban/cards/card"]);
        assert_eq!(prepare(&home, &project, &card).unwrap(), path);
        assert_eq!(
            std::fs::read_to_string(path.join("change.txt")).unwrap(),
            "keep me"
        );
        assert!(source.join("seed.txt").exists());
    }
    #[test]
    fn remote_urls_prioritizes_preferred_remote_and_handles_non_git_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ops = Ops::local();
        assert!(
            ops.remote_urls(dir.path().to_str().unwrap(), "origin")
                .is_empty()
        );
        assert!(
            ops.remote_urls("/nonexistent/path/for/canban", "origin")
                .is_empty()
        );

        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        checked(&repo, &["init", "-b", "main"]).unwrap();
        checked(
            &repo,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/org/upstream.git",
            ],
        )
        .unwrap();
        checked(
            &repo,
            &["remote", "add", "origin", "git@github.com:jjyr/canban.git"],
        )
        .unwrap();

        let urls = ops.remote_urls(repo.to_str().unwrap(), "origin");
        assert_eq!(
            urls.first().map(|(r, u)| (r.as_str(), u.as_str())),
            Some(("origin", "git@github.com:jjyr/canban.git"))
        );
        assert!(
            urls.iter()
                .any(|(r, u)| r == "upstream" && u == "https://github.com/org/upstream.git")
        );
    }
}

pub mod batch;
pub mod compare;
pub mod context;
pub mod git_client;
pub mod review;

#[cfg(test)]
mod sync_tests {
    use super::*;
    use std::fs;
    struct Fixture {
        dir: tempfile::TempDir,
        source: PathBuf,
        writer: PathBuf,
        project: Project,
        card: Card,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            let writer = dir.path().join("writer");
            let remote = dir.path().join("remote.git");
            fs::create_dir(&source).unwrap();
            checked(&source, &["init", "--bare", remote.to_str().unwrap()]).unwrap();
            checked(&source, &["init", "-b", "main"]).unwrap();
            for (k, v) in [
                ("user.name", "Test"),
                ("user.email", "test@example.invalid"),
                ("core.hooksPath", "/dev/null"),
                ("commit.gpgsign", "false"),
            ] {
                checked(&source, &["config", k, v]).unwrap();
            }
            fs::write(source.join("seed"), "seed").unwrap();
            checked(&source, &["add", "."]).unwrap();
            checked(&source, &["commit", "-m", "seed"]).unwrap();
            checked(
                &source,
                &["remote", "add", "origin", remote.to_str().unwrap()],
            )
            .unwrap();
            checked(&source, &["push", "-u", "origin", "main"]).unwrap();
            checked(
                &source,
                &[
                    "clone",
                    "-b",
                    "main",
                    remote.to_str().unwrap(),
                    writer.to_str().unwrap(),
                ],
            )
            .unwrap();
            for (k, v) in [
                ("user.name", "Test"),
                ("user.email", "test@example.invalid"),
                ("core.hooksPath", "/dev/null"),
                ("commit.gpgsign", "false"),
            ] {
                checked(&writer, &["config", k, v]).unwrap();
            }
            let project = Project {
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Default::default(),
                id: "p".into(),
                name: "Test".into(),
                location: None,
                directory: source.to_string_lossy().into(),
                workflow: Default::default(),
                target_branch: Some("main".into()),
                created_at: 0,
            };
            let card:Card=serde_json::from_value(serde_json::json!({"id":"c","project_id":"p","number":42,"title":"Task","body":"","stage":"develop","revision":1,"created_at":0})).unwrap();
            Self {
                dir,
                source,
                writer,
                project,
                card,
            }
        }
        fn remote_change(&self) -> String {
            fs::write(self.writer.join("remote-change"), "latest").unwrap();
            checked(&self.writer, &["add", "."]).unwrap();
            checked(&self.writer, &["commit", "-m", "remote change"]).unwrap();
            checked(&self.writer, &["push", "origin", "main"]).unwrap();
            checked(&self.writer, &["rev-parse", "HEAD"]).unwrap()
        }
        fn prepare(&self) -> Result<PathBuf> {
            prepare(&self.dir.path().join("home"), &self.project, &self.card)
        }
    }
    #[test]
    fn directory_and_short_project_id_names_survive_short_id_collisions_and_renames() {
        let mut f = Fixture::new();
        f.project.id = "4e5r-first".into();
        f.card.project_id = f.project.id.clone();
        f.project.name = "项目显示名与目录无关".into();
        f.card.title = "设置 / 调整 .. @{invalid}".into();
        let cwd = f.prepare().unwrap();
        assert_eq!(cwd, f.dir.path().join("home/worktrees/source-4e5r/42"));
        assert_eq!(
            review::current_branch(cwd.to_str().unwrap()).as_deref(),
            Some("source-4e5r/42")
        );
        let mut other = f.project.clone();
        other.id = "4e5r-second".into();
        let mut card = f.card.clone();
        card.id = "second-card".into();
        card.project_id = other.id.clone();
        let home = f.dir.path().join("home");
        let second = prepare(&home, &other, &card).unwrap();
        assert_eq!(second, home.join("worktrees/source-4e5r/42-2"));
        card.worktree = Some(second.to_string_lossy().into());
        card.branch = Some("source-4e5r/42-2".into());
        assert_eq!(
            review::current_branch(second.to_str().unwrap()),
            card.branch
        );
        card.title = "A changed title".into();
        other.name = "Renamed Project".into();
        assert_eq!(prepare(&home, &other, &card).unwrap(), second);
        archive(&home, &other, &card).unwrap();
        assert_eq!(Ops::local().resume(&home, &other, &card).unwrap(), second);
        assert_eq!(
            review::current_branch(second.to_str().unwrap()),
            card.branch
        );
    }
    #[test]
    fn occupied_branches_directories_and_registered_missing_paths_choose_matching_suffixes() {
        let f = Fixture::new();
        let home = f.dir.path().join("home");
        let root = home.join("worktrees/source-p");
        fs::create_dir_all(root.join("42")).unwrap();
        fs::write(root.join("42/keep"), "unrelated").unwrap();
        checked(&f.source, &["branch", "source-p/42-2"]).unwrap();
        // Descendant refs also prevent a leaf from being created.
        checked(&f.source, &["branch", "source-p/42-3/nested"]).unwrap();
        let missing = root.join("42-4");
        checked(
            &f.source,
            &[
                "worktree",
                "add",
                "-b",
                "unrelated",
                missing.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
        fs::remove_dir_all(&missing).unwrap();
        let result = f.prepare().unwrap();
        assert_eq!(result, root.join("42-5"));
        assert_eq!(
            review::current_branch(result.to_str().unwrap()).as_deref(),
            Some("source-p/42-5")
        );
        assert_eq!(
            fs::read_to_string(root.join("42/keep")).unwrap(),
            "unrelated"
        );
        assert!(!missing.exists());
    }
    #[test]
    fn namespace_ref_file_and_symlink_conflicts_are_preserved() {
        let f = Fixture::new();
        let home = f.dir.path().join("home");
        checked(&f.source, &["branch", "source-p"]).unwrap();
        let root = home.join("worktrees");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("source-p-2"), "keep").unwrap();
        fs::create_dir(root.join("source-p-3")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/canban-worktree", root.join("source-p-3/42"))
            .unwrap();
        let result = f.prepare().unwrap();
        assert_eq!(result, root.join("source-p-3/42-2"));
        assert_eq!(
            review::current_branch(result.to_str().unwrap()).as_deref(),
            Some("source-p-3/42-2")
        );
        assert_eq!(fs::read_to_string(root.join("source-p-2")).unwrap(), "keep");
        assert!(
            fs::symlink_metadata(root.join("source-p-3/42"))
                .unwrap()
                .is_symlink()
        );
    }
    #[test]
    fn concurrent_creators_do_not_adopt_each_others_refs_or_paths() {
        let f = Fixture::new();
        let mut project = f.project.clone();
        project.workflow.workspace.sync_base = false;
        let home = f.dir.path().join("home");
        let barrier = std::sync::Barrier::new(4);
        let paths = std::thread::scope(|scope| {
            let handles = (0..4)
                .map(|i| {
                    let project = &project;
                    let home = &home;
                    let barrier = &barrier;
                    let mut card = f.card.clone();
                    card.id = format!("concurrent-{i}");
                    scope.spawn(move || {
                        barrier.wait();
                        prepare(home, project, &card).unwrap()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<std::collections::BTreeSet<_>>()
        });
        assert_eq!(paths.len(), 4);
        for path in paths {
            let branch = review::current_branch(path.to_str().unwrap()).unwrap();
            assert_eq!(path, home.join("worktrees").join(branch));
        }
    }
    #[test]
    fn concurrent_creators_with_different_homes_reserve_distinct_git_branches() {
        let f = Fixture::new();
        let mut project = f.project.clone();
        project.workflow.workspace.sync_base = false;
        let barrier = std::sync::Barrier::new(4);
        let branches = std::thread::scope(|scope| {
            let handles = (0..4)
                .map(|i| {
                    let project = &project;
                    let home = f.dir.path().join(format!("home-{i}"));
                    let barrier = &barrier;
                    let mut card = f.card.clone();
                    card.id = format!("concurrent-{i}");
                    scope.spawn(move || {
                        barrier.wait();
                        let path = prepare(&home, project, &card).unwrap();
                        let branch = review::current_branch(path.to_str().unwrap()).unwrap();
                        assert_eq!(path, home.join("worktrees").join(&branch));
                        branch
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<std::collections::BTreeSet<_>>()
        });
        assert_eq!(branches.len(), 4);
    }
    #[test]
    fn legacy_numeric_path_and_branch_restore_without_renaming() {
        let mut f = Fixture::new();
        let home = f.dir.path().join("home");
        let path = home.join("worktrees/p/42");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        checked(
            &f.source,
            &[
                "worktree",
                "add",
                "-b",
                "canban/42",
                path.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
        f.card.worktree = Some(path.to_string_lossy().into());
        f.card.branch = Some("canban/42".into());
        archive(&home, &f.project, &f.card).unwrap();
        assert_eq!(
            Ops::local().resume(&home, &f.project, &f.card).unwrap(),
            path
        );
        assert_eq!(
            review::current_branch(path.to_str().unwrap()),
            f.card.branch
        );
    }
    #[test]
    fn legacy_uuid_checkout_and_branch_resume_at_the_exact_saved_binding() {
        let mut f = Fixture::new();
        let home = f.dir.path().join("home");
        let path = home.join("worktrees/p/c");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        checked(
            &f.source,
            &[
                "worktree",
                "add",
                "-b",
                "canban/p/42",
                path.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
        f.card.worktree = Some(path.to_string_lossy().into());
        f.card.branch = Some("canban/p/42".into());
        let original = checked(&path, &["rev-parse", "HEAD"]).unwrap();
        archive(&home, &f.project, &f.card).unwrap();
        assert!(!path.exists());
        assert_eq!(
            Ops::local().resume(&home, &f.project, &f.card).unwrap(),
            path
        );
        assert_eq!(checked(&path, &["rev-parse", "HEAD"]).unwrap(), original);
        assert_eq!(
            review::current_branch(path.to_str().unwrap()).as_deref(),
            Some("canban/p/42")
        );
    }
    #[test]
    fn resume_restores_archived_checkout_without_fetch_or_new_base_and_rejects_missing_history() {
        let mut f = Fixture::new();
        let cwd = f.prepare().unwrap();
        f.card.worktree = Some(cwd.to_string_lossy().into());
        f.card.branch = Some("source-p/42".into());
        let original = checked(&cwd, &["rev-parse", "HEAD"]).unwrap();
        let home = f.dir.path().join("home");
        archive(&home, &f.project, &f.card).unwrap();
        assert!(!cwd.exists());
        // Rebuild even if the ordinary branch was removed: Archive's retained
        // Card ref is authoritative, rather than the now-current Project base.
        checked(&f.source, &["branch", "-D", "source-p/42"]).unwrap();
        // The template can change after archiving. Resume uses the saved binding.
        f.project.workflow.workspace.create_worktree = false;
        checked(&f.source, &["remote", "remove", "origin"]).unwrap();
        let restored = Ops::local().resume(&home, &f.project, &f.card).unwrap();
        assert_eq!(restored, cwd);
        assert_eq!(checked(&cwd, &["rev-parse", "HEAD"]).unwrap(), original);
        archive(&home, &f.project, &f.card).unwrap();
        checked(&f.source, &["update-ref", "-d", "refs/canban/cards/c"]).unwrap();
        checked(&f.source, &["branch", "-D", "source-p/42"]).unwrap();
        assert!(
            Ops::local()
                .resume(&home, &f.project, &f.card)
                .unwrap_err()
                .to_string()
                .contains("fresh base")
        );
        assert!(!cwd.exists());
        let outside = f.dir.path().join("removed-manual-directory");
        f.card.worktree = Some(outside.to_string_lossy().into());
        assert!(
            Ops::local()
                .resume(&home, &f.project, &f.card)
                .unwrap_err()
                .to_string()
                .contains("only a managed")
        );
        assert!(!outside.exists());
    }
    #[test]
    fn fresh_worktree_syncs_base_and_keeps_branch_across_archive_restore() {
        let mut f = Fixture::new();
        let latest = f.remote_change();
        let cwd = f.prepare().unwrap();
        assert_eq!(checked(&f.source, &["rev-parse", "HEAD"]).unwrap(), latest);
        assert_eq!(
            review::current_branch(cwd.to_str().unwrap()).as_deref(),
            Some("source-p/42")
        );
        assert!(cwd.join("remote-change").exists());
        f.card.worktree = Some(cwd.to_string_lossy().into());
        f.card.branch = Some("source-p/42".into());
        checked(&f.source, &["remote", "remove", "origin"]).unwrap();
        assert_eq!(f.prepare().unwrap(), cwd); // Existing work never re-fetches.
        archive(&f.dir.path().join("home"), &f.project, &f.card).unwrap();
        assert!(!cwd.exists());
        assert_eq!(
            checked(&f.source, &["rev-parse", "refs/heads/source-p/42"]).unwrap(),
            latest
        );
        assert_eq!(f.prepare().unwrap(), cwd);
        assert!(cwd.join("remote-change").exists());
    }
    #[test]
    fn divergence_dirty_base_and_fetch_failure_never_create_stale_work() {
        let f = Fixture::new();
        let before = checked(&f.source, &["rev-parse", "HEAD"]).unwrap();
        f.remote_change();
        fs::write(f.source.join("human-draft"), "keep").unwrap();
        assert!(
            f.prepare()
                .unwrap_err()
                .to_string()
                .contains("local changes")
        );
        assert_eq!(checked(&f.source, &["rev-parse", "HEAD"]).unwrap(), before);
        checked(&f.source, &["add", "."]).unwrap();
        checked(&f.source, &["commit", "-m", "local change"]).unwrap();
        assert!(f.prepare().unwrap_err().to_string().contains("diverged"));
        checked(
            &f.source,
            &[
                "remote",
                "set-url",
                "origin",
                "/nonexistent/canban-test-remote",
            ],
        )
        .unwrap();
        assert!(f.prepare().is_err());
        assert!(
            !git(
                &f.source,
                &["show-ref", "--verify", "refs/heads/source-p/42"]
            )
            .unwrap()
            .status
            .success()
        );
        assert_eq!(
            fs::read_to_string(f.source.join("human-draft")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn base_checked_out_elsewhere_updates_that_checkout_and_leaves_source_branch_alone() {
        let f = Fixture::new();
        let checkout = f.dir.path().join("base-checkout");
        checked(&f.source, &["switch", "-c", "other"]).unwrap();
        let other = checked(&f.source, &["rev-parse", "HEAD"]).unwrap();
        checked(
            &f.source,
            &["worktree", "add", checkout.to_str().unwrap(), "main"],
        )
        .unwrap();
        let latest = f.remote_change();
        let cwd = f.prepare().unwrap();
        assert_eq!(checked(&checkout, &["rev-parse", "HEAD"]).unwrap(), latest);
        assert_eq!(checked(&f.source, &["rev-parse", "HEAD"]).unwrap(), other);
        assert_eq!(
            review::current_branch(f.source.to_str().unwrap()).as_deref(),
            Some("other")
        );
        assert_eq!(checked(&cwd, &["rev-parse", "HEAD"]).unwrap(), latest);
    }
    #[test]
    fn legacy_detached_worktree_is_adopted_without_losing_edits() {
        let mut f = Fixture::new();
        let cwd = f.dir.path().join("legacy");
        checked(
            &f.source,
            &["worktree", "add", "--detach", cwd.to_str().unwrap(), "HEAD"],
        )
        .unwrap();
        fs::write(cwd.join("draft"), "preserve").unwrap();
        let head = checked(&cwd, &["rev-parse", "HEAD"]).unwrap();
        f.card.worktree = Some(cwd.to_string_lossy().into());
        assert_eq!(f.prepare().unwrap(), cwd);
        assert_eq!(checked(&cwd, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            review::current_branch(cwd.to_str().unwrap()).as_deref(),
            Some("source-p/42")
        );
        assert_eq!(fs::read_to_string(cwd.join("draft")).unwrap(), "preserve");
        assert!(info(&f.project, &f.card).unwrap().dirty);
    }
    #[test]
    fn mid_rebase_worktree_preserves_conflict_and_branch_identity() {
        let mut f = Fixture::new();
        f.project.workflow.workspace.sync_base = false;
        let cwd = f.prepare().unwrap();
        let ops = Ops::local();
        let branch = review::current_branch(cwd.to_str().unwrap()).unwrap();
        assert_eq!(branch, "source-p/42");
        fs::write(cwd.join("seed"), "card edit\n").unwrap();
        checked(&cwd, &["add", "seed"]).unwrap();
        checked(&cwd, &["commit", "-m", "card edit"]).unwrap();

        fs::write(f.source.join("seed"), "main edit\n").unwrap();
        checked(&f.source, &["add", "seed"]).unwrap();
        checked(&f.source, &["commit", "-m", "main edit"]).unwrap();

        let rebase = git(&cwd, &["rebase", "main"]).unwrap();
        assert!(!rebase.status.success());
        assert!(ops.in_rebase(&cwd).unwrap());
        assert_eq!(review::current_branch(cwd.to_str().unwrap()), None);

        f.card.worktree = Some(cwd.to_string_lossy().into());
        f.card.branch = Some(branch.clone());
        assert_eq!(f.prepare().unwrap(), cwd);
        assert!(ops.in_rebase(&cwd).unwrap());

        let ws = info(&f.project, &f.card).unwrap();
        assert_eq!(ws.branch.as_deref(), Some(branch.as_str()));
        assert_eq!(ws.state, "conflict");
        assert_eq!(
            ops.worktree_status(&cwd).unwrap().2.as_deref(),
            Some(branch.as_str())
        );
    }
}

mod publish;
