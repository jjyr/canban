//! Disposable response cache. Freshness is driven by explicit refresh events, never a TTL.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
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
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    response: Response,
}
#[derive(Default)]
struct State {
    entries: HashMap<String, (Response, usize, u64)>,
    gates: HashMap<String, Weak<Mutex<()>>>,
    tick: u64,
    page_epochs: HashMap<String, u64>,
}
pub struct Cache {
    root: PathBuf,
    state: Mutex<State>,
    generation: AtomicU64,
}
impl Cache {
    pub fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            state: Mutex::new(State::default()),
            generation: AtomicU64::new(0),
        })
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
        for (path, _, _) in self.files() {
            if path != current
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
            {
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
            let _ = std::fs::remove_file(&path);
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let entry: Entry = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                let _ = std::fs::remove_file(path);
                return None;
            }
        };
        if entry.key != key {
            return None;
        }
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
        let bytes = serde_json::to_vec(&Entry {
            key: key.into(),
            response: response.clone(),
        })?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.persist(self.path(key))?;
        Self::remember(&mut state_guard, key, response.clone(), bytes.len());
        self.trim()?;
        drop(state_guard);
        Ok(())
    }
    fn trim(&self) -> Result<()> {
        let mut files = self.files();
        let mut bytes: u64 = files.iter().map(|(_, size, _)| size).sum();
        files.sort_by_key(|(_, _, time)| *time);
        for (path, size, _) in files {
            if bytes <= DISK_LIMIT {
                break;
            }
            std::fs::remove_file(path)?;
            bytes = bytes.saturating_sub(size);
        }
        Ok(())
    }
    fn files(&self) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if entry.path().extension()?.to_str()? != "json" {
                    return None;
                }
                let m = entry.metadata().ok()?;
                Some((
                    entry.path(),
                    m.len(),
                    m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                ))
            })
            .collect()
    }
    pub fn patch(&self, mut update: impl FnMut(&str, &mut Response)) -> Result<()> {
        let generation = {
            let _guard = self.state.lock().unwrap();
            self.generation.fetch_add(1, Ordering::SeqCst) + 1
        };
        let keys: Vec<String> = self
            .files()
            .into_iter()
            .filter_map(|(path, _, _)| {
                serde_json::from_slice::<Entry>(&std::fs::read(path).ok()?)
                    .ok()
                    .map(|e| e.key)
            })
            .collect();
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
        self.files().iter().map(|(_, n, _)| n).sum()
    }
    pub fn clear(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        self.generation.fetch_add(1, Ordering::SeqCst);
        state.entries.clear();
        state.page_epochs.clear();
        for (path, _, _) in self.files() {
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
}
