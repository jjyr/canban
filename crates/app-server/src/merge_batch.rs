//! Batch merge-queue integration, `@merger` session dispatch, and `batch.report` handling.

use super::*;
use canban_types::{
    Delivery, DeliveryMode, MergeBatch, MergeBatchItem, MergeStrategy, TerminalRuntime,
    WorkflowStep,
};
use canban_workspaces::{batch::BatchIntegrationOutcome, review::DeliveryError};

pub(super) enum BatchSelection {
    Empty,
    HoldQueue,
    Single(String),
    Batch { run_ids: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DeliverConfig {
    target_branch: Option<String>,
    strategy: MergeStrategy,
    sync_upstream: bool,
    batch_size: u32,
    test_command: Option<String>,
    conflict_profile: String,
    max_conflict_attempts: u32,
}

fn trimmed_opt(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn deliver_config(run: &WorkflowRun, config: &Config) -> DeliverConfig {
    let step = run
        .step
        .as_ref()
        .and_then(|k| run.definition.steps.get(k))
        .cloned();
    if let Some(WorkflowStep::Deliver {
        profile,
        target_branch,
        strategy,
        sync_upstream,
        batch_size,
        test_command,
        rework,
        max_conflict_attempts,
        ..
    }) = step
    {
        let conflict_profile = profile
            .as_deref()
            .and_then(|p| trimmed_opt(Some(p)))
            .or_else(|| {
                config
                    .profiles
                    .iter()
                    .any(|p| p.id == "merger" && p.enabled)
                    .then(|| "merger".to_owned())
            })
            .or_else(|| {
                rework
                    .as_ref()
                    .and_then(|to| match run.definition.steps.get(to) {
                        Some(WorkflowStep::Agent {
                            profile: Some(p), ..
                        }) if config.profiles.iter().any(|x| x.id == *p && x.enabled) => {
                            Some(p.clone())
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| config.default_agent.clone());
        return DeliverConfig {
            target_branch: trimmed_opt(target_branch.as_deref()),
            strategy,
            sync_upstream,
            batch_size: batch_size.clamp(1, 50),
            test_command: trimmed_opt(test_command.as_deref()),
            conflict_profile,
            max_conflict_attempts,
        };
    }

    let strategy = run.inputs["strategy"]
        .as_str()
        .and_then(|s| serde_json::from_value::<MergeStrategy>(json!(s)).ok())
        .unwrap_or_default();
    let conflict_profile = if config
        .profiles
        .iter()
        .any(|p| p.id == "merger" && p.enabled)
    {
        "merger".to_owned()
    } else {
        config.default_agent.clone()
    };
    DeliverConfig {
        target_branch: trimmed_opt(run.inputs["target_branch"].as_str()),
        strategy,
        sync_upstream: run.inputs["sync_upstream"].as_bool().unwrap_or(true),
        batch_size: (run.inputs["batch_size"]
            .as_u64()
            .unwrap_or(canban_types::DEFAULT_MERGE_BATCH_SIZE as u64) as u32)
            .clamp(1, 50),
        test_command: trimmed_opt(run.inputs["test_command"].as_str()),
        conflict_profile,
        max_conflict_attempts: canban_types::DEFAULT_MAX_CONFLICT_ATTEMPTS,
    }
}

fn card_agent_busy_or_settling(
    terminals: &[TerminalEntry],
    card_id: &str,
    run_updated_at: u64,
    now_ms: u64,
) -> (bool, bool) {
    let has_pending_attempts = terminals.iter().filter_map(|t| t.agent.as_ref()).any(|a| {
        a.card_id == card_id
            && (a.pending.is_some()
                || matches!(
                    a.state.as_str(),
                    "queued" | "starting" | "running" | "working" | "unknown"
                ))
    });
    let settling_agent = terminals.iter().any(|t| {
        let Some(a) = &t.agent else {
            return false;
        };
        if a.card_id != card_id || a.terminal.is_none() || a.agent_stopped() {
            return false;
        }
        if a.connection_state != "connected" {
            return false;
        }
        let anchor = a
            .completed_at
            .max(Some(run_updated_at))
            .unwrap_or(t.created_at);
        now_ms.saturating_sub(anchor) < AppServer::DELIVERY_AGENT_SETTLE_MS
    });
    (has_pending_attempts, settling_agent)
}

fn same_batch_config(a: &DeliverConfig, b: &DeliverConfig) -> bool {
    a.target_branch == b.target_branch
        && a.strategy == b.strategy
        && a.sync_upstream == b.sync_upstream
        && a.test_command == b.test_command
        && a.conflict_profile == b.conflict_profile
}

fn profile_agent_engine(prof: &canban_types::Profile) -> String {
    let cmd = prof.command_name();
    match cmd.as_str() {
        "codex" | "claude" => cmd,
        _ => prof.preset.clone().unwrap_or(cmd),
    }
}

fn delete_batch_row(db: Db<'_>, batch_id: &str) -> Result<()> {
    db.conn
        .execute("DELETE FROM merge_batches WHERE id=?1", params![batch_id])?;
    Ok(())
}

fn clear_batch_inputs(run: &mut WorkflowRun) {
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("batch_id");
        obj.remove("queued_at");
        obj.remove("queue_pause_reason");
    }
}

fn render_batch_prompt(
    project: &Project,
    batch: &MergeBatch,
    cards: &std::collections::BTreeMap<String, Card>,
) -> String {
    let short_id = batch.short_id();
    let strategy = serde_json::to_value(batch.strategy)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "rebase".into());
    let failure_summary = match batch.failure_kind.as_deref() {
        Some("conflict") => {
            let num = batch.conflict_card_number.unwrap_or(0);
            let files = if batch.conflict_files.is_empty() {
                "inspect `git status` in the batch worktree".to_owned()
            } else {
                batch.conflict_files.join(", ")
            };
            format!(
                "Merge/cherry-pick conflict while integrating Card #{num}. Unmerged files: {files}"
            )
        }
        Some("test_failure") => {
            let cmd = batch.test_command.as_deref().unwrap_or("(test command)");
            let tail = batch
                .test_output
                .as_deref()
                .unwrap_or("(no output captured)");
            format!(
                "Batch verification command `{cmd}` failed after integrating all Cards cleanly:\n```\n{tail}\n```"
            )
        }
        _ => batch
            .error
            .clone()
            .unwrap_or_else(|| "Resolve batch integration issue before completing.".into()),
    };
    let mut manifest = String::new();
    for (idx, item) in batch.items.iter().enumerate() {
        let title = cards
            .get(&item.card_id)
            .map(|c| c.title.as_str())
            .unwrap_or("");
        let status = if item.integrated_commit.is_some() {
            "integrated"
        } else if batch.conflict_card_number == Some(item.card_number) {
            "conflict (currently checked out / in-progress here)"
        } else {
            "pending integration"
        };
        manifest.push_str(&format!(
            "{}. Card #{} — {} (branch `{}`, ref `refs/canban/batch/{short_id}/card-{}`, status: {})\n",
            idx + 1,
            item.card_number,
            title,
            item.branch,
            item.card_number,
            status
        ));
    }
    let test_command_section = match batch.test_command.as_deref() {
        Some(cmd) => format!(
            "\n## Batch Test Command (must pass before reporting `completed`)\n```sh\n{cmd}\n```\nCanban will re-run this exact command once when you report `completed`.\n"
        ),
        None => String::new(),
    };
    canban_context::render_template(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prompts/runtime/merge_batch.txt"
        ))
        .trim_end(),
        &[
            ("project_name", &project.name),
            ("batch_id", &batch.id),
            ("short_id", short_id),
            ("target_branch", &batch.target_branch),
            ("strategy", &strategy),
            ("worktree", &batch.worktree),
            ("failure_summary", &failure_summary),
            ("cards_manifest", manifest.trim_end()),
            ("test_command_section", &test_command_section),
        ],
    )
}

impl AppServer {
    /// Selects either a single-card delivery (for held/no-worktree/single-without-tests) or a
    /// compatible FIFO batch of up to `batch_size` `waiting_delivery` runs for `project_id`.
    pub(super) fn collect_compatible_batch(&self, project_id: &str) -> Result<BatchSelection> {
        let config = self.config();
        let store = self.store.lock().unwrap();
        let db = store.db();
        if db
            .list::<MergeBatch>()?
            .into_iter()
            .any(|b| b.project_id == project_id && b.active())
        {
            return Ok(BatchSelection::HoldQueue);
        }
        let cards: Vec<Card> = db.list()?;
        let runs: Vec<WorkflowRun> = db.list()?;
        let queue = canban_types::project_merge_queue(&runs, &cards, project_id);
        let Some(head) = queue.first().copied() else {
            return Ok(BatchSelection::Empty);
        };
        if head.state != "waiting_delivery" {
            return Ok(BatchSelection::HoldQueue);
        }
        let project: Project = db.get(project_id)?;
        let cards_by_id: std::collections::BTreeMap<&str, &Card> =
            cards.iter().map(|c| (c.id.as_str(), c)).collect();
        let terminals: Vec<TerminalEntry> = db.list()?;
        let now_ms = self.now_ms();
        let Some(head_card) = head
            .card_id
            .as_deref()
            .and_then(|cid| cards_by_id.get(cid).copied())
        else {
            return Ok(BatchSelection::Single(head.id.clone()));
        };
        let (head_pending, head_settling) =
            card_agent_busy_or_settling(&terminals, &head_card.id, head.updated_at, now_ms);
        let ops = self.ops();
        let head_cfg = deliver_config(head, &config);
        if !head_card.active()
            || head_card.hold_auto_merge
            || head_card.worktree.is_none()
            || head_pending
            || head_settling
            || !ops.card_worktree_ready_for_batch(&project, head_card)
            || (head_cfg.batch_size == 1 && head_cfg.test_command.is_none())
        {
            return Ok(BatchSelection::Single(head.id.clone()));
        }

        let mut selected = vec![head.id.clone()];
        for run in queue.iter().copied().skip(1) {
            if selected.len() >= head_cfg.batch_size as usize {
                break;
            }
            if run.state != "waiting_delivery" {
                break;
            }
            let Some(card) = run
                .card_id
                .as_deref()
                .and_then(|cid| cards_by_id.get(cid).copied())
            else {
                break;
            };
            if !card.active()
                || card.hold_auto_merge
                || card.worktree.is_none()
                || !ops.card_worktree_ready_for_batch(&project, card)
            {
                break;
            }
            let (pending, settling) =
                card_agent_busy_or_settling(&terminals, &card.id, run.updated_at, now_ms);
            if pending || settling {
                break;
            }
            if !same_batch_config(&head_cfg, &deliver_config(run, &config)) {
                break;
            }
            selected.push(run.id.clone());
        }

        if selected.len() == 1 && head_cfg.test_command.is_none() {
            return Ok(BatchSelection::Single(head.id.clone()));
        }
        Ok(BatchSelection::Batch { run_ids: selected })
    }

    /// Executes one batch delivery cycle for `run_ids` in `project_id`.
    /// Returns `Ok((advance_queue, mutated))`: `advance_queue` is `true` when the queue can
    /// continue advancing, `false` when the queue head is paused or handed off to `@merger`;
    /// `mutated` is `true` when DB state changed.
    pub(super) fn execute_batch_delivery(
        &self,
        project_id: &str,
        run_ids: &[String],
    ) -> Result<(bool, bool)> {
        let config = self.config();
        let (project, candidates) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let project: Project = db.get(project_id)?;
            let mut items = Vec::with_capacity(run_ids.len());
            for rid in run_ids {
                let Ok(run) = db.get::<WorkflowRun>(rid) else {
                    continue;
                };
                if run.state != "waiting_delivery" {
                    continue;
                }
                let Some(card_id) = run.card_id.as_deref() else {
                    continue;
                };
                let Ok(card) = db.get::<Card>(card_id) else {
                    continue;
                };
                if !card.active() || card.hold_auto_merge || card.worktree.is_none() {
                    continue;
                }
                items.push((card, run));
            }
            (project, items)
        };
        if candidates.is_empty() {
            return Ok((true, false));
        }
        let card_ids: Vec<String> = candidates.iter().map(|(c, _)| c.id.clone()).collect();
        let _merging = self.track_merging_many(&card_ids);
        let cfg = deliver_config(&candidates[0].1, &config);
        let head_card_id = candidates[0].0.id.clone();
        let head_run_id = candidates[0].1.id.clone();
        let ops = self.ops();

        let mut committed_any = false;
        for (card, _) in &candidates {
            if ops
                .commit_card_worktree_if_dirty(
                    &project,
                    card,
                    &format!("Canban #{}: {}", card.number, card.title),
                )?
                .is_some()
            {
                committed_any = true;
            }
        }
        if committed_any {
            self.git_changed();
        }

        let (resolved_branch, target_head) = {
            let sync_holder = self.checkout_holder_label(
                "Batch code delivery sync",
                &project,
                cfg.target_branch.as_deref(),
            );
            let _checkout_gate = match self.try_lock_checkout(&project, sync_holder) {
                Ok(guard) => guard,
                Err(reason) => {
                    let changed = self.set_queue_pause_reason(
                        &head_card_id,
                        &head_run_id,
                        Some(&format!("Merge queue waiting: {reason}")),
                    )?;
                    return Ok((false, changed));
                }
            };
            if cfg.sync_upstream
                && ops.has_push_remote(&project)
                && let Err(e) = ops.sync_target(&project, cfg.target_branch.as_deref())
            {
                drop(_checkout_gate);
                if let Some(DeliveryError::MainCheckoutBusy { reason }) =
                    e.downcast_ref::<DeliveryError>()
                {
                    let changed = self.set_queue_pause_reason(
                        &head_card_id,
                        &head_run_id,
                        Some(&format!("Merge queue waiting: {reason}")),
                    )?;
                    return Ok((false, changed));
                }
                let reason = format!("Auto-merge paused (upstream sync): {e:#}");
                let mutated = self.store.lock().unwrap().write(|db| {
                    let mut current_card: Card = db.get(&head_card_id)?;
                    let mut current_run: WorkflowRun = db.get(&head_run_id)?;
                    if current_run.state == "waiting_delivery" {
                        canban_workflow_engine::runtime::hold_delivery_at(
                            db,
                            &mut current_card,
                            &mut current_run,
                            &reason,
                            self.now_ms(),
                        )?;
                        return Ok(true);
                    }
                    Ok(false)
                })?;
                if mutated {
                    self.emit("review", Some(head_card_id));
                }
                return Ok((true, mutated));
            }

            match ops.check_main_checkout_ready(&project, cfg.target_branch.as_deref()) {
                Ok(v) => v,
                Err(e) => {
                    drop(_checkout_gate);
                    if let Some(DeliveryError::MainCheckoutBusy { reason }) =
                        e.downcast_ref::<DeliveryError>()
                    {
                        let changed = self.set_queue_pause_reason(
                            &head_card_id,
                            &head_run_id,
                            Some(&format!("Merge queue waiting: {reason}")),
                        )?;
                        return Ok((false, changed));
                    }
                    return Err(e);
                }
            }
        };
        for (card, run) in &candidates {
            self.set_queue_pause_reason(&card.id, &run.id, None)?;
        }

        let short_id = self.next_id();
        let batch_id = format!("merge-queue:{project_id}:{short_id}");
        let worktree_path =
            canban_workspaces::batch::merge_queue_worktree_path(&self.home, &project);
        let started_at = self.now_ms();
        let placeholder_items: Vec<MergeBatchItem> = candidates
            .iter()
            .map(|(card, run)| MergeBatchItem {
                card_id: card.id.clone(),
                card_number: card.number,
                workflow_run_id: run.id.clone(),
                branch: card.branch.clone().unwrap_or_default(),
                worktree: card.worktree.clone().unwrap_or_default(),
                base_head: String::new(),
                integrated_commit: None,
                evicted: false,
            })
            .collect();
        let mut batch = MergeBatch {
            id: batch_id.clone(),
            project_id: project_id.to_owned(),
            profile_id: cfg.conflict_profile.clone(),
            provider_session_id: None,
            target_branch: resolved_branch.clone(),
            target_head: target_head.clone(),
            strategy: cfg.strategy,
            sync_upstream: cfg.sync_upstream,
            test_command: cfg.test_command.clone(),
            worktree: worktree_path.to_string_lossy().into_owned(),
            state: "integrating".into(),
            failure_kind: None,
            conflict_card_number: None,
            conflict_files: Vec::new(),
            test_output: None,
            items: placeholder_items,
            prompt: String::new(),
            error: None,
            queued_at: None,
            started_at: None,
            created_at: started_at,
            updated_at: started_at,
        };
        self.store.lock().unwrap().write(|db| db.save(&batch))?;
        self.emit("review", None);

        let cards_with_runs: Vec<(Card, String)> = candidates
            .iter()
            .map(|(card, run)| (card.clone(), run.id.clone()))
            .collect();
        let integration = ops.integrate_batch(
            &self.home,
            &project,
            &resolved_branch,
            &target_head,
            cfg.strategy,
            &cards_with_runs,
        );
        let outcome = match integration {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = self
                    .store
                    .lock()
                    .unwrap()
                    .write(|db| delete_batch_row(db, &batch_id));
                self.emit("review", None);
                return Err(err);
            }
        };

        match outcome {
            BatchIntegrationOutcome::Clean {
                worktree,
                target_branch,
                target_head,
                head_sha,
                items,
            } => {
                batch.worktree = worktree.to_string_lossy().into_owned();
                batch.items = items.clone();
                batch.updated_at = self.now_ms();
                if let Some(cmd) = cfg.test_command.as_deref() {
                    batch.state = "testing".into();
                    self.store.lock().unwrap().write(|db| db.save(&batch))?;
                    self.emit("review", None);
                    if let Some(hook) = self.on_batch_tests_hook.lock().unwrap().clone() {
                        hook(&project.id, &worktree);
                    }
                    let log_path = self
                        .home
                        .join("terminals")
                        .join(&batch_id)
                        .join("batch-test.log");
                    let test_outcome = match ops.run_batch_tests(&worktree, cmd, &log_path) {
                        Ok(outcome) => outcome,
                        Err(err) => {
                            let _ = self
                                .store
                                .lock()
                                .unwrap()
                                .write(|db| delete_batch_row(db, &batch_id));
                            self.emit("review", None);
                            return Err(err);
                        }
                    };
                    if !test_outcome.passed {
                        batch.failure_kind = Some("test_failure".into());
                        batch.test_output = Some(test_outcome.tail);
                        let advance =
                            self.handoff_failed_batch(&project, &cfg, &mut batch, &candidates)?;
                        return Ok((advance, true));
                    }
                }

                let push_result = {
                    let ff_holder = self.checkout_holder_label(
                        &format!("Batch code delivery ({})", batch.short_id()),
                        &project,
                        Some(&target_branch),
                    );
                    let _checkout_gate = self.lock_checkout(&project, ff_holder);
                    match ops.fast_forward_target(&project, &target_branch, &target_head, &head_sha)
                    {
                        Ok(()) => {}
                        Err(err) => {
                            drop(_checkout_gate);
                            let _ = self
                                .store
                                .lock()
                                .unwrap()
                                .write(|db| delete_batch_row(db, &batch_id));
                            match err.downcast_ref::<DeliveryError>() {
                                Some(DeliveryError::TargetAdvanced) => {
                                    let now_ms = self.now_ms();
                                    self.store.lock().unwrap().write(|db| {
                                        for (card, run) in &candidates {
                                            if let Ok(c) = db.get::<Card>(&card.id) {
                                                log(
                                                    db,
                                                    &c,
                                                    "review.merge_retry",
                                                    "system",
                                                    "Target branch advanced during batch delivery; retrying",
                                                    None,
                                                    json!({"target_branch":target_branch,"batch_id":batch_id,"workflow_run_id":run.id}),
                                                    now_ms,
                                                )?;
                                            }
                                        }
                                        Ok(())
                                    })?;
                                    self.emit("review", None);
                                    return Ok((true, true));
                                }
                                Some(DeliveryError::MainCheckoutBusy { reason }) => {
                                    let changed = self.set_queue_pause_reason(
                                        &head_card_id,
                                        &head_run_id,
                                        Some(&format!("Merge queue waiting: {reason}")),
                                    )?;
                                    self.emit("review", None);
                                    return Ok((false, changed));
                                }
                                _ => return Err(err),
                            }
                        }
                    }

                    if cfg.sync_upstream {
                        Some(ops.push_target_branch(&project, &target_branch))
                    } else {
                        None
                    }
                };
                if matches!(&push_result, Some(Ok(Some(_))))
                    && let Err(err) =
                        self.check_failpoint(canban_types::FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE)
                {
                    let _ = self
                        .store
                        .lock()
                        .unwrap()
                        .write(|db| delete_batch_row(db, &batch_id));
                    self.emit("review", None);
                    return Err(err);
                }
                if let Err(err) = self.finalize_clean_batch(
                    &project,
                    &mut batch,
                    &head_sha,
                    push_result,
                    true,
                    "Auto-merged",
                    "system",
                ) {
                    let _ = self
                        .store
                        .lock()
                        .unwrap()
                        .write(|db| delete_batch_row(db, &batch_id));
                    self.emit("review", None);
                    return Err(err);
                }
                Ok((true, true))
            }
            BatchIntegrationOutcome::Conflict {
                worktree,
                items,
                conflict_card_number,
                conflict_files,
                ..
            } => {
                batch.worktree = worktree.to_string_lossy().into_owned();
                batch.items = items;
                batch.failure_kind = Some("conflict".into());
                batch.conflict_card_number = Some(conflict_card_number);
                batch.conflict_files = conflict_files;
                let advance = self.handoff_failed_batch(&project, &cfg, &mut batch, &candidates)?;
                Ok((advance, true))
            }
        }
    }

    fn handoff_failed_batch(
        &self,
        project: &Project,
        cfg: &DeliverConfig,
        batch: &mut MergeBatch,
        candidates: &[(Card, WorkflowRun)],
    ) -> Result<bool> {
        let config = self.config();
        let ops = self.ops();
        let is_conflict = batch.failure_kind.as_deref() == Some("conflict");

        let offending = if is_conflict {
            let num = batch.conflict_card_number.unwrap_or(0);
            candidates
                .iter()
                .find(|(c, _)| c.number == num)
                .unwrap_or(&candidates[0])
        } else {
            &candidates[0]
        };
        let attempts_used = offending.1.inputs["conflict_attempts"]
            .as_u64()
            .unwrap_or(0) as u32;
        let profile_res = canban_scheduler::select_in_project(
            &config,
            Some(project),
            Some(&cfg.conflict_profile),
        );
        let budget_exhausted = attempts_used >= cfg.max_conflict_attempts;

        if profile_res.is_err() || budget_exhausted {
            let _ = ops.ensure_merge_queue_worktree(&self.home, project, &batch.target_head);
            let reason = match profile_res {
                Err(err) => format!("Auto-merge paused: {err}"),
                Ok(_) if is_conflict => format!(
                    "Merge conflict with {} in batch after {} automatic attempt(s); resolve manually before merging",
                    batch.target_branch, cfg.max_conflict_attempts
                ),
                Ok(_) => format!(
                    "Batch tests failed after {} automatic attempt(s); review and fix before merging",
                    cfg.max_conflict_attempts
                ),
            };
            let offending_card_id = offending.0.id.clone();
            let offending_run_id = offending.1.id.clone();
            self.store.lock().unwrap().write(|db| {
                delete_batch_row(db, &batch.id)?;
                let mut card: Card = db.get(&offending_card_id)?;
                let mut run: WorkflowRun = db.get(&offending_run_id)?;
                if run.state == "waiting_delivery" {
                    let now_ms = self.now_ms();
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db, &mut card, &mut run, &reason, now_ms,
                    )?;
                    log(
                        db,
                        &card,
                        if is_conflict {
                            "review.merge_conflict"
                        } else {
                            "review.batch_test_failed"
                        },
                        "system",
                        &reason,
                        None,
                        json!({"batch_id":batch.id,"workflow_run_id":run.id}),
                        now_ms,
                    )?;
                }
                Ok(())
            })?;
            self.emit("review", Some(offending_card_id));
            return Ok(true);
        }

        let card_heads: Vec<(u64, String)> = batch
            .items
            .iter()
            .map(|i| (i.card_number, i.base_head.clone()))
            .collect();
        ops.write_batch_refs(project, batch.short_id(), &batch.target_head, &card_heads)?;

        let card_map: std::collections::BTreeMap<String, Card> = candidates
            .iter()
            .map(|(c, _)| (c.id.clone(), c.clone()))
            .collect();
        let queued_now = self.now_ms();
        batch.prompt = render_batch_prompt(project, batch, &card_map);
        batch.state = "queued".into();
        batch.queued_at = Some(queued_now);
        batch.updated_at = queued_now;

        let offending_num = offending.0.number;
        let step_key_fallback = "deliver".to_owned();
        let entry_name = if batch.profile_id == "merger" {
            "@merger".to_owned()
        } else {
            format!("Merger · @{}", batch.profile_id)
        };
        let agent_engine = profile_res.as_ref().ok().map(profile_agent_engine);
        let failure_kind = batch.failure_kind.clone().unwrap_or_default();
        self.store.lock().unwrap().write(|db| {
            let now_ms = self.now_ms();
            db.save(batch)?;
            let number = db.counter("terminal-number")?;
            let entry = TerminalEntry {
                id: batch.id.clone(),
                conversation_id: None,
                number,
                name: Some(entry_name.clone()),
                origin_project_id: Some(batch.project_id.clone()),
                origin_card_id: None,
                agent_engine: agent_engine.clone(),
                runtime: None,
                agent: None,
                created_at: now_ms,
            };
            db.save(&entry)?;
            for (card, _) in candidates {
                let current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(
                    &batch
                        .items
                        .iter()
                        .find(|i| i.card_id == card.id)
                        .unwrap()
                        .workflow_run_id,
                )?;
                if current_run.state != "waiting_delivery" {
                    continue;
                }
                if !is_conflict || card.number == offending_num {
                    let prev = current_run.inputs["conflict_attempts"]
                        .as_u64()
                        .unwrap_or(0);
                    current_run.inputs["conflict_attempts"] = json!(prev + 1);
                }
                current_run.state = "waiting_batch".into();
                current_run.terminal_id = Some(batch.id.clone());
                current_run.inputs["batch_id"] = json!(batch.id);
                current_run.updated_at = now_ms;
                let step_key = current_run
                    .step
                    .clone()
                    .unwrap_or_else(|| step_key_fallback.clone());
                current_run.history.push(json!({
                    "step": step_key,
                    "visit": current_run.visit,
                    "decision": if is_conflict { "merge_conflict" } else { "batch_test_failure" },
                    "profile_id": batch.profile_id,
                    "batch_id": batch.id,
                    "at": now_ms,
                }));
                db.save(&current_run)?;
                let msg = if is_conflict {
                    format!(
                        "Batch merge conflict on Card #{offending_num}; @{} queued in _merge_queue",
                        batch.profile_id
                    )
                } else {
                    format!(
                        "Batch test verification failed; @{} queued in _merge_queue",
                        batch.profile_id
                    )
                };
                log(
                    db,
                    &current_card,
                    if is_conflict {
                        "review.merge_conflict"
                    } else {
                        "review.batch_test_failed"
                    },
                    "system",
                    &msg,
                    Some(batch.id.clone()),
                    json!({
                        "batch_id": batch.id,
                        "failure_kind": failure_kind,
                        "conflict_card_number": batch.conflict_card_number,
                        "conflict_files": batch.conflict_files,
                    }),
                    now_ms,
                )?;
            }
            Ok(())
        })?;
        self.emit("terminals", None);
        self.emit("review", None);
        let _ = self.pump_merge_batches();
        self.wake_runtime();
        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_clean_batch(
        &self,
        project: &Project,
        batch: &mut MergeBatch,
        head_sha: &str,
        push_result: Option<Result<Option<String>>>,
        delete_transient_row: bool,
        verb: &str,
        actor: &str,
    ) -> Result<()> {
        let config = self.config();
        let short_id = batch.short_id().to_owned();
        let batch_id = batch.id.clone();
        let target_branch = batch.target_branch.clone();
        let active_items: Vec<MergeBatchItem> =
            batch.items.iter().filter(|i| !i.evicted).cloned().collect();
        let batch_size = active_items.len();

        let (push_ok, remote_name, push_err_msg) = match push_result {
            Some(Ok(Some(remote))) => (true, Some(remote), None),
            Some(Ok(None)) => (true, None, Some("no_remote".to_owned())),
            Some(Err(err)) => (
                false,
                None,
                Some(format!(
                    "Merged locally into {target_branch}, but syncing upstream failed: {err:#}"
                )),
            ),
            None => (true, None, None),
        };

        let mut merged_cards = Vec::new();
        self.store.lock().unwrap().write(|db| {
            let now_ms = self.now_ms();
            if delete_transient_row {
                delete_batch_row(db, &batch_id)?;
            } else {
                batch.state = "completed".into();
                batch.updated_at = now_ms;
                db.save(batch)?;
            }
            for item in &active_items {
                let mut card: Card = db.get(&item.card_id)?;
                let mut run: WorkflowRun = db.get(&item.workflow_run_id)?;
                if !matches!(run.state.as_str(), "waiting_delivery" | "waiting_batch") {
                    continue;
                }
                let resolved_at = now_ms;
                card.delivery_mode = Some(DeliveryMode::Local);
                card.delivery = Some(Delivery {
                    target_branch: target_branch.clone(),
                    commit: head_sha.to_owned(),
                    request_key: format!("batch:{short_id}:{}", card.number),
                    merged_at: resolved_at,
                });
                card.resolution = Some(CardResolution {
                    resolved_at,
                    kind: "merged".into(),
                    reason: None,
                    actor: actor.into(),
                });
                card.attention = None;
                card.human_gate = None;
                card.revision += 1;
                clear_batch_inputs(&mut run);
                if !push_ok {
                    let reason = push_err_msg.clone().unwrap_or_default();
                    db.save(&card)?;
                    log(
                        db,
                        &card,
                        "review.upstream_sync_failed",
                        actor,
                        &reason,
                        if delete_transient_row {
                            None
                        } else {
                            Some(batch_id.clone())
                        },
                        json!({"commit":head_sha,"target_branch":target_branch,"batch_id":batch_id,"workflow_run_id":run.id}),
                        resolved_at,
                    )?;
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db, &mut card, &mut run, &reason, resolved_at,
                    )?;
                } else {
                    let synced = remote_name.is_some();
                    let msg = match (&remote_name, push_err_msg.as_deref()) {
                        (Some(r), _) => format!(
                            "{verb} batch ({batch_size} card(s)) into {target_branch} and synced to {r}/{target_branch}"
                        ),
                        (None, Some("no_remote")) => {
                            format!("{verb} batch ({batch_size} card(s)) into {target_branch}")
                        }
                        _ => format!(
                            "{verb} batch ({batch_size} card(s)) into {target_branch} (local only, upstream sync disabled)"
                        ),
                    };
                    let queued_at = run.inputs["queued_at"].as_u64();
                    let mut merged_data = json!({"commit":head_sha,"target_branch":target_branch,"synced_upstream":synced,"batch_id":batch_id,"batch_size":batch_size,"workflow_run_id":run.id});
                    let mut delivery_out = json!({"state":"merged","commit":head_sha,"target_branch":target_branch,"synced_upstream":synced,"batch_id":batch_id,"batch_size":batch_size});
                    if let Some(q) = queued_at {
                        merged_data["queued_at"] = json!(q);
                        delivery_out["queued_at"] = json!(q);
                    }
                    log(
                        db,
                        &card,
                        "review.merged",
                        actor,
                        &msg,
                        if delete_transient_row {
                            None
                        } else {
                            Some(batch_id.clone())
                        },
                        merged_data,
                        resolved_at,
                    )?;
                    canban_workflow_engine::runtime::complete_delivery_at(
                        db,
                        &config,
                        &mut card,
                        &mut run,
                        delivery_out,
                        enqueue,
                        resolved_at,
                    )?;
                    merged_cards.push(card.id.clone());
                }
            }
            Ok(())
        })?;

        for cid in merged_cards {
            let _ = self.try_auto_archive_resolved(&cid);
        }
        self.git_changed();
        self.request_scripts();
        self.emit("review", None);
        self.wake_runtime();
        let _ = project;
        Ok(())
    }

    /// Launches the oldest queued `MergeBatch` into `<home>/worktrees/<ns>/_merge_queue` when a
    /// global scheduler slot is available, pre-creating and trusting all participating paths for `agy`.
    pub(super) fn pump_merge_batches(&self) -> Result<()> {
        let config = self.config();
        let snap = self.snapshot()?;
        if canban_scheduler::active_count_at(&snap, self.now_ms()) >= config.scheduler.max_agents {
            return Ok(());
        }
        let next = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let mut queued: Vec<MergeBatch> = db
                .list::<MergeBatch>()?
                .into_iter()
                .filter(|b| b.state == "queued")
                .collect();
            queued.sort_by_key(|b| b.created_at);
            let mut chosen = None;
            for b in queued {
                let entry_ok = db
                    .get::<TerminalEntry>(&b.id)
                    .ok()
                    .is_some_and(|e| e.runtime.as_ref().is_none_or(|r| r.state == "finished"));
                if entry_ok {
                    chosen = Some(b);
                    break;
                }
            }
            chosen
        };
        let Some(mut batch) = next else {
            return Ok(());
        };
        let (project, entry_opt) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            (
                db.get::<Project>(&batch.project_id)?,
                db.get::<TerminalEntry>(&batch.id).ok(),
            )
        };
        let Some(mut entry) = entry_opt else {
            return Ok(());
        };
        let client = self.herdr_client();
        // Leave the batch queued if Herdr Core is not reachable yet (e.g. startup or non-runtime tests).
        let Ok(core) = self.verified_core_id() else {
            return Ok(());
        };
        let profile =
            canban_scheduler::select_in_project(&config, Some(&project), Some(&batch.profile_id))?;

        let batch_run_dir = self.home.join("terminals").join(&batch.id);
        std::fs::create_dir_all(&self.home)?;
        std::fs::create_dir_all(&batch.worktree)?;
        std::fs::create_dir_all(&batch_run_dir)?;

        let mut extra_paths = vec![
            batch.worktree.clone(),
            project.directory.clone(),
            batch_run_dir.to_string_lossy().into_owned(),
        ];
        for item in &batch.items {
            if !item.worktree.is_empty() && Path::new(&item.worktree).exists() {
                extra_paths.push(item.worktree.clone());
            }
        }
        let extra_dirs = extra_paths.join("\0");

        let session_id = profile
            .command()
            .iter()
            .any(|s| s.contains("{session_id}"))
            .then(|| self.next_uuid());
        let mut argv = canban_profiles::command(
            &profile,
            session_id.as_deref(),
            true,
            &[
                ("home", &self.home.to_string_lossy()),
                ("cwd", &batch.worktree),
                ("project_dir", &project.directory),
                ("extra_dirs", &extra_dirs),
                ("cli", &self.cli.to_string_lossy()),
                ("prompt", &batch.prompt),
            ],
        )?;
        let is_herdr_template = profile
            .preset
            .as_deref()
            .is_some_and(|s| s.starts_with("herdr:"));
        if !argv.is_empty() && !is_herdr_template {
            argv[0] = canban_profiles::executable_with_runner(
                &argv[0],
                &client.config.hc_env,
                self.exec_runner.as_ref(),
            );
        }

        let mut env = std::collections::BTreeMap::from([
            (
                "CANBAN_HOME".into(),
                self.home.to_string_lossy().into_owned(),
            ),
            (
                "CANBAN_ENDPOINT".into(),
                self.endpoint().to_string_lossy().into_owned(),
            ),
            ("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned()),
            ("CANBAN_PROJECT".into(), batch.project_id.clone()),
            ("CANBAN_BATCH_ID".into(), batch.id.clone()),
            ("CANBAN_PROFILE_ID".into(), profile.id.clone()),
            (
                "CANBAN_RUN_DIR".into(),
                batch_run_dir.to_string_lossy().into_owned(),
            ),
        ]);
        if let Some(sid) = &session_id {
            env.insert("CANBAN_SESSION_ID".into(), sid.clone());
        }
        let labels = std::collections::BTreeMap::from([
            ("canban.terminal".into(), batch.id.clone()),
            ("canban.project".into(), batch.project_id.clone()),
            ("canban.merge_batch".into(), batch.short_id().to_owned()),
            ("canban.profile".into(), profile.id.clone()),
        ]);

        let mut runtime = TerminalRuntime {
            core_id: Some(core.clone()),
            terminal: None,
            state: "starting".into(),
            connection: "connecting".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
            command: None,
        };
        let launch_res = self.launch_herdr_with_env(crate::env::HerdrLaunchRequest {
            profile: Some(&profile),
            project: Some(&project),
            protected_context: &env,
            workflow_env: None,
            source_trust: Some("local"),
            argv,
            cwd: batch.worktree.clone(),
            labels,
            size: None,
        });
        match launch_res.map(|o| o.receipt) {
            Ok(receipt) => {
                let address = strp(&receipt, "address")?.to_owned();
                let started_ms = self.now_ms();
                runtime.terminal = Some(address.clone());
                runtime.state = "running".into();
                runtime.connection = "connected".into();
                batch.state = "running".into();
                batch.started_at = Some(started_ms);
                batch.error = None;
                batch.updated_at = started_ms;
                {
                    let mut inv = self.inventory.lock().unwrap();
                    if inv.core == core {
                        inv.records.entry(address.clone()).or_insert_with(|| {
                            let mut seeded = receipt.clone();
                            seeded["address"] = json!(address);
                            if seeded["state"].is_null() {
                                seeded["state"] = json!("live");
                            }
                            if seeded["cwd"].is_null() {
                                seeded["cwd"] = json!(batch.worktree);
                            }
                            seeded
                        });
                    }
                }
            }
            Err(err) => {
                let msg = format!("{err:#}; launch was not replayed");
                runtime.state = "unknown".into();
                runtime.connection = "unavailable".into();
                runtime.error = Some(msg.clone());
                batch.error = Some(msg);
                batch.updated_at = self.now_ms();
            }
        }
        entry
            .agent_engine
            .get_or_insert_with(|| profile_agent_engine(&profile));
        entry.runtime = Some(runtime);
        self.store.lock().unwrap().write(|db| {
            db.save(&entry)?;
            db.save(&batch)
        })?;
        if let Ok(value) = self.terminal_context(json!({"terminal_id": entry.id}))
            && let Ok(context) = serde_json::from_value::<TerminalContext>(value)
        {
            self.upsert_terminal_context(context);
        }
        self.emit("terminals", None);
        self.emit("review", None);
        Ok(())
    }

    /// Marks any `running` merge batch whose `@merger` terminal exited without calling
    /// `batch.report` as `failed` and moves waiting Cards to human approval with retry available.
    pub(super) fn reconcile_merge_batches(&self) -> Result<()> {
        let exited: Vec<(MergeBatch, TerminalEntry)> = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let mut out = Vec::new();
            for b in db.list::<MergeBatch>()? {
                if b.state == "running"
                    && let Ok(t) = db.get::<TerminalEntry>(&b.id)
                    && t.runtime.as_ref().is_some_and(|r| r.state == "finished")
                {
                    out.push((b, t));
                }
            }
            out
        };
        if exited.is_empty() {
            return Ok(());
        }
        let ops = self.ops();
        for (mut batch, _) in exited {
            if let Ok(project) = self
                .store
                .lock()
                .unwrap()
                .db()
                .get::<Project>(&batch.project_id)
            {
                ops.cleanup_batch_refs(&project, batch.short_id());
            }
            let reason = format!(
                "Batch merger @{} exited without calling `canban batch report`",
                batch.profile_id
            );
            batch.state = "failed".into();
            batch.error = Some(reason.clone());
            batch.updated_at = self.now_ms();
            self.fail_batch_runs(&mut batch, "system", "execution.failed", &reason)?;
        }
        self.request_deliveries();
        self.emit("review", None);
        Ok(())
    }

    fn fail_batch_runs(
        &self,
        batch: &mut MergeBatch,
        actor: &str,
        event_kind: &str,
        reason: &str,
    ) -> Result<()> {
        self.store.lock().unwrap().write(|db| {
            let now_ms = self.now_ms();
            db.save(batch)?;
            for item in batch.items.iter().filter(|i| !i.evicted) {
                let Ok(mut card) = db.get::<Card>(&item.card_id) else {
                    continue;
                };
                let Ok(mut run) = db.get::<WorkflowRun>(&item.workflow_run_id) else {
                    continue;
                };
                if run.state != "waiting_batch" {
                    continue;
                }
                if event_kind == "execution.failed" {
                    run.state = "failed".into();
                    run.error = Some(reason.to_owned());
                    clear_batch_inputs(&mut run);
                    run.updated_at = now_ms;
                    card.attention = Some(reason.to_owned());
                    card.human_gate = Some(HumanGate {
                        reason: reason.to_owned(),
                        terminal_id: Some(batch.id.clone()),
                        next_stage: None,
                    });
                    card.revision += 1;
                    db.save(&run)?;
                    db.save(&card)?;
                    db.save(&canban_notifications::attention_at(
                        &card.project_id,
                        &card.id,
                        reason,
                        now_ms,
                    ))?;
                } else {
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db, &mut card, &mut run, reason, now_ms,
                    )?;
                }
                log(
                    db,
                    &card,
                    event_kind,
                    actor,
                    reason,
                    Some(batch.id.clone()),
                    json!({"batch_id":batch.id,"workflow_run_id":run.id}),
                    now_ms,
                )?;
            }
            Ok(())
        })
    }

    /// Retries Deliver queue entries that previously belonged to a `merge-queue:` batch.
    pub(super) fn retry_merge_batch_runs(
        &self,
        db: Db<'_>,
        p: &Value,
        entry: &TerminalEntry,
    ) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Retry requires an explicit user decision"
        );
        let batch = db.get::<MergeBatch>(&entry.id)?;
        ensure!(!batch.active(), "Merge batch is still active");
        let target_card = p["card_id"]
            .as_str()
            .and_then(|s| (!s.trim().is_empty()).then_some(s.trim().to_owned()));
        let mut retried = 0usize;
        let now_ms = self.now_ms();
        for item in &batch.items {
            if target_card
                .as_deref()
                .is_some_and(|cid| cid != item.card_id)
            {
                continue;
            }
            let Ok(mut card) = db.get::<Card>(&item.card_id) else {
                continue;
            };
            if card.archived_at.is_some() {
                continue;
            }
            let Ok(mut run) = db.get::<WorkflowRun>(&item.workflow_run_id) else {
                continue;
            };
            if !run.active() || !run.step_is_deliver() {
                continue;
            }
            if run.state == "waiting_approval" {
                let config = self.config();
                canban_workflow_engine::runtime::approve_with_feedback_at(
                    db, &config, &mut card, &mut run, false, None, enqueue, now_ms,
                )?;
            } else {
                canban_workflow_engine::runtime::requeue_delivery_at(
                    db, &mut card, &mut run, now_ms,
                )?;
            }
            log(
                db,
                &card,
                "execution.retry_queued",
                "user",
                "Queued in merge queue",
                Some(entry.id.clone()),
                json!({"terminal_id":entry.id,"workflow_run_id":run.id,"queued_for_delivery":true}),
                now_ms,
            )?;
            retried += 1;
        }
        ensure!(retried > 0, "No retryable Cards found in merge batch");
        Ok(serde_json::to_value(entry)?)
    }

    fn close_batch_terminal(&self, batch_id: &str) {
        let entry_opt = {
            let store = self.store.lock().unwrap();
            store.db().get::<TerminalEntry>(batch_id).ok()
        };
        let Some(mut entry) = entry_opt else {
            return;
        };
        let kill_target = entry.runtime.as_ref().and_then(|r| {
            if r.state != "finished" {
                r.core_id.clone().zip(r.terminal.clone())
            } else {
                None
            }
        });
        if let Some((core, terminal)) = kill_target {
            let client = self.herdr_client();
            if self.verified_core_id().ok().as_deref() == Some(core.as_str()) {
                let _ = client.call("terminal.kill", json!({"terminal": terminal}));
            }
        }
        let _ = self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(batch_id)?;
            if let Some(r) = &mut current.runtime {
                r.state = "finished".into();
            } else {
                current.runtime = Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "finished".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                    command: None,
                });
            }
            entry = current.clone();
            db.save(&current)
        });
        self.emit("terminals", None);
    }

    pub(super) fn resolve_batch_for_params(&self, db: Db<'_>, p: &Value) -> Result<MergeBatch> {
        if let Some(bid) = trimmed_opt(p["batch_id"].as_str())
            .or_else(|| trimmed_opt(p["batch"].as_str()))
            .or_else(|| {
                trimmed_opt(p["terminal_id"].as_str()).filter(|s| s.starts_with("merge-queue:"))
            })
        {
            if let Ok(b) = db.get::<MergeBatch>(&bid) {
                return Ok(b);
            }
            if let Some(b) = db
                .list::<MergeBatch>()?
                .into_iter()
                .find(|b| b.short_id() == bid)
            {
                return Ok(b);
            }
            bail!("Merge batch not found: {bid}");
        }
        let project_id = trimmed_opt(p["project_id"].as_str())
            .map(|raw| super::resolve_project_id(db, &raw))
            .transpose()?;
        let cwd = trimmed_opt(p["cwd"].as_str());
        let mut active: Vec<MergeBatch> = db
            .list::<MergeBatch>()?
            .into_iter()
            .filter(|b| b.active())
            .filter(|b| project_id.as_deref().is_none_or(|pid| b.project_id == pid))
            .filter(|b| {
                cwd.as_deref()
                    .is_none_or(|c| super::path_within(&b.worktree, c))
            })
            .collect();
        active.sort_by_key(|b| b.created_at);
        ensure!(
            !active.is_empty(),
            "No active merge batch matches the provided scope"
        );
        ensure!(
            active.len() == 1,
            "Multiple active merge batches found; pass --batch"
        );
        Ok(active.pop().unwrap())
    }

    /// `batch.bind`: captures a provider session ID onto an active `MergeBatch`.
    pub(super) fn batch_bind(&self, p: Value) -> Result<Value> {
        let session = strp(&p, "session_id")?;
        ensure!(
            !session.is_empty() && session.len() < 512,
            "Invalid provider session ID"
        );
        self.mutate("batch.bind", &p, |db| {
            let mut batch = self.resolve_batch_for_params(db, &p)?;
            batch.provider_session_id = Some(session.to_owned());
            batch.updated_at = self.now_ms();
            db.save(&batch)?;
            Ok(json!({
                "bound": true,
                "batch_id": batch.id,
                "session_id": session,
            }))
        })
    }

    /// `batch.report`: handles `working | completed | blocked | failed` outcomes (and optional
    /// `--evict` card numbers) from the batch `@merger` session.
    pub(super) fn batch_report(&self, p: Value) -> Result<Value> {
        let state = strp(&p, "state")?;
        ensure!(
            matches!(state, "working" | "completed" | "blocked" | "failed"),
            "invalid_report: batch.report supports working | completed | blocked | failed"
        );
        let key = strp(&p, "request_key")?;
        let fingerprint = canban_context::digest(&serde_json::to_vec(&p)?);
        let message = p["message"].as_str().unwrap_or("").trim().to_owned();

        let (batch, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            if let Some(old) = db
                .list::<Report>()?
                .into_iter()
                .find(|r| r.request_key == key)
            {
                ensure!(old.fingerprint == fingerprint, "request_key_conflict");
                return Ok(json!({"accepted":true,"duplicate":true}));
            }
            let batch = self.resolve_batch_for_params(db, &p)?;
            let project: Project = db.get(&batch.project_id)?;
            (batch, project)
        };
        ensure!(
            batch.active(),
            "Merge batch {} is already {}",
            batch.short_id(),
            batch.state
        );

        let evict_tokens: Vec<String> = p["evict"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        v.as_str()
                            .map(|s| s.trim().trim_start_matches('#').to_owned())
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let report_id = self.next_id();
        let report_path = if let Some(body) = p["body"].as_str() {
            let path = self
                .home
                .join("terminals")
                .join(&batch.id)
                .join("reports")
                .join(format!("{report_id}.md"));
            canban_context::atomic_with_durability(&path, body.as_bytes(), self.fs_durability)?;
            Some(canban_context::relative(&self.home, &path)?)
        } else {
            None
        };
        let profile_label = p["profile_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&batch.profile_id);
        let actor = format!("agent:{profile_label}");

        if state == "working" {
            let primary_card = batch
                .items
                .first()
                .map(|i| i.card_id.clone())
                .unwrap_or_default();
            self.store.lock().unwrap().write(|db| {
                let now_ms = self.now_ms();
                db.save(&Report {
                    id: report_id,
                    terminal_id: Some(batch.id.clone()),
                    workflow_run_id: None,
                    actor: actor.clone(),
                    project_id: batch.project_id.clone(),
                    card_id: primary_card,
                    state: state.into(),
                    message: message.clone(),
                    path: report_path.clone(),
                    request_key: key.into(),
                    fingerprint,
                    created_at: now_ms,
                })?;
                for item in &batch.items {
                    if let Ok(card) = db.get::<Card>(&item.card_id) {
                        log(
                            db,
                            &card,
                            "execution.reported",
                            &actor,
                            &format!("working: {message}"),
                            Some(batch.id.clone()),
                            json!({"batch_id":batch.id,"state":"working","path":report_path}),
                            now_ms,
                        )?;
                    }
                }
                Ok(())
            })?;
            return Ok(json!({
                "accepted": true,
                "duplicate": false,
                "recorded_state": "working",
                "batch_id": batch.id,
            }));
        }

        let mut updated_batch = batch.clone();
        for tok in &evict_tokens {
            let mut matched = false;
            for item in &mut updated_batch.items {
                if item.card_id == *tok || item.card_number.to_string() == *tok {
                    item.evicted = true;
                    matched = true;
                }
            }
            ensure!(
                matched,
                "Card `{tok}` is not part of batch {}",
                batch.short_id()
            );
        }

        let ops = self.ops();
        if state == "completed" {
            let card_ids: Vec<String> = updated_batch
                .items
                .iter()
                .filter(|i| !i.evicted)
                .map(|i| i.card_id.clone())
                .collect();
            let _merging = self.track_merging_many(&card_ids);
            let remaining: Vec<MergeBatchItem> = updated_batch
                .items
                .iter()
                .filter(|i| !i.evicted)
                .cloned()
                .collect();

            if remaining.is_empty() {
                let _ = ops.ensure_merge_queue_worktree(
                    &self.home,
                    &project,
                    &updated_batch.target_head,
                );
                ops.cleanup_batch_refs(&project, updated_batch.short_id());
                updated_batch.state = "completed".into();
                updated_batch.updated_at = self.now_ms();
                let evict_reason = if message.is_empty() {
                    format!("Evicted from merge batch by @{profile_label}")
                } else {
                    format!("Evicted from merge batch by @{profile_label}: {message}")
                };
                self.hold_evicted_batch_items(&mut updated_batch, &actor, &evict_reason, true)?;
                self.save_batch_report_record(
                    &updated_batch,
                    report_id,
                    &actor,
                    state,
                    &message,
                    report_path,
                    key,
                    fingerprint,
                )?;
                self.close_batch_terminal(&updated_batch.id);
                self.request_deliveries();
                self.emit("review", None);
                return Ok(json!({
                    "accepted": true,
                    "duplicate": false,
                    "recorded_state": "completed",
                    "batch_id": updated_batch.id,
                    "delivered": 0,
                    "evicted": updated_batch.items.len(),
                }));
            }

            let resolved_head = ops.verify_clean_batch_worktree(
                Path::new(&updated_batch.worktree),
                &updated_batch.target_head,
            )?;
            if let Some(cmd) = updated_batch.test_command.as_deref() {
                let log_path = self
                    .home
                    .join("terminals")
                    .join(&updated_batch.id)
                    .join("batch-test.log");
                let test_outcome =
                    ops.run_batch_tests(Path::new(&updated_batch.worktree), cmd, &log_path)?;
                if !test_outcome.passed {
                    bail!(
                        "Batch test command `{cmd}` failed (exit {:?}):\n{}",
                        test_outcome.exit_code,
                        test_outcome.tail
                    );
                }
            }
            let push_result = {
                let holder = self.checkout_holder_label(
                    &format!("Batch code delivery ({})", updated_batch.short_id()),
                    &project,
                    Some(&updated_batch.target_branch),
                );
                let _checkout_gate = self.lock_checkout(&project, holder);
                let ff_res = ops.fast_forward_target(
                    &project,
                    &updated_batch.target_branch,
                    &updated_batch.target_head,
                    &resolved_head,
                );
                if let Err(err) = ff_res {
                    let already_at_resolved_head = matches!(
                        err.downcast_ref::<DeliveryError>(),
                        Some(DeliveryError::TargetAdvanced)
                    ) && ops
                        .check_main_checkout_ready(&project, Some(&updated_batch.target_branch))
                        .is_ok_and(|(_, cur_head)| cur_head == resolved_head);
                    if !already_at_resolved_head {
                        return Err(err);
                    }
                }
                if updated_batch.sync_upstream {
                    Some(ops.push_target_branch(&project, &updated_batch.target_branch))
                } else {
                    None
                }
            };
            if matches!(&push_result, Some(Ok(Some(_)))) {
                self.check_failpoint(canban_types::FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE)?;
            }
            ops.cleanup_batch_refs(&project, updated_batch.short_id());

            let evict_reason = if message.is_empty() {
                format!("Evicted from merge batch by @{profile_label}")
            } else {
                format!("Evicted from merge batch by @{profile_label}: {message}")
            };
            let evicted_count = updated_batch.items.iter().filter(|i| i.evicted).count();
            if evicted_count > 0 {
                self.hold_evicted_batch_items(&mut updated_batch, &actor, &evict_reason, false)?;
            }
            self.finalize_clean_batch(
                &project,
                &mut updated_batch,
                &resolved_head,
                push_result,
                false,
                "Resolved & merged",
                &actor,
            )?;
            self.save_batch_report_record(
                &updated_batch,
                report_id,
                &actor,
                state,
                &message,
                report_path,
                key,
                fingerprint,
            )?;
            self.close_batch_terminal(&updated_batch.id);
            self.request_deliveries();
            return Ok(json!({
                "accepted": true,
                "duplicate": false,
                "recorded_state": "completed",
                "batch_id": updated_batch.id,
                "commit": resolved_head,
                "delivered": remaining.len(),
                "evicted": evicted_count,
            }));
        }

        // state is "blocked" | "failed"
        let _ = ops.ensure_merge_queue_worktree(&self.home, &project, &updated_batch.target_head);
        ops.cleanup_batch_refs(&project, updated_batch.short_id());
        let reason = if message.is_empty() {
            format!("Batch merge {state} (reported by @{profile_label})")
        } else {
            format!("@{profile_label}: {message}")
        };
        updated_batch.state = state.into();
        updated_batch.error = Some(reason.clone());
        updated_batch.updated_at = self.now_ms();
        self.fail_batch_runs(&mut updated_batch, &actor, "execution.reported", &reason)?;
        self.save_batch_report_record(
            &updated_batch,
            report_id,
            &actor,
            state,
            &message,
            report_path,
            key,
            fingerprint,
        )?;
        self.close_batch_terminal(&updated_batch.id);
        self.request_deliveries();
        self.emit("review", None);
        Ok(json!({
            "accepted": true,
            "duplicate": false,
            "recorded_state": state,
            "batch_id": updated_batch.id,
        }))
    }

    fn hold_evicted_batch_items(
        &self,
        batch: &mut MergeBatch,
        actor: &str,
        reason: &str,
        save_batch: bool,
    ) -> Result<()> {
        self.store.lock().unwrap().write(|db| {
            let now_ms = self.now_ms();
            if save_batch {
                db.save(batch)?;
            }
            for item in batch.items.iter().filter(|i| i.evicted) {
                let mut card: Card = db.get(&item.card_id)?;
                let mut run: WorkflowRun = db.get(&item.workflow_run_id)?;
                if run.state == "waiting_batch" {
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db, &mut card, &mut run, reason, now_ms,
                    )?;
                    log(
                        db,
                        &card,
                        "review.batch_evicted",
                        actor,
                        reason,
                        Some(batch.id.clone()),
                        json!({"batch_id":batch.id,"workflow_run_id":run.id}),
                        now_ms,
                    )?;
                }
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn save_batch_report_record(
        &self,
        batch: &MergeBatch,
        report_id: String,
        actor: &str,
        state: &str,
        message: &str,
        path: Option<String>,
        request_key: &str,
        fingerprint: String,
    ) -> Result<()> {
        let primary_card = batch
            .items
            .first()
            .map(|i| i.card_id.clone())
            .unwrap_or_default();
        self.store.lock().unwrap().write(|db| {
            db.save(&Report {
                id: report_id,
                terminal_id: Some(batch.id.clone()),
                workflow_run_id: None,
                actor: actor.to_owned(),
                project_id: batch.project_id.clone(),
                card_id: primary_card,
                state: state.to_owned(),
                message: message.to_owned(),
                path,
                request_key: request_key.to_owned(),
                fingerprint,
                created_at: self.now_ms(),
            })
        })
    }

    /// Cancels an active `MergeBatch` when its `merge-queue:` terminal is manually closed.
    pub(super) fn cancel_batch_from_terminal_close(
        &self,
        db: Db<'_>,
        batch: &mut MergeBatch,
        item: &mut TerminalEntry,
    ) -> Result<()> {
        if batch.state == "queued" {
            item.runtime = Some(TerminalRuntime {
                core_id: None,
                terminal: None,
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            });
            db.save(item)?;
        }
        let now_ms = self.now_ms();
        batch.state = "cancelled".into();
        batch.error = Some("Merge batch terminal closed by user".into());
        batch.updated_at = now_ms;
        db.save(batch)?;
        if let Ok(project) = db.get::<Project>(&batch.project_id) {
            let ops = self.ops();
            let _ = ops.ensure_merge_queue_worktree(&self.home, &project, &batch.target_head);
            ops.cleanup_batch_refs(&project, batch.short_id());
        }
        for batch_item in batch.items.iter().filter(|i| !i.evicted) {
            let Ok(mut card) = db.get::<Card>(&batch_item.card_id) else {
                continue;
            };
            let Ok(mut run) = db.get::<WorkflowRun>(&batch_item.workflow_run_id) else {
                continue;
            };
            if run.state == "waiting_batch" {
                canban_workflow_engine::runtime::hold_delivery_at(
                    db,
                    &mut card,
                    &mut run,
                    "Merge batch terminal closed by user; resume or retry to queue again",
                    now_ms,
                )?;
                log(
                    db,
                    &card,
                    "execution.close_requested",
                    "user",
                    "Merge batch terminal closed by user",
                    Some(batch.id.clone()),
                    json!({"batch_id":batch.id,"workflow_run_id":run.id}),
                    now_ms,
                )?;
            }
        }
        Ok(())
    }
}
