//! Registry projections reused by Settings and the Card mention composer.
use canban_types::Profile;
use canban_ui_kit::*;
use nsh_core::prelude::*;

pub fn mention_picker(profiles: Signal<Vec<Profile>>, draft: Signal<String>) -> impl IntoElement {
    dynamic(move || {
        view().flex_row().flex_wrap().gap_2().children(
            profiles
                .get()
                .into_iter()
                .map(|profile| {
                    let id = profile.id;
                    action(format!("Ask @{id}"), Variant::Quiet, move |_| {
                        let body = draft.get_untracked();
                        draft.set(format!("@{id} {body}"));
                    })
                    .into_element()
                })
                .collect::<Vec<_>>(),
        )
    })
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
