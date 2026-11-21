//! Card header quick merge: accept a local Card without opening the Diff workbench.
//!
//! It reuses the AppServer review contract (`review.preview` → `review.merge`), so the
//! token, stale-revision, target-cleanliness and conflict protections stay in AppServer.
//! A preview warning (for example a still-running Agent) requires explicit confirmation.
use super::*;
use canban_types::{Card, DeliveryMode, Project, ReviewPreview};
use serde_json::json;

const PREVIEW: &str = "review.preview";
const MERGE: &str = "review.merge";

/// Local delivery Cards with a worktree that are not merged or bound to a PR.
pub(crate) fn eligible(card: &Card, project: Option<&Project>) -> bool {
    let mode = card
        .delivery_mode
        .clone()
        .or_else(|| project.map(|p| p.delivery_mode.clone()))
        .unwrap_or_default();
    card.active()
        && card.worktree.is_some()
        && mode != DeliveryMode::GitHubPr
        && !card.is_github_pr()
        && card.delivery.is_none()
        && card.pull_request.is_none()
}

pub(crate) fn busy(ui: Ui) -> bool {
    ui.is_method_pending(PREVIEW) || ui.is_method_pending(MERGE)
}

fn message(card: &Card) -> String {
    format!("Canban #{}: {}", card.number, card.title)
}

/// Preview, then merge immediately unless the preview blocks or warns.
pub(crate) fn start(ui: Ui, card: Card) {
    if busy(ui) {
        return;
    }
    if !crate::backend::is_connected() {
        ui.operation_error
            .set("Gallery preview: no AppServer connected".into());
        return;
    }
    let target = ui.snapshot.with_untracked(|s| {
        s.projects
            .iter()
            .find(|p| p.id == card.project_id)
            .and_then(|p| p.target_branch.clone())
    });
    ui.pending_methods.update(|s| {
        s.insert(PREVIEW);
    });
    ui.merging_cards.update(|ids| {
        ids.insert(card.id.clone());
    });
    ui.operation_error.set(String::new());
    ui.status.set("Checking merge…".into());
    let card_id = card.id.clone();
    let merging_id = card.id.clone();
    let params = json!({"card_id": card_id, "target_branch": target});
    ui.spawn(async move {
        let result = blocking(move || crate::routed_call(PREVIEW, params, None)).await;
        ui.pending_methods.update(|s| {
            s.remove(PREVIEW);
        });
        ui.merging_cards.update(|ids| {
            ids.remove(&merging_id);
        });
        let preview = match result {
            Ok(Ok(value)) => serde_json::from_value::<ReviewPreview>(value)
                .map_err(|e| format!("Unexpected merge preview: {e}")),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Err(e) => Err(e.to_string()),
        };
        match preview {
            Err(error) => {
                ui.status.set(error.clone());
                ui.operation_error.set(error);
            }
            Ok(p) if !p.merge_allowed => {
                let reason = format!(
                    "Cannot merge: {}",
                    p.reason.as_deref().unwrap_or("merge unavailable")
                );
                ui.status.set(reason.clone());
                ui.operation_error.set(reason);
            }
            Ok(p) if p.warning.is_some() => {
                ui.status.set(String::new());
                ui.quick_merge.set(Some(p));
            }
            Ok(p) => {
                ui.status.set(String::new());
                merge(ui, p, &card);
            }
        }
    });
}

/// Merge exactly the previewed snapshot with the default commit message.
fn merge(ui: Ui, p: ReviewPreview, card: &Card) {
    // A response for another Card revision must not merge newer, unseen work.
    let current = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == p.card_id).cloned());
    if current.as_ref().is_none_or(|c| c.revision != p.revision) {
        ui.operation_error
            .set("Card changed while checking; try Merge again".into());
        return;
    }
    dispatch(
        ui,
        MERGE,
        json!({
            "card_id": p.card_id,
            "revision": p.revision,
            "token": p.token,
            "target_branch": p.target_branch,
            "message": message(current.as_ref().unwrap_or(card)),
        }),
    );
}

fn dismiss(ui: Ui) {
    ui.quick_merge.set(None);
}

/// Confirmation for a preview warning. Rendered once by the Card header.
pub(crate) fn confirm_dialog(ui: Ui) -> impl IntoElement {
    when(
        move || ui.quick_merge.get().is_some(),
        move || {
            let Some(p) = ui.quick_merge.get_untracked() else {
                return view().into_element();
            };
            let card = ui
                .snapshot
                .with_untracked(|s| s.cards.iter().find(|c| c.id == p.card_id).cloned());
            let title = card
                .as_ref()
                .map(|c| format!("Merge Card #{} into {}?", c.number, p.target_branch))
                .unwrap_or_else(|| format!("Merge into {}?", p.target_branch));
            let files = match p.files.len() {
                1 => "1 file changed".to_owned(),
                n => format!("{n} files changed"),
            };
            // `source_branch` is the Project checkout; show the Card branch being merged.
            let branch = card
                .as_ref()
                .and_then(|c| c.branch.clone())
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "Card worktree".into());
            let warning = p.warning.clone().unwrap_or_default();
            let terminals = p.card_id.clone();
            let accepted = p.clone();
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
                                            "Cancel quick merge",
                                            move |_| dismiss(ui),
                                        )),
                                )
                                .child(muted(format!("{} → {} · {files}", branch, p.target_branch)))
                                .child(notice(warning, Tone::Attention))
                                .child(
                                    view()
                                        .flex_row()
                                        .gap_2()
                                        .child(
                                            action("Merge anyway", Variant::Primary, move |_| {
                                                dismiss(ui);
                                                if let Some(c) = card.as_ref() {
                                                    merge(ui, accepted.clone(), c);
                                                }
                                            })
                                            .disabled(move || busy(ui)),
                                        )
                                        .child(action(
                                            "Terminals ↗",
                                            Variant::Secondary,
                                            move |_| {
                                                dismiss(ui);
                                                crate::terminals::open_card(ui, &terminals);
                                            },
                                        ))
                                        .child(view().flex_1())
                                        .child(action("Cancel", Variant::Quiet, move |_| {
                                            dismiss(ui)
                                        })),
                                ),
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
            "id": "c", "project_id": "p", "number": 7, "title": "Quick", "body": "",
            "stage": "review", "revision": 1, "created_at": 0,
            "worktree": "/tmp/w", "branch": "canban/7"
        }))
        .unwrap()
    }

    #[test]
    fn eligibility_follows_local_delivery_contract() {
        let c = card();
        assert!(eligible(&c, None));
        assert_eq!(message(&c), "Canban #7: Quick");

        let mut no_worktree = c.clone();
        no_worktree.worktree = None;
        assert!(!eligible(&no_worktree, None));

        let mut pr_mode = c.clone();
        pr_mode.delivery_mode = Some(DeliveryMode::GitHubPr);
        assert!(!eligible(&pr_mode, None));

        let mut archived = c.clone();
        archived.archived_at = Some(1);
        assert!(!eligible(&archived, None));

        let mut merged = c;
        merged.delivery = Some(canban_types::Delivery {
            target_branch: "main".into(),
            commit: "abc".into(),
            request_key: "k".into(),
            merged_at: 1,
        });
        assert!(!eligible(&merged, None));
    }
}
