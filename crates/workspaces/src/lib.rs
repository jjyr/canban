use anyhow::{Context, Result, ensure};
use canban_types::{Card, Project, WorkspaceInfo};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()?)
}
fn checked(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git(dir, args)?;
    ensure!(
        out.status.success(),
        "git {}: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?.trim_end_matches('\n').into())
}
pub fn card_branch(project: &Project, card: &Card) -> String {
    card.branch
        .clone()
        .unwrap_or_else(|| format!("canban/{}/{}", project.id, card.number))
}
fn sync_base(source: &Path, base: &str, remote: &str) -> Result<()> {
    ensure!(
        !remote.is_empty() && !remote.starts_with('-'),
        "Invalid remote name"
    );
    checked(source, &["check-ref-format", "--branch", base])?;
    let local = format!("refs/heads/{base}");
    let tracking = format!("refs/remotes/{remote}/{base}");
    checked(
        source,
        &[
            "fetch",
            "--",
            remote,
            &format!("+refs/heads/{base}:{tracking}"),
        ],
    )?;
    let old = checked(source, &["rev-parse", "--verify", &local])?;
    let latest = checked(source, &["rev-parse", "--verify", &tracking])?;
    if git(source, &["merge-base", "--is-ancestor", &latest, &old])?
        .status
        .success()
    {
        return Ok(());
    }
    ensure!(
        git(source, &["merge-base", "--is-ancestor", &old, &latest])?
            .status
            .success(),
        "Base branch and remote have diverged; review them before continuing"
    );
    // A checked-out base must be updated through that checkout, never update-ref.
    let worktrees = checked(source, &["worktree", "list", "--porcelain"])?;
    for record in worktrees.split("\n\n") {
        if record.lines().any(|l| l == format!("branch {local}")) {
            let path = record
                .lines()
                .find_map(|l| l.strip_prefix("worktree "))
                .context("Base checkout path missing")?;
            let cwd = Path::new(path);
            ensure!(
                checked(cwd, &["status", "--porcelain", "--untracked-files=all"])?.is_empty(),
                "Base checkout has local changes; preserve them before synchronizing"
            );
            ensure!(
                checked(cwd, &["rev-parse", "HEAD"])? == old,
                "Base changed during synchronization; retry after inspection"
            );
            checked(
                cwd,
                &["merge", "--ff-only", "--no-overwrite-ignore", &latest],
            )?;
            return Ok(());
        }
    }
    checked(source, &["update-ref", &local, &latest, &old])?;
    Ok(())
}
pub fn prepare(home: &Path, project: &Project, card: &Card) -> Result<PathBuf> {
    let source = Path::new(&project.directory);
    let branch = card_branch(project, card);
    if let Some(path) = &card.worktree {
        let p = PathBuf::from(path);
        if p.is_dir() {
            if p.canonicalize()? != source.canonicalize()? && review::current_branch(path).is_none()
            {
                // Adopt legacy detached checkouts at their current HEAD, preserving edits.
                checked(&p, &["switch", "-c", &branch])?;
            }
            return Ok(p);
        }
        ensure!(
            p == home.join("worktrees").join(&project.id).join(&card.id),
            "Saved working directory is missing: {}",
            p.display()
        );
    }
    if !project.workflow.workspace.create_worktree {
        return Ok(source.into());
    }
    if !git(source, &["rev-parse", "--verify", "HEAD"])?
        .status
        .success()
    {
        ensure!(
            !project.workflow.workspace.sync_base,
            "Initialize a Git repository with a base commit, or disable worktree/sync_base in the template"
        );
        return Ok(source.into());
    }
    let target = home.join("worktrees").join(&project.id).join(&card.id);
    std::fs::create_dir_all(target.parent().unwrap())?;
    let keep = format!("refs/canban/cards/{}", card.id);
    let branch_ref = format!("refs/heads/{branch}");
    if git(source, &["rev-parse", "--verify", &branch_ref])?
        .status
        .success()
    {
        ensure!(
            card.worktree.is_some()
                || git(source, &["rev-parse", "--verify", &keep])?
                    .status
                    .success(),
            "Card branch already exists without a saved binding; inspect it before retrying"
        );
        checked(
            source,
            &["worktree", "add", target.to_str().unwrap(), &branch],
        )?;
    } else {
        let start = if git(source, &["rev-parse", "--verify", &keep])?
            .status
            .success()
        {
            keep
        } else {
            let base = project
                .workflow
                .workspace
                .base_branch
                .clone()
                .or_else(|| project.target_branch.clone())
                .or_else(|| review::current_branch(&project.directory))
                .context("Choose a base_branch for the workspace")?;
            checked(source, &["check-ref-format", "--branch", &base])?;
            if project.workflow.workspace.sync_base {
                sync_base(source, &base, &project.workflow.workspace.remote)?;
            }
            format!("refs/heads/{base}")
        };
        checked(
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
/// Read-only projection for the Card's workspace panel.
pub fn info(project: &Project, card: &Card) -> Result<WorkspaceInfo> {
    let mut value = WorkspaceInfo {
        path: card.worktree.clone(),
        branch: card.branch.clone(),
        target_branch: project
            .target_branch
            .clone()
            .or_else(|| project.workflow.workspace.base_branch.clone()),
        base_commit: card.base_commit.clone(),
        state: "not_created".into(),
        dirty: false,
    };
    if let Some(path) = &card.worktree {
        let cwd = Path::new(path);
        value.state = if !cwd.is_dir() {
            "missing"
        } else if !git(cwd, &["rev-parse", "--verify", "HEAD"])?
            .status
            .success()
        {
            "directory"
        } else {
            "available"
        }
        .into();
        if value.state == "available" {
            value.branch = review::current_branch(path);
            value.base_commit = card
                .base_commit
                .clone()
                .or_else(|| checked(cwd, &["rev-parse", "HEAD"]).ok());
            value.dirty =
                !checked(cwd, &["status", "--porcelain", "--untracked-files=all"])?.is_empty();
            if !checked(cwd, &["diff", "--name-only", "--diff-filter=U"])?.is_empty() {
                value.state = "conflict".into();
            }
        }
    }
    Ok(value)
}
pub fn archive(home: &Path, project: &Project, card: &Card) -> Result<Option<String>> {
    let Some(path) = &card.worktree else {
        return Ok(None);
    };
    let p = Path::new(path);
    if !p.starts_with(home.join("worktrees")) || !p.exists() {
        return Ok(None);
    }
    let status = git(p, &["status", "--porcelain"])?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Ok(Some(
            "Managed worktree retained: it contains changes or could not be checked".into(),
        ));
    }
    // Retain commits not reachable from the source HEAD before removing the checkout.
    let keep = format!("refs/canban/cards/{}", card.id);
    let sha = git(p, &["rev-parse", "HEAD"])?;
    let sha = String::from_utf8(sha.stdout)?;
    let out = git(
        Path::new(&project.directory),
        &["update-ref", &keep, sha.trim()],
    )?;
    ensure!(out.status.success(), "Could not retain Card commit");
    let out = git(
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

#[cfg(test)]
mod tests {
    use super::*;
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
            id: "project".into(),
            name: "Test".into(),
            directory: source.to_str().unwrap().into(),
            workflow,
            target_branch: None,
            created_at: 0,
        };
        let mut card = Card {
            id: "card".into(),
            project_id: project.id.clone(),
            number: 1,
            title: "Task".into(),
            body: String::new(),
            tags: vec![],
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
            created_at: 0,
        };
        let home = dir.path().join("home");
        let path = prepare(&home, &project, &card).unwrap();
        card.worktree = Some(path.to_string_lossy().into_owned());
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
}

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
                id: "p".into(),
                name: "Test".into(),
                directory: source.to_string_lossy().into(),
                workflow: Default::default(),
                target_branch: Some("main".into()),
                created_at: 0,
            };
            let card:Card=serde_json::from_value(serde_json::json!({"id":"c","project_id":"p","number":42,"title":"Task","body":"","tags":[],"stage":"develop","revision":1,"created_at":0})).unwrap();
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
    fn fresh_worktree_syncs_base_and_keeps_branch_across_archive_restore() {
        let mut f = Fixture::new();
        let latest = f.remote_change();
        let cwd = f.prepare().unwrap();
        assert_eq!(checked(&f.source, &["rev-parse", "HEAD"]).unwrap(), latest);
        assert_eq!(
            review::current_branch(cwd.to_str().unwrap()).as_deref(),
            Some("canban/p/42")
        );
        assert!(cwd.join("remote-change").exists());
        f.card.worktree = Some(cwd.to_string_lossy().into());
        f.card.branch = Some("canban/p/42".into());
        checked(&f.source, &["remote", "remove", "origin"]).unwrap();
        assert_eq!(f.prepare().unwrap(), cwd); // Existing work never re-fetches.
        archive(&f.dir.path().join("home"), &f.project, &f.card).unwrap();
        assert!(!cwd.exists());
        assert_eq!(
            checked(&f.source, &["rev-parse", "refs/heads/canban/p/42"]).unwrap(),
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
                &["show-ref", "--verify", "refs/heads/canban/p/42"]
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
            Some("canban/p/42")
        );
        assert_eq!(fs::read_to_string(cwd.join("draft")).unwrap(), "preserve");
        assert!(info(&f.project, &f.card).unwrap().dirty);
    }
}
