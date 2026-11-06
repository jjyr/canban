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

pub fn registry(profiles: Signal<Vec<Profile>>) -> impl IntoElement {
    panel().child(label("Available Agent profiles"))
        .child(muted("Profiles are defined once in agents.json. Workflow stages reference their IDs; mentions select one directly."))
        .child(dynamic(move || {
            view().gap_3().children(profiles.get().into_iter().map(|profile| {
                panel().child(label(format!("@{}",profile.id)))
                    .child(muted(if profile.role_instruction.is_empty() { "No fixed role instruction".into() } else { profile.role_instruction }))
                    .into_element()
            }).collect::<Vec<_>>())
        }))
}
