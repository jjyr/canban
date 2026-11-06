use anyhow::{Result, ensure};
use canban_types::{Card, Project};
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
pub fn prepare(home: &Path, project: &Project, card: &Card) -> Result<PathBuf> {
    if let Some(path) = &card.worktree {
        let p = PathBuf::from(path);
        ensure!(
            p.is_dir(),
            "Saved working directory is missing: {}",
            p.display()
        );
        return Ok(p);
    }
    let source = Path::new(&project.directory);
    if !git(source, &["rev-parse", "--verify", "HEAD"])?
        .status
        .success()
    {
        return Ok(source.into());
    }
    // Existing repository edits are not silently copied into a separate checkout.
    let target = home.join("worktrees").join(&project.id).join(&card.id);
    std::fs::create_dir_all(target.parent().unwrap())?;
    let out = git(
        source,
        &[
            "worktree",
            "add",
            "--detach",
            target.to_str().unwrap(),
            "HEAD",
        ],
    )?;
    ensure!(
        out.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(target)
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
        let project = Project {
            id: "project".into(),
            name: "Test".into(),
            directory: source.to_str().unwrap().into(),
            workflow: Default::default(),
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
        assert!(source.join("seed.txt").exists());
    }
}
