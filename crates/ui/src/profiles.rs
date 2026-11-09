//! Registry projections reused by Settings and the Card mention composer.
use canban_types::Profile;
use canban_ui_kit::*;
use nsh_core::prelude::*;

/// Only leading @tokens address Agents; names quoted/discussed in prose are ordinary text.
pub fn addressed(body: &str, profiles: &[Profile]) -> Vec<String> {
    let mut ids = Vec::new();
    for word in body.split_whitespace() {
        let Some(name) = word.strip_prefix('@') else {
            break;
        };
        let name = name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        if !profiles.iter().any(|p| p.id == name) {
            break;
        }
        if !ids.iter().any(|id| id == name) {
            ids.push(name.to_owned());
        }
    }
    ids
}
fn remove_address(body: &str, id: &str, profiles: &[Profile]) -> String {
    let prefix = addressed(body, profiles);
    let mut rest = body;
    // Consume actual leading tokens, including duplicates and arbitrary whitespace.
    loop {
        let trimmed = rest.trim_start();
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        if end == 0 || addressed(&trimmed[..end], profiles).is_empty() {
            break;
        }
        rest = &trimmed[end..];
    }
    let mut next = prefix
        .into_iter()
        .filter(|p| p != id)
        .map(|p| format!("@{p}"))
        .collect::<Vec<_>>();
    if !rest.trim_start().is_empty() {
        next.push(rest.trim_start().into());
    }
    next.join(" ")
}
pub fn mention_picker(profiles: Signal<Vec<Profile>>, draft: Signal<String>) -> impl IntoElement {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .min_w(px(0.))
        .child(
            action("@", Variant::Quiet, move |_| {
                spawn(async move {
                    let selected = addressed(&draft.get_untracked(), &profiles.get_untracked());
                    let items = profiles
                        .get_untracked()
                        .into_iter()
                        .map(|p| {
                            let id = p.id;
                            let checked = selected.contains(&id);
                            MenuItem::new(format!("@{id}"))
                                .checked(checked)
                                .on_select(move || {
                                    if let Some(body) = draft.try_get() {
                                        let next = if checked {
                                            remove_address(&body, &id, &profiles.get_untracked())
                                        } else {
                                            format!("@{id} {body}")
                                        };
                                        draft.try_set(next);
                                    }
                                })
                        })
                        .collect();
                    popup_menu(items).await;
                });
            })
            .accessibility_label(|| Some("Mention Agent".into()))
            .disabled(move || profiles.get().is_empty()),
        )
        .child(dynamic(move || {
            view().flex_row().flex_wrap().gap_1().children(
                addressed(&draft.get(), &profiles.get())
                    .into_iter()
                    .map(|id| {
                        let title = format!("@{id} ×");
                        action(title, Variant::Quiet, move |_| {
                            draft.set(remove_address(
                                &draft.get_untracked(),
                                &id,
                                &profiles.get_untracked(),
                            ));
                        })
                        .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
        }))
}

pub fn registry(profiles: Signal<Vec<Profile>>) -> View {
    settings_section(
        "Agent profiles",
        "Workflow stages and @mentions select a profile from agents.json.",
    )
    .child(registry_items(profiles))
}
/// Content-only projection for composition inside the screen's settings group.
pub fn registry_items(profiles: Signal<Vec<Profile>>) -> impl IntoElement {
    dynamic(move || {
        view().min_w(px(0.)).gap_3().children(
            profiles
                .get()
                .into_iter()
                .map(|profile| {
                    view()
                        .min_w(px(0.))
                        .gap_1()
                        .child(label(format!("@{}", profile.id)).font_weight(FontWeight::MEDIUM))
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

/// Same provider identity on Cards, Terminal inventory and tabs.
pub fn provider(profile: &Profile) -> (Icon, String) {
    let command = profile.command_name();
    let provider = match command.as_str() {
        "codex" | "claude" => command.as_str(),
        _ => profile.preset.as_deref().unwrap_or(&command),
    };
    match provider {
        "codex" => (Icon::Codex, "Codex".into()),
        "claude" => (Icon::Claude, "Claude".into()),
        other => (Icon::Agent, other.into()),
    }
}

#[cfg(test)]
mod mention_tests {
    use super::*;
    #[test]
    fn only_explicit_prefix_targets_are_notified_and_removing_one_retains_the_question() {
        let profiles = canban_types::Config::default().profiles;
        assert_eq!(
            addressed("@codex @claude explain this", &profiles),
            vec!["codex", "claude"]
        );
        assert!(addressed("The UI shows Ask @claude", &profiles).is_empty());
        assert!(addressed("> @claude quoted", &profiles).is_empty());
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
    }
}
