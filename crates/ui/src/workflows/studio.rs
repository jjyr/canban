//! Stage-aligned visual workflow studio: maps triggers, Board stages, step nodes,
//! branching/rework transitions, and per-step Agent/Prompt inspection.
use super::{edit_agent, source_editor, update_doc};
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use canban_workflow_engine::source::{self, default_step_prompt, is_default_step_prompt};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn render(ui: Ui) -> AnyElement {
    let state = ui.workflows;
    let name = state.selected.get();
    let project = ui.project.get();
    if name.is_empty() {
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(muted("Choose or create a workflow"))
            .into_element();
    }
    let raw = state
        .drafts
        .with(|d| d.get(&project).and_then(|d| d.sources.get(&name)).cloned())
        .unwrap_or_default();
    let doc = match source::parse(&raw) {
        Ok(doc) => doc,
        Err(err) => {
            return view()
                .flex_1()
                .min_h(px(0.))
                .gap_3()
                .child(notice(
                    format!("YAML syntax or schema error: {err}. Switch to YAML Source to fix it."),
                    Tone::Danger,
                ))
                .child(source_editor(ui))
                .into_element();
        }
    };

    let options = source::derive_options(&doc);

    let effective_columns = if !doc.columns.is_empty() {
        doc.columns.clone()
    } else {
        ui.current_project()
            .map(|p| {
                p.workflow
                    .stages
                    .iter()
                    .map(|s| canban_types::WorkflowColumn {
                        id: s.id.clone(),
                        name: s.name.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(default_columns)
    };

    let visible_steps = ordered_visible_steps(&doc);
    let active_step_id = {
        let current = state.selected_step.get();
        if visible_steps.iter().any(|s| s.id == current) {
            current
        } else {
            visible_steps
                .iter()
                .find(|s| s.uses == "agent")
                .or_else(|| visible_steps.first())
                .map(|s| s.id.clone())
                .unwrap_or_default()
        }
    };

    let col_names: BTreeMap<String, String> = effective_columns
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    let step_titles: BTreeMap<String, String> = doc
        .steps
        .iter()
        .map(|s| (s.id.clone(), humanize_step_title(s, &col_names)))
        .collect();

    let overview = overview_header(
        ui,
        &doc,
        &options,
        &active_step_id,
        &col_names,
        &step_titles,
    );
    let pipeline = stage_canvas(
        ui,
        &doc,
        &effective_columns,
        &visible_steps,
        &active_step_id,
        &col_names,
        &step_titles,
    );
    let inspector = step_inspector(
        ui,
        &doc,
        &effective_columns,
        &visible_steps,
        &active_step_id,
        &step_titles,
    );
    let policies = options_bar(ui, options);

    scroll()
        .flex_1()
        .min_h(px(0.))
        .child(
            view()
                .gap_3()
                .pb_4()
                .child(overview)
                .child(pipeline)
                .child(inspector)
                .child(policies),
        )
        .into_element()
}

fn ordered_visible_steps(doc: &source::Document) -> Vec<source::Step> {
    let non_complete: Vec<_> = doc
        .steps
        .iter()
        .filter(|s| s.uses != "complete")
        .cloned()
        .collect();
    if non_complete.len() <= 1 {
        return non_complete;
    }
    let step_map: BTreeMap<String, usize> = doc
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut ordered = Vec::with_capacity(non_complete.len());

    fn walk(
        id: &str,
        doc: &source::Document,
        step_map: &BTreeMap<String, usize>,
        visited: &mut std::collections::BTreeSet<String>,
        ordered: &mut Vec<source::Step>,
    ) {
        if !visited.insert(id.to_owned()) {
            return;
        }
        let Some(&idx) = step_map.get(id) else {
            return;
        };
        let step = &doc.steps[idx];
        if step.uses == "complete" {
            return;
        }
        ordered.push(step.clone());
        if step.uses == "condition" {
            if let Some(t) = step.with.get("then").and_then(|v| v.as_str()) {
                walk(t, doc, step_map, visited, ordered);
            }
            if let Some(o) = step.with.get("otherwise").and_then(|v| v.as_str()) {
                walk(o, doc, step_map, visited, ordered);
            }
        } else {
            let next_id = step
                .next
                .clone()
                .or_else(|| doc.steps.get(idx + 1).map(|s| s.id.clone()));
            if let Some(next_id) = next_id {
                walk(&next_id, doc, step_map, visited, ordered);
            }
        }
    }

    let mut roots: Vec<String> = Vec::new();
    for col_key in ["backlog", "develop", "review"] {
        if let Some(r) = doc.on.get(col_key)
            && r.enabled
            && !roots.contains(&r.start_at)
        {
            roots.push(r.start_at.clone());
        }
    }
    for col in &doc.columns {
        for r in doc.on.values() {
            if r.enabled
                && r.selector.column.as_deref() == Some(col.id.as_str())
                && !roots.contains(&r.start_at)
            {
                roots.push(r.start_at.clone());
            }
        }
    }
    for r in doc.on.values() {
        if r.enabled && !roots.contains(&r.start_at) {
            roots.push(r.start_at.clone());
        }
    }
    for root in roots {
        walk(&root, doc, &step_map, &mut visited, &mut ordered);
    }
    for step in non_complete {
        if visited.insert(step.id.clone()) {
            ordered.push(step);
        }
    }
    ordered
}

fn default_columns() -> Vec<canban_types::WorkflowColumn> {
    vec![
        canban_types::WorkflowColumn {
            id: "backlog".into(),
            name: "Backlog".into(),
        },
        canban_types::WorkflowColumn {
            id: "develop".into(),
            name: "Develop".into(),
        },
        canban_types::WorkflowColumn {
            id: "review".into(),
            name: "Review".into(),
        },
    ]
}

fn humanize_step_title(step: &source::Step, col_names: &BTreeMap<String, String>) -> String {
    if let Some(name) = step
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return name.to_owned();
    }
    match (step.id.as_str(), step.uses.as_str()) {
        ("agent:plan" | "plan", "agent") => "Plan task".into(),
        ("agent:develop" | "develop", "agent") => "Implement".into(),
        ("agent:review" | "review", "agent") => "Review".into(),
        ("agent:merge" | "merge_verify", "agent") => "Pre-merge check".into(),
        (_, "card.move") => {
            let target = step
                .with
                .get("column")
                .and_then(|v| v.as_str())
                .unwrap_or("stage");
            let cname = col_names.get(target).map(String::as_str).unwrap_or(target);
            format!("Move to {cname}")
        }
        (_, "approval") => "Human approval".into(),
        (_, "deliver" | "card.deliver") => "Queue merge".into(),
        ("condition:external_pr", "condition") => "External PR check".into(),
        _ => step.id.clone(),
    }
}

fn short_workflow_id(id: &str) -> String {
    if let Some((prefix, rest)) = id.split_once(':')
        && rest.len() > 12
        && rest.chars().all(|c| c.is_ascii_hexdigit())
    {
        format!("{prefix}:{}", &rest[..8])
    } else if id.len() > 28 {
        format!("{}…", &id[..24])
    } else {
        id.to_owned()
    }
}

fn overview_header(
    ui: Ui,
    doc: &source::Document,
    options: &source::WorkflowOptions,
    active_step_id: &str,
    col_names: &BTreeMap<String, String>,
    step_titles: &BTreeMap<String, String>,
) -> View {
    let state = ui.workflows;
    let mut triggers_row = view().flex_row().flex_wrap().items_center().gap_1p5();
    triggers_row = triggers_row.child(muted("Entry triggers:"));
    for (rule_id, rule) in &doc.on {
        let start_id = rule.start_at.clone();
        let target_label = step_titles
            .get(&start_id)
            .cloned()
            .unwrap_or_else(|| start_id.clone());
        let event_desc = match (rule.event.as_str(), rule.selector.column.as_deref()) {
            ("manual", _) => "Manual run".to_owned(),
            ("card.created", _) => "Card created".to_owned(),
            ("card.column_entered", Some(col)) => {
                let cname = col_names.get(col).map(String::as_str).unwrap_or(col);
                format!("Enter {cname}")
            }
            ("card.column_left", Some(col)) => {
                let cname = col_names.get(col).map(String::as_str).unwrap_or(col);
                format!("Leave {cname}")
            }
            (ev, Some(col)) => format!("{ev} ({col})"),
            (ev, None) => ev.to_owned(),
        };
        let is_selected = active_step_id == start_id;
        let click_start = start_id.clone();
        let disabled_suffix = if rule.enabled { "" } else { " · off" };
        triggers_row = triggers_row.child(
            view()
                .tooltip(
                    nsh_base::tooltip()
                        .placement(Placement::Top)
                        .px_2p5()
                        .py_1p5()
                        .rounded(px(RADIUS))
                        .bg(SURFACE)
                        .border_1()
                        .border_color(BORDER)
                        .child(label(format!(
                            "Rule `{rule_id}` ({}) → starts at `{start_id}`",
                            rule.event
                        ))),
                )
                .child(
                    nsh_base::button()
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .bg(if is_selected { SELECTED } else { INSET })
                        .border_1()
                        .border_color(if is_selected { ACCENT } else { BORDER })
                        .hover(|s| s.border_color(ACCENT))
                        .on_press(move |_: &()| state.selected_step.set(click_start.clone()))
                        .child(icon_tinted(Icon::Trigger, ACCENT).w(px(12.)).h(px(12.)))
                        .child(
                            label(format!("{event_desc} → {target_label}{disabled_suffix}"))
                                .text_size(px(12.)),
                        ),
                ),
        );
    }

    panel()
        .gap_2p5()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(icon_tinted(Icon::Trigger, ACCENT).w(px(16.)).h(px(16.)))
                        .child(label(doc.name.clone()).font_weight(FontWeight::SEMIBOLD))
                        .child(
                            view()
                                .tooltip(
                                    nsh_base::tooltip()
                                        .placement(Placement::Top)
                                        .px_2p5()
                                        .py_1p5()
                                        .rounded(px(RADIUS))
                                        .bg(SURFACE)
                                        .border_1()
                                        .border_color(BORDER)
                                        .child(label(format!("Workflow ID: {}", doc.id))),
                                )
                                .child(badge(short_workflow_id(&doc.id))),
                        )
                        .child(status(
                            if doc.enabled { "Active" } else { "Disabled" },
                            if doc.enabled {
                                Tone::Success
                            } else {
                                Tone::Neutral
                            },
                        ))
                        .child(status(
                            if options.review_manual {
                                "Manual Review"
                            } else if options.sync_upstream {
                                "Queue Merge · Sync"
                            } else {
                                "Queue Merge · Local"
                            },
                            if options.review_manual {
                                Tone::Attention
                            } else {
                                Tone::Info
                            },
                        )),
                )
                .child(compact_action(
                    "Ask Agent to design…",
                    Variant::Secondary,
                    move |_| edit_agent(ui),
                )),
        )
        .child(triggers_row)
}

fn stage_canvas(
    ui: Ui,
    doc: &source::Document,
    effective_columns: &[canban_types::WorkflowColumn],
    visible_steps: &[source::Step],
    active_step_id: &str,
    col_names: &BTreeMap<String, String>,
    step_titles: &BTreeMap<String, String>,
) -> View {
    let state = ui.workflows;
    let first_col_id = effective_columns
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "backlog".into());
    let mut lane_steps: BTreeMap<String, Vec<source::Step>> = effective_columns
        .iter()
        .map(|c| (c.id.clone(), Vec::new()))
        .collect();
    let mut current_lane = first_col_id;
    for step in visible_steps {
        if let Some(rule_col) = doc
            .on
            .values()
            .filter(|r| r.enabled && r.start_at == step.id)
            .find_map(|r| r.selector.column.as_deref())
            && lane_steps.contains_key(rule_col)
        {
            current_lane = rule_col.to_owned();
        }
        lane_steps
            .entry(current_lane.clone())
            .or_default()
            .push(step.clone());
        if step.uses == "card.move"
            && let Some(target_col) = step.with.get("column").and_then(|v| v.as_str())
            && lane_steps.contains_key(target_col)
        {
            current_lane = target_col.to_owned();
        }
    }

    let cols_snapshot = effective_columns.to_vec();
    let cols_for_add = cols_snapshot.clone();
    let stage_count = effective_columns.len();
    let step_count = visible_steps.len();

    let use_equal_lanes = stage_count <= 3;
    let mut lanes_row = view()
        .flex_row()
        .items_stretch()
        .gap_2p5()
        .when(use_equal_lanes, |v| v.w_full());
    for (col_idx, col) in effective_columns.iter().enumerate() {
        if col_idx > 0 {
            lanes_row = lanes_row.child(
                view()
                    .justify_center()
                    .items_center()
                    .child(icon(Icon::Forward).w(px(14.)).h(px(14.)).opacity(0.55)),
            );
        }
        let col_id = col.id.clone();
        let col_name = col.name.clone();
        let can_remove =
            stage_count > 1 && !matches!(col_id.as_str(), "backlog" | "develop" | "review");
        let cols_for_remove = cols_snapshot.clone();
        let steps_in_col = lane_steps.get(&col_id).cloned().unwrap_or_default();

        let mut col_header_right = view().flex_row().items_center().gap_1();
        col_header_right = col_header_right.child(badge(format!(
            "{} step{}",
            steps_in_col.len(),
            if steps_in_col.len() == 1 { "" } else { "s" }
        )));
        if can_remove {
            let remove_id = col_id.clone();
            col_header_right =
                col_header_right.child(compact_action("×", Variant::Quiet, move |_| {
                    let remaining: Vec<_> = cols_for_remove
                        .iter()
                        .filter(|c| c.id != remove_id)
                        .cloned()
                        .collect();
                    update_doc(ui, |d| {
                        d.columns = remaining;
                    });
                }));
        }

        let mut steps_stack = view().gap_1p5().flex_1();
        if steps_in_col.is_empty() {
            steps_stack = steps_stack.child(
                view()
                    .py_4()
                    .px_3()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(BORDER)
                    .bg(BACKGROUND)
                    .opacity(0.65)
                    .items_center()
                    .justify_center()
                    .child(muted("No automated steps in this stage")),
            );
        } else {
            for (s_idx, step) in steps_in_col.iter().enumerate() {
                if s_idx > 0 {
                    steps_stack = steps_stack.child(
                        view()
                            .items_center()
                            .py_0p5()
                            .child(muted("↓").text_size(px(11.))),
                    );
                }
                steps_stack = steps_stack.child(step_node_card(
                    ui,
                    doc,
                    step,
                    active_step_id,
                    col_names,
                    step_titles,
                ));
            }
        }

        let lane_card = view()
            .when(use_equal_lanes, |v| v.flex_1().min_w(px(210.)))
            .when(!use_equal_lanes, |v| v.w(px(276.)).flex_shrink_0())
            .p_3()
            .gap_2p5()
            .rounded(px(RADIUS))
            .bg(INSET)
            .border_1()
            .border_color(BORDER)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pb_1p5()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .min_w(px(0.))
                            .child(
                                view()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(SURFACE)
                                    .border_1()
                                    .border_color(BORDER)
                                    .child(muted(format!("{}", col_idx + 1)).text_size(px(11.))),
                            )
                            .child(label(col_name).font_weight(FontWeight::SEMIBOLD).truncate())
                            .child(muted(format!("({col_id})")).text_size(px(11.))),
                    )
                    .child(col_header_right),
            )
            .child(steps_stack);

        lanes_row = lanes_row.child(lane_card);
    }

    let add_col_controls = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(field(state.new_column_name, "New stage (e.g. QA)…").w(px(170.)))
        .child(compact_action(
            "+ Add stage",
            Variant::Secondary,
            move |_| {
                let raw_name = state.new_column_name.get_untracked();
                let title = raw_name.trim().to_owned();
                if title.is_empty() {
                    return;
                }
                let slug: String = title
                    .to_ascii_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect::<String>()
                    .trim_matches('_')
                    .to_owned();
                if slug.is_empty() || cols_for_add.iter().any(|c| c.id == slug) {
                    return;
                }
                let mut updated_cols = cols_for_add.clone();
                if let Some(pos) = updated_cols.iter().position(|c| c.id == "review") {
                    updated_cols.insert(
                        pos,
                        canban_types::WorkflowColumn {
                            id: slug,
                            name: title,
                        },
                    );
                } else {
                    updated_cols.push(canban_types::WorkflowColumn {
                        id: slug,
                        name: title,
                    });
                }
                state.new_column_name.set(String::new());
                update_doc(ui, |d| {
                    d.columns = updated_cols;
                });
            },
        ));

    panel()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .gap_0p5()
                        .child(
                            label(format!(
                                "Stage Execution Graph · {stage_count} stages · {step_count} steps"
                            ))
                            .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(muted(
                            "Steps are grouped under the Board stage where they run. Click any node to inspect or customize its Agent & Prompt.",
                        )),
                )
                .child(add_col_controls),
        )
        .child(if use_equal_lanes {
            lanes_row.into_element()
        } else {
            scroll().horizontal().w_full().child(lanes_row).into_element()
        })
}

fn step_node_card(
    ui: Ui,
    doc: &source::Document,
    step: &source::Step,
    active_step_id: &str,
    col_names: &BTreeMap<String, String>,
    step_titles: &BTreeMap<String, String>,
) -> impl IntoElement {
    let state = ui.workflows;
    let step_id = step.id.clone();
    let is_selected = step_id == active_step_id;
    let click_id = step_id.clone();
    let title = step_titles
        .get(&step_id)
        .cloned()
        .unwrap_or_else(|| step.id.clone());

    let (step_icon, step_tone, kind_badge, detail_line) = match step.uses.as_str() {
        "agent" => {
            let prof = step
                .with
                .get("profile")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            let instr = step
                .with
                .get("instruction")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let customized = !is_default_step_prompt(&step.id, Some(prof), instr);
            let first_line = instr
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("Run agent task");
            let agent_icon = if prof.contains("codex") {
                Icon::Codex
            } else if prof.contains("claude") {
                Icon::Claude
            } else {
                Icon::Agent
            };
            (
                agent_icon,
                ACCENT,
                format!("@{prof}{}", if customized { " · custom" } else { "" }),
                first_line.to_owned(),
            )
        }
        "card.move" => {
            let target = step
                .with
                .get("column")
                .and_then(|v| v.as_str())
                .unwrap_or("column");
            let target_name = col_names.get(target).map(String::as_str).unwrap_or(target);
            (
                Icon::Forward,
                ACCENT,
                "Stage Move".into(),
                format!("Move Card into {target_name}"),
            )
        }
        "approval" => {
            let gate = step
                .with
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Pause for human review & approval");
            (Icon::Check, ATTENTION, "Human Gate".into(), gate.to_owned())
        }
        "deliver" | "card.deliver" => {
            let sync = step
                .with
                .get("sync_upstream")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let branch = step
                .with
                .get("target_branch")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let detail = match (branch, sync) {
                (Some(b), true) => format!("Merge into {b} & sync upstream"),
                (Some(b), false) => format!("Merge into {b} (local only)"),
                (None, true) => "Merge & sync upstream remote".into(),
                (None, false) => "Merge locally only (no upstream sync)".into(),
            };
            let badge_txt = if sync {
                "Queue Merge · Sync"
            } else {
                "Queue Merge · Local"
            };
            (Icon::Branch, SUCCESS, badge_txt.into(), detail)
        }
        "condition" => {
            let ptr = step
                .with
                .get("pointer")
                .and_then(|v| v.as_str())
                .unwrap_or("/input");
            let eq = step
                .with
                .get("equals")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| step.with.get("equals").unwrap_or(&Value::Null).to_string());
            (
                Icon::Trigger,
                ATTENTION,
                "Condition".into(),
                format!("{ptr} == {eq}"),
            )
        }
        other => (Icon::Info, MUTED, other.to_owned(), "Workflow step".into()),
    };

    let entry_rules: Vec<String> = doc
        .on
        .iter()
        .filter(|(_, r)| r.enabled && r.start_at == step_id)
        .map(|(k, _)| k.clone())
        .collect();

    let mut has_meta = false;
    let mut meta_pills = view().flex_row().flex_wrap().items_center().gap_1();
    for r_name in entry_rules {
        has_meta = true;
        meta_pills = meta_pills.child(
            view()
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .child(muted(format!("⚡ on:{r_name}")).text_size(px(11.))),
        );
    }
    if let Some(rework) = &step.on_changes_requested {
        has_meta = true;
        let rw_label = step_titles
            .get(rework)
            .cloned()
            .unwrap_or_else(|| rework.clone());
        meta_pills = meta_pills.child(
            view()
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .bg(INSET)
                .border_1()
                .border_color(ATTENTION)
                .child(
                    label(format!("↺ Rework → {rw_label}"))
                        .text_size(px(11.))
                        .text_color(ATTENTION),
                ),
        );
    }
    if step.uses == "condition" {
        if let Some(then_id) = step.with.get("then").and_then(|v| v.as_str())
            && let Some(t_lbl) = step_titles.get(then_id)
        {
            has_meta = true;
            meta_pills = meta_pills.child(
                view()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.))
                    .bg(INSET)
                    .border_1()
                    .border_color(SUCCESS)
                    .child(
                        label(format!("✓ Then → {t_lbl}"))
                            .text_size(px(11.))
                            .text_color(SUCCESS),
                    ),
            );
        }
        if let Some(else_id) = step.with.get("otherwise").and_then(|v| v.as_str())
            && let Some(e_lbl) = step_titles.get(else_id)
        {
            has_meta = true;
            meta_pills = meta_pills.child(
                view()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.))
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .child(muted(format!("✕ Else → {e_lbl}")).text_size(px(11.))),
            );
        }
    } else if let Some(explicit_next) = &step.next
        && doc
            .steps
            .iter()
            .find(|s| &s.id == explicit_next)
            .is_some_and(|s| s.uses == "complete")
    {
        has_meta = true;
        meta_pills = meta_pills.child(
            view()
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .child(muted("■ Ends run").text_size(px(11.))),
        );
    }

    let mut card_btn = nsh_base::button()
        .w_full()
        .p_2p5()
        .gap_1p5()
        .rounded(px(RADIUS))
        .bg(if is_selected { SELECTED } else { SURFACE })
        .border_1()
        .border_color(if is_selected { ACCENT } else { BORDER })
        .hover(|s| s.border_color(ACCENT).bg(CARD_HOVER))
        .on_press(move |_: &()| state.selected_step.set(click_id.clone()))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .min_w(px(0.))
                        .flex_1()
                        .child(icon_tinted(step_icon, step_tone).w(px(14.)).h(px(14.)))
                        .child(
                            label(title)
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .min_w(px(0.)),
                        ),
                )
                .child(badge(kind_badge)),
        )
        .child(
            muted(detail_line)
                .text_size(px(12.))
                .truncate()
                .w_full()
                .min_w(px(0.)),
        );

    if has_meta {
        card_btn = card_btn.child(meta_pills);
    }
    card_btn
}

fn step_inspector(
    ui: Ui,
    doc: &source::Document,
    effective_columns: &[canban_types::WorkflowColumn],
    visible_steps: &[source::Step],
    active_step_id: &str,
    step_titles: &BTreeMap<String, String>,
) -> AnyElement {
    let state = ui.workflows;
    let Some((step_pos, step)) = visible_steps
        .iter()
        .enumerate()
        .find(|(_, s)| s.id == active_step_id)
    else {
        return view().into_element();
    };
    let step_id = step.id.clone();
    let step_title = step_titles
        .get(&step_id)
        .cloned()
        .unwrap_or_else(|| step.id.clone());
    let next_step_desc = step
        .next
        .clone()
        .or_else(|| visible_steps.get(step_pos + 1).map(|s| s.id.clone()))
        .map(|id| {
            if doc
                .steps
                .iter()
                .find(|s| s.id == id)
                .is_some_and(|s| s.uses == "complete")
            {
                "Complete workflow run".to_owned()
            } else {
                step_titles
                    .get(&id)
                    .map(|t| format!("{t} ({id})"))
                    .unwrap_or(id)
            }
        })
        .unwrap_or_else(|| "Complete workflow run".into());

    match step.uses.as_str() {
        "agent" => {
            let current_profile = step
                .with
                .get("profile")
                .and_then(|v| v.as_str())
                .unwrap_or("developer")
                .to_owned();
            let current_instruction = step
                .with
                .get("instruction")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let default_prompt = default_step_prompt(&step_id, Some(&current_profile));
            let is_default =
                is_default_step_prompt(&step_id, Some(&current_profile), &current_instruction);

            let prompt_sig = signal(current_instruction.clone());
            let step_for_effect = step_id.clone();
            effect(move || {
                let val = prompt_sig.get();
                if val != current_instruction {
                    let target_id = step_for_effect.clone();
                    update_doc(ui, |d| {
                        if let Some(s) = d.steps.iter_mut().find(|s| s.id == target_id) {
                            s.with.insert("instruction".into(), json!(val));
                        }
                    });
                }
            });

            let step_for_reset = step_id.clone();
            let step_for_prof = step_id.clone();
            let active_prof = current_profile.clone();
            let prompt_editor = canban_ui_kit::markdown::MarkdownInput::new();

            panel()
                .gap_3()
                .border_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(icon_tinted(Icon::Agent, ACCENT).w(px(15.)).h(px(15.)))
                                .child(
                                    label(format!("Step Inspector · {step_title} ({step_id})"))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(status(
                                    if is_default {
                                        "Default prompt"
                                    } else {
                                        "Customized prompt"
                                    },
                                    if is_default {
                                        Tone::Neutral
                                    } else {
                                        Tone::Info
                                    },
                                ))
                                .child(muted(format!("Next → {next_step_desc}"))),
                        )
                        .child(when(
                            move || !is_default,
                            move || {
                                let reset_id = step_for_reset.clone();
                                compact_action(
                                    "Reset to default prompt",
                                    Variant::Secondary,
                                    move |_| {
                                        let def = default_step_prompt(&reset_id, None).to_owned();
                                        prompt_sig.set(def.clone());
                                        let target = reset_id.clone();
                                        update_doc(ui, |d| {
                                            if let Some(s) =
                                                d.steps.iter_mut().find(|s| s.id == target)
                                            {
                                                s.with.insert("instruction".into(), json!(def));
                                            }
                                        });
                                    },
                                )
                            },
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(muted("Agent Profile:"))
                        .child(segmented().child(each(
                            move || ui.profiles.get(),
                            |p| p.id.clone(),
                            move |p, _| {
                                let p = p.get_untracked();
                                let pid = p.id.clone();
                                let is_prof = pid == active_prof;
                                let target = step_for_prof.clone();
                                compact_segment(
                                    format!("@{pid}"),
                                    None,
                                    move || is_prof,
                                    move |_| {
                                        let chosen = pid.clone();
                                        let tid = target.clone();
                                        update_doc(ui, |d| {
                                            if let Some(s) =
                                                d.steps.iter_mut().find(|s| s.id == tid)
                                            {
                                                s.with.insert("profile".into(), json!(chosen));
                                            }
                                        });
                                    },
                                )
                            },
                        ))),
                )
                .child(
                    view()
                        .h(px(180.))
                        .bg(INSET)
                        .rounded(px(RADIUS))
                        .p_2()
                        .child(
                            prompt_editor
                                .source(prompt_sig, "Step Prompt", default_prompt)
                                .size_full(),
                        ),
                )
                .into_element()
        }
        "deliver" | "card.deliver" => {
            let sync_upstream = step
                .with
                .get("sync_upstream")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let current_target = step
                .with
                .get("target_branch")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let branch_sig = signal(current_target.clone());
            let step_for_branch = step_id.clone();
            effect(move || {
                let val = branch_sig.get();
                if val != current_target {
                    let target_id = step_for_branch.clone();
                    let trimmed = val.trim().to_owned();
                    update_doc(ui, |d| {
                        if let Some(s) = d.steps.iter_mut().find(|s| s.id == target_id) {
                            if trimmed.is_empty() {
                                s.with.remove("target_branch");
                            } else {
                                s.with.insert("target_branch".into(), json!(trimmed));
                            }
                        }
                    });
                }
            });
            let step_sync_on = step_id.clone();
            let step_sync_off = step_id.clone();
            let current_rework = step.on_changes_requested.clone().unwrap_or_default();
            let agent_steps: Vec<(String, String)> = visible_steps
                .iter()
                .filter(|s| s.uses == "agent")
                .map(|s| {
                    let lbl = step_titles
                        .get(&s.id)
                        .cloned()
                        .unwrap_or_else(|| s.id.clone());
                    (s.id.clone(), lbl)
                })
                .collect();
            let mut rework_seg = segmented().child({
                let is_none = current_rework.is_empty();
                let tid = step_id.clone();
                compact_segment(
                    "None",
                    None,
                    move || is_none,
                    move |_| {
                        let target = tid.clone();
                        update_doc(ui, |d| {
                            if let Some(s) = d.steps.iter_mut().find(|s| s.id == target) {
                                s.on_changes_requested = None;
                            }
                        });
                    },
                )
            });
            for (aid, albl) in agent_steps {
                let is_sel = current_rework == aid;
                let tid = step_id.clone();
                let chosen = aid.clone();
                rework_seg = rework_seg.child(compact_segment(
                    albl,
                    None,
                    move || is_sel,
                    move |_| {
                        let target = tid.clone();
                        let rw = chosen.clone();
                        update_doc(ui, |d| {
                            if let Some(s) = d.steps.iter_mut().find(|s| s.id == target) {
                                s.on_changes_requested = Some(rw);
                            }
                        });
                    },
                ));
            }

            panel()
                .gap_3()
                .border_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(icon_tinted(Icon::Branch, SUCCESS).w(px(15.)).h(px(15.)))
                                .child(
                                    label(format!("Step Inspector · {step_title} ({step_id})"))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(badge("uses: deliver"))
                                .child(muted(format!("Next → {next_step_desc}"))),
                        )
                        .child(compact_action(
                            "Edit in YAML Source",
                            Variant::Secondary,
                            move |_| state.mode.set("yaml".into()),
                        )),
                )
                .child(muted(
                    "Queue Merge automatically integrates the Card worktree into the target branch. Clicking 'Hold auto-merge' on a Card pauses this step.",
                ))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_4()
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(muted("Upstream sync:"))
                                .child(
                                    segmented()
                                        .child(compact_segment(
                                            "Merge & Sync Upstream",
                                            None,
                                            move || sync_upstream,
                                            move |_| {
                                                let tid = step_sync_on.clone();
                                                update_doc(ui, |d| {
                                                    if let Some(s) =
                                                        d.steps.iter_mut().find(|s| s.id == tid)
                                                    {
                                                        s.with.insert(
                                                            "sync_upstream".into(),
                                                            json!(true),
                                                        );
                                                    }
                                                });
                                            },
                                        ))
                                        .child(compact_segment(
                                            "Merge Only (Local)",
                                            None,
                                            move || !sync_upstream,
                                            move |_| {
                                                let tid = step_sync_off.clone();
                                                update_doc(ui, |d| {
                                                    if let Some(s) =
                                                        d.steps.iter_mut().find(|s| s.id == tid)
                                                    {
                                                        s.with.insert(
                                                            "sync_upstream".into(),
                                                            json!(false),
                                                        );
                                                    }
                                                });
                                            },
                                        )),
                                ),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(muted("Target branch:"))
                                .child(
                                    field(branch_sig, "Project default (e.g. main)").w(px(190.)),
                                ),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(muted("Rework target:"))
                        .child(rework_seg),
                )
                .into_element()
        }
        "approval" => {
            let current_title = step
                .with
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let title_sig = signal(current_title.clone());
            let step_for_title = step_id.clone();
            effect(move || {
                let val = title_sig.get();
                if val != current_title {
                    let target_id = step_for_title.clone();
                    update_doc(ui, |d| {
                        if let Some(s) = d.steps.iter_mut().find(|s| s.id == target_id) {
                            s.with.insert("title".into(), json!(val));
                        }
                    });
                }
            });
            let current_rework = step.on_changes_requested.clone().unwrap_or_default();
            let agent_steps: Vec<(String, String)> = visible_steps
                .iter()
                .filter(|s| s.uses == "agent")
                .map(|s| {
                    let lbl = step_titles
                        .get(&s.id)
                        .cloned()
                        .unwrap_or_else(|| s.id.clone());
                    (s.id.clone(), lbl)
                })
                .collect();
            let mut rework_seg = segmented().child({
                let is_none = current_rework.is_empty();
                let tid = step_id.clone();
                compact_segment(
                    "None",
                    None,
                    move || is_none,
                    move |_| {
                        let target = tid.clone();
                        update_doc(ui, |d| {
                            if let Some(s) = d.steps.iter_mut().find(|s| s.id == target) {
                                s.on_changes_requested = None;
                            }
                        });
                    },
                )
            });
            for (aid, albl) in agent_steps {
                let is_sel = current_rework == aid;
                let tid = step_id.clone();
                let chosen = aid.clone();
                rework_seg = rework_seg.child(compact_segment(
                    albl,
                    None,
                    move || is_sel,
                    move |_| {
                        let target = tid.clone();
                        let rw = chosen.clone();
                        update_doc(ui, |d| {
                            if let Some(s) = d.steps.iter_mut().find(|s| s.id == target) {
                                s.on_changes_requested = Some(rw);
                            }
                        });
                    },
                ));
            }
            panel()
                .gap_3()
                .border_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(icon_tinted(Icon::Check, ATTENTION).w(px(15.)).h(px(15.)))
                                .child(
                                    label(format!("Step Inspector · {step_title} ({step_id})"))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(badge("uses: approval"))
                                .child(muted(format!("Next → {next_step_desc}"))),
                        )
                        .child(compact_action(
                            "Edit in YAML Source",
                            Variant::Secondary,
                            move |_| state.mode.set("yaml".into()),
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(muted("Gate message:"))
                        .child(
                            field(title_sig, "Human review is required before continuing")
                                .flex_1()
                                .min_w(px(240.)),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(muted("Rework target:"))
                        .child(rework_seg),
                )
                .into_element()
        }
        "card.move" => {
            let current_col = step
                .with
                .get("column")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let mut col_seg = segmented();
            for col in effective_columns {
                let cid = col.id.clone();
                let cname = col.name.clone();
                let is_sel = current_col == cid;
                let tid = step_id.clone();
                col_seg = col_seg.child(compact_segment(
                    cname,
                    None,
                    move || is_sel,
                    move |_| {
                        let chosen = cid.clone();
                        let target = tid.clone();
                        update_doc(ui, |d| {
                            if let Some(s) = d.steps.iter_mut().find(|s| s.id == target) {
                                s.with.insert("column".into(), json!(chosen));
                            }
                        });
                    },
                ));
            }
            panel()
                .gap_3()
                .border_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(icon_tinted(Icon::Forward, ACCENT).w(px(15.)).h(px(15.)))
                                .child(
                                    label(format!("Step Inspector · {step_title} ({step_id})"))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(badge("uses: card.move"))
                                .child(muted(format!("Next → {next_step_desc}"))),
                        )
                        .child(compact_action(
                            "Edit in YAML Source",
                            Variant::Secondary,
                            move |_| state.mode.set("yaml".into()),
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(muted("Destination stage:"))
                        .child(col_seg),
                )
                .into_element()
        }
        other => {
            let summary = match other {
                "condition" => format!(
                    "Evaluates JSON pointer `{}` == `{}` and routes to `{}` (then) or `{}` (otherwise).",
                    step.with
                        .get("pointer")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    step.with.get("equals").unwrap_or(&Value::Null),
                    step.with
                        .get("then")
                        .and_then(|v| v.as_str())
                        .unwrap_or("end"),
                    step.with
                        .get("otherwise")
                        .and_then(|v| v.as_str())
                        .unwrap_or("end"),
                ),
                "notify" => format!(
                    "Posts a Card attention notification: {}",
                    step.with
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                ),
                _ => "Structural workflow step.".to_owned(),
            };
            let mut transition_row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(badge(format!("uses: {other}")));
            if other != "condition" {
                transition_row = transition_row.child(muted(format!("Next → {next_step_desc}")));
            }
            panel()
                .gap_2p5()
                .border_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            label(format!("Step Inspector · {step_title} ({step_id})"))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(compact_action(
                            "Edit in YAML Source",
                            Variant::Secondary,
                            move |_| state.mode.set("yaml".into()),
                        )),
                )
                .child(muted(summary))
                .child(transition_row)
                .into_element()
        }
    }
}

fn options_bar(ui: Ui, opts: source::WorkflowOptions) -> View {
    let analyze = opts.backlog_analyze;
    let auto_dev = opts.backlog_auto_develop;
    let review_manual = opts.review_manual;
    let sync_upstream = opts.sync_upstream;
    let ext = opts.external_pr;
    let ext1 = ext.clone();
    let ext2 = ext.clone();
    let ext3 = ext;

    panel()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .gap_0p5()
                .child(label("Workflow Automation Options").font_weight(FontWeight::SEMIBOLD))
                .child(muted(
                    "Derived directly from the workflow graph. Toggling options surgically updates Backlog or Review rules while preserving custom columns, custom steps, and custom prompts.",
                )),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_4()
                .child(
                    view()
                        .gap_1()
                        .child(label("1. Backlog Auto-Analyze"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Off",
                                    None,
                                    move || !analyze,
                                    move |_| {
                                        update_doc(ui, |d| {
                                            source::set_backlog_options(d, false, auto_dev)
                                        });
                                    },
                                ))
                                .child(compact_segment(
                                    "Auto-Analyze",
                                    None,
                                    move || analyze,
                                    move |_| {
                                        update_doc(ui, |d| {
                                            source::set_backlog_options(d, true, auto_dev)
                                        });
                                    },
                                )),
                        ),
                )
                .child(
                    view()
                        .gap_1()
                        .child(label("2. Backlog Auto-Start Develop"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Off (Stay in Backlog)",
                                    None,
                                    move || !auto_dev,
                                    move |_| {
                                        update_doc(ui, |d| {
                                            source::set_backlog_options(d, analyze, false)
                                        });
                                    },
                                ))
                                .child(compact_segment(
                                    if analyze {
                                        "On (After Analysis)"
                                    } else {
                                        "On (Move Immediately)"
                                    },
                                    None,
                                    move || auto_dev,
                                    move |_| {
                                        update_doc(ui, |d| {
                                            source::set_backlog_options(d, analyze, true)
                                        });
                                    },
                                )),
                        ),
                )
                .child(
                    view()
                        .gap_1()
                        .child(label("3. Review Mode"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Queue Merge (Auto)",
                                    None,
                                    move || !review_manual,
                                    move |_| {
                                        update_doc(ui, |d| source::set_review_manual(d, false));
                                    },
                                ))
                                .child(compact_segment(
                                    "Manual Review",
                                    None,
                                    move || review_manual,
                                    move |_| {
                                        update_doc(ui, |d| source::set_review_manual(d, true));
                                    },
                                )),
                        ),
                )
                .child(
                    view()
                        .gap_1()
                        .opacity(if review_manual { 0.65 } else { 1.0 })
                        .child(label("4. Upstream Remote Sync"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Merge & Sync Upstream",
                                    None,
                                    move || !review_manual && sync_upstream,
                                    move |_| {
                                        update_doc(ui, |d| source::set_sync_upstream(d, true));
                                    },
                                ))
                                .child(compact_segment(
                                    "Merge Only (No Sync)",
                                    None,
                                    move || !review_manual && !sync_upstream,
                                    move |_| {
                                        update_doc(ui, |d| source::set_sync_upstream(d, false));
                                    },
                                )),
                        ),
                )
                .child(
                    view()
                        .gap_1()
                        .child(label("5. External PR Rule"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Require Human Review",
                                    None,
                                    move || ext1 == "require_review",
                                    move |_| {
                                        update_doc(ui, |d| {
                                            source::set_external_pr_rule(d, "require_review")
                                        });
                                    },
                                ))
                                .child(compact_segment(
                                    "Same as Internal",
                                    None,
                                    move || ext2 == "same",
                                    move |_| {
                                        update_doc(ui, |d| source::set_external_pr_rule(d, "same"));
                                    },
                                ))
                                .child(compact_segment(
                                    "Manual Only",
                                    None,
                                    move || ext3 == "manual_only",
                                    move |_| {
                                        update_doc(ui, |d| {
                                            source::set_external_pr_rule(d, "manual_only")
                                        });
                                    },
                                )),
                        ),
                ),
        )
}
