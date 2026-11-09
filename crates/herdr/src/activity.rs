//! Local viewer leases. No Card or workflow knowledge lives in the transport.
use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        mpsc::{SyncSender, TrySendError},
    },
};
#[derive(Clone, Copy, Default)]
pub struct Activity {
    pub viewers: usize,
    pub last_at: u64,
}
fn registry() -> &'static Mutex<HashMap<(String, String), Activity>> {
    static REGISTRY: OnceLock<Mutex<HashMap<(String, String), Activity>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
pub fn get(core: &str, terminal: &str) -> Activity {
    registry()
        .lock()
        .unwrap()
        .get(&(core.into(), terminal.into()))
        .copied()
        .unwrap_or_default()
}
pub struct Viewer {
    key: (String, String),
}
impl Viewer {
    pub fn new(core: &str, terminal: &str) -> Self {
        let key = (core.into(), terminal.into());
        let mut reg = registry().lock().unwrap();
        let a = reg.entry(key.clone()).or_default();
        a.viewers += 1;
        a.last_at = canban_types::now();
        drop(reg);
        changed();
        Self { key }
    }
    pub fn touch(&self) {
        if let Some(a) = registry().lock().unwrap().get_mut(&self.key) {
            a.last_at = canban_types::now();
        }
        changed();
    }
}
impl Drop for Viewer {
    fn drop(&mut self) {
        if let Some(a) = registry().lock().unwrap().get_mut(&self.key) {
            a.viewers = a.viewers.saturating_sub(1);
            a.last_at = canban_types::now();
        }
        changed();
    }
}

/// Human input observed by AppServer's framed relay (separate from render acknowledgments).
pub fn touch(core: &str, terminal: &str) {
    if let Some(a) = registry()
        .lock()
        .unwrap()
        .get_mut(&(core.into(), terminal.into()))
    {
        a.last_at = canban_types::now();
    }
    changed();
}

fn listeners() -> &'static Mutex<Vec<SyncSender<()>>> {
    static LISTENERS: OnceLock<Mutex<Vec<SyncSender<()>>>> = OnceLock::new();
    LISTENERS.get_or_init(Default::default)
}
pub fn subscribe(sender: SyncSender<()>) {
    listeners().lock().unwrap().push(sender);
}
fn changed() {
    listeners()
        .lock()
        .unwrap()
        .retain(|tx| !matches!(tx.try_send(()), Err(TrySendError::Disconnected(_))));
}
