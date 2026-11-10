use crate::models::*;
use anyhow::{Context, Result, ensure};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{io::Read, sync::OnceLock, time::Duration};
#[derive(Clone)]
pub struct Client {
    pub(crate) token: SecretString,
    api: String,
    cache: Option<(std::sync::Arc<crate::cache::Cache>, String)>,
    refresh: bool,
    cache_generation: u64,
    cli: Option<(std::sync::Arc<crate::cli::Service>, u64, u64)>,
}
pub fn repository(value: &str) -> Result<String> {
    let parts: Vec<_> = value.trim().split('/').collect();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|s| !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "Use a GitHub repository in owner/repo form"
    );
    Ok(parts.join("/"))
}
pub fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn remote_repository(url: &str) -> Result<String> {
    let value = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = value
        .strip_prefix("git@github.com:")
        .or_else(|| value.strip_prefix("ssh://git@github.com/"))
        .or_else(|| value.strip_prefix("https://github.com/"))
        .context("Push remote must be a github.com SSH or HTTPS repository")?;
    repository(path)
}
pub(crate) fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(25)))
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into()
        })
        .clone()
}
pub(crate) fn decode<T: DeserializeOwned>(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<T> {
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(crate::HttpError(status).into());
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "GitHub response exceeds 4 MiB"
    );
    if bytes.is_empty() {
        bytes.extend_from_slice(b"null");
    }
    Ok(serde_json::from_slice(&bytes)?)
}
impl Client {
    pub fn new(token: SecretString) -> Self {
        Self {
            token,
            api: "https://api.github.com".into(),
            cache: None,
            refresh: false,
            cache_generation: 0,
            cli: None,
        }
    }
    pub fn from_cli(service: std::sync::Arc<crate::cli::Service>, validate: bool) -> Result<Self> {
        let user = if validate {
            service.identify()?
        } else {
            service
                .identity()
                .context("Check the gh connection on the GitHub page")?
        };
        let generation = service.cache.generation();
        let mut client = Self::new(SecretString::from("gh-owned-credential"));
        client.cli = Some((service, user.id, generation));
        Ok(client)
    }
    pub fn with_cache(mut self, cache: std::sync::Arc<crate::cache::Cache>, scope: String) -> Self {
        self.cache_generation = cache.generation();
        self.cache = Some((cache, scope));
        self
    }
    pub fn seed(&self, path: &str, data: Value) {
        if let Some((cli, user, generation)) = &self.cli {
            if cli.identity().is_some_and(|u| u.id == *user) {
                let value = crate::cache::Response {
                    data,
                    link: None,
                    etag: None,
                    modified: None,
                    poll_interval: None,
                    validated_at: crate::cache::now(),
                    cached: false,
                };
                let _ = cli
                    .cache
                    .put(&format!("github.com:gh:{user}:{path}"), &value, *generation);
            }
            return;
        }
        if let Some((cache, scope)) = &self.cache {
            let path_key = format!("{scope}:{path}");
            let gate = cache.gate(&path_key);
            let _guard = gate.lock().unwrap();
            if cache.get(&path_key).is_some_and(|old| {
                old.data["updated_at"]
                    .as_str()
                    .zip(data["updated_at"].as_str())
                    .is_some_and(|(old, new)| old > new)
            }) {
                return;
            }
            let value = crate::cache::Response {
                data,
                link: None,
                etag: None,
                modified: None,
                poll_interval: None,
                validated_at: crate::cache::now(),
                cached: false,
            };
            let _ = cache.put(&format!("{scope}:{path}"), &value, self.cache_generation);
        }
    }
    pub fn cached_response(&self, path: &str) -> Option<crate::cache::Response> {
        if let Some((cli, user, _)) = &self.cli {
            return cli.cache.get(&format!("github.com:gh:{user}:{path}"));
        }
        let (cache, scope) = self.cache.as_ref()?;
        cache.get(&format!("{scope}:{path}"))
    }
    pub fn seed_response(&self, path: &str, value: &crate::cache::Response) {
        if let Some((cli, user, generation)) = &self.cli {
            if cli.identity().is_some_and(|u| u.id == *user) {
                let _ = cli
                    .cache
                    .put(&format!("github.com:gh:{user}:{path}"), value, *generation);
            }
            return;
        }
        if let Some((cache, scope)) = &self.cache {
            let _ = cache.put(&format!("{scope}:{path}"), value, self.cache_generation);
        }
    }
    pub fn refreshing(mut self) -> Self {
        self.refresh = true;
        self
    }
    pub fn cached<T: DeserializeOwned>(&self, path: &str) -> Option<T> {
        serde_json::from_value(self.cached_response(path)?.data).ok()
    }
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(serde_json::from_value(self.response(path)?.data)?)
    }
    pub fn response(&self, path: &str) -> Result<crate::cache::Response> {
        if let Some((cli, user, _)) = &self.cli {
            ensure!(
                cli.identity().is_some_and(|u| u.id == *user),
                "GitHub CLI account changed"
            );
            let response = cli.response(path, self.refresh)?;
            ensure!(
                cli.identity().is_some_and(|u| u.id == *user),
                "GitHub CLI account changed"
            );
            return Ok(response);
        }
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
        let key = self
            .cache
            .as_ref()
            .map(|(_, scope)| format!("{scope}:{path}"));
        let gate = self
            .cache
            .as_ref()
            .map(|(cache, _)| cache.gate(key.as_deref().unwrap()));
        let started = crate::cache::now();
        let _guard = gate.as_ref().map(|gate| gate.lock().unwrap());
        let old = self
            .cache
            .as_ref()
            .and_then(|(cache, _)| cache.get(key.as_deref().unwrap()));
        if let Some(old) = &old
            && (!self.refresh || old.validated_at > started)
        {
            return Ok(old.clone());
        }
        let generation = self
            .cache
            .as_ref()
            .map(|_| self.cache_generation)
            .unwrap_or(0);
        let page_epoch = self
            .cache
            .as_ref()
            .map(|(cache, _)| cache.page_epoch(key.as_deref().unwrap()))
            .unwrap_or(0);
        let mut request = agent()
            .get(format!("{}{path}", self.api))
            .header(
                "Authorization",
                format!("Bearer {}", self.token.expose_secret()),
            )
            .header("User-Agent", "Canban")
            .header("Accept", "application/vnd.github+json");
        if let Some(etag) = old.as_ref().and_then(|v| v.etag.as_deref()) {
            request = request.header("If-None-Match", etag);
        } else if let Some(modified) = old.as_ref().and_then(|v| v.modified.as_deref()) {
            request = request.header("If-Modified-Since", modified);
        }
        let mut response = request.call()?;
        let status = response.status().as_u16();
        if (status == 401
            || (status == 403
                && response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    != Some("0")))
            && let Some((cache, _)) = &self.cache
        {
            let _ = cache.clear();
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let value = if status == 304 {
            let mut old = old.context("GitHub returned 304 without a cached response")?;
            old.validated_at = crate::cache::now();
            old.cached = false;
            old
        } else {
            if !(200..300).contains(&status) {
                return Err(crate::HttpError(status).into());
            }
            let link = header("link");
            let etag = header("etag");
            let modified = header("last-modified");
            let poll_interval = header("x-poll-interval").and_then(|v| v.parse().ok());
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 4 * 1024 * 1024,
                "GitHub response exceeds 4 MiB"
            );
            crate::cache::Response {
                data: serde_json::from_slice(&bytes)?,
                link,
                etag,
                modified,
                poll_interval,
                validated_at: crate::cache::now(),
                cached: false,
            }
        };
        if let Some((cache, _)) = &self.cache {
            let key = key.as_deref().unwrap();
            let epoch = if self.refresh {
                cache.refresh_pages(key, generation, page_epoch)
            } else {
                page_epoch
            };
            let _ = cache.put_request(key, &value, generation, epoch);
        }
        Ok(value)
    }
    pub fn write(&self, method: &str, path: &str, body: &Value) -> Result<Value> {
        if let Some((cli, user, _)) = &self.cli {
            ensure!(
                cli.identify()?.id == *user,
                "GitHub CLI account changed; review the action again"
            );
            return cli.write(method, path, body);
        }
        ensure!(
            path.starts_with('/')
                && !path.starts_with("//")
                && path
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .split('/')
                    .all(|s| s != ".." && s != "."),
            "Invalid GitHub API path"
        );
        let response = match method {
            "POST" => agent()
                .post(format!("{}{path}", self.api))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.expose_secret()),
                )
                .header("User-Agent", "Canban")
                .header("Accept", "application/vnd.github+json")
                .send_json(body)?,
            "PUT" => agent()
                .put(format!("{}{path}", self.api))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.expose_secret()),
                )
                .header("User-Agent", "Canban")
                .header("Accept", "application/vnd.github+json")
                .send_json(body)?,
            _ => anyhow::bail!("Unsupported GitHub write"),
        };
        decode(response)
    }
    pub fn pages<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let mut all = Vec::new();
        for page in 1..=100 {
            let join = if path.contains('?') { '&' } else { '?' };
            let values: Vec<T> = self.get(&format!("{path}{join}per_page=100&page={page}"))?;
            let done = values.len() < 100;
            all.extend(values);
            if done {
                return Ok(all);
            }
        }
        anyhow::bail!("GitHub listing exceeds 10,000 records; incomplete sync was not committed")
    }
    pub fn repo(&self, repo: &str) -> Result<Repository> {
        self.get(&format!("/repos/{}", repository(repo)?))
    }
    pub fn collaborators(&self, repo: &str) -> Result<Vec<User>> {
        self.pages(&format!(
            "/repos/{}/collaborators?affiliation=all",
            repository(repo)?
        ))
    }
    pub fn item(&self, repo: &str, number: u64) -> Result<Issue> {
        self.get(&format!("/repos/{}/issues/{number}", repository(repo)?))
    }
    pub fn issue(&self, repo: &str, number: u64) -> Result<Issue> {
        let value = self.item(repo, number)?;
        ensure!(
            value.pull_request.is_none(),
            "This number is a pull request, not an Issue"
        );
        Ok(value)
    }
    pub fn issues(&self, repo: &str) -> Result<Vec<Issue>> {
        let values = self.pages::<Issue>(&format!(
            "/repos/{}/issues?state=open&sort=updated&direction=desc",
            repository(repo)?
        ))?;
        for issue in &values {
            self.seed(
                &format!("/repos/{repo}/issues/{}", issue.number),
                serde_json::to_value(issue)?,
            );
        }
        Ok(values
            .into_iter()
            .filter(|i| i.pull_request.is_none())
            .collect())
    }
    pub fn search(&self, repo: &str, text: &str) -> Result<Vec<Issue>> {
        let repo = repository(repo)?;
        let text = text.trim();
        ensure!(!text.is_empty(), "Enter a title or Issue number");
        if let Ok(n) = text.trim_start_matches('#').parse::<u64>() {
            return Ok(vec![self.issue(&repo, n)?]);
        }
        let terms = text
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace(['"', '\\'], "")))
            .collect::<Vec<_>>()
            .join(" ");
        let value: Value = self.get(&format!(
            "/search/issues?q={}&per_page=30",
            segment(&format!("repo:{repo} is:issue in:title {terms}"))
        ))?;
        Ok(serde_json::from_value(value["items"].clone())?)
    }
    pub fn comments(&self, repo: &str, number: u64) -> Result<Vec<Comment>> {
        self.pages(&format!(
            "/repos/{}/issues/{number}/comments",
            repository(repo)?
        ))
    }
    pub fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<Value> {
        self.write(
            "POST",
            &format!("/repos/{}/pulls", repository(repo)?),
            &json!({"head":head,"base":base,"title":title,"body":body}),
        )
    }
    pub fn merge_pr(&self, repo: &str, number: u64, head: &str) -> Result<Value> {
        self.write(
            "PUT",
            &format!("/repos/{}/pulls/{number}/merge", repository(repo)?),
            &json!({"sha":head,"merge_method":"merge"}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn mock(replies: Vec<(u16, String)>) -> (Client, std::thread::JoinHandle<()>) {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for (status, body) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(stream,"HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        (
            Client {
                token: SecretString::from("fixture-token"),
                api: endpoint,
                cache: None,
                refresh: false,
                cache_generation: 0,
                cli: None,
            },
            worker,
        )
    }
    #[test]
    fn dot_sequences_in_repository_names_or_queries_are_not_path_traversal() {
        let (client, worker) = mock(vec![(200, "{}".into())]);
        client.get::<Value>("/repos/owner/a..b?q=../").unwrap();
        worker.join().unwrap();
        assert!(
            client
                .get::<Value>("/repos/../private")
                .unwrap_err()
                .to_string()
                .contains("Invalid GitHub API path")
        );
    }
    #[test]
    fn incomplete_pagination_never_returns_partial_membership() {
        let page = serde_json::to_string(
            &(0..100)
                .map(|id| serde_json::json!({"id":id,"login":format!("u{id}")}))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let (client, worker) = mock(vec![
            (200, page),
            (403, "{\"message\":\"fixture-private-diagnostic\"}".into()),
        ]);
        let result = client.collaborators("owner/repo");
        assert!(result.is_err());
        assert!(!result.unwrap_err().to_string().contains("fixture-private"));
        worker.join().unwrap();
    }
    #[test]
    fn http_errors_do_not_expose_remote_bodies_or_credentials() {
        let (client, worker) = mock(vec![(
            429,
            "{\"access_token\":\"fixture-sensitive-value\"}".into(),
        )]);
        let error = client.get::<Value>("/user").unwrap_err().to_string();
        assert!(error.contains("429"));
        assert!(!error.contains("sensitive"));
        assert!(!error.contains("fixture-token"));
        worker.join().unwrap();
    }
    #[test]
    fn rejects_repository_query_injection() {
        for bad in ["a/b?x=y", "a/../b", "../b", "a/b/c", "a/ b"] {
            assert!(repository(bad).is_err(), "{bad}");
        }
        assert_eq!(repository("jjyr/canban").unwrap(), "jjyr/canban");
    }
    #[test]
    fn escapes_search_syntax() {
        assert_eq!(segment("repo:a/b x&y"), "repo%3Aa%2Fb%20x%26y");
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
    };
    #[test]
    fn explicit_refresh_uses_etag_and_cached_reads_do_not_issue_http() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            for status in [200, 304] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                tx.send(request).unwrap();
                let body = if status == 200 {
                    "{\"title\":\"initial\"}"
                } else {
                    ""
                };
                write!(stream,"HTTP/1.1 {status} OK\r\nETag: \"v1\"\r\nLink: <https://api.github.com/items?page=2>; rel=\"next\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let cache = crate::cache::Cache::new(dir.path().into());
        let client = Client {
            token: SecretString::from("fixture"),
            api,
            cache: None,
            refresh: false,
            cache_generation: 0,
            cli: None,
        }
        .with_cache(cache.clone(), "account-a".into());
        assert_eq!(
            client.get::<Value>("/items?page=1").unwrap()["title"],
            "initial"
        );
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..10 {
            assert!(client.response("/items?page=1").unwrap().cached);
        }
        assert!(rx.try_recv().is_err());
        let refreshed = client
            .clone()
            .refreshing()
            .response("/items?page=1")
            .unwrap();
        assert_eq!(refreshed.data["title"], "initial");
        assert!(refreshed.link.unwrap().contains("next"));
        assert!(
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .to_lowercase()
                .contains("if-none-match: \"v1\"")
        );
        worker.join().unwrap();
        let restarted = Client {
            token: SecretString::from("fixture"),
            api: "http://127.0.0.1:1".into(),
            cache: None,
            refresh: false,
            cache_generation: 0,
            cli: None,
        }
        .with_cache(
            crate::cache::Cache::new(dir.path().into()),
            "account-a".into(),
        );
        assert_eq!(
            restarted.get::<Value>("/items?page=1").unwrap()["title"],
            "initial"
        );
        assert!(restarted.cached::<Value>("/items?page=2").is_none());
    }
    #[test]
    fn simultaneous_cache_misses_share_one_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            count.fetch_add(1, Ordering::SeqCst);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]"
            )
            .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let client = Client {
            token: SecretString::from("fixture"),
            api,
            cache: None,
            refresh: false,
            cache_generation: 0,
            cli: None,
        }
        .with_cache(crate::cache::Cache::new(dir.path().into()), "a".into());
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let c = client.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    c.get::<Value>("/items").unwrap()
                })
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), json!([]));
        }
        worker.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
