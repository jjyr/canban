//! Local viewer leases. No Card or workflow knowledge lives in the transport.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
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
        Self { key }
    }
    pub fn touch(&self) {
        if let Some(a) = registry().lock().unwrap().get_mut(&self.key) {
            a.last_at = canban_types::now();
        }
    }
}
impl Drop for Viewer {
    fn drop(&mut self) {
        if let Some(a) = registry().lock().unwrap().get_mut(&self.key) {
            a.viewers = a.viewers.saturating_sub(1);
            a.last_at = canban_types::now();
        }
    }
}
