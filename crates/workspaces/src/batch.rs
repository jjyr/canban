//! Batch merge-queue integration in a dedicated project `_merge_queue` worktree.
use super::Ops;
use super::review::{DeliveryError, summarize_porcelain_z};
use anyhow::{Context, Result, ensure};
use canban_types::{Card, MergeBatchItem, MergeStrategy, Project};
use std::path::{Path, PathBuf};

/// Returns `<home>/worktrees/<project-namespace>/_merge_queue`, matching how Card worktrees
/// are namespaced under `worktrees/`.
pub fn merge_queue_worktree_path(home: &Path, project: &Project) -> PathBuf {
    home.join("worktrees")
        .join(super::names::namespace(project))
        .join("_merge_queue")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchIntegrationOutcome {
    Clean {
        worktree: PathBuf,
        target_branch: String,
        target_head: String,
        head_sha: String,
        items: Vec<MergeBatchItem>,
    },
    Conflict {
        worktree: PathBuf,
        target_branch: String,
        target_head: String,
        items: Vec<MergeBatchItem>,
        conflict_card_number: u64,
        conflict_files: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchTestOutcome {
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub tail: String,
}

fn split_nul_names(bytes: &[u8]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for part in bytes.split(|&b| b == 0) {
        if !part.is_empty() {
            out.push(String::from_utf8(part.to_vec()).context("Git returned a non-UTF-8 path")?);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn utf8_tail(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let slice = &bytes[bytes.len() - max_bytes..];
    let text = String::from_utf8_lossy(slice);
    let trimmed = text.trim_start_matches('\u{FFFD}');
    if let Some((_, rest)) = trimmed.split_once('\n') {
        format!("...[truncated]\n{rest}")
    } else {
        format!("...[truncated]\n{trimmed}")
    }
}

impl Ops {
    fn resolve_batch_target_branch(
        &self,
        project: &Project,
        target: Option<&str>,
    ) -> Result<String> {
        let source = Path::new(&project.directory);
        let source_branch = self.current_branch(&project.directory).unwrap_or_default();
        let branch = target
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                project
                    .target_branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or(source_branch);
        ensure!(
            !branch.is_empty(),
            "Choose a target branch; the Project checkout is detached"
        );
        self.r_checked(source, &["check-ref-format", "--branch", &branch])?;
        Ok(branch)
    }

    /// Checks whether the Project's main checkout is currently on `target_branch` with a clean
    /// working tree, returning `(target_branch, target_head)` or [`DeliveryError::MainCheckoutBusy`].
    pub fn check_main_checkout_ready(
        &self,
        project: &Project,
        target: Option<&str>,
    ) -> Result<(String, String)> {
        let source = Path::new(&project.directory);
        let target_branch = self.resolve_batch_target_branch(project, target)?;
        let target_head = self.r_text(
            source,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{target_branch}^{{commit}}"),
            ],
        )?;
        let source_branch = self.current_branch(&project.directory).unwrap_or_default();
        if source_branch != target_branch {
            return Err(DeliveryError::MainCheckoutBusy {
                reason: format!(
                    "Main repository checkout ({}) is on branch `{source_branch}`; switch it back to `{target_branch}` before merging.",
                    source.display()
                ),
            }
            .into());
        }
        let source_status = self.r_checked(
            source,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !source_status.is_empty() {
            let summary = summarize_porcelain_z(&source_status, 4);
            return Err(DeliveryError::MainCheckoutBusy {
                reason: format!(
                    "Main repository checkout ({}) has uncommitted local changes ({summary}). Commit or stash them before merging.",
                    source.display()
                ),
            }
            .into());
        }
        Ok((target_branch, target_head))
    }

    fn unmerged_files(&self, dir: &Path) -> Result<Vec<String>> {
        split_nul_names(&self.r_checked(dir, &["diff", "--name-only", "--diff-filter=U", "-z"])?)
    }

    /// Returns `true` when a Card has its own non-root Git worktree in the Project's repository
    /// without an already-in-progress merge/rebase or unmerged index entries.
    pub fn card_worktree_ready_for_batch(&self, project: &Project, card: &Card) -> bool {
        let Some(wt) = card.worktree.as_deref() else {
            return false;
        };
        let cwd = Path::new(wt);
        let source = Path::new(&project.directory);
        if !self.host.is_dir(cwd) {
            return false;
        }
        let (Ok(canon_wt), Ok(canon_src)) = (self.host.canonical(cwd), self.host.canonical(source))
        else {
            return false;
        };
        if canon_wt == canon_src {
            return false;
        }
        let (Ok(common_wt), Ok(common_src)) = (self.r_common(cwd), self.r_common(source)) else {
            return false;
        };
        if common_wt != common_src {
            return false;
        }
        !self.in_merge_or_rebase(cwd).unwrap_or(true)
            && self.unmerged_files(cwd).is_ok_and(|f| f.is_empty())
    }

    fn ensure_git_identity(&self, wt: &Path) {
        if self
            .git(wt, &["config", "--get", "user.name"])
            .is_ok_and(|o| !o.status.success())
        {
            let _ = self.git(wt, &["config", "user.name", "Canban"]);
        }
        if self
            .git(wt, &["config", "--get", "user.email"])
            .is_ok_and(|o| !o.status.success())
        {
            let _ = self.git(wt, &["config", "user.email", "canban@localhost"]);
        }
    }

    /// Creates or resets the detached Project `_merge_queue` worktree to `target_head`.
    pub fn ensure_merge_queue_worktree(
        &self,
        home: &Path,
        project: &Project,
        target_head: &str,
    ) -> Result<PathBuf> {
        let source = Path::new(&project.directory);
        let wt = merge_queue_worktree_path(home, project);
        let _ = self.git(source, &["worktree", "prune"]);
        let common_source = self.r_common(source)?;
        let reuse = self.host.is_dir(&wt)
            && self
                .git(&wt, &["rev-parse", "--is-inside-work-tree"])
                .is_ok_and(|o| o.status.success())
            && self.r_common(&wt).ok().as_ref() == Some(&common_source);
        if reuse {
            let _ = self.git(&wt, &["rebase", "--abort"]);
            let _ = self.git(&wt, &["merge", "--abort"]);
            let _ = self.git(&wt, &["cherry-pick", "--abort"]);
            self.r_checked(&wt, &["checkout", "--detach", target_head])?;
            self.r_checked(&wt, &["reset", "--hard", target_head])?;
            self.r_checked(&wt, &["clean", "-fd"])?;
        } else {
            if self.host.exists(&wt) {
                let wt_s = wt.to_string_lossy();
                let _ = self.host.run(source, &["rm", "-rf", &wt_s], None);
            }
            if let Some(parent) = wt.parent() {
                self.host.mkdir(parent)?;
            }
            let wt_s = wt.to_string_lossy().into_owned();
            self.r_checked(source, &["worktree", "add", "--detach", &wt_s, target_head])?;
        }
        self.ensure_git_identity(&wt);
        Ok(wt)
    }

    /// Writes `refs/canban/batch/<batch_short_id>/{target,card-<number>}` so `@merger` can inspect
    /// exact pre-batch refs from inside `_merge_queue`.
    pub fn write_batch_refs(
        &self,
        project: &Project,
        batch_short_id: &str,
        target_head: &str,
        card_heads: &[(u64, String)],
    ) -> Result<()> {
        let source = Path::new(&project.directory);
        let target_ref = format!("refs/canban/batch/{batch_short_id}/target");
        self.r_checked(source, &["update-ref", &target_ref, target_head])?;
        for (card_number, head) in card_heads {
            let card_ref = format!("refs/canban/batch/{batch_short_id}/card-{card_number}");
            self.r_checked(source, &["update-ref", &card_ref, head])?;
        }
        Ok(())
    }

    /// Removes temporary `refs/canban/batch/<batch_short_id>/*` refs after a batch finishes.
    pub fn cleanup_batch_refs(&self, project: &Project, batch_short_id: &str) {
        let source = Path::new(&project.directory);
        let prefix = format!("refs/canban/batch/{batch_short_id}/");
        let Ok(out) = self.git(source, &["for-each-ref", "--format=%(refname)", &prefix]) else {
            return;
        };
        if !out.status.success() {
            return;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let _ = self.git(source, &["update-ref", "-d", trimmed]);
            }
        }
    }

    /// Sequentially integrates `cards` (`(Card, workflow_run_id)`) onto `target_head` inside
    /// `<home>/worktrees/<ns>/_merge_queue`. On conflict, leaves conflicts materialized in `_merge_queue`
    /// and returns [`BatchIntegrationOutcome::Conflict`].
    pub fn integrate_batch(
        &self,
        home: &Path,
        project: &Project,
        target_branch: &str,
        target_head: &str,
        strategy: MergeStrategy,
        cards: &[(Card, String)],
    ) -> Result<BatchIntegrationOutcome> {
        ensure!(!cards.is_empty(), "Batch requires at least one Card");
        let source = Path::new(&project.directory);
        let common_source = self.r_common(source)?;
        let canonical_source = self.host.canonical(source)?;

        let mut resolved = Vec::with_capacity(cards.len());
        let mut items = Vec::with_capacity(cards.len());
        for (card, run_id) in cards {
            ensure!(
                card.archived_at.is_none(),
                "Restore Card #{} before merging",
                card.number
            );
            let wt_str = card
                .worktree
                .as_deref()
                .with_context(|| format!("Card #{} has no prepared worktree", card.number))?;
            let cwd = Path::new(wt_str);
            ensure!(
                self.host.is_dir(cwd),
                "Card #{} worktree is missing",
                card.number
            );
            ensure!(
                self.host.canonical(cwd)? != canonical_source,
                "Merge delivery requires a separate Git Card worktree"
            );
            ensure!(
                self.r_common(cwd)? == common_source,
                "Card #{} worktree belongs to a different Git repository",
                card.number
            );
            self.commit_card_worktree_if_dirty(project, card, "")?;
            ensure!(
                !self.in_merge_or_rebase(cwd)?,
                "Card #{} worktree has an unfinished merge or rebase",
                card.number
            );
            let card_head = self.r_text(cwd, &["rev-parse", "--verify", "HEAD"])?;
            let branch = self
                .current_branch(wt_str)
                .or_else(|| card.branch.clone())
                .unwrap_or_default();
            items.push(MergeBatchItem {
                card_id: card.id.clone(),
                card_number: card.number,
                workflow_run_id: run_id.clone(),
                branch,
                worktree: wt_str.to_owned(),
                base_head: card_head.clone(),
                integrated_commit: None,
                evicted: false,
            });
            resolved.push((card, card_head));
        }

        let wt = self.ensure_merge_queue_worktree(home, project, target_head)?;
        let mut cur_head = target_head.to_owned();

        for (idx, (card, card_head)) in resolved.into_iter().enumerate() {
            if card_head == cur_head
                || self
                    .git(&wt, &["merge-base", "--is-ancestor", &card_head, &cur_head])?
                    .status
                    .success()
            {
                items[idx].integrated_commit = Some(cur_head.clone());
                continue;
            }

            let conflict_files = match strategy {
                MergeStrategy::Rebase => {
                    self.integrate_card_rebase(&wt, &cur_head, &card_head, card)?
                }
                MergeStrategy::Squash => {
                    let out = self.git(&wt, &["merge", "--squash", "--no-commit", &card_head])?;
                    let conflicts = self.unmerged_files(&wt)?;
                    if !out.status.success() || !conflicts.is_empty() {
                        Some(if conflicts.is_empty() {
                            self.modified_status_paths(&wt)?
                        } else {
                            conflicts
                        })
                    } else {
                        if !self
                            .git(&wt, &["diff", "--cached", "--quiet"])?
                            .status
                            .success()
                        {
                            let msg = format!("Canban #{}: {}", card.number, card.title);
                            self.r_checked(&wt, &["commit", "-m", &msg])?;
                        }
                        None
                    }
                }
                MergeStrategy::Merge => {
                    let out = self.git(&wt, &["merge", "--no-ff", "--no-commit", &card_head])?;
                    let conflicts = self.unmerged_files(&wt)?;
                    if !out.status.success() || !conflicts.is_empty() {
                        Some(if conflicts.is_empty() {
                            self.modified_status_paths(&wt)?
                        } else {
                            conflicts
                        })
                    } else {
                        if self
                            .git(&wt, &["rev-parse", "--verify", "MERGE_HEAD"])?
                            .status
                            .success()
                        {
                            let msg = format!("Merge card #{}: {}", card.number, card.title);
                            self.r_checked(&wt, &["commit", "-m", &msg])?;
                        }
                        None
                    }
                }
            };

            if let Some(files) = conflict_files {
                return Ok(BatchIntegrationOutcome::Conflict {
                    worktree: wt,
                    target_branch: target_branch.to_owned(),
                    target_head: target_head.to_owned(),
                    items,
                    conflict_card_number: card.number,
                    conflict_files: files,
                });
            }
            cur_head = self.r_text(&wt, &["rev-parse", "HEAD"])?;
            items[idx].integrated_commit = Some(cur_head.clone());
        }

        Ok(BatchIntegrationOutcome::Clean {
            worktree: wt,
            target_branch: target_branch.to_owned(),
            target_head: target_head.to_owned(),
            head_sha: cur_head,
            items,
        })
    }

    fn modified_status_paths(&self, dir: &Path) -> Result<Vec<String>> {
        split_nul_names(&self.r_checked(dir, &["diff", "--name-only", "-z", "HEAD"])?)
    }

    fn integrate_card_rebase(
        &self,
        wt: &Path,
        cur_head: &str,
        card_head: &str,
        card: &Card,
    ) -> Result<Option<Vec<String>>> {
        let base = self.r_text(wt, &["merge-base", cur_head, card_head])?;
        let commits_raw = self.r_text(
            wt,
            &[
                "rev-list",
                "--reverse",
                "--no-merges",
                &format!("{base}..{card_head}"),
            ],
        )?;
        let commits: Vec<&str> = commits_raw
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if commits.is_empty() {
            // Card branch only differs via a merge commit; fold its diff cleanly onto cur_head.
            let out = self.git(wt, &["merge", "--squash", "--no-commit", card_head])?;
            let conflicts = self.unmerged_files(wt)?;
            if !out.status.success() || !conflicts.is_empty() {
                return Ok(Some(if conflicts.is_empty() {
                    self.modified_status_paths(wt)?
                } else {
                    conflicts
                }));
            }
            if !self
                .git(wt, &["diff", "--cached", "--quiet"])?
                .status
                .success()
            {
                let msg = format!("Canban #{}: {}", card.number, card.title);
                self.r_checked(wt, &["commit", "-m", &msg])?;
            }
            return Ok(None);
        }

        for sha in commits {
            let out = self.git(wt, &["cherry-pick", sha])?;
            if out.status.success() {
                continue;
            }
            let conflicts = self.unmerged_files(wt)?;
            if !conflicts.is_empty() {
                return Ok(Some(conflicts));
            }
            // Empty/redundant cherry-pick if changes are already present on `wt`.
            if self
                .git(wt, &["rev-parse", "--verify", "CHERRY_PICK_HEAD"])?
                .status
                .success()
                && self
                    .git(wt, &["diff", "--cached", "--quiet"])?
                    .status
                    .success()
            {
                self.r_checked(wt, &["cherry-pick", "--skip"])?;
                continue;
            }
            return Ok(Some(self.modified_status_paths(wt)?));
        }
        Ok(None)
    }

    /// Runs `command` inside `worktree` via `/bin/sh -lc`, writes the full log to `log_path`,
    /// and returns status plus a truncated tail for diagnostics/UI.
    pub fn run_batch_tests(
        &self,
        worktree: &Path,
        command: &str,
        log_path: &Path,
    ) -> Result<BatchTestOutcome> {
        let out = self
            .host
            .run(worktree, &["/bin/sh", "-lc", command], None)?;
        let mut combined = Vec::with_capacity(out.stdout.len() + out.stderr.len() + 1);
        combined.extend_from_slice(&out.stdout);
        if !out.stdout.is_empty() && !out.stdout.ends_with(b"\n") && !out.stderr.is_empty() {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&out.stderr);
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(log_path, &combined)?;
        Ok(BatchTestOutcome {
            passed: out.status.success(),
            exit_code: out.status.code(),
            tail: utf8_tail(&combined, 8 * 1024),
        })
    }

    /// Verifies that `_merge_queue` has no unfinished cherry-pick/merge/rebase, no unmerged paths,
    /// no uncommitted tracked changes, and is a fast-forward descendant of `target_head`.
    pub fn verify_clean_batch_worktree(
        &self,
        worktree: &Path,
        target_head: &str,
    ) -> Result<String> {
        ensure!(self.host.is_dir(worktree), "Batch worktree is missing");
        ensure!(
            !self.in_merge_or_rebase(worktree)?,
            "Merge or rebase is still in progress inside _merge_queue; finish committing before reporting"
        );
        ensure!(
            !self
                .git(worktree, &["rev-parse", "--verify", "CHERRY_PICK_HEAD"])?
                .status
                .success(),
            "A cherry-pick is still in progress inside _merge_queue; finish committing before reporting"
        );
        let unmerged = self.unmerged_files(worktree)?;
        ensure!(
            unmerged.is_empty(),
            "Unresolved conflict files remain in _merge_queue: {}",
            unmerged.join(", ")
        );
        let status = self.r_checked(
            worktree,
            &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
        )?;
        ensure!(
            status.is_empty(),
            "Uncommitted tracked changes remain in _merge_queue ({}); commit or reset them before reporting",
            summarize_porcelain_z(&status, 4)
        );
        ensure!(
            self.git(
                worktree,
                &["merge-base", "--is-ancestor", target_head, "HEAD"]
            )?
            .status
            .success(),
            "_merge_queue HEAD is not a descendant of target commit {target_head}"
        );
        self.r_text(worktree, &["rev-parse", "--verify", "HEAD"])
    }

    /// Fast-forwards `target_branch` in `project.directory` to `resolved_head`, rejecting a busy or
    /// diverged main checkout with [`DeliveryError`].
    pub fn fast_forward_target(
        &self,
        project: &Project,
        target_branch: &str,
        expected_target_head: &str,
        resolved_head: &str,
    ) -> Result<()> {
        let source = Path::new(&project.directory);
        let current_source_branch = self.current_branch(&project.directory).unwrap_or_default();
        if current_source_branch != target_branch {
            return Err(DeliveryError::MainCheckoutBusy {
                reason: format!(
                    "Main repository checkout ({}) switched to branch `{current_source_branch}` while preparing the batch; switch it back to `{target_branch}`.",
                    source.display()
                ),
            }
            .into());
        }
        if self.r_text(source, &["rev-parse", "HEAD"])? != expected_target_head {
            return Err(DeliveryError::TargetAdvanced.into());
        }
        let source_status = self.r_checked(
            source,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !source_status.is_empty() {
            return Err(DeliveryError::MainCheckoutBusy {
                reason: format!(
                    "Main repository checkout ({}) has uncommitted local changes ({}); commit or stash them before merging.",
                    source.display(),
                    summarize_porcelain_z(&source_status, 4)
                ),
            }
            .into());
        }
        self.r_checked(
            source,
            &["merge", "--ff-only", "--no-overwrite-ignore", resolved_head],
        )?;
        ensure!(
            self.r_text(source, &["rev-parse", "HEAD"])? == resolved_head,
            "Target HEAD did not reach the batch commit"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    struct BatchFixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
        source: PathBuf,
        project: Project,
    }

    impl BatchFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            let source = dir.path().join("source");
            fs::create_dir_all(&home).unwrap();
            fs::create_dir_all(&source).unwrap();
            git_run(&source, &["init", "-b", "main"]);
            for (k, v) in [
                ("user.name", "Tester"),
                ("user.email", "tester@example.invalid"),
                ("core.hooksPath", "/dev/null"),
                ("commit.gpgsign", "false"),
            ] {
                git_run(&source, &["config", k, v]);
            }
            fs::write(source.join("README.md"), "base\n").unwrap();
            git_run(&source, &["add", "."]);
            git_run(&source, &["commit", "-m", "initial"]);
            let mut workflow = canban_types::Workflow::default();
            workflow.workspace.sync_base = false;
            let project = Project {
                id: "abcd1234".into(),
                name: "Demo".into(),
                directory: source.to_string_lossy().into_owned(),
                workflow,
                github: None,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Default::default(),
                location: None,
                target_branch: Some("main".into()),
                created_at: 1,
            };
            Self {
                _dir: dir,
                home,
                source,
                project,
            }
        }

        fn make_card(&self, number: u64, file: &str, content: &str, msg: &str) -> Card {
            let card: Card = serde_json::from_value(serde_json::json!({
                "id": format!("c{number:07}"),
                "project_id": self.project.id,
                "number": number,
                "title": format!("Card {number}"),
                "body": "",
                "stage": "review",
                "revision": 1,
                "created_at": number
            }))
            .unwrap();
            let wt = Ops::local()
                .prepare(&self.home, &self.project, &card)
                .unwrap();
            for (k, v) in [
                ("user.name", "Tester"),
                ("user.email", "tester@example.invalid"),
                ("core.hooksPath", "/dev/null"),
                ("commit.gpgsign", "false"),
            ] {
                git_run(&wt, &["config", k, v]);
            }
            fs::write(wt.join(file), content).unwrap();
            git_run(&wt, &["add", "."]);
            git_run(&wt, &["commit", "-m", msg]);
            let mut out = card;
            out.worktree = Some(wt.to_string_lossy().into_owned());
            out.branch = Ops::local().current_branch(wt.to_str().unwrap());
            out
        }
    }

    fn git_run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn integrate_batch_supports_rebase_squash_and_merge_across_three_cards() {
        for strategy in [
            MergeStrategy::Rebase,
            MergeStrategy::Squash,
            MergeStrategy::Merge,
        ] {
            let f = BatchFixture::new();
            let ops = Ops::local();
            let c1 = f.make_card(1, "a.txt", "a\n", "feat: card 1");
            let c2 = f.make_card(2, "b.txt", "b\n", "feat: card 2");
            let c3 = f.make_card(3, "c.txt", "c\n", "feat: card 3");
            let (_, target_head) = ops
                .check_main_checkout_ready(&f.project, Some("main"))
                .unwrap();

            let outcome = ops
                .integrate_batch(
                    &f.home,
                    &f.project,
                    "main",
                    &target_head,
                    strategy,
                    &[(c1, "r1".into()), (c2, "r2".into()), (c3, "r3".into())],
                )
                .unwrap();
            let BatchIntegrationOutcome::Clean {
                worktree,
                head_sha,
                items,
                ..
            } = outcome
            else {
                panic!("expected clean integration for {strategy:?}");
            };
            assert_eq!(items.len(), 3);
            assert!(items.iter().all(|i| i.integrated_commit.is_some()));
            assert_eq!(
                ops.verify_clean_batch_worktree(&worktree, &target_head)
                    .unwrap(),
                head_sha
            );

            ops.fast_forward_target(&f.project, "main", &target_head, &head_sha)
                .unwrap();
            assert_eq!(fs::read_to_string(f.source.join("a.txt")).unwrap(), "a\n");
            assert_eq!(fs::read_to_string(f.source.join("b.txt")).unwrap(), "b\n");
            assert_eq!(fs::read_to_string(f.source.join("c.txt")).unwrap(), "c\n");
        }
    }

    #[test]
    fn integrate_batch_materializes_conflict_and_run_batch_tests_reports_failure() {
        let f = BatchFixture::new();
        let ops = Ops::local();
        let c1 = f.make_card(1, "shared.txt", "from card 1\n", "feat: card 1");
        let c2 = f.make_card(2, "shared.txt", "from card 2\n", "feat: card 2");
        let (_, target_head) = ops
            .check_main_checkout_ready(&f.project, Some("main"))
            .unwrap();

        let outcome = ops
            .integrate_batch(
                &f.home,
                &f.project,
                "main",
                &target_head,
                MergeStrategy::Rebase,
                &[(c1, "r1".into()), (c2, "r2".into())],
            )
            .unwrap();
        let BatchIntegrationOutcome::Conflict {
            worktree,
            conflict_card_number,
            conflict_files,
            items,
            ..
        } = outcome
        else {
            panic!("expected conflict");
        };
        assert_eq!(conflict_card_number, 2);
        assert_eq!(conflict_files, vec!["shared.txt"]);
        assert!(items[0].integrated_commit.is_some());
        assert!(items[1].integrated_commit.is_none());
        // Cannot verify clean while conflict is unresolved.
        assert!(
            ops.verify_clean_batch_worktree(&worktree, &target_head)
                .is_err()
        );

        // Resolve conflict and cherry-pick continue cleanly.
        fs::write(worktree.join("shared.txt"), "resolved\n").unwrap();
        git_run(&worktree, &["add", "shared.txt"]);
        let st = Command::new("git")
            .current_dir(&worktree)
            .env("GIT_EDITOR", "true")
            .args(["cherry-pick", "--continue"])
            .status()
            .unwrap();
        assert!(st.success());

        let log = f.home.join("test.log");
        let fail = ops
            .run_batch_tests(&worktree, "echo boom >&2; exit 2", &log)
            .unwrap();
        assert!(!fail.passed);
        assert_eq!(fail.exit_code, Some(2));
        assert!(fail.tail.contains("boom"));

        let pass = ops
            .run_batch_tests(&worktree, "test -f shared.txt && echo ok", &log)
            .unwrap();
        assert!(pass.passed);
        assert!(pass.tail.contains("ok"));

        let resolved_head = ops
            .verify_clean_batch_worktree(&worktree, &target_head)
            .unwrap();
        ops.fast_forward_target(&f.project, "main", &target_head, &resolved_head)
            .unwrap();
        assert_eq!(
            fs::read_to_string(f.source.join("shared.txt")).unwrap(),
            "resolved\n"
        );
    }
}
