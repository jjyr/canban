use super::*;
use nsh_core::element::desktop::{MenuItem, popup_menu};
use serde_json::json;
fn close(ui: Ui) {
    ui.close_card();
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
                ui.card_tab.set(String::new());
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
                        .disabled(
                            active || !card.active() || card.merged() || card.human_gate.is_some(),
                        )
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
        if has_deliver_in_project && card.active() && !card.merged() {
            if card.worktree.is_some() {
                let cid = card.id.clone();
                let rev = card.revision;
                items.push(
                    MenuItem::new("Retry auto-merge")
                        .disabled(active_non_deliver)
                        .on_select(move || {
                            dispatch(
                                ui,
                                "workflow.deliver",
                                json!({"card_id":cid,"revision":rev}),
                            )
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
                        ui.page.get() == "card"
                            && (ui.narrow.get()
                                || ui.card_standalone.get()
                                || ui.card_terminal_open.get())
                    },
                    move || chrome_button(Icon::Back, "Back to Board", move |_| close(ui)),
                ))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .tooltip(hint_tooltip(move || {
                            ui.current_card().map(|c| c.title).unwrap_or_default()
                        }))
                        .child(
                            label(move || ui.current_card().map(|c| c.title).unwrap_or_default())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(17.))
                                .truncate(),
                        ),
                )
                .child(when(
                    move || ui.page.get() != "review" && !ui.card_terminal_open.get(),
                    move || crate::editor::expand(ui),
                ))
                .child(when(
                    move || ui.current_card().is_some_and(|c| c.active() && c.merged()),
                    move || {
                        chrome_button_disabled(
                            Icon::Archive,
                            "Archive Card",
                            move || ui.pending.get(),
                            move |_| {
                                if let Some(c) = ui.current_card() {
                                    crate::card_actions::apply(
                                        ui,
                                        c,
                                        crate::card_actions::Action::Archive,
                                    );
                                }
                            },
                        )
                    },
                ))
                .child(chrome_button_disabled(
                    Icon::Agent,
                    "Open Agent",
                    move || {
                        ui.pending.get()
                            || ui.current_card().is_none_or(|c| c.archived_at.is_some())
                    },
                    move |_| {
                        if let Some(c) = ui.current_card() {
                            crate::terminals::create_in_card(ui, c.project_id, c.id, true);
                        }
                    },
                ))
                .child(chrome_button_disabled(
                    Icon::Terminal,
                    "Open Terminal",
                    move || {
                        ui.pending.get()
                            || ui.current_card().is_none_or(|c| c.archived_at.is_some())
                    },
                    move |_| {
                        if let Some(c) = ui.current_card() {
                            if ui.page.get_untracked() == "review" {
                                crate::terminals::open_card(ui, &c.id);
                                return;
                            }
                            if ui.card_terminal_open.get_untracked() {
                                ui.card_terminal_open.set(false);
                                ui.card_terminal_focused.set(false);
                            }
                            let live = crate::terminals::card_live_entries(
                                &ui.snapshot.get_untracked(),
                                &c.id,
                            );
                            if ui.card_tab.get_untracked().is_empty() && !live.is_empty() {
                                let recent = ui.terminal_recent.with(|m| m.get(&c.id).cloned());
                                let target = recent
                                    .filter(|rid| live.iter().any(|e| &e.saved.id == rid))
                                    .or_else(|| live.last().map(|e| e.saved.id.clone()))
                                    .unwrap_or_default();
                                if !target.is_empty() {
                                    ui.terminal_recent.with(|m| {
                                        m.insert(c.id.clone(), target.clone());
                                    });
                                }
                                ui.card_tab.set(target);
                                ui.terminal_focus.update(|v| *v += 1);
                            } else {
                                crate::terminals::create_in_card(ui, c.project_id, c.id, false);
                            }
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
                    canban_ui_kit::press_target()
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
            if c.merged() {
                row = row.child(status("Merged", Tone::Success));
            } else if ui.is_card_merging(&s, &c) {
                row = row.child(live_status("Merging", Tone::Info));
            }
            if let Some(c_active) = Some(&c).filter(|c| c.active() && !c.merged()) {
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
                        .tooltip(hint_tooltip(branch.clone()))
                        .child(
                            canban_ui_kit::press_target()
                                .min_w(px(0.))
                                .h(px(24.))
                                .px_1p5()
                                .rounded(px(5.))
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .bg(CLEAR)
                                .hover(|st| st.bg(INSET))
                                .focus(|st| st.border_1().border_color(ACCENT))
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
            let attachment_count = s.artifacts.iter().filter(|a| a.card_id == c.id).count();
            if attachment_count > 0 {
                row = row.child(
                    canban_ui_kit::press_target()
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
                            ui.card_tab.set(String::new());
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
            let project = s.projects.iter().find(|p| p.id == c.project_id);
            if ui.page.get() != "review" && quick_merge::eligible(&c, project) {
                let target = project
                    .and_then(|p| p.target_branch.clone())
                    .unwrap_or_else(|| "main".into());
                let card = c.clone();
                let a11y = format!("Quick merge into {target}");
                row = row.child(
                    view()
                        .tooltip(hint_tooltip(format!(
                            "Merge into {target} with the default commit message, without opening the diff"
                        )))
                        .child(
                            compact_action(
                                move || {
                                    if quick_merge::busy(ui) {
                                        "Merging…"
                                    } else {
                                        "Merge"
                                    }
                                },
                                Variant::Secondary,
                                move |_| quick_merge::start(ui, card.clone()),
                            )
                            .accessibility_label(move || Some(a11y.clone().into()))
                            .disabled(move || quick_merge::busy(ui)),
                        ),
                );
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
                        .tooltip(hint_tooltip(reason))
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
        .child(tab_bar(ui))
        .child(quick_merge::confirm_dialog(ui))
}

fn card_tab_title(e: &crate::terminals::Entry) -> String {
    if let Some(name) = &e.saved.name {
        return name.clone();
    }
    if let Some(profile) = e.profile() {
        format!("{profile} · {}", e.display_status())
    } else {
        e.title()
    }
}

fn tab_bar(ui: Ui) -> impl IntoElement {
    when(
        move || {
            ui.page.get() != "review"
                && !ui.card_terminal_open.get()
                && ui.current_card().is_some_and(|c| {
                    !crate::terminals::card_live_entries(&ui.snapshot.get(), &c.id).is_empty()
                })
        },
        move || {
            view()
                .mt_1()
                .mx(px(-12.))
                .mb(px(-8.))
                .h(px(34.))
                .border_t_1()
                .border_color(BORDER)
                .bg(INSET)
                .flex_row()
                .items_center()
                .gap_0()
                .child(
                    scroll()
                        .horizontal()
                        .scrollbar(false)
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .child(dynamic(move || {
                            let Some(card) = ui.current_card() else {
                                return view();
                            };
                            let s = ui.snapshot.get();
                            let live = crate::terminals::card_live_entries(&s, &card.id);
                            let selected_tab = ui.card_tab.get();
                            let card_selected = selected_tab.is_empty()
                                || !live.iter().any(|e| e.saved.id == selected_tab);
                            let unread = ui
                                .reminder_counts
                                .with(|all| all.get(&card.id).copied().unwrap_or_default());

                            let mut strip = view().h_full().flex_row().items_center().gap_0();
                            strip = strip.child(
                                view()
                                    .h_full()
                                    .focusable(true)
                                    .cursor_pointer()
                                    .px_3()
                                    .flex_shrink_0()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .bg(if card_selected { SURFACE } else { CLEAR })
                                    .hover(move |st| {
                                        st.bg(if card_selected { SURFACE } else { CARD_HOVER })
                                    })
                                    .border_r_1()
                                    .border_color(BORDER)
                                    .role(AccessibilityRole::Tab)
                                    .accessibility_state(move || nsh_core::AccessibilityState {
                                        selected: card_selected,
                                        ..Default::default()
                                    })
                                    .label("Card tab")
                                    .child(label("Card").font_weight(if card_selected {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    }))
                                    .child(reminder_count(
                                        move || unread.total,
                                        move || unread.pending > 0,
                                    ))
                                    .on_press(move |_: &nsh_core::PressEvent| {
                                        ui.card_tab.set(String::new());
                                    }),
                            );

                            for e in live {
                                let id = e.saved.id.clone();
                                let close_id = id.clone();
                                let menu_id = id.clone();
                                let rename_id = id.clone();
                                let card_id = card.id.clone();
                                let is_sel = selected_tab == id;
                                let text = card_tab_title(&e);
                                let hint = e.hint();
                                let mark = e.provider().map(|(ic, _)| ic).unwrap_or(Icon::Terminal);
                                let dot_color = if e.needs_attention() {
                                    ATTENTION
                                } else if e.ended() || e.inactive() {
                                    MUTED
                                } else {
                                    SUCCESS
                                };
                                strip = strip.child(
                                    view()
                                        .h_full()
                                        .max_w(px(230.))
                                        .flex_shrink_0()
                                        .flex_row()
                                        .items_center()
                                        .gap_0()
                                        .pr_0p5()
                                        .bg(if is_sel { SURFACE } else { CLEAR })
                                        .hover(move |st| {
                                            st.bg(if is_sel { SURFACE } else { CARD_HOVER })
                                        })
                                        .border_r_1()
                                        .border_color(BORDER)
                                        .tooltip(
                                            hint_tooltip(hint.clone()).placement(Placement::Bottom),
                                        )
                                        .child(
                                            view()
                                                .h_full()
                                                .focusable(true)
                                                .cursor_pointer()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .overflow_hidden()
                                                .px_2p5()
                                                .bg(CLEAR)
                                                .flex_row()
                                                .items_center()
                                                .gap_1p5()
                                                .role(AccessibilityRole::Tab)
                                                .accessibility_state(move || {
                                                    nsh_core::AccessibilityState {
                                                        selected: is_sel,
                                                        ..Default::default()
                                                    }
                                                })
                                                .label(format!("Tab {text}"))
                                                .child(
                                                    view()
                                                        .w(px(5.))
                                                        .h(px(5.))
                                                        .rounded(px(3.))
                                                        .bg(dot_color)
                                                        .flex_shrink_0()
                                                        .pointer_events(PointerEvents::None),
                                                )
                                                .child(
                                                    icon(mark)
                                                        .w(px(13.))
                                                        .h(px(13.))
                                                        .flex_shrink_0(),
                                                )
                                                .child(
                                                    label(text.clone())
                                                        .font_weight(if is_sel {
                                                            FontWeight::SEMIBOLD
                                                        } else {
                                                            FontWeight::NORMAL
                                                        })
                                                        .truncate()
                                                        .min_w(px(0.)),
                                                )
                                                .on_press_down(
                                                    move |event: &nsh_core::PressEvent| {
                                                        if event.clicks >= 2 {
                                                            ui.terminal_rename
                                                                .set(Some(rename_id.clone()));
                                                        }
                                                    },
                                                )
                                                .on_context_menu(
                                                    move |event: &nsh_core::ContextMenuEvent| {
                                                        ui.spawn(crate::terminals::tabs::menu(
                                                            ui,
                                                            menu_id.clone(),
                                                            Some(event.page_position),
                                                        ));
                                                    },
                                                )
                                                .on_press(move |_: &nsh_core::PressEvent| {
                                                    ui.terminal_recent.with(|m| {
                                                        m.insert(card_id.clone(), id.clone());
                                                    });
                                                    ui.card_tab.set(id.clone());
                                                    ui.terminal_focus.update(|v| *v += 1);
                                                }),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            format!("Close terminal {}", e.title()),
                                            move |_| crate::terminals::close_entry(ui, &close_id),
                                        )),
                                );
                            }
                            strip
                        })),
                )
                .child(view().px_1().flex_shrink_0().child(chrome_button_disabled(
                    Icon::Plus,
                    "New terminal",
                    move || {
                        ui.pending.get()
                            || ui.current_card().is_none_or(|c| c.archived_at.is_some())
                    },
                    move |_| {
                        if let Some(c) = ui.current_card() {
                            crate::terminals::create_in_card(ui, c.project_id, c.id, false);
                        }
                    },
                )))
        },
    )
}
