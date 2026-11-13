//! Runtime status, user activity and idle Agent terminal reclamation.
use super::*;

pub(super) fn idle_candidate(
    snapshot: &Snapshot,
    run: &Attempt,
    minutes: u64,
    at: u64,
    _viewed: bool,
) -> bool {
    if minutes == 0
        || !matches!(run.runtime_status(), "idle" | "done")
        || !run.close_state.is_empty()
    {
        return false;
    }
    let Some(since) = run.lifecycle.idle_since else {
        return false;
    };
    if at.saturating_sub(since.max(run.lifecycle.last_interaction_at.unwrap_or(0)))
        < minutes.saturating_mul(60_000)
    {
        return false;
    }
    let Some(req) = snapshot.requests.iter().find(|r| r.id == run.request_id) else {
        return false;
    };
    if (canban_scheduler::automatic(req) || req.workflow_run_id.is_some())
        && !matches!(req.state.as_str(), "completed" | "backlog")
    {
        return false;
    }
    if !matches!(
        req.state.as_str(),
        "running" | "working" | "completed" | "backlog"
    ) {
        return false;
    }
    if snapshot.cards.iter().any(|c| {
        c.id == run.card_id
            && !matches!(req.state.as_str(), "completed" | "backlog")
            && (c.attention.is_some() || c.human_gate.is_some())
    }) {
        return false;
    }
    if snapshot
        .questions
        .iter()
        .any(|q| q.card_id == run.card_id && q.profile_id == req.profile.id && q.state != "replied")
    {
        return false;
    }
    true
}

impl AppServer {
    pub(super) fn observe_lifecycle(&self, run: &Attempt, observed: &Value) -> Result<()> {
        let activity = canban_herdr::activity::get(
            run.core_id.as_deref().unwrap_or(""),
            run.terminal.as_deref().unwrap_or(""),
        );
        let status = observed["status"]["state"].as_str().unwrap_or("unknown");
        let previous = run.observed.as_ref().map(|v| &v["status"]);
        let changed = previous != Some(&observed["status"])
            || run.observed.as_ref().map(|v| &v["state"]) != Some(&observed["state"])
            || run.connection_state != "connected";
        // Poll freshness is durable, but avoid rewriting every unchanged poll.
        if !changed
            && run
                .lifecycle
                .observed_at
                .is_some_and(|t| now().saturating_sub(t) < 10_000)
            && activity.last_at <= run.lifecycle.last_interaction_at.unwrap_or(0)
        {
            return Ok(());
        }
        self.store.lock().unwrap().write(|db| {
            let mut a: Attempt = db.get(&run.id)?;
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
                        Some(a.request_id.clone()),
                        json!({"wait":w,"attempt":a.number}),
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
                    Some(a.request_id.clone()),
                    json!({"wait_id":w.id,"attempt":a.number}),
                )?;
            }
            db.save(&a)
        })?;
        if matches!(status, "idle" | "done") {
            let _ = self.try_auto_archive_merged(&run.card_id);
        }
        self.emit("runtime", Some(run.card_id.clone()));
        Ok(())
    }

    pub(super) fn close_idle(&self, client: &canban_herdr::Client) -> Result<()> {
        let snapshot = self.snapshot()?;
        for run in &snapshot.attempts {
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
            {
                continue;
            }
            let Some(request) = snapshot.requests.iter().find(|r| r.id == run.request_id) else {
                continue;
            };
            let exit_action = request.profile.exit.clone();
            let close_note = match &exit_action {
                Some(ExitAction::Input { .. }) => {
                    "Closing an idle Agent using its normal exit command"
                }
                _ => "Closing an idle Agent terminal after idle timeout",
            };
            let execute = self.store.lock().unwrap().write(|db| {
                let current = db.snapshot()?;
                let mut a: Attempt = db.get(&run.id)?;
                let activity =
                    canban_herdr::activity::get(run.core_id.as_deref().unwrap_or(""), terminal);
                a.lifecycle.last_interaction_at = Some(
                    activity
                        .last_at
                        .max(a.lifecycle.last_interaction_at.unwrap_or(0)),
                );
                if !idle_candidate(
                    &current,
                    &a,
                    self.config().scheduler.idle_minutes,
                    now(),
                    activity.viewers > 0,
                ) {
                    return Ok(false);
                }
                a.lifecycle.close_reason = "idle".into();
                a.close_state = "closing".into();
                a.close_requested_at = Some(now());
                db.save(&a)?;
                log(
                    db,
                    &db.get::<Card>(&a.card_id)?,
                    "execution.idle_close_requested",
                    "system",
                    close_note,
                    Some(a.request_id.clone()),
                    json!({"attempt":a.number}),
                )?;
                Ok(true)
            })?;
            if !execute {
                continue;
            }
            let _ = self.capture_output(run);
            // Herdr guards `idle` at delivery and refuses blocked dialogs without force.
            // Profiles with an explicit exit command try graceful input first; profiles using
            // `ExitAction::Manual` (or `None`, e.g. `agy`, `gemini`, `opencode`) terminate
            // the verified-idle PTY cleanly via `terminal.kill`.
            let result = (|| -> Result<Value> {
                let actual = client.get(terminal)?;
                let state = actual["status"]["state"].as_str().unwrap_or("");
                ensure!(
                    actual["state"] == "live" && matches!(state, "idle" | "done"),
                    "Agent is no longer idle"
                );
                if let Some(ExitAction::Input { text }) = &exit_action {
                    let mut params = json!({"terminal":terminal,"text":text});
                    if state == "idle" {
                        params["if"] = json!("idle");
                    }
                    client.call("terminal.send", params)
                } else {
                    client.call("terminal.kill", json!({"terminal":terminal}))
                }
            })();
            if let Err(e) = result {
                self.store.lock().unwrap().write(|db| {
                    let mut a: Attempt = db.get(&run.id)?;
                    a.close_state = "error".into();
                    a.error = Some(format!("Idle exit was not confirmed: {e:#}. Inspect before retrying; no forced kill was sent."));
                    let card:Card=db.get(&a.card_id)?;
                    db.save(&canban_notifications::attention(&a.project_id,&a.card_id,a.error.as_deref().unwrap()))?;
                    log(db,&card,"execution.idle_close_failed","system",a.error.as_deref().unwrap(),Some(a.request_id.clone()),json!({"attempt":a.number}))?;
                    db.save(&a)
                })?;
            }
            self.emit("runtime", Some(run.card_id.clone()));
        }
        Ok(())
    }
}
