//! Event-driven runtime inventory. No periodic terminal or Git inventory scan.
use super::*;
use std::{collections::BTreeMap, sync::mpsc::RecvTimeoutError, time::Instant};

#[derive(Default)]
pub(super) struct Inventory {
    pub core: String,
    pub machines: Value,
    pub records: BTreeMap<String, Value>,
    pub last_at: u64,
    pub generation: u64,
    pub error: Option<String>,
    pub remote_error: Option<String>,
    seq: u64,
}
impl Inventory {
    pub(super) fn address(&self, address: &str) -> String {
        let Some((name, local)) = address.split_once('/') else {
            return address.into();
        };
        let owner = self.machines["machines"]
            .as_array()
            .and_then(|ms| ms.iter().find(|m| m["name"] == name || m["id"] == name))
            .and_then(|m| m["id"].as_str())
            .unwrap_or(name);
        format!("{owner}/{local}")
    }
    fn snapshot(&mut self, snapshot: &Value) {
        self.seq = snapshot["seq"].as_u64().unwrap_or(0);
        self.records.clear();
        for value in snapshot["terminals"].as_array().into_iter().flatten() {
            if let Some(address) = value["address"].as_str() {
                let address = self.address(address);
                let mut v = value.clone();
                v["address"] = json!(address);
                self.records.insert(address, v);
            }
        }
    }
    fn apply(&mut self, message: &Value) -> bool {
        self.last_at = now();
        self.error = None;
        if message["method"] == "heartbeat" {
            return false;
        }
        let p = &message["params"];
        if message["method"] == "missed" {
            self.snapshot(p);
            self.generation += 1;
            return true;
        }
        if message["method"] != "event" {
            return false;
        }
        let seq = p["seq"].as_u64().unwrap_or(0);
        if seq <= self.seq {
            return false;
        }
        self.seq = seq; // Filters can legitimately skip sequence numbers.
        let Some(raw) = p["terminal"].as_str() else {
            return false;
        };
        let key = self.address(raw);
        let data = &p["data"];
        let kind = p["type"].as_str().unwrap_or("");
        if kind == "removed" {
            self.records.remove(&key);
            return true;
        }
        if matches!(kind, "created" | "restarted") || data["record"].is_object() {
            let mut value = if data["record"].is_object() {
                data["record"].clone()
            } else {
                data.clone()
            };
            value["address"] = json!(key);
            self.records.insert(key, value);
            return true;
        }
        let Some(value) = self.records.get_mut(&key) else {
            return false;
        };
        let before = value.clone();
        match kind {
            "foreground" => value["foreground"] = data.clone(),
            "finished" => {
                value["state"] = json!("finished");
                value.as_object_mut().unwrap().remove("foreground");
                for k in ["reason", "exit_code", "signal", "finished_ms"] {
                    value[k] = data[k].clone();
                }
            }
            "lost" => {
                value["state"] = json!("live");
                value["pid"] = Value::Null;
                value.as_object_mut().unwrap().remove("foreground");
                for k in ["reason", "exit_code", "signal", "finished_ms"] {
                    value[k] = data[k].clone();
                }
            }
            "status" => {
                for k in ["status", "records", "reported"] {
                    value[k] = data[k].clone();
                }
            }
            "turn" => {
                value["turn"] = data["turn"].clone();
            }
            "labels" => value["labels"] = data["labels"].clone(),
            "title" => value["title"] = data["title"].clone(),
            _ => return false,
        }
        before != *value
    }
}
impl AppServer {
    pub(super) fn wake_runtime(&self) {
        if let Some(tx) = self.runtime_wake.lock().unwrap().as_ref() {
            let _ = tx.try_send(());
        }
    }
    pub(super) fn runtime_inventory(&self) -> Result<(String, Vec<Value>)> {
        #[cfg(test)]
        if !self.observer_running.load(Ordering::SeqCst) {
            // Explicit synchronous test driver, never used by the running service.
            let client = canban_herdr::Client {
                config: self.config(),
            };
            let core = client.status()?["core"]["id"]
                .as_str()
                .context("Core identity unavailable")?
                .to_owned();
            let list = client.terminals()?;
            ensure!(
                client.status()?["core"]["id"] == core,
                "Core changed during observation"
            );
            return Ok((
                core,
                list["terminals"]
                    .as_array()
                    .context("Invalid inventory")?
                    .clone(),
            ));
        }
        let state = self.inventory.lock().unwrap();
        ensure!(
            state.error.is_none()
                && state.last_at > 0
                && now().saturating_sub(state.last_at) < 45_000,
            "{}",
            state
                .error
                .as_deref()
                .unwrap_or("Herdr subscription is connecting")
        );
        Ok((
            state.core.clone(),
            state.records.values().cloned().collect(),
        ))
    }
    pub(super) fn start_observer(self: &Arc<Self>) {
        self.observer_running.store(true, Ordering::SeqCst);
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut delay = 1;
            loop {
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if server.stopped() {
                    break;
                }
                let config = server.config();
                let config_key =
                    serde_json::to_vec(&json!([config.hc, config.hc_session, config.hc_env]))
                        .unwrap();
                let subscription = canban_herdr::Client { config }.subscribe();
                let result: Result<()> = match subscription {
                    Err(e) => Err(e),
                    Ok(subscription) => {
                        {
                            let mut inv = server.inventory.lock().unwrap();
                            inv.core = subscription.core.clone();
                            inv.machines = subscription.machines.clone();
                            inv.generation += 1;
                            inv.snapshot(&subscription.snapshot);
                            inv.last_at = now();
                            inv.error = None;
                        }
                        server.contexts.lock().unwrap().reset();
                        server.wake_runtime();
                        delay = 1;
                        drop(server);
                        loop {
                            let Some(server) = weak.upgrade() else {
                                return;
                            };
                            if server.stopped() {
                                return;
                            }
                            let config = server.config();
                            if serde_json::to_vec(&json!([
                                config.hc,
                                config.hc_session,
                                config.hc_env
                            ]))
                            .unwrap()
                                != config_key
                            {
                                break Err(anyhow::anyhow!("Herdr configuration changed"));
                            }
                            match subscription.recv_timeout(Duration::from_secs(1)) {
                                Ok(Ok(message)) => {
                                    let changed = server.inventory.lock().unwrap().apply(&message);
                                    let remote = message["params"]["terminal"]
                                        .as_str()
                                        .is_some_and(|t| t.contains('/'));
                                    if remote || message["method"] == "heartbeat" {
                                        let s = server.snapshot().unwrap_or_default();
                                        if s.attempts.iter().any(|a| {
                                            !a.finished()
                                                && a.terminal
                                                    .as_deref()
                                                    .is_some_and(|t| t.contains('/'))
                                        }) || s.terminals.iter().any(|t| {
                                            t.runtime.as_ref().is_some_and(|r| {
                                                r.state != "finished"
                                                    && r.terminal
                                                        .as_deref()
                                                        .is_some_and(|t| t.contains('/'))
                                            })
                                        }) {
                                            server.remote_health_due.store(true, Ordering::SeqCst);
                                            server.wake_runtime();
                                        }
                                    }
                                    if changed {
                                        server.wake_runtime();
                                    }
                                }
                                Ok(Err(e)) => break Err(e),
                                Err(RecvTimeoutError::Disconnected) => {
                                    break Err(anyhow::anyhow!(
                                        "Herdr event stream closed or overflowed"
                                    ));
                                }
                                Err(RecvTimeoutError::Timeout) => {
                                    if now()
                                        .saturating_sub(server.inventory.lock().unwrap().last_at)
                                        > 45_000
                                    {
                                        break Err(anyhow::anyhow!(
                                            "Herdr event heartbeat timed out"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                };
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if let Err(e) = result {
                    let mut inv = server.inventory.lock().unwrap();
                    inv.error = Some(format!("{e:#}"));
                    inv.generation += 1;
                }
                server.wake_runtime();
                drop(server);
                // Interruptible reconnect backoff, not a runtime scan.
                for _ in 0..delay * 10 {
                    if weak.upgrade().is_none_or(|s| s.stopped()) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                delay = (delay * 2).min(30);
            }
        });
    }
    pub(super) fn migrate_exit_failures(&self) -> Result<()> {
        const REASONS: [&str; 2] = [
            "Agent exited without an explicit outcome. Inspect the retained output before resuming.",
            "Agent exited without an explicit outcome",
        ];
        self.store.lock().unwrap().write(|db| {
            for mut a in db.list::<Attempt>()?.into_iter().filter(|a| {
                a.state == "needs_report"
                    && a.observed
                        .as_ref()
                        .is_some_and(|v| v["state"] == "finished")
            }) {
                let mut r: Request = db.get(&a.request_id)?;
                if matches!(r.state.as_str(), "completed" | "backlog") {
                    continue;
                }
                a.state = "failed".into();
                a.error = Some("Agent exited without reporting an outcome".into());
                db.save(&a)?;
                if r.attempt == a.number && r.state == "needs_report" {
                    r.state = "failed".into();
                    db.save(&r)?;
                }
                let mut c: Card = db.get(&a.card_id)?;
                if c.human_gate.as_ref().is_some_and(|g| {
                    g.request_id.as_deref() == Some(&r.id) && REASONS.contains(&g.reason.as_str())
                }) {
                    c.human_gate = None;
                    if c.attention
                        .as_ref()
                        .is_some_and(|s| REASONS.contains(&s.as_str()))
                    {
                        c.attention = None;
                    }
                    c.revision += 1;
                    db.save(&c)?;
                }
                for mut n in db.list::<Notice>()?.into_iter().filter(|n| {
                    n.card_id == c.id && REASONS.contains(&n.message.as_str()) && !n.read
                }) {
                    n.read = true;
                    db.save(&n)?;
                }
                log(
                    db,
                    &c,
                    "execution.failed",
                    "system",
                    "Migrated the automatic missing-report gate to a failed execution",
                    Some(r.id),
                    json!({"attempt":a.number,"migration":true}),
                )?;
            }
            Ok(())
        })
    }
    pub(super) fn runtime_missing(&self, run: &Attempt) -> Result<()> {
        if run.finished() {
            return Ok(());
        } // Retention cannot erase an already confirmed exit.
        if run.terminal.as_deref().is_some_and(|t| t.contains('/')) {
            return self.connection_lost(
                run,
                "unavailable",
                "Remote terminal is not in the current mirror; its exit is unconfirmed",
            );
        }
        self.observe_lifecycle(run, &json!({"state":"missing"}))?;
        self.store.lock().unwrap().write(|db| {
            let mut a: Attempt = db.get(&run.id)?;
            if a.state == "missing" {
                return Ok(());
            }
            if !matches!(a.state.as_str(), "completed" | "backlog") {
                a.state = "missing".into();
            }
            a.connection_state = "missing".into();
            a.error = Some(
                "Original Core no longer retains this Terminal; no exit code is available".into(),
            );
            a.close_state = "closed".into();
            a.observed = Some(json!({"state":"missing"}));
            let mut r: Request = db.get(&a.request_id)?;
            if r.attempt == a.number
                && !matches!(r.state.as_str(), "completed" | "backlog" | "blocked")
            {
                r.state = "failed".into();
                db.save(&r)?;
            }
            log(
                db,
                &db.get::<Card>(&a.card_id)?,
                "execution.missing",
                "herdr",
                a.error.as_deref().unwrap(),
                Some(a.request_id.clone()),
                json!({"attempt":a.number}),
            )?;
            db.save(&a)
        })?;
        let _ = self.pump_deliveries();
        let _ = self.try_auto_archive_merged(&run.card_id);
        self.emit("runtime", Some(run.card_id.clone()));
        Ok(())
    }
    pub(super) fn start_runtime(self: &Arc<Self>) {
        let (tx, rx) = mpsc::sync_channel(1);
        canban_herdr::activity::subscribe(tx.clone());
        *self.runtime_wake.lock().unwrap() = Some(tx);
        self.start_observer();
        self.start_context_workers();
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut retry = Instant::now() + Duration::from_secs(1);
            let mut backoff = 1;
            let mut queued = Vec::<String>::new();
            loop {
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if server.stopped() {
                    break;
                }
                server.start_remote_workers();
                if let Err(e) = server.tick() {
                    eprintln!("Canban runtime: {e:#}");
                }
                let s = server.snapshot().unwrap_or_default();
                let mut wait: Option<Duration> = None;
                let mut due = |at: u64| {
                    let duration = Duration::from_millis(at.saturating_sub(now()).max(1));
                    wait = Some(wait.map_or(duration, |w| w.min(duration)));
                };
                if let Some(at) = server.event_deadline() {
                    due(at);
                }
                if s.workflow_runs
                    .iter()
                    .any(|r| r.state == "waiting_delivery" && r.queue_pause_reason().is_some())
                {
                    // Re-check transient main-checkout blockers (dirty tree or switched branch)
                    // so the FIFO merge queue automatically drains once the checkout is clean.
                    due(now().saturating_add(2_000));
                }
                for a in &s.attempts {
                    if a.close_state == "closing"
                        && let Some(at) = a.close_requested_at
                    {
                        due(at.saturating_add(30_001));
                    }
                    if a.terminal.is_some()
                        && a.connection_state == "connected"
                        && !a.agent_stopped()
                        && let Some(run) = s
                            .workflow_runs
                            .iter()
                            .find(|r| r.card_id == a.card_id && r.state == "waiting_delivery")
                    {
                        let anchor = a
                            .completed_at
                            .max(Some(run.updated_at))
                            .unwrap_or(a.created_at);
                        due(anchor.saturating_add(Self::DELIVERY_AGENT_SETTLE_MS + 1));
                    }
                    if lifecycle::idle_candidate(
                        &s,
                        a,
                        server.config().scheduler.idle_minutes,
                        u64::MAX,
                        false,
                    ) {
                        let activity = canban_herdr::activity::get(
                            a.core_id.as_deref().unwrap_or(""),
                            a.terminal.as_deref().unwrap_or(""),
                        );
                        due(a
                            .lifecycle
                            .idle_since
                            .unwrap_or(now())
                            .max(a.lifecycle.last_interaction_at.unwrap_or(0))
                            .max(activity.last_at)
                            .saturating_add(
                                server
                                    .config()
                                    .scheduler
                                    .idle_minutes
                                    .saturating_mul(60_000),
                            ));
                    }
                }
                // Only queued work awaiting an unavailable machine needs retry admission.
                let retry_needed =
                    canban_scheduler::next(&s, server.config().scheduler.max_agents).is_some();
                if retry_needed {
                    let current = s
                        .requests
                        .iter()
                        .filter(|r| r.state == "queued")
                        .map(|r| r.id.clone())
                        .collect::<Vec<_>>();
                    if current != queued {
                        queued = current;
                        backoff = 1;
                        retry = Instant::now() + Duration::from_secs(1);
                    }
                    if Instant::now() >= retry {
                        backoff = (backoff * 2).min(30);
                        retry = Instant::now() + Duration::from_secs(backoff);
                    }
                    let d = retry.saturating_duration_since(Instant::now());
                    wait = Some(wait.map_or(d, |w| w.min(d)));
                }
                drop(server);
                match wait {
                    Some(d) => {
                        let _ = rx.recv_timeout(d);
                    }
                    None => {
                        if rx.recv().is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_events_replace_snapshots_and_ignore_duplicates_without_inventing_exits() {
        let mut inv = Inventory {
            core: "core-a".into(),
            machines: json!({"machines":[{"id":"owner","name":"box"}]}),
            ..Default::default()
        };
        inv.snapshot(&json!({"seq":10,"terminals":[{"address":"box/t1","state":"live","status":{"state":"blocked"}}]}));
        assert!(inv.records.contains_key("owner/t1"));
        assert!(!inv.apply(&json!({"method":"event","params":{"seq":10,"type":"removed","terminal":"box/t1","data":{}}})));
        inv.apply(&json!({"method":"event","params":{"seq":14,"type":"finished","terminal":"box/t1","data":{"exit_code":1,"reason":"exited"}}}));
        assert_eq!(inv.records["owner/t1"]["state"], "finished");
        assert_eq!(inv.records["owner/t1"]["exit_code"], 1);
        inv.apply(&json!({"method":"missed","params":{"seq":20,"terminals":[{"address":"t9","state":"live"}]}}));
        assert!(!inv.records.contains_key("owner/t1"));
        assert_eq!(inv.records["t9"]["state"], "live");
        assert!(!inv.apply(&json!({"method":"heartbeat","params":{"seq":20}})));
    }
}
