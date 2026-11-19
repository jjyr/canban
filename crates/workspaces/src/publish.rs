//! Commit and push the reviewed Card checkout without updating the target branch.
use super::*;
impl Ops {
    pub fn verify_clean_head(&self, directory: &str, expected: &str) -> Result<()> {
        let cwd = Path::new(directory);
        ensure!(
            self.checked(cwd, &["rev-parse", "HEAD"])? == expected,
            "Local Card head differs from the PR. Sync and review the Card worktree before merging"
        );
        ensure!(
            self.checked(cwd, &["status", "--porcelain=v1", "--untracked-files=all"])?
                .is_empty(),
            "Card has unpublished changes. Review and publish them before merging the PR"
        );
        Ok(())
    }
    pub fn push_remote(&self, project: &Project, card: &Card) -> Result<String> {
        let cwd = Path::new(card.worktree.as_deref().context("Card has no worktree")?);
        self.checked(
            cwd,
            &[
                "remote",
                "get-url",
                "--push",
                &project.workflow.workspace.remote,
            ],
        )
    }
    pub fn publish(
        &self,
        project: &Project,
        card: &Card,
        target: &str,
        token: &str,
        message: &str,
    ) -> Result<String> {
        let plan = self.review_preview(project, card, Some(target))?;
        ensure!(
            plan.token == token,
            "Review is stale; refresh the diff before publishing"
        );
        ensure!(
            plan.merge_allowed,
            "{}",
            plan.reason.as_deref().unwrap_or("Delivery unavailable")
        );
        let cwd = Path::new(&plan.worktree);
        let branch = card.branch.as_deref().context("Card has no named branch")?;
        ensure!(
            self.current_branch(&plan.worktree).as_deref() == Some(branch),
            "Card branch changed; refresh before publishing"
        );
        ensure!(branch != target, "Publish requires a separate Card branch");
        self.checked(cwd, &["check-ref-format", "--branch", branch])?;
        ensure!(!message.trim().is_empty(), "Commit message is empty");
        if plan.dirty {
            self.checked(cwd, &["add", "-A", "--", "."])?;
            let tree = self.checked(cwd, &["write-tree"])?;
            self.checked(cwd, &["commit", "-m", message])?;
            ensure!(
                self.checked(cwd, &["rev-parse", "HEAD^{tree}"])? == tree,
                "Commit hooks changed the reviewed tree; inspect the retained commit"
            );
        }
        ensure!(
            self.checked(cwd, &["status", "--porcelain=v1", "--untracked-files=all"])?
                .is_empty(),
            "Card changed during commit; refresh before pushing"
        );
        let head = self.checked(cwd, &["rev-parse", "HEAD"])?;
        let remote = &project.workflow.workspace.remote;
        ensure!(
            !remote.starts_with('-') && !remote.is_empty(),
            "Invalid push remote"
        );
        self.checked(
            cwd,
            &[
                "push",
                "--porcelain",
                "--set-upstream",
                remote,
                &format!("HEAD:refs/heads/{branch}"),
            ],
        )?;
        Ok(head)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::*;
    fn git(dir: &Path, args: &[&str]) -> String {
        let output = canban_exec::command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    #[test]
    fn publishes_without_merging_and_rejects_stale_or_unpublished_work() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let remote = temp.path().join("remote.git");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&remote).unwrap();
        git(&source, &["init", "-b", "main"]);
        git(&source, &["config", "user.name", "Acceptance"]);
        git(
            &source,
            &["config", "user.email", "acceptance@example.invalid"],
        );
        git(&source, &["config", "core.hooksPath", "/dev/null"]);
        git(&source, &["config", "commit.gpgsign", "false"]);
        std::fs::write(source.join("greeting.txt"), "before\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "Seed"]);
        git(&remote, &["init", "--bare"]);
        git(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&source, &["push", "origin", "main"]);
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
            directory: source.to_string_lossy().into_owned(),
            workflow: Workflow {
                workspace: WorkspaceSettings {
                    sync_base: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            target_branch: Some("main".into()),
            created_at: 0,
        };
        let mut card:Card=serde_json::from_value(serde_json::json!({"id":"card","project_id":"project","number":1,"title":"Greeting","body":"","stage":"review","revision":1,"attention":null,"archived_at":null,"archive":null,"archive_error":null,"worktree":null,"created_at":0})).unwrap();
        let ops = Ops::local();
        card.branch = Some(card_branch(&project, &card));
        let path = ops.prepare(temp.path(), &project, &card).unwrap();
        card.worktree = Some(path.to_string_lossy().into_owned());
        let original = git(&source, &["rev-parse", "HEAD"]);
        std::fs::write(path.join("greeting.txt"), "after\n").unwrap();
        let preview = ops.review_preview(&project, &card, Some("main")).unwrap();
        let head = ops
            .publish(&project, &card, "main", &preview.token, "Deliver")
            .unwrap();
        assert_ne!(head, original);
        assert_eq!(git(&source, &["rev-parse", "HEAD"]), original);
        assert_eq!(
            git(&remote, &["rev-parse", card.branch.as_deref().unwrap()]),
            head
        );
        ops.verify_clean_head(path.to_str().unwrap(), &head)
            .unwrap();
        assert!(
            ops.publish(&project, &card, "main", &preview.token, "Stale")
                .is_err()
        );
        std::fs::write(path.join("greeting.txt"), "unpublished\n").unwrap();
        assert!(
            ops.verify_clean_head(path.to_str().unwrap(), &head)
                .is_err()
        );
        assert_eq!(
            git(&remote, &["rev-parse", card.branch.as_deref().unwrap()]),
            head
        );
    }
}
