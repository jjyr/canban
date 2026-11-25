//! Deterministic in-memory Herdr Core simulator implementing [`canban_herdr::HerdrBackend`].
use anyhow::{Result, bail};
use canban_herdr::{BackendSubscription, HerdrBackend, activity::Activity};
use canban_types::Clock;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, mpsc},
};

#[derive(Clone, Debug)]
pub struct RecordedSend {
    pub terminal: String,
    pub text: String,
    pub if_guard: Option<String>,
    pub at_ms: u64,
}

struct SubEntry {
    tx: mpsc::Sender<Result<Value>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

struct State {
    core_id: String,
    seq: u64,
    next_term_num: u64,
    terminals: BTreeMap<String, Value>,
    activities: BTreeMap<(String, String), Activity>,
    subscribers: Vec<SubEntry>,
    sent_inputs: Vec<RecordedSend>,
    killed_terminals: Vec<String>,
    /// Optional scripted next error for `terminal.send` (e.g. `status_mismatch` or `blocked`).
    next_send_errors: VecDeque<String>,
    /// When true, `subscribe` / `call` fail to simulate Herdr Core outage.
    offline: bool,
}

/// Per-host deterministic Herdr Core simulator with chaos fault injection.
pub struct SimHerdr {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl SimHerdr {
    pub fn new(core_id: impl Into<String>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            state: Mutex::new(State {
                core_id: core_id.into(),
                seq: 1,
                next_term_num: 1,
                terminals: BTreeMap::new(),
                activities: BTreeMap::new(),
                subscribers: Vec::new(),
                sent_inputs: Vec::new(),
                killed_terminals: Vec::new(),
                next_send_errors: VecDeque::new(),
                offline: false,
            }),
        })
    }

    pub fn core_id(&self) -> String {
        self.state.lock().unwrap().core_id.clone()
    }

    pub fn set_offline(&self, offline: bool) {
        let mut st = self.state.lock().unwrap();
        st.offline = offline;
        if offline {
            let subs = std::mem::take(&mut st.subscribers);
            for sub in subs {
                let _ = sub.tx.send(Err(anyhow::anyhow!("Herdr Core went offline")));
                (sub.wake)();
            }
        }
    }

    /// Simulates a Herdr Core restart with a new `core_id`, closing active subscriptions.
    pub fn restart_core(&self, new_core_id: impl Into<String>) {
        let mut st = self.state.lock().unwrap();
        st.core_id = new_core_id.into();
        st.seq += 1;
        let subs = std::mem::take(&mut st.subscribers);
        for sub in subs {
            let _ = sub.tx.send(Err(anyhow::anyhow!(
                "Herdr Core restarted with new identity"
            )));
            (sub.wake)();
        }
    }

    /// Inserts or updates a simulated terminal and broadcasts a `created` event.
    pub fn upsert_terminal(
        &self,
        address: impl Into<String>,
        cwd: impl Into<String>,
        status_state: &str,
        turn: u64,
        agent_app: Option<&str>,
    ) {
        let address = address.into();
        let cwd = cwd.into();
        let mut status = json!({ "state": status_state });
        if let Some(app) = agent_app {
            status["app"] = json!(app);
        }
        let mut record = json!({
            "address": address,
            "state": "live",
            "cwd": cwd,
            "pid": 1000,
            "turn": turn,
            "status": status,
            "labels": {},
        });
        if let Some(app) = agent_app {
            record["foreground"] = json!({
                "pid": 1001,
                "name": app,
                "argv": [app],
                "cwd": cwd,
            });
        }
        let mut st = self.state.lock().unwrap();
        st.seq += 1;
        let seq = st.seq;
        st.terminals.insert(address.clone(), record.clone());
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "created",
                    "terminal": address,
                    "data": { "record": record },
                }
            }),
        );
    }

    /// Transitions a terminal's Herdr status (`working`, `idle`, `done`, `blocked`) and optional turn.
    pub fn set_status(&self, address: &str, status_state: &str, turn: Option<u64>) {
        self.set_status_detailed(address, json!({ "state": status_state }), turn);
    }

    pub fn set_status_detailed(&self, address: &str, status_obj: Value, turn: Option<u64>) {
        let mut st = self.state.lock().unwrap();
        let Some(rec) = st.terminals.get_mut(address) else {
            return;
        };
        rec["status"] = status_obj.clone();
        if let Some(t) = turn {
            rec["turn"] = json!(t);
        }
        let turn_val = rec["turn"].clone();
        st.seq += 1;
        let seq = st.seq;
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "status",
                    "terminal": address,
                    "data": { "status": status_obj },
                }
            }),
        );
        if turn.is_some() {
            st.seq += 1;
            let turn_seq = st.seq;
            Self::broadcast_locked(
                &mut st,
                json!({
                    "method": "event",
                    "params": {
                        "seq": turn_seq,
                        "type": "turn",
                        "terminal": address,
                        "data": { "turn": turn_val },
                    }
                }),
            );
        }
    }

    /// Injects an out-of-order / duplicate `seq` event (must be ignored by `Inventory::apply_at`).
    pub fn inject_stale_event(&self, address: &str, stale_seq: u64, bogus_status: &str) {
        let mut st = self.state.lock().unwrap();
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": stale_seq,
                    "type": "status",
                    "terminal": address,
                    "data": { "status": { "state": bogus_status } },
                }
            }),
        );
    }

    /// Marks a terminal as finished with an exit code and broadcasts `finished`.
    pub fn finish_terminal(&self, address: &str, exit_code: i32, reason: &str) {
        let mut st = self.state.lock().unwrap();
        let finished_ms = self.clock.now_ms();
        let Some(rec) = st.terminals.get_mut(address) else {
            return;
        };
        rec["state"] = json!("finished");
        rec["exit_code"] = json!(exit_code);
        rec["reason"] = json!(reason);
        rec["finished_ms"] = json!(finished_ms);
        rec.as_object_mut().unwrap().remove("foreground");
        st.seq += 1;
        let seq = st.seq;
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "finished",
                    "terminal": address,
                    "data": {
                        "exit_code": exit_code,
                        "reason": reason,
                        "finished_ms": finished_ms,
                    },
                }
            }),
        );
    }

    /// Marks a terminal as lost (pid null, foreground cleared) and broadcasts `lost`.
    pub fn lose_terminal(&self, address: &str, reason: &str) {
        let mut st = self.state.lock().unwrap();
        let finished_ms = self.clock.now_ms();
        let Some(rec) = st.terminals.get_mut(address) else {
            return;
        };
        rec["state"] = json!("live");
        rec["pid"] = Value::Null;
        rec["reason"] = json!(reason);
        rec["finished_ms"] = json!(finished_ms);
        rec.as_object_mut().unwrap().remove("foreground");
        st.seq += 1;
        let seq = st.seq;
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "lost",
                    "terminal": address,
                    "data": {
                        "reason": reason,
                        "finished_ms": finished_ms,
                    },
                }
            }),
        );
    }

    /// Sends a `missed` snapshot resync frame to all subscribers.
    pub fn emit_missed_snapshot(&self) {
        let mut st = self.state.lock().unwrap();
        st.seq += 5;
        let seq = st.seq;
        let terminals: Vec<Value> = st.terminals.values().cloned().collect();
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "missed",
                "params": {
                    "seq": seq,
                    "terminals": terminals,
                }
            }),
        );
    }

    /// Sends a Herdr control-plane `heartbeat` frame.
    pub fn emit_heartbeat(&self) {
        let mut st = self.state.lock().unwrap();
        let seq = st.seq;
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "heartbeat",
                "params": { "seq": seq }
            }),
        );
    }

    /// Forces all current event subscribers to disconnect (testing reconnect recovery).
    pub fn drop_subscribers(&self) {
        let mut st = self.state.lock().unwrap();
        let subs = std::mem::take(&mut st.subscribers);
        for sub in subs {
            let wake = sub.wake;
            drop(sub.tx);
            wake();
        }
    }

    /// Queues a synthetic error response on the next `terminal.send` call (e.g.
    /// `{"kind":"status_mismatch"}` or `{"kind":"blocked"}`).
    pub fn inject_next_send_error(&self, err_msg: impl Into<String>) {
        self.state
            .lock()
            .unwrap()
            .next_send_errors
            .push_back(err_msg.into());
    }

    /// Updates simulated viewer presence / human terminal activity for `(core_id, address)`.
    pub fn set_viewer_activity(&self, address: &str, viewers: usize, touch_now: bool) {
        let mut st = self.state.lock().unwrap();
        let core = st.core_id.clone();
        let now_ms = self.clock.now_ms();
        let entry = st.activities.entry((core, address.to_owned())).or_default();
        entry.viewers = viewers;
        if touch_now {
            entry.last_at = now_ms;
        }
    }

    pub fn sent_inputs(&self) -> Vec<RecordedSend> {
        self.state.lock().unwrap().sent_inputs.clone()
    }

    pub fn killed_terminals(&self) -> Vec<String> {
        self.state.lock().unwrap().killed_terminals.clone()
    }

    pub fn terminal_addresses(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .terminals
            .keys()
            .cloned()
            .collect()
    }

    pub fn terminal_record(&self, address: &str) -> Option<Value> {
        self.state.lock().unwrap().terminals.get(address).cloned()
    }

    fn broadcast_locked(st: &mut State, msg: Value) {
        st.subscribers.retain(|sub| {
            let ok = sub.tx.send(Ok(msg.clone())).is_ok();
            if ok {
                (sub.wake)();
            }
            ok
        });
    }
}

impl HerdrBackend for SimHerdr {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut st = self.state.lock().unwrap();
        if st.offline {
            bail!("Herdr Core is offline");
        }
        match method {
            "server.status" => Ok(json!({
                "core": { "id": st.core_id },
                "ready": true,
            })),
            "templates.list" => Ok(json!({ "templates": [] })),
            "terminal.list" => {
                let terminals: Vec<Value> = st.terminals.values().cloned().collect();
                Ok(json!({ "terminals": terminals }))
            }
            "terminal.get" => {
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing terminal"))?;
                st.terminals
                    .get(addr)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Terminal {addr} not found"))
            }
            "terminal.read" => Ok(json!({ "output": "", "lines": [] })),
            "terminal.run" => {
                let num = st.next_term_num;
                st.next_term_num += 1;
                let addr = format!("sim-t{num}");
                let cwd = params["cwd"].as_str().unwrap_or("/sim/repo").to_owned();
                let labels = params.get("labels").cloned().unwrap_or_else(|| json!({}));
                let rec = json!({
                    "address": addr,
                    "state": "live",
                    "cwd": cwd,
                    "pid": 2000 + num,
                    "turn": 1,
                    "status": { "state": "working" },
                    "labels": labels,
                });
                st.seq += 1;
                let seq = st.seq;
                st.terminals.insert(addr.clone(), rec.clone());
                Self::broadcast_locked(
                    &mut st,
                    json!({
                        "method": "event",
                        "params": {
                            "seq": seq,
                            "type": "created",
                            "terminal": addr,
                            "data": { "record": rec },
                        }
                    }),
                );
                let mut resp = rec;
                resp["terminal"] = json!(addr);
                resp["core_id"] = json!(st.core_id);
                Ok(resp)
            }
            "terminal.send" => {
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing terminal"))?
                    .to_owned();
                let text = params["text"].as_str().unwrap_or("").to_owned();
                let if_guard = params.get("if").and_then(Value::as_str).map(str::to_owned);
                if let Some(err) = st.next_send_errors.pop_front() {
                    if err.contains("\"status_mismatch\"")
                        && let Some(rec) = st.terminals.get_mut(&addr)
                    {
                        rec["status"] = json!({ "state": "working" });
                        st.seq += 1;
                        let seq = st.seq;
                        Self::broadcast_locked(
                            &mut st,
                            json!({
                                "method": "event",
                                "params": {
                                    "seq": seq,
                                    "type": "status",
                                    "terminal": addr,
                                    "data": { "status": { "state": "working" } },
                                }
                            }),
                        );
                    }
                    bail!("{err}");
                }
                let Some(rec) = st.terminals.get(&addr) else {
                    bail!("Terminal {addr} not found");
                };
                if let Some(expected) = if_guard.as_deref() {
                    let actual = rec["status"]["state"].as_str().unwrap_or("unknown");
                    if actual != expected {
                        bail!(
                            "{{\"kind\":\"status_mismatch\",\"expected\":\"{expected}\",\"actual\":\"{actual}\"}}"
                        );
                    }
                }
                let at_ms = self.clock.now_ms();
                st.sent_inputs.push(RecordedSend {
                    terminal: addr.clone(),
                    text: text.clone(),
                    if_guard,
                    at_ms,
                });
                // If an exit command like `/exit` is sent, transition terminal to finished.
                if text.trim().contains("/exit") || text.trim() == "exit" {
                    let finished_ms = at_ms;
                    if let Some(rec) = st.terminals.get_mut(&addr) {
                        rec["state"] = json!("finished");
                        rec["exit_code"] = json!(0);
                        rec["reason"] = json!("exited");
                        rec["finished_ms"] = json!(finished_ms);
                        rec.as_object_mut().unwrap().remove("foreground");
                    }
                    st.seq += 1;
                    let seq = st.seq;
                    Self::broadcast_locked(
                        &mut st,
                        json!({
                            "method": "event",
                            "params": {
                                "seq": seq,
                                "type": "finished",
                                "terminal": addr,
                                "data": {
                                    "exit_code": 0,
                                    "reason": "exited",
                                    "finished_ms": finished_ms,
                                },
                            }
                        }),
                    );
                }
                Ok(json!({ "sent": true }))
            }
            "terminal.kill" => {
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing terminal"))?
                    .to_owned();
                st.killed_terminals.push(addr.clone());
                let finished_ms = self.clock.now_ms();
                if let Some(rec) = st.terminals.get_mut(&addr) {
                    rec["state"] = json!("finished");
                    rec["exit_code"] = json!(0);
                    rec["reason"] = json!("killed");
                    rec["finished_ms"] = json!(finished_ms);
                    rec.as_object_mut().unwrap().remove("foreground");
                }
                st.seq += 1;
                let seq = st.seq;
                Self::broadcast_locked(
                    &mut st,
                    json!({
                        "method": "event",
                        "params": {
                            "seq": seq,
                            "type": "finished",
                            "terminal": addr,
                            "data": {
                                "exit_code": 0,
                                "reason": "killed",
                                "finished_ms": finished_ms,
                            },
                        }
                    }),
                );
                Ok(json!({ "killed": true }))
            }
            other => bail!("SimHerdr unsupported method: {other}"),
        }
    }

    fn subscribe(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Result<BackendSubscription> {
        let mut st = self.state.lock().unwrap();
        if st.offline {
            bail!("Herdr Core is offline");
        }
        let (tx, rx) = mpsc::channel();
        let snapshot = json!({
            "seq": st.seq,
            "terminals": st.terminals.values().cloned().collect::<Vec<_>>(),
        });
        let core = st.core_id.clone();
        st.subscribers.push(SubEntry { tx, wake });
        Ok(BackendSubscription {
            core,
            snapshot,
            receiver: rx,
        })
    }

    fn activity(&self, core: &str, terminal: &str) -> Activity {
        self.state
            .lock()
            .unwrap()
            .activities
            .get(&(core.to_owned(), terminal.to_owned()))
            .copied()
            .unwrap_or_default()
    }
}
