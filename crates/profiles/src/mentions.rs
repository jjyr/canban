use canban_types::Profile;

fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_ticks = false;
    let mut prev_backslash = false;
    for c in line.chars() {
        if c == '`' && !prev_backslash {
            in_ticks = !in_ticks;
            out.push(' ');
            prev_backslash = false;
            continue;
        }
        if !in_ticks {
            out.push(c);
        } else {
            out.push(' ');
        }
        prev_backslash = c == '\\' && !prev_backslash;
    }
    out
}

/// Scans prose for standalone enabled `@profile` tokens, ignoring fenced code blocks,
/// inline backtick code spans, escaped `\@`, emails (`user@profile`), and `>` blockquotes.
pub fn addressed(body: &str, profiles: &[Profile]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut fence: Option<char> = None;
    for raw_line in body.lines() {
        let trimmed = raw_line.trim_start();
        let fence_mark = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        if let Some(mark) = fence_mark {
            if fence == Some(mark) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(mark);
            }
            continue;
        }
        if fence.is_some() || trimmed.starts_with('>') {
            continue;
        }
        let clean = strip_inline_code(raw_line);
        let bytes = clean.as_bytes();
        let mut idx = 0;
        while let Some(rel) = clean[idx..].find('@') {
            let at = idx + rel;
            let valid_before = if at == 0 {
                true
            } else {
                let prev = clean[..at].chars().next_back().unwrap_or(' ');
                prev.is_whitespace()
                    || matches!(prev, '(' | '[' | '{' | '"' | '\'' | ',' | ';' | ':')
            };
            let rest = &clean[at + 1..];
            let name_len = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .map(char::len_utf8)
                .sum::<usize>();
            if valid_before && name_len > 0 {
                let name = &rest[..name_len];
                if profiles.iter().any(|p| p.enabled && p.id == name)
                    && !ids.iter().any(|id| id == name)
                {
                    ids.push(name.to_owned());
                }
                idx = at + 1 + name_len;
            } else {
                idx = at + 1;
            }
            if idx >= bytes.len() {
                break;
            }
        }
    }
    ids
}

/// Remove leading standalone `@id` tokens when dismissing a recipient chip from prose.
pub fn remove_address(body: &str, id: &str, profiles: &[Profile]) -> String {
    let mut rest = body;
    let mut kept_prefix = Vec::new();
    let mut removed_any = false;
    loop {
        let trimmed = rest.trim_start();
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        if end == 0 {
            break;
        }
        let token = &trimmed[..end];
        let Some(name) = token.strip_prefix('@') else {
            break;
        };
        let clean = name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        if !profiles.iter().any(|p| p.enabled && p.id == clean) {
            break;
        }
        if clean == id {
            removed_any = true;
        } else if !kept_prefix.iter().any(|p| p == clean) {
            kept_prefix.push(clean.to_owned());
        }
        rest = &trimmed[end..];
    }
    if !removed_any {
        return body.to_owned();
    }
    let mut next = kept_prefix
        .into_iter()
        .map(|p| format!("@{p}"))
        .collect::<Vec<_>>();
    if !rest.trim_start().is_empty() {
        next.push(rest.trim_start().into());
    }
    next.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::Config;

    #[test]
    fn prose_mentions_are_addressed_while_code_and_quotes_are_ignored() {
        let mut profiles = Config::default().profiles;
        if let Some(p) = profiles.iter_mut().find(|p| p.id == "planner") {
            p.enabled = false;
        }
        assert_eq!(
            addressed("@codex @claude explain this", &profiles),
            vec!["codex", "claude"]
        );
        assert_eq!(
            addressed("Please ask @claude, and let (@codex) review.", &profiles),
            vec!["claude", "codex"]
        );
        assert!(addressed("Disabled @planner ignored", &profiles).is_empty());
        assert!(addressed("> @claude quoted", &profiles).is_empty());
        assert!(addressed("Use `@claude` inline or email user@codex", &profiles).is_empty());
        assert!(addressed("```md\n@claude in fence\n```", &profiles).is_empty());
        assert!(addressed("\\@claude escaped", &profiles).is_empty());
        assert_eq!(
            remove_address(
                "  @codex  @codex\n@claude  explain\n  details",
                "claude",
                &profiles
            ),
            "@codex explain\n  details"
        );
        assert_eq!(
            remove_address("@codex\n  question", "codex", &profiles),
            "question"
        );
        assert_eq!(
            remove_address("@codex @claude explain this", "claude", &profiles),
            "@codex explain this"
        );
        assert_eq!(
            remove_address("Please ask @claude to check", "claude", &profiles),
            "Please ask @claude to check"
        );
    }
}
