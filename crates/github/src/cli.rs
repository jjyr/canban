//! GitHub CLI transport. Credentials stay in gh; Canban never extracts a token.
use crate::{
    cache::{Cache, Response, now},
    models::User,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    io::{Read, Seek},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub struct Service {
    pub cache: Arc<Cache>,
    root: PathBuf,
    user: Mutex<Option<User>>,
}
impl Service {
    pub fn new(root: PathBuf) -> Self {
        let user = std::fs::read(root.join("identity.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        Self {
            cache: Cache::new(root.join("responses")),
            root,
            user: Mutex::new(user),
        }
    }
    pub fn identity(&self) -> Option<User> {
        self.user.lock().unwrap().clone()
    }
    pub fn write(&self, method: &str, path: &str, body: &Value) -> Result<Value> {
        let payload = if method == "DELETE" { None } else { Some(body) };
        Ok(request_with_body(path, None, method, payload)?.data)
    }
    pub fn login(&self) -> Option<String> {
        self.user.lock().unwrap().as_ref().map(|u| u.login.clone())
    }
    pub fn identify(&self) -> Result<User> {
        let value = request("/user", None)?;
        let user: User = serde_json::from_value(value.data)?;
        let mut current = self.user.lock().unwrap();
        if current.as_ref().is_some_and(|u| u.id != user.id) {
            self.cache.clear()?;
        }
        std::fs::create_dir_all(&self.root)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&user)?)?;
        file.persist(self.root.join("identity.json"))?;
        *current = Some(user.clone());
        Ok(user)
    }
    pub fn response(&self, path: &str, refresh: bool) -> Result<Response> {
        let user = self.user.lock().unwrap().clone();
        let user = match user {
            Some(u) => u,
            None => self.identify()?,
        };
        let key = format!("github.com:gh:{}:{path}", user.id);
        let gate = self.cache.gate(&key);
        let start = now();
        let _guard = gate.lock().unwrap();
        let old = self.cache.get(&key);
        if let Some(old) = &old
            && (!refresh
                || old.fetched_at > start
                || (path.starts_with("/notifications?")
                    && old.poll_interval.is_some_and(|interval| {
                        now()
                            < old
                                .validated_at
                                .saturating_add(interval.saturating_mul(1000))
                    })))
        {
            return Ok(old.clone());
        }
        let generation = self.cache.generation();
        let page_epoch = self.cache.page_epoch(&key);
        let live = self.identify()?;
        ensure!(
            live.id == user.id,
            "GitHub CLI account changed; refresh the page"
        );
        let response = request(path, old.as_ref());
        let response = response?;
        ensure!(
            self.user
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|u| u.id == user.id),
            "GitHub CLI account changed; discarded stale response"
        );
        let epoch = if refresh {
            self.cache.refresh_pages(&key, generation, page_epoch)
        } else {
            page_epoch
        };
        let _ = self.cache.put_request(&key, &response, generation, epoch);
        Ok(response)
    }
    pub fn cached(&self, path: &str) -> Option<Response> {
        let user = self.user.lock().unwrap().clone()?;
        self.cache.get(&format!("github.com:gh:{}:{path}", user.id))
    }
    pub fn seed(&self, path: &str, value: &Response) {
        if let Some(user) = self.user.lock().unwrap().clone() {
            let _ = self.cache.put(
                &format!("github.com:gh:{}:{path}", user.id),
                value,
                self.cache.generation(),
            );
        }
    }
    pub fn mutate(&self, method: &str, path: &str) -> Result<Response> {
        ensure!(
            matches!(method, "PATCH" | "DELETE"),
            "Unsupported notification action"
        );
        self.identify()?;
        request_method(path, None, method)
    }
    pub fn notification_changed(&self, id: &str, done: bool) -> Result<()> {
        self.cache.patch(|key, value| {
            if key.contains(":/notifications?")
                && let Some(items) = value.data.as_array_mut()
            {
                if done {
                    items.retain(|v| v["id"] != id)
                } else {
                    for v in items.iter_mut().filter(|v| v["id"] == id) {
                        v["unread"] = serde_json::json!(false);
                    }
                }
            }
        })
    }
    pub fn clear(&self) -> Result<()> {
        self.cache.clear()
    }
    pub fn binary_path() -> PathBuf {
        binary()
    }
    pub fn installed() -> bool {
        Command::new(binary())
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}
fn binary() -> PathBuf {
    std::env::var_os("CANBAN_GH_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("gh"))
        })
}
fn request(path: &str, old: Option<&Response>) -> Result<Response> {
    request_method(path, old, "GET")
}
fn request_method(path: &str, old: Option<&Response>, method: &str) -> Result<Response> {
    request_with_body(path, old, method, None)
}
fn request_with_body(
    path: &str,
    old: Option<&Response>,
    method: &str,
    body: Option<&Value>,
) -> Result<Response> {
    ensure!(
        path.starts_with('/')
            && !path.starts_with("//")
            && !path
                .split('?')
                .next()
                .unwrap_or("")
                .split('/')
                .any(|part| matches!(part, "." | ".."))
            && !path.contains('\\'),
        "Invalid GitHub API path"
    );
    let fetched_at = now();
    let mut output = tempfile::tempfile()?;
    let mut command = Command::new(binary());
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--include",
        "--method",
        method,
        path,
    ]);
    if let Some(etag) = old.and_then(|v| v.etag.as_ref()) {
        command.args(["-H", &format!("If-None-Match: {etag}")]);
    } else if let Some(modified) = old.and_then(|v| v.modified.as_ref()) {
        command.args(["-H", &format!("If-Modified-Since: {modified}")]);
    }
    if body.is_some() {
        command.args(["--input", "-"]);
    }
    let mut child = command
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(output.try_clone()?)
        .stderr(Stdio::null())
        .spawn()
        .context("GitHub CLI unavailable. Install gh and run gh auth login")?;
    if let Some(body) = body {
        std::io::Write::write_all(
            &mut child.stdin.take().context("GitHub CLI stdin unavailable")?,
            &serde_json::to_vec(body)?,
        )?;
    }
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(25) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("GitHub CLI request timed out");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    output.rewind()?;
    let mut text = String::new();
    output.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
    ensure!(
        text.len() <= 4 * 1024 * 1024,
        "GitHub response exceeds 4 MiB"
    );
    let normalized = text.replace("\r\n", "\n");
    let (headers, body) = normalized
        .split_once("\n\n")
        .context("GitHub CLI returned no HTTP response; check gh auth status")?;
    let code = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .context("Invalid GitHub CLI HTTP status")?;
    if code == 304 {
        let mut value = old.cloned().context("304 without cache")?;
        value.validated_at = now();
        value.fetched_at = fetched_at;
        value.cached = false;
        return Ok(value);
    }
    if !(200..300).contains(&code) {
        return Err(crate::HttpError(code).into());
    }
    ensure!(
        status.success(),
        "GitHub CLI failed after receiving a response; check gh auth status"
    );
    let header = |name: &str| {
        headers
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_owned())
    };
    Ok(Response {
        data: if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str::<Value>(body)?
        },
        link: header("link"),
        etag: header("etag"),
        modified: header("last-modified"),
        poll_interval: header("x-poll-interval").and_then(|v| v.parse().ok()),
        validated_at: now(),
        fetched_at,
        cached: false,
    })
}
