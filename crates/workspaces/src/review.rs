//! Review and delivery use Git in the Card checkout first. A conflict never
//! leaves the Project checkout in a half-merged state.
use super::Ops;
use anyhow::{Context, Result, ensure};
use canban_types::{Card, MergeStrategy, Project, ReviewPreview};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Delivery failures callers can route automatically (downcast from `anyhow::Error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryError {
    /// Integrating the target into the Card worktree produced textual conflicts.
    /// The worktree is left mid-merge so a person or Agent can resolve it there.
    Conflict {
        target_branch: String,
        target_head: String,
        files: Vec<String>,
        detail: String,
    },
    /// The Project target branch moved while delivery was being prepared; retrying is safe.
    TargetAdvanced,
    /// The main repository checkout temporarily has uncommitted local changes or is checked out
    /// on another branch; Queue Merge pauses non-destructively at the head of the queue and
    /// automatically resumes once the checkout is clean and on the target branch.
    MainCheckoutBusy { reason: String },
}
impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict {
                target_branch,
                files,
                detail,
                ..
            } => {
                if files.is_empty() {
                    write!(
                        f,
                        "Merge needs attention in the Card worktree; the Project branch ({target_branch}) was not updated. {detail}"
                    )
                } else {
                    write!(
                        f,
                        "Merge needs attention in the Card worktree; the Project branch was not updated. Conflicts with {target_branch} in: {}. {detail}",
                        files.join(", ")
                    )
                }
            }
            Self::TargetAdvanced => {
                f.write_str("Target branch advanced; refresh the review. Card commit is retained.")
            }
            Self::MainCheckoutBusy { reason } => f.write_str(reason),
        }
    }
}
impl std::error::Error for DeliveryError {}

/// Returns `true` when a `ReviewPreview` rejection reason is purely a transient main-checkout
/// condition (uncommitted changes or another branch checked out) rather than a Card-level blocker.
pub fn is_main_checkout_busy_reason(reason: &str) -> bool {
    reason.starts_with("Main repository checkout (")
}

impl Ops {
    pub(crate) fn r_checked(&self, dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
        let out = self.git(dir, args)?;
        ensure!(
            out.status.success(),
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(out.stdout)
    }

    fn git_path(&self, dir: &Path, rel: &str) -> Result<PathBuf> {
        let p = PathBuf::from(self.r_text(dir, &["rev-parse", "--git-path", rel])?);
        Ok(if p.is_absolute() { p } else { dir.join(p) })
    }

    pub(crate) fn in_rebase(&self, cwd: &Path) -> Result<bool> {
        Ok(self.host.exists(&self.git_path(cwd, "rebase-merge")?)
            || self.host.exists(&self.git_path(cwd, "rebase-apply")?))
    }

    pub(crate) fn rebase_branch(&self, cwd: &Path) -> Option<String> {
        let raw = self
            .read_git_file(cwd, "rebase-merge/head-name")
            .or_else(|| self.read_git_file(cwd, "rebase-apply/head-name"))?;
        raw.strip_prefix("refs/heads/")
            .filter(|b| !b.is_empty())
            .map(str::to_owned)
    }

    fn rebase_onto_head(&self, cwd: &Path) -> Option<String> {
        self.read_git_file(cwd, "rebase-merge/onto")
            .or_else(|| self.read_git_file(cwd, "rebase-apply/onto"))
    }

    fn read_git_file(&self, cwd: &Path, rel: &str) -> Option<String> {
        let p = self.git_path(cwd, rel).ok()?;
        if !self.host.exists(&p) {
            return None;
        }
        std::fs::read_to_string(&p)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    /// If an Agent or user resolved a paused rebase conflict by running `git commit` directly
    /// instead of `git rebase --continue`, `HEAD` now contains that resolved commit on a detached
    /// HEAD while `rebase-merge` is still open on the already-committed `stopped-sha`. Advance the
    /// rebase with `git rebase --skip` so remaining commits replay and the Card branch reattaches.
    fn finalize_clean_manual_rebase_commit(&self, cwd: &Path) -> Result<()> {
        if !self.in_rebase(cwd)? {
            return Ok(());
        }
        let status = self.r_checked(
            cwd,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !status.is_empty() {
            return Ok(());
        }
        let Some(onto) = self.rebase_onto_head(cwd) else {
            return Ok(());
        };
        let head = self.r_text(cwd, &["rev-parse", "--verify", "HEAD"])?;
        if head == onto {
            return Ok(());
        }
        let is_on_onto = self
            .git(cwd, &["merge-base", "--is-ancestor", &onto, &head])?
            .status
            .success();
        if is_on_onto {
            let _ = self.git(cwd, &["rebase", "--skip"]);
        }
        Ok(())
    }

    pub(crate) fn in_merge_or_rebase(&self, cwd: &Path) -> Result<bool> {
        let in_merge = self
            .git(cwd, &["rev-parse", "--verify", "MERGE_HEAD"])?
            .status
            .success();
        Ok(in_merge || self.in_rebase(cwd)?)
    }

    /// Best-effort checkpoint commit of uncommitted deliverable edits in a managed Card
    /// worktree when an Agent reports completion without running `git commit` itself.
    /// Never touches the Project's main checkout or an in-progress merge/rebase.
    pub fn commit_card_worktree_if_dirty(
        &self,
        project: &Project,
        card: &Card,
        message: &str,
    ) -> Result<Option<String>> {
        let Some(worktree) = card.worktree.as_deref() else {
            return Ok(None);
        };
        let cwd = Path::new(worktree);
        let source = Path::new(&project.directory);
        if !self.host.is_dir(cwd) || self.host.canonical(source)? == self.host.canonical(cwd)? {
            return Ok(None);
        }
        if self.in_merge_or_rebase(cwd)? {
            return Ok(None);
        }
        let status = self.r_checked(
            cwd,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if status.is_empty() {
            return Ok(None);
        }
        let msg = message.trim();
        let fallback = format!("Canban #{}: {}", card.number, card.title);
        let commit_msg = if msg.is_empty() {
            fallback.as_str()
        } else {
            msg
        };
        self.r_checked(cwd, &["add", "-A", "--", "."])?;
        if self
            .git(cwd, &["diff", "--cached", "--quiet"])?
            .status
            .success()
        {
            return Ok(None);
        }
        self.r_checked(cwd, &["commit", "-m", commit_msg])?;
        Ok(Some(self.r_text(cwd, &["rev-parse", "HEAD"])?))
    }

    /// Returns any in-progress merge or rebase conflict in the Card worktree (`MERGE_HEAD` or
    /// `rebase-merge`/`rebase-apply`) so an interrupted Queue merge can re-route conflict
    /// resolution on retry.
    pub fn pending_merge_conflict(
        &self,
        project: &Project,
        card: &Card,
        target: Option<&str>,
    ) -> Result<Option<(String, String, Vec<String>)>> {
        let Some(worktree) = card.worktree.as_deref() else {
            return Ok(None);
        };
        let cwd = Path::new(worktree);
        let source = Path::new(&project.directory);
        if !self.host.is_dir(cwd) || self.host.canonical(source)? == self.host.canonical(cwd)? {
            return Ok(None);
        }
        let in_merge = self
            .git(cwd, &["rev-parse", "--verify", "MERGE_HEAD"])?
            .status
            .success();
        if !in_merge {
            self.finalize_clean_manual_rebase_commit(cwd)?;
        }
        let in_rebase = self.in_rebase(cwd)?;
        let files =
            names(&self.r_checked(cwd, &["diff", "--name-only", "--diff-filter=U", "-z"])?)?;
        if (!in_merge && !in_rebase) || (in_merge && files.is_empty()) {
            return Ok(None);
        }
        let source_branch = self.current_branch(&project.directory).unwrap_or_default();
        let target_branch = target
            .map(str::to_owned)
            .or_else(|| project.target_branch.clone())
            .unwrap_or(source_branch);
        if target_branch.is_empty() {
            return Ok(None);
        }
        let target_head = if in_merge {
            self.r_text(cwd, &["rev-parse", "--verify", "MERGE_HEAD"])
                .unwrap_or_default()
        } else {
            self.rebase_onto_head(cwd)
                .or_else(|| {
                    self.r_text(
                        source,
                        &[
                            "rev-parse",
                            "--verify",
                            &format!("refs/heads/{target_branch}^{{commit}}"),
                        ],
                    )
                    .ok()
                })
                .unwrap_or_default()
        };
        Ok(Some((target_branch, target_head, files)))
    }
    pub(crate) fn r_text(&self, dir: &Path, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8(self.r_checked(dir, args)?)
            .context("Git returned a non-UTF-8 path/ref")?
            .trim_end_matches('\n')
            .into())
    }
    pub(crate) fn r_common(&self, dir: &Path) -> Result<PathBuf> {
        let path = PathBuf::from(self.r_text(dir, &["rev-parse", "--git-common-dir"])?);
        let resolved = if path.is_absolute() {
            path
        } else {
            dir.join(path)
        };
        if !self.host.runner().is_system() {
            return Ok(self.host.canonical(&resolved).unwrap_or(resolved));
        }
        self.host.canonical(&resolved)
    }
    pub fn current_branch(&self, directory: &str) -> Option<String> {
        let out = self
            .git(
                Path::new(directory),
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
            )
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    pub fn review_preview(
        &self,
        project: &Project,
        card: &Card,
        target: Option<&str>,
    ) -> Result<ReviewPreview> {
        ensure!(
            card.archived_at.is_none(),
            "Restore the Card before reviewing its checkout"
        );
        let source = Path::new(&project.directory);
        let cwd = Path::new(
            card.worktree
                .as_deref()
                .context("This Card has no prepared worktree")?,
        );
        ensure!(
            self.host.is_dir(cwd),
            "Card worktree is missing; restore/open the Card Agent before reviewing"
        );
        ensure!(
            self.host.canonical(source)? != self.host.canonical(cwd)?,
            "Merge delivery requires a separate Git Card worktree"
        );
        ensure!(
            self.r_common(source)? == self.r_common(cwd)?,
            "Card worktree belongs to a different Git repository"
        );
        self.finalize_clean_manual_rebase_commit(cwd)?;
        let source_branch = self.current_branch(&project.directory).unwrap_or_default();
        let target = target
            .map(str::to_owned)
            .or_else(|| project.target_branch.clone())
            .unwrap_or_else(|| source_branch.clone());
        ensure!(
            !target.is_empty(),
            "Choose a target branch; the Project checkout is detached"
        );
        self.r_checked(source, &["check-ref-format", "--branch", &target])?;
        let target_ref = format!("refs/heads/{target}");
        let target_head = self.r_text(
            source,
            &["rev-parse", "--verify", &format!("{target_ref}^{{commit}}")],
        )?;
        let card_head = self.r_text(cwd, &["rev-parse", "--verify", "HEAD"])?;
        let base = self.r_text(cwd, &["merge-base", &target_head, &card_head])?;
        let status = self.r_checked(
            cwd,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let mut files = names(&self.r_checked(
            cwd,
            &["diff", "--no-ext-diff", "--name-only", "-z", &base, "--"],
        )?)?;
        let mut patch = self.r_checked(
            cwd,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--binary",
                &base,
                "--",
            ],
        )?;
        for path in
            names(&self.r_checked(cwd, &["ls-files", "--others", "--exclude-standard", "-z"])?)?
        {
            let out = self.git(
                cwd,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--binary",
                    "--no-index",
                    "--",
                    "/dev/null",
                    &path,
                ],
            )?;
            ensure!(
                out.status.code() == Some(1) || out.status.success(),
                "Could not read untracked file {path}"
            );
            patch.extend_from_slice(&out.stdout);
            files.push(path);
        }
        files.sort();
        files.dedup();
        let conflicts =
            names(&self.r_checked(cwd, &["diff", "--name-only", "--diff-filter=U", "-z"])?)?;
        let source_status = self.r_checked(
            source,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let in_merge_or_rebase = self.in_merge_or_rebase(cwd)?;
        let reason = if source_branch != target {
            Some(format!(
                "Main repository checkout ({}) is on branch `{source_branch}`; switch it to `{target}` before merging this Card.",
                source.display()
            ))
        } else if !source_status.is_empty() {
            let summary = summarize_porcelain_z(&source_status, 4);
            Some(format!(
                "Main repository checkout ({}) has uncommitted local changes ({summary}). Commit or stash them before merging this Card.",
                source.display()
            ))
        } else if in_merge_or_rebase || !conflicts.is_empty() {
            Some(
                "Finish or abort the existing merge or rebase in the Card terminal before accepting."
                    .into(),
            )
        } else {
            None
        };
        let mut hash = Sha256::new();
        for value in [
            &target,
            &target_head,
            &card_head,
            &source_branch,
            &card.revision.to_string(),
        ] {
            hash.update(value.as_bytes());
            hash.update([0]);
        }
        hash.update(&status);
        hash.update(&source_status);
        hash.update(&patch);
        let token = format!("{:x}", hash.finalize());
        let truncated = patch.len() > 2 * 1024 * 1024;
        let patch =
            String::from_utf8_lossy(&patch[..patch.len().min(2 * 1024 * 1024)]).into_owned();
        Ok(ReviewPreview {
            card_id: card.id.clone(),
            revision: card.revision,
            target_branch: target,
            target_head,
            card_head,
            source_branch,
            worktree: cwd.to_string_lossy().into_owned(),
            files,
            patch,
            truncated,
            dirty: !status.is_empty(),
            conflicts,
            token,
            merge_allowed: reason.is_none(),
            reason,
            warning: None,
        })
    }

    /// The caller has persisted a delivery intent before invoking this operation.
    pub fn review_merge(
        &self,
        project: &Project,
        card: &Card,
        target: Option<&str>,
        token: &str,
        message: &str,
    ) -> Result<String> {
        self.review_merge_with_strategy(
            project,
            card,
            target,
            token,
            message,
            MergeStrategy::Rebase,
            false,
        )
    }

    /// Integrates the reviewed Card worktree into `target` using `strategy` (`rebase`, `squash`,
    /// or forward `merge`). Any conflict or integration failure leaves `source` untouched and
    /// returns a typed [`DeliveryError::Conflict`] so Queue Merge routes through `@merger`.
    #[allow(
        clippy::too_many_arguments,
        reason = "Delivery operation threads preview token, strategy, and explicit customization state"
    )]
    pub fn review_merge_with_strategy(
        &self,
        project: &Project,
        card: &Card,
        target: Option<&str>,
        token: &str,
        message: &str,
        strategy: MergeStrategy,
        message_customized: bool,
    ) -> Result<String> {
        let plan = self.review_preview(project, card, target)?;
        ensure!(
            plan.token == token,
            "Review is stale: files or branch heads changed. Refresh the diff before accepting."
        );
        ensure!(
            plan.merge_allowed,
            "{}",
            plan.reason.as_deref().unwrap_or("Merge unavailable")
        );
        let cwd = Path::new(&plan.worktree);
        let source = Path::new(&project.directory);
        let trimmed_msg = message.trim();
        ensure!(!trimmed_msg.is_empty(), "Commit message is empty");
        if plan.dirty {
            // Explicit Commit and merge accepts the complete reviewed Card checkout.
            self.r_checked(cwd, &["add", "-A", "--", "."])?;
            let reviewed_tree = self.r_text(cwd, &["write-tree"])?;
            self.r_checked(cwd, &["commit", "-m", trimmed_msg])?;
            ensure!(
                self.r_text(cwd, &["rev-parse", "HEAD^{tree}"])? == reviewed_tree,
                "Git hooks changed the committed tree. Inspect the retained Card commit and refresh the review; the Project branch was not updated."
            );
        }
        let card_head = self.r_text(cwd, &["rev-parse", "HEAD"])?;
        let target_is_ancestor = self
            .git(
                cwd,
                &["merge-base", "--is-ancestor", &plan.target_head, &card_head],
            )?
            .status
            .success();

        let head = match strategy {
            MergeStrategy::Rebase => {
                if !target_is_ancestor {
                    let rebase = self.git(cwd, &["rebase", &plan.target_head])?;
                    if !rebase.status.success() {
                        let detail = format!(
                            "{} {}",
                            String::from_utf8_lossy(&rebase.stdout).trim(),
                            String::from_utf8_lossy(&rebase.stderr).trim()
                        )
                        .trim()
                        .to_owned();
                        let files = self.collect_or_abort_rebase_conflicts(
                            cwd,
                            &plan.target_head,
                            &card_head,
                        )?;
                        return Err(DeliveryError::Conflict {
                            target_branch: plan.target_branch.clone(),
                            target_head: plan.target_head.clone(),
                            files,
                            detail,
                        }
                        .into());
                    }
                }
                let rebased_head = self.r_text(cwd, &["rev-parse", "HEAD"])?;
                if message_customized && !plan.dirty && rebased_head != plan.target_head {
                    let count = self
                        .r_text(
                            cwd,
                            &[
                                "rev-list",
                                "--count",
                                &format!("{}..{rebased_head}", plan.target_head),
                            ],
                        )
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    if count == 1 {
                        let tree_before = self.r_text(cwd, &["rev-parse", "HEAD^{tree}"])?;
                        self.r_checked(cwd, &["commit", "--amend", "-m", trimmed_msg])?;
                        ensure!(
                            self.r_text(cwd, &["rev-parse", "HEAD^{tree}"])? == tree_before,
                            "Git hooks changed the committed tree. Inspect the retained Card commit and refresh the review; the Project branch was not updated."
                        );
                    }
                }
                self.r_text(cwd, &["rev-parse", "HEAD"])?
            }
            MergeStrategy::Squash => {
                let merged_tree = if target_is_ancestor {
                    self.r_text(cwd, &["rev-parse", &format!("{card_head}^{{tree}}")])?
                } else {
                    self.compute_or_materialize_merge_tree(
                        cwd,
                        &plan.target_branch,
                        &plan.target_head,
                        &card_head,
                    )?
                };
                let target_tree = self.r_text(
                    cwd,
                    &["rev-parse", &format!("{}^{{tree}}", plan.target_head)],
                )?;
                if merged_tree == target_tree {
                    plan.target_head.clone()
                } else {
                    let squash_msg = if message_customized || plan.dirty {
                        trimmed_msg.to_owned()
                    } else {
                        self.semantic_squash_message(
                            cwd,
                            &plan.target_head,
                            &card_head,
                            trimmed_msg,
                        )
                    };
                    let commit = self.r_text(
                        cwd,
                        &[
                            "commit-tree",
                            &merged_tree,
                            "-p",
                            &plan.target_head,
                            "-m",
                            &squash_msg,
                        ],
                    )?;
                    self.r_checked(cwd, &["reset", "--hard", &commit])?;
                    commit
                }
            }
            MergeStrategy::Merge => {
                if card_head == plan.target_head {
                    plan.target_head.clone()
                } else {
                    let (merged_tree, second_parent) = if target_is_ancestor {
                        let tree =
                            self.r_text(cwd, &["rev-parse", &format!("{card_head}^{{tree}}")])?;
                        // If `@merger` resolved conflicts via a reverse merge commit `[card_prev, target_head]`,
                        // peel `card_prev` as the second parent so `target_branch` gets a clean forward merge
                        // without a redundant reverse merge in its graph.
                        let second = self
                            .peel_reverse_sync_merge(cwd, &card_head, &plan.target_head)
                            .unwrap_or_else(|| card_head.clone());
                        (tree, second)
                    } else {
                        let tree = self.compute_or_materialize_merge_tree(
                            cwd,
                            &plan.target_branch,
                            &plan.target_head,
                            &card_head,
                        )?;
                        (tree, card_head.clone())
                    };
                    let merge_msg = if message_customized {
                        trimmed_msg.to_owned()
                    } else {
                        self.semantic_merge_message(card, &plan.target_branch, trimmed_msg)
                    };
                    let commit = self.r_text(
                        cwd,
                        &[
                            "commit-tree",
                            &merged_tree,
                            "-p",
                            &plan.target_head,
                            "-p",
                            &second_parent,
                            "-m",
                            &merge_msg,
                        ],
                    )?;
                    self.r_checked(cwd, &["reset", "--hard", &commit])?;
                    commit
                }
            }
        };

        ensure!(
            self.r_checked(
                cwd,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"]
            )?
            .is_empty(),
            "Git hooks left Card changes after commit/merge; inspect and refresh before delivery"
        );
        let current_source_branch = self.current_branch(&project.directory).unwrap_or_default();
        if current_source_branch != plan.target_branch {
            return Err(DeliveryError::MainCheckoutBusy {
                reason: format!(
                    "Main repository checkout ({}) switched to branch `{current_source_branch}` while preparing the merge; switch it back to `{}` (Card commit is retained).",
                    source.display(),
                    plan.target_branch
                ),
            }
            .into());
        }
        if self.r_text(source, &["rev-parse", "HEAD"])? != plan.target_head {
            return Err(DeliveryError::TargetAdvanced.into());
        }
        let latest_source_status = self.r_checked(
            source,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !latest_source_status.is_empty() {
            return Err(DeliveryError::MainCheckoutBusy {
                reason: format!(
                    "Main repository checkout ({}) has uncommitted local changes ({}); commit or stash them before merging (Card commit is retained).",
                    source.display(),
                    summarize_porcelain_z(&latest_source_status, 4)
                ),
            }
            .into());
        }
        self.r_checked(
            source,
            &["merge", "--ff-only", "--no-overwrite-ignore", &head],
        )?;
        ensure!(
            self.r_text(source, &["rev-parse", "HEAD"])? == head,
            "Target HEAD did not reach the accepted Card commit"
        );
        Ok(head)
    }

    fn collect_or_abort_rebase_conflicts(
        &self,
        cwd: &Path,
        target_head: &str,
        card_head: &str,
    ) -> Result<Vec<String>> {
        let mut files = self
            .r_checked(cwd, &["diff", "--name-only", "--diff-filter=U", "-z"])
            .and_then(|out| names(&out))
            .unwrap_or_default();
        if files.is_empty() {
            // If rebase failed without leaving index conflict markers, abort any half-started
            // rebase so the Card worktree stays on its branch, and extract any conflicted paths
            // from `merge-tree` for `@merger`.
            if self.in_rebase(cwd).unwrap_or(false) {
                let _ = self.git(cwd, &["rebase", "--abort"]);
            }
            files = self.merge_tree_conflict_files(cwd, target_head, card_head);
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    fn merge_tree_conflict_files(
        &self,
        cwd: &Path,
        target_head: &str,
        card_head: &str,
    ) -> Vec<String> {
        let Ok(out) = self.git(
            cwd,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "-z",
                target_head,
                card_head,
            ],
        ) else {
            return Vec::new();
        };
        if out.status.success() {
            return Vec::new();
        }
        let mut parts = out.stdout.split(|b| *b == 0);
        // First NUL-terminated item is the tree OID (or empty); subsequent items before an empty
        // separator are conflicted file names.
        let _ = parts.next();
        let mut files = Vec::new();
        for item in parts {
            if item.is_empty() {
                break;
            }
            if let Ok(path) = std::str::from_utf8(item) {
                files.push(path.to_owned());
            }
        }
        files.sort();
        files.dedup();
        files
    }

    fn compute_or_materialize_merge_tree(
        &self,
        cwd: &Path,
        target_branch: &str,
        target_head: &str,
        card_head: &str,
    ) -> Result<String> {
        let mt = self.git(cwd, &["merge-tree", "--write-tree", target_head, card_head])?;
        if mt.status.success() {
            let stdout =
                String::from_utf8(mt.stdout).context("Git merge-tree returned non-UTF-8 output")?;
            let tree = stdout
                .lines()
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("Git merge-tree returned an empty tree OID")?;
            return Ok(tree.to_owned());
        }
        // Materialize the conflict inside the Card worktree so `@merger` (and `review_preview` /
        // `pending_merge_conflict`) sees live conflict markers while `source` remains untouched.
        let integrate = self.git(cwd, &["merge", "--no-commit", "--no-ff", target_head])?;
        let detail = format!(
            "{} {} {}",
            String::from_utf8_lossy(&mt.stdout).trim(),
            String::from_utf8_lossy(&integrate.stdout).trim(),
            String::from_utf8_lossy(&integrate.stderr).trim()
        )
        .trim()
        .to_owned();
        let mut files = self
            .r_checked(cwd, &["diff", "--name-only", "--diff-filter=U", "-z"])
            .and_then(|out| names(&out))
            .unwrap_or_default();
        if files.is_empty() {
            files = self.merge_tree_conflict_files(cwd, target_head, card_head);
        }
        files.sort();
        files.dedup();
        Err(DeliveryError::Conflict {
            target_branch: target_branch.to_owned(),
            target_head: target_head.to_owned(),
            files,
            detail,
        }
        .into())
    }

    fn peel_reverse_sync_merge(
        &self,
        cwd: &Path,
        card_head: &str,
        target_head: &str,
    ) -> Option<String> {
        let rev = self
            .r_text(cwd, &["rev-list", "--parents", "-n", "1", card_head])
            .ok()?;
        let parts: Vec<&str> = rev.split_whitespace().collect();
        if parts.len() == 3 && parts[2] == target_head && parts[1] != target_head {
            Some(parts[1].to_owned())
        } else {
            None
        }
    }

    fn semantic_squash_message(
        &self,
        cwd: &Path,
        target_head: &str,
        card_head: &str,
        fallback: &str,
    ) -> String {
        let Ok(raw) = self.r_checked(
            cwd,
            &[
                "log",
                "--no-merges",
                "--reverse",
                "--format=%B%x00",
                &format!("{target_head}..{card_head}"),
            ],
        ) else {
            return fallback.to_owned();
        };
        let commits: Vec<String> = raw
            .split(|b| *b == 0)
            .filter_map(|chunk| std::str::from_utf8(chunk).ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        match commits.as_slice() {
            [] => fallback.to_owned(),
            [single] => single.clone(),
            many => {
                let bullets: Vec<String> = many
                    .iter()
                    .filter_map(|m| m.lines().next())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("- {s}"))
                    .collect();
                if bullets.is_empty() {
                    fallback.to_owned()
                } else {
                    format!("{fallback}\n\n{}", bullets.join("\n"))
                }
            }
        }
    }

    fn semantic_merge_message(&self, card: &Card, _target_branch: &str, fallback: &str) -> String {
        let default_canban = format!("Canban #{}: {}", card.number, card.title);
        if fallback == default_canban {
            format!("Merge card #{}: {}", card.number, card.title)
        } else {
            fallback.to_owned()
        }
    }
}
fn names(bytes: &[u8]) -> Result<Vec<String>> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| Ok(std::str::from_utf8(s)?.to_owned()))
        .collect()
}
pub(crate) fn summarize_porcelain_z(bytes: &[u8], limit: usize) -> String {
    let mut paths = Vec::new();
    let mut parts = bytes.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let Some(entry) = parts.next() {
        let s = String::from_utf8_lossy(entry);
        let status = s.get(..2).unwrap_or("");
        let path = s.get(3..).unwrap_or(s.as_ref());
        // In `-z` porcelain v1, rename/copy entries (`R` or `C` in X/Y) are followed by a second NUL-terminated orig_path.
        if status.contains('R') || status.contains('C') {
            let _ = parts.next();
        }
        paths.push(path.to_owned());
    }
    if paths.is_empty() {
        return "dirty working tree".into();
    }
    let total = paths.len();
    let shown: Vec<String> = paths.into_iter().take(limit).collect();
    if total > shown.len() {
        format!("{}, +{} more", shown.join(", "), total - shown.len())
    } else {
        shown.join(", ")
    }
}

pub fn current_branch(dir: &str) -> Option<String> {
    Ops::local().current_branch(dir)
}
pub fn preview(project: &Project, card: &Card, target: Option<&str>) -> Result<ReviewPreview> {
    Ops::local().review_preview(project, card, target)
}
pub fn merge(
    project: &Project,
    card: &Card,
    target: Option<&str>,
    token: &str,
    message: &str,
) -> Result<String> {
    Ops::local().review_merge(project, card, target, token, message)
}

#[cfg(test)]
fn checked(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    Ops::local().r_checked(dir, args)
}
#[cfg(test)]
fn text(dir: &Path, args: &[&str]) -> Result<String> {
    Ops::local().r_text(dir, args)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    struct Fixture {
        _dir: tempfile::TempDir,
        project: Project,
        card: Card,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            fs::create_dir(&source).unwrap();
            checked(&source, &["init", "-b", "main"]).unwrap();
            for (key, value) in [
                ("user.name", "Acceptance"),
                ("user.email", "acceptance@example.invalid"),
                ("core.hooksPath", "/dev/null"),
                ("commit.gpgsign", "false"),
            ] {
                checked(&source, &["config", key, value]).unwrap();
            }
            fs::write(source.join("seed"), "original\n").unwrap();
            checked(&source, &["add", "."]).unwrap();
            checked(&source, &["commit", "-m", "seed"]).unwrap();
            let mut workflow = canban_types::Workflow::default();
            workflow.workspace.sync_base = false;
            let project = Project {
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Default::default(),
                id: "p".into(),
                name: "Test".into(),
                directory: source.to_str().unwrap().into(),
                workflow,
                target_branch: Some("main".into()),
                created_at: 0,
            };
            let mut card: Card = serde_json::from_value(serde_json::json!({"id":"c","project_id":"p","number":1,"title":"Task","body":"","stage":"review","revision":1,"created_at":0})).unwrap();
            card.worktree = Some(
                super::super::prepare(&dir.path().join("home"), &project, &card)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            Self {
                _dir: dir,
                project,
                card,
            }
        }
        fn source(&self) -> &Path {
            Path::new(&self.project.directory)
        }
        fn cwd(&self) -> &Path {
            Path::new(self.card.worktree.as_ref().unwrap())
        }
        fn source_commit(&self) {
            checked(self.source(), &["add", "."]).unwrap();
            checked(self.source(), &["commit", "-m", "target change"]).unwrap();
        }
        fn accept(&self) -> Result<String> {
            let p = preview(&self.project, &self.card, None)?;
            merge(&self.project, &self.card, None, &p.token, "accepted")
        }
    }
    #[test]
    fn accepts_reviewed_untracked_and_tracked_changes_without_archiving() {
        let f = Fixture::new();
        fs::write(f.cwd().join("seed"), "changed\n").unwrap();
        fs::write(f.cwd().join("new file"), "new\n").unwrap();
        let p = preview(&f.project, &f.card, None).unwrap();
        assert_eq!(p.files, vec!["new file", "seed"]);
        assert!(p.patch.contains("+new"));
        assert!(p.dirty);
        let commit = f.accept().unwrap();
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), commit);
        assert_eq!(
            fs::read_to_string(f.source().join("seed")).unwrap(),
            "changed\n"
        );
        assert!(f.cwd().exists());
        assert!(f.card.archived_at.is_none());
        assert!(
            checked(f.cwd(), &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn stale_diff_and_dirty_target_never_commit_or_merge() {
        let f = Fixture::new();
        fs::write(f.cwd().join("new"), "one").unwrap();
        let p = preview(&f.project, &f.card, None).unwrap();
        let before = text(f.cwd(), &["rev-parse", "HEAD"]).unwrap();
        fs::write(f.cwd().join("new"), "two").unwrap();
        assert!(
            merge(&f.project, &f.card, None, &p.token, "accepted")
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        assert_eq!(text(f.cwd(), &["rev-parse", "HEAD"]).unwrap(), before);
        fs::write(f.source().join("local"), "keep").unwrap();
        let p = preview(&f.project, &f.card, None).unwrap();
        assert!(!p.merge_allowed);
        assert!(f.accept().is_err());
        assert_eq!(
            fs::read_to_string(f.source().join("local")).unwrap(),
            "keep"
        );
        assert_eq!(text(f.cwd(), &["rev-parse", "HEAD"]).unwrap(), before);
    }
    #[test]
    fn divergent_target_integrates_in_card_then_fast_forwards_source() {
        let f = Fixture::new();
        fs::write(f.cwd().join("card"), "card").unwrap();
        fs::write(f.source().join("target"), "target").unwrap();
        f.source_commit();
        f.accept().unwrap();
        assert!(f.source().join("card").exists());
        assert!(f.source().join("target").exists());
        assert_eq!(
            text(f.source(), &["rev-parse", "HEAD"]).unwrap(),
            text(f.cwd(), &["rev-parse", "HEAD"]).unwrap()
        );
    }
    #[test]
    fn conflict_stays_in_card_and_does_not_change_source() {
        let f = Fixture::new();
        fs::write(f.cwd().join("seed"), "card version\n").unwrap();
        fs::write(f.source().join("seed"), "target version\n").unwrap();
        f.source_commit();
        let before = text(f.source(), &["rev-parse", "HEAD"]).unwrap();
        let err = f.accept().unwrap_err();
        assert!(err.to_string().contains("needs attention"));
        match err.downcast_ref::<DeliveryError>() {
            Some(DeliveryError::Conflict {
                files,
                target_branch,
                target_head,
                ..
            }) => {
                assert_eq!(files, &vec!["seed".to_string()]);
                assert_eq!(target_branch, "main");
                assert_eq!(target_head, &before);
            }
            other => panic!("expected typed conflict, got {other:?}"),
        }
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), before);
        assert_eq!(
            fs::read_to_string(f.source().join("seed")).unwrap(),
            "target version\n"
        );
        let p = preview(&f.project, &f.card, None).unwrap();
        assert!(!p.merge_allowed);
        assert_eq!(p.conflicts, vec!["seed"]);
        assert!(
            checked(f.source(), &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn source_ignored_files_are_not_overwritten_by_delivery() {
        let f = Fixture::new();
        fs::write(f.source().join(".gitignore"), "*.local\n").unwrap();
        f.source_commit();
        let before = text(f.source(), &["rev-parse", "HEAD"]).unwrap();
        fs::write(f.source().join("config.local"), "user data").unwrap();
        fs::write(f.cwd().join("config.local"), "agent version").unwrap();
        checked(f.cwd(), &["add", "-f", "config.local"]).unwrap();
        checked(f.cwd(), &["commit", "-m", "Card adds config"]).unwrap();
        assert!(f.accept().is_err());
        assert_eq!(
            fs::read_to_string(f.source().join("config.local")).unwrap(),
            "user data"
        );
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), before);
    }
    #[test]
    fn hook_added_code_requires_a_new_review_before_source_delivery() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let hooks = f._dir.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        fs::write(
            &hook,
            "#!/bin/sh\nprintf 'hook code' > hook-added\ngit add hook-added\n",
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        checked(
            f.cwd(),
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        )
        .unwrap();
        let before = text(f.source(), &["rev-parse", "HEAD"]).unwrap();
        fs::write(f.cwd().join("reviewed"), "reviewed code").unwrap();
        assert!(
            f.accept()
                .unwrap_err()
                .to_string()
                .contains("hooks changed")
        );
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), before);
        assert!(!f.source().join("hook-added").exists());
        assert!(f.cwd().join("hook-added").exists());
    }
    #[test]
    fn summarize_porcelain_z_lists_paths_and_truncates_cleanly() {
        let raw = b" M crates/ui/src/lib.rs\0?? scratch.txt\0R  new_name.rs\0old_name.rs\0?? a.rs\0?? b.rs\0";
        assert_eq!(
            summarize_porcelain_z(raw, 4),
            "crates/ui/src/lib.rs, scratch.txt, new_name.rs, a.rs, +1 more"
        );
    }

    #[test]
    fn rebase_strategy_preserves_semantic_commits_without_raw_sha_merge_commit() {
        let f = Fixture::new();
        fs::write(f.cwd().join("feature.rs"), "pub fn run() {}\n").unwrap();
        checked(f.cwd(), &["add", "."]).unwrap();
        checked(f.cwd(), &["commit", "-m", "feat: add run entrypoint"]).unwrap();
        fs::write(f.source().join("other.txt"), "main moved\n").unwrap();
        f.source_commit();

        let p = preview(&f.project, &f.card, None).unwrap();
        let head = Ops::local()
            .review_merge_with_strategy(
                &f.project,
                &f.card,
                None,
                &p.token,
                "Canban #1: Task",
                MergeStrategy::Rebase,
                false,
            )
            .unwrap();
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            text(f.source(), &["log", "-n", "1", "--format=%s"]).unwrap(),
            "feat: add run entrypoint"
        );
        assert!(
            text(f.source(), &["rev-list", "--merges", "HEAD"])
                .unwrap()
                .is_empty(),
            "rebase strategy must not emit any merge commits"
        );
    }

    #[test]
    fn rebase_conflict_reports_pending_conflict_and_finishes_cleanly_after_continue() {
        let f = Fixture::new();
        fs::write(f.cwd().join("seed"), "card edit\n").unwrap();
        checked(f.cwd(), &["add", "."]).unwrap();
        checked(f.cwd(), &["commit", "-m", "feat: update seed"]).unwrap();
        fs::write(f.source().join("seed"), "main edit\n").unwrap();
        f.source_commit();
        let target_before = text(f.source(), &["rev-parse", "HEAD"]).unwrap();

        let p = preview(&f.project, &f.card, None).unwrap();
        let err = Ops::local()
            .review_merge_with_strategy(
                &f.project,
                &f.card,
                None,
                &p.token,
                "Canban #1: Task",
                MergeStrategy::Rebase,
                false,
            )
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<DeliveryError>(),
            Some(DeliveryError::Conflict { files, .. }) if files == &vec!["seed".to_string()]
        ));
        let pending = Ops::local()
            .pending_merge_conflict(&f.project, &f.card, None)
            .unwrap();
        assert_eq!(
            pending,
            Some(("main".into(), target_before.clone(), vec!["seed".into()]))
        );
        // Checkpoint helper must never auto-commit while mid-rebase.
        assert_eq!(
            Ops::local()
                .commit_card_worktree_if_dirty(&f.project, &f.card, "checkpoint")
                .unwrap(),
            None
        );

        // Simulate `@merger` resolving and finishing rebase.
        fs::write(f.cwd().join("seed"), "main + card edit\n").unwrap();
        checked(f.cwd(), &["add", "seed"]).unwrap();
        checked(f.cwd(), &["rebase", "--continue"]).unwrap();

        let p2 = preview(&f.project, &f.card, None).unwrap();
        assert!(p2.merge_allowed);
        let head = Ops::local()
            .review_merge_with_strategy(
                &f.project,
                &f.card,
                None,
                &p2.token,
                "Canban #1: Task",
                MergeStrategy::Rebase,
                false,
            )
            .unwrap();
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            text(f.source(), &["log", "-n", "1", "--format=%s"]).unwrap(),
            "feat: update seed"
        );
        assert_eq!(
            text(f.source(), &["rev-parse", "HEAD^"]).unwrap(),
            target_before
        );
    }

    #[test]
    fn squash_strategy_condenses_commits_and_recovers_via_merger_on_conflict() {
        let f = Fixture::new();
        fs::write(f.cwd().join("a.txt"), "step 1\n").unwrap();
        checked(f.cwd(), &["add", "."]).unwrap();
        checked(f.cwd(), &["commit", "-m", "feat: add step 1"]).unwrap();
        fs::write(f.cwd().join("seed"), "card seed\n").unwrap();
        checked(f.cwd(), &["add", "."]).unwrap();
        checked(f.cwd(), &["commit", "-m", "fix: refine seed"]).unwrap();

        fs::write(f.source().join("seed"), "main seed\n").unwrap();
        f.source_commit();
        let target_before = text(f.source(), &["rev-parse", "HEAD"]).unwrap();

        let p = preview(&f.project, &f.card, None).unwrap();
        let err = Ops::local()
            .review_merge_with_strategy(
                &f.project,
                &f.card,
                None,
                &p.token,
                "Canban #1: Task",
                MergeStrategy::Squash,
                false,
            )
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<DeliveryError>(),
            Some(DeliveryError::Conflict { files, .. }) if files == &vec!["seed".to_string()]
        ));
        assert!(
            Ops::local()
                .pending_merge_conflict(&f.project, &f.card, None)
                .unwrap()
                .is_some(),
            "squash conflict must materialize MERGE_HEAD in Card worktree for @merger"
        );

        // `@merger` resolves the conflict in the Card worktree.
        fs::write(f.cwd().join("seed"), "resolved seed\n").unwrap();
        checked(f.cwd(), &["add", "seed"]).unwrap();
        checked(
            f.cwd(),
            &["commit", "-m", "chore(merge): integrate main into card #1"],
        )
        .unwrap();

        let p2 = preview(&f.project, &f.card, None).unwrap();
        let head = Ops::local()
            .review_merge_with_strategy(
                &f.project,
                &f.card,
                None,
                &p2.token,
                "Canban #1: Task",
                MergeStrategy::Squash,
                false,
            )
            .unwrap();
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            text(f.source(), &["rev-parse", "HEAD^"]).unwrap(),
            target_before,
            "squash strategy must land a single commit directly on top of target_head"
        );
        assert!(
            text(f.source(), &["rev-list", "--merges", "HEAD"])
                .unwrap()
                .is_empty()
        );
        let body = text(f.source(), &["log", "-n", "1", "--format=%B"]).unwrap();
        assert!(body.contains("Canban #1: Task"), "{body}");
        assert!(body.contains("- feat: add step 1"), "{body}");
        assert!(body.contains("- fix: refine seed"), "{body}");
    }

    #[test]
    fn merge_strategy_creates_forward_semantic_merge_preserving_first_parent() {
        let f = Fixture::new();
        fs::write(f.cwd().join("card.txt"), "card\n").unwrap();
        checked(f.cwd(), &["add", "."]).unwrap();
        checked(f.cwd(), &["commit", "-m", "feat: card commit"]).unwrap();
        let card_commit = text(f.cwd(), &["rev-parse", "HEAD"]).unwrap();

        fs::write(f.source().join("main.txt"), "main\n").unwrap();
        f.source_commit();
        let main_before = text(f.source(), &["rev-parse", "HEAD"]).unwrap();

        let p = preview(&f.project, &f.card, None).unwrap();
        let head = Ops::local()
            .review_merge_with_strategy(
                &f.project,
                &f.card,
                None,
                &p.token,
                "Canban #1: Task",
                MergeStrategy::Merge,
                false,
            )
            .unwrap();
        assert_eq!(text(f.source(), &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            text(f.source(), &["rev-parse", "HEAD^1"]).unwrap(),
            main_before,
            "first parent must remain main so git log --first-parent stays linear"
        );
        assert_eq!(
            text(f.source(), &["rev-parse", "HEAD^2"]).unwrap(),
            card_commit,
            "second parent must point to the Card branch commit"
        );
        assert_eq!(
            text(f.source(), &["log", "-n", "1", "--format=%s"]).unwrap(),
            "Merge card #1: Task"
        );
    }
}
