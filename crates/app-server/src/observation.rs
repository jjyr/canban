//! Event-driven runtime inventory. No periodic terminal or Git inventory scan.
use super::*;
use std::{collections::BTreeMap, sync::mpsc::RecvTimeoutError, time::Instant};

const MIN_AGENT_WORK_MS: u64 = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OpenAgentWork {
    pub core: String,
    pub since: u64,
    pub engine: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClosedAgentWork {
    pub core: String,
    pub address: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub engine: Option<String>,
}

#[derive(Default)]
pub(super) struct Inventory {
    pub core: String,
    pub records: BTreeMap<String, Value>,
    pub working: std::collections::HashMap<String, OpenAgentWork>,
    pub closed_work: Vec<ClosedAgentWork>,
    pub last_at: u64,
    pub generation: u64,
    pub error: Option<String>,
    seq: u64,
}
impl Inventory {
    pub(super) fn is_working(record: &Value) -> bool {
        record["status"]["state"] == "working" && record["state"] != "finished"
    }
    fn snapshot(&mut self, snapshot: &Value) {
        self.snapshot_at(snapshot, now());
    }
    pub(super) fn snapshot_at(&mut self, snapshot: &Value, ts: u64) {
        self.seq = snapshot["seq"].as_u64().unwrap_or(0);
        let mut next_records = BTreeMap::new();
        for value in snapshot["terminals"].as_array().into_iter().flatten() {
            if let Some(address) = value["address"].as_str() {
                let mut v = value.clone();
                v["address"] = json!(address);
                next_records.insert(address.to_owned(), v);
            }
        }
        let mut next_working = std::collections::HashMap::new();
        for (addr, mut open) in self.working.drain() {
            if open.core == self.core
                && let Some(rec) = next_records.get(&addr)
                && Self::is_working(rec)
            {
                if open.engine.is_none() {
                    open.engine = canban_profiles::identify_herdr_agent(rec).map(str::to_owned);
                }
                next_working.insert(addr, open);
            } else if ts.saturating_sub(open.since) >= MIN_AGENT_WORK_MS {
                self.closed_work.push(ClosedAgentWork {
                    core: open.core,
                    address: addr,
                    start_ms: open.since,
                    end_ms: ts,
                    engine: open.engine,
                });
            }
        }
        for (addr, val) in &next_records {
            if Self::is_working(val) && !next_working.contains_key(addr) {
                next_working.insert(
                    addr.clone(),
                    OpenAgentWork {
                        core: self.core.clone(),
                        since: ts,
                        engine: canban_profiles::identify_herdr_agent(val).map(str::to_owned),
                    },
                );
            }
        }
        self.records = next_records;
        self.working = next_working;
    }
    fn sync_working_key(&mut self, key: &str, ts: u64) {
        if let Some(rec) = self.records.get(key)
            && Self::is_working(rec)
        {
            let engine = canban_profiles::identify_herdr_agent(rec).map(str::to_owned);
            if let Some(existing) = self.working.get_mut(key) {
                if existing.engine.is_none() {
                    existing.engine = engine;
                }
            } else {
                self.working.insert(
                    key.to_owned(),
                    OpenAgentWork {
                        core: self.core.clone(),
                        since: ts,
                        engine,
                    },
                );
            }
        } else if let Some(open) = self.working.remove(key)
            && ts.saturating_sub(open.since) >= MIN_AGENT_WORK_MS
        {
            self.closed_work.push(ClosedAgentWork {
                core: open.core,
                address: key.to_owned(),
                start_ms: open.since,
                end_ms: ts,
                engine: open.engine,
            });
        }
    }
    pub(super) fn close_all_working(&mut self, ts: u64) {
        for (addr, open) in self.working.drain() {
            if ts.saturating_sub(open.since) >= MIN_AGENT_WORK_MS {
                self.closed_work.push(ClosedAgentWork {
                    core: open.core,
                    address: addr,
                    start_ms: open.since,
                    end_ms: ts,
                    engine: open.engine,
                });
            }
        }
    }
    fn apply(&mut self, message: &Value) -> bool {
        self.apply_at(message, now())
    }
    pub(super) fn apply_at(&mut self, message: &Value, ts: u64) -> bool {
        self.last_at = ts;
        self.error = None;
        if message["method"] == "heartbeat" {
            return false;
        }
        let p = &message["params"];
        if message["method"] == "missed" {
            self.snapshot_at(p, ts);
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
        let Some(key) = p["terminal"].as_str().map(str::to_owned) else {
            return false;
        };
        let data = &p["data"];
        let kind = p["type"].as_str().unwrap_or("");
        if kind == "removed" {
            self.records.remove(&key);
            self.sync_working_key(&key, ts);
            return true;
        }
        if matches!(kind, "created" | "restarted") || data["record"].is_object() {
            let mut value = if data["record"].is_object() {
                data["record"].clone()
            } else {
                data.clone()
            };
            value["address"] = json!(key);
            self.records.insert(key.clone(), value);
            self.sync_working_key(&key, ts);
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
        let changed = before != *value;
        self.sync_working_key(&key, ts);
        changed
    }
}
impl AppServer {
    pub(super) fn wake_runtime(&self) {
        if let Some(tx) = self.runtime_wake.lock().unwrap().as_ref() {
            let _ = tx.try_send(());
        }
    }
    pub(super) fn wake_delivery(&self) {
        if let Some(tx) = self.delivery_wake.lock().unwrap().as_ref() {
            let _ = tx.try_send(());
        }
    }
    /// Wakes the background delivery worker when the service runtime is active;
    /// runs synchronously for in-memory `AppServer::open` callers and unit tests.
    pub(crate) fn request_deliveries(&self) {
        if let Some(tx) = self.delivery_wake.lock().unwrap().clone() {
            let _ = tx.try_send(());
        } else {
            let _ = self.pump_deliveries();
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
            let mut records = list["terminals"]
                .as_array()
                .context("Invalid inventory")?
                .clone();
            self.apply_refinements(&core, &mut records);
            return Ok((core, records));
        }
        let (core, mut records) = {
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
            (
                state.core.clone(),
                state.records.values().cloned().collect::<Vec<_>>(),
            )
        };
        self.apply_refinements(&core, &mut records);
        Ok((core, records))
    }
    /// Returns the currently verified Herdr Core ID from the active event subscription
    /// without a round-trip when fresh, falling back to `Client::status()`.
    pub(super) fn verified_core_id(&self) -> Result<String> {
        if self.observer_running.load(Ordering::SeqCst) {
            let state = self.inventory.lock().unwrap();
            if state.error.is_none()
                && !state.core.is_empty()
                && state.last_at > 0
                && now().saturating_sub(state.last_at) < 45_000
            {
                return Ok(state.core.clone());
            }
        }
        let status = canban_herdr::Client {
            config: self.config(),
        }
        .status()?;
        Ok(status["core"]["id"]
            .as_str()
            .context("Core identity unavailable")?
            .to_owned())
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
                            if !inv.core.is_empty() && inv.core != subscription.core {
                                inv.close_all_working(now());
                            }
                            inv.core = subscription.core.clone();
                            inv.generation += 1;
                            inv.snapshot(&subscription.snapshot);
                            inv.last_at = now();
                            inv.error = None;
                            server.observed_at.store(inv.last_at, Ordering::SeqCst);
                        }
                        server.contexts.lock().unwrap().reset();
                        server.emit("terminals", None);
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
                                    let (changed, last_at) = {
                                        let mut inv = server.inventory.lock().unwrap();
                                        let changed = inv.apply(&message);
                                        (changed, inv.last_at)
                                    };
                                    server.observed_at.store(last_at, Ordering::SeqCst);
                                    if changed {
                                        server.emit("terminals", None);
                                    }
                                    if matches!(
                                        message["params"]["type"].as_str(),
                                        Some("finished" | "removed")
                                    ) || message["method"] == "missed"
                                    {
                                        server.wake_scripts();
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
                    {
                        let mut inv = server.inventory.lock().unwrap();
                        inv.error = Some(format!("{e:#}"));
                        inv.generation += 1;
                    }
                    server.emit("terminals", None);
                } else {
                    server.wake_runtime();
                }
                drop(server);
                // Interruptible reconnect backoff, not a runtime scan.
                let mut woke_early = false;
                for _ in 0..delay * 10 {
                    let Some(s) = weak.upgrade() else {
                        return;
                    };
                    if s.stopped() {
                        return;
                    }
                    let cfg = s.config();
                    let next_key =
                        serde_json::to_vec(&json!([cfg.hc, cfg.hc_session, cfg.hc_env])).unwrap();
                    if s.herdr_wake.swap(false, Ordering::SeqCst) || next_key != config_key {
                        woke_early = true;
                        break;
                    }
                    drop(s);
                    std::thread::sleep(Duration::from_millis(100));
                }
                delay = if woke_early { 1 } else { (delay * 2).min(30) };
            }
        });
    }
    pub(super) fn migrate_exit_failures(&self) -> Result<()> {
        const REASONS: [&str; 2] = [
            "Agent exited without an explicit outcome. Inspect the retained output before resuming.",
            "Agent exited without an explicit outcome",
        ];
        self.store.lock().unwrap().write(|db| {
            for mut t in db.list::<TerminalEntry>()? {
                let Some(a) = &mut t.agent else {
                    continue;
                };
                if !(a.state == "needs_report"
                    && a.observed
                        .as_ref()
                        .is_some_and(|v| v["state"] == "finished"))
                {
                    continue;
                }
                a.state = "failed".into();
                a.error = Some("Agent exited without reporting an outcome".into());
                let card_id = a.card_id.clone();
                let run_num = a.number;
                db.save(&t)?;
                let mut c: Card = db.get(&card_id)?;
                if c.human_gate.as_ref().is_some_and(|g| {
                    g.terminal_id.as_deref() == Some(&t.id) && REASONS.contains(&g.reason.as_str())
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
                    Some(t.id.clone()),
                    json!({"run_number":run_num,"migration":true}),
                )?;
            }
            Ok(())
        })
    }
    pub(super) fn runtime_missing(&self, entry: &TerminalEntry) -> Result<()> {
        let Some(run) = &entry.agent else {
            return Ok(());
        };
        if run.finished() {
            return Ok(());
        } // Retention cannot erase an already confirmed exit.
        self.observe_lifecycle(entry, &json!({"state":"missing"}))?;
        let card_id = run.card_id.clone();
        self.store.lock().unwrap().write(|db| {
            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
            let Some(a) = &mut current_entry.agent else {
                return Ok(());
            };
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
            let err_msg = a.error.clone().unwrap();
            let run_num = a.number;
            log(
                db,
                &db.get::<Card>(&card_id)?,
                "execution.missing",
                "herdr",
                &err_msg,
                Some(entry.id.clone()),
                json!({"run_number":run_num}),
            )?;
            db.save(&current_entry)
        })?;
        self.request_deliveries();
        self.request_scripts();
        let _ = self.try_auto_archive_resolved(&card_id);
        self.emit("runtime", Some(card_id));
        Ok(())
    }
    fn start_delivery_worker(self: &Arc<Self>, drx: mpsc::Receiver<()>) {
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if server.stopped() {
                    break;
                }
                let mut wait: Option<Duration> = None;
                if canban_updater::handoff::active(&server.home) {
                    wait = Some(Duration::from_millis(250));
                } else {
                    if let Err(e) = server.pump_deliveries() {
                        eprintln!("Canban delivery: {e:#}");
                    }
                    let _ = server.pump_admissions();
                    let s = server
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .admission_snapshot()
                        .unwrap_or_default();
                    if s.terminals.iter().any(|t| {
                        t.agent
                            .as_ref()
                            .is_some_and(|a| a.state == "queued" || a.pending.is_some())
                    }) || s.merge_batches.iter().any(|b| b.state == "queued")
                    {
                        server.wake_runtime();
                    }
                    let mut due = |at: u64| {
                        let duration = Duration::from_millis(at.saturating_sub(now()).max(1));
                        wait = Some(wait.map_or(duration, |w| w.min(duration)));
                    };
                    if s.workflow_runs
                        .iter()
                        .any(|r| r.state == "waiting_delivery" && r.queue_pause_reason().is_some())
                    {
                        due(now().saturating_add(2_000));
                    }
                    for t in &s.terminals {
                        let Some(a) = &t.agent else {
                            continue;
                        };
                        if a.terminal.is_some()
                            && a.connection_state == "connected"
                            && !a.agent_stopped()
                            && let Some(run) = s.workflow_runs.iter().find(|r| {
                                r.card_id.as_deref() == Some(a.card_id.as_str())
                                    && r.state == "waiting_delivery"
                            })
                        {
                            let anchor = a
                                .completed_at
                                .max(Some(run.updated_at))
                                .unwrap_or(t.created_at);
                            due(anchor.saturating_add(Self::DELIVERY_AGENT_SETTLE_MS + 1));
                        }
                    }
                }
                drop(server);
                match wait {
                    Some(d) => {
                        let _ = drx.recv_timeout(d);
                    }
                    None => {
                        if drx.recv().is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    fn start_script_worker(self: &Arc<Self>, srx: mpsc::Receiver<()>) {
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if server.stopped() {
                    break;
                }
                let mut wait: Option<Duration> = None;
                if canban_updater::handoff::active(&server.home) {
                    wait = Some(Duration::from_millis(250));
                } else {
                    match server.pump_scripts() {
                        Ok(true) => {
                            wait = Some(Duration::from_millis(200));
                        }
                        Ok(false) => {}
                        Err(e) => {
                            eprintln!("Canban scripts: {e:#}");
                            wait = Some(Duration::from_millis(500));
                        }
                    }
                }
                drop(server);
                match wait {
                    Some(d) => {
                        let _ = srx.recv_timeout(d);
                    }
                    None => {
                        if srx.recv().is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    pub(super) fn start_runtime(self: &Arc<Self>) {
        let (tx, rx) = mpsc::sync_channel(1);
        canban_herdr::activity::subscribe(tx.clone());
        *self.runtime_wake.lock().unwrap() = Some(tx);
        let (dtx, drx) = mpsc::sync_channel(1);
        *self.delivery_wake.lock().unwrap() = Some(dtx);
        let (stx, srx) = mpsc::sync_channel(1);
        *self.script_wake.lock().unwrap() = Some(stx);
        self.start_observer();
        self.start_context_workers();
        self.start_delivery_worker(drx);
        self.start_script_worker(srx);
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
                if let Err(e) = server.tick() {
                    eprintln!("Canban runtime: {e:#}");
                }
                let mut s = server
                    .store
                    .lock()
                    .unwrap()
                    .db()
                    .runtime_snapshot()
                    .unwrap_or_default();
                server.project_observation_freshness(&mut s);
                let mut wait: Option<Duration> = None;
                let mut due = |at: u64| {
                    let duration = Duration::from_millis(at.saturating_sub(now()).max(1));
                    wait = Some(wait.map_or(duration, |w| w.min(duration)));
                };
                if let Some(at) = server.event_deadline() {
                    due(at);
                }
                if let Some(at) = server.refine_deadline() {
                    due(at);
                }
                if let Some(at) = server.schedule_deadline(&s) {
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
                for t in &s.terminals {
                    let Some(a) = &t.agent else {
                        continue;
                    };
                    if a.close_state == "closing"
                        && let Some(at) = a.close_requested_at
                    {
                        due(at.saturating_add(30_001));
                    }
                    if a.terminal.is_some()
                        && a.connection_state == "connected"
                        && !a.agent_stopped()
                        && let Some(run) = s.workflow_runs.iter().find(|r| {
                            r.card_id.as_deref() == Some(a.card_id.as_str())
                                && r.state == "waiting_delivery"
                        })
                    {
                        let anchor = a
                            .completed_at
                            .max(Some(run.updated_at))
                            .unwrap_or(t.created_at);
                        due(anchor.saturating_add(Self::DELIVERY_AGENT_SETTLE_MS + 1));
                    }
                    if let Some(anchor) = canban_notifications::unreported_idle_anchor(&s, &t.id, a)
                    {
                        let activity = canban_herdr::activity::get(
                            a.core_id.as_deref().unwrap_or(""),
                            a.terminal.as_deref().unwrap_or(""),
                        );
                        let deadline = anchor
                            .max(activity.last_at)
                            .saturating_add(canban_notifications::UNREPORTED_IDLE_ALERT_MS);
                        if a.lifecycle.observed_at.unwrap_or(0) < deadline {
                            due(deadline.saturating_add(1));
                        }
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
                        if let Some(deadline) = lifecycle::idle_reclaim_deadline(
                            a,
                            server.config().scheduler.idle_minutes,
                            activity.last_at,
                        ) {
                            due(deadline);
                        }
                    }
                }
                // Only queued work awaiting a scheduler slot needs retry admission.
                let retry_needed = canban_scheduler::next(&s, server.config().scheduler.max_agents)
                    .is_some()
                    || canban_scheduler::timers::next_scheduled_run(
                        &s,
                        server.config().scheduler.max_agents,
                    )
                    .is_some()
                    || (canban_scheduler::active_count(&s) < server.config().scheduler.max_agents
                        && (s.merge_batches.iter().any(|b| b.state == "queued")
                            || s.configuration_sessions.iter().any(|c| c.state == "queued")));
                if retry_needed {
                    let mut current = s
                        .terminals
                        .iter()
                        .filter(|t| {
                            t.agent
                                .as_ref()
                                .is_some_and(|a| a.state == "queued" || a.pending.is_some())
                        })
                        .map(|t| t.id.clone())
                        .collect::<Vec<_>>();
                    current.extend(
                        s.merge_batches
                            .iter()
                            .filter(|b| b.state == "queued")
                            .map(|b| b.id.clone()),
                    );
                    current.extend(
                        s.scheduled_runs
                            .iter()
                            .filter(|r| !r.creates_card && r.state == "queued")
                            .map(|r| r.id.clone()),
                    );
                    current.extend(
                        s.configuration_sessions
                            .iter()
                            .filter(|c| c.state == "queued")
                            .map(|c| format!("{}:{}", c.id, c.generation)),
                    );
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
            ..Default::default()
        };
        inv.snapshot(&json!({"seq":10,"terminals":[{"address":"t1","state":"live","status":{"state":"blocked"}}]}));
        assert!(inv.records.contains_key("t1"));
        assert!(!inv.apply(&json!({"method":"event","params":{"seq":10,"type":"removed","terminal":"t1","data":{}}})));
        inv.apply(&json!({"method":"event","params":{"seq":14,"type":"finished","terminal":"t1","data":{"exit_code":1,"reason":"exited"}}}));
        assert_eq!(inv.records["t1"]["state"], "finished");
        assert_eq!(inv.records["t1"]["exit_code"], 1);
        inv.apply(&json!({"method":"missed","params":{"seq":20,"terminals":[{"address":"t9","state":"live"}]}}));
        assert!(!inv.records.contains_key("t1"));
        assert_eq!(inv.records["t9"]["state"], "live");
        assert!(!inv.apply(&json!({"method":"heartbeat","params":{"seq":20}})));
    }

    #[test]
    fn working_transitions_record_closed_segments_and_drop_short_bursts() {
        let mut inv = Inventory {
            core: "core-a".into(),
            ..Default::default()
        };
        inv.snapshot_at(
            &json!({"seq":1,"terminals":[{"address":"t1","state":"live","status":{"state":"idle"}}]}),
            10_000,
        );
        assert!(inv.working.is_empty());

        // Short working burst (< 2s) is dropped when stopped.
        inv.apply_at(
            &json!({"method":"event","params":{"seq":2,"type":"status","terminal":"t1","data":{"status":{"state":"working","app":"claude"}}}}),
            11_000,
        );
        assert_eq!(inv.working["t1"].since, 11_000);
        assert_eq!(inv.working["t1"].engine.as_deref(), Some("claude"));
        inv.apply_at(
            &json!({"method":"event","params":{"seq":3,"type":"status","terminal":"t1","data":{"status":{"state":"idle"}}}}),
            12_500,
        );
        assert!(inv.working.is_empty());
        assert!(inv.closed_work.is_empty());

        // Working >= 2s through finished closes and records a segment.
        inv.apply_at(
            &json!({"method":"event","params":{"seq":4,"type":"status","terminal":"t1","data":{"status":{"state":"working","app":"codex"}}}}),
            20_000,
        );
        inv.apply_at(
            &json!({"method":"event","params":{"seq":5,"type":"finished","terminal":"t1","data":{"exit_code":0}}}),
            25_000,
        );
        assert!(inv.working.is_empty());
        assert_eq!(inv.closed_work.len(), 1);
        assert_eq!(inv.closed_work[0].start_ms, 20_000);
        assert_eq!(inv.closed_work[0].end_ms, 25_000);
        assert_eq!(inv.closed_work[0].engine.as_deref(), Some("codex"));

        // Missed snapshot preserves still-working terminals and closes removed ones.
        inv.apply_at(
            &json!({"method":"event","params":{"seq":6,"type":"created","terminal":"t2","data":{"state":"live","status":{"state":"working"},"foreground":{"name":"claude","argv":["claude"]}}}}),
            30_000,
        );
        inv.apply_at(
            &json!({"method":"event","params":{"seq":7,"type":"created","terminal":"t3","data":{"state":"live","status":{"state":"working"}}}}),
            30_500,
        );
        inv.apply_at(
            &json!({"method":"missed","params":{"seq":10,"terminals":[{"address":"t2","state":"live","status":{"state":"working"}}]}}),
            35_000,
        );
        assert_eq!(inv.working["t2"].since, 30_000);
        assert!(!inv.working.contains_key("t3"));
        assert_eq!(inv.closed_work.len(), 2);
        assert_eq!(inv.closed_work[1].address, "t3");
        assert_eq!(inv.closed_work[1].end_ms, 35_000);

        // Removing a working terminal closes it cleanly.
        inv.apply_at(
            &json!({"method":"event","params":{"seq":11,"type":"removed","terminal":"t2","data":{}}}),
            40_000,
        );
        assert!(inv.working.is_empty());
        assert_eq!(inv.closed_work.len(), 3);
        assert_eq!(inv.closed_work[2].address, "t2");
        assert_eq!(inv.closed_work[2].start_ms, 30_000);
        assert_eq!(inv.closed_work[2].end_ms, 40_000);
    }
}
