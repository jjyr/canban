use super::*;
use nsh_core::element::desktop::{MenuItem, popup_menu};
use serde_json::json;
fn close(ui: Ui) {
    ui.card.set(String::new());
    ui.card_standalone.set(false);
    ui.page.set("board".into());
}
fn review(ui: Ui) {
    if let Some(c) = ui.current_card() {
        crate::review::open_card(ui, &c.id);
    } else {
        crate::review::open_compare(ui);
    }
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
        for run in ui.snapshot.with_untracked(|s| {
            s.workflow_runs
                .iter()
                .filter(|r| r.card_id == card.id && r.active())
                .cloned()
                .collect::<Vec<_>>()
        }) {
            items.push(
                MenuItem::new(format!("Stop {}", run.definition.name))
                    .on_select(move || dispatch(ui, "workflow.stop", json!({"run_id":run.id}))),
            );
        }
        let (has_deliver_in_project, active_non_deliver) = ui.snapshot.with_untracked(|s| {
            let has_deliver = s.workflow_revisions.iter().any(|w| {
                w.project_id == card.project_id
                    && w.definition
                        .steps
                        .values()
                        .any(|st| matches!(st, canban_types::WorkflowStep::Deliver { .. }))
            });
            let non_deliver = s.workflow_runs.iter().any(|r| {
                r.card_id == card.id
                    && r.active()
                    && !r.step.as_ref().is_some_and(|k| {
                        matches!(
                            r.definition.steps.get(k),
                            Some(canban_types::WorkflowStep::Deliver { .. })
                        )
                    })
            });
            (has_deliver, non_deliver)
        });
        if has_deliver_in_project && card.active() {
            if card.worktree.is_some()
                && card.delivery.is_none()
                && !card.pull_request.as_ref().is_some_and(|p| p.merged)
            {
                let cid = card.id.clone();
                let rev = card.revision;
                items.push(
                    MenuItem::new("Retry auto-merge")
                        .disabled(active_non_deliver)
                        .on_select(move || {
                            dispatch(ui, "workflow.deliver", json!({"card_id":cid,"revision":rev}))
                        }),
                );
            }
            let held = card.hold_auto_merge;
            let cid = card.id.clone();
            let rev = card.revision;
            items.push(
                MenuItem::new(if held {
                    "Resume auto-merge"
                } else {
                    "Hold auto-merge"
                })
                .on_select(move || {
                    dispatch(
                        ui,
                        "card.hold_auto_merge",
                        json!({"card_id":cid,"revision":rev,"hold":!held}),
                    )
                }),
            );
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
        .child(dynamic(move || {
            let Some(c) = ui.current_card() else {
                return view().into_element();
            };
            let mut row = view().flex_row().flex_wrap().items_center().gap_2();
            if let Some(source) = c.github_issue() {
                let state = ui
                    .snapshot
                    .with(|s| {
                        s.github_issues
                            .iter()
                            .find(|i| i.project_id == c.project_id && i.source == *source)
                            .map(|i| i.state.clone())
                    })
                    .unwrap_or_else(|| "Status unavailable".into());
                row = row.child(icon(Icon::GitHub)).child(muted(format!(
                    "{} {}#{} · {}",
                    if c.is_github_pr() { "PR" } else { "Issue" },
                    source.repository,
                    source.number,
                    state
                )));
                let url = source.url.clone();
                row = row.child(view().flex_1()).child(compact_action(
                    "Open in GitHub ↗",
                    Variant::Quiet,
                    move |_| crate::github::open_url(ui, url.clone()),
                ));
            } else {
                let project = ui
                    .snapshot
                    .with(|s| {
                        s.projects
                            .iter()
                            .find(|p| p.id == c.project_id)
                            .map(|p| p.name.clone())
                    })
                    .unwrap_or_default();
                row = row.child(muted(format!("{project} / Card #{}", c.number)));
            }
            row.into_element()
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(when(
                    move || {
                        ui.page.get() == "card" && (ui.narrow.get() || ui.card_standalone.get())
                    },
                    move || chrome_button(Icon::Back, "Back to Board", move |_| close(ui)),
                ))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .tooltip(nsh_base::tooltip().child(label(move || {
                            ui.current_card().map(|c| c.title).unwrap_or_default()
                        })))
                        .child(
                            label(move || ui.current_card().map(|c| c.title).unwrap_or_default())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(17.))
                                .truncate(),
                        ),
                )
                .child(when(
                    move || ui.page.get() != "review",
                    move || crate::editor::expand(ui),
                ))
                .child(chrome_button(
                    Icon::Terminal,
                    "Open Card Terminals",
                    move |_| {
                        if let Some(c) = ui.current_card() {
                            crate::terminals::open_card(ui, &c.id);
                        }
                    },
                ))
                .child(chrome_button(Icon::More, "Card actions", move |_| {
                    menu(ui, draft, reading)
                }))
                .child(when(
                    move || ui.page.get() != "review",
                    move || chrome_button(Icon::Close, "Close Card details", move |_| close(ui)),
                )),
        )
        .child(dynamic(move || {
            let Some(c) = ui.current_card() else {
                return view();
            };
            let s = ui.snapshot.get();
            let project_stages = s
                .projects
                .iter()
                .find(|p| p.id == c.project_id)
                .map(|p| p.workflow.stages.clone())
                .unwrap_or_default();
            let stage = project_stages.iter().find(|t| t.id == c.stage);
            let stage_name = stage
                .map(|t| t.name.clone())
                .unwrap_or_else(|| c.stage.clone());
            let stage_a11y = stage_name.clone();
            let stage_anchor = element_ref();
            let stage_card = c.clone();
            let stages_for_menu = project_stages.clone();
            let mut row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .child(
                    nsh_base::button()
                        .element_ref(&stage_anchor)
                        .h(px(24.))
                        .px_2()
                        .rounded(px(5.))
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .bg(INSET)
                        .hover(|st| st.bg(CARD_HOVER))
                        .focus(|st| st.border_1().border_color(ACCENT))
                        .disabled(!c.active() || stages_for_menu.is_empty())
                        .accessibility_label(move || {
                            Some(format!("Change stage · {stage_a11y}").into())
                        })
                        .child(muted(format!("{stage_name} ▾")))
                        .on_press(move |_: &()| {
                            use nsh_core::element::desktop::popup_menu_at;
                            let stages = stages_for_menu.clone();
                            let card = stage_card.clone();
                            ui.spawn(async move {
                                let Some(bounds) = stage_anchor.measure().await else {
                                    return;
                                };
                                let items = stages
                                    .into_iter()
                                    .map(|st| {
                                        let cid = card.id.clone();
                                        let target = st.id.clone();
                                        let prefix = if st.id == card.stage { "✓ " } else { "" };
                                        MenuItem::new(format!("{prefix}{}", st.name))
                                            .disabled(st.id == card.stage || !card.active())
                                            .on_select(move || {
                                                crate::presenter::move_card(ui, &cid, &target)
                                            })
                                    })
                                    .collect::<Vec<_>>();
                                popup_menu_at(
                                    items,
                                    point(bounds.page.x, bounds.page.y + bounds.frame.size.height),
                                )
                                .await;
                            });
                        }),
                );
            if c.delivery.is_some() || c.pull_request.as_ref().is_some_and(|p| p.merged) {
                row = row.child(status("Merged", Tone::Success));
            }
            if let Some(c_active) = Some(&c).filter(|c| c.active()) {
                let has_deliver_in_project = s.workflow_revisions.iter().any(|w| {
                    w.project_id == c_active.project_id
                        && w.definition
                            .steps
                            .values()
                            .any(|st| matches!(st, canban_types::WorkflowStep::Deliver { .. }))
                });
                if has_deliver_in_project && c_active.hold_auto_merge {
                    row = row.child(status("Auto-merge held", Tone::Attention));
                }
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
                    if crate::terminals::model::can_resume(&agent, &ui.profiles.get()) {
                        "Resume"
                    } else {
                        "View history"
                    }
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
                let resumable = crate::terminals::model::can_resume(&agent, &ui.profiles.get());
                let can_open = !c.removed_at.is_some();
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
                                    if ended && resumable {
                                        crate::terminals::navigation::resume(ui, &id);
                                    } else {
                                        ui.open_terminal(id.clone());
                                    }
                                }),
                        ),
                );
            }
            let attachment_count = s.artifacts.iter().filter(|a| a.card_id == c.id).count();
            if attachment_count > 0 {
                row = row.child(
                    nsh_base::button()
                        .h(px(24.))
                        .px_2()
                        .rounded(px(5.))
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .bg(CLEAR)
                        .hover(|st| st.bg(INSET))
                        .focus(|st| st.border_1().border_color(ACCENT))
                        .accessibility_label(move || {
                            Some(format!("Toggle attachments ({attachment_count})").into())
                        })
                        .child(icon(Icon::Paperclip).w(px(12.)).h(px(12.)))
                        .child(muted(attachment_count.to_string()))
                        .on_press(move |_: &()| {
                            draft.update(|d| d.details = !d.details);
                            reading.top();
                        }),
                );
            }
            row = row.child(view().flex_1().min_w(px(0.)));
            let in_review_stage =
                c.stage == "review" || stage.is_some_and(|t| t.require_human_review);
            let can_review_diff = in_review_stage
                || c.worktree.is_some()
                || c.branch.as_ref().is_some_and(|b| !b.is_empty())
                || c.is_github_pr()
                || c.pull_request.is_some();
            if ui.page.get() != "review" && can_review_diff {
                row = row.child(compact_action(
                    if in_review_stage || c.is_github_pr() {
                        "Review"
                    } else {
                        "Diff"
                    },
                    if c.stage == "review" {
                        Variant::Primary
                    } else {
                        Variant::Secondary
                    },
                    move |_| review(ui),
                ));
            }
            if !in_review_stage && c.human_gate.is_some() {
                row = row.child(
                    compact_action("Continue workflow", Variant::Secondary, move |_| {
                        if let Some(c) = ui.current_card() {
                            dispatch(
                                ui,
                                "card.continue",
                                json!({"card_id":c.id,"revision":c.revision}),
                            );
                        }
                    })
                    .disabled(move || ui.is_method_pending("card.continue")),
                );
            }
            if let Some(reason) = c.attention.or_else(|| c.human_gate.map(|g| g.reason)) {
                row = row.child(
                    view()
                        .tooltip(nsh_base::tooltip().child(label(reason)))
                        .child(
                            compact_action("Needs attention", Variant::Secondary, move |_| {
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
