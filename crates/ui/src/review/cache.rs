//! Window-owned Git read cache. RPC and patch preparation run on the blocking
//! pool; identical concurrent reads share a result. Events/manual refresh clear
//! the generation so an older request cannot repopulate invalidated entries.
use super::model::{DiffLine, parse_unified_hunk_lines};
use crate::cache::LruCache;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

pub type PreparedLines = BTreeMap<String, Arc<Vec<DiffLine>>>;

#[derive(Clone)]
pub struct ReadValue {
    pub data: Value,
    pub lines: Arc<PreparedLines>,
}

impl ReadValue {
    pub fn new(data: Value) -> Self {
        let lines = data["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some((
                    f["filename"].as_str()?.to_owned(),
                    Arc::new(parse_unified_hunk_lines(f["patch"].as_str().unwrap_or(""))),
                ))
            })
            .collect();
        Self {
            data,
            lines: Arc::new(lines),
        }
    }
}

type ReadResult = Result<Arc<ReadValue>, String>;
type Flight = Arc<OnceLock<ReadResult>>;
struct Inner {
    generation: u64,
    entries: LruCache<String, (Instant, Arc<ReadValue>)>,
    inflight: BTreeMap<String, Flight>,
}

pub struct ReadCache {
    ttl: Duration,
    inner: Mutex<Inner>,
}

impl Default for ReadCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

impl ReadCache {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(Inner {
                generation: 0,
                entries: LruCache::new(24),
                inflight: BTreeMap::new(),
            }),
        }
    }

    pub fn invalidate(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        inner.entries.clear();
        inner.inflight.clear();
    }

    pub fn load(&self, key: String, fetch: impl FnOnce() -> Result<Value, String>) -> ReadResult {
        let (generation, flight, owner) = {
            let mut inner = self.inner.lock().unwrap();
            if let Some((at, value)) = inner.entries.get(&key)
                && at.elapsed() < self.ttl
            {
                return Ok(value.clone());
            }
            let generation = inner.generation;
            match inner.inflight.get(&key) {
                Some(flight) => (generation, flight.clone(), false),
                None => {
                    let flight = Arc::new(OnceLock::new());
                    inner.inflight.insert(key.clone(), flight.clone());
                    (generation, flight, true)
                }
            }
        };
        if !owner {
            return flight.wait().clone();
        }
        let result = fetch().map(|v| Arc::new(ReadValue::new(v)));
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.generation == generation {
                inner.inflight.remove(&key);
                if let Ok(value) = &result {
                    inner.entries.insert(key, (Instant::now(), value.clone()));
                }
            }
        }
        let _ = flight.set(result.clone());
        result
    }
}

pub fn key(host: &str, project: &str, method: &str, params: &Value) -> String {
    // Refresh bypasses the cache by invalidating its generation; it is not a
    // different comparison and must preserve selected file/scroll state.
    let mut identity = params.clone();
    if let Some(object) = identity.as_object_mut() {
        object.remove("refresh");
    }
    serde_json::json!([host, project, method, identity]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_are_reused_but_checkout_range_host_and_expiry_are_distinct() {
        let cache = ReadCache::default();
        let a = key(
            "local",
            "p",
            "git.compare",
            &json!({"cwd":"a", "base":"main", "compare":"topic"}),
        );
        cache.load(a.clone(), || Ok(json!({"files":[]}))).unwrap();
        cache
            .load(a.clone(), || panic!("cached read must not run RPC"))
            .unwrap();
        for b in [
            key(
                "remote",
                "p",
                "git.compare",
                &json!({"cwd":"a", "base":"main", "compare":"topic"}),
            ),
            key(
                "local",
                "p",
                "git.compare",
                &json!({"cwd":"b", "base":"main", "compare":"topic"}),
            ),
            key(
                "local",
                "p",
                "git.compare",
                &json!({"cwd":"a", "base":"HEAD", "compare":"WORKDIR"}),
            ),
        ] {
            assert_ne!(a, b);
            assert_eq!(
                cache.load(b, || Ok(json!({"new":true}))).unwrap().data["new"],
                true
            );
        }
        let expired = ReadCache::new(Duration::ZERO);
        expired.load(a.clone(), || Ok(json!(1))).unwrap();
        assert_eq!(expired.load(a, || Ok(json!(2))).unwrap().data, 2);
    }

    #[test]
    fn invalidation_during_request_cannot_restore_stale_cache() {
        let cache = ReadCache::default();
        cache
            .load("a".into(), || {
                cache.invalidate();
                Ok(json!(1))
            })
            .unwrap();
        assert_eq!(cache.load("a".into(), || Ok(json!(2))).unwrap().data, 2);
        assert!(
            cache
                .load("error".into(), || Err("offline".into()))
                .is_err()
        );
        assert_eq!(cache.load("error".into(), || Ok(json!(3))).unwrap().data, 3);
    }

    #[test]
    fn concurrent_reads_share_rpc_and_prepared_patch() {
        let cache = Arc::new(ReadCache::default());
        let (started, rx) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let c = cache.clone();
        let first = std::thread::spawn(move || {
            c.load("a".into(), || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                Ok(json!({"files":[{"filename":"a", "patch":"@@ -1 +1 @@\n-old\n+new"}]}))
            })
            .unwrap()
        });
        rx.recv().unwrap();
        let c = cache.clone();
        let second =
            std::thread::spawn(move || c.load("a".into(), || panic!("duplicate RPC")).unwrap());
        release.send(()).unwrap();
        let one = first.join().unwrap();
        let two = second.join().unwrap();
        assert!(Arc::ptr_eq(&one, &two));
        assert_eq!(one.lines["a"][2].new_line, Some(1));
    }
}
