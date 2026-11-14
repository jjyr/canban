//! Registry projections reused by Settings and the Card mention composer.
use canban_types::Profile;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

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

/// Scans prose for standalone `@profile` tokens, ignoring fenced code blocks,
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
                if profiles.iter().any(|p| p.id == name) && !ids.iter().any(|id| id == name) {
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
        if !profiles.iter().any(|p| p.id == clean) {
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

pub fn registry(profiles: Signal<Vec<Profile>>) -> SectionGroup {
    settings_section(
        "Agent profiles",
        "Workflow stages and @mentions select a profile from agents.json.",
    )
    .child(registry_items(profiles))
}
/// Content-only projection for composition inside the screen's settings group.
pub fn registry_items(profiles: Signal<Vec<Profile>>) -> impl IntoElement {
    dynamic(move || {
        view().min_w(px(0.)).gap_2().children(
            profiles
                .get()
                .into_iter()
                .map(|profile| {
                    let (provider_icon, provider_name) = provider(&profile);
                    let model_badge = profile.model.clone();
                    let effort_badge = profile.effort.clone();
                    view()
                        .min_w(px(0.))
                        .py_2()
                        .px_2p5()
                        .rounded(px(RADIUS))
                        .bg(INSET)
                        .gap_1()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .min_w(px(0.))
                                .child(icon(provider_icon).w(px(14.)).h(px(14.)).flex_shrink_0())
                                .child(
                                    label(format!("@{}", profile.id))
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate(),
                                )
                                .child(badge(provider_name))
                                .child(if let Some(m) = model_badge {
                                    badge(m).into_element()
                                } else {
                                    view().into_element()
                                })
                                .child(if let Some(e) = effort_badge {
                                    badge(format!("effort: {e}")).into_element()
                                } else {
                                    view().into_element()
                                }),
                        )
                        .child(muted(if profile.role_instruction.is_empty() {
                            "No fixed role instruction".into()
                        } else {
                            profile.role_instruction
                        }))
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
    })
}

/// Canonical icon for any Herdr built-in agent / binary / template key.
pub fn agent_icon(key: &str) -> Icon {
    let clean = key.strip_prefix("herdr:").unwrap_or(key);
    match clean {
        "codex" => Icon::Codex,
        "claude" => Icon::Claude,
        "agy" | "antigravity" => Icon::Agy,
        "gemini" => Icon::Gemini,
        "opencode" => Icon::OpenCode,
        "cursor" | "cursor-agent" => Icon::Cursor,
        "pi" => Icon::Pi,
        "omp" => Icon::Omp,
        "copilot" => Icon::Copilot,
        "droid" => Icon::Droid,
        "amp" => Icon::Amp,
        "kimi" => Icon::Kimi,
        "kiro" | "kiro-cli" => Icon::Kiro,
        "qwen" => Icon::Qwen,
        "devin" => Icon::Devin,
        "cline" => Icon::Cline,
        "grok" => Icon::Grok,
        "hermes" => Icon::Hermes,
        "kilo" => Icon::Kilo,
        "qodercli" | "qoder" => Icon::Qoder,
        "letta" => Icon::Letta,
        "mastracode" | "mastra" => Icon::Mastra,
        "maki" => Icon::Maki,
        "muse" => Icon::Muse,
        _ => Icon::Agent,
    }
}

/// Same provider identity on Cards, Terminal inventory and tabs.
pub fn provider(profile: &Profile) -> (Icon, String) {
    let command = profile.command_name();
    let provider_key = match command.as_str() {
        "codex" | "claude" => command.as_str(),
        _ => profile.preset.as_deref().unwrap_or(&command),
    };
    if let Some(tpl) = provider_key.strip_prefix("herdr:") {
        return (agent_icon(tpl), format!("Herdr · {tpl}"));
    }
    match provider_key {
        "codex" => (Icon::Codex, "Codex".into()),
        "claude" => (Icon::Claude, "Claude".into()),
        other => (
            agent_icon(other),
            canban_profiles::display_name(other)
                .map(str::to_owned)
                .or_else(|| profile.name.clone())
                .unwrap_or_else(|| other.to_owned()),
        ),
    }
}

/// Effective provider display for a profile in an optional Project context (respecting `agent_overrides`).
pub fn effective_provider(
    profile: &Profile,
    project: Option<&canban_types::Project>,
) -> (Icon, String, bool) {
    if profile.is_identity()
        && let Some(proj) = project
        && let Some(ov) = proj
            .agent_overrides
            .get(&profile.id)
            .map(canban_types::ProjectAgentOverride::normalized)
            .filter(|ov| !ov.is_empty())
    {
        if let Some(override_id) = ov.agent.as_deref() {
            let label = override_id
                .strip_prefix("herdr:")
                .map(|t| format!("Herdr · {t}"))
                .or_else(|| canban_profiles::display_name(override_id).map(str::to_owned))
                .unwrap_or_else(|| override_id.to_owned());
            return (agent_icon(override_id), label, true);
        }
        let (mark, label) = provider(profile);
        return (mark, label, true);
    }
    let (mark, label) = provider(profile);
    (mark, label, false)
}

/// Effective provider display name for a profile in an optional Project context (respecting `agent_overrides`).
pub fn effective_provider_name(
    profile: &Profile,
    project: Option<&canban_types::Project>,
) -> (String, bool) {
    let (_, label, overridden) = effective_provider(profile, project);
    (label, overridden)
}

#[cfg(test)]
mod mention_tests {
    use super::*;
    #[test]
    fn prose_mentions_are_addressed_while_code_and_quotes_are_ignored() {
        let profiles = canban_types::Config::default().profiles;
        assert_eq!(
            addressed("@codex @claude explain this", &profiles),
            vec!["codex", "claude"]
        );
        assert_eq!(
            addressed("Please ask @claude, and let (@codex) review.", &profiles),
            vec!["claude", "codex"]
        );
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

/// Offer a recipient only at a standalone @word outside quotations/code/email.
/// Editor offsets use UTF-16, not UTF-8 bytes.
pub fn mention_context(body: &str, offset: u32) -> bool {
    let prefix = body
        .chars()
        .scan(0u32, |n, c| {
            *n += c.len_utf16() as u32;
            Some((*n, c))
        })
        .take_while(|(n, _)| *n <= offset)
        .map(|(_, c)| c)
        .collect::<String>();
    let Some(at) = prefix.rfind('@') else {
        return false;
    };
    if prefix[at + 1..]
        .chars()
        .any(|c| !c.is_alphanumeric() && c != '-' && c != '_')
    {
        return false;
    }
    let before = &prefix[..at];
    if before
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
    {
        return false;
    }
    let line = before.rsplit('\n').next().unwrap_or_default();
    if line.trim_start().starts_with('>') {
        return false;
    }
    let mut fence = None;
    for line in before.lines() {
        let line = line.trim_start();
        let mark = if line.starts_with("```") {
            Some('`')
        } else if line.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        if let Some(mark) = mark {
            if fence == Some(mark) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(mark);
            }
        }
    }
    fence.is_none() && line.matches('`').count() % 2 == 0
}
#[cfg(test)]
mod completion_tests {
    use super::*;
    #[test]
    fn completion_respects_markdown_context_and_utf16_positions() {
        for body in ["@", "Please ask @dev", "😀 help @cod", "done\n@claude"] {
            assert!(
                mention_context(body, body.encode_utf16().count() as u32),
                "{body}"
            );
        }
        for body in [
            "hello@example",
            "`@cod",
            "Use `hello @cod",
            "> @cod",
            "```rust\n@cod",
            "~~~\n@dev",
            "\\@cod",
            "@cod hello",
            "prefix@cod",
        ] {
            assert!(
                !mention_context(body, body.encode_utf16().count() as u32),
                "{body}"
            );
        }
        let body = "😀 @cod trailing";
        assert!(mention_context(
            body,
            "😀 @cod".encode_utf16().count() as u32
        ));
    }
}
