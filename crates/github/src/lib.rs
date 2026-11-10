//! GitHub protocol and credentials. No Card, database, UI or execution policy.
pub mod auth;
pub mod cache;
pub mod cli;
mod client;
mod credentials;
pub mod models;
pub use client::{Client, remote_repository, repository, segment};

/// Canonical GitHub references; parsing never creates a Card or performs network IO.
pub fn parse_reference(url: &str) -> anyhow::Result<canban_types::GitHubReference> {
    let url = url.trim();
    let path = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| anyhow::anyhow!("Paste a github.com Issue, PR or commit URL"))?;
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let parts: Vec<_> = path.split('/').collect();
    anyhow::ensure!(parts.len() >= 4, "Incomplete GitHub URL");
    let repository = repository(&format!("{}/{}", parts[0], parts[1]))?;
    let (kind, number, sha) = match parts[2] {
        "issues" => ("issue", Some(parts[3].parse::<u64>()?), None),
        "pull" => ("pr", Some(parts[3].parse::<u64>()?), None),
        "commit" => {
            anyhow::ensure!(
                (7..=40).contains(&parts[3].len())
                    && parts[3].bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid commit SHA"
            );
            ("commit", None, Some(parts[3].to_lowercase()))
        }
        _ => anyhow::bail!("Use an Issue, PR or commit URL"),
    };
    anyhow::ensure!(number.is_none_or(|n| n > 0), "Invalid Issue/PR number");
    Ok(canban_types::GitHubReference {
        repository: repository.clone(),
        kind: kind.into(),
        number,
        sha: sha.clone(),
        url: format!(
            "https://github.com/{repository}/{}/{}",
            parts[2],
            number
                .map(|n| n.to_string())
                .unwrap_or_else(|| sha.unwrap())
        ),
    })
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    #[test]
    fn canonical_references_reject_other_hosts_and_invalid_identities() {
        assert_eq!(
            parse_reference("https://github.com/a/b/pull/42/files#diff-abc")
                .unwrap()
                .url,
            "https://github.com/a/b/pull/42"
        );
        assert_eq!(
            parse_reference("https://github.com/a/b/commit/ABCDEF1")
                .unwrap()
                .sha
                .as_deref(),
            Some("abcdef1")
        );
        for bad in [
            "https://github.com.evil/a/b/issues/1",
            "https://github.com/a/b/issues/0",
            "https://github.com/a/b/commit/nope",
            "https://github.com/a/../issues/1",
        ] {
            assert!(parse_reference(bad).is_err(), "{bad}");
        }
    }
}

#[derive(Debug)]
pub struct HttpError(pub u16);
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GitHub HTTP {}; check sign-in, repository installation, permissions or rate limits",
            self.0
        )
    }
}
impl std::error::Error for HttpError {}
