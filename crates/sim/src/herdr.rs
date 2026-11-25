//! Deterministic in-memory Herdr Core simulator implementing [`canban_herdr::HerdrBackend`].
use anyhow::{Result, bail};
use canban_herdr::{BackendSubscription, HerdrBackend, activity::Activity};
use canban_types::Clock;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, mpsc},
};

/// Bounded subscriber queue capacity matching production `crates/herdr/src/events.rs`.
pub const SUBSCRIBER_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone, Debug)]
pub struct RecordedSend {
    pub terminal: String,
    pub text: String,
    pub if_guard: Option<String>,
    pub at_ms: u64,
}

/// Injectable fault triggered on the next `terminal.run` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunFault {
    /// Fail `terminal.run` before creating any terminal.
    FailBeforeCreate(String),
    /// Create the live terminal and broadcast `created`, then fail the RPC
    /// (simulates an orphaned PTY after timeout/disconnect before receipt commit).
    OrphanAfterCreate(String),
    /// Return a live terminal record with `first_message: { "sent": false }`.
    FirstMessageUnsent,
}

#[derive(Clone, Debug, Default)]
struct TerminalBuffer {
    screen: String,
    scrollback: Vec<String>,
}

struct SubEntry {
    tx: mpsc::SyncSender<Result<Value>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

struct State {
    core_id: String,
    seq: u64,
    next_term_num: u64,
    terminals: BTreeMap<String, Value>,
    buffers: BTreeMap<String, TerminalBuffer>,
    activities: BTreeMap<(String, String), Activity>,
    subscribers: Vec<SubEntry>,
    sent_inputs: Vec<RecordedSend>,
    killed_terminals: Vec<String>,
    interrupted_terminals: Vec<String>,
    /// Optional scripted next error for `terminal.send` (e.g. `status_mismatch` or `blocked`).
    next_send_errors: VecDeque<String>,
    /// Optional scripted next error for `terminal.read`.
    next_read_errors: VecDeque<String>,
    /// Optional scripted next error for `terminal.kill`.
    next_kill_errors: VecDeque<String>,
    /// Optional scripted next error for `terminal.interrupt`.
    next_interrupt_errors: VecDeque<String>,
    /// Optional scripted next fault for `terminal.run`.
    next_run_faults: VecDeque<RunFault>,
    /// Terminals whose `/exit` send does not auto-finish (tests hung close & 30s timeout escalation).
    hang_exit_terminals: BTreeSet<String>,
    hang_all_exits: bool,
    /// Terminals whose `terminal.kill` does not auto-finish (tests stuck kill).
    hang_kill_terminals: BTreeSet<String>,
    hang_all_kills: bool,
    /// Terminals whose `terminal.interrupt` leaves status unchanged.
    hang_interrupt_terminals: BTreeSet<String>,
    /// Event stream fault injection knobs.
    drop_next_events: usize,
    duplicate_next_events: usize,
    reorder_remaining: usize,
    reorder_buffer: Vec<Value>,
    /// Count of subscribers dropped due to bounded-channel overflow.
    overflow_drops: u64,
    /// Total control-plane heartbeats emitted via `emit_heartbeat` / `maybe_emit_heartbeat`.
    heartbeat_count: u64,
    /// Total successful `subscribe` calls accepted by this core.
    subscribe_count: u64,
    /// Last virtual timestamp at which `maybe_emit_heartbeat` emitted a heartbeat.
    last_heartbeat_ms: u64,
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
        let initial_ms = clock.now_ms();
        Arc::new(Self {
            clock,
            state: Mutex::new(State {
                core_id: core_id.into(),
                seq: 1,
                next_term_num: 1,
                terminals: BTreeMap::new(),
                buffers: BTreeMap::new(),
                activities: BTreeMap::new(),
                subscribers: Vec::new(),
                sent_inputs: Vec::new(),
                killed_terminals: Vec::new(),
                interrupted_terminals: Vec::new(),
                next_send_errors: VecDeque::new(),
                next_read_errors: VecDeque::new(),
                next_kill_errors: VecDeque::new(),
                next_interrupt_errors: VecDeque::new(),
                next_run_faults: VecDeque::new(),
                hang_exit_terminals: BTreeSet::new(),
                hang_all_exits: false,
                hang_kill_terminals: BTreeSet::new(),
                hang_all_kills: false,
                hang_interrupt_terminals: BTreeSet::new(),
                drop_next_events: 0,
                duplicate_next_events: 0,
                reorder_remaining: 0,
                reorder_buffer: Vec::new(),
                overflow_drops: 0,
                heartbeat_count: 0,
                subscribe_count: 0,
                last_heartbeat_ms: initial_ms,
                offline: false,
            }),
        })
    }

    pub fn core_id(&self) -> String {
        self.state.lock().unwrap().core_id.clone()
    }

    pub fn seq(&self) -> u64 {
        self.state.lock().unwrap().seq
    }

    pub fn overflow_drop_count(&self) -> u64 {
        self.state.lock().unwrap().overflow_drops
    }

    pub fn subscriber_count(&self) -> usize {
        self.state.lock().unwrap().subscribers.len()
    }

    pub fn heartbeat_count(&self) -> u64 {
        self.state.lock().unwrap().heartbeat_count
    }

    pub fn subscribe_count(&self) -> u64 {
        self.state.lock().unwrap().subscribe_count
    }

    pub fn set_offline(&self, offline: bool) {
        let mut st = self.state.lock().unwrap();
        st.offline = offline;
        if offline {
            let subs = std::mem::take(&mut st.subscribers);
            for sub in subs {
                let _ = sub
                    .tx
                    .try_send(Err(anyhow::anyhow!("Herdr Core went offline")));
                (sub.wake)();
            }
        }
    }

    /// Simulates a Herdr Core daemon restart with a new `core_id`: resets `seq`, drops
    /// all prior live PTYs/activities/buffers (matching real daemon restart semantics),
    /// and closes active subscriptions.
    pub fn restart_core(&self, new_core_id: impl Into<String>) {
        crate::trace::record_coverage_hit("herdr.restart_core");
        let mut st = self.state.lock().unwrap();
        st.core_id = new_core_id.into();
        st.seq = 1;
        st.terminals.clear();
        st.buffers.clear();
        st.activities.clear();
        st.hang_exit_terminals.clear();
        st.hang_kill_terminals.clear();
        st.hang_interrupt_terminals.clear();
        st.reorder_buffer.clear();
        st.reorder_remaining = 0;
        let subs = std::mem::take(&mut st.subscribers);
        for sub in subs {
            let _ = sub.tx.try_send(Err(anyhow::anyhow!(
                "Herdr Core restarted with new identity"
            )));
            (sub.wake)();
        }
    }

    /// Alias for [`Self::restart_core`] emphasizing full daemon state reset semantics.
    pub fn restart_core_clean(&self, new_core_id: impl Into<String>) {
        self.restart_core(new_core_id);
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
        st.buffers.entry(address.clone()).or_default();
        Self::emit_event_locked(
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
        Self::emit_event_locked(
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
            Self::emit_event_locked(
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

    /// Sets the visible screen text returned by `terminal.read` (`lines: 0`).
    pub fn set_screen(&self, address: &str, screen_text: impl Into<String>) {
        let mut st = self.state.lock().unwrap();
        if st.terminals.contains_key(address) {
            st.buffers.entry(address.to_owned()).or_default().screen = screen_text.into();
        }
    }

    /// Appends scrollback lines returned by `terminal.read` (`lines > 0`).
    pub fn append_output<I, S>(&self, address: &str, lines: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut st = self.state.lock().unwrap();
        if st.terminals.contains_key(address) {
            let buf = st.buffers.entry(address.to_owned()).or_default();
            for line in lines {
                buf.scrollback.push(line.into());
            }
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
        Self::emit_event_locked(
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
        Self::emit_event_locked(
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

    /// Removes a terminal from Herdr inventory and broadcasts a `removed` event.
    pub fn remove_terminal(&self, address: &str) {
        let mut st = self.state.lock().unwrap();
        st.terminals.remove(address);
        st.buffers.remove(address);
        st.hang_exit_terminals.remove(address);
        st.hang_kill_terminals.remove(address);
        st.hang_interrupt_terminals.remove(address);
        st.seq += 1;
        let seq = st.seq;
        Self::emit_event_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "removed",
                    "terminal": address,
                    "data": {},
                }
            }),
        );
    }

    /// Restarts a terminal in place with a fresh pid and broadcasts a `restarted` event.
    pub fn restart_terminal(&self, address: &str, cwd: Option<&str>, status_state: &str) {
        let mut st = self.state.lock().unwrap();
        let existing_cwd = st
            .terminals
            .get(address)
            .and_then(|r| r["cwd"].as_str())
            .unwrap_or("/sim/repo")
            .to_owned();
        let labels = st
            .terminals
            .get(address)
            .and_then(|r| r.get("labels"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let pid = 3000 + st.seq;
        let rec = json!({
            "address": address,
            "state": "live",
            "cwd": cwd.unwrap_or(&existing_cwd),
            "pid": pid,
            "turn": 1,
            "status": { "state": status_state },
            "labels": labels,
        });
        st.seq += 1;
        let seq = st.seq;
        st.terminals.insert(address.to_owned(), rec.clone());
        st.buffers.entry(address.to_owned()).or_default();
        Self::emit_event_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "restarted",
                    "terminal": address,
                    "data": { "record": rec },
                }
            }),
        );
    }

    /// Updates a terminal's `foreground` process metadata and broadcasts a `foreground` event.
    pub fn set_foreground(&self, address: &str, foreground: Option<Value>) {
        let mut st = self.state.lock().unwrap();
        let Some(rec) = st.terminals.get_mut(address) else {
            return;
        };
        let data = match foreground {
            Some(fg) => {
                rec["foreground"] = fg.clone();
                fg
            }
            None => {
                rec.as_object_mut().unwrap().remove("foreground");
                Value::Null
            }
        };
        st.seq += 1;
        let seq = st.seq;
        Self::emit_event_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "foreground",
                    "terminal": address,
                    "data": data,
                }
            }),
        );
    }

    /// Updates a terminal's OSC window title and broadcasts a `title` event.
    pub fn set_title(&self, address: &str, title: impl Into<String>) {
        let title = title.into();
        let mut st = self.state.lock().unwrap();
        let Some(rec) = st.terminals.get_mut(address) else {
            return;
        };
        rec["title"] = json!(title);
        st.seq += 1;
        let seq = st.seq;
        Self::emit_event_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "title",
                    "terminal": address,
                    "data": { "title": title },
                }
            }),
        );
    }

    /// Updates a terminal's labels and broadcasts a `labels` event.
    pub fn set_labels(&self, address: &str, labels: Value) {
        let mut st = self.state.lock().unwrap();
        let Some(rec) = st.terminals.get_mut(address) else {
            return;
        };
        rec["labels"] = labels.clone();
        st.seq += 1;
        let seq = st.seq;
        Self::emit_event_locked(
            &mut st,
            json!({
                "method": "event",
                "params": {
                    "seq": seq,
                    "type": "labels",
                    "terminal": address,
                    "data": { "labels": labels },
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
                    "type": "missed",
                    "terminals": terminals,
                }
            }),
        );
    }

    /// Sends a Herdr control-plane `heartbeat` frame.
    pub fn emit_heartbeat(&self) {
        let now_ms = self.clock.now_ms();
        let mut st = self.state.lock().unwrap();
        st.last_heartbeat_ms = now_ms;
        st.heartbeat_count += 1;
        let seq = st.seq;
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "heartbeat",
                "params": { "seq": seq }
            }),
        );
    }

    /// Emits a control-plane `heartbeat` if at least `interval_ms` virtual time has elapsed
    /// since the previous heartbeat (and the core is online). Returns `true` if emitted.
    pub fn maybe_emit_heartbeat(&self, interval_ms: u64) -> bool {
        let now_ms = self.clock.now_ms();
        let mut st = self.state.lock().unwrap();
        if st.offline || now_ms.saturating_sub(st.last_heartbeat_ms) < interval_ms {
            return false;
        }
        st.last_heartbeat_ms = now_ms;
        st.heartbeat_count += 1;
        let seq = st.seq;
        Self::broadcast_locked(
            &mut st,
            json!({
                "method": "heartbeat",
                "params": { "seq": seq }
            }),
        );
        true
    }

    /// Floods subscribers with more than [`SUBSCRIBER_CHANNEL_CAPACITY`] title frames without
    /// yielding so any unconsumed subscriber overflows and disconnects (matching production
    /// bounded `sync_channel(256)` backpressure in `crates/herdr/src/events.rs`).
    pub fn simulate_subscriber_overflow(&self, address: &str) {
        let mut st = self.state.lock().unwrap();
        for i in 0..(SUBSCRIBER_CHANNEL_CAPACITY + 16) {
            st.seq += 1;
            let seq = st.seq;
            Self::broadcast_locked(
                &mut st,
                json!({
                    "method": "event",
                    "params": {
                        "seq": seq,
                        "type": "title",
                        "terminal": address,
                        "data": { "title": format!("spinner-{i}") },
                    }
                }),
            );
        }
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

    /// Queues a synthetic error response on the next `terminal.read` call.
    pub fn inject_next_read_error(&self, err_msg: impl Into<String>) {
        self.state
            .lock()
            .unwrap()
            .next_read_errors
            .push_back(err_msg.into());
    }

    /// Queues a synthetic error response on the next `terminal.kill` call.
    pub fn inject_next_kill_error(&self, err_msg: impl Into<String>) {
        self.state
            .lock()
            .unwrap()
            .next_kill_errors
            .push_back(err_msg.into());
    }

    /// Queues a synthetic error response on the next `terminal.interrupt` call.
    pub fn inject_next_interrupt_error(&self, err_msg: impl Into<String>) {
        self.state
            .lock()
            .unwrap()
            .next_interrupt_errors
            .push_back(err_msg.into());
    }

    /// Queues a scripted fault on the next `terminal.run` call.
    pub fn inject_next_run_fault(&self, fault: RunFault) {
        self.state.lock().unwrap().next_run_faults.push_back(fault);
    }

    /// Causes the next `terminal.run` to return `first_message: { "sent": false }`.
    pub fn inject_next_run_first_message_unsent(&self) {
        self.inject_next_run_fault(RunFault::FirstMessageUnsent);
    }

    /// Causes the next `terminal.run` to create a live PTY in Herdr and then fail the RPC,
    /// leaving an orphaned terminal binding for recovery testing.
    pub fn inject_next_run_orphan_error(&self, err_msg: impl Into<String>) {
        self.inject_next_run_fault(RunFault::OrphanAfterCreate(err_msg.into()));
    }

    /// Configures whether `/exit` sent to `address` hangs instead of immediately exiting.
    pub fn set_hang_exit(&self, address: &str, hang: bool) {
        let mut st = self.state.lock().unwrap();
        if hang {
            st.hang_exit_terminals.insert(address.to_owned());
        } else {
            st.hang_exit_terminals.remove(address);
        }
    }

    /// Configures whether `/exit` hangs across all terminals.
    pub fn set_hang_all_exits(&self, hang: bool) {
        self.state.lock().unwrap().hang_all_exits = hang;
    }

    /// Configures whether `terminal.kill` on `address` hangs without emitting `finished`.
    pub fn set_hang_kill(&self, address: &str, hang: bool) {
        let mut st = self.state.lock().unwrap();
        if hang {
            st.hang_kill_terminals.insert(address.to_owned());
        } else {
            st.hang_kill_terminals.remove(address);
        }
    }

    /// Configures whether `terminal.kill` hangs across all terminals.
    pub fn set_hang_all_kills(&self, hang: bool) {
        self.state.lock().unwrap().hang_all_kills = hang;
    }

    /// Configures whether `terminal.interrupt` on `address` leaves status unchanged.
    pub fn set_hang_interrupt(&self, address: &str, hang: bool) {
        let mut st = self.state.lock().unwrap();
        if hang {
            st.hang_interrupt_terminals.insert(address.to_owned());
        } else {
            st.hang_interrupt_terminals.remove(address);
        }
    }

    /// Drops the next `count` event broadcasts (state & `seq` still advance, simulating silent gaps).
    pub fn drop_next_events(&self, count: usize) {
        self.state.lock().unwrap().drop_next_events += count;
    }

    /// Duplicates the next `count` event broadcasts (delivered twice to subscribers).
    pub fn duplicate_next_events(&self, count: usize) {
        self.state.lock().unwrap().duplicate_next_events += count;
    }

    /// Buffers the next `count` event broadcasts and delivers them in reverse order once collected.
    pub fn buffer_next_events_for_reorder(&self, count: usize) {
        let mut st = self.state.lock().unwrap();
        st.reorder_remaining = count;
    }

    /// Flushes any currently buffered reordered events immediately (in reverse order).
    pub fn flush_reordered_events(&self) {
        let mut st = self.state.lock().unwrap();
        st.reorder_remaining = 0;
        let buffered = std::mem::take(&mut st.reorder_buffer);
        for msg in buffered.into_iter().rev() {
            Self::broadcast_locked(&mut st, msg);
        }
    }

    /// Clears all queued faults, hang flags, and event-stream fault buffers.
    pub fn clear_injected_faults(&self) {
        let mut st = self.state.lock().unwrap();
        st.next_send_errors.clear();
        st.next_read_errors.clear();
        st.next_kill_errors.clear();
        st.next_interrupt_errors.clear();
        st.next_run_faults.clear();
        st.hang_exit_terminals.clear();
        st.hang_all_exits = false;
        st.hang_kill_terminals.clear();
        st.hang_all_kills = false;
        st.hang_interrupt_terminals.clear();
        st.drop_next_events = 0;
        st.duplicate_next_events = 0;
        st.reorder_remaining = 0;
        let buffered = std::mem::take(&mut st.reorder_buffer);
        for msg in buffered {
            Self::broadcast_locked(&mut st, msg);
        }
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
        for sub in &st.subscribers {
            (sub.wake)();
        }
    }

    pub fn sent_inputs(&self) -> Vec<RecordedSend> {
        self.state.lock().unwrap().sent_inputs.clone()
    }

    pub fn killed_terminals(&self) -> Vec<String> {
        self.state.lock().unwrap().killed_terminals.clone()
    }

    pub fn interrupted_terminals(&self) -> Vec<String> {
        self.state.lock().unwrap().interrupted_terminals.clone()
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

    /// Returns `(address, record)` for all simulated script PTY terminals (`labels["canban.script_run"]`).
    pub fn script_terminals(&self) -> Vec<(String, Value)> {
        self.state
            .lock()
            .unwrap()
            .terminals
            .iter()
            .filter(|(_, rec)| {
                rec.get("labels")
                    .and_then(|l| l.get("canban.script_run"))
                    .and_then(Value::as_str)
                    .is_some()
            })
            .map(|(addr, rec)| (addr.clone(), rec.clone()))
            .collect()
    }

    /// Appends scrollback output lines to a simulated script PTY terminal and finishes it with `exit_code`.
    pub fn complete_script_terminal(&self, address: &str, output_lines: &[&str], exit_code: i32) {
        if !output_lines.is_empty() {
            self.append_output(address, output_lines.iter().copied());
        }
        self.finish_terminal(address, exit_code, "exited");
    }

    fn emit_event_locked(st: &mut State, msg: Value) {
        if st.drop_next_events > 0 {
            st.drop_next_events -= 1;
            crate::trace::record_coverage_hit("herdr.fault.event_drop");
            return;
        }
        if st.reorder_remaining > 0 {
            st.reorder_remaining -= 1;
            st.reorder_buffer.push(msg);
            if st.reorder_remaining == 0 {
                crate::trace::record_coverage_hit("herdr.fault.event_reorder");
                let buffered = std::mem::take(&mut st.reorder_buffer);
                for ev in buffered.into_iter().rev() {
                    Self::broadcast_locked(st, ev);
                }
            }
            return;
        }
        if st.duplicate_next_events > 0 {
            st.duplicate_next_events -= 1;
            crate::trace::record_coverage_hit("herdr.fault.event_duplicate");
            Self::broadcast_locked(st, msg.clone());
        }
        Self::broadcast_locked(st, msg);
    }

    fn broadcast_locked(st: &mut State, msg: Value) {
        let mut overflowed = 0u64;
        st.subscribers
            .retain(|sub| match sub.tx.try_send(Ok(msg.clone())) {
                Ok(()) => {
                    (sub.wake)();
                    true
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    overflowed += 1;
                    (sub.wake)();
                    false
                }
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            });
        if overflowed > 0 {
            st.overflow_drops += overflowed;
            crate::trace::record_coverage_hit("herdr.overflow_drop");
        }
    }
}

impl HerdrBackend for SimHerdr {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut st = self.state.lock().unwrap();
        if st.offline {
            bail!("Herdr {method}: Herdr Core is offline");
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
                    .ok_or_else(|| anyhow::anyhow!("Herdr terminal.get: Missing terminal"))?;
                st.terminals
                    .get(addr)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Herdr terminal.get: Terminal {addr} not found"))
            }
            "terminal.read" => {
                crate::trace::record_coverage_hit("herdr.terminal.read");
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Herdr terminal.read: Missing terminal"))?;
                if let Some(err) = st.next_read_errors.pop_front() {
                    bail!("Herdr terminal.read: {err}");
                }
                if !st.terminals.contains_key(addr) {
                    bail!("Herdr terminal.read: Terminal {addr} not found");
                }
                let requested_lines = params["lines"].as_u64().unwrap_or(0) as usize;
                let buf = st.buffers.get(addr);
                let (text, lines_out): (String, Vec<String>) = match buf {
                    Some(b) if requested_lines == 0 => {
                        let screen = if !b.screen.is_empty() {
                            b.screen.clone()
                        } else {
                            b.scrollback.join("\n")
                        };
                        let lines = if screen.is_empty() {
                            Vec::new()
                        } else {
                            screen.lines().map(str::to_owned).collect()
                        };
                        (screen, lines)
                    }
                    Some(b) => {
                        let source: Vec<String> = if !b.scrollback.is_empty() {
                            let skip = b.scrollback.len().saturating_sub(requested_lines);
                            b.scrollback[skip..].to_vec()
                        } else if !b.screen.is_empty() {
                            let all: Vec<String> = b.screen.lines().map(str::to_owned).collect();
                            let skip = all.len().saturating_sub(requested_lines);
                            all[skip..].to_vec()
                        } else {
                            Vec::new()
                        };
                        let joined = source.join("\n");
                        (joined, source)
                    }
                    None => (String::new(), Vec::new()),
                };
                Ok(json!({ "text": text, "lines": lines_out }))
            }
            "terminal.run" => {
                crate::trace::record_coverage_hit("herdr.terminal.run");
                let run_fault = st.next_run_faults.pop_front();
                if let Some(RunFault::FailBeforeCreate(err)) = run_fault {
                    bail!("Herdr terminal.run: {err}");
                }
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
                st.buffers.insert(addr.clone(), TerminalBuffer::default());
                Self::emit_event_locked(
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
                if let Some(RunFault::OrphanAfterCreate(err)) = run_fault {
                    crate::trace::record_coverage_hit("herdr.fault.orphan_run");
                    bail!("Herdr terminal.run: {err}");
                }
                let mut resp = rec;
                resp["terminal"] = json!(addr);
                resp["core_id"] = json!(st.core_id);
                if matches!(run_fault, Some(RunFault::FirstMessageUnsent)) {
                    crate::trace::record_coverage_hit("herdr.fault.first_message_unsent");
                    resp["first_message"] = json!({ "sent": false });
                }
                Ok(resp)
            }
            "terminal.send" => {
                crate::trace::record_coverage_hit("herdr.terminal.send");
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Herdr terminal.send: Missing terminal"))?
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
                        Self::emit_event_locked(
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
                    bail!("Herdr terminal.send: Terminal {addr} not found");
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
                // If an exit command like `/exit` is sent, transition terminal to finished
                // unless exit-hang fault injection is active for this terminal.
                if text.trim().contains("/exit") || text.trim() == "exit" {
                    let hung = st.hang_all_exits || st.hang_exit_terminals.contains(&addr);
                    if hung {
                        crate::trace::record_coverage_hit("herdr.fault.hang_exit");
                    } else {
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
                        Self::emit_event_locked(
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
                }
                Ok(json!({ "sent": true }))
            }
            "terminal.interrupt" => {
                crate::trace::record_coverage_hit("herdr.terminal.interrupt");
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Herdr terminal.interrupt: Missing terminal"))?
                    .to_owned();
                if let Some(err) = st.next_interrupt_errors.pop_front() {
                    bail!("Herdr terminal.interrupt: {err}");
                }
                if !st.terminals.contains_key(&addr) {
                    bail!("Herdr terminal.interrupt: Terminal {addr} not found");
                }
                st.interrupted_terminals.push(addr.clone());
                if !st.hang_interrupt_terminals.contains(&addr)
                    && let Some(rec) = st.terminals.get_mut(&addr)
                {
                    let cur = rec["status"]["state"].as_str().unwrap_or("");
                    if matches!(cur, "working" | "blocked") {
                        rec["status"] = json!({ "state": "idle" });
                        st.seq += 1;
                        let seq = st.seq;
                        Self::emit_event_locked(
                            &mut st,
                            json!({
                                "method": "event",
                                "params": {
                                    "seq": seq,
                                    "type": "status",
                                    "terminal": addr,
                                    "data": { "status": { "state": "idle" } },
                                }
                            }),
                        );
                    }
                }
                Ok(json!({ "interrupted": true }))
            }
            "terminal.kill" => {
                crate::trace::record_coverage_hit("herdr.terminal.kill");
                let addr = params["terminal"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Herdr terminal.kill: Missing terminal"))?
                    .to_owned();
                if let Some(err) = st.next_kill_errors.pop_front() {
                    bail!("Herdr terminal.kill: {err}");
                }
                st.killed_terminals.push(addr.clone());
                let hung = st.hang_all_kills || st.hang_kill_terminals.contains(&addr);
                if hung {
                    crate::trace::record_coverage_hit("herdr.fault.hang_kill");
                } else {
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
                    Self::emit_event_locked(
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
                }
                Ok(json!({ "killed": true }))
            }
            other => bail!("SimHerdr unsupported method: {other}"),
        }
    }

    fn subscribe(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Result<BackendSubscription> {
        crate::trace::record_coverage_hit("herdr.subscribe");
        let mut st = self.state.lock().unwrap();
        if st.offline {
            bail!("Herdr Core is offline");
        }
        let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_CHANNEL_CAPACITY);
        let snapshot = json!({
            "seq": st.seq,
            "terminals": st.terminals.values().cloned().collect::<Vec<_>>(),
        });
        let core = st.core_id.clone();
        st.subscribe_count += 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TurmoilClock;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_herdr() -> (Arc<SimHerdr>, Arc<AtomicU64>) {
        let elapsed = Arc::new(AtomicU64::new(0));
        let clock = Arc::new(TurmoilClock::new(Arc::clone(&elapsed)));
        (SimHerdr::new("core-test", clock), elapsed)
    }

    #[test]
    fn terminal_read_validates_existence_returns_text_and_supports_injected_errors() {
        let (herdr, _) = test_herdr();
        assert!(
            herdr
                .call(
                    "terminal.read",
                    json!({ "terminal": "missing", "lines": 10 })
                )
                .is_err()
        );

        let resp = herdr
            .call("terminal.run", json!({ "cwd": "/repo" }))
            .expect("terminal.run");
        let addr = resp["terminal"].as_str().unwrap().to_owned();

        herdr.set_screen(&addr, "prompt> working\nbackground task running");
        herdr.append_output(&addr, ["line-1", "line-2", "line-3"]);

        // lines=0 returns visible screen under `"text"`
        let screen = herdr
            .call("terminal.read", json!({ "terminal": addr, "lines": 0 }))
            .expect("screen read");
        assert_eq!(
            screen["text"].as_str(),
            Some("prompt> working\nbackground task running")
        );

        // lines=2 returns last 2 scrollback lines under `"text"`
        let tail = herdr
            .call("terminal.read", json!({ "terminal": addr, "lines": 2 }))
            .expect("tail read");
        assert_eq!(tail["text"].as_str(), Some("line-2\nline-3"));

        // injected read error fires once
        herdr.inject_next_read_error("pipe broken");
        assert!(
            herdr
                .call("terminal.read", json!({ "terminal": addr, "lines": 10 }))
                .is_err()
        );
        assert!(
            herdr
                .call("terminal.read", json!({ "terminal": addr, "lines": 10 }))
                .is_ok()
        );
    }

    #[test]
    fn restart_core_resets_seq_and_clears_prior_terminals_and_activities() {
        let (herdr, _) = test_herdr();
        let sub = herdr.subscribe(Arc::new(|| {})).expect("subscribe");
        herdr.upsert_terminal("t1", "/repo", "working", 1, Some("claude"));
        herdr.set_viewer_activity("t1", 2, true);
        assert!(herdr.seq() > 1);
        assert_eq!(herdr.terminal_addresses(), vec!["t1"]);

        herdr.restart_core("core-v2");
        assert_eq!(herdr.core_id(), "core-v2");
        assert_eq!(herdr.seq(), 1);
        assert!(herdr.terminal_addresses().is_empty());
        assert_eq!(herdr.activity("core-test", "t1").viewers, 0);

        // Old subscriber got restart error and closed.
        let mut saw_err = false;
        while let Ok(item) = sub.receiver.try_recv() {
            if item.is_err() {
                saw_err = true;
            }
        }
        assert!(saw_err);
    }

    #[test]
    fn subscriber_bounded_channel_disconnects_on_overflow() {
        let (herdr, _) = test_herdr();
        let sub = herdr.subscribe(Arc::new(|| {})).expect("subscribe");
        herdr.upsert_terminal("t1", "/repo", "working", 1, None);
        herdr.simulate_subscriber_overflow("t1");
        assert_eq!(herdr.overflow_drop_count(), 1);

        // Drain the 256 buffered messages, then channel must be Disconnected.
        let mut received = 0usize;
        loop {
            match sub.receiver.try_recv() {
                Ok(_) => received += 1,
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    panic!("expected subscriber channel to disconnect after overflow")
                }
            }
        }
        assert_eq!(received, SUBSCRIBER_CHANNEL_CAPACITY);
    }

    #[test]
    fn interrupt_orphan_run_first_message_hang_exit_and_event_faults() {
        let (herdr, elapsed) = test_herdr();
        let sub = herdr.subscribe(Arc::new(|| {})).expect("subscribe");

        // 1. first_message.sent = false
        herdr.inject_next_run_first_message_unsent();
        let r1 = herdr
            .call("terminal.run", json!({ "cwd": "/repo" }))
            .expect("run 1");
        assert_eq!(r1["first_message"]["sent"], json!(false));
        let addr1 = r1["terminal"].as_str().unwrap().to_owned();

        // 2. terminal.interrupt transitions working -> idle
        let intr = herdr
            .call("terminal.interrupt", json!({ "terminal": addr1 }))
            .expect("interrupt");
        assert_eq!(intr["interrupted"], json!(true));
        assert_eq!(herdr.interrupted_terminals(), vec![addr1.clone()]);
        assert_eq!(
            herdr.terminal_record(&addr1).unwrap()["status"]["state"],
            json!("idle")
        );

        // 3. Orphan-after-create leaves a live terminal despite returning Err
        herdr.inject_next_run_orphan_error("timed out waiting for response");
        assert!(
            herdr
                .call("terminal.run", json!({ "cwd": "/repo" }))
                .is_err()
        );
        assert_eq!(herdr.terminal_addresses().len(), 2);

        // 4. Hung /exit and hung kill leave terminal live until finish_terminal
        herdr.set_hang_exit(&addr1, true);
        herdr
            .call(
                "terminal.send",
                json!({ "terminal": addr1, "text": "/exit\r" }),
            )
            .expect("send /exit");
        assert_eq!(
            herdr.terminal_record(&addr1).unwrap()["state"],
            json!("live")
        );

        herdr.set_hang_kill(&addr1, true);
        herdr
            .call("terminal.kill", json!({ "terminal": addr1 }))
            .expect("kill");
        assert_eq!(
            herdr.terminal_record(&addr1).unwrap()["state"],
            json!("live")
        );

        // 5. Additional event types & reorder/drop/duplicate
        while sub.receiver.try_recv().is_ok() {}
        herdr.set_foreground(&addr1, Some(json!({ "name": "codex", "cwd": "/repo/wt" })));
        herdr.set_title(&addr1, "running tests");
        herdr.restart_terminal(&addr1, Some("/repo/wt"), "working");
        herdr.remove_terminal(&addr1);

        let mut types = Vec::new();
        while let Ok(Ok(ev)) = sub.receiver.try_recv() {
            if let Some(t) = ev["params"]["type"].as_str() {
                types.push(t.to_owned());
            }
        }
        assert_eq!(types, vec!["foreground", "title", "restarted", "removed"]);

        // 6. Drop / duplicate / reorder
        herdr.upsert_terminal("t-fault", "/repo", "working", 1, None);
        while sub.receiver.try_recv().is_ok() {}

        herdr.drop_next_events(1);
        herdr.set_status("t-fault", "idle", None);
        assert!(sub.receiver.try_recv().is_err());

        herdr.duplicate_next_events(1);
        herdr.set_status("t-fault", "working", None);
        let d1 = sub.receiver.try_recv().unwrap().unwrap();
        let d2 = sub.receiver.try_recv().unwrap().unwrap();
        assert_eq!(d1["params"]["seq"], d2["params"]["seq"]);

        herdr.buffer_next_events_for_reorder(2);
        herdr.set_status("t-fault", "idle", None);
        herdr.set_status("t-fault", "done", None);
        let e1 = sub.receiver.try_recv().unwrap().unwrap();
        let e2 = sub.receiver.try_recv().unwrap().unwrap();
        assert!(e1["params"]["seq"].as_u64().unwrap() > e2["params"]["seq"].as_u64().unwrap());

        // 7. Periodic heartbeat helper
        assert!(!herdr.maybe_emit_heartbeat(10_000));
        elapsed.store(10_000, Ordering::SeqCst);
        assert!(herdr.maybe_emit_heartbeat(10_000));
        assert!(!herdr.maybe_emit_heartbeat(10_000));
    }
}
