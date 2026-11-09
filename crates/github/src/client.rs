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
    ensure!(
        (200..300).contains(&status),
        "GitHub HTTP {status}; check sign-in, repository installation, permissions or rate limits"
    );
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
        }
    }
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
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
        decode(
            agent()
                .get(format!("{}{path}", self.api))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.expose_secret()),
                )
                .header("User-Agent", "Canban")
                .header("Accept", "application/vnd.github+json")
                .call()?,
        )
    }
    pub fn write(&self, method: &str, path: &str, body: &Value) -> Result<Value> {
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
    pub fn issue(&self, repo: &str, number: u64) -> Result<Issue> {
        let value: Issue = self.get(&format!("/repos/{}/issues/{number}", repository(repo)?))?;
        ensure!(
            value.pull_request.is_none(),
            "This number is a pull request, not an Issue"
        );
        Ok(value)
    }
    pub fn issues(&self, repo: &str) -> Result<Vec<Issue>> {
        Ok(self
            .pages::<Issue>(&format!(
                "/repos/{}/issues?state=open&sort=updated&direction=desc",
                repository(repo)?
            ))?
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
            },
            worker,
        )
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
