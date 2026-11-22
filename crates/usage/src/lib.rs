//! Local subscription quota readings. Never returns or persists credentials.
use canban_types::Config;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const PROBE: &str = include_str!("probe.py");

#[derive(Default)]
struct Entry {
    readings: Vec<Value>,
    error: Option<String>,
    busy: bool,
    access_blocked: bool,
    updated: Option<u64>,
    due: Option<Instant>,
    requested: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct Service {
    entry: Arc<Mutex<Entry>>,
}

impl Service {
    pub fn request(&self, config: Config, refresh: bool) -> Value {
        let now = Instant::now();
        let mut e = self.entry.lock().unwrap();
        let due = e.due.is_none_or(|t| now >= t);
        let spaced = e
            .requested
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(10));
        if !e.busy && spaced && (due || refresh) && (!e.access_blocked || refresh) {
            e.busy = true;
            e.requested = Some(now);
            let shared = self.entry.clone();
            std::thread::spawn(move || {
                let result = canban_herdr::host::Host::new(config).python(PROBE, &[]);
                let mut e = shared.lock().unwrap();
                e.busy = false;
                match result {
                    Ok(v) if v.is_array() => {
                        e.access_blocked = v.as_array().unwrap().iter().any(|r| {
                            r["error"]
                                .as_str()
                                .is_some_and(|s| s.contains("Credential access"))
                        });
                        e.readings = reconcile(&e.readings, v.as_array().unwrap());
                        e.error = None;
                        e.updated = Some(canban_types::now());
                        let rejected = e.readings.iter().any(|r| {
                            r["error"].as_str().is_some_and(|s| {
                                s.contains("401")
                                    || s.contains("403")
                                    || s.contains("429")
                                    || s.contains("Credential")
                            })
                        });
                        e.due = Some(
                            Instant::now() + Duration::from_secs(if rejected { 1800 } else { 600 }),
                        );
                    }
                    _ => {
                        e.error = Some("Usage unavailable".into());
                        for r in &mut e.readings {
                            r["stale"] = json!(true);
                        }
                        e.due = Some(Instant::now() + Duration::from_secs(300));
                    }
                }
            });
        }
        json!({"readings": e.readings, "busy": e.busy, "updated_at": e.updated, "error": e.error})
    }
}

fn reconcile(old: &[Value], incoming: &[Value]) -> Vec<Value> {
    let mut rows = Vec::new();
    for r in incoming {
        match r["auth"].as_str() {
            Some("present") => {
                let mut r = r.clone();
                if r["error"].is_string()
                    && let Some(previous) = old
                        .iter()
                        .find(|p| p["provider"] == r["provider"] && p["account"] == r["account"])
                {
                    r["windows"] = previous["windows"].clone();
                    r["stale"] = json!(true);
                }
                rows.push(r);
            }
            Some("unavailable") => {
                for previous in old.iter().filter(|p| p["provider"] == r["provider"]) {
                    let mut p = previous.clone();
                    p["stale"] = json!(true);
                    p["error"] = r["error"].clone();
                    rows.push(p);
                }
            }
            _ => {}
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logout_hides_but_errors_preserve_known_readings_and_never_expose_credentials() {
        let old = vec![
            json!({"provider":"codex","account":"a","auth":"present","windows":[{"used":42}]}),
        ];
        assert!(reconcile(&old, &[json!({"provider":"codex","auth":"none"})]).is_empty());
        let r = reconcile(
            &old,
            &[
                json!({"provider":"codex","auth":"unavailable","error":"Credential access unavailable"}),
            ],
        );
        assert_eq!(r[0]["windows"], old[0]["windows"]);
        assert_eq!(r[0]["stale"], true);
        assert!(reconcile(&[], &[json!({"provider":"claude","auth":"unavailable"})]).is_empty());
        let r = reconcile(
            &old,
            &[
                json!({"provider":"codex","account":"a","auth":"present","windows":[],"error":"HTTP 429"}),
            ],
        );
        assert_eq!(r[0]["windows"], old[0]["windows"]);
    }
}
