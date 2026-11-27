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
    let project = ui
        .index
        .with_untracked(|idx| idx.projects.get(&card.project_id).cloned());
    let snapshot_merging = ui
        .card_status
        .with_untracked(|cs| cs.get(&card.id).is_some_and(|st| st.is_merging));
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
    let project = ui
        .index
        .with_untracked(|idx| idx.projects.get(&card.project_id).cloned());
    let snapshot_merging = ui
        .card_status
        .with_untracked(|cs| cs.get(&card.id).is_some_and(|st| st.is_merging));
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
    let btn_model = computed(move || {
        let cid = ui.card_id();
        let live = if cid.is_empty() {
            Vec::new()
        } else {
            crate::terminals::card_live_entries_for_ui(ui, &cid)
        };
        let tint = crate::components::card_terminal_icon_tint(&live, selected.get());
        let card = ui.current_card();
        let assignee = card.as_ref().and_then(|c| c.assignee.clone());
        if !live.is_empty() {
            let active_prof = crate::card::terminal::select_entry_for_card(ui, &cid)
                .and_then(|e| e.profile().map(str::to_owned))
                .or_else(|| live.iter().find_map(|e| e.profile().map(str::to_owned)));
            let tip = match active_prof {
                Some(p) => format!(
                    "Toggle Terminal · @{p} · {}",
                    crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
                ),
                None => format!(
                    "Toggle Terminal · {}",
                    crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
                ),
            };
            (Icon::Terminal, tint, tip)
        } else if let Some(aid) = assignee {
            let ic = ui
                .profiles
                .with(|ps| {
                    ps.iter()
                        .find(|p| p.id == aid)
                        .map(|p| crate::profiles::provider(p).0)
                })
                .unwrap_or(Icon::Agent);
            let tip = format!(
                "Open @{aid} · {}",
                crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
            );
            (ic, ACCENT, tip)
        } else {
            (
                Icon::Terminal,
                tint,
                format!(
                    "Open Terminal · {}",
                    crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
                ),
            )
        }
    });
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(hint_tooltip(move || btn_model.with(|m| m.2.clone())).placement(Placement::Bottom))
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
                .accessibility_label(move || Some(btn_model.with(|m| m.2.clone()).into()))
                .child(dynamic(move || {
                    Some(btn_model.with(|m| icon_tinted(m.0, m.1).w(px(16.)).h(px(16.))))
                }))
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
                .any(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active())
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
                .filter(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active())
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
                r.card_id.as_deref() == Some(card.id.as_str())
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
#[derive(Clone, Debug, PartialEq, Eq)]
enum HeaderTopModel {
    GitHub {
        is_pr: bool,
        repository: String,
        number: u64,
        state: String,
        url: String,
        source_kind: String,
        gh_connected: bool,
    },
    Local {
        crumb: String,
        schedule: Option<(String, String, u64, String)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StagePickerModel {
    card_id: String,
    active: bool,
    stage_id: String,
    stage_name: String,
    stages: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResolvedOrMergingModel {
    Resolved(Option<String>),
    Merging,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HeaderWorkspaceModel {
    Branch(String),
    NotCreated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HeaderReviewModel {
    PrActive {
        card_id: String,
        profile: String,
        started_at: Option<u64>,
        in_review_stage: bool,
    },
    PrDraft {
        card_id: String,
        count: usize,
    },
    PrIdle {
        card_id: String,
        default_prof: String,
        in_review_stage: bool,
    },
    LocalDiff {
        in_review_stage: bool,
        is_review_stage_id: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HeaderResolutionModel {
    QuickMerge { target: String },
    Resolve,
    Reopen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HeaderAdvanceModel {
    card_id: String,
    target_stage_id: String,
    button_label: String,
    a11y_label: String,
    tooltip: String,
    has_automation: bool,
    primary: bool,
}

pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let summary = changes::observe(ui);
    let is_review = computed(move || ui.page.get() == "review");
    let show_back = computed(move || {
        ui.page.get() == "card"
            && (ui.narrow.get() || ui.card_standalone.get() || ui.card_terminal_open.get())
    });
    let show_expand = computed(move || !is_review.get() && !ui.card_terminal_open.get());

    let top_model = computed(move || {
        let c = ui.current_card()?;
        if let Some(source) = c.github_issue() {
            let state = ui
                .index
                .with(|idx| {
                    idx.github_issue(&c.project_id, source.repository_id, source.issue_id)
                        .map(|i| i.state.clone())
                })
                .unwrap_or_else(|| "Status unavailable".into());
            let gh_connected = ui.snapshot.with(|s| {
                ui.github
                    .gh_status
                    .with(|gh| crate::github::active_login_from(s, gh).is_some())
            });
            Some(HeaderTopModel::GitHub {
                is_pr: c.is_github_pr(),
                repository: source.repository.clone(),
                number: source.number,
                state,
                url: source.url.clone(),
                source_kind: c.source_kind().into(),
                gh_connected,
            })
        } else {
            let project = ui
                .index
                .with(|idx| idx.projects.get(&c.project_id).map(|p| p.name.clone()))
                .unwrap_or_default();
            let crumb = if c.number == 0 {
                format!("{project} / Local")
            } else {
                format!("{project} / Card #{}", c.number)
            };
            let schedule = c.schedule_source.as_ref().map(|sched| {
                (
                    sched.schedule_id.clone(),
                    sched.workflow_name.clone(),
                    sched.run_number,
                    sched.cron.clone(),
                )
            });
            Some(HeaderTopModel::Local { crumb, schedule })
        }
    });

    let has_card = computed(move || ui.current_card().is_some());
    let is_merging = computed(move || {
        let Some(c) = ui.current_card() else {
            return false;
        };
        if !c.active() || c.resolved() {
            return false;
        }
        let snapshot_merging = ui
            .card_status
            .with(|cs| cs.get(&c.id).is_some_and(|s| s.is_merging));
        let ui_merging = ui.merging_cards.with(|ids| ids.contains(&c.id));
        snapshot_merging || ui_merging
    });

    let stage_picker_model = computed(move || {
        let c = ui.current_card()?;
        let project_stages = ui.index.with(|idx| {
            idx.projects
                .get(&c.project_id)
                .map(|p| p.workflow.stages.clone())
                .unwrap_or_default()
        });
        let stage_name = project_stages
            .iter()
            .find(|t| t.id == c.stage)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| c.stage.clone());
        let stages = project_stages
            .into_iter()
            .map(|st| (st.id, st.name))
            .collect();
        let active = c.active();
        Some(StagePickerModel {
            card_id: c.id,
            active,
            stage_id: c.stage,
            stage_name,
            stages,
        })
    });

    let resolved_or_merging = computed(move || {
        let c = ui.current_card()?;
        if c.resolved() {
            Some(ResolvedOrMergingModel::Resolved(resolve::tooltip(&c)))
        } else if is_merging.get() {
            Some(ResolvedOrMergingModel::Merging)
        } else {
            None
        }
    });

    let show_hold_auto_merge = computed(move || {
        let Some(c) = ui.current_card() else {
            return false;
        };
        if !c.active() || c.resolved() || !c.hold_auto_merge {
            return false;
        }
        ui.index
            .with(|idx| idx.project_has_deliver.contains(&c.project_id))
    });

    let sync_mark_model = computed(move || {
        let c = ui.current_card()?;
        ui.snapshot.with(|s| s.pending_sync.get(&c.id).copied())
    });

    let workspace_model = computed(move || {
        let c = ui.current_card()?;
        if let Some(branch) = c.branch.as_ref().filter(|b| !b.is_empty()) {
            Some(HeaderWorkspaceModel::Branch(branch.clone()))
        } else if c.worktree.is_none() && !c.is_github_pr() && c.pull_request.is_none() {
            Some(HeaderWorkspaceModel::NotCreated)
        } else {
            None
        }
    });

    let pr_view_model = computed(move || {
        let c = ui.current_card()?;
        ui.snapshot.with(|s| {
            ui.index
                .with(|idx| crate::components::pr_status::from_card(s, idx, &c))
        })
    });

    let attachment_count_model = computed(move || {
        let Some(c) = ui.current_card() else {
            return 0;
        };
        ui.index
            .with(|idx| idx.artifact_counts.get(&c.id).copied().unwrap_or(0))
    });

    let review_model = computed(move || {
        if is_review.get() {
            return None;
        }
        let c = ui.current_card()?;
        let project = ui
            .index
            .with(|idx| idx.projects.get(&c.project_id).cloned());
        let stage_require_review = project
            .as_ref()
            .and_then(|p| p.workflow.stages.iter().find(|t| t.id == c.stage))
            .is_some_and(|t| t.require_human_review);
        let in_review_stage = c.stage == "review" || stage_require_review;
        let is_pr = crate::review::card_has_pr(&c);
        let can_review_diff = in_review_stage
            || c.worktree.is_some()
            || c.branch.as_ref().is_some_and(|b| !b.is_empty())
            || is_pr;

        if c.active() && !c.resolved() && is_pr {
            let (review_draft, active_review) = ui.snapshot.with(|s| {
                let d = ui.index.with(|idx| idx.review_draft(&c.id).cloned());
                let r = crate::review::active_review_run(s, &c.id);
                (d, r)
            });
            if let Some((profile, started_at)) = active_review {
                Some(HeaderReviewModel::PrActive {
                    card_id: c.id,
                    profile,
                    started_at,
                    in_review_stage,
                })
            } else if let Some(d) = review_draft {
                Some(HeaderReviewModel::PrDraft {
                    card_id: c.id,
                    count: d.comments.len(),
                })
            } else {
                let _ = ui.default_agent.get();
                let default_prof = crate::sessions::default_review_profile(ui);
                Some(HeaderReviewModel::PrIdle {
                    card_id: c.id,
                    default_prof,
                    in_review_stage,
                })
            }
        } else if can_review_diff
            && (in_review_stage || c.branch.as_ref().is_none_or(|b| b.is_empty()))
        {
            Some(HeaderReviewModel::LocalDiff {
                in_review_stage,
                is_review_stage_id: c.stage == "review",
            })
        } else {
            None
        }
    });

    let resolution_model = computed(move || {
        if is_review.get() {
            return None;
        }
        let c = ui.current_card()?;
        let project = ui
            .index
            .with(|idx| idx.projects.get(&c.project_id).cloned());
        let is_pr = crate::review::card_has_pr(&c);
        if quick_merge::eligible(&c, project.as_ref()) {
            let target = project
                .as_ref()
                .and_then(|p| p.target_branch.clone())
                .unwrap_or_else(|| "main".into());
            Some(HeaderResolutionModel::QuickMerge { target })
        } else if c.active() && !c.resolved() && !is_pr {
            Some(HeaderResolutionModel::Resolve)
        } else if c.active() && c.resolved() && !c.merged() {
            Some(HeaderResolutionModel::Reopen)
        } else {
            None
        }
    });

    let show_continue_workflow = computed(move || {
        let Some(c) = ui.current_card() else {
            return false;
        };
        let stage_require_review = ui.index.with(|idx| {
            idx.projects
                .get(&c.project_id)
                .and_then(|p| p.workflow.stages.iter().find(|t| t.id == c.stage))
                .is_some_and(|t| t.require_human_review)
        });
        let in_review_stage = c.stage == "review" || stage_require_review;
        !in_review_stage && c.human_gate.is_some()
    });

    let attention_reason = computed(move || {
        let c = ui.current_card()?;
        c.attention.or_else(|| c.human_gate.map(|g| g.reason))
    });

    let advance_model = computed(move || {
        let c = ui.current_card()?;
        let review_mode = is_review.get();
        let merging = is_merging.get();
        let has_running_agent = ui
            .card_status
            .with(|cs| cs.get(&c.id).is_some_and(|s| !s.running.is_empty()));
        let (project, review_draft) = ui.index.with(|idx| {
            (
                idx.projects.get(&c.project_id).cloned(),
                idx.review_draft(&c.id).cloned(),
            )
        });
        let next = next_stage_for_card(&c, project.as_ref(), merging)?;
        let stage_require_review = project
            .as_ref()
            .and_then(|p| p.workflow.stages.iter().find(|t| t.id == c.stage))
            .is_some_and(|t| t.require_human_review);
        let in_review_stage = c.stage == "review" || stage_require_review;
        let is_pr = crate::review::card_has_pr(&c);
        let can_review_diff = in_review_stage
            || c.worktree.is_some()
            || c.branch.as_ref().is_some_and(|b| !b.is_empty())
            || is_pr;
        let (has_active_workflow, next_automation) = ui.snapshot.with(|s| {
            let active_wf = s
                .workflow_runs
                .iter()
                .any(|run| run.card_id.as_deref() == Some(c.id.as_str()) && run.active());
            let auto = crate::workflows::project_column_automation(s, &c.project_id, &next.id);
            (active_wf, auto)
        });
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
        let tooltip = advance_tooltip(&next.name, next_automation.as_ref(), has_active_workflow);
        Some(HeaderAdvanceModel {
            card_id: c.id,
            target_stage_id: next.id,
            button_label,
            a11y_label,
            tooltip,
            has_automation: next_automation.is_some(),
            primary: matches!(variant, Variant::Primary),
        })
    });

    view()
        .px_3()
        .py_2()
        .gap_1()
        .border_b_1()
        .border_color(BORDER)
        .flex_shrink_0()
        .child(dynamic(move || {
            let Some(model) = top_model.get() else {
                return view().into_element();
            };
            let mut row = view().flex_row().flex_wrap().items_center().gap_2();
            match model {
                HeaderTopModel::GitHub {
                    is_pr,
                    repository,
                    number,
                    state,
                    url,
                    source_kind,
                    gh_connected,
                } => {
                    row = row.child(icon(Icon::GitHub)).child(muted(format!(
                        "{} {}#{} · {}",
                        if is_pr { "PR" } else { "Issue" },
                        repository,
                        number,
                        state
                    )));
                    let gh_ref = canban_types::GitHubReference {
                        repository,
                        kind: source_kind,
                        number: Some(number),
                        sha: None,
                        url: url.clone(),
                    };
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
                }
                HeaderTopModel::Local { crumb, schedule } => {
                    row = row.child(muted(crumb));
                    if let Some((sid, workflow_name, run_number, cron)) = schedule {
                        row = row
                            .child(muted("·"))
                            .child(icon_tinted(Icon::Clock, ACCENT).w(px(13.)).h(px(13.)))
                            .child(muted(format!(
                                "Scheduled · {workflow_name} #{run_number}"
                            )))
                            .child(badge(cron))
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
                                .truncate()
                                .select_text(),
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
        .child(when(
            move || has_card.get(),
            move || {
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.))
                    .child(dynamic(move || {
                        let m = stage_picker_model.get()?;
                        let stage_anchor = element_ref();
                        let stage_a11y = m.stage_name.clone();
                        let stages_for_menu = m.stages;
                        let card_id = m.card_id;
                        let current_stage = m.stage_id;
                        let active = m.active;
                        let disabled = !active || stages_for_menu.is_empty();
                        Some(
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
                                .disabled(disabled)
                                .accessibility_label(move || {
                                    Some(format!("Change stage · {stage_a11y}").into())
                                })
                                .child(muted(format!("{} ▾", m.stage_name)))
                                .on_press(move |_: &()| {
                                    use nsh_core::element::desktop::popup_menu_at;
                                    let stages = stages_for_menu.clone();
                                    let cid_menu = card_id.clone();
                                    let cur_stage = current_stage.clone();
                                    ui.spawn(async move {
                                        let Some(bounds) = stage_anchor.measure().await else {
                                            return;
                                        };
                                        let items = stages
                                            .into_iter()
                                            .map(|(st_id, st_name)| {
                                                let cid = cid_menu.clone();
                                                let target = st_id.clone();
                                                let prefix =
                                                    if st_id == cur_stage { "✓ " } else { "" };
                                                MenuItem::new(format!("{prefix}{st_name}"))
                                                    .disabled(st_id == cur_stage || !active)
                                                    .on_select(move || {
                                                        crate::presenter::move_card(
                                                            ui, &cid, &target,
                                                        )
                                                    })
                                            })
                                            .collect::<Vec<_>>();
                                        popup_menu_at(
                                            items,
                                            point(
                                                bounds.page.x,
                                                bounds.page.y + bounds.frame.size.height,
                                            ),
                                        )
                                        .await;
                                    });
                                }),
                        )
                    }))
                    .child(crate::components::assignee_picker::card_header_assignee_picker(ui))
                    .child(dynamic(move || match resolved_or_merging.get()? {
                        ResolvedOrMergingModel::Resolved(tip) => {
                            let badge = status("Resolved", Tone::Success);
                            if let Some(tip) = tip {
                                Some(view().tooltip(hint_tooltip(tip)).child(badge))
                            } else {
                                Some(view().child(badge))
                            }
                        }
                        ResolvedOrMergingModel::Merging => {
                            Some(view().child(live_status("Merging", Tone::Info)))
                        }
                    }))
                    .child(dynamic(move || {
                        show_hold_auto_merge
                            .get()
                            .then(|| status("Auto-merge held", Tone::Attention))
                    }))
                    .child(dynamic(move || {
                        let mark = sync_mark_model.get()?;
                        let (chip_label, chip_tone) = crate::hosts::sync_chip_spec(mark);
                        Some(status(chip_label, chip_tone))
                    }))
                    .child(dynamic(move || match workspace_model.get()? {
                        HeaderWorkspaceModel::Branch(branch) => {
                            Some(changes::button(ui, branch, summary).into_element())
                        }
                        HeaderWorkspaceModel::NotCreated => {
                            Some(muted("Workspace not created").into_element())
                        }
                    }))
                    .child(dynamic(move || {
                        let info = pr_view_model.get()?;
                        crate::components::pr_status::pill(ui, &info, false)
                    }))
                    .child(dynamic(move || {
                        let attachment_count = attachment_count_model.get();
                        (attachment_count > 0).then(|| {
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
                                })
                        })
                    }))
                    .child(view().flex_1().min_w(px(0.)))
                    .child(dynamic(move || match review_model.get()? {
                        HeaderReviewModel::PrActive {
                            card_id,
                            profile,
                            started_at,
                            ..
                        } => Some(
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
                                        crate::terminals::open_card_quick(ui, &card_id);
                                    }
                                })
                                .child(live_dot(ACCENT))
                                .child(
                                    label(move || {
                                        let elapsed = started_at
                                            .map(|t| {
                                                format!(
                                                    " · {}",
                                                    crate::components::activity::compact_duration(
                                                        ui.runtime_clock.get().saturating_sub(t)
                                                    )
                                                )
                                            })
                                            .unwrap_or_default();
                                        format!("@{profile} reviewing{elapsed}")
                                    })
                                    .text_size(px(11.5))
                                    .text_color(ACCENT),
                                )
                                .into_element(),
                        ),
                        HeaderReviewModel::PrDraft { card_id, .. } => Some(
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
                                    crate::review::start_agent_review(ui, &card_id, None, None);
                                },
                            )
                            .disabled(move || ui.is_method_pending("review.run_agent"))
                            .into_element(),
                        ),
                        HeaderReviewModel::PrIdle {
                            card_id,
                            default_prof,
                            ..
                        } => Some(
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
                                    crate::review::start_agent_review(ui, &card_id, None, None)
                                },
                            )
                            .disabled(move || ui.is_method_pending("review.run_agent"))
                            .into_element(),
                        ),
                        HeaderReviewModel::LocalDiff {
                            in_review_stage,
                            is_review_stage_id,
                        } => Some(
                            compact_action(
                                if in_review_stage { "Review" } else { "Diff" },
                                if is_review_stage_id {
                                    Variant::Primary
                                } else {
                                    Variant::Secondary
                                },
                                move |_| changes::open(ui),
                            )
                            .into_element(),
                        ),
                    }))
                    .child(dynamic(move || match review_model.get()? {
                        HeaderReviewModel::PrDraft { card_id, count } => {
                            let btn_label = if count > 0 {
                                format!("Review Drafts ({count}) →")
                            } else {
                                "Review Ready →".into()
                            };
                            Some(compact_action(btn_label, Variant::Primary, move |_| {
                                crate::review::open_card_pr(ui, &card_id)
                            }))
                        }
                        HeaderReviewModel::PrActive {
                            in_review_stage, ..
                        }
                        | HeaderReviewModel::PrIdle {
                            in_review_stage, ..
                        } => Some(compact_action(
                            "View PR diff",
                            if in_review_stage {
                                Variant::Primary
                            } else {
                                Variant::Secondary
                            },
                            move |_| changes::open(ui),
                        )),
                        HeaderReviewModel::LocalDiff { .. } => None,
                    }))
                    .child(dynamic(move || match resolution_model.get()? {
                        HeaderResolutionModel::QuickMerge { target } => {
                            let a11y = format!("Quick merge into {target}");
                            Some(
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
                                            move |_| {
                                                if let Some(card) = untracked(|| ui.current_card()) {
                                                    quick_merge::start(ui, card);
                                                }
                                            },
                                        )
                                        .accessibility_label(move || Some(a11y.clone().into()))
                                        .disabled(move || quick_merge::busy(ui)),
                                    ),
                            )
                        }
                        HeaderResolutionModel::Resolve => Some(
                            view()
                                .tooltip(hint_tooltip(
                                    "Mark this Card as resolved without merging code (auto-archives tomorrow)",
                                ))
                                .child(
                                    compact_action_with_icon(
                                        Icon::Check,
                                        "Resolve",
                                        Variant::Secondary,
                                        move |_| {
                                            if let Some(card) = untracked(|| ui.current_card()) {
                                                resolve::start(ui, card);
                                            }
                                        },
                                    )
                                    .accessibility_label(|| Some("Mark Card as resolved".into()))
                                    .disabled(move || resolve::busy(ui) || ui.pending.get()),
                                ),
                        ),
                        HeaderResolutionModel::Reopen => Some(
                            view()
                                .tooltip(hint_tooltip("Reopen this resolved Card"))
                                .child(
                                    compact_action_with_icon(
                                        Icon::Undo,
                                        "Reopen",
                                        Variant::Secondary,
                                        move |_| {
                                            if let Some(card) = untracked(|| ui.current_card()) {
                                                resolve::reopen(ui, card);
                                            }
                                        },
                                    )
                                    .accessibility_label(|| Some("Reopen resolved Card".into()))
                                    .disabled(move || resolve::busy(ui) || ui.pending.get()),
                                ),
                        ),
                    }))
                    .child(dynamic(move || {
                        show_continue_workflow.get().then(|| {
                            compact_action("Continue workflow", Variant::Secondary, move |_| {
                                if let Some(c) = untracked(|| ui.current_card()) {
                                    dispatch(
                                        ui,
                                        "card.continue",
                                        json!({"card_id":c.id,"revision":c.revision}),
                                    );
                                }
                            })
                            .disabled(move || ui.is_method_pending("card.continue"))
                        })
                    }))
                    .child(dynamic(move || {
                        let reason = attention_reason.get()?;
                        Some(
                            view().tooltip(hint_tooltip(reason)).child(
                                compact_action("Needs attention", Variant::Secondary, move |_| {
                                    crate::overview::show_card_attention(
                                        ui,
                                        &untracked(|| ui.card_id()),
                                    );
                                })
                                .text_color(ATTENTION),
                            ),
                        )
                    }))
                    .child(dynamic(move || {
                        let m = advance_model.get()?;
                        let variant = if m.primary {
                            Variant::Primary
                        } else {
                            Variant::Secondary
                        };
                        let cid = m.card_id;
                        let target_stage = m.target_stage_id;
                        let a11y_label = m.a11y_label;
                        let btn = if m.has_automation {
                            compact_action_with_icon(
                                Icon::Zap,
                                m.button_label,
                                variant,
                                move |_| {
                                    crate::presenter::move_card(ui, &cid, &target_stage);
                                },
                            )
                        } else {
                            compact_action(m.button_label, variant, move |_| {
                                crate::presenter::move_card(ui, &cid, &target_stage);
                            })
                        }
                        .accessibility_label(move || Some(a11y_label.clone().into()))
                        .disabled(move || ui.is_method_pending("card.move"));
                        Some(
                            view()
                                .tooltip(
                                    tooltip_box()
                                        .placement(Placement::Bottom)
                                        .max_w(px(320.))
                                        .child(
                                            text(m.tooltip)
                                                .text_size(px(CAPTION_SIZE))
                                                .text_color(TEXT),
                                        ),
                                )
                                .child(btn),
                        )
                    }))
            },
        ))
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
