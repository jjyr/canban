//! Runtime status, user activity and idle Agent terminal reclamation.
use super::*;

/// Completed (`completed` / `backlog`) Agent runs reclaim their idle PTY after 30s
/// instead of waiting for the full interactive `scheduler.idle_minutes` window.
pub(super) const COMPLETED_IDLE_RECLAIM_MS: u64 = 30_000;
pub(super) const SHORT_RECLAIM_WARNING_MS: u64 = 15_000;
pub(super) const LONG_RECLAIM_WARNING_MS: u64 = 30_000;

pub(super) fn reclaim_warning_window_ms(delay_ms: u64) -> u64 {
    if delay_ms <= 60_000 {
        SHORT_RECLAIM_WARNING_MS.min(delay_ms)
    } else {
        LONG_RECLAIM_WARNING_MS.min(delay_ms)
    }
}

pub(super) fn is_interactive_followup(run: &AgentRun) -> bool {
    if !matches!(run.state.as_str(), "completed" | "backlog") {
        return false;
    }
    if run.lifecycle.interactive_followup {
        return true;
    }
    let observed_turn = run.observed.as_ref().and_then(|v| v["turn"].as_u64());
    run.lifecycle.idle_since.is_some()
        && run
            .completion_turn
            .zip(observed_turn)
            .is_some_and(|(ct, t)| t > ct)
}

pub(super) fn idle_reclaim_delay_ms(run: &AgentRun, minutes: u64) -> Option<u64> {
    if minutes == 0 {
        return None;
    }
    if matches!(run.state.as_str(), "completed" | "backlog") && !is_interactive_followup(run) {
        Some(COMPLETED_IDLE_RECLAIM_MS)
    } else {
        Some(minutes.saturating_mul(60_000))
    }
}

pub(super) fn idle_reclaim_anchor(run: &AgentRun) -> Option<u64> {
    let since = run.lifecycle.idle_since?;
    let mut anchor = since.max(run.lifecycle.last_interaction_at.unwrap_or(0));
    if matches!(run.state.as_str(), "completed" | "backlog") {
        anchor = anchor.max(run.completed_at.unwrap_or(0));
    }
    Some(anchor)
}

pub(super) fn idle_reclaim_deadline(
    run: &AgentRun,
    minutes: u64,
    extra_activity_at: u64,
) -> Option<u64> {
    let delay = idle_reclaim_delay_ms(run, minutes)?;
    let anchor = idle_reclaim_anchor(run)?.max(extra_activity_at);
    Some(anchor.saturating_add(delay))
}

fn is_non_idle_send_skip(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    msg.contains("\"kind\":\"status_mismatch\"") || msg.contains("\"kind\":\"blocked\"")
}

pub(super) fn idle_eligible_without_clock(
    snapshot: &Snapshot,
    run: &AgentRun,
    minutes: u64,
    at: u64,
) -> bool {
    if idle_reclaim_delay_ms(run, minutes).is_none() {
        return false;
    }
    if !matches!(run.runtime_status_at(at), "idle" | "done")
        || !run.close_state.is_empty()
        || run.pending.is_some()
    {
        return false;
    }
    if idle_reclaim_anchor(run).is_none() {
        return false;
    }
    if (canban_scheduler::automatic(run) || run.workflow_run_id.is_some())
        && !matches!(run.state.as_str(), "completed" | "backlog")
    {
        return false;
    }
    if !matches!(
        run.state.as_str(),
        "running" | "working" | "completed" | "backlog"
    ) {
        return false;
    }
    if snapshot.cards.iter().any(|c| {
        c.id == run.card_id
            && !matches!(run.state.as_str(), "completed" | "backlog")
            && (c.attention.is_some() || c.human_gate.is_some())
    }) {
        return false;
    }
    if snapshot
        .questions
        .iter()
        .any(|q| q.card_id == run.card_id && q.profile_id == run.profile.id && q.state != "replied")
    {
        return false;
    }
    true
}

pub(super) fn idle_candidate_with_extra(
    snapshot: &Snapshot,
    run: &AgentRun,
    minutes: u64,
    at: u64,
    extra_activity_at: u64,
    protected: bool,
) -> bool {
    if protected {
        return false;
    }
    let Some(delay_ms) = idle_reclaim_delay_ms(run, minutes) else {
        return false;
    };
    if !idle_eligible_without_clock(snapshot, run, minutes, at) {
        return false;
    }
    let Some(anchor) = idle_reclaim_anchor(run) else {
        return false;
    };
    at.saturating_sub(anchor.max(extra_activity_at)) >= delay_ms
}

#[cfg(test)]
pub(super) fn idle_candidate(
    snapshot: &Snapshot,
    run: &AgentRun,
    minutes: u64,
    at: u64,
    protected: bool,
) -> bool {
    idle_candidate_with_extra(snapshot, run, minutes, at, 0, protected)
}

impl AppServer {
    pub(super) fn observe_lifecycle(&self, entry: &TerminalEntry, observed: &Value) -> Result<()> {
        let Some(run) = &entry.agent else {
            return Ok(());
        };
        let activity = self.herdr_activity(
            run.core_id.as_deref().unwrap_or(""),
            run.terminal.as_deref().unwrap_or(""),
        );
        let status = observed["status"]["state"].as_str().unwrap_or("unknown");
        let previous = run.observed.as_ref().map(|v| &v["status"]);
        let changed = previous != Some(&observed["status"])
            || run.observed.as_ref().map(|v| &v["state"]) != Some(&observed["state"])
            || run.observed.as_ref().map(|v| &v["turn"]) != Some(&observed["turn"])
            || run.connection_state != "connected";
        let current_at = self.now_ms();
        let stall_crossed = matches!(status, "idle" | "done")
            && observed["state"] != "finished"
            && run.close_state.is_empty()
            && run.trigger.is_automatic()
            && matches!(run.state.as_str(), "running" | "working")
            && run.lifecycle.idle_since.is_some_and(|since| {
                let anchor = since
                    .max(run.lifecycle.last_interaction_at.unwrap_or(0))
                    .max(activity.last_at);
                let deadline =
                    anchor.saturating_add(canban_notifications::UNREPORTED_IDLE_ALERT_MS);
                current_at >= deadline && run.lifecycle.observed_at.unwrap_or(0) < deadline
            });
        // Poll freshness is durable, but avoid rewriting every unchanged poll.
        if !changed
            && !stall_crossed
            && run
                .lifecycle
                .observed_at
                .is_some_and(|t| current_at.saturating_sub(t) < 10_000)
            && activity.last_at <= run.lifecycle.last_interaction_at.unwrap_or(0)
        {
            return Ok(());
        }
        let freshness_only = !changed
            && !stall_crossed
            && status != "blocked"
            && run.lifecycle.waiting.is_none()
            && (matches!(status, "idle" | "done") == run.lifecycle.idle_since.is_some())
            && !(!matches!(status, "idle" | "done")
                && run.close_state == "error"
                && run.lifecycle.close_reason == "idle");
        let card_id = run.card_id.clone();
        self.store.lock().unwrap().write_topic(
            if freshness_only {
                "terminals"
            } else {
                "application"
            },
            |db| {
                let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                let Some(a) = &mut current_entry.agent else {
                    return Ok(());
                };
                a.lifecycle.observed_at = Some(self.now_ms());
                if activity.last_at > a.lifecycle.last_interaction_at.unwrap_or(0) {
                    a.lifecycle.last_interaction_at = Some(activity.last_at);
                }
                if freshness_only {
                    return db.save(&current_entry);
                }
                let was_settled = a.lifecycle.idle_since.is_some();
                let is_completed = matches!(a.state.as_str(), "completed" | "backlog");
                let observed_turn = observed["turn"].as_u64();
                let live = observed["state"] != "finished";
                if matches!(status, "idle" | "done") && live {
                    if is_completed {
                        if !was_settled && !a.lifecycle.interactive_followup {
                            if let Some(turn) = observed_turn {
                                if a.completion_turn
                                    .is_some_and(|ct| turn > ct.saturating_add(1))
                                {
                                    a.lifecycle.interactive_followup = true;
                                } else {
                                    a.completion_turn = Some(turn);
                                }
                            }
                        } else if was_settled
                            && a.completion_turn
                                .zip(observed_turn)
                                .is_some_and(|(ct, turn)| turn > ct)
                        {
                            a.lifecycle.interactive_followup = true;
                        }
                    }
                    if changed || a.lifecycle.idle_since.is_none() {
                        a.lifecycle.idle_since = Some(self.now_ms());
                    }
                } else {
                    if live
                        && is_completed
                        && matches!(status, "working" | "running" | "blocked")
                        && (was_settled
                            || a.completion_turn
                                .zip(observed_turn)
                                .is_some_and(|(ct, turn)| turn > ct))
                    {
                        a.lifecycle.interactive_followup = true;
                    }
                    a.lifecycle.idle_since = None;
                }
                if !matches!(status, "idle" | "done")
                    && a.close_state == "error"
                    && a.lifecycle.close_reason == "idle"
                {
                    a.lifecycle.close_reason.clear();
                }
                let card: Card = db.get(&a.card_id)?;
                if status == "blocked" && observed["state"] != "finished" {
                    let kind = observed["status"]["kind"]
                        .as_str()
                        .unwrap_or("input")
                        .to_owned();
                    let title = observed["status"]["title"]
                        .as_str()
                        .unwrap_or("Agent needs input")
                        .to_owned();
                    let message = observed["status"]["msg"].as_str().unwrap_or("").to_owned();
                    let old = a.lifecycle.waiting.as_ref();
                    if old
                        .is_none_or(|w| w.kind != kind || w.title != title || w.message != message)
                    {
                        let now_ms = self.now_ms();
                        let w = RuntimeWait {
                            id: old.map(|w| w.id.clone()).unwrap_or_else(|| self.next_id()),
                            since: old.map(|w| w.since).unwrap_or(now_ms),
                            kind,
                            title,
                            message,
                        };
                        let mut notice = canban_notifications::attention_at(
                            &a.project_id,
                            &a.card_id,
                            &format!("Agent input required: {} — {}", w.title, w.message),
                            now_ms,
                        );
                        notice.id = w.id.clone();
                        db.save(&notice)?;
                        log(
                            db,
                            &card,
                            "execution.input_required",
                            "herdr",
                            &notice.message,
                            Some(entry.id.clone()),
                            json!({"wait":w,"run_number":a.number}),
                            now_ms,
                        )?;
                        a.lifecycle.waiting = Some(w);
                    }
                } else if let Some(w) = a.lifecycle.waiting.take() {
                    if let Ok(mut n) = db.get::<Notice>(&w.id) {
                        n.read = true;
                        db.save(&n)?;
                    }
                    log(
                        db,
                        &card,
                        "execution.input_resolved",
                        "herdr",
                        "Runtime input wait ended; workflow approval is unchanged",
                        Some(entry.id.clone()),
                        json!({"wait_id":w.id,"run_number":a.number}),
                        self.now_ms(),
                    )?;
                }
                db.save(&current_entry)
            },
        )?;
        if matches!(status, "idle" | "done") {
            let _ = self.try_auto_archive_resolved(&card_id);
        }
        if freshness_only {
            self.emit("terminals", None);
        } else {
            self.emit("runtime", Some(card_id));
        }
        Ok(())
    }

    pub(super) fn compute_reclaim_status(
        &self,
        snapshot: &Snapshot,
        at: u64,
    ) -> Vec<TerminalReclaimStatus> {
        let idle_minutes = self.config().scheduler.idle_minutes;
        let mut out = Vec::new();
        for entry in &snapshot.terminals {
            let Some(run) = &entry.agent else {
                continue;
            };
            if run.finished() || run.terminal.is_none() {
                continue;
            }
            let auto_reclaiming =
                run.close_state == "closing" && run.lifecycle.close_reason == "idle";
            let paused = !auto_reclaiming
                && self
                    .presence
                    .is_protected(&entry.id, Some(&run.card_id), at);
            let interactive_followup = is_interactive_followup(run);
            let (deadline_at, reclaiming_soon) = if !auto_reclaiming
                && !paused
                && idle_eligible_without_clock(snapshot, run, idle_minutes, self.now_ms())
            {
                let activity = self.herdr_activity(
                    run.core_id.as_deref().unwrap_or(""),
                    run.terminal.as_deref().unwrap_or(""),
                );
                let viewed_at = self
                    .presence
                    .last_viewed_at(&entry.id, Some(&run.card_id), at);
                let extra = activity.last_at.max(viewed_at);
                if let Some(deadline) = idle_reclaim_deadline(run, idle_minutes, extra) {
                    let delay_ms = idle_reclaim_delay_ms(run, idle_minutes)
                        .unwrap_or(COMPLETED_IDLE_RECLAIM_MS);
                    let warn_window = reclaim_warning_window_ms(delay_ms);
                    (Some(deadline), at >= deadline.saturating_sub(warn_window))
                } else {
                    (None, false)
                }
            } else {
                (None, false)
            };
            out.push(TerminalReclaimStatus {
                terminal_id: entry.id.clone(),
                paused,
                reclaiming_soon,
                auto_reclaiming,
                interactive_followup,
                deadline_at,
            });
        }
        for run in &snapshot.scheduled_runs {
            if run.creates_card {
                continue;
            }
            let Some(tid) = run.terminal_id.as_deref() else {
                continue;
            };
            let Some(rt) = snapshot
                .terminals
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.runtime.as_ref())
            else {
                continue;
            };
            if rt.state == "finished" {
                continue;
            }
            let auto_reclaiming = run.close_requested_at.is_some() || rt.state == "closing";
            let paused = !auto_reclaiming && self.presence.is_protected(tid, None, at);
            let track = self.presence.get_scheduled_track(tid);
            let interactive_followup = track.as_ref().is_some_and(|t| t.interactive_followup);
            let (deadline_at, reclaiming_soon) = if !auto_reclaiming
                && !paused
                && matches!(run.state.as_str(), "completed" | "failed")
                && idle_minutes > 0
                && !matches!(
                    rt.agent_status.as_deref(),
                    Some("working" | "running" | "blocked")
                ) {
                let last_viewed = self.presence.last_viewed_at(tid, None, at);
                let delay_ms = if interactive_followup {
                    Some(idle_minutes.saturating_mul(60_000))
                } else if last_viewed > 0 {
                    Some(COMPLETED_IDLE_RECLAIM_MS)
                } else {
                    None
                };
                if let Some(delay_ms) = delay_ms {
                    let anchor = track
                        .as_ref()
                        .and_then(|t| t.idle_since)
                        .unwrap_or(0)
                        .max(last_viewed)
                        .max(run.completed_at.unwrap_or(0));
                    if anchor > 0 {
                        let deadline = anchor.saturating_add(delay_ms);
                        let warn_window = reclaim_warning_window_ms(delay_ms);
                        (Some(deadline), at >= deadline.saturating_sub(warn_window))
                    } else {
                        (None, false)
                    }
                } else {
                    (None, false)
                }
            } else {
                (None, false)
            };
            out.push(TerminalReclaimStatus {
                terminal_id: tid.to_owned(),
                paused,
                reclaiming_soon,
                auto_reclaiming,
                interactive_followup,
                deadline_at,
            });
        }
        out
    }

    pub(super) fn populate_reclaim_status(&self, snapshot: &mut Snapshot) {
        let at = self.now_ms();
        self.presence.prune_expired(at);
        snapshot.reclaim_status = self.compute_reclaim_status(snapshot, at);
    }

    pub(super) fn reclaim_wake_deadline(&self, snapshot: &Snapshot, at: u64) -> Option<u64> {
        self.presence.prune_expired(at);
        let mut earliest = self.presence.next_expiry(at);
        let mut consider = |ts: u64| {
            earliest = Some(earliest.map_or(ts, |cur| cur.min(ts)));
        };
        let idle_minutes = self.config().scheduler.idle_minutes;
        if idle_minutes == 0 {
            return earliest;
        }
        for entry in &snapshot.terminals {
            let Some(run) = &entry.agent else {
                continue;
            };
            if run.finished() || run.terminal.is_none() {
                continue;
            }
            if self
                .presence
                .is_protected(&entry.id, Some(&run.card_id), at)
            {
                continue;
            }
            if !idle_eligible_without_clock(snapshot, run, idle_minutes, self.now_ms()) {
                continue;
            }
            let activity = self.herdr_activity(
                run.core_id.as_deref().unwrap_or(""),
                run.terminal.as_deref().unwrap_or(""),
            );
            let viewed_at = self
                .presence
                .last_viewed_at(&entry.id, Some(&run.card_id), at);
            let extra = activity.last_at.max(viewed_at);
            if let Some(deadline) = idle_reclaim_deadline(run, idle_minutes, extra) {
                let delay_ms =
                    idle_reclaim_delay_ms(run, idle_minutes).unwrap_or(COMPLETED_IDLE_RECLAIM_MS);
                let warn_at = deadline.saturating_sub(reclaim_warning_window_ms(delay_ms));
                if at < warn_at {
                    consider(warn_at.saturating_add(1));
                } else if at < deadline {
                    consider(deadline.saturating_add(1));
                } else {
                    consider(at.saturating_add(5_000));
                }
            }
        }
        for run in &snapshot.scheduled_runs {
            if run.creates_card
                || !matches!(run.state.as_str(), "completed" | "failed")
                || run.close_requested_at.is_some()
            {
                continue;
            }
            let Some(tid) = run.terminal_id.as_deref() else {
                continue;
            };
            let Some(rt) = snapshot
                .terminals
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.runtime.as_ref())
            else {
                continue;
            };
            if rt.state == "finished"
                || rt.state == "closing"
                || matches!(
                    rt.agent_status.as_deref(),
                    Some("working" | "running" | "blocked")
                )
            {
                continue;
            }
            if self.presence.is_protected(tid, None, at) {
                continue;
            }
            let last_viewed = self.presence.last_viewed_at(tid, None, at);
            let track = self.presence.get_scheduled_track(tid);
            let interactive_followup = track.as_ref().is_some_and(|t| t.interactive_followup);
            let delay_ms = if interactive_followup {
                Some(idle_minutes.saturating_mul(60_000))
            } else if last_viewed > 0 {
                Some(COMPLETED_IDLE_RECLAIM_MS)
            } else {
                None
            };
            if let Some(delay_ms) = delay_ms {
                let anchor = track
                    .as_ref()
                    .and_then(|t| t.idle_since)
                    .unwrap_or(0)
                    .max(last_viewed)
                    .max(run.completed_at.unwrap_or(0));
                if anchor > 0 {
                    let deadline = anchor.saturating_add(delay_ms);
                    let warn_at = deadline.saturating_sub(reclaim_warning_window_ms(delay_ms));
                    if at < warn_at {
                        consider(warn_at.saturating_add(1));
                    } else if at < deadline {
                        consider(deadline.saturating_add(1));
                    } else {
                        consider(at.saturating_add(5_000));
                    }
                }
            }
        }
        earliest
    }

    pub(super) fn close_idle(&self, client: &canban_herdr::Client) -> Result<()> {
        let snapshot = self.snapshot()?;
        let idle_minutes = self.config().scheduler.idle_minutes;
        let check_at = self.now_ms();
        self.presence.prune_expired(check_at);
        for entry in &snapshot.terminals {
            let Some(run) = &entry.agent else {
                continue;
            };
            let activity = self.herdr_activity(
                run.core_id.as_deref().unwrap_or(""),
                run.terminal.as_deref().unwrap_or(""),
            );
            let protected = self
                .presence
                .is_protected(&entry.id, Some(&run.card_id), check_at);
            let viewed_at = self
                .presence
                .last_viewed_at(&entry.id, Some(&run.card_id), check_at);
            let extra = activity.last_at.max(viewed_at);
            if !idle_candidate_with_extra(&snapshot, run, idle_minutes, check_at, extra, protected)
            {
                continue;
            }
            let terminal = run.terminal.as_deref().context("Idle terminal missing")?;
            if client.status()?["core"]["id"].as_str() != run.core_id.as_deref() {
                continue;
            }
            let actual = client.get(terminal)?;
            if actual["state"] != "live"
                || !matches!(actual["status"]["state"].as_str(), Some("idle" | "done"))
                || self.is_refined_working_now(client, entry, &actual)
            {
                continue;
            }
            let exit_action = run.profile.exit.clone();
            let close_note = match &exit_action {
                Some(ExitAction::Input { .. }) => {
                    "Closing an idle Agent using its normal exit command"
                }
                _ => "Closing an idle Agent terminal after idle timeout",
            };
            let execute = self.store.lock().unwrap().write(|db| {
                let current = db.snapshot()?;
                let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                let Some(a) = &mut current_entry.agent else {
                    return Ok(false);
                };
                let lock_at = self.now_ms();
                self.presence.prune_expired(lock_at);
                let protected =
                    self.presence
                        .is_protected(&current_entry.id, Some(&a.card_id), lock_at);
                let viewed_at =
                    self.presence
                        .last_viewed_at(&current_entry.id, Some(&a.card_id), lock_at);
                let activity = self.herdr_activity(run.core_id.as_deref().unwrap_or(""), terminal);
                if activity.last_at > a.lifecycle.last_interaction_at.unwrap_or(0) {
                    a.lifecycle.last_interaction_at = Some(activity.last_at);
                }
                let extra = activity.last_at.max(viewed_at);
                if !idle_candidate_with_extra(&current, a, idle_minutes, lock_at, extra, protected)
                {
                    return Ok(false);
                }
                a.lifecycle.close_reason = "idle".into();
                a.close_state = "closing".into();
                let close_at = self.now_ms();
                a.close_requested_at = Some(close_at);
                let card_id = a.card_id.clone();
                let run_num = a.number;
                db.save(&current_entry)?;
                log(
                    db,
                    &db.get::<Card>(&card_id)?,
                    "execution.idle_close_requested",
                    "system",
                    close_note,
                    Some(entry.id.clone()),
                    json!({"run_number":run_num}),
                    close_at,
                )?;
                Ok(true)
            })?;
            if !execute {
                continue;
            }
            let _ = self.capture_output(entry);
            enum CloseAttempt {
                Sent,
                SkippedNonIdle,
                AlreadyFinished,
                Failed(anyhow::Error),
            }
            let attempt = (|| -> CloseAttempt {
                let actual = match client.get(terminal) {
                    Ok(v) => v,
                    Err(e) => return CloseAttempt::Failed(e),
                };
                if actual["state"] == "finished" {
                    return CloseAttempt::AlreadyFinished;
                }
                let state = actual["status"]["state"].as_str().unwrap_or("");
                if actual["state"] != "live"
                    || !matches!(state, "idle" | "done")
                    || self.is_refined_working_now(client, entry, &actual)
                {
                    return CloseAttempt::SkippedNonIdle;
                }
                let res = if let Some(ExitAction::Input { text }) = &exit_action {
                    let mut params = json!({"terminal":terminal,"text":text});
                    if state == "idle" {
                        params["if"] = json!("idle");
                    }
                    client.call("terminal.send", params)
                } else {
                    client.call("terminal.kill", json!({"terminal":terminal}))
                };
                match res {
                    Ok(_) => CloseAttempt::Sent,
                    Err(e) if is_non_idle_send_skip(&e) => CloseAttempt::SkippedNonIdle,
                    Err(e) => CloseAttempt::Failed(e),
                }
            })();
            match attempt {
                CloseAttempt::Sent | CloseAttempt::AlreadyFinished => {}
                CloseAttempt::SkippedNonIdle => {
                    // Runtime left `idle`/`done` right before sending exit; roll back cleanly
                    // without capping retries so the next `idle`/`done` window reclaims normally.
                    self.store.lock().unwrap().write(|db| {
                        let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                        if let Some(a) = &mut current_entry.agent
                            && a.close_state == "closing"
                            && a.lifecycle.close_reason == "idle"
                        {
                            a.close_state.clear();
                            a.close_requested_at = None;
                            a.lifecycle.close_reason.clear();
                            db.save(&current_entry)?;
                        }
                        Ok(())
                    })?;
                }
                CloseAttempt::Failed(e) => {
                    self.store.lock().unwrap().write(|db| {
                        let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                        if let Some(a) = &mut current_entry.agent {
                            a.close_state = "error".into();
                            a.error = Some(format!("Idle exit was not confirmed: {e:#}. Inspect before retrying; no forced kill was sent."));
                            let err_msg = a.error.clone().unwrap();
                            let card_id = a.card_id.clone();
                            let proj_id = a.project_id.clone();
                            let run_num = a.number;
                            let card: Card = db.get(&card_id)?;
                            let now_ms = self.now_ms();
                            db.save(&canban_notifications::attention_at(
                                &proj_id, &card_id, &err_msg, now_ms,
                            ))?;
                            log(
                                db,
                                &card,
                                "execution.idle_close_failed",
                                "system",
                                &err_msg,
                                Some(entry.id.clone()),
                                json!({"run_number":run_num}),
                                now_ms,
                            )?;
                        }
                        db.save(&current_entry)
                    })?;
                }
            }
            self.emit("runtime", Some(run.card_id.clone()));
        }
        Ok(())
    }
}
