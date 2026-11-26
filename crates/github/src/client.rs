use crate::models::*;
use anyhow::{Context, Result, ensure};
use canban_types::{Clock, system_clock};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::{Arc, OnceLock},
    time::Duration,
};

/// Minimal HTTP request passed to an injectable [`HttpTransport`] (used by DST).
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

/// Minimal HTTP response returned by an injectable [`HttpTransport`].
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Injectable GitHub HTTP transport seam for deterministic simulation tests.
pub trait HttpTransport: Send + Sync + 'static {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse>;
}

#[derive(Clone)]
pub struct Client {
    pub(crate) token: SecretString,
    api: String,
    cache: Option<(std::sync::Arc<crate::cache::Cache>, String)>,
    refresh: bool,
    cache_generation: u64,
    cli: Option<(std::sync::Arc<crate::cli::Service>, u64, u64)>,
    transport: Option<Arc<dyn HttpTransport>>,
    clock: Arc<dyn Clock>,
}
pub use crate::repository;
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
        .or_else(|| {
            let rest = value
                .strip_prefix("ssh://")
                .or_else(|| value.strip_prefix("https://"))
                .or_else(|| value.strip_prefix("http://"))
                .or_else(|| value.strip_prefix("git://"))?;
            let host_and_path = match rest.find('/') {
                Some(idx) => {
                    let authority = &rest[..idx];
                    let host = authority.rsplit('@').next().unwrap_or(authority);
                    let host = host.split(':').next().unwrap_or(host);
                    host.eq_ignore_ascii_case("github.com")
                        .then_some(&rest[idx + 1..])?
                }
                None => return None,
            };
            Some(host_and_path)
        })
        .or_else(|| {
            let (left, right) = value.split_once(':')?;
            let host = left.rsplit('@').next().unwrap_or(left);
            (!left.contains('/') && host.eq_ignore_ascii_case("github.com")).then_some(right)
        })
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
            transport: None,
            clock: system_clock(),
        }
    }
    pub fn with_transport(mut self, transport: Arc<dyn HttpTransport>) -> Self {
        self.transport = Some(transport);
        self
    }
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
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
        let clock = service.clock().clone();
        let mut client = Self::new(SecretString::from("gh-owned-credential")).with_clock(clock);
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
                let now = self.clock.now_ms();
                let value = crate::cache::Response {
                    data,
                    link: None,
                    etag: None,
                    modified: None,
                    poll_interval: None,
                    validated_at: now,
                    fetched_at: 0,
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
            let now = self.clock.now_ms();
            let preserved_fetched_at = cache
                .get(&path_key)
                .map(|old| old.fetched_at)
                .filter(|&ts| ts == u64::MAX)
                .unwrap_or(0);
            let value = crate::cache::Response {
                data,
                link: None,
                etag: None,
                modified: None,
                poll_interval: None,
                validated_at: now,
                fetched_at: preserved_fetched_at,
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
        let started_seq = self
            .cache
            .as_ref()
            .map(|(cache, _)| cache.next_fetch_seq())
            .unwrap_or(0);
        let _guard = gate.as_ref().map(|gate| gate.lock().unwrap());
        let old = self
            .cache
            .as_ref()
            .and_then(|(cache, _)| cache.get(key.as_deref().unwrap()));
        let coalesced_fresh = self
            .cache
            .as_ref()
            .is_some_and(|(cache, _)| cache.last_fetch_seq(key.as_deref().unwrap()) > started_seq);
        if let Some(old) = &old
            && (!self.refresh || old.fetched_at == u64::MAX || coalesced_fresh)
        {
            return Ok(old.clone());
        }
        let request_seq = self
            .cache
            .as_ref()
            .map(|(cache, _)| cache.next_fetch_seq())
            .unwrap_or(0);
        let fetched_at = self.clock.now_ms();
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
        let value = if let Some(transport) = &self.transport {
            let mut headers = vec![
                (
                    "Authorization".into(),
                    format!("Bearer {}", self.token.expose_secret()),
                ),
                ("User-Agent".into(), "Canban".into()),
                ("Accept".into(), "application/vnd.github+json".into()),
            ];
            if let Some(etag) = old.as_ref().and_then(|v| v.etag.as_deref()) {
                headers.push(("If-None-Match".into(), etag.to_owned()));
            } else if let Some(modified) = old.as_ref().and_then(|v| v.modified.as_deref()) {
                headers.push(("If-Modified-Since".into(), modified.to_owned()));
            }
            let response = transport.send(HttpRequest {
                method: "GET".into(),
                path: path.to_owned(),
                headers,
                body: None,
            })?;
            let status = response.status;
            if (status == 401
                || (status == 403 && response.header("x-ratelimit-remaining") != Some("0")))
                && let Some((cache, _)) = &self.cache
            {
                let _ = cache.clear();
            }
            if status == 304 {
                let mut old = old.context("GitHub returned 304 without a cached response")?;
                old.validated_at = self.clock.now_ms();
                old.fetched_at = fetched_at;
                old.cached = false;
                old
            } else {
                if !(200..300).contains(&status) {
                    return Err(crate::HttpError(status).into());
                }
                ensure!(
                    response.body.len() <= 4 * 1024 * 1024,
                    "GitHub response exceeds 4 MiB"
                );
                crate::cache::Response {
                    data: serde_json::from_slice(&response.body)?,
                    link: response.header("link").map(str::to_owned),
                    etag: response.header("etag").map(str::to_owned),
                    modified: response.header("last-modified").map(str::to_owned),
                    poll_interval: response
                        .header("x-poll-interval")
                        .and_then(|v| v.parse().ok()),
                    validated_at: self.clock.now_ms(),
                    fetched_at,
                    cached: false,
                }
            }
        } else {
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
            if status == 304 {
                let mut old = old.context("GitHub returned 304 without a cached response")?;
                old.validated_at = self.clock.now_ms();
                old.fetched_at = fetched_at;
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
                    validated_at: self.clock.now_ms(),
                    fetched_at,
                    cached: false,
                }
            }
        };
        if let Some((cache, _)) = &self.cache {
            let key = key.as_deref().unwrap();
            let epoch = if self.refresh {
                cache.refresh_pages(key, generation, page_epoch)
            } else {
                page_epoch
            };
            let _ = cache.put_request_with_seq(key, &value, generation, epoch, request_seq);
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
        if let Some(transport) = &self.transport {
            ensure!(
                matches!(method, "POST" | "PUT" | "PATCH" | "DELETE"),
                "Unsupported GitHub write"
            );
            let response = transport.send(HttpRequest {
                method: method.to_owned(),
                path: path.to_owned(),
                headers: vec![
                    (
                        "Authorization".into(),
                        format!("Bearer {}", self.token.expose_secret()),
                    ),
                    ("User-Agent".into(), "Canban".into()),
                    ("Accept".into(), "application/vnd.github+json".into()),
                ],
                body: (method != "DELETE").then(|| body.clone()),
            })?;
            if !(200..300).contains(&response.status) {
                return Err(crate::HttpError(response.status).into());
            }
            ensure!(
                response.body.len() <= 4 * 1024 * 1024,
                "GitHub response exceeds 4 MiB"
            );
            let bytes = if response.body.is_empty() {
                b"null".as_slice()
            } else {
                response.body.as_slice()
            };
            return Ok(serde_json::from_slice(bytes)?);
        }
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
            "PATCH" => agent()
                .patch(format!("{}{path}", self.api))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.expose_secret()),
                )
                .header("User-Agent", "Canban")
                .header("Accept", "application/vnd.github+json")
                .send_json(body)?,
            "DELETE" => agent()
                .delete(format!("{}{path}", self.api))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.expose_secret()),
                )
                .header("User-Agent", "Canban")
                .header("Accept", "application/vnd.github+json")
                .call()?,
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
    /// Bounded context search, including Issues and PRs; never mutates GitHub.
    pub fn references(&self, repo: &str, text: &str) -> Result<Vec<Issue>> {
        let repo = repository(repo)?;
        let text = text.trim();
        if let Ok(number) = text.trim_start_matches('#').parse::<u64>() {
            return Ok(vec![self.get(&format!("/repos/{repo}/issues/{number}"))?]);
        }
        let terms = text
            .split_whitespace()
            .take(12)
            .map(|w| format!("\"{}\"", w.replace(['"', '\\'], "")))
            .collect::<Vec<_>>()
            .join(" ");
        let value: Value = self.get(&format!(
            "/search/issues?q={}&sort=updated&order=desc&per_page=20",
            segment(&format!("repo:{repo} in:title {terms}"))
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
                transport: None,
                clock: system_clock(),
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
    fn reference_search_includes_prs_and_exact_numbers_without_importing_them_as_issues() {
        let issue = json!({"id":1,"node_id":"I1","number":12,"title":"Context issue","body":"Markdown","state":"closed","html_url":"https://github.com/owner/repo/issues/12","user":{"id":1,"login":"fixture"},"updated_at":"2026-11-10"});
        let mut pr = issue.clone();
        pr["id"] = json!(2);
        pr["number"] = json!(13);
        pr["pull_request"] = json!({});
        pr["html_url"] = json!("https://github.com/owner/repo/pull/13");
        let (client, worker) = mock(vec![
            (200, json!({"items":[issue,pr.clone()]}).to_string()),
            (200, pr.to_string()),
        ]);
        let rows = client.references("owner/repo", "context").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].pull_request.is_none());
        assert!(rows[1].pull_request.is_some());
        assert_eq!(
            client.references("owner/repo", "#13").unwrap()[0].number,
            13
        );
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

fn redact_release_download_error(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::StatusCode(code) => {
            anyhow::anyhow!("Release asset download failed: HTTP {code}")
        }
        ureq::Error::Timeout(kind) => {
            anyhow::anyhow!("Release asset download timed out ({kind})")
        }
        ureq::Error::Io(io_err) => {
            anyhow::anyhow!("Release asset download I/O error ({})", io_err.kind())
        }
        ureq::Error::Protocol(_) => {
            anyhow::anyhow!("Release asset download failed: HTTP protocol error")
        }
        ureq::Error::HostNotFound => {
            anyhow::anyhow!("Release asset download failed: host not found")
        }
        ureq::Error::ConnectionFailed => {
            anyhow::anyhow!("Release asset download failed: connection failed")
        }
        ureq::Error::TooManyRedirects => {
            anyhow::anyhow!("Release asset download failed: too many redirects")
        }
        ureq::Error::RedirectFailed => {
            anyhow::anyhow!("Release asset download failed: redirect failed")
        }
        ureq::Error::BodyStalled => {
            anyhow::anyhow!("Release asset download failed: body data reading stalled")
        }
        _ => anyhow::anyhow!("Release asset download failed"),
    }
}

pub(crate) const RELEASE_PER_READ_TIMEOUT: Duration = Duration::from_secs(30);

impl Client {
    /// Private release assets use the API octet-stream route. Never forward the
    /// GitHub credential when following its signed storage redirect.
    pub fn release_response(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        self.release_response_with_read_timeout(url, RELEASE_PER_READ_TIMEOUT)
    }

    fn release_response_with_read_timeout(
        &self,
        url: &str,
        per_read: Duration,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        ensure!(
            url.starts_with(&format!("{}/repos/", self.api)) && url.contains("/releases/assets/"),
            "Invalid GitHub release asset URL"
        );
        let response = agent()
            .get(url)
            .header("User-Agent", "Canban updater")
            .header("Accept", "application/octet-stream")
            .header(
                "Authorization",
                format!("Bearer {}", self.token.expose_secret()),
            )
            .call()?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("Location")
                .context("Release redirect is missing")?
                .to_str()?
                .to_owned();
            ensure!(
                location.starts_with("https://") || (cfg!(test) && location.starts_with(&self.api)),
                "Release download redirect requires HTTPS"
            );
            let public: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(20)))
                .timeout_send_request(Some(Duration::from_secs(20)))
                .timeout_recv_response(Some(Duration::from_secs(30)))
                .timeout_per_read(Some(per_read))
                .timeout_recv_body(Some(Duration::from_secs(20 * 60)))
                .max_redirects(5)
                .build()
                .into();
            return public
                .get(location)
                .header("User-Agent", "Canban updater")
                .call()
                .map_err(redact_release_download_error);
        }
        ensure!(
            response.status().is_success(),
            "Release asset HTTP {}; check repository installation access",
            response.status()
        );
        Ok(response)
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };
    #[test]
    fn private_asset_auth_does_not_follow_storage_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let redirect = base.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut input = BufReader::new(socket.try_clone().unwrap());
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    input.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                let authenticated = headers.to_lowercase().contains("authorization:");
                assert_eq!(authenticated, i == 0);
                let reply = if i == 0 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {redirect}/signed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture"
                        .into()
                };
                socket.write_all(reply.as_bytes()).unwrap();
            }
        });
        let mut client = Client::new(SecretString::from("fixture-only"));
        client.api = base.clone();
        let mut response = client
            .release_response(&format!("{base}/repos/test/app/releases/assets/1"))
            .unwrap();
        let mut bytes = String::new();
        response
            .body_mut()
            .as_reader()
            .read_to_string(&mut bytes)
            .unwrap();
        assert_eq!(bytes, "fixture");
        thread.join().unwrap();
    }

    #[test]
    fn release_redirect_error_surfaces_reason_without_leaking_signed_query_tokens() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let redirect = base.clone();
        let secret_sig = "X-Amz-Signature=super-secret-signed-token-998877";
        let thread = std::thread::spawn(move || {
            for i in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut input = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    input.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                let reply = if i == 0 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {redirect}/signed/asset?{secret_sig}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 18\r\nConnection: close\r\n\r\nstorage-overloaded"
                        .into()
                };
                socket.write_all(reply.as_bytes()).unwrap();
            }
        });
        let mut client = Client::new(SecretString::from("gh-bearer-secret"));
        client.api = base.clone();
        let err = client
            .release_response(&format!("{base}/repos/test/app/releases/assets/42"))
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("HTTP 503"), "unexpected error: {msg}");
        assert!(
            !msg.contains("super-secret-signed-token"),
            "leaked signed query: {msg}"
        );
        assert!(!msg.contains("X-Amz-Signature"), "leaked query key: {msg}");
        assert!(
            !msg.contains("gh-bearer-secret"),
            "leaked bearer token: {msg}"
        );
        assert!(
            !msg.contains("storage-overloaded"),
            "leaked remote body: {msg}"
        );
        thread.join().unwrap();
    }

    #[test]
    fn redirected_release_download_enforces_per_read_stall_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let redirect = base.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let mut held_socket = None;
            for i in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut input = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    input.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                if i == 0 {
                    let reply = format!(
                        "HTTP/1.1 302 Found\r\nLocation: {redirect}/signed-stall\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    socket.write_all(reply.as_bytes()).unwrap();
                } else {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\npartial")
                        .unwrap();
                    let _ = socket.flush();
                    held_socket = Some(socket);
                }
            }
            let _ = done_rx.recv();
            drop(held_socket);
        });
        let mut client = Client::new(SecretString::from("gh-bearer-secret"));
        client.api = base.clone();
        let mut response = client
            .release_response_with_read_timeout(
                &format!("{base}/repos/test/app/releases/assets/99"),
                Duration::from_millis(200),
            )
            .unwrap();
        let mut buf = Vec::new();
        let started = std::time::Instant::now();
        let err = response
            .body_mut()
            .as_reader()
            .read_to_end(&mut buf)
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "per-read timeout took too long: {:?}",
            started.elapsed()
        );
        assert!(
            err.to_string().contains("timeout") || err.kind() == std::io::ErrorKind::TimedOut,
            "unexpected stall error: {err}"
        );
        let _ = done_tx.send(());
        thread.join().unwrap();
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
            transport: None,
            clock: system_clock(),
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
            transport: None,
            clock: system_clock(),
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
            transport: None,
            clock: system_clock(),
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
    #[test]
    fn delete_write_accepts_empty_no_content_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            tx.send(request_line).unwrap();
            write!(
                stream,
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let client = Client {
            token: SecretString::from("fixture"),
            api,
            cache: None,
            refresh: false,
            cache_generation: 0,
            cli: None,
            transport: None,
            clock: system_clock(),
        };
        let value = client
            .write(
                "DELETE",
                "/repos/fixture/repo/issues/comments/42",
                &json!({}),
            )
            .unwrap();
        assert_eq!(value, Value::Null);
        assert!(
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .starts_with("DELETE /repos/fixture/repo/issues/comments/42 ")
        );
        worker.join().unwrap();
    }
    #[test]
    fn remote_repository_parses_ssh_https_and_rejects_non_github_urls() {
        for url in [
            "git@github.com:jjyr/canban.git",
            "git@github.com:jjyr/canban",
            "ssh://git@github.com/jjyr/canban.git",
            "ssh://git@github.com:22/jjyr/canban.git/",
            "https://github.com/jjyr/canban.git",
            "https://github.com/jjyr/canban/",
            "https://x-access-token:secret@github.com/jjyr/canban.git",
            "git://github.com/jjyr/canban.git",
        ] {
            assert_eq!(remote_repository(url).unwrap(), "jjyr/canban", "{url}");
        }
        for bad in [
            "/tmp/local/origin.git",
            "https://gitlab.com/jjyr/canban.git",
            "git@gitlab.com:jjyr/canban.git",
            "https://github.com/jjyr",
            "https://github.com/jjyr/canban/extra",
            "",
        ] {
            assert!(remote_repository(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn backward_clock_jump_does_not_suppress_github_refresh() {
        use std::sync::{
            Mutex,
            atomic::{AtomicU64, AtomicUsize},
        };

        struct MockClock(AtomicU64);
        impl Clock for MockClock {
            fn now_ms(&self) -> u64 {
                self.0.load(Ordering::SeqCst)
            }
        }

        struct ScriptedTransport {
            calls: AtomicUsize,
            last_if_none_match: Mutex<Option<String>>,
            next_title: Mutex<String>,
            next_etag: Mutex<String>,
        }
        impl HttpTransport for ScriptedTransport {
            fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let inm = req
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("if-none-match"))
                    .map(|(_, v)| v.clone());
                *self.last_if_none_match.lock().unwrap() = inm;
                let title = self.next_title.lock().unwrap().clone();
                let etag = self.next_etag.lock().unwrap().clone();
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![("ETag".into(), etag)],
                    body: serde_json::to_vec(&json!({"title": title}))?,
                })
            }
        }

        const T0: u64 = 1_700_000_100_000;
        let path = "/repos/o/r/issues/42";
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(MockClock(AtomicU64::new(T0)));
        let transport = Arc::new(ScriptedTransport {
            calls: AtomicUsize::new(0),
            last_if_none_match: Mutex::new(None),
            next_title: Mutex::new("v1".into()),
            next_etag: Mutex::new("\"v1\"".into()),
        });
        let cache = crate::cache::Cache::new_with_clock(dir.path().into(), clock.clone());
        let client = Client::new(SecretString::from("fixture"))
            .with_clock(clock.clone())
            .with_transport(transport.clone())
            .with_cache(cache, "acct".into());

        // 1. Initial fetch populates cache at T0.
        let r1 = client.response(path).unwrap();
        assert_eq!(r1.data["title"], "v1");
        assert_eq!(r1.fetched_at, T0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);

        // 2. Step wall clock backward by 1 hour (simulating an NTP backward jump).
        let t_back_1 = T0 - 3_600_000;
        clock.0.store(t_back_1, Ordering::SeqCst);

        // Non-refreshing read still serves from cache without HTTP.
        let cached = client.response(path).unwrap();
        assert!(cached.cached);
        assert_eq!(cached.data["title"], "v1");
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);

        // Explicit refresh MUST NOT be suppressed by old.fetched_at (T0) > started (T0 - 1h).
        *transport.next_title.lock().unwrap() = "v2".into();
        *transport.next_etag.lock().unwrap() = "\"v2\"".into();
        let r2 = client.clone().refreshing().response(path).unwrap();
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            transport.last_if_none_match.lock().unwrap().as_deref(),
            Some("\"v1\"")
        );
        assert_eq!(r2.data["title"], "v2");
        assert_eq!(r2.fetched_at, t_back_1);

        // 3. Step clock backward again and seed a summary; refresh still issues HTTP.
        let t_back_2 = T0 - 7_200_000;
        clock.0.store(t_back_2, Ordering::SeqCst);
        client.seed(path, json!({"title": "seeded-summary"}));
        *transport.next_title.lock().unwrap() = "v3".into();
        *transport.next_etag.lock().unwrap() = "\"v3\"".into();
        let r3 = client.clone().refreshing().response(path).unwrap();
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
        assert_eq!(r3.data["title"], "v3");
        assert_eq!(r3.fetched_at, t_back_2);

        // 4. Reload cache from disk with a backward clock: cached read hits disk, refresh hits HTTP.
        let reloaded_cache = crate::cache::Cache::new_with_clock(dir.path().into(), clock.clone());
        let reloaded = Client::new(SecretString::from("fixture"))
            .with_clock(clock.clone())
            .with_transport(transport.clone())
            .with_cache(reloaded_cache, "acct".into());
        assert_eq!(reloaded.response(path).unwrap().data["title"], "v3");
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
        *transport.next_title.lock().unwrap() = "v4".into();
        let r4 = reloaded.clone().refreshing().response(path).unwrap();
        assert_eq!(transport.calls.load(Ordering::SeqCst), 4);
        assert_eq!(r4.data["title"], "v4");

        // 5. Explicitly pinned `fetched_at == u64::MAX` test fixtures still short-circuit.
        let pinned_path = "/repos/o/r/issues/999";
        reloaded.seed_response(
            pinned_path,
            &crate::cache::Response {
                data: json!({"title": "pinned"}),
                link: None,
                etag: None,
                modified: None,
                poll_interval: None,
                validated_at: 1,
                fetched_at: u64::MAX,
                cached: false,
            },
        );
        let pinned = reloaded.clone().refreshing().response(pinned_path).unwrap();
        assert_eq!(pinned.data["title"], "pinned");
        assert_eq!(transport.calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn concurrent_refresh_waiters_coalesce_without_clock_advance() {
        use std::sync::{Mutex, atomic::AtomicUsize};

        struct ConstantClock(u64);
        impl Clock for ConstantClock {
            fn now_ms(&self) -> u64 {
                self.0
            }
        }

        struct GatedTransport {
            calls: AtomicUsize,
            entered_tx: Mutex<Option<std::sync::mpsc::Sender<()>>>,
            release_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        }
        impl HttpTransport for GatedTransport {
            fn send(&self, _req: HttpRequest) -> Result<HttpResponse> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    if let Some(tx) = self.entered_tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    if let Some(rx) = self.release_rx.lock().unwrap().take() {
                        let _ = rx.recv();
                    }
                }
                let body = serde_json::to_vec(&json!({"seq": n}))?;
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body,
                })
            }
        }

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ConstantClock(1_700_000_000_000));
        let transport = Arc::new(GatedTransport {
            calls: AtomicUsize::new(0),
            entered_tx: Mutex::new(Some(entered_tx)),
            release_rx: Mutex::new(Some(release_rx)),
        });
        let cache = crate::cache::Cache::new_with_clock(dir.path().into(), clock.clone());
        let client = Client::new(SecretString::from("fixture"))
            .with_clock(clock)
            .with_transport(transport.clone())
            .with_cache(cache.clone(), "acct".into());

        // Thread 1 enters and holds the per-key gate during HTTP call #1 (allocates seq 1 and 2).
        let c1 = client.clone().refreshing();
        let t1 = std::thread::spawn(move || c1.get::<Value>("/repos/o/r/pulls/7").unwrap());
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Threads 2 & 3 enter while HTTP #1 is in flight (allocate seq 3 and 4) and block on gate.
        let c2 = client.clone().refreshing();
        let t2 = std::thread::spawn(move || c2.get::<Value>("/repos/o/r/pulls/7").unwrap());
        let c3 = client.clone().refreshing();
        let t3 = std::thread::spawn(move || c3.get::<Value>("/repos/o/r/pulls/7").unwrap());
        while cache.fetch_seq_count() < 4 {
            std::thread::yield_now();
        }
        release_tx.send(()).unwrap();

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();
        let v3 = t3.join().unwrap();
        assert_eq!(v1["seq"], 1);
        // Both waiters must see HTTP #2 (never pre-mutation #1), coalescing into ONE follow-up call.
        assert_eq!(v2["seq"], 2);
        assert_eq!(v3["seq"], 2);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    }
}
