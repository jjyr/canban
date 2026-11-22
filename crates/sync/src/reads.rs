//! Bounded per-host LRU read cache stored on disk under `<host-cache>/reads/`.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const DEFAULT_READ_CACHE_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadCacheEntry {
    pub key: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_revision: Option<u64>,
    pub bytes: u64,
    pub last_accessed_ms: u64,
    #[serde(default)]
    pub seq: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ReadCacheIndex {
    #[serde(default)]
    next_seq: u64,
    #[serde(default)]
    entries: BTreeMap<String, ReadCacheEntry>,
}

#[derive(Debug)]
pub struct ReadCache {
    dir: PathBuf,
    budget_bytes: u64,
    index: ReadCacheIndex,
}

fn canonicalize_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, val)| (k.clone(), canonicalize_value(val)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect();
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_value).collect()),
        other => other.clone(),
    }
}

/// Compute a stable hex digest for `(method, params)` ignoring routing/idempotency metadata.
pub fn canonical_key(method: &str, params: &Value) -> String {
    let mut clean = params.clone();
    if let Some(obj) = clean.as_object_mut() {
        obj.remove("host_id");
        obj.remove("request_key");
    }
    let payload = serde_json::json!([method, canonicalize_value(&clean)]);
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    canban_context::digest(&bytes)
}

impl ReadCache {
    pub fn open(dir: PathBuf, budget_bytes: u64) -> Self {
        let index_path = dir.join("index.json");
        let index = std::fs::read(&index_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<ReadCacheIndex>(&b).ok())
            .unwrap_or_default();
        Self {
            dir,
            budget_bytes,
            index,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn entry_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    fn save_index(&self) -> Result<()> {
        canban_context::atomic(&self.index_path(), &serde_json::to_vec(&self.index)?)
    }

    pub fn total_bytes(&self) -> u64 {
        self.index.entries.values().map(|e| e.bytes).sum()
    }

    pub fn len(&self) -> usize {
        self.index.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.entries.is_empty()
    }

    pub fn put(
        &mut self,
        method: &str,
        params: &Value,
        response: &Value,
        card_id: Option<&str>,
        card_revision: Option<u64>,
        now_ms: u64,
    ) -> Result<()> {
        let key = canonical_key(method, params);
        let bytes_vec = serde_json::to_vec(response)?;
        let size = bytes_vec.len() as u64;
        if size > self.budget_bytes {
            return Ok(());
        }
        canban_context::atomic(&self.entry_path(&key), &bytes_vec)?;
        self.index.next_seq = self.index.next_seq.saturating_add(1);
        let seq = self.index.next_seq;
        self.index.entries.insert(
            key.clone(),
            ReadCacheEntry {
                key,
                method: method.to_owned(),
                card_id: card_id.map(str::to_owned),
                card_revision,
                bytes: size,
                last_accessed_ms: now_ms,
                seq,
            },
        );
        self.evict_over_budget();
        self.save_index()
    }

    pub fn get(&mut self, method: &str, params: &Value, now_ms: u64) -> Option<Value> {
        let key = canonical_key(method, params);
        let path = self.entry_path(&key);
        let data = std::fs::read(&path).ok()?;
        let val: Value = serde_json::from_slice(&data).ok()?;
        self.index.next_seq = self.index.next_seq.saturating_add(1);
        let seq = self.index.next_seq;
        if let Some(entry) = self.index.entries.get_mut(&key) {
            entry.last_accessed_ms = now_ms;
            entry.seq = seq;
            let _ = self.save_index();
        }
        Some(val)
    }

    pub fn has_fresh_timeline(&self, card_id: &str, card_revision: u64) -> bool {
        let key = canonical_key(
            "timeline.list",
            &serde_json::json!({"card_id": card_id, "tail": true, "limit": 200, "view": "full"}),
        );
        self.index.entries.get(&key).is_some_and(|e| {
            e.card_revision == Some(card_revision) && self.entry_path(&key).exists()
        })
    }

    pub fn remove_all(&mut self) {
        self.index.entries.clear();
        if self.dir.exists() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn evict_over_budget(&mut self) {
        while self.total_bytes() > self.budget_bytes {
            let Some(oldest_key) = self
                .index
                .entries
                .values()
                .min_by_key(|e| (e.last_accessed_ms, e.seq))
                .map(|e| e.key.clone())
            else {
                break;
            };
            self.index.entries.remove(&oldest_key);
            let _ = std::fs::remove_file(self.entry_path(&oldest_key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_key_ignores_host_id_and_request_key() {
        let a = canonical_key(
            "timeline.list",
            &json!({"card_id": "c1", "tail": true, "host_id": "h1", "request_key": "r1"}),
        );
        let b = canonical_key(
            "timeline.list",
            &json!({"tail": true, "card_id": "c1", "request_key": "r2"}),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn lru_eviction_enforces_byte_budget() {
        let dir = tempfile::tempdir().unwrap();
        let entry_val = json!({"payload": "x".repeat(40)});
        let single_bytes = serde_json::to_vec(&entry_val).unwrap().len() as u64;
        let mut cache = ReadCache::open(dir.path().join("reads"), single_bytes * 2);

        cache
            .put(
                "timeline.list",
                &json!({"card_id": "c1"}),
                &entry_val,
                Some("c1"),
                Some(1),
                10,
            )
            .unwrap();
        cache
            .put(
                "timeline.list",
                &json!({"card_id": "c2"}),
                &entry_val,
                Some("c2"),
                Some(1),
                20,
            )
            .unwrap();
        assert_eq!(cache.len(), 2);

        // Touch c1 so c2 becomes the least recently used.
        assert!(
            cache
                .get("timeline.list", &json!({"card_id": "c1"}), 30)
                .is_some()
        );

        cache
            .put(
                "timeline.list",
                &json!({"card_id": "c3"}),
                &entry_val,
                Some("c3"),
                Some(1),
                40,
            )
            .unwrap();

        assert_eq!(cache.len(), 2);
        assert!(cache.total_bytes() <= single_bytes * 2);
        assert!(
            cache
                .get("timeline.list", &json!({"card_id": "c2"}), 50)
                .is_none()
        );
        assert!(
            cache
                .get("timeline.list", &json!({"card_id": "c1"}), 50)
                .is_some()
        );
        assert!(
            cache
                .get("timeline.list", &json!({"card_id": "c3"}), 50)
                .is_some()
        );
    }
}
