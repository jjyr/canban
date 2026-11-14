use super::*;
use canban_types::{MergeStrategy, WorkflowDefinition, WorkflowStep};
use serde_json::json;

fn ordered_steps(definition: &WorkflowDefinition, root: Option<&str>) -> Vec<String> {
    let mut order = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    fn walk(
        id: &str,
        definition: &WorkflowDefinition,
        seen: &mut std::collections::BTreeSet<String>,
        order: &mut Vec<String>,
    ) {
        if !seen.insert(id.to_owned()) {
            return;
        }
        let Some(step) = definition.steps.get(id) else {
            return;
        };
        if !matches!(step, WorkflowStep::Complete) {
            order.push(id.to_owned());
        }
        if let Some(next) = step.next() {
            walk(next, definition, seen, order);
        }
        if let WorkflowStep::Condition {
            then, otherwise, ..
        } = step
        {
            walk(then, definition, seen, order);
            walk(otherwise, definition, seen, order);
        }
    }
    if let Some(root) = root {
        walk(root, definition, &mut seen, &mut order);
    }
    for id in definition.entries.values() {
        walk(id, definition, &mut seen, &mut order);
    }
    for id in definition.steps.keys() {
        walk(id, definition, &mut seen, &mut order);
    }
    order
}

fn step_badge_label(id: &str, step: &WorkflowStep) -> String {
    match step {
        WorkflowStep::Agent { profile, .. } => {
            let role = id.strip_prefix("agent:").unwrap_or(id);
            if let Some(p) = profile {
                format!("{role} (@{p})")
            } else {
                role.to_owned()
            }
        }
        WorkflowStep::Script { shell, .. } => {
            let name = id.strip_prefix("script:").unwrap_or(id);
            match shell.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(s) => format!("Script · {name} ({s})"),
                None => format!("Script · {name}"),
            }
        }
        WorkflowStep::Move { column, .. } => format!("→ {column}"),
        WorkflowStep::Approval { .. } => "Approval".into(),
        WorkflowStep::Deliver {
            strategy,
            sync_upstream,
            ..
        } => {
            let strat = match strategy {
                MergeStrategy::Rebase => "Queue merge",
                MergeStrategy::Squash => "Queue squash",
                MergeStrategy::Merge => "Queue merge commit",
            };
            if *sync_upstream {
                format!("{strat} · Sync")
            } else {
                format!("{strat} · Local")
            }
        }
        WorkflowStep::Condition { .. } => "Rule check".into(),
        WorkflowStep::Notify { .. } => "Notify".into(),
        WorkflowStep::Admit { agent_slot, .. } => {
            if *agent_slot {
                "Wait for free Agent".into()
            } else {
                "Plan-ahead limit".into()
            }
        }
        WorkflowStep::Complete => "Done".into(),
    }
}

pub fn render(ui: Ui) -> impl IntoElement {
    let rework_open = signal(false);
    let rework_text = signal(String::new());
    dynamic(move || {
        let card_id = ui.card_id();
        let card_opt = ui.current_card();
        let (
            runs,
            has_any_active_run,
            idle_deliver_workflow,
            had_previous_run,
            att_profiles,
            queue_info,
        ) = ui.snapshot.with(|s| {
            let att_profiles: std::collections::BTreeMap<String, (String, Option<u64>, bool)> = s
                .terminals
                .iter()
                .filter_map(|t| {
                    let a = t.agent.as_ref()?;
                    if a.card_id != card_id {
                        return None;
                    }
                    let started_at = crate::components::activity::agent_started_at(s, t);
                    let stopped = a.terminal.is_none()
                        || a.observed.as_ref().is_some_and(|v| {
                            matches!(v["state"].as_str(), Some("finished" | "missing"))
                        });
                    let retryable = stopped
                        && matches!(
                            a.state.as_str(),
                            "failed" | "blocked" | "needs_report" | "missing" | "unknown"
                        );
                    Some((t.id.clone(), (a.profile.id.clone(), started_at, retryable)))
                })
                .collect();
            let card_runs: Vec<_> = s
                .workflow_runs
                .iter()
                .filter(|r| r.card_id == card_id)
                .collect();
            let active_visible = card_runs
                .iter()
                .filter(|r| r.active())
                .map(|r| (*r).clone())
                .collect::<Vec<_>>();
            let any_active = !active_visible.is_empty();
            let had_prev = !card_runs.is_empty();
            let idle_deliver = if !any_active && let Some(c) = &card_opt {
                let enabled_ids: std::collections::BTreeSet<&str> = s
                    .workflow_sources
                    .iter()
                    .filter(|src| src.project_id == c.project_id && src.enabled)
                    .map(|src| src.workflow_id.as_str())
                    .collect();
                let mut latest = std::collections::BTreeMap::new();
                for w in s
                    .workflow_revisions
                    .iter()
                    .filter(|w| w.project_id == c.project_id)
                {
                    if enabled_ids.is_empty() || enabled_ids.contains(w.workflow_id.as_str()) {
                        latest.insert(w.workflow_id.clone(), w);
                    }
                }
                latest.into_values().find_map(|w| {
                    let stage_has_deliver = c.stage == "review"
                        || w.definition.entries.get(&c.stage).is_some_and(|k| {
                            matches!(
                                w.definition.steps.get(k),
                                Some(WorkflowStep::Deliver { .. })
                            )
                        });
                    if !stage_has_deliver {
                        return None;
                    }
                    w.definition.steps.iter().find_map(|(k, st)| match st {
                        WorkflowStep::Deliver { .. } => {
                            Some((w.definition.name.clone(), step_badge_label(k, st)))
                        }
                        _ => None,
                    })
                })
            } else {
                None
            };
            let queue_info = s.merge_queue_status(&card_id).map(|(pos, total, head)| {
                let head_card_num = s
                    .cards
                    .iter()
                    .find(|c| c.id == head.card_id)
                    .map(|c| c.number);
                let head_pause = head.queue_pause_reason().map(str::to_owned);
                let head_resolving = matches!(head.state.as_str(), "waiting_agent" | "running");
                (pos, total, head_card_num, head_pause, head_resolving)
            });
            (
                active_visible,
                any_active,
                idle_deliver,
                had_prev,
                att_profiles,
                queue_info,
            )
        });

        if runs.is_empty()
            && !has_any_active_run
            && let Some(c) = &card_opt
            && c.active()
            && c.worktree.is_some()
            && c.delivery.is_none()
            && !c.pull_request.as_ref().is_some_and(|p| p.merged)
            && let Some((wf_name, badge_text)) = idle_deliver_workflow
        {
            let delivering = ui.is_method_pending("workflow.deliver");
            let cid = c.id.clone();
            let rev = c.revision;
            let held = c.hold_auto_merge;
            let hold_cid = c.id.clone();
            let state_label = if held {
                "Auto-merge held"
            } else if had_previous_run {
                "Auto-merge stopped"
            } else {
                "Ready to merge"
            };
            let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
            actions = actions.child(
                compact_action(
                    if held {
                        "Resume auto-merge"
                    } else {
                        "Hold auto-merge"
                    },
                    Variant::Secondary,
                    move |_| {
                        dispatch(
                            ui,
                            "card.hold_auto_merge",
                            json!({"card_id":hold_cid,"revision":rev,"hold":!held}),
                        )
                    },
                )
                .disabled(delivering || ui.is_method_pending("card.hold_auto_merge")),
            );
            actions = actions.child(
                compact_action(
                    if delivering {
                        "Merging…"
                    } else if had_previous_run {
                        "Retry auto-merge"
                    } else {
                        "Trigger auto-merge"
                    },
                    Variant::Primary,
                    move |_| {
                        if let Some(card) = ui.current_card() {
                            dispatch(
                                ui,
                                "workflow.deliver",
                                json!({"card_id":cid,"revision":card.revision}),
                            );
                        }
                    },
                )
                .disabled(delivering),
            );
            let mut body = view()
                .p_2()
                .gap_2()
                .border_t_1()
                .border_color(BORDER)
                .bg(INSET)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            label(format!("{wf_name} · {state_label}"))
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(actions),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(status(format!("○ {badge_text}"), Tone::Neutral)),
                );
            if let Some(reason) = c
                .human_gate
                .as_ref()
                .map(|g| g.reason.clone())
                .or_else(|| c.attention.clone())
                .filter(|r| !r.trim().is_empty())
            {
                body = body.child(notice(reason, Tone::Attention));
            }
            return body.into_element();
        }

        view().gap_2().children(
            runs.into_iter()
                .rev()
                .map(|run| {
                    let root = run
                        .history
                        .iter()
                        .find_map(|h| h["step"].as_str())
                        .or_else(|| run.definition.entries.values().next().map(String::as_str));
                    let steps = ordered_steps(&run.definition, root);
                    let has_deliver = run
                        .definition
                        .steps
                        .values()
                        .any(|s| matches!(s, WorkflowStep::Deliver { .. }));
                    let is_deliver_step = run.step.as_ref().is_some_and(|k| {
                        matches!(
                            run.definition.steps.get(k),
                            Some(WorkflowStep::Deliver { .. })
                        )
                    });
                    let is_script_step = run.step_is_script();

                    let active_agent_meta = run
                        .terminal_id
                        .as_ref()
                        .and_then(|tid| att_profiles.get(tid));
                    let active_profile = active_agent_meta.map(|(p, _, _)| p.as_str());
                    let active_agent_started_at = active_agent_meta.and_then(|(_, ts, _)| *ts);
                    let active_agent_retryable = active_agent_meta.is_some_and(|(_, _, r)| *r);
                    let deliver_retryable = is_deliver_step
                        && (matches!(run.state.as_str(), "failed" | "blocked")
                            || (run.state == "waiting_agent"
                                && (run.error.is_some() || active_agent_retryable)));
                    let mut stepper = view().flex_row().flex_wrap().items_center().gap_1();
                    for (idx, step_id) in steps.iter().enumerate() {
                        if idx > 0 {
                            stepper = stepper.child(muted("→"));
                        }
                        let step = &run.definition.steps[step_id];
                        let is_current = run.step.as_ref() == Some(step_id) && run.active();
                        let is_done = !is_current
                            && (run.outputs.contains_key(step_id)
                                || run
                                    .history
                                    .iter()
                                    .any(|h| h["step"] == *step_id && h["event"] == "completed"));
                        let script_out = matches!(step, WorkflowStep::Script { .. })
                            .then(|| run.outputs.get(step_id))
                            .flatten();
                        let script_failed_continued =
                            script_out.is_some_and(|o| o["outcome"] == "failed_continued");
                        let marker = if is_current {
                            "●"
                        } else if script_failed_continued {
                            "!"
                        } else if is_done {
                            "✓"
                        } else {
                            "○"
                        };
                        let mut badge_label = step_badge_label(step_id, step);
                        if is_current
                            && matches!(step, WorkflowStep::Deliver { .. })
                            && matches!(run.state.as_str(), "waiting_agent" | "running")
                            && !deliver_retryable
                            && let Some(prof) = active_profile
                        {
                            badge_label = format!("{badge_label} (@{prof})");
                        }
                        if let Some(out) = script_out {
                            if out["timed_out"] == true {
                                badge_label = format!("{badge_label} (timeout)");
                            } else if let Some(code) = out["exit_code"].as_i64()
                                && code != 0
                            {
                                badge_label = format!("{badge_label} (exit {code})");
                            }
                        }
                        let tone = if is_current {
                            if run.state == "failed" {
                                Tone::Danger
                            } else if matches!(
                                run.state.as_str(),
                                "waiting_approval" | "stale" | "blocked"
                            ) || deliver_retryable
                            {
                                Tone::Attention
                            } else {
                                Tone::Info
                            }
                        } else if script_failed_continued {
                            Tone::Attention
                        } else if is_done {
                            Tone::Success
                        } else {
                            Tone::Neutral
                        };
                        if is_current
                            && matches!(
                                run.state.as_str(),
                                "waiting_agent" | "running" | "waiting_script"
                            )
                            && !deliver_retryable
                            && let Some(elapsed_base) = crate::components::activity::workflow_step_elapsed(
                                &run,
                                step_id,
                                ui.runtime_clock.get_untracked(),
                                active_agent_started_at,
                            )
                        {
                            let start_ts = ui
                                .runtime_clock
                                .get_untracked()
                                .saturating_sub(elapsed_base);
                            let base_prefix = format!("{marker} {badge_label}");
                            stepper = stepper.child(status(
                                move || {
                                    let dur = crate::components::activity::compact_duration(
                                        ui.runtime_clock.get().saturating_sub(start_ts),
                                    );
                                    format!("{base_prefix} · {dur}")
                                },
                                tone,
                            ));
                        } else if (is_done || is_current)
                            && let Some(elapsed_ms) =
                                crate::components::activity::workflow_step_elapsed(
                                    &run,
                                    step_id,
                                    ui.runtime_clock.get_untracked(),
                                    active_agent_started_at,
                                )
                        {
                            let dur = crate::components::activity::compact_duration(elapsed_ms);
                            stepper =
                                stepper.child(status(format!("{marker} {badge_label} · {dur}"), tone));
                        } else {
                            stepper = stepper.child(status(format!("{marker} {badge_label}"), tone));
                        }
                    }

                    let pending_action = ui.is_method_pending("workflow.decide")
                        || ui.is_method_pending("workflow.deliver")
                        || ui.is_method_pending("card.hold_auto_merge");

                    let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
                    if run.active() {
                        if has_deliver
                            && let Some(c) = &card_opt
                        {
                            let held = c.hold_auto_merge;
                            let cid = c.id.clone();
                            let rev = c.revision;
                            actions = actions.child(
                                compact_action(
                                    if held {
                                        "Resume auto-merge"
                                    } else {
                                        "Hold auto-merge"
                                    },
                                    if held && !is_deliver_step {
                                        Variant::Primary
                                    } else {
                                        Variant::Secondary
                                    },
                                    move |_| {
                                        dispatch(
                                            ui,
                                            "card.hold_auto_merge",
                                            json!({"card_id":cid,"revision":rev,"hold":!held}),
                                        )
                                    },
                                )
                                .disabled(pending_action),
                            );
                        }
                        if run.state == "waiting_approval" || run.state == "stale" {
                            if run.state == "waiting_approval" {
                                let id = run.id.clone();
                                let visit = run.visit;
                                actions = actions.child(
                                    compact_action(
                                        if is_deliver_step {
                                            "Retry auto-merge"
                                        } else if is_script_step {
                                            "Retry script"
                                        } else {
                                            "Approve and continue"
                                        },
                                        Variant::Primary,
                                        move |_| {
                                            if let Some(card) = ui.current_card() {
                                                if is_deliver_step {
                                                    dispatch(
                                                        ui,
                                                        "workflow.deliver",
                                                        json!({"card_id":card.id,"revision":card.revision}),
                                                    );
                                                } else {
                                                    dispatch(
                                                        ui,
                                                        "workflow.decide",
                                                        json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"approve"}),
                                                    );
                                                }
                                            }
                                        },
                                    )
                                    .disabled(pending_action),
                                );
                            }
                            let can_rework = run.state == "stale"
                                || run.step.as_ref().is_some_and(|k| {
                                    matches!(
                                        &run.definition.steps[k],
                                        WorkflowStep::Approval {
                                            rework: Some(_),
                                            ..
                                        } | WorkflowStep::Deliver {
                                            rework: Some(_),
                                            ..
                                        } | WorkflowStep::Script {
                                            on_failure: Some(_),
                                            ..
                                        }
                                    )
                                });
                            if can_rework {
                                let id = run.id.clone();
                                let visit = run.visit;
                                actions = actions.child(
                                    compact_action(
                                        "Request changes",
                                        Variant::Secondary,
                                        move |_| {
                                            if rework_open.get_untracked() {
                                                if let Some(card) = ui.current_card() {
                                                    let note = rework_text.get_untracked();
                                                    rework_open.set(false);
                                                    rework_text.set(String::new());
                                                    dispatch(
                                                        ui,
                                                        "workflow.decide",
                                                        json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"rework","feedback":note}),
                                                    );
                                                }
                                            } else if let Some(card) = ui.current_card() {
                                                dispatch(
                                                    ui,
                                                    "workflow.decide",
                                                    json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"rework"}),
                                                );
                                            }
                                        },
                                    )
                                    .disabled(pending_action),
                                );
                                actions = actions.child(compact_action(
                                    if rework_open.get() {
                                        "Hide note"
                                    } else {
                                        "With note…"
                                    },
                                    Variant::Quiet,
                                    move |_| rework_open.update(|v| *v = !*v),
                                ));
                            }
                        } else if (run.state == "waiting_delivery" && is_deliver_step)
                            || deliver_retryable
                        {
                            actions = actions.child(
                                compact_action(
                                    "Retry auto-merge",
                                    if deliver_retryable {
                                        Variant::Primary
                                    } else {
                                        Variant::Secondary
                                    },
                                    move |_| {
                                        if let Some(card) = ui.current_card() {
                                            dispatch(
                                                ui,
                                                "workflow.deliver",
                                                json!({"card_id":card.id,"revision":card.revision}),
                                            );
                                        }
                                    },
                                )
                                .disabled(pending_action),
                            );
                        }
                    }
                    for (step_key, output) in &run.outputs {
                        if let Some(path) = output["path"].as_str() {
                            let path = path.to_owned();
                            let card = card_id.clone();
                            let is_script = matches!(
                                run.definition.steps.get(step_key),
                                Some(WorkflowStep::Script { .. })
                            );
                            let label_text = if is_script {
                                if run.outputs.len() > 1 {
                                    format!(
                                        "Log ({})",
                                        step_key.strip_prefix("script:").unwrap_or(step_key)
                                    )
                                } else {
                                    "Script log".into()
                                }
                            } else if run.outputs.len() > 1 {
                                format!(
                                    "Report ({})",
                                    step_key.strip_prefix("agent:").unwrap_or(step_key)
                                )
                            } else {
                                "Report".into()
                            };
                            actions = actions.child(compact_action(
                                label_text,
                                Variant::Quiet,
                                move |_| open_resource(ui, card.clone(), path.clone()),
                            ));
                        }
                    }

                    let state_label = match run.state.as_str() {
                        "stale" => "Changes required".to_owned(),
                        "waiting_delivery" => {
                            if let Some((pos, total, head_num, _, head_resolving)) = &queue_info {
                                if *pos > 1 {
                                    if *head_resolving {
                                        if let Some(num) = head_num {
                                            format!(
                                                "Queued for merge (#{pos} of {total} · waiting on #{num} conflict)"
                                            )
                                        } else {
                                            format!("Queued for merge (#{pos} of {total})")
                                        }
                                    } else {
                                        format!("Queued for merge (#{pos} of {total})")
                                    }
                                } else if run.queue_pause_reason().is_some() {
                                    "Merge queue waiting".to_owned()
                                } else {
                                    "Merging…".to_owned()
                                }
                            } else if run.queue_pause_reason().is_some() {
                                "Merge queue waiting".to_owned()
                            } else {
                                "Merging…".to_owned()
                            }
                        }
                        "waiting_capacity" => "Waiting for Agent slot…".to_owned(),
                        "waiting_script" => "Running script…".to_owned(),
                        "waiting_agent" | "running" => {
                            if deliver_retryable {
                                "Auto-merge stopped".to_owned()
                            } else if run.inputs["conflict_attempts"].as_u64().unwrap_or(0) > 0 {
                                if let Some(prof) = active_profile {
                                    format!("Resolving merge conflict (@{prof})…")
                                } else {
                                    "Resolving merge conflict…".to_owned()
                                }
                            } else {
                                "Agent running…".to_owned()
                            }
                        }
                        "blocked" => "Blocked".to_owned(),
                        "failed" if is_deliver_step => {
                            if card_opt.as_ref().is_some_and(|c| c.hold_auto_merge) {
                                "Auto-merge held".to_owned()
                            } else {
                                "Auto-merge failed".to_owned()
                            }
                        }
                        "failed" => "Workflow failed".to_owned(),
                        _ if is_deliver_step => {
                            if card_opt.as_ref().is_some_and(|c| c.hold_auto_merge) {
                                "Auto-merge held".to_owned()
                            } else {
                                "Auto-merge paused".to_owned()
                            }
                        }
                        _ => "Review required".to_owned(),
                    };
                    let active_step_start = (matches!(
                        run.state.as_str(),
                        "waiting_agent" | "running" | "waiting_script"
                    ) && !deliver_retryable)
                    .then(|| {
                        let now_ms = ui.runtime_clock.get_untracked();
                        if let Some(step_id) = &run.step
                            && let Some(elapsed) =
                                crate::components::activity::workflow_step_elapsed(
                                    &run,
                                    step_id,
                                    now_ms,
                                    active_agent_started_at,
                                )
                        {
                            Some(now_ms.saturating_sub(elapsed))
                        } else {
                            active_agent_started_at.filter(|&t| t > 1_000_000_000_000)
                        }
                    })
                    .flatten();
                    let wf_title_prefix = format!("{} · {state_label}", run.definition.name);
                    let header_label = if let Some(start_ts) = active_step_start {
                        label(move || {
                            let dur = crate::components::activity::compact_duration(
                                ui.runtime_clock.get().saturating_sub(start_ts),
                            );
                            format!("{wf_title_prefix} · {dur}")
                        })
                    } else {
                        label(wf_title_prefix)
                    }
                    .font_weight(FontWeight::MEDIUM);
                    let mut body = view()
                        .p_2()
                        .gap_2()
                        .border_t_1()
                        .border_color(BORDER)
                        .bg(INSET)
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(header_label)
                                .child(actions),
                        )
                        .child(stepper);

                    if run.state == "waiting_delivery" {
                        if let Some(pause_reason) = run.queue_pause_reason() {
                            body = body.child(notice(pause_reason.to_owned(), Tone::Attention));
                        } else if let Some((pos, _, _, Some(head_pause), _)) = &queue_info
                            && *pos > 1
                        {
                            body = body.child(notice(head_pause.clone(), Tone::Attention));
                        }
                    } else if (matches!(
                        run.state.as_str(),
                        "waiting_approval" | "stale" | "failed" | "blocked"
                    ) || deliver_retryable)
                        && let Some(reason) = card_opt
                            .as_ref()
                            .and_then(|c| {
                                c.human_gate
                                    .as_ref()
                                    .map(|g| g.reason.clone())
                                    .or_else(|| c.attention.clone())
                            })
                            .filter(|r| !r.trim().is_empty())
                            .or_else(|| run.error.clone().filter(|r| !r.trim().is_empty()))
                    {
                        let notice_tone = if run.state == "failed" {
                            Tone::Danger
                        } else {
                            Tone::Attention
                        };
                        body = body.child(notice(reason, notice_tone));
                    }

                    if rework_open.get() && (run.state == "waiting_approval" || run.state == "stale")
                    {
                        let id = run.id.clone();
                        let visit = run.visit;
                        body = body.child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    field(
                                        rework_text,
                                        "Rework instructions for the Agent (optional)…",
                                    )
                                    .flex_1(),
                                )
                                .child(compact_action(
                                    "Send rework",
                                    Variant::Primary,
                                    move |_| {
                                        if let Some(card) = ui.current_card() {
                                            let note = rework_text.get_untracked();
                                            rework_open.set(false);
                                            rework_text.set(String::new());
                                            dispatch(
                                                ui,
                                                "workflow.decide",
                                                json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"rework","feedback":note}),
                                            );
                                        }
                                    },
                                )),
                        );
                    }
                    body.into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
    })
}
