use super::*;
use canban_types::{WorkflowDefinition, WorkflowStep};
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
        WorkflowStep::Move { column, .. } => format!("→ {column}"),
        WorkflowStep::Approval { .. } => "Approval".into(),
        WorkflowStep::Deliver { .. } => "Auto-merge".into(),
        WorkflowStep::Condition { .. } => "Rule check".into(),
        WorkflowStep::Notify { .. } => "Notify".into(),
        WorkflowStep::Complete => "Done".into(),
    }
}

pub fn render(ui: Ui) -> impl IntoElement {
    let rework_open = signal(false);
    let rework_text = signal(String::new());
    dynamic(move || {
        let card_id = ui.card_id();
        let card_opt = ui.current_card();
        let runs = ui.snapshot.with(|s| {
            s.workflow_runs
                .iter()
                .filter(|r| r.card_id == card_id && r.active())
                .cloned()
                .collect::<Vec<_>>()
        });
        let has_deliver_in_project = ui.snapshot.with(|s| {
            let project = ui.project.get();
            s.workflow_revisions.iter().any(|w| {
                w.project_id == project
                    && w.definition
                        .steps
                        .values()
                        .any(|st| matches!(st, WorkflowStep::Deliver { .. }))
            })
        });
        let mut container = view().gap_2();
        if runs.is_empty()
            && has_deliver_in_project
            && let Some(c) = &card_opt
            && c.active()
        {
            let rev = c.revision;
            let held = c.hold_auto_merge;
            let cid = c.id.clone();
            container = container.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .p_2()
                    .rounded(px(RADIUS))
                    .bg(INSET)
                    .child(muted(if held {
                        "Auto-merge is held for this Card; workflow delivery will pause for your approval."
                    } else {
                        "Auto-merge queue is enabled for this Project."
                    }))
                    .child(compact_action(
                        if held { "Resume auto-merge" } else { "Hold auto-merge" },
                        if held { Variant::Primary } else { Variant::Secondary },
                        move |_| {
                            dispatch(
                                ui,
                                "card.hold_auto_merge",
                                json!({"card_id":cid,"revision":rev,"hold":!held}),
                            )
                        },
                    )),
            );
        }
        container.children(
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

                    let mut stepper = view().flex_row().flex_wrap().items_center().gap_1();
                    for (idx, step_id) in steps.iter().enumerate() {
                        if idx > 0 {
                            stepper = stepper.child(muted("→"));
                        }
                        let step = &run.definition.steps[step_id];
                        let is_current = run.step.as_ref() == Some(step_id) && run.active();
                        let is_done = run.outputs.contains_key(step_id)
                            || run
                                .history
                                .iter()
                                .any(|h| h["step"] == *step_id && h["event"] == "completed");
                        let marker = if is_current {
                            "●"
                        } else if is_done {
                            "✓"
                        } else {
                            "○"
                        };
                        let text_label = format!("{marker} {}", step_badge_label(step_id, step));
                        let tone = if is_current {
                            if run.state == "waiting_approval" || run.state == "stale" {
                                Tone::Attention
                            } else {
                                Tone::Info
                            }
                        } else if is_done {
                            Tone::Success
                        } else {
                            Tone::Neutral
                        };
                        stepper = stepper.child(status(text_label, tone));
                    }

                    let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
                    if run.active() {
                        if has_deliver
                            && let Some(c) = &card_opt
                        {
                            let held = c.hold_auto_merge;
                            let cid = c.id.clone();
                            let rev = c.revision;
                            actions = actions.child(compact_action(
                                if held {
                                    "Resume auto-merge"
                                } else {
                                    "Hold auto-merge"
                                },
                                if held {
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
                            ));
                        }
                        if run.state == "waiting_approval" || run.state == "stale" {
                            if run.state == "waiting_approval" {
                                let id = run.id.clone();
                                let visit = run.visit;
                                actions = actions.child(compact_action(
                                    "Approve",
                                    Variant::Primary,
                                    move |_| {
                                        if let Some(card) = ui.current_card() {
                                            dispatch(
                                                ui,
                                                "workflow.decide",
                                                json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"approve"}),
                                            );
                                        }
                                    },
                                ));
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
                                        }
                                    )
                                });
                            if can_rework {
                                let id = run.id.clone();
                                let visit = run.visit;
                                actions = actions.child(compact_action(
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
                                ));
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
                        }
                        let id = run.id.clone();
                        actions = actions.child(compact_action("Stop", Variant::Quiet, move |_| {
                            dispatch(ui, "workflow.stop", json!({"run_id":id}))
                        }));
                    }
                    for (step_key, output) in &run.outputs {
                        if let Some(path) = output["path"].as_str() {
                            let path = path.to_owned();
                            let card = card_id.clone();
                            let label_text = if run.outputs.len() > 1 {
                                format!("Report ({})", step_key.strip_prefix("agent:").unwrap_or(step_key))
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

                    let mut body = view()
                        .p_3()
                        .gap_2()
                        .rounded(px(RADIUS))
                        .bg(INSET)
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    label(format!(
                                        "{} · {}",
                                        run.definition.name,
                                        run.state.replace('_', " ")
                                    ))
                                    .font_weight(FontWeight::MEDIUM),
                                )
                                .child(actions),
                        )
                        .child(stepper);

                    if rework_open.get() && (run.state == "waiting_approval" || run.state == "stale") {
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
    })
}
