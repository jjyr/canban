use super::*;
use nsh_core::element::desktop::{MenuItem, popup_menu};
use serde_json::json;
fn close(ui: Ui) {
    ui.card.set(String::new());
    ui.card_standalone.set(false);
    ui.page.set("board".into());
}
fn review(ui: Ui) {
    ui.review.set(None);
    ui.review_request.update(|n| *n += 1);
    ui.page.set("review".into());
    ui.expanded.set(false);
    ui.focused_card.set(false);
}
fn menu(ui: Ui, draft: Signal<Draft>, reading: Reading) {
    let Some(card) = ui.current_card() else {
        return;
    };
    ui.spawn(async move {
        let mut items = Vec::new();
        for (title, kind) in [
            ("Edit task…", 0),
            ("Attachments…", 1),
            ("Workspace details…", 2),
        ] {
            items.push(MenuItem::new(title).on_select(move || {
                if let Some(mut d) = draft.try_get() {
                    match kind {
                        0 => {
                            d.editing = true;
                        }
                        1 => d.details = true,
                        _ => d.workspace_details = true,
                    }
                    draft.try_set(d);
                    reading.top();
                }
            }));
        }
        items.push(MenuItem::separator());
        if let Some(p) = ui.current_project() {
            for stage in p.workflow.stages {
                let id = card.id.clone();
                items.push(
                    MenuItem::new(format!("Move to {}", stage.name))
                        .disabled(stage.id == card.stage || !card.active())
                        .on_select(move || crate::presenter::move_card(ui, &id, &stage.id)),
                );
            }
        }
        let workflows = ui.snapshot.with_untracked(|s| {
            let mut latest = std::collections::BTreeMap::new();
            for w in s
                .workflow_revisions
                .iter()
                .filter(|w| w.project_id == card.project_id)
            {
                latest.insert(w.workflow_id.clone(), w.clone());
            }
            latest.into_values().collect::<Vec<_>>()
        });
        let active = ui.snapshot.with_untracked(|s| {
            s.workflow_runs
                .iter()
                .any(|r| r.card_id == card.id && r.active())
        });
        for workflow in workflows {
            for entry in workflow.definition.entries.keys() {
                let permitted = ui.snapshot.with_untracked(|s| {
                    s.workflow_sources
                        .iter()
                        .find(|f| f.workflow_id == workflow.workflow_id)
                        .is_none_or(|f| {
                            f.enabled
                                && f.rules
                                    .get(entry)
                                    .is_some_and(|r| r.enabled && r.event == "manual")
                        })
                });
                if !permitted {
                    continue;
                }
                let params =
                    json!({"workflow_id":workflow.workflow_id,"card_id":card.id,"entry":entry});
                items.push(
                    MenuItem::new(format!("Run {} · {}", workflow.definition.name, entry))
                        .disabled(active || !card.active() || card.human_gate.is_some())
                        .on_select(move || dispatch(ui, "workflow.start", params.clone())),
                );
            }
        }
        items.push(MenuItem::separator());
        items.extend(crate::card_actions::items(ui, card));
        popup_menu(items).await;
    });
}
pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    view()
        .px_3()
        .py_2()
        .gap_1()
        .border_b_1()
        .border_color(BORDER)
        .flex_shrink_0()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .tooltip(nsh_base::tooltip().child(label(move || {
                            ui.current_card()
                                .map(|c| format!("#{} {}", c.number, c.title))
                                .unwrap_or_default()
                        })))
                        .child(
                            label(move || {
                                ui.current_card()
                                    .map(|c| format!("#{} {}", c.number, c.title))
                                    .unwrap_or_default()
                            })
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(15.))
                            .truncate(),
                        ),
                )
                .child(crate::editor::expand(ui))
                .child(chrome_button(
                    Icon::Terminal,
                    "Open Card Terminals",
                    move |_| {
                        if let Some(c) = ui.current_card() {
                            crate::terminals::open_card(ui, &c.id);
                        }
                    },
                ))
                .child(dynamic(move || {
                    let Some(c) = ui.current_card() else {
                        return view();
                    };
                    let archived = c.archived_at.is_some();
                    chrome_button_disabled(
                        Icon::Archive,
                        if archived {
                            "Restore Card"
                        } else {
                            "Archive Card"
                        },
                        move || ui.pending.get(),
                        move |_| {
                            crate::card_actions::apply(
                                ui,
                                c.clone(),
                                if archived {
                                    crate::card_actions::Action::Restore
                                } else {
                                    crate::card_actions::Action::Archive
                                },
                            );
                        },
                    )
                }))
                .child(chrome_button(Icon::More, "Card actions", move |_| {
                    menu(ui, draft, reading)
                }))
                .child(chrome_button(
                    Icon::Close,
                    "Close Card details",
                    move |_| close(ui),
                )),
        )
        .child(dynamic(move || {
            let Some(c) = ui.current_card() else {
                return view();
            };
            let s = ui.snapshot.get();
            let stage = s
                .projects
                .iter()
                .find(|p| p.id == c.project_id)
                .and_then(|p| p.workflow.stages.iter().find(|t| t.id == c.stage));
            let mut row = view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .child(muted(
                    stage
                        .map(|t| t.name.clone())
                        .unwrap_or_else(|| c.stage.clone()),
                ));
            if c.delivery.is_some() || c.pull_request.as_ref().is_some_and(|p| p.merged) {
                row = row.child(status("Merged", Tone::Success));
            }
            if let Some(branch) = c.branch.clone().filter(|b| !b.is_empty()) {
                row = row.child(
                    view()
                        .min_w(px(0.))
                        .max_w(px(150.))
                        .tooltip(nsh_base::tooltip().child(label(branch.clone())))
                        .child(
                            nsh_base::button()
                                .min_w(px(0.))
                                .h(px(24.))
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .bg(CLEAR)
                                .accessibility_label(|| Some("Copy Card branch".into()))
                                .child(icon(Icon::Branch).w(px(12.)).h(px(12.)))
                                .child(
                                    muted(crate::components::git_workspace::compact(&branch, 22))
                                        .truncate(),
                                )
                                .on_press(move |_: &()| copy_text(ui, branch.clone())),
                        ),
                );
            }
            for agent in canban_notifications::overview(&s)
                .into_iter()
                .filter(|r| r.card.id == c.id && r.request.is_some())
            {
                let req = agent.request.as_ref().unwrap();
                let (mark, _) = crate::profiles::provider(&req.profile);
                let text = if crate::terminals::model::closed(&agent) {
                    "Resume"
                } else if agent
                    .attempt
                    .as_ref()
                    .is_some_and(|a| a.connection_state != "connected" && !a.finished())
                {
                    "Disconnected"
                } else if agent
                    .attempt
                    .as_ref()
                    .is_some_and(|a| a.runtime_status() == "blocked")
                {
                    "Needs input"
                } else if agent.group == "Needs attention" {
                    "Check terminal"
                } else {
                    agent.group.as_str()
                };
                let hint = agent
                    .reasons
                    .iter()
                    .map(|r| r.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let title = format!("{} · {text}", req.profile.id);
                let id = agent.id.clone();
                let ended = crate::terminals::model::closed(&agent);
                let can_open = c.active();
                let profile = req.profile.id.clone();
                let card_id = c.id.clone();
                row = row.child(
                    view()
                        .min_w(px(0.))
                        .max_w(px(170.))
                        .tooltip(nsh_base::tooltip().child(label(if hint.is_empty() {
                            title.clone()
                        } else {
                            hint
                        })))
                        .child(
                            nsh_base::button()
                                .min_w(px(0.))
                                .h(px(24.))
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .bg(CLEAR)
                                .disabled(!can_open)
                                .accessibility_label(move || {
                                    Some(format!("Open Agent {title}").into())
                                })
                                .child(icon(mark).w(px(13.)).h(px(13.)))
                                .child(
                                    muted(format!("{} · {text}", req.profile.id))
                                        .text_color(if agent.actionable() {
                                            ATTENTION
                                        } else {
                                            MUTED
                                        })
                                        .truncate(),
                                )
                                .on_press(move |_: &()| {
                                    if ended {
                                        crate::sessions::open(ui, card_id.clone(), profile.clone());
                                    } else {
                                        ui.open_terminal(id.clone());
                                    }
                                }),
                        ),
                );
            }
            row = row.child(view().flex_1().min_w(px(0.)));
            if c.stage == "review" || stage.is_some_and(|t| t.require_human_review) {
                row = row.child(compact_action("Review", Variant::Quiet, move |_| {
                    review(ui)
                }));
            } else if c.human_gate.is_some() {
                row = row.child(
                    compact_action("Continue workflow", Variant::Quiet, move |_| {
                        if let Some(c) = ui.current_card() {
                            dispatch(
                                ui,
                                "card.continue",
                                json!({"card_id":c.id,"revision":c.revision}),
                            );
                        }
                    })
                    .disabled(move || ui.pending.get()),
                );
            }
            if let Some(reason) = c.attention.or_else(|| c.human_gate.map(|g| g.reason)) {
                row = row.child(
                    view()
                        .tooltip(nsh_base::tooltip().child(label(reason)))
                        .child(
                            compact_action("Needs attention", Variant::Quiet, move |_| {
                                crate::overview::show_card_attention(
                                    ui,
                                    &untracked(|| ui.card_id()),
                                );
                            })
                            .text_color(ATTENTION),
                        ),
                );
            }
            row
        }))
}
