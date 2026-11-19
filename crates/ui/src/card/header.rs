use super::*;
use nsh_core::element::desktop::{MenuItem, popup_menu};
use serde_json::json;

/// Returns the immediate next stage in `project`'s workflow when `card` is
/// active and unresolved, with a later stage available and no merge in progress.
pub(crate) fn next_stage_for_card(
    card: &canban_types::Card,
    project: Option<&canban_types::Project>,
    is_merging: bool,
) -> Option<canban_types::Stage> {
    if !card.active() || card.resolved() || is_merging {
        return None;
    }
    let stages = &project?.workflow.stages;
    stages
        .iter()
        .position(|s| s.id == card.stage)
        .and_then(|idx| stages.get(idx + 1))
        .cloned()
}

pub(crate) fn next_stage_for_ui(ui: Ui, card: &canban_types::Card) -> Option<canban_types::Stage> {
    let (project, snapshot_merging) = ui.index.with_untracked(|idx| {
        (
            idx.projects.get(&card.project_id).cloned(),
            idx.card_status
                .get(&card.id)
                .is_some_and(|st| st.is_merging),
        )
    });
    let ui_merging = ui
        .merging_cards
        .with_untracked(|ids| ids.contains(&card.id));
    next_stage_for_card(card, project.as_ref(), snapshot_merging || ui_merging)
}

pub(crate) fn advance_stage(ui: Ui) {
    let Some(card) = ui.current_card() else {
        ui.notify("Select a Card first");
        return;
    };
    if !card.active() {
        ui.notify("Restore this Card before advancing its stage");
        return;
    }
    if card.resolved() {
        ui.notify("Current Card is already resolved");
        return;
    }
    let (project, snapshot_merging) = ui.index.with_untracked(|idx| {
        (
            idx.projects.get(&card.project_id).cloned(),
            idx.card_status
                .get(&card.id)
                .is_some_and(|st| st.is_merging),
        )
    });
    let ui_merging = ui
        .merging_cards
        .with_untracked(|ids| ids.contains(&card.id));
    if snapshot_merging || ui_merging {
        ui.notify("Current Card is currently merging");
        return;
    }
    let Some(next) = next_stage_for_card(&card, project.as_ref(), false) else {
        ui.notify("Current Card is already in the final stage");
        return;
    };
    crate::presenter::move_card(ui, &card.id, &next.id);
}

fn advance_button_variant(
    has_other_primary: bool,
    has_active_workflow: bool,
    has_running_agent: bool,
    has_human_gate: bool,
    has_attention: bool,
) -> Variant {
    if has_other_primary
        || has_active_workflow
        || has_running_agent
        || has_human_gate
        || has_attention
    {
        Variant::Secondary
    } else {
        Variant::Primary
    }
}

fn advance_tooltip(
    next_name: &str,
    automation: Option<&canban_workflow_engine::source::ColumnAutomation>,
    has_active_workflow: bool,
) -> String {
    let mut sections = Vec::new();
    if let Some(auto) = automation {
        sections.push(format!("Move to {next_name} · ⚡ {}", auto.label));
        if !auto.tooltip.is_empty() {
            sections.push(auto.tooltip.clone());
        }
    } else {
        sections.push(format!("Move to {next_name}"));
    }
    if has_active_workflow {
        sections.push("A workflow run is currently active on this Card.".into());
    }
    sections.join("\n\n")
}

fn close(ui: Ui) {
    ui.close_card();
}
fn terminal_button(ui: Ui) -> View {
    let selected = computed(move || ui.page.get() == "card" && ui.card_terminal_open.get());
    let has_live = computed(move || {
        let cid = ui.card_id();
        !cid.is_empty() && !crate::terminals::card_live_entries_for_ui(ui, &cid).is_empty()
    });
    let icon_tint = computed(move || {
        let cid = ui.card_id();
        let live = if cid.is_empty() {
            Vec::new()
        } else {
            crate::terminals::card_live_entries_for_ui(ui, &cid)
        };
        crate::components::card_terminal_icon_tint(&live, selected.get())
    });
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(format!(
                "Toggle Terminal · {}",
                crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
            ))
            .placement(Placement::Bottom),
        )
        .child(
            canban_ui_kit::press_target()
                .disabled_style(|s| s.opacity(0.4))
                .w(px(30.))
                .h(px(30.))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(move || if selected.get() { SELECTED } else { CLEAR })
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_1().border_color(ACCENT))
                .selected(move || selected.get())
                .disabled(move || {
                    ui.pending.get()
                        || ui.current_card().is_none_or(|c| {
                            c.archived_at.is_some()
                                && !ui.card_terminal_open.get()
                                && !has_live.get()
                        })
                })
                .accessibility_label(|| {
                    Some(
                        format!(
                            "Toggle Terminal · {}",
                            crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
                        )
                        .into(),
                    )
                })
                .child(
                    icon_tinted(Icon::Terminal, move || icon_tint.get().resolve())
                        .w(px(16.))
                        .h(px(16.)),
                )
                .on_press(move |_: &()| {
                    if let Some(c) = ui.current_card() {
                        if ui.page.get_untracked() != "card" {
                            crate::terminals::open_card_quick(ui, &c.id);
                            return;
                        }
                        crate::card::terminal::toggle(ui);
                    }
                }),
        )
}

fn menu(ui: Ui, draft: Signal<Draft>, reading: Reading) {
    let Some(card) = ui.current_card() else {
        return;
    };
    ui.spawn(async move {
        let mut items = Vec::new();
        if card.branch.as_ref().is_some_and(|b| !b.is_empty()) {
            items.push(
                MenuItem::new("Copy Card branch").on_select(move || changes::copy_branch(ui)),
            );
            items.push(MenuItem::new("Refresh Card diff").on_select(move || changes::refresh(ui)));
        }
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
                        .disabled(
                            active
                                || !card.active()
                                || card.resolved()
                                || card.human_gate.is_some(),
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
        if has_deliver_in_project && card.active() && !card.resolved() {
            if card.worktree.is_some() {
                let cid = card.id.clone();
                let fallback_rev = card.revision;
                items.push(
                    MenuItem::new("Retry auto-merge")
                        .disabled(active_non_deliver)
                        .on_select(move || {
                            let rev = ui
                                .snapshot
                                .with_untracked(|s| {
                                    s.cards.iter().find(|c| c.id == cid).map(|c| c.revision)
                                })
                                .unwrap_or(fallback_rev);
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
                .on_select(move || crate::presenter::hold_auto_merge(ui, &cid, rev, !held)),
            );
        }
        items.push(MenuItem::separator());
        items.extend(crate::card_actions::items(ui, card));
        popup_menu(items).await;
    });
}
pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let summary = changes::observe(ui);
    let is_review = computed(move || ui.page.get() == "review");
    let show_back = computed(move || {
        ui.page.get() == "card"
            && (ui.narrow.get() || ui.card_standalone.get() || ui.card_terminal_open.get())
    });
    let show_expand = computed(move || !is_review.get() && !ui.card_terminal_open.get());
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
                    .index
                    .with(|idx| {
                        idx.github_issue(&c.project_id, source.repository_id, source.issue_id)
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
                let gh_ref = canban_types::GitHubReference {
                    repository: source.repository.clone(),
                    kind: c.source_kind().into(),
                    number: Some(source.number),
                    sha: None,
                    url: source.url.clone(),
                };
                let gh_connected = ui.snapshot.with(|s| {
                    ui.github
                        .gh_status
                        .with(|gh| crate::github::active_login_from(s, gh).is_some())
                });
                row = row.child(view().flex_1());
                if gh_connected {
                    row = row.child(compact_action(
                        "GitHub discussion",
                        Variant::Quiet,
                        move |_| crate::github::open_section(ui, gh_ref.clone(), "discussion"),
                    ));
                }
                row = row.child(compact_action(
                    "Open in GitHub ↗",
                    Variant::Quiet,
                    move |_| crate::github::open_url(ui, url.clone()),
                ));
            } else {
                let project = ui
                    .index
                    .with(|idx| idx.projects.get(&c.project_id).map(|p| p.name.clone()))
                    .unwrap_or_default();
                row = row.child(muted(format!("{project} / Card #{}", c.number)));
                if let Some(sched) = &c.schedule_source {
                    let sid = sched.schedule_id.clone();
                    row = row
                        .child(muted("·"))
                        .child(icon_tinted(Icon::Clock, ACCENT).w(px(13.)).h(px(13.)))
                        .child(muted(format!(
                            "Scheduled · {} #{}",
                            sched.workflow_name, sched.run_number
                        )))
                        .child(badge(sched.cron.clone()))
                        .child(view().flex_1())
                        .child(compact_action(
                            "View Schedule →",
                            Variant::Quiet,
                            move |_| {
                                crate::workflows::open_schedules(ui, Some(sid.clone()));
                            },
                        ));
                }
            }
            row.into_element()
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(when(
                    move || show_back.get(),
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
                    move || show_expand.get(),
                    move || crate::editor::expand(ui),
                ))
                .child(when(
                    move || ui.current_card().is_some_and(|c| c.active() && c.resolved()),
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
                .child(terminal_button(ui))
                .child(chrome_button(Icon::More, "Card actions", move |_| {
                    menu(ui, draft, reading)
                }))
                .child(when(
                    move || !is_review.get(),
                    move || chrome_button(Icon::Close, "Close Card details", move |_| close(ui)),
                )),
        )
        .child(dynamic(move || {
            let Some(c) = ui.current_card() else {
                return view();
            };
            let review_mode = is_review.get();
            let ui_merging = c.active()
                && !c.resolved()
                && ui.merging_cards.with(|ids| ids.contains(&c.id));
            let (
                project,
                has_deliver_in_project,
                attachment_count,
                snapshot_merging,
                has_running_agent,
            ) = ui.index.with(|idx| {
                let st = idx.card_status.get(&c.id);
                (
                    idx.projects.get(&c.project_id).cloned(),
                    idx.project_has_deliver.contains(&c.project_id),
                    idx.artifact_counts.get(&c.id).copied().unwrap_or(0),
                    st.is_some_and(|s| s.is_merging),
                    st.is_some_and(|s| !s.running.is_empty()),
                )
            });
            let project_stages = project
                .as_ref()
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
            if c.resolved() {
                let badge = status("Resolved", Tone::Success);
                if let Some(tip) = resolve::tooltip(&c) {
                    row = row.child(view().tooltip(hint_tooltip(tip)).child(badge));
                } else {
                    row = row.child(badge);
                }
            } else if snapshot_merging || ui_merging {
                row = row.child(live_status("Merging", Tone::Info));
            }
            if c.active() && !c.resolved() && has_deliver_in_project && c.hold_auto_merge {
                row = row.child(status("Auto-merge held", Tone::Attention));
            }
            if let Some(branch) = c.branch.clone().filter(|b| !b.is_empty()) {
                row = row.child(changes::button(ui, branch, summary));
            } else if c.worktree.is_none() && !c.is_github_pr() && c.pull_request.is_none() {
                row = row.child(muted("Workspace not created"));
            }
            let pr_view = ui.snapshot.with(|s| {
                ui.index
                    .with(|idx| crate::components::pr_status::from_card(s, idx, &c))
            });
            if let Some(pr_pill) = pr_view
                .as_ref()
                .and_then(|info| crate::components::pr_status::pill(ui, info, false))
            {
                row = row.child(pr_pill);
            }

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
                            draft.update(|d| d.details = !d.details);
                            reading.top();
                        }),
                );
            }
            row = row.child(view().flex_1().min_w(px(0.)));
            let in_review_stage =
                c.stage == "review" || stage.is_some_and(|t| t.require_human_review);
            let is_pr = crate::review::card_has_pr(&c);
            let can_review_diff = in_review_stage
                || c.worktree.is_some()
                || c.branch.as_ref().is_some_and(|b| !b.is_empty())
                || is_pr;
            let next_stage =
                next_stage_for_card(&c, project.as_ref(), snapshot_merging || ui_merging);
            let (review_draft, active_review, has_active_workflow, next_automation) =
                ui.snapshot.with(|s| {
                    let d = ui.index.with(|idx| idx.review_draft(&c.id).cloned());
                    let r = crate::review::active_review_run(s, &c.id);
                    let active_wf = s.workflow_runs.iter().any(|run| run.card_id == c.id && run.active());
                    let auto = next_stage.as_ref().and_then(|next| {
                        let files: Vec<_> = s
                            .workflow_sources
                            .iter()
                            .filter(|f| f.project_id == c.project_id)
                            .cloned()
                            .collect();
                        canban_workflow_engine::source::column_automation(
                            &next.id,
                            &files,
                            &s.workflow_revisions,
                            &s.event_subscriptions,
                        )
                    });
                    (d, r, active_wf, auto)
                });
            if !review_mode && c.active() && !c.resolved() && is_pr {
                if let Some((prof, started_at)) = active_review {
                    let now_ms = ui.runtime_clock.get();
                    let elapsed = started_at
                        .map(|t| {
                            format!(
                                " · {}",
                                crate::components::activity::compact_duration(
                                    now_ms.saturating_sub(t)
                                )
                            )
                        })
                        .unwrap_or_default();
                    let cid_term = c.id.clone();
                    row = row.child(
                        canban_ui_kit::press_target()
                            .h(px(26.))
                            .px_2()
                            .rounded(px(13.))
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .bg(SELECTED)
                            .hover(|st| st.bg(CARD_HOVER))
                            .on_press(move |_: &()| {
                                if ui.page.get_untracked() == "card" {
                                    crate::card::terminal::toggle(ui);
                                } else {
                                    crate::terminals::open_card_quick(ui, &cid_term);
                                }
                            })
                            .child(live_dot(ACCENT))
                            .child(
                                label(format!("@{prof} reviewing{elapsed}"))
                                    .text_size(px(11.5))
                                    .text_color(ACCENT),
                            ),
                    );
                } else if let Some(ref d) = review_draft {
                    let count = d.comments.len();
                    let cid_rerun = c.id.clone();
                    let cid_open = c.id.clone();
                    let btn_label = if count > 0 {
                        format!("Review Drafts ({count}) →")
                    } else {
                        "Review Ready →".into()
                    };
                    row = row
                        .child(
                            compact_action(
                                move || {
                                    if ui.is_method_pending("review.run_agent") {
                                        "Starting…"
                                    } else {
                                        "↻ Re-run"
                                    }
                                },
                                Variant::Quiet,
                                move |_| {
                                    crate::review::start_agent_review(ui, &cid_rerun, None, None);
                                },
                            )
                            .disabled(move || ui.is_method_pending("review.run_agent")),
                        )
                        .child(compact_action(
                            btn_label,
                            Variant::Primary,
                            move |_| crate::review::open_card_pr(ui, &cid_open),
                        ));
                } else {
                    let cid_run = c.id.clone();
                    let default_prof = crate::sessions::default_review_profile(ui);
                    row = row.child(
                        compact_action(
                            move || {
                                if ui.is_method_pending("review.run_agent") {
                                    format!("Starting @{default_prof}…")
                                } else {
                                    format!("Agent Review (@{default_prof})")
                                }
                            },
                            Variant::Secondary,
                            move |_| {
                                crate::review::start_agent_review(ui, &cid_run, None, None)
                            },
                        )
                        .disabled(move || ui.is_method_pending("review.run_agent")),
                    );
                }
                if review_draft.is_none() {
                    row = row.child(compact_action(
                        "View PR diff",
                        if in_review_stage {
                            Variant::Primary
                        } else {
                            Variant::Secondary
                        },
                        move |_| changes::open(ui),
                    ));
                }
            } else if !review_mode
                && can_review_diff
                && (in_review_stage || c.branch.as_ref().is_none_or(|b| b.is_empty()))
            {
                row = row.child(compact_action(
                    if in_review_stage { "Review" } else { "Diff" },
                    if c.stage == "review" {
                        Variant::Primary
                    } else {
                        Variant::Secondary
                    },
                    move |_| changes::open(ui),
                ));
            }
            let quick_merge_visible = !review_mode && quick_merge::eligible(&c, project.as_ref());
            if quick_merge_visible {
                let target = project
                    .as_ref()
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
            } else if !review_mode && c.active() && !c.resolved() && !is_pr {
                let card = c.clone();
                row = row.child(
                    view()
                        .tooltip(hint_tooltip(
                            "Mark this Card as resolved without merging code (auto-archives tomorrow)",
                        ))
                        .child(
                            compact_action_with_icon(
                                Icon::Check,
                                "Resolve",
                                Variant::Secondary,
                                move |_| resolve::start(ui, card.clone()),
                            )
                            .accessibility_label(|| Some("Mark Card as resolved".into()))
                            .disabled(move || resolve::busy(ui) || ui.pending.get()),
                        ),
                );
            } else if !review_mode && c.active() && c.resolved() && !c.merged() {
                let card = c.clone();
                row = row.child(
                    view()
                        .tooltip(hint_tooltip("Reopen this resolved Card"))
                        .child(
                            compact_action_with_icon(
                                Icon::Undo,
                                "Reopen",
                                Variant::Secondary,
                                move |_| resolve::reopen(ui, card.clone()),
                            )
                            .accessibility_label(|| Some("Reopen resolved Card".into()))
                            .disabled(move || resolve::busy(ui) || ui.pending.get()),
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
            if let Some(reason) = c.attention.clone().or_else(|| c.human_gate.as_ref().map(|g| g.reason.clone())) {
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
            if let Some(next) = next_stage {
                let pr_primary = !review_mode
                    && c.active()
                    && !c.resolved()
                    && is_pr
                    && (review_draft.is_some() || in_review_stage);
                let diff_primary = !review_mode
                    && !is_pr
                    && can_review_diff
                    && (in_review_stage || c.branch.as_ref().is_none_or(|b| b.is_empty()))
                    && c.stage == "review";
                let variant = advance_button_variant(
                    pr_primary || diff_primary,
                    has_active_workflow,
                    has_running_agent,
                    c.human_gate.is_some(),
                    c.attention.is_some(),
                );
                let button_label = format!("{} →", next.name);
                let a11y_label = match &next_automation {
                    Some(auto) => format!("Move to {} · {}", next.name, auto.label),
                    None => format!("Move to {}", next.name),
                };
                let cid = c.id.clone();
                let target_stage = next.id.clone();
                let btn = if next_automation.is_some() {
                    compact_action_with_icon(Icon::Zap, button_label, variant, move |_| {
                        crate::presenter::move_card(ui, &cid, &target_stage);
                    })
                } else {
                    compact_action(button_label, variant, move |_| {
                        crate::presenter::move_card(ui, &cid, &target_stage);
                    })
                }
                .accessibility_label(move || Some(a11y_label.clone().into()))
                .disabled(move || ui.is_method_pending("card.move"));
                let tip = advance_tooltip(&next.name, next_automation.as_ref(), has_active_workflow);
                row = row.child(
                    view()
                        .tooltip(
                            tooltip_box()
                                .placement(Placement::Bottom)
                                .max_w(px(320.))
                                .child(
                                    text(tip)
                                        .text_size(px(CAPTION_SIZE))
                                        .text_color(TEXT),
                                ),
                        )
                        .child(btn),
                );
            }
            row
        }))
        .child(quick_merge::confirm_dialog(ui))
        .child(resolve::confirm_dialog(ui))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_project(stages: Vec<(&str, &str)>) -> canban_types::Project {
        let mut project = canban_types::Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            location: None,
            id: "p1".into(),
            name: "Canban".into(),
            directory: "/example/canban".into(),
            workflow: canban_types::Workflow::default(),
            target_branch: None,
            created_at: 1,
        };
        if !stages.is_empty() {
            project.workflow.stages = stages
                .into_iter()
                .map(|(id, name)| canban_types::Stage {
                    id: id.into(),
                    name: name.into(),
                    execute: false,
                    profile: None,
                    instruction: String::new(),
                    on_completed: None,
                    require_human_review: false,
                })
                .collect();
        }
        project
    }

    fn sample_card(stage: &str) -> canban_types::Card {
        serde_json::from_value(json!({
            "id": "c1",
            "project_id": "p1",
            "number": 131,
            "title": "Advance stage button",
            "body": "",
            "stage": stage,
            "revision": 1,
            "created_at": 1
        }))
        .unwrap()
    }

    #[test]
    fn next_stage_resolves_ordered_stages_and_respects_terminal_states() {
        let project = sample_project(vec![]);
        assert_eq!(
            next_stage_for_card(&sample_card("backlog"), Some(&project), false).map(|s| s.id),
            Some("develop".into())
        );
        assert_eq!(
            next_stage_for_card(&sample_card("develop"), Some(&project), false).map(|s| s.id),
            Some("review".into())
        );
        assert_eq!(
            next_stage_for_card(&sample_card("review"), Some(&project), false),
            None
        );
        assert_eq!(
            next_stage_for_card(&sample_card("unknown"), Some(&project), false),
            None
        );

        let custom = sample_project(vec![
            ("design", "Design"),
            ("develop", "Develop"),
            ("qa", "QA"),
            ("review", "Review"),
        ]);
        assert_eq!(
            next_stage_for_card(&sample_card("develop"), Some(&custom), false).map(|s| s.name),
            Some("QA".into())
        );

        assert_eq!(
            next_stage_for_card(&sample_card("backlog"), Some(&project), true),
            None
        );
        let mut archived = sample_card("backlog");
        archived.archived_at = Some(10);
        assert_eq!(next_stage_for_card(&archived, Some(&project), false), None);

        let mut merged = sample_card("develop");
        merged.delivery = Some(canban_types::Delivery {
            target_branch: "main".into(),
            commit: "abc1234".into(),
            request_key: "req-1".into(),
            merged_at: 10,
        });
        assert_eq!(next_stage_for_card(&merged, Some(&project), false), None);

        let mut resolved = sample_card("develop");
        resolved.resolution = Some(canban_types::CardResolution {
            resolved_at: 10,
            kind: "manual".into(),
            reason: None,
            actor: "user".into(),
        });
        assert_eq!(next_stage_for_card(&resolved, Some(&project), false), None);
    }

    #[test]
    fn advance_button_variant_and_tooltip_reflect_automation_and_activity() {
        assert!(matches!(
            advance_button_variant(false, false, false, false, false),
            Variant::Primary
        ));
        assert!(matches!(
            advance_button_variant(true, false, false, false, false),
            Variant::Secondary
        ));
        assert!(matches!(
            advance_button_variant(false, true, false, false, false),
            Variant::Secondary
        ));
        assert!(matches!(
            advance_button_variant(false, false, true, false, false),
            Variant::Secondary
        ));
        assert!(matches!(
            advance_button_variant(false, false, false, true, false),
            Variant::Secondary
        ));
        assert!(matches!(
            advance_button_variant(false, false, false, false, true),
            Variant::Secondary
        ));

        assert_eq!(advance_tooltip("QA", None, false), "Move to QA");
        let auto = canban_workflow_engine::source::ColumnAutomation {
            label: "Auto develop".into(),
            tooltip: "Development (development.yaml)\n• Runs @developer".into(),
        };
        assert_eq!(
            advance_tooltip("Develop", Some(&auto), true),
            "Move to Develop · ⚡ Auto develop\n\nDevelopment (development.yaml)\n• Runs @developer\n\nA workflow run is currently active on this Card."
        );
    }
}
