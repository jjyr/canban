//! One coalescing writer per desktop process; never blocks native keystrokes.
use super::Draft;
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant},
};

static SERVICE: OnceLock<Service> = OnceLock::new();
#[derive(Default)]
struct Entry {
    revision: u64,
    latest: Value,
    saved: Value,
    error: String,
}
enum Message {
    Write(String, Value),
    Flush(mpsc::Sender<()>),
}
pub struct Service {
    initial: BTreeMap<String, Draft>,
    tx: mpsc::Sender<Message>,
    entries: Arc<Mutex<BTreeMap<String, Entry>>>,
}
pub fn start(app: Arc<canban_app_server::AppClient>) -> Result<()> {
    let rows = app.call("draft.list", json!({}))?;
    let mut initial = BTreeMap::new();
    let mut entries = BTreeMap::new();
    for row in rows.as_array().into_iter().flatten() {
        let key = row["id"].as_str().unwrap_or_default().to_owned();
        let value = row["data"].clone();
        if !value.is_null() {
            initial.insert(key.clone(), serde_json::from_value(value.clone())?);
        }
        entries.insert(
            key,
            Entry {
                revision: row["revision"].as_u64().unwrap_or(0),
                latest: value.clone(),
                saved: value,
                error: String::new(),
            },
        );
    }
    let entries = Arc::new(Mutex::new(entries));
    let (tx, rx) = mpsc::channel();
    let worker_entries = entries.clone();
    std::thread::Builder::new()
        .name("card-drafts".into())
        .spawn(move || {
            let mut pending = BTreeMap::<String, Value>::new();
            let mut last_write = Instant::now();
            loop {
                let message = rx.recv_timeout(Duration::from_millis(300));
                let mut flush = None;
                match message {
                    Ok(Message::Write(key, value)) => {
                        pending.insert(key, value);
                    }
                    Ok(Message::Flush(done)) => flush = Some(done),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                // Continuous typing is checkpointed at least once a second.
                if flush.is_none()
                    && !pending.is_empty()
                    && last_write.elapsed() < Duration::from_millis(300)
                {
                    continue;
                }
                // Drain queued text before saving, including all input before Flush.
                while let Ok(message) = rx.try_recv() {
                    match message {
                        Message::Write(key, value) => {
                            pending.insert(key, value);
                        }
                        Message::Flush(done) => {
                            if let Some(previous) = flush.replace(done) {
                                let _ = previous.send(());
                            }
                        }
                    }
                }
                for (key, value) in std::mem::take(&mut pending) {
                    let revision = worker_entries
                        .lock()
                        .unwrap()
                        .get(&key)
                        .map(|e| e.revision)
                        .unwrap_or(0);
                    let result = app
                        .call(
                            "draft.save",
                            json!({"id":key,"revision":revision,"data":value}),
                        )
                        .or_else(|error| {
                            // A transport error can follow a committed write. Recover
                            // its acknowledged revision before sending newer input.
                            if !error.to_string().contains("draft_conflict")
                                && let Ok(rows) = app.call("draft.list", json!({}))
                                && let Some(row) = rows.as_array().and_then(|rows| {
                                    rows.iter()
                                        .find(|row| row["id"] == key && row["data"] == value)
                                })
                            {
                                return Ok(row.clone());
                            }
                            Err(error)
                        });
                    let mut all = worker_entries.lock().unwrap();
                    let entry = all.entry(key.clone()).or_default();
                    match result {
                        Ok(saved) => {
                            entry.revision = saved["revision"].as_u64().unwrap_or(revision);
                            entry.saved = value;
                            entry.error.clear();
                        }
                        Err(error) => {
                            entry.error = format!("Draft not saved: {error:#}");
                        }
                    }
                }
                last_write = Instant::now();
                if let Some(done) = flush {
                    let _ = done.send(());
                }
            }
        })?;
    let _ = SERVICE.set(Service {
        initial,
        tx,
        entries,
    });
    Ok(())
}
pub fn initial() -> BTreeMap<String, Draft> {
    SERVICE.get().map(|s| s.initial.clone()).unwrap_or_default()
}
pub fn queue(key: &str, data: Value) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let mut all = service.entries.lock().unwrap();
    let entry = all.entry(key.to_owned()).or_default();
    if entry.latest == data && entry.error.is_empty() {
        return;
    }
    entry.latest = data.clone();
    entry.error.clear();
    let _ = service.tx.send(Message::Write(key.into(), data));
}
pub fn retry(key: &str) {
    let value = SERVICE.get().and_then(|service| {
        service
            .entries
            .lock()
            .unwrap()
            .get(key)
            .map(|entry| entry.latest.clone())
    });
    if let Some(value) = value {
        queue(key, value);
    }
}
pub fn status(key: &str) -> String {
    SERVICE
        .get()
        .and_then(|s| {
            s.entries.lock().unwrap().get(key).map(|e| {
                // Autosave is silent while it succeeds; only failures surface.
                e.error.clone()
            })
        })
        .unwrap_or_default()
}
pub fn flush() {
    if let Some(service) = SERVICE.get() {
        let (tx, rx) = mpsc::channel();
        let _ = service.tx.send(Message::Flush(tx));
        let _ = rx.recv_timeout(Duration::from_secs(10));
    }
}
