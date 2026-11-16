//! Window-owned Git read cache. RPC and patch preparation run on the blocking
//! pool; identical concurrent reads share a result. Scoped events/manual refresh detach
//! matching flights so older requests cannot repopulate invalidated entries.
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
                entries: LruCache::new(24),
                inflight: BTreeMap::new(),
            }),
        }
    }

    pub fn invalidate(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.entries.clear();
        inner.inflight.clear();
    }

    /// Detach matching flights as well as cached values. Their owners may finish,
    /// but cannot refill this scope or remove a newer flight for the same key.
    pub fn invalidate_scope(&self, host: &str, project: Option<&str>) {
        let prefix = match project {
            Some(project) => format!(
                "[{},{},",
                serde_json::json!(host),
                serde_json::json!(project)
            ),
            None => format!("[{},", serde_json::json!(host)),
        };
        let mut inner = self.inner.lock().unwrap();
        inner.entries.retain(|key, _| !key.starts_with(&prefix));
        inner.inflight.retain(|key, _| !key.starts_with(&prefix));
    }

    pub fn invalidate_checkout(&self, host: &str, project: &str, cwd: &str, card: Option<&str>) {
        let prefix = format!(
            "[{},{},",
            serde_json::json!(host),
            serde_json::json!(project)
        );
        let matches = |key: &str| {
            if !key.starts_with(&prefix) {
                return false;
            }
            let key: Value = serde_json::from_str(key).unwrap_or_default();
            if key[2] == "review.github_pr" {
                return true;
            }
            let params = &key[3];
            if params["use_card_worktree"] == true {
                card.is_some() && params["card_id"].as_str() == card
            } else {
                params["cwd"].as_str().unwrap_or("") == cwd
            }
        };
        let mut inner = self.inner.lock().unwrap();
        inner.entries.retain(|key, _| !matches(key));
        inner.inflight.retain(|key, _| !matches(key));
    }

    pub fn load(&self, key: String, fetch: impl FnOnce() -> Result<Value, String>) -> ReadResult {
        let (flight, owner) = {
            let mut inner = self.inner.lock().unwrap();
            if let Some((at, value)) = inner.entries.get(&key)
                && at.elapsed() < self.ttl
            {
                return Ok(value.clone());
            }
            match inner.inflight.get(&key) {
                Some(flight) => (flight.clone(), false),
                None => {
                    let flight = Arc::new(OnceLock::new());
                    inner.inflight.insert(key.clone(), flight.clone());
                    (flight, true)
                }
            }
        };
        if !owner {
            return flight.wait().clone();
        }
        let result = fetch().map(|v| Arc::new(ReadValue::new(v)));
        {
            let mut inner = self.inner.lock().unwrap();
            if inner
                .inflight
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &flight))
            {
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
    // Refresh bypasses the cache by invalidating its scope; it is not a
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
    fn checkout_invalidation_keeps_other_worktrees_and_matches_card_comparisons() {
        let cache = ReadCache::default();
        let here = key("local", "p", "git.refs", &json!({"cwd":"/wt/a"}));
        let card = key(
            "local",
            "p",
            "git.compare",
            &json!({"card_id":"a", "use_card_worktree":true}),
        );
        let other = key("local", "p", "git.refs", &json!({"cwd":"/wt/b"}));
        let other_card = key(
            "local",
            "p",
            "git.compare",
            &json!({"card_id":"b", "use_card_worktree":true}),
        );
        for request in [&here, &card, &other, &other_card] {
            cache.load(request.clone(), || Ok(json!("before"))).unwrap();
        }
        cache.invalidate_checkout("local", "p", "/wt/a", Some("a"));
        for request in [here, card] {
            assert_eq!(
                cache.load(request, || Ok(json!("after"))).unwrap().data,
                "after"
            );
        }
        for request in [other, other_card] {
            cache
                .load(request, || panic!("another worktree was invalidated"))
                .unwrap();
        }
    }

    #[test]
    fn scoped_invalidation_preserves_other_hosts_and_projects_including_inflight_reads() {
        let cache = ReadCache::default();
        let local = key("local", "p", "git.refs", &json!({}));
        let other_project = key("local", "prefix", "git.refs", &json!({}));
        let remote = key("devpc", "p", "git.refs", &json!({}));
        cache.load(local.clone(), || Ok(json!(1))).unwrap();
        cache.load(other_project.clone(), || Ok(json!(2))).unwrap();
        cache
            .load(remote.clone(), || {
                cache.invalidate_scope("local", Some("p"));
                Ok(json!(3))
            })
            .unwrap();
        assert_eq!(cache.load(local.clone(), || Ok(json!(4))).unwrap().data, 4);
        cache
            .load(other_project, || panic!("another Project was invalidated"))
            .unwrap();
        cache
            .load(remote.clone(), || {
                panic!("another host flight was invalidated")
            })
            .unwrap();
        cache.invalidate_scope("devpc", None);
        assert_eq!(cache.load(remote, || Ok(json!(5))).unwrap().data, 5);
        cache
            .load(local, || panic!("remote recovery invalidated local Git"))
            .unwrap();
    }

    #[test]
    fn invalidated_old_flight_cannot_replace_newer_result_in_the_same_scope() {
        let cache = ReadCache::default();
        let request = key("local", "p", "git.compare", &json!({}));
        cache
            .load(request.clone(), || {
                cache.invalidate_scope("local", Some("p"));
                cache.load(request.clone(), || Ok(json!("new"))).unwrap();
                Ok(json!("old"))
            })
            .unwrap();
        assert_eq!(
            cache
                .load(request, || panic!("new result was lost"))
                .unwrap()
                .data,
            "new"
        );
    }

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
