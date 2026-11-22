use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Desensitized metadata for an environment variable or secret stored in configuration or snapshots.
/// Never carries secret plaintext.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnvEntryMeta {
    Plain {
        value: String,
    },
    Secret {
        #[serde(default)]
        configured: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hint: String,
    },
    Unset,
}

impl EnvEntryMeta {
    pub fn is_secret(&self) -> bool {
        matches!(self, Self::Secret { .. })
    }

    pub fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }

    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain { .. })
    }
}

/// Write-only payload sent by UI/CLI when mutating environment variables & secrets.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnvWriteEntry {
    Plain {
        value: String,
    },
    Secret {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default)]
        keep_existing: bool,
    },
    Unset,
}

/// Sanitized effective-environment preview row returned by `env.preview`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvPreviewRow {
    pub key: String,
    pub display_value: String,
    #[serde(default)]
    pub is_secret: bool,
    #[serde(default)]
    pub is_unset: bool,
    #[serde(default)]
    pub configured: bool,
    pub source_layer: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overridden_layers: Vec<String>,
}

/// Cross-host replication status for Global Environment & Secrets.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalEnvSyncSummary {
    pub synced_hosts: usize,
    #[serde(default)]
    pub pending_hosts: Vec<String>,
    pub total_remote_hosts: usize,
}

/// Validate a POSIX-compatible environment variable name.
pub fn is_valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 256
        && !key.contains('=')
        && !key.contains('\0')
        && !key.contains('\n')
        && !key.contains('\r')
        && !key.chars().any(char::is_whitespace)
}

/// Heuristic to auto-classify likely secret keys in the UI and shell picker.
pub fn is_likely_secret_key(key: &str) -> bool {
    let upper = key.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return false;
    }
    const MARKERS: &[&str] = &[
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "APIKEY",
        "PRIVATE_KEY",
        "AUTH",
        "CREDENTIAL",
        "COOKIE",
        "SESSION_KEY",
        "ACCESS_KEY",
        "CLIENT_SECRET",
        "SIGNING_KEY",
    ];
    if MARKERS.iter().any(|m| upper.contains(m)) {
        return true;
    }
    upper.ends_with("_KEY")
        && !matches!(
            upper.as_str(),
            "SORT_KEY" | "CACHE_KEY" | "PRIMARY_KEY" | "PUBLIC_KEY"
        )
}

/// Produce a safe, masked suffix hint for configured secrets.
/// Short secrets (`< 12` chars) yield `"••••"` without any characters.
pub fn secret_hint(plaintext: &str) -> String {
    let count = plaintext.chars().count();
    if count >= 12 {
        let tail: String = plaintext
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("••••{tail}")
    } else {
        "••••".to_owned()
    }
}

/// Parse multi-line `.env` or `export KEY=VALUE` content into ordered `(key, value)` pairs.
/// Duplicate keys keep the last value in their first-seen position.
pub fn parse_dotenv_lines(input: &str) -> Result<Vec<(String, String)>, String> {
    let mut order: Vec<String> = Vec::new();
    let mut values: BTreeMap<String, String> = BTreeMap::new();

    for (idx, raw_line) in input.lines().enumerate() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let body = if let Some(rest) = trimmed.strip_prefix("export")
            && rest.chars().next().is_some_and(char::is_whitespace)
        {
            rest.trim_start()
        } else {
            trimmed
        };
        let Some((raw_key, raw_val)) = body.split_once('=') else {
            return Err(format!("Line {}: expected KEY=VALUE", idx + 1));
        };
        let key = raw_key.trim();
        if !is_valid_env_key(key) {
            return Err(format!("Line {}: invalid variable name '{key}'", idx + 1));
        }
        let val_trimmed = raw_val.trim();
        let value =
            if val_trimmed.len() >= 2 && val_trimmed.starts_with('"') && val_trimmed.ends_with('"')
            {
                unescape_double_quoted(&val_trimmed[1..val_trimmed.len() - 1])
            } else if val_trimmed.len() >= 2
                && val_trimmed.starts_with('\'')
                && val_trimmed.ends_with('\'')
            {
                val_trimmed[1..val_trimmed.len() - 1].to_owned()
            } else {
                strip_inline_comment(val_trimmed).trim_end().to_owned()
            };
        if !values.contains_key(key) {
            order.push(key.to_owned());
        }
        values.insert(key.to_owned(), value);
    }

    Ok(order
        .into_iter()
        .filter_map(|k| values.remove(&k).map(|v| (k, v)))
        .collect())
}

fn unescape_double_quoted(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn strip_inline_comment(raw: &str) -> &str {
    for (idx, ch) in raw.char_indices() {
        if ch == '#'
            && idx > 0
            && raw[..idx]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            return &raw[..idx];
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_key_validation_and_secret_classification() {
        assert!(is_valid_env_key("ANTHROPIC_API_KEY"));
        assert!(is_valid_env_key("RUST_LOG"));
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("BAD KEY"));
        assert!(!is_valid_env_key("BAD=KEY"));
        assert!(!is_valid_env_key("BAD\0KEY"));

        assert!(is_likely_secret_key("ANTHROPIC_API_KEY"));
        assert!(is_likely_secret_key("github_token"));
        assert!(is_likely_secret_key("DB_PASSWORD"));
        assert!(is_likely_secret_key("CUSTOM_SERVICE_KEY"));
        assert!(!is_likely_secret_key("ANTHROPIC_BASE_URL"));
        assert!(!is_likely_secret_key("RUST_LOG"));
        assert!(!is_likely_secret_key("SORT_KEY"));
        assert!(!is_likely_secret_key("PUBLIC_KEY"));

        assert_eq!(secret_hint("short"), "••••");
        assert_eq!(secret_hint("sk-ant-api03-abcdef9a2f"), "••••9a2f");
    }

    #[test]
    fn dotenv_parser_supports_export_quotes_comments_and_dedup() {
        let input = r#"
# comment
export ANTHROPIC_BASE_URL="https://api.example.com"
ANTHROPIC_API_KEY='sk-test-123456'
RUST_LOG=info # inline comment
EMPTY=
MULTILINE="line1\nline2"
RUST_LOG=debug
"#;
        let parsed = parse_dotenv_lines(input).unwrap();
        assert_eq!(
            parsed,
            vec![
                (
                    "ANTHROPIC_BASE_URL".into(),
                    "https://api.example.com".into()
                ),
                ("ANTHROPIC_API_KEY".into(), "sk-test-123456".into()),
                ("RUST_LOG".into(), "debug".into()),
                ("EMPTY".into(), "".into()),
                ("MULTILINE".into(), "line1\nline2".into()),
            ]
        );
        assert!(parse_dotenv_lines("INVALID LINE").is_err());
        assert!(parse_dotenv_lines("BAD KEY=1").is_err());
    }
}
