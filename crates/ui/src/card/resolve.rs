//! Card resolve & reopen actions with lightweight confirmation when runs or branch changes are active.
use super::*;
use canban_types::{Card, CardResolution, agent_activity_warning};
use serde_json::json;

const PREVIEW: &str = "card.resolve.preview";
const RESOLVE: &str = "card.resolve";
const REOPEN: &str = "card.reopen";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveConfirm {
    pub card_id: String,
    pub revision: u64,
    pub warnings: Vec<String>,
    pub has_active_agent: bool,
}

pub(crate) fn busy(ui: Ui) -> bool {
    ui.is_method_pending(PREVIEW) || ui.is_method_pending(RESOLVE) || ui.is_method_pending(REOPEN)
}

fn actor_label(actor: &str) -> String {
    if let Some(profile) = actor.strip_prefix("agent:") {
        format!("@{profile}")
    } else if actor == "user" {
        "user".into()
    } else {
        actor.to_owned()
    }
}

pub(crate) fn tooltip(card: &Card) -> Option<String> {
    if !card.resolved() {
        return None;
    }
    let summary = card.resolution.as_ref().map(|res| {
        let who = actor_label(&res.actor);
        match res
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(reason) => format!("Resolved by {who} · {reason}"),
            None => format!("Resolved by {who}"),
        }
    });
    match (summary, card.active()) {
        (Some(summary), true) => Some(format!("{summary}\nAuto-archives the next day")),
        (Some(summary), false) => Some(summary),
        (None, true) => Some("Auto-archives the next day".into()),
        (None, false) => None,
    }
}

fn resolve_now(ui: Ui, card_id: &str, revision: u64) {
    let current = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned());
    if current.as_ref().is_none_or(|c| c.revision != revision) {
        ui.operation_error
            .set("Card changed while checking; try Resolve again".into());
        return;
    }
    dispatch(
        ui,
        RESOLVE,
        json!({
            "card_id": card_id,
            "revision": revision,
        }),
    );
}

fn apply_gallery_resolve(ui: Ui, card_id: &str) {
    let now_ms = canban_types::now();
    let mut proj = None;
    ui.snapshot.update(|s| {
        if let Some(c) = s.cards.iter_mut().find(|c| c.id == card_id) {
            c.resolution = Some(CardResolution {
                resolved_at: now_ms,
                kind: "manual".into(),
                reason: None,
                actor: "user".into(),
            });
            c.attention = None;
            c.human_gate = None;
            c.revision += 1;
            proj = Some(c.project_id.clone());
        }
    });
    if let Some(project_id) = proj {
        ui.card_timelines.update(|timelines| {
            let entry = timelines.entry(card_id.to_owned()).or_default();
            let seq = entry.events.iter().map(|e| e.sequence).max().unwrap_or(0) + 1;
            entry.events.push(canban_types::Event {
                id: canban_types::id(),
                project_id,
                card_id: card_id.to_owned(),
                sequence: seq,
                kind: "card.resolved".into(),
                actor: "user".into(),
                message: "Card resolved".into(),
                data: json!({"kind": "manual", "resolved_at": now_ms}),
                created_at: now_ms,
            });
            entry.highwater = entry.highwater.max(seq);
            entry.oldest_seq.get_or_insert(seq);
            entry.initialized = true;
        });
    }
}

fn apply_gallery_reopen(ui: Ui, card_id: &str) {
    let now_ms = canban_types::now();
    let mut proj = None;
    ui.snapshot.update(|s| {
        if let Some(c) = s.cards.iter_mut().find(|c| c.id == card_id) {
            if c.merged() {
                return;
            }
            c.resolution = None;
            c.revision += 1;
            proj = Some(c.project_id.clone());
        }
    });
    if let Some(project_id) = proj {
        ui.card_timelines.update(|timelines| {
            let entry = timelines.entry(card_id.to_owned()).or_default();
            let seq = entry.events.iter().map(|e| e.sequence).max().unwrap_or(0) + 1;
            entry.events.push(canban_types::Event {
                id: canban_types::id(),
                project_id,
                card_id: card_id.to_owned(),
                sequence: seq,
                kind: "card.reopened".into(),
                actor: "user".into(),
                message: "Card reopened".into(),
                data: json!({}),
                created_at: now_ms,
            });
            entry.highwater = entry.highwater.max(seq);
            entry.oldest_seq.get_or_insert(seq);
            entry.initialized = true;
        });
    }
}

pub(crate) fn start(ui: Ui, card: Card) {
    let card = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card.id).cloned())
        .unwrap_or(card);
    if !card.active() || card.resolved() || busy(ui) {
        return;
    }
    if !crate::backend::is_connected() {
        apply_gallery_resolve(ui, &card.id);
        return;
    }
    if card.worktree.is_none() {
        let (warnings, has_active_agent) = ui.snapshot.with_untracked(|s| {
            let mut warnings = Vec::new();
            if s.workflow_runs
                .iter()
                .any(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active())
            {
                warnings.push("Active workflow run will be stopped".into());
            }
            let agent_warning = agent_activity_warning(&card.id, &s.terminals);
            let has_active_agent = agent_warning.is_some();
            if let Some(w) = agent_warning {
                warnings.push(w);
            }
            (warnings, has_active_agent)
        });
        if warnings.is_empty() {
            resolve_now(ui, &card.id, card.revision);
        } else {
            ui.resolve_confirm.set(Some(ResolveConfirm {
                card_id: card.id,
                revision: card.revision,
                warnings,
                has_active_agent,
            }));
        }
        return;
    }
    ui.pending_methods.update(|s| {
        s.insert(PREVIEW);
    });
    ui.operation_error.set(String::new());
    ui.status.set("Checking Card status…".into());
    let card_id = card.id.clone();
    let expected_rev = card.revision;
    let params = json!({"card_id": card_id});
    ui.spawn(async move {
        let result = blocking(move || crate::routed_call(PREVIEW, params, None)).await;
        ui.pending_methods.update(|s| {
            s.remove(PREVIEW);
        });
        ui.status.set(String::new());
        match result {
            Ok(Ok(v)) => {
                let rev = v["revision"].as_u64().unwrap_or(expected_rev);
                let warnings: Vec<String> = v["warnings"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let has_active_agent = v["has_active_agent"].as_bool().unwrap_or(false);
                if warnings.is_empty() {
                    resolve_now(ui, &card_id, rev);
                } else {
                    ui.resolve_confirm.set(Some(ResolveConfirm {
                        card_id,
                        revision: rev,
                        warnings,
                        has_active_agent,
                    }));
                }
            }
            Ok(Err(e)) => {
                let msg = format!("{e:#}");
                ui.status.set(msg.clone());
                ui.operation_error.set(msg);
            }
            Err(e) => {
                let msg = e.to_string();
                ui.status.set(msg.clone());
                ui.operation_error.set(msg);
            }
        }
    });
}

pub(crate) fn reopen(ui: Ui, card: Card) {
    let card = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card.id).cloned())
        .unwrap_or(card);
    if !card.active() || !card.resolved() || card.merged() || busy(ui) {
        return;
    }
    if !crate::backend::is_connected() {
        apply_gallery_reopen(ui, &card.id);
        return;
    }
    dispatch(
        ui,
        REOPEN,
        json!({
            "card_id": card.id,
            "revision": card.revision,
        }),
    );
}

fn dismiss(ui: Ui) {
    ui.resolve_confirm.set(None);
}

pub(crate) fn confirm_dialog(ui: Ui) -> impl IntoElement {
    when(
        move || ui.resolve_confirm.get().is_some(),
        move || {
            let Some(p) = ui.resolve_confirm.get_untracked() else {
                return view().into_element();
            };
            let card = ui
                .snapshot
                .with_untracked(|s| s.cards.iter().find(|c| c.id == p.card_id).cloned());
            let title = card
                .as_ref()
                .map(|c| format!("Resolve Card #{}?", c.number))
                .unwrap_or_else(|| "Resolve Card?".into());
            let subtitle = card.as_ref().map(|c| c.title.clone()).unwrap_or_default();
            let warning_text = p.warnings.join("\n");
            let card_id = p.card_id.clone();
            let revision = p.revision;
            let terminals_cid = p.card_id.clone();
            let has_active_agent = p.has_active_agent;
            let mut actions = view().flex_row().gap_2().child(
                action("Mark as Resolved", Variant::Primary, move |_| {
                    dismiss(ui);
                    resolve_now(ui, &card_id, revision);
                })
                .disabled(move || busy(ui)),
            );
            if has_active_agent {
                actions = actions.child(action("Terminals ↗", Variant::Secondary, move |_| {
                    dismiss(ui);
                    crate::terminals::open_card(ui, &terminals_cid);
                }));
            }
            actions =
                actions
                    .child(view().flex_1())
                    .child(action("Cancel", Variant::Quiet, move |_| dismiss(ui)));

            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(move || dismiss(ui))
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(
                            panel()
                                .w(px(420.))
                                .border_1()
                                .border_color(BORDER)
                                .gap_2()
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_between()
                                        .items_center()
                                        .child(label(title).font_weight(FontWeight::SEMIBOLD))
                                        .child(chrome_button(
                                            Icon::Close,
                                            "Cancel resolve",
                                            move |_| dismiss(ui),
                                        )),
                                )
                                .child(muted(subtitle).truncate())
                                .child(notice(warning_text, Tone::Attention))
                                .child(actions),
                        ),
                )
                .into_element()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Card {
        serde_json::from_value(json!({
            "id": "c",
            "project_id": "p",
            "number": 132,
            "title": "Use resolved outcome",
            "body": "",
            "stage": "develop",
            "revision": 1,
            "created_at": 0
        }))
        .unwrap()
    }

    #[test]
    fn tooltip_formats_resolutions_and_next_day_auto_archive() {
        let mut c = card();
        assert_eq!(tooltip(&c), None);

        c.resolution = Some(CardResolution {
            resolved_at: 10,
            kind: "agent".into(),
            reason: Some("Already fixed in #130".into()),
            actor: "agent:developer".into(),
        });
        assert_eq!(
            tooltip(&c).as_deref(),
            Some("Resolved by @developer · Already fixed in #130\nAuto-archives the next day")
        );

        // Merged Cards still use the "Resolved by ..." wording rather than "Merged into ...".
        c.resolution = Some(CardResolution {
            resolved_at: 20,
            kind: "merged".into(),
            reason: None,
            actor: "user".into(),
        });
        c.delivery = Some(canban_types::Delivery {
            target_branch: "main".into(),
            commit: "abc1234".into(),
            request_key: "k1".into(),
            merged_at: 20,
        });
        assert_eq!(
            tooltip(&c).as_deref(),
            Some("Resolved by user\nAuto-archives the next day")
        );

        // Already archived Cards omit the next-day auto-archive hint.
        let mut archived = c.clone();
        archived.archived_at = Some(30);
        assert_eq!(tooltip(&archived).as_deref(), Some("Resolved by user"));

        // Legacy merged Cards without `resolution` still hint next-day auto-archive while active.
        c.resolution = None;
        assert_eq!(tooltip(&c).as_deref(), Some("Auto-archives the next day"));
        c.archived_at = Some(30);
        assert_eq!(tooltip(&c), None);
    }
}
