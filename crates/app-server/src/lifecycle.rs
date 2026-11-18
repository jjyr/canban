//! Runtime status, user activity and idle Agent terminal reclamation.
use super::*;

/// Completed (`completed` / `backlog`) Agent runs reclaim their idle PTY after 30s
/// instead of waiting for the full interactive `scheduler.idle_minutes` window.
pub(super) const COMPLETED_IDLE_RECLAIM_MS: u64 = 30_000;

pub(super) fn idle_reclaim_delay_ms(run: &AgentRun, minutes: u64) -> Option<u64> {
    if minutes == 0 {
        return None;
    }
    if matches!(run.state.as_str(), "completed" | "backlog") {
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

pub(super) fn idle_candidate(
    snapshot: &Snapshot,
    run: &AgentRun,
    minutes: u64,
    at: u64,
    _viewed: bool,
) -> bool {
    let Some(delay_ms) = idle_reclaim_delay_ms(run, minutes) else {
        return false;
    };
    if !matches!(run.runtime_status(), "idle" | "done")
        || !run.close_state.is_empty()
        || run.pending.is_some()
    {
        return false;
    }
    let Some(anchor) = idle_reclaim_anchor(run) else {
        return false;
    };
    if at.saturating_sub(anchor) < delay_ms {
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

impl AppServer {
    pub(super) fn observe_lifecycle(&self, entry: &TerminalEntry, observed: &Value) -> Result<()> {
        let Some(run) = &entry.agent else {
            return Ok(());
        };
        let activity = canban_herdr::activity::get(
            run.core_id.as_deref().unwrap_or(""),
            run.terminal.as_deref().unwrap_or(""),
        );
        let status = observed["status"]["state"].as_str().unwrap_or("unknown");
        let previous = run.observed.as_ref().map(|v| &v["status"]);
        let changed = previous != Some(&observed["status"])
            || run.observed.as_ref().map(|v| &v["state"]) != Some(&observed["state"])
            || run.observed.as_ref().map(|v| &v["turn"]) != Some(&observed["turn"])
            || run.connection_state != "connected";
        let current_at = now();
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
        let card_id = run.card_id.clone();
        self.store.lock().unwrap().write(|db| {
            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
            let Some(a) = &mut current_entry.agent else {
                return Ok(());
            };
            a.lifecycle.observed_at = Some(now());
            if activity.last_at > a.lifecycle.last_interaction_at.unwrap_or(0) {
                a.lifecycle.last_interaction_at = Some(activity.last_at);
            }
            if matches!(status, "idle" | "done") && observed["state"] != "finished" {
                if changed || a.lifecycle.idle_since.is_none() {
                    a.lifecycle.idle_since = Some(now());
                }
            } else {
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
                if old.is_none_or(|w| w.kind != kind || w.title != title || w.message != message) {
                    let w = RuntimeWait {
                        id: old.map(|w| w.id.clone()).unwrap_or_else(id),
                        since: old.map(|w| w.since).unwrap_or_else(now),
                        kind,
                        title,
                        message,
                    };
                    let mut notice = canban_notifications::attention(
                        &a.project_id,
                        &a.card_id,
                        &format!("Agent input required: {} — {}", w.title, w.message),
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
                )?;
            }
            db.save(&current_entry)
        })?;
        if matches!(status, "idle" | "done") {
            let _ = self.try_auto_archive_resolved(&card_id);
        }
        self.emit("runtime", Some(card_id));
        Ok(())
    }

    pub(super) fn close_idle(&self, client: &canban_herdr::Client) -> Result<()> {
        let snapshot = self.snapshot()?;
        for entry in &snapshot.terminals {
            let Some(run) = &entry.agent else {
                continue;
            };
            let activity = canban_herdr::activity::get(
                run.core_id.as_deref().unwrap_or(""),
                run.terminal.as_deref().unwrap_or(""),
            );
            let mut candidate = run.clone();
            candidate.lifecycle.last_interaction_at = Some(
                activity
                    .last_at
                    .max(run.lifecycle.last_interaction_at.unwrap_or(0)),
            );
            if !idle_candidate(
                &snapshot,
                &candidate,
                self.config().scheduler.idle_minutes,
                now(),
                activity.viewers > 0,
            ) {
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
                let activity =
                    canban_herdr::activity::get(run.core_id.as_deref().unwrap_or(""), terminal);
                a.lifecycle.last_interaction_at = Some(
                    activity
                        .last_at
                        .max(a.lifecycle.last_interaction_at.unwrap_or(0)),
                );
                if !idle_candidate(
                    &current,
                    a,
                    self.config().scheduler.idle_minutes,
                    now(),
                    activity.viewers > 0,
                ) {
                    return Ok(false);
                }
                a.lifecycle.close_reason = "idle".into();
                a.close_state = "closing".into();
                a.close_requested_at = Some(now());
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
                            db.save(&canban_notifications::attention(&proj_id, &card_id, &err_msg))?;
                            log(db, &card, "execution.idle_close_failed", "system", &err_msg, Some(entry.id.clone()), json!({"run_number":run_num}))?;
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
