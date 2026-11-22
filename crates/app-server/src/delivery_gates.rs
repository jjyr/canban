//! Fine-grained concurrency gates for code delivery and main checkout operations.
//!
//! Main checkout mutations (`git.pull`, `git.commit`, `review.merge`, single-card delivery, and
//! batch fast-forward/push) are sharded by canonical checkout directory so unrelated repositories
//! never block each other, while symlinked paths to the same physical checkout serialize safely.
//! Project merge-queue execution is serialized per `project_id` so `_merge_queue` integration and
//! test runs do not hold any main-checkout lock. Remote GitHub PR actions serialize per `card_id`.

use super::*;
use std::collections::HashMap;
use std::hash::Hash;

pub(crate) type CheckoutGateKey = PathBuf;

#[derive(Default)]
struct GateState {
    busy: bool,
    holder: Option<String>,
}

pub(crate) struct Gate {
    state: Mutex<GateState>,
    signal: std::sync::Condvar,
}

impl Gate {
    fn new() -> Self {
        Self {
            state: Mutex::new(GateState::default()),
            signal: std::sync::Condvar::new(),
        }
    }

    fn is_busy(&self) -> bool {
        self.state.lock().unwrap().busy
    }

    pub(crate) fn try_lock(
        self: &Arc<Self>,
        holder: impl Into<String>,
    ) -> std::result::Result<GateGuard, Option<String>> {
        let mut state = self.state.lock().unwrap();
        if state.busy {
            return Err(state.holder.clone());
        }
        state.busy = true;
        state.holder = Some(holder.into());
        Ok(GateGuard {
            gate: Arc::clone(self),
        })
    }

    pub(crate) fn lock(self: &Arc<Self>, holder: impl Into<String>) -> GateGuard {
        let mut state = self.state.lock().unwrap();
        while state.busy {
            state = self.signal.wait(state).unwrap();
        }
        state.busy = true;
        state.holder = Some(holder.into());
        GateGuard {
            gate: Arc::clone(self),
        }
    }
}

pub(crate) struct GateGuard {
    gate: Arc<Gate>,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        {
            let mut state = self.gate.state.lock().unwrap();
            state.busy = false;
            state.holder = None;
        }
        self.gate.signal.notify_all();
    }
}

pub(crate) struct KeyedGateTable<K> {
    entries: Mutex<HashMap<K, Arc<Gate>>>,
}

impl<K> Default for KeyedGateTable<K> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash + Clone> KeyedGateTable<K> {
    pub(crate) fn gate(&self, key: K) -> Arc<Gate> {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() > 256 {
            entries.retain(|_, gate| Arc::strong_count(gate) > 1 || gate.is_busy());
        }
        Arc::clone(entries.entry(key).or_insert_with(|| Arc::new(Gate::new())))
    }
}

pub(crate) fn format_checkout_busy_message(holder: Option<&str>) -> String {
    match holder {
        Some(desc) => format!("{desc} is updating this checkout; retry in a moment"),
        None => "Another code delivery is updating this checkout; retry in a moment".into(),
    }
}

impl AppServer {
    pub(crate) fn checkout_gate_key(&self, project: &Project) -> CheckoutGateKey {
        std::fs::canonicalize(&project.directory)
            .unwrap_or_else(|_| PathBuf::from(&project.directory))
    }

    pub(crate) fn checkout_holder_label(
        &self,
        action: &str,
        project: &Project,
        branch: Option<&str>,
    ) -> String {
        let branch = branch.map(str::trim).filter(|b| !b.is_empty()).or_else(|| {
            project
                .target_branch
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
        });
        match branch {
            Some(b) => format!("{action} for {} ({b})", project.name),
            None => format!("{action} for {}", project.name),
        }
    }

    pub(crate) fn try_lock_checkout(
        &self,
        project: &Project,
        holder: impl Into<String>,
    ) -> std::result::Result<GateGuard, String> {
        let key = self.checkout_gate_key(project);
        self.checkout_gates
            .gate(key)
            .try_lock(holder)
            .map_err(|active| format_checkout_busy_message(active.as_deref()))
    }

    pub(crate) fn lock_checkout(&self, project: &Project, holder: impl Into<String>) -> GateGuard {
        let key = self.checkout_gate_key(project);
        self.checkout_gates.gate(key).lock(holder)
    }

    pub(crate) fn try_lock_delivery_queue(&self, project_id: &str) -> Option<GateGuard> {
        self.queue_gates
            .gate(project_id.to_owned())
            .try_lock(project_id)
            .ok()
    }

    pub(crate) fn try_lock_card_delivery(
        &self,
        card_id: &str,
        holder: impl Into<String>,
    ) -> Result<GateGuard> {
        self.card_delivery_gates
            .gate(card_id.to_owned())
            .try_lock(holder)
            .map_err(|_| {
                anyhow::anyhow!(
                    "Another delivery for this Card is in progress; retry after it completes"
                )
            })
    }
}
