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
                let now = crate::cache::now();
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
            let now = crate::cache::now();
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
            && (!self.refresh || old.fetched_at > started)
        {
            return Ok(old.clone());
        }
        let fetched_at = crate::cache::now();
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
                validated_at: crate::cache::now(),
                fetched_at,
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

impl Client {
    /// Private release assets use the API octet-stream route. Never forward the
    /// GitHub credential when following its signed storage redirect.
    pub fn release_response(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
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
    #[test]
    fn refresh_started_during_in_flight_fetch_does_not_return_pre_mutation_cache() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            for (i, head) in ["old", "new"].into_iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                if i == 0 {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                let body = format!("{{\"head\":{{\"sha\":\"{head}\"}}}}");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
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
        let bg = client.clone().refreshing();
        let in_flight = std::thread::spawn(move || bg.get::<Value>("/repos/a/b/pulls/2").unwrap());
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Remote mutates after the first fetch already started; ensure clock advances past `fetched_at`.
        std::thread::sleep(Duration::from_millis(5));
        let caller = client.clone().refreshing();
        let waiter = std::thread::spawn(move || caller.get::<Value>("/repos/a/b/pulls/2").unwrap());
        std::thread::sleep(Duration::from_millis(5));
        release_tx.send(()).unwrap();
        assert_eq!(in_flight.join().unwrap()["head"]["sha"], "old");
        assert_eq!(waiter.join().unwrap()["head"]["sha"], "new");
        worker.join().unwrap();
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
}
