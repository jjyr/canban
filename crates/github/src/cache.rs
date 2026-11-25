//! Disposable response cache. Freshness is driven by explicit refresh events, never a TTL.
use anyhow::Result;
use canban_types::{Clock, system_clock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

const MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const DISK_LIMIT: u64 = 256 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub data: Value,
    pub link: Option<String>,
    pub etag: Option<String>,
    pub modified: Option<String>,
    pub poll_interval: Option<u64>,
    pub validated_at: u64,
    #[serde(default)]
    pub fetched_at: u64,
    #[serde(default)]
    pub cached: bool,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    response: Response,
    #[serde(default)]
    stored_at: u64,
}
impl Entry {
    fn effective_stored_at(&self) -> u64 {
        if self.stored_at > 0 {
            self.stored_at
        } else {
            self.response.validated_at.max(self.response.fetched_at)
        }
    }
}
#[derive(Default)]
struct State {
    entries: BTreeMap<String, (Response, usize, u64)>,
    gates: BTreeMap<String, Weak<Mutex<()>>>,
    tick: u64,
    page_epochs: BTreeMap<String, u64>,
    disk_stored_at: BTreeMap<PathBuf, u64>,
}
pub struct Cache {
    root: PathBuf,
    state: Mutex<State>,
    generation: AtomicU64,
    clock: Arc<dyn Clock>,
    disk_limit: u64,
}
impl Cache {
    pub fn new(root: PathBuf) -> Arc<Self> {
        Self::new_with_clock(root, system_clock())
    }
    pub fn new_with_clock(root: PathBuf, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            root,
            state: Mutex::new(State::default()),
            generation: AtomicU64::new(0),
            clock,
            disk_limit: DISK_LIMIT,
        })
    }
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }
    fn page_family(key: &str) -> Option<(String, bool)> {
        let start = key.rfind("&page=")? + 6;
        let end = key[start..]
            .find('&')
            .map(|i| start + i)
            .unwrap_or(key.len());
        let page = key[start..end].parse::<u64>().ok()?;
        Some((format!("{}{}", &key[..start], &key[end..]), page == 1))
    }
    fn path(&self, key: &str) -> PathBuf {
        let prefix = Self::page_family(key)
            .map(|(family, _)| format!("{:x}.", Sha256::digest(family.as_bytes())))
            .unwrap_or_default();
        self.root
            .join(format!("{prefix}{:x}.json", Sha256::digest(key.as_bytes())))
    }
    pub fn page_epoch(&self, key: &str) -> u64 {
        Self::page_family(key)
            .and_then(|(family, _)| self.state.lock().unwrap().page_epochs.get(&family).copied())
            .unwrap_or(0)
    }
    /// A refreshed first page invalidates old membership/order on its later pages.
    /// Prefixes avoid reading every cached diff body to discover sibling pages.
    pub fn refresh_pages(&self, key: &str, generation: u64, request_epoch: u64) -> u64 {
        let Some((family, true)) = Self::page_family(key) else {
            // Later-page refreshes must retain their start epoch, even when a
            // first-page refresh completed while their HTTP request was pending.
            return request_epoch;
        };
        let mut state = self.state.lock().unwrap();
        if generation != self.generation() {
            return 0;
        }
        let epoch = state.page_epochs.entry(family.clone()).or_default();
        *epoch += 1;
        let epoch = *epoch;
        state
            .entries
            .retain(|k, _| k == key || Self::page_family(k).is_none_or(|(f, _)| f != family));
        let prefix = format!("{:x}.", Sha256::digest(family.as_bytes()));
        let current = self.path(key);
        for (path, _) in self.files() {
            if path != current
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
            {
                state.disk_stored_at.remove(&path);
                let _ = std::fs::remove_file(path);
            }
        }
        epoch
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub fn gate(&self, key: &str) -> Arc<Mutex<()>> {
        let mut state = self.state.lock().unwrap();
        if let Some(gate) = state.gates.get(key).and_then(Weak::upgrade) {
            return gate;
        }
        state.gates.retain(|_, v| v.strong_count() > 0);
        let gate = Arc::new(Mutex::new(()));
        state.gates.insert(key.into(), Arc::downgrade(&gate));
        gate
    }
    pub fn get(&self, key: &str) -> Option<Response> {
        let mut state = self.state.lock().unwrap();
        state.tick += 1;
        let tick = state.tick;
        if let Some((value, _, used)) = state.entries.get_mut(key) {
            *used = tick;
            let mut v = value.clone();
            v.cached = true;
            return Some(v);
        }
        let path = self.path(key);
        if std::fs::metadata(&path).ok()?.len() > 5 * 1024 * 1024 {
            state.disk_stored_at.remove(&path);
            let _ = std::fs::remove_file(&path);
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let entry: Entry = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                state.disk_stored_at.remove(&path);
                let _ = std::fs::remove_file(path);
                return None;
            }
        };
        if entry.key != key {
            return None;
        }
        state
            .disk_stored_at
            .insert(path, entry.effective_stored_at());
        Self::remember(&mut state, key, entry.response.clone(), bytes.len());
        let mut v = entry.response;
        v.cached = true;
        Some(v)
    }
    fn remember(state: &mut State, key: &str, value: Response, bytes: usize) {
        state.tick += 1;
        let tick = state.tick;
        state.entries.insert(key.into(), (value, bytes, tick));
        while state.entries.values().map(|(_, n, _)| n).sum::<usize>() > MEMORY_LIMIT {
            let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used))| used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            state.entries.remove(&oldest);
        }
    }
    pub fn put(&self, key: &str, response: &Response, generation: u64) -> Result<()> {
        self.put_checked(key, response, generation, None)
    }
    pub fn put_request(
        &self,
        key: &str,
        response: &Response,
        generation: u64,
        page_epoch: u64,
    ) -> Result<()> {
        self.put_checked(key, response, generation, Some(page_epoch))
    }
    fn put_checked(
        &self,
        key: &str,
        response: &Response,
        generation: u64,
        page_epoch: Option<u64>,
    ) -> Result<()> {
        // Clearing the cache fences in-flight writers as well as readers.
        let mut state_guard = self.state.lock().unwrap();
        if generation != self.generation()
            || page_epoch.is_some_and(|expected| {
                Self::page_family(key).is_some_and(|(family, _)| {
                    state_guard.page_epochs.get(&family).copied().unwrap_or(0) != expected
                })
            })
        {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
        }
        let stored_at = self.clock.now_ms();
        let bytes = serde_json::to_vec(&Entry {
            key: key.into(),
            response: response.clone(),
            stored_at,
        })?;
        let path = self.path(key);
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.persist(&path)?;
        state_guard.disk_stored_at.insert(path, stored_at);
        Self::remember(&mut state_guard, key, response.clone(), bytes.len());
        self.trim(&mut state_guard)?;
        drop(state_guard);
        Ok(())
    }
    fn stored_at_from_disk(path: &Path) -> u64 {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
            .map(|entry| entry.effective_stored_at())
            .unwrap_or(0)
    }
    fn trim(&self, state: &mut State) -> Result<()> {
        let files = self.files();
        let mut bytes: u64 = files.iter().map(|(_, size)| size).sum();
        if bytes <= self.disk_limit {
            return Ok(());
        }
        let mut ranked: Vec<(u64, PathBuf, u64)> = files
            .into_iter()
            .map(|(path, size)| {
                let ts = *state
                    .disk_stored_at
                    .entry(path.clone())
                    .or_insert_with(|| Self::stored_at_from_disk(&path));
                (ts, path, size)
            })
            .collect();
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_, path, size) in ranked {
            if bytes <= self.disk_limit {
                break;
            }
            std::fs::remove_file(&path)?;
            state.disk_stored_at.remove(&path);
            bytes = bytes.saturating_sub(size);
        }
        Ok(())
    }
    /// Lists cached `.json` files in deterministic lexicographical path order.
    fn files(&self) -> Vec<(PathBuf, u64)> {
        let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension()?.to_str()? != "json" {
                    return None;
                }
                let m = entry.metadata().ok()?;
                Some((path, m.len()))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }
    pub fn patch(&self, mut update: impl FnMut(&str, &mut Response)) -> Result<()> {
        let generation = {
            let _guard = self.state.lock().unwrap();
            self.generation.fetch_add(1, Ordering::SeqCst) + 1
        };
        let mut keys: Vec<String> = self
            .files()
            .into_iter()
            .filter_map(|(path, _)| {
                serde_json::from_slice::<Entry>(&std::fs::read(path).ok()?)
                    .ok()
                    .map(|e| e.key)
            })
            .collect();
        keys.sort();
        for key in keys {
            let gate = self.gate(&key);
            let _guard = gate.lock().unwrap();
            if let Some(mut value) = self.get(&key) {
                update(&key, &mut value);
                self.put(&key, &value, generation)?;
            }
        }
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        self.files().iter().map(|(_, n)| n).sum()
    }
    pub fn clear(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        self.generation.fetch_add(1, Ordering::SeqCst);
        state.entries.clear();
        state.page_epochs.clear();
        state.disk_stored_at.clear();
        for (path, _) in self.files() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn value() -> Response {
        Response {
            data: serde_json::json!({"title":"cached"}),
            link: None,
            etag: Some("v1".into()),
            modified: None,
            poll_interval: None,
            validated_at: 1,
            fetched_at: 1,
            cached: false,
        }
    }
    #[test]
    fn restart_no_ttl_account_isolation_corruption_and_clear_fence() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::new(dir.path().into());
        let generation = c.generation();
        c.put("account-a:/issues?page=1", &value(), generation)
            .unwrap();
        let c = Cache::new(dir.path().into());
        assert_eq!(c.get("account-a:/issues?page=1").unwrap().validated_at, 1);
        assert!(c.get("account-b:/issues?page=1").is_none());
        assert!(c.get("account-a:/issues?page=2").is_none());
        c.clear().unwrap();
        c.put("late", &value(), generation).unwrap();
        assert!(c.get("late").is_none());
        std::fs::write(c.path("broken"), "partial").unwrap();
        assert!(c.get("broken").is_none());
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;
    #[test]
    fn refreshed_first_page_invalidates_siblings_and_fences_late_page_writes() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::new(dir.path().into());
        let first = "account:/items?sort=updated&page=1";
        let second = "account:/items?sort=updated&page=2";
        let other = "account:/items?sort=created&page=2";
        let generation = c.generation();
        let epoch = c.page_epoch(second);
        let response = Response {
            data: serde_json::json!([1]),
            link: None,
            etag: None,
            modified: None,
            poll_interval: None,
            validated_at: 1,
            fetched_at: 1,
            cached: false,
        };
        for key in [first, second, other] {
            c.put(key, &response, generation).unwrap();
        }
        c.refresh_pages(first, generation, epoch);
        assert!(c.get(first).is_some());
        assert!(c.get(second).is_none());
        assert!(c.get(other).is_some());
        c.put_request(second, &response, generation, epoch).unwrap();
        assert!(c.get(second).is_none());
        let refreshed_epoch = c.refresh_pages(second, generation, epoch);
        c.put_request(second, &response, generation, refreshed_epoch)
            .unwrap();
        assert!(c.get(second).is_none());
        let restarted = Cache::new(dir.path().into());
        assert!(restarted.get(second).is_none());
        assert!(restarted.get(other).is_some());
    }

    struct MockClock(AtomicU64);
    impl Clock for MockClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn mock_clock_drives_deterministic_trim_and_sorted_patch_order() {
        const T0: u64 = 1_700_000_000_000;
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(MockClock(AtomicU64::new(T0)));

        let make_resp = |tag: &str, ts: u64| Response {
            data: serde_json::json!({"tag": tag, "pad": "x".repeat(64)}),
            link: None,
            etag: None,
            modified: None,
            poll_interval: None,
            validated_at: ts,
            fetched_at: ts,
            cached: false,
        };

        // Measure one serialized entry on disk, then set `disk_limit` to fit exactly two entries.
        let probe = Cache::new_with_clock(dir.path().into(), clock.clone());
        probe
            .put("acct:/k_old", &make_resp("old", T0), probe.generation())
            .unwrap();
        let one_entry_bytes = probe.bytes();
        probe.clear().unwrap();

        let cache = Arc::new(Cache {
            root: dir.path().into(),
            state: Mutex::new(State::default()),
            generation: AtomicU64::new(0),
            clock: clock.clone(),
            disk_limit: one_entry_bytes * 2 + 16,
        });
        let generation = cache.generation();

        // Write `k_old` at T0, `k_mid` at T0 + 100, then `k_new` at T0 + 200 to trigger trim.
        cache
            .put("acct:/k_old", &make_resp("old", T0), generation)
            .unwrap();
        clock.0.store(T0 + 100, Ordering::SeqCst);
        cache
            .put("acct:/k_mid", &make_resp("mid", T0 + 100), generation)
            .unwrap();
        clock.0.store(T0 + 200, Ordering::SeqCst);
        cache
            .put("acct:/k_new", &make_resp("new", T0 + 200), generation)
            .unwrap();

        // Reload from disk with a fresh Cache instance to verify persisted `stored_at` eviction.
        let reloaded = Cache::new_with_clock(dir.path().into(), clock.clone());
        assert!(reloaded.get("acct:/k_old").is_none());
        assert!(reloaded.get("acct:/k_mid").is_some());
        assert!(reloaded.get("acct:/k_new").is_some());

        // `patch` visits keys in deterministic sorted order.
        let mut visited = Vec::new();
        reloaded
            .patch(|key, resp| {
                visited.push(key.to_owned());
                resp.validated_at = T0 + 999;
            })
            .unwrap();
        assert_eq!(visited, vec!["acct:/k_mid", "acct:/k_new"]);
        assert_eq!(reloaded.get("acct:/k_mid").unwrap().validated_at, T0 + 999);
    }
}
