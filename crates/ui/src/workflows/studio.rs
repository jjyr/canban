//! Stage-aligned visual workflow studio with fine-grained layout-only updates.
//! Stage columns, inline stage automation switches, step cards, and the Step
//! Inspector stay mounted across selection, option toggles, and prompt edits.
use super::update_doc;
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use canban_workflow_engine::source::{self, default_step_prompt, is_default_step_prompt};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PillTone {
    Muted,
    Success,
    Attention,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PillModel {
    key: String,
    text: String,
    tone: PillTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeTone {
    Accent,
    Success,
    Attention,
    Muted,
}

impl NodeTone {
    fn color(self) -> AdaptiveColor {
        match self {
            Self::Accent => ACCENT.into(),
            Self::Success => SUCCESS.into(),
            Self::Attention => ATTENTION.into(),
            Self::Muted => MUTED.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeModel {
    id: String,
    connector: Option<String>,
    icon: Icon,
    tone: NodeTone,
    title: String,
    kind_badge: String,
    detail_line: String,
    pills: Vec<PillModel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LaneModel {
    id: String,
    name: String,
    can_remove: bool,
    has_custom_steps: bool,
    nodes: Vec<NodeModel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StepInspectorModel {
    id: String,
    title: String,
    uses: String,
    next_desc: String,
    profile: String,
    instruction: String,
    is_default_prompt: bool,
    sync_upstream: bool,
    target_branch: String,
    rework_target: String,
    gate_title: String,
    move_column: String,
    other_summary: String,
}

#[derive(Clone, Debug, PartialEq)]
struct GraphModel {
    has_doc: bool,
    workflow_name: String,
    workflow_id: String,
    workflow_enabled: bool,
    options: source::WorkflowOptions,
    columns: Vec<canban_types::WorkflowColumn>,
    lanes: Vec<LaneModel>,
    steps: Vec<StepInspectorModel>,
    agent_targets: Vec<(String, String)>,
}

impl Default for GraphModel {
    fn default() -> Self {
        Self {
            has_doc: false,
            workflow_name: String::new(),
            workflow_id: String::new(),
            workflow_enabled: true,
            options: source::WorkflowOptions {
                backlog_analyze: false,
                backlog_auto_develop: false,
                review_manual: false,
                sync_upstream: true,
                external_pr: "require_review".into(),
            },
            columns: Vec::new(),
            lanes: Vec::new(),
            steps: Vec::new(),
            agent_targets: Vec::new(),
        }
    }
}

pub(super) fn render(ui: Ui) -> AnyElement {
    let state = ui.workflows;

    let fallback_columns = computed(move || {
        let project_id = ui.project.get();
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|p| {
                    p.workflow
                        .stages
                        .iter()
                        .map(|st| canban_types::WorkflowColumn {
                            id: st.id.clone(),
                            name: st.name.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|cols| !cols.is_empty())
                .unwrap_or_else(default_columns)
        })
    });

    let graph = computed(move || {
        let fallback = fallback_columns.get();
        state.studio_doc.with(|opt| match opt {
            Some(doc) => build_graph_model(doc, &fallback),
            None => GraphModel::default(),
        })
    });

    let active_step_id = computed(move || {
        let requested = state.selected_step.get();
        graph.with(|g| {
            if g.steps.iter().any(|s| s.id == requested) {
                requested
            } else {
                g.steps
                    .iter()
                    .find(|s| s.uses == "agent")
                    .or_else(|| g.steps.first())
                    .map(|s| s.id.clone())
                    .unwrap_or_default()
            }
        })
    });

    let active_step = computed(move || {
        let id = active_step_id.get();
        graph.with(|g| g.steps.iter().find(|s| s.id == id).cloned())
    });

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(when(
            move || state.selected.get().is_empty(),
            move || {
                view()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(muted("Choose or create a workflow"))
            },
        ))
        .child(when(
            move || !state.selected.get().is_empty() && !state.studio_error.get().is_empty(),
            move || {
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .gap_3()
                    .child(notice(
                        move || {
                            format!(
                                "YAML syntax or schema error: {}. Switch to YAML Source to fix it.",
                                state.studio_error.get()
                            )
                        },
                        Tone::Danger,
                    ))
                    .child(
                        action("Open YAML Source", Variant::Primary, move |_| {
                            state.mode.set("yaml".into());
                        })
                        .self_start(),
                    )
            },
        ))
        .child(when(
            move || {
                !state.selected.get().is_empty()
                    && state.studio_error.get().is_empty()
                    && graph.with(|g| g.has_doc)
            },
            move || {
                scroll().flex_1().min_h(px(0.)).child(
                    view()
                        .gap_3()
                        .pb_4()
                        .child(stage_canvas(ui, graph, active_step_id))
                        .child(step_inspector(ui, graph, active_step)),
                )
            },
        ))
        .into_element()
}

fn build_graph_model(
    doc: &source::Document,
    fallback_columns: &[canban_types::WorkflowColumn],
) -> GraphModel {
    let options = source::derive_options(doc);
    let effective_columns = if !doc.columns.is_empty() {
        doc.columns.clone()
    } else {
        fallback_columns.to_vec()
    };
    let visible_steps = ordered_visible_steps(doc);
    let col_names: BTreeMap<String, String> = effective_columns
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    let step_titles: BTreeMap<String, String> = doc
        .steps
        .iter()
        .map(|s| (s.id.clone(), humanize_step_title(s, &col_names)))
        .collect();

    let first_col_id = effective_columns
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "backlog".into());
    let mut lane_steps: BTreeMap<String, Vec<source::Step>> = effective_columns
        .iter()
        .map(|c| (c.id.clone(), Vec::new()))
        .collect();
    let mut current_lane = first_col_id;
    for step in &visible_steps {
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

    let stage_count = effective_columns.len();
    let lanes = effective_columns
        .iter()
        .map(|col| {
            let steps_in_col = lane_steps.get(&col.id).cloned().unwrap_or_default();
            let has_custom_steps = match col.id.as_str() {
                "backlog" => steps_in_col.iter().any(|s| {
                    !matches!(
                        (s.id.as_str(), s.uses.as_str()),
                        ("agent:plan" | "plan", "agent")
                            | ("move:backlog" | "enter_develop", "card.move")
                    )
                }),
                "review" => steps_in_col.iter().any(|s| s.uses == "agent"),
                _ => false,
            };
            let nodes = steps_in_col
                .iter()
                .enumerate()
                .map(|(idx, step)| {
                    let connector = if idx == 0 {
                        None
                    } else {
                        step_connector_label(doc, &steps_in_col[..idx], step)
                    };
                    build_node_model(doc, step, connector, &col_names, &step_titles)
                })
                .collect();
            LaneModel {
                id: col.id.clone(),
                name: col.name.clone(),
                can_remove: stage_count > 1
                    && !matches!(col.id.as_str(), "backlog" | "develop" | "review"),
                has_custom_steps,
                nodes,
            }
        })
        .collect();

    let agent_targets = visible_steps
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

    let steps = visible_steps
        .iter()
        .enumerate()
        .map(|(pos, step)| build_inspector_model(doc, &visible_steps, pos, step, &step_titles))
        .collect();

    GraphModel {
        has_doc: true,
        workflow_name: doc.name.clone(),
        workflow_id: doc.id.clone(),
        workflow_enabled: doc.enabled,
        options,
        columns: effective_columns,
        lanes,
        steps,
        agent_targets,
    }
}

fn step_connector_label(
    doc: &source::Document,
    preceding_in_lane: &[source::Step],
    current: &source::Step,
) -> Option<String> {
    for prev in preceding_in_lane.iter().rev() {
        if prev.uses == "condition" {
            if prev.with.get("then").and_then(|v| v.as_str()) == Some(current.id.as_str()) {
                return Some("↳ Then".into());
            }
            if prev.with.get("otherwise").and_then(|v| v.as_str()) == Some(current.id.as_str()) {
                return Some("↳ Else".into());
            }
        }
    }
    if let Some(prev) = preceding_in_lane.last()
        && prev.uses != "condition"
    {
        let prev_next = prev.next.as_deref().or_else(|| {
            doc.steps
                .iter()
                .position(|s| s.id == prev.id)
                .and_then(|i| doc.steps.get(i + 1))
                .map(|s| s.id.as_str())
        });
        if prev_next == Some(current.id.as_str()) {
            return Some("↓".into());
        }
    }
    None
}

fn build_node_model(
    doc: &source::Document,
    step: &source::Step,
    connector: Option<String>,
    col_names: &BTreeMap<String, String>,
    step_titles: &BTreeMap<String, String>,
) -> NodeModel {
    let step_id = step.id.clone();
    let title = step_titles
        .get(&step_id)
        .cloned()
        .unwrap_or_else(|| step.id.clone());

    let (icon, tone, kind_badge, detail_line) = match step.uses.as_str() {
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
            (
                Icon::Agent,
                NodeTone::Accent,
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
                NodeTone::Accent,
                "Stage Move".into(),
                format!("Move Card into {target_name}"),
            )
        }
        "approval" => {
            let gate = step
                .with
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("Pause for human review & approval");
            (
                Icon::Check,
                NodeTone::Attention,
                "Human Gate".into(),
                gate.to_owned(),
            )
        }
        "deliver" => {
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
            (Icon::Branch, NodeTone::Success, badge_txt.into(), detail)
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
                NodeTone::Attention,
                "Condition".into(),
                format!("{ptr} == {eq}"),
            )
        }
        other => (
            Icon::Info,
            NodeTone::Muted,
            other.to_owned(),
            "Workflow step".into(),
        ),
    };

    let mut pills = Vec::new();
    for (rule_name, rule) in &doc.on {
        if rule.enabled && rule.start_at == step_id {
            pills.push(PillModel {
                key: format!("on:{rule_name}"),
                text: format!("⚡ on:{rule_name}"),
                tone: PillTone::Muted,
            });
        }
    }
    if let Some(rework) = &step.on_changes_requested {
        let rw_label = step_titles
            .get(rework)
            .cloned()
            .unwrap_or_else(|| rework.clone());
        pills.push(PillModel {
            key: "rework".into(),
            text: format!("↺ Rework → {rw_label}"),
            tone: PillTone::Attention,
        });
    }
    if step.uses == "condition" {
        if let Some(then_id) = step.with.get("then").and_then(|v| v.as_str())
            && let Some(t_lbl) = step_titles.get(then_id)
        {
            pills.push(PillModel {
                key: "then".into(),
                text: format!("✓ Then → {t_lbl}"),
                tone: PillTone::Success,
            });
        }
        if let Some(else_id) = step.with.get("otherwise").and_then(|v| v.as_str())
            && let Some(e_lbl) = step_titles.get(else_id)
        {
            pills.push(PillModel {
                key: "else".into(),
                text: format!("✕ Else → {e_lbl}"),
                tone: PillTone::Muted,
            });
        }
    } else if let Some(explicit_next) = &step.next
        && doc
            .steps
            .iter()
            .find(|s| &s.id == explicit_next)
            .is_some_and(|s| s.uses == "complete")
    {
        pills.push(PillModel {
            key: "end".into(),
            text: "■ Ends run".into(),
            tone: PillTone::Muted,
        });
    }

    NodeModel {
        id: step_id,
        connector,
        icon,
        tone,
        title,
        kind_badge,
        detail_line,
        pills,
    }
}

fn build_inspector_model(
    doc: &source::Document,
    visible_steps: &[source::Step],
    step_pos: usize,
    step: &source::Step,
    step_titles: &BTreeMap<String, String>,
) -> StepInspectorModel {
    let step_id = step.id.clone();
    let title = step_titles
        .get(&step_id)
        .cloned()
        .unwrap_or_else(|| step.id.clone());
    let next_desc = step
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

    let profile = step
        .with
        .get("profile")
        .and_then(|v| v.as_str())
        .unwrap_or("developer")
        .to_owned();
    let instruction = step
        .with
        .get("instruction")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let is_default_prompt = is_default_step_prompt(&step_id, Some(&profile), &instruction);
    let sync_upstream = step
        .with
        .get("sync_upstream")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let target_branch = step
        .with
        .get("target_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let rework_target = step.on_changes_requested.clone().unwrap_or_default();
    let gate_title = step
        .with
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let move_column = step
        .with
        .get("column")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let other_summary = match step.uses.as_str() {
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

    StepInspectorModel {
        id: step_id,
        title,
        uses: step.uses.clone(),
        next_desc,
        profile,
        instruction,
        is_default_prompt,
        sync_upstream,
        target_branch,
        rework_target,
        gate_title,
        move_column,
        other_summary,
    }
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
        (_, "deliver") => "Queue merge".into(),
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

fn stage_canvas(ui: Ui, graph: Memo<GraphModel>, active_step_id: Memo<String>) -> AnyElement {
    let state = ui.workflows;

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
                let mut updated_cols = graph.with_untracked(|g| g.columns.clone());
                if slug.is_empty() || updated_cols.iter().any(|c| c.id == slug) {
                    return;
                }
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

    let lanes_row = view()
        .flex_row()
        .items_stretch()
        .gap_2p5()
        .flex_1()
        .child(each(
            move || graph.with(|g| g.lanes.clone()),
            |lane| lane.id.clone(),
            move |lane, col_idx| stage_lane(ui, graph, lane, col_idx, active_step_id),
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
                        .min_w(px(0.))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .min_w(px(0.))
                                .child(icon_tinted(Icon::Trigger, ACCENT).w(px(15.)).h(px(15.)))
                                .child(
                                    label("Stage Execution Graph")
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(muted("·"))
                                .child(
                                    label(move || graph.with(|g| g.workflow_name.clone()))
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate(),
                                )
                                .child(badge(move || {
                                    graph.with(|g| short_workflow_id(&g.workflow_id))
                                }))
                                .child(when(
                                    move || graph.with(|g| g.workflow_enabled),
                                    move || status("Active", Tone::Success),
                                ))
                                .child(when(
                                    move || graph.with(|g| !g.workflow_enabled),
                                    move || status("Disabled", Tone::Neutral),
                                ))
                                .child(muted(move || {
                                    graph.with(|g| {
                                        format!("{} stages · {} steps", g.lanes.len(), g.steps.len())
                                    })
                                })),
                        )
                        .child(muted(
                            "Configure stage automation inline or click any step node below to customize its Agent & Prompt.",
                        )),
                )
                .child(add_col_controls),
        )
        .child(scroll().horizontal().w_full().child(lanes_row))
        .into_element()
}

fn stage_lane(
    ui: Ui,
    graph: Memo<GraphModel>,
    lane: ReadSignal<LaneModel>,
    col_idx: ReadSignal<usize>,
    active_step_id: Memo<String>,
) -> impl IntoElement {
    let col_id = lane.with_untracked(|l| l.id.clone());
    let is_backlog = col_id == "backlog";
    let is_review = col_id == "review";
    let remove_id = col_id.clone();

    let header = view()
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
                        .child(muted(move || format!("{}", col_idx.get() + 1)).text_size(px(11.))),
                )
                .child(
                    label(move || lane.with(|l| l.name.clone()))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate(),
                )
                .child(muted(format!("({col_id})")).text_size(px(11.))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(when(
                    move || lane.with(|l| l.has_custom_steps),
                    move || badge("Custom"),
                ))
                .child(badge(move || {
                    let count = lane.with(|l| l.nodes.len());
                    format!("{count} step{}", if count == 1 { "" } else { "s" })
                }))
                .child(when(
                    move || lane.with(|l| l.can_remove),
                    move || {
                        let target = remove_id.clone();
                        compact_action("×", Variant::Quiet, move |_| {
                            let remaining: Vec<_> = graph
                                .with_untracked(|g| g.columns.clone())
                                .into_iter()
                                .filter(|c| c.id != target)
                                .collect();
                            update_doc(ui, |d| {
                                d.columns = remaining;
                            });
                        })
                    },
                )),
        );

    let lane_card = view()
        .w(px(260.))
        .flex_1()
        .min_w(px(248.))
        .p_3()
        .gap_2p5()
        .rounded(px(RADIUS))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .child(header)
        .child(when(
            move || is_backlog,
            move || backlog_stage_controls(ui, graph),
        ))
        .child(when(
            move || is_review,
            move || review_stage_controls(ui, graph),
        ))
        .child(
            view()
                .gap_1p5()
                .flex_1()
                .child(when(
                    move || lane.with(|l| l.nodes.is_empty()),
                    move || {
                        view()
                            .py_3p5()
                            .px_3()
                            .rounded(px(RADIUS))
                            .border_1()
                            .border_color(BORDER)
                            .bg(BACKGROUND)
                            .opacity(0.68)
                            .items_center()
                            .justify_center()
                            .child(muted(if is_backlog {
                                "Quiet intake · Cards stay in Backlog"
                            } else {
                                "No automated steps in this stage"
                            }))
                    },
                ))
                .child(each(
                    move || lane.with(|l| l.nodes.clone()),
                    |node| node.id.clone(),
                    move |node, _| {
                        view()
                            .w_full()
                            .gap_1()
                            .child(when(
                                move || node.with(|n| n.connector.is_some()),
                                move || {
                                    view().items_center().py_0p5().child(
                                        muted(move || {
                                            node.with(|n| n.connector.clone().unwrap_or_default())
                                        })
                                        .text_size(px(11.)),
                                    )
                                },
                            ))
                            .child(step_node_card(ui, node, active_step_id))
                    },
                )),
        );

    view()
        .flex_row()
        .items_stretch()
        .gap_2p5()
        .flex_1()
        .min_w(px(256.))
        .child(when(
            move || col_idx.get() > 0,
            move || {
                view()
                    .justify_center()
                    .items_center()
                    .child(icon(Icon::Forward).w(px(14.)).h(px(14.)).opacity(0.55))
            },
        ))
        .child(lane_card)
}

fn backlog_stage_controls(ui: Ui, graph: Memo<GraphModel>) -> View {
    view()
        .p_2()
        .gap_1p5()
        .rounded(px(RADIUS))
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1p5()
                .child(muted("New Card analyze"))
                .child(
                    segmented()
                        .child(compact_segment(
                            "Off",
                            None,
                            move || graph.with(|g| !g.options.backlog_analyze),
                            move |_| {
                                let auto_dev =
                                    graph.with_untracked(|g| g.options.backlog_auto_develop);
                                update_doc(ui, |d| source::set_backlog_options(d, false, auto_dev));
                            },
                        ))
                        .child(compact_segment(
                            "Auto",
                            None,
                            move || graph.with(|g| g.options.backlog_analyze),
                            move |_| {
                                let auto_dev =
                                    graph.with_untracked(|g| g.options.backlog_auto_develop);
                                update_doc(ui, |d| source::set_backlog_options(d, true, auto_dev));
                            },
                        )),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1p5()
                .child(muted("Auto-start Develop"))
                .child(
                    segmented()
                        .child(compact_segment(
                            "Stay",
                            None,
                            move || graph.with(|g| !g.options.backlog_auto_develop),
                            move |_| {
                                let analyze = graph.with_untracked(|g| g.options.backlog_analyze);
                                update_doc(ui, |d| source::set_backlog_options(d, analyze, false));
                            },
                        ))
                        .child(compact_segment(
                            "Auto-Develop",
                            None,
                            move || graph.with(|g| g.options.backlog_auto_develop),
                            move |_| {
                                let analyze = graph.with_untracked(|g| g.options.backlog_analyze);
                                update_doc(ui, |d| source::set_backlog_options(d, analyze, true));
                            },
                        )),
                ),
        )
}

fn review_stage_controls(ui: Ui, graph: Memo<GraphModel>) -> View {
    view()
        .p_2()
        .gap_1p5()
        .rounded(px(RADIUS))
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1p5()
                .child(muted("Review delivery"))
                .child(
                    segmented()
                        .child(compact_segment(
                            "Queue Merge",
                            None,
                            move || graph.with(|g| !g.options.review_manual),
                            move |_| {
                                update_doc(ui, |d| source::set_review_manual(d, false));
                            },
                        ))
                        .child(compact_segment(
                            "Manual",
                            None,
                            move || graph.with(|g| g.options.review_manual),
                            move |_| {
                                update_doc(ui, |d| source::set_review_manual(d, true));
                            },
                        )),
                ),
        )
        .child(when(
            move || graph.with(|g| !g.options.review_manual),
            move || {
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_1p5()
                    .child(muted("Upstream"))
                    .child(
                        segmented()
                            .child(compact_segment(
                                "Sync",
                                None,
                                move || graph.with(|g| g.options.sync_upstream),
                                move |_| {
                                    update_doc(ui, |d| source::set_sync_upstream(d, true));
                                },
                            ))
                            .child(compact_segment(
                                "Local Only",
                                None,
                                move || graph.with(|g| !g.options.sync_upstream),
                                move |_| {
                                    update_doc(ui, |d| source::set_sync_upstream(d, false));
                                },
                            )),
                    )
            },
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1p5()
                .child(muted("External PR"))
                .child(
                    segmented()
                        .child(compact_segment(
                            "Human",
                            None,
                            move || graph.with(|g| g.options.external_pr == "require_review"),
                            move |_| {
                                update_doc(ui, |d| {
                                    source::set_external_pr_rule(d, "require_review")
                                });
                            },
                        ))
                        .child(compact_segment(
                            "Same",
                            None,
                            move || graph.with(|g| g.options.external_pr == "same"),
                            move |_| {
                                update_doc(ui, |d| source::set_external_pr_rule(d, "same"));
                            },
                        ))
                        .child(compact_segment(
                            "Manual",
                            None,
                            move || graph.with(|g| g.options.external_pr == "manual_only"),
                            move |_| {
                                update_doc(ui, |d| source::set_external_pr_rule(d, "manual_only"));
                            },
                        )),
                ),
        )
}

fn step_node_card(
    ui: Ui,
    node: ReadSignal<NodeModel>,
    active_step_id: Memo<String>,
) -> impl IntoElement {
    let state = ui.workflows;
    let step_id = node.with_untracked(|n| n.id.clone());
    let click_id = step_id.clone();
    let sel_id = step_id.clone();
    let border_id = step_id.clone();
    let initial_icon = node.with_untracked(|n| n.icon);

    canban_ui_kit::press_target()
        .w_full()
        .p_2p5()
        .gap_1p5()
        .rounded(px(RADIUS))
        .bg(move || {
            if active_step_id.get() == sel_id {
                SELECTED
            } else {
                SURFACE
            }
        })
        .border_1()
        .border_color(move || {
            if active_step_id.get() == border_id {
                ACCENT
            } else {
                BORDER
            }
        })
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
                        .child(
                            icon_tinted(initial_icon, move || node.with(|n| n.tone.color()))
                                .w(px(14.))
                                .h(px(14.)),
                        )
                        .child(
                            label(move || node.with(|n| n.title.clone()))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .min_w(px(0.)),
                        ),
                )
                .child(badge(move || node.with(|n| n.kind_badge.clone()))),
        )
        .child(
            muted(move || node.with(|n| n.detail_line.clone()))
                .text_size(px(12.))
                .truncate()
                .w_full()
                .min_w(px(0.)),
        )
        .child(when(
            move || node.with(|n| !n.pills.is_empty()),
            move || {
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(each(
                        move || node.with(|n| n.pills.clone()),
                        |p| p.key.clone(),
                        move |pill, _| {
                            view()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .bg(INSET)
                                .border_1()
                                .border_color(move || match pill.with(|p| p.tone) {
                                    PillTone::Muted => BORDER,
                                    PillTone::Success => SUCCESS,
                                    PillTone::Attention => ATTENTION,
                                })
                                .child(
                                    label(move || pill.with(|p| p.text.clone()))
                                        .text_size(px(11.))
                                        .text_color(move || match pill.with(|p| p.tone) {
                                            PillTone::Muted => MUTED,
                                            PillTone::Success => SUCCESS,
                                            PillTone::Attention => ATTENTION,
                                        }),
                                )
                        },
                    ))
            },
        ))
}

fn step_inspector(
    ui: Ui,
    graph: Memo<GraphModel>,
    active_step: Memo<Option<StepInspectorModel>>,
) -> AnyElement {
    let state = ui.workflows;
    let prompt_sig = signal(String::new());
    let branch_sig = signal(String::new());
    let gate_title_sig = signal(String::new());
    let syncing = stored(false);
    let bound_step_id = stored(String::new());

    let prompt_editor = canban_ui_kit::markdown::MarkdownInput::new();
    prompt_editor.restore_after(
        computed(move || state.agent.get() || state.history_open.get() || ui.quick_open.get()),
        computed(move || ui.page.get() == "workflows" && state.mode.get() != "yaml"),
    );

    // Sync bound editor signals whenever active step changes or external YAML changes.
    effect(move || {
        let Some(step) = active_step.get() else {
            bound_step_id.replace(String::new());
            return;
        };
        let step_changed = bound_step_id.get() != step.id;
        bound_step_id.replace(step.id.clone());
        syncing.replace(true);
        if (step_changed || !prompt_editor.focused.get_untracked())
            && prompt_sig.get_untracked() != step.instruction
        {
            prompt_sig.set(step.instruction);
        }
        if (step_changed || branch_sig.get_untracked().trim() != step.target_branch)
            && branch_sig.get_untracked() != step.target_branch
        {
            branch_sig.set(step.target_branch);
        }
        if gate_title_sig.get_untracked() != step.gate_title {
            gate_title_sig.set(step.gate_title);
        }
        syncing.replace(false);
    });

    // Incremental in-memory updates from bound fields (never remounts the inspector).
    effect(move || {
        let val = prompt_sig.get();
        if syncing.get() {
            return;
        }
        let Some(step) = active_step.get_untracked() else {
            return;
        };
        if step.uses == "agent" && val != step.instruction {
            let target_id = step.id;
            update_doc(ui, |d| {
                if let Some(s) = d.steps.iter_mut().find(|s| s.id == target_id) {
                    s.with.insert("instruction".into(), json!(val));
                }
            });
        }
    });

    effect(move || {
        let val = branch_sig.get();
        if syncing.get() {
            return;
        }
        let Some(step) = active_step.get_untracked() else {
            return;
        };
        let trimmed = val.trim().to_owned();
        if step.uses == "deliver" && trimmed != step.target_branch {
            let target_id = step.id;
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

    effect(move || {
        let val = gate_title_sig.get();
        if syncing.get() {
            return;
        }
        let Some(step) = active_step.get_untracked() else {
            return;
        };
        if step.uses == "approval" && val != step.gate_title {
            let target_id = step.id;
            update_doc(ui, |d| {
                if let Some(s) = d.steps.iter_mut().find(|s| s.id == target_id) {
                    s.with.insert("title".into(), json!(val));
                }
            });
        }
    });

    let active_uses = computed(move || {
        active_step.with(|s| s.as_ref().map(|st| st.uses.clone()).unwrap_or_default())
    });

    let rework_selector = move || {
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(muted("Rework target:"))
            .child(
                segmented()
                    .child(compact_segment(
                        "None",
                        None,
                        move || {
                            active_step
                                .with(|s| s.as_ref().is_none_or(|st| st.rework_target.is_empty()))
                        },
                        move |_| {
                            let Some(step) = active_step.get_untracked() else {
                                return;
                            };
                            update_doc(ui, |d| {
                                if let Some(s) = d.steps.iter_mut().find(|s| s.id == step.id) {
                                    s.on_changes_requested = None;
                                }
                            });
                        },
                    ))
                    .child(each(
                        move || graph.with(|g| g.agent_targets.clone()),
                        |(id, _)| id.clone(),
                        move |target, _| {
                            let (aid, albl) = target.get_untracked();
                            let sel_id = aid.clone();
                            let click_id = aid;
                            compact_segment(
                                albl,
                                None,
                                move || {
                                    active_step.with(|s| {
                                        s.as_ref().is_some_and(|st| st.rework_target == sel_id)
                                    })
                                },
                                move |_| {
                                    let Some(step) = active_step.get_untracked() else {
                                        return;
                                    };
                                    let chosen = click_id.clone();
                                    update_doc(ui, |d| {
                                        if let Some(s) =
                                            d.steps.iter_mut().find(|s| s.id == step.id)
                                        {
                                            s.on_changes_requested = Some(chosen);
                                        }
                                    });
                                },
                            )
                        },
                    )),
            )
    };

    when(
        move || active_step.with(|s| s.is_some()),
        move || {
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
                                .child(
                                    label(move || {
                                        active_step.with(|s| match s {
                                            Some(st) => {
                                                format!("Step Inspector · {} ({})", st.title, st.id)
                                            }
                                            None => "Step Inspector".into(),
                                        })
                                    })
                                    .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(badge(move || {
                                    active_step.with(|s| match s {
                                        Some(st) => format!("uses: {}", st.uses),
                                        None => String::new(),
                                    })
                                }))
                                .child(when(
                                    move || {
                                        active_step.with(|s| {
                                            s.as_ref().is_some_and(|st| {
                                                st.uses == "agent" && st.is_default_prompt
                                            })
                                        })
                                    },
                                    move || status("Default prompt", Tone::Neutral),
                                ))
                                .child(when(
                                    move || {
                                        active_step.with(|s| {
                                            s.as_ref().is_some_and(|st| {
                                                st.uses == "agent" && !st.is_default_prompt
                                            })
                                        })
                                    },
                                    move || status("Customized prompt", Tone::Info),
                                ))
                                .child(when(
                                    move || active_uses.get() != "condition",
                                    move || {
                                        muted(move || {
                                            active_step.with(|s| match s {
                                                Some(st) => format!("Next → {}", st.next_desc),
                                                None => String::new(),
                                            })
                                        })
                                    },
                                )),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(when(
                                    move || {
                                        active_step.with(|s| {
                                            s.as_ref().is_some_and(|st| {
                                                st.uses == "agent" && !st.is_default_prompt
                                            })
                                        })
                                    },
                                    move || {
                                        compact_action(
                                            "Reset to default prompt",
                                            Variant::Secondary,
                                            move |_| {
                                                let Some(step) = active_step.get_untracked() else {
                                                    return;
                                                };
                                                let def = default_step_prompt(
                                                    &step.id,
                                                    Some(&step.profile),
                                                )
                                                .to_owned();
                                                syncing.replace(true);
                                                prompt_sig.set(def.clone());
                                                syncing.replace(false);
                                                let target = step.id;
                                                update_doc(ui, |d| {
                                                    if let Some(s) =
                                                        d.steps.iter_mut().find(|s| s.id == target)
                                                    {
                                                        s.with
                                                            .insert("instruction".into(), json!(def));
                                                    }
                                                });
                                            },
                                        )
                                    },
                                ))
                                .child(compact_action(
                                    "Edit in YAML Source",
                                    Variant::Secondary,
                                    move |_| state.mode.set("yaml".into()),
                                )),
                        ),
                )
                .child(when(
                    move || active_uses.get() == "agent",
                    move || {
                        view()
                            .gap_2p5()
                            .child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(muted("Agent Profile:"))
                                    .child(segmented().child(each(
                                        move || {
                                            let mut list = ui.profiles.get();
                                            if let Some(cur) =
                                                active_step.with(|s| s.as_ref().map(|st| st.profile.clone()))
                                                && !cur.is_empty()
                                                && !list.iter().any(|p| p.id == cur)
                                            {
                                                list.push(canban_types::Profile {
                                                    id: cur,
                                                    name: None,
                                                    enabled: false,
                                                    identity: false,
                                                    preset: None,
                                                    argv: Some(vec!["disabled".into()]),
                                                    resume_argv: None,
                                                    exit: None,
                                                    session_instruction: None,
                                                    role_instruction: String::new(),
                                                });
                                            }
                                            list
                                        },
                                        |p| p.id.clone(),
                                        move |p, _| {
                                            let prof = p.get_untracked();
                                            let pid = prof.id.clone();
                                            let sel_pid = pid.clone();
                                            let click_pid = pid.clone();
                                            let label_str = if !prof.enabled {
                                                format!("@{pid} (disabled)")
                                            } else {
                                                format!("@{pid}")
                                            };
                                            let (mark, _, _) = crate::profiles::effective_provider(
                                                &prof,
                                                ui.current_project().as_ref(),
                                            );
                                            compact_segment(
                                                label_str,
                                                Some(mark),
                                                move || {
                                                    active_step.with(|s| {
                                                        s.as_ref()
                                                            .is_some_and(|st| st.profile == sel_pid)
                                                    })
                                                },
                                                move |_| {
                                                    let Some(step) = active_step.get_untracked()
                                                    else {
                                                        return;
                                                    };
                                                    let chosen = click_pid.clone();
                                                    update_doc(ui, |d| {
                                                        if let Some(s) = d
                                                            .steps
                                                            .iter_mut()
                                                            .find(|s| s.id == step.id)
                                                        {
                                                            s.with.insert(
                                                                "profile".into(),
                                                                json!(chosen),
                                                            );
                                                        }
                                                    });
                                                },
                                            )
                                        },
                                    )))
                                    .child(dynamic(move || {
                                        let Some(step) = active_step.get() else {
                                            return view().into_element();
                                        };
                                        let proj = ui.current_project();
                                        let Some(prof) = ui
                                            .all_profiles
                                            .with(|all| all.iter().find(|p| p.id == step.profile).cloned())
                                        else {
                                            return view().into_element();
                                        };
                                        if !prof.enabled {
                                            return status("Disabled in Settings", Tone::Attention)
                                                .into_element();
                                        }
                                        let (prov, overridden) =
                                            crate::profiles::effective_provider_name(&prof, proj.as_ref());
                                        if overridden {
                                            status(
                                                format!("Runs via {prov} (Project override)"),
                                                Tone::Info,
                                            )
                                            .into_element()
                                        } else if prof.is_identity() {
                                            muted(format!("→ {prov}")).text_size(px(11.5)).into_element()
                                        } else {
                                            view().into_element()
                                        }
                                    })),
                            )
                            .child(
                                view()
                                    .h(px(172.))
                                    .bg(INSET)
                                    .rounded(px(RADIUS))
                                    .p_2()
                                    .child(
                                        prompt_editor
                                            .source(
                                                prompt_sig,
                                                "Step Prompt",
                                                "Write step instruction for the Agent…",
                                            )
                                            .size_full(),
                                    ),
                            )
                    },
                ))
                .child(when(
                    move || active_uses.get() == "deliver",
                    move || {
                        view()
                            .gap_2p5()
                            .child(muted(
                                "Queue Merge integrates the Card worktree into the target branch. Clicking 'Hold auto-merge' on a Card pauses this step.",
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
                                                        move || {
                                                            active_step.with(|s| {
                                                                s.as_ref().is_none_or(|st| {
                                                                    st.sync_upstream
                                                                })
                                                            })
                                                        },
                                                        move |_| {
                                                            let Some(step) =
                                                                active_step.get_untracked()
                                                            else {
                                                                return;
                                                            };
                                                            update_doc(ui, |d| {
                                                                if let Some(s) = d
                                                                    .steps
                                                                    .iter_mut()
                                                                    .find(|s| s.id == step.id)
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
                                                        move || {
                                                            active_step.with(|s| {
                                                                s.as_ref().is_some_and(|st| {
                                                                    !st.sync_upstream
                                                                })
                                                            })
                                                        },
                                                        move |_| {
                                                            let Some(step) =
                                                                active_step.get_untracked()
                                                            else {
                                                                return;
                                                            };
                                                            update_doc(ui, |d| {
                                                                if let Some(s) = d
                                                                    .steps
                                                                    .iter_mut()
                                                                    .find(|s| s.id == step.id)
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
                                                field(branch_sig, "Project default (e.g. main)")
                                                    .w(px(190.)),
                                            ),
                                    ),
                            )
                            .child(rework_selector())
                    },
                ))
                .child(when(
                    move || active_uses.get() == "approval",
                    move || {
                        view()
                            .gap_2p5()
                            .child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(muted("Gate message:"))
                                    .child(
                                        field(
                                            gate_title_sig,
                                            "Human review is required before continuing",
                                        )
                                        .flex_1()
                                        .min_w(px(240.)),
                                    ),
                            )
                            .child(rework_selector())
                    },
                ))
                .child(when(
                    move || active_uses.get() == "card.move",
                    move || {
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(muted("Destination stage:"))
                            .child(segmented().child(each(
                                move || graph.with(|g| g.columns.clone()),
                                |col| col.id.clone(),
                                move |col, _| {
                                    let c = col.get_untracked();
                                    let sel_id = c.id.clone();
                                    let click_id = c.id;
                                    compact_segment(
                                        c.name,
                                        None,
                                        move || {
                                            active_step.with(|s| {
                                                s.as_ref()
                                                    .is_some_and(|st| st.move_column == sel_id)
                                            })
                                        },
                                        move |_| {
                                            let Some(step) = active_step.get_untracked() else {
                                                return;
                                            };
                                            let chosen = click_id.clone();
                                            update_doc(ui, |d| {
                                                if let Some(s) =
                                                    d.steps.iter_mut().find(|s| s.id == step.id)
                                                {
                                                    s.with.insert("column".into(), json!(chosen));
                                                }
                                            });
                                        },
                                    )
                                },
                            )))
                    },
                ))
                .child(when(
                    move || {
                        !matches!(
                            active_uses.get().as_str(),
                            "" | "agent" | "deliver" | "approval" | "card.move"
                        )
                    },
                    move || {
                        muted(move || {
                            active_step.with(|s| match s {
                                Some(st) => st.other_summary.clone(),
                                None => String::new(),
                            })
                        })
                    },
                ))
        },
    )
    .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_stage_toggles_preserve_lane_keys_and_update_only_affected_columns() {
        let mut doc =
            source::default_document("delivery:test", canban_types::WorkspaceSettings::default());
        let fallback = default_columns();
        let initial = build_graph_model(&doc, &fallback);
        assert_eq!(
            initial
                .lanes
                .iter()
                .map(|l| l.id.as_str())
                .collect::<Vec<_>>(),
            vec!["backlog", "develop", "review"]
        );
        assert!(initial.lanes[0].nodes.is_empty());
        assert_eq!(initial.lanes[1].nodes.len(), 2);
        assert_eq!(initial.lanes[2].nodes.len(), 3);

        // Toggling Backlog Auto-Analyze leaves Develop and Review LaneModels identical (`PartialEq`).
        source::set_backlog_options(&mut doc, true, true);
        let after_backlog = build_graph_model(&doc, &fallback);
        assert_eq!(after_backlog.lanes[0].nodes.len(), 2);
        assert_eq!(after_backlog.lanes[1], initial.lanes[1]);
        assert_eq!(after_backlog.lanes[2], initial.lanes[2]);

        // Toggling Review sync_upstream keeps the exact same Review node keys.
        let review_keys_before: Vec<_> = after_backlog.lanes[2]
            .nodes
            .iter()
            .map(|n| n.id.clone())
            .collect();
        source::set_sync_upstream(&mut doc, false);
        let after_sync = build_graph_model(&doc, &fallback);
        let review_keys_after: Vec<_> = after_sync.lanes[2]
            .nodes
            .iter()
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(review_keys_before, review_keys_after);
        assert_eq!(after_sync.lanes[0], after_backlog.lanes[0]);
        assert_eq!(after_sync.lanes[1], after_backlog.lanes[1]);
        assert!(
            after_sync.lanes[2]
                .nodes
                .iter()
                .any(|n| n.kind_badge == "Queue Merge · Local")
        );

        // Switching Review to Manual collapses Review to `approval:review`, leaving Backlog & Develop untouched.
        source::set_review_manual(&mut doc, true);
        let after_manual = build_graph_model(&doc, &fallback);
        assert_eq!(after_manual.lanes[0], after_sync.lanes[0]);
        assert_eq!(after_manual.lanes[1], after_sync.lanes[1]);
        assert_eq!(after_manual.lanes[2].nodes.len(), 1);
    }
}
