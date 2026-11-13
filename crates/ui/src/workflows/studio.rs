//! Stage-oriented visual Workflow Studio.
//!
//! Every Board column is a Stage lane generated from the YAML graph
//! (`canban_workflow_engine::stages`): an ordered pipeline of step cards plus an
//! exit. Each card is configured inline (forms are derived from its `uses`),
//! can be dragged within or across Stages, disabled, or removed. Prompts open
//! in a dialog. Structure edits go through the same draft/publish path as YAML.
use super::update_doc;
use crate::presenter::*;
use canban_types::WorkflowColumn;
use canban_ui_kit::{tokens::*, *};
use canban_workflow_engine::{
    source::{self, Document, NEXT_COLUMN, default_step_prompt, is_default_step_prompt},
    stages::{self, Exit},
};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tint {
    Accent,
    Success,
    Attention,
    Muted,
}
impl Tint {
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
struct StepModel {
    id: String,
    /// Previous card in the lane (drop indicator anchor).
    prev: Option<String>,
    title: String,
    name: String,
    uses: String,
    icon: Icon,
    tint: Tint,
    badge: String,
    summary: String,
    enabled: bool,
    /// "If …" / "Otherwise" when this card starts a condition branch.
    branch: Option<String>,
    pills: Vec<String>,
    profile: String,
    instruction: String,
    default_prompt: bool,
    sync_upstream: bool,
    target_branch: String,
    max_conflict: u32,
    rework: String,
    gate_title: String,
    agent_slot: bool,
    max_ready: Option<u32>,
    message: String,
    move_column: String,
    condition: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LaneModel {
    id: String,
    name: String,
    index: usize,
    last: bool,
    exit: Exit,
    exit_label: String,
    entry: bool,
    nodes: Vec<StepModel>,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct GraphModel {
    has_doc: bool,
    workflow_name: String,
    workflow_id: String,
    workflow_enabled: bool,
    columns: Vec<WorkflowColumn>,
    lanes: Vec<LaneModel>,
    step_count: usize,
    rework_targets: Vec<(String, String)>,
}

fn with_str<'a>(step: &'a source::Step, key: &str) -> Option<&'a str> {
    step.with.get(key).and_then(|v| v.as_str())
}

fn humanize_step_title(step: &source::Step, col_names: &BTreeMap<String, String>) -> String {
    if let Some(name) = step.name.as_deref().filter(|n| !n.trim().is_empty()) {
        return name.to_owned();
    }
    match (step.id.as_str(), step.uses.as_str()) {
        ("agent:plan" | "plan", "agent") => "Plan task".into(),
        ("agent:develop" | "develop", "agent") => "Implement".into(),
        ("agent:review" | "review", "agent") => "Review".into(),
        ("agent:merge" | "merge_verify", "agent") => "Pre-merge check".into(),
        (_, "card.move") => {
            let target = with_str(step, "column").unwrap_or("stage");
            if target == NEXT_COLUMN {
                "Move to next stage".into()
            } else {
                format!(
                    "Move to {}",
                    col_names.get(target).map(String::as_str).unwrap_or(target)
                )
            }
        }
        (_, "approval") => "Human approval".into(),
        (_, "deliver") => "Queue merge".into(),
        ("condition:external_pr", "condition") => "External PR check".into(),
        (_, "condition") => "Condition".into(),
        (_, "admit") => {
            if step.with.get("agent_slot").and_then(|v| v.as_bool()) == Some(false) {
                "Plan-ahead limit".into()
            } else {
                "Wait for free Agent".into()
            }
        }
        (_, "notify") => "Notify".into(),
        _ => step.id.clone(),
    }
}

fn step_model(
    doc: &Document,
    step: &source::Step,
    prev: Option<String>,
    branch: Option<String>,
    titles: &BTreeMap<String, String>,
    _col_names: &BTreeMap<String, String>,
) -> StepModel {
    let title = titles
        .get(&step.id)
        .cloned()
        .unwrap_or_else(|| step.id.clone());
    let profile = with_str(step, "profile").unwrap_or("").to_owned();
    let raw_instruction = with_str(step, "instruction").unwrap_or("");
    let is_default = is_default_step_prompt(&step.id, Some(&profile), raw_instruction);
    let instruction = if is_default {
        default_step_prompt(&step.id, Some(&profile)).to_owned()
    } else {
        raw_instruction.to_owned()
    };
    let sync_upstream = step
        .with
        .get("sync_upstream")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let target_branch = with_str(step, "target_branch").unwrap_or("").to_owned();
    let max_conflict = step
        .with
        .get("max_conflict_attempts")
        .and_then(|v| v.as_u64())
        .map_or(canban_types::DEFAULT_MAX_CONFLICT_ATTEMPTS, |v| v as u32);
    let agent_slot = step
        .with
        .get("agent_slot")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let max_ready = step
        .with
        .get("max_ready")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let move_column = with_str(step, "column").unwrap_or("").to_owned();
    let condition = format!(
        "{} == {}",
        with_str(step, "pointer").unwrap_or("/input"),
        step.with
            .get("equals")
            .map(|v| v
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string()))
            .unwrap_or_default()
    );
    let (icon, tint, badge, summary) = match step.uses.as_str() {
        "agent" => {
            let custom = !is_default;
            (
                Icon::Agent,
                Tint::Accent,
                format!(
                    "@{}{}",
                    if profile.is_empty() {
                        "default"
                    } else {
                        &profile
                    },
                    if custom { " · custom" } else { "" }
                ),
                instruction
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("Run Agent task")
                    .to_owned(),
            )
        }
        "admit" => (
            Icon::History,
            Tint::Muted,
            "Gate".into(),
            match (agent_slot, max_ready) {
                (true, Some(k)) => format!("Free Agent slot · keep ≤{k} ready"),
                (true, None) => "Waits until an automatic Agent slot is free".into(),
                (false, Some(k)) => format!("Keep at most {k} ready Cards in this stage"),
                (false, None) => "Admission gate".into(),
            },
        ),
        "approval" => (
            Icon::Check,
            Tint::Attention,
            "Human gate".into(),
            with_str(step, "title")
                .filter(|s| !s.is_empty())
                .unwrap_or("Pause for human approval")
                .to_owned(),
        ),
        "deliver" => {
            let conflict_agent = if profile.is_empty() {
                "merger"
            } else {
                profile.as_str()
            };
            let conflict_summary = if max_conflict == 0 {
                "conflicts → human".to_owned()
            } else {
                format!("conflicts → @{conflict_agent} ×{max_conflict}")
            };
            (
                Icon::Branch,
                Tint::Success,
                if sync_upstream {
                    "Merge · Sync"
                } else {
                    "Merge · Local"
                }
                .into(),
                format!(
                    "{}{} · {conflict_summary}",
                    if target_branch.is_empty() {
                        "Project branch".to_owned()
                    } else {
                        target_branch.clone()
                    },
                    if sync_upstream {
                        " + upstream"
                    } else {
                        " (local)"
                    }
                ),
            )
        }
        "condition" => (
            Icon::Trigger,
            Tint::Attention,
            "Condition".into(),
            condition.clone(),
        ),
        "notify" => (
            Icon::Info,
            Tint::Muted,
            "Notify".into(),
            with_str(step, "message").unwrap_or("").to_owned(),
        ),
        "card.move" => (Icon::Forward, Tint::Accent, "Move".into(), title.clone()),
        other => (
            Icon::Info,
            Tint::Muted,
            other.to_owned(),
            "Workflow step".into(),
        ),
    };
    let mut pills = Vec::new();
    if let Some(rework) = &step.on_changes_requested {
        pills.push(format!(
            "↺ Rework → {}",
            titles
                .get(rework)
                .cloned()
                .unwrap_or_else(|| rework.clone())
        ));
    }
    if step.uses == "condition" {
        for (key, mark) in [("then", "✓"), ("otherwise", "✕")] {
            if let Some(t) = with_str(step, key) {
                pills.push(format!(
                    "{mark} {} → {}",
                    if key == "then" { "Then" } else { "Else" },
                    titles.get(t).cloned().unwrap_or_else(|| t.to_owned())
                ));
            }
        }
    } else if step.next.as_deref().is_some_and(|n| {
        doc.steps
            .iter()
            .find(|s| s.id == n)
            .is_some_and(|s| s.uses == "complete")
    }) {
        pills.push("■ Ends run".into());
    }
    StepModel {
        id: step.id.clone(),
        prev,
        title,
        name: step.name.clone().unwrap_or_default(),
        uses: step.uses.clone(),
        icon,
        tint,
        badge,
        summary,
        enabled: step.enabled,
        branch,
        pills,
        default_prompt: is_default,
        profile,
        instruction,
        sync_upstream,
        target_branch,
        max_conflict,
        rework: step.on_changes_requested.clone().unwrap_or_default(),
        gate_title: with_str(step, "title").unwrap_or("").to_owned(),
        agent_slot,
        max_ready,
        message: with_str(step, "message").unwrap_or("").to_owned(),
        move_column,
        condition,
    }
}

fn build_model(doc: &Document) -> GraphModel {
    let col_names: BTreeMap<String, String> = doc
        .columns
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    let titles: BTreeMap<String, String> = doc
        .steps
        .iter()
        .map(|s| (s.id.clone(), humanize_step_title(s, &col_names)))
        .collect();
    let mut branches = BTreeMap::new();
    for s in doc.steps.iter().filter(|s| s.uses == "condition") {
        if let Some(t) = with_str(s, "then") {
            branches.insert(t.to_owned(), format!("If {}", titles[&s.id]));
        }
        if let Some(o) = with_str(s, "otherwise") {
            branches.insert(o.to_owned(), "Otherwise".to_owned());
        }
    }
    let views = stages::stages(doc);
    let total = views.len();
    let lanes = views
        .into_iter()
        .enumerate()
        .map(|(index, v)| {
            let nodes = v
                .steps
                .iter()
                .enumerate()
                .filter_map(|(k, id)| {
                    let step = doc.steps.iter().find(|s| &s.id == id)?;
                    Some(step_model(
                        doc,
                        step,
                        k.checked_sub(1).map(|p| v.steps[p].clone()),
                        branches.get(id).cloned(),
                        &titles,
                        &col_names,
                    ))
                })
                .collect();
            let exit_label = match &v.exit {
                Exit::Next => match doc.next_column(&v.column) {
                    Some(n) => format!("Next stage · {}", col_names.get(&n).unwrap_or(&n)),
                    None => "Next stage".into(),
                },
                Exit::Column(c) => format!("→ {}", col_names.get(c).unwrap_or(c)),
                Exit::Stay => "Stay in this column".into(),
            };
            LaneModel {
                id: v.column,
                name: v.name,
                index,
                last: index + 1 == total,
                exit: v.exit,
                exit_label,
                entry: v.entry_rule.is_some(),
                nodes,
            }
        })
        .collect();
    GraphModel {
        has_doc: true,
        workflow_name: doc.name.clone(),
        workflow_id: doc.id.clone(),
        workflow_enabled: doc.enabled,
        columns: doc.columns.clone(),
        lanes,
        step_count: doc.steps.iter().filter(|s| s.uses != "complete").count(),
        rework_targets: doc
            .steps
            .iter()
            .filter(|s| s.uses == "agent")
            .map(|s| (s.id.clone(), titles[&s.id].clone()))
            .collect(),
    }
}

fn default_columns() -> Vec<WorkflowColumn> {
    [
        ("backlog", "Backlog"),
        ("develop", "Develop"),
        ("review", "Review"),
    ]
    .into_iter()
    .map(|(id, name)| WorkflowColumn {
        id: id.into(),
        name: name.into(),
    })
    .collect()
}

fn short_workflow_id(id: &str) -> String {
    if id.len() > 24 {
        format!("{}…", &id[..23])
    } else {
        id.to_owned()
    }
}

/// Step drag session: per-card pan values drive translation; a frame loop
/// hit-tests lanes and card midpoints to compute the drop slot.
#[derive(Clone, Copy)]
struct StepDrag {
    dragging: Signal<String>,
    target: Signal<Option<(String, Option<String>)>>,
    down: Stored<Point<Points>>,
    cards: Stored<BTreeMap<String, (String, ElementRef)>>,
    lanes: Stored<BTreeMap<String, ElementRef>>,
}
impl StepDrag {
    fn new() -> Self {
        Self {
            dragging: signal(String::new()),
            target: signal(None),
            down: stored(Point::default()),
            cards: stored(BTreeMap::new()),
            lanes: stored(BTreeMap::new()),
        }
    }
    fn press(self, event: &PressEvent, x: AnimatedValue, y: AnimatedValue) {
        if self.dragging.get_untracked().is_empty() {
            self.down.replace(event.page_position);
            x.set(0.);
            y.set(0.);
        }
    }
    fn start(self, id: String, x: AnimatedValue, y: AnimatedValue) {
        self.dragging.set(id.clone());
        spawn(async move {
            let lanes = self.lanes.get();
            let cards = self.cards.get();
            let mut lane_rects = Vec::new();
            for (lane, r) in lanes {
                if let Some(m) = r.measure().await {
                    lane_rects.push((lane, m.page.x.0, m.frame.size.width.0));
                }
            }
            let mut mids = Vec::new();
            for (cid, (lane, r)) in cards {
                if cid == id {
                    continue;
                }
                if let Some(m) = r.measure().await {
                    mids.push((lane, cid, m.page.y.0 + m.frame.size.height.0 / 2.));
                }
            }
            let origin = self.down.get();
            while self.dragging.try_get().is_some_and(|d| d == id) {
                next_frame().await;
                let dx = x.value().await;
                let dy = y.value().await;
                let (px, py) = (origin.x.0 + dx, origin.y.0 + dy);
                let target = lane_rects
                    .iter()
                    .find(|(_, lx, w)| px >= *lx && px <= lx + w)
                    .map(|(lane, _, _)| {
                        let after = mids
                            .iter()
                            .filter(|(l, _, mid)| l == lane && *mid < py)
                            .max_by(|a, b| a.2.total_cmp(&b.2))
                            .map(|(_, c, _)| c.clone());
                        (lane.clone(), after)
                    });
                if self.target.try_get().is_some_and(|t| t != target) {
                    self.target.set(target);
                }
            }
        });
    }
    fn end(self, ctx: Ctx, x: AnimatedValue, y: AnimatedValue) {
        let id = self.dragging.get_untracked();
        let target = self.target.get_untracked();
        self.dragging.set(String::new());
        self.target.set(None);
        x.set(0.);
        y.set(0.);
        let Some((lane, after)) = target else { return };
        if id.is_empty() || after.as_deref() == Some(id.as_str()) {
            return;
        }
        ctx.edit(move |d| {
            let after = insertion_anchor(d, &lane, after, &id);
            stages::move_step(d, &id, &lane, after.as_deref())
        });
    }
}

/// Steps cannot be inserted directly after a condition; fall back to the
/// closest earlier pipeline step (or the Stage start).
fn insertion_anchor(
    doc: &Document,
    lane: &str,
    after: Option<String>,
    moving: &str,
) -> Option<String> {
    let view = stages::stages(doc).into_iter().find(|s| s.column == lane)?;
    let list: Vec<_> = view.steps.into_iter().filter(|s| s != moving).collect();
    let mut pos = after.and_then(|a| list.iter().position(|s| *s == a))?;
    loop {
        let id = &list[pos];
        if doc
            .steps
            .iter()
            .find(|s| &s.id == id)
            .is_some_and(|s| s.uses != "condition")
        {
            return Some(id.clone());
        }
        pos = pos.checked_sub(1)?;
    }
}

#[derive(Clone, Copy)]
struct Ctx {
    ui: Ui,
    fallback: Memo<Vec<WorkflowColumn>>,
    graph: Memo<GraphModel>,
    error: Signal<String>,
    drag: StepDrag,
    prompt: Signal<String>,
    renaming: Signal<String>,
    rename_text: Signal<String>,
    adding: Signal<String>,
    confirm_delete: Signal<String>,
}
impl Ctx {
    /// Applies a structural edit atomically to the draft (normalizing first);
    /// failures leave the draft untouched and surface as a notice.
    fn edit(self, f: impl FnOnce(&mut Document) -> anyhow::Result<()>) {
        let fallback = self.fallback.get_untracked();
        let mut failure = None;
        update_doc(self.ui, |d| {
            let mut next = d.clone();
            stages::normalize(&mut next, &fallback);
            match f(&mut next) {
                Ok(()) => *d = next,
                Err(e) => failure = Some(format!("{e:#}")),
            }
        });
        self.error.set(failure.unwrap_or_default());
    }
    fn set_with(self, id: &str, key: &'static str, value: Option<Value>) {
        let id = id.to_owned();
        self.edit(move |d| stages::set_step_with(d, &id, key, value));
    }
}

pub(super) fn render(ui: Ui) -> AnyElement {
    let state = ui.workflows;
    let fallback = computed(move || {
        let project_id = ui.project.get();
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|p| {
                    p.workflow
                        .stages
                        .iter()
                        .map(|st| WorkflowColumn {
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
        let fb = fallback.get();
        state.studio_doc.with(|opt| match opt {
            Some(doc) => {
                let mut d = doc.clone();
                stages::normalize(&mut d, &fb);
                build_model(&d)
            }
            None => GraphModel::default(),
        })
    });
    let ctx = Ctx {
        ui,
        fallback,
        graph,
        error: signal(String::new()),
        drag: StepDrag::new(),
        prompt: signal(String::new()),
        renaming: signal(String::new()),
        rename_text: signal(String::new()),
        adding: signal(String::new()),
        confirm_delete: signal(String::new()),
    };

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
                scroll()
                    .flex_1()
                    .min_h(px(0.))
                    .child(view().gap_3().pb_4().child(stage_canvas(ctx)))
            },
        ))
        .child(when(
            move || !ctx.prompt.get().is_empty(),
            move || prompt_dialog(ctx),
        ))
        .into_element()
}

fn stage_canvas(ctx: Ctx) -> AnyElement {
    let state = ctx.ui.workflows;
    let graph = ctx.graph;
    let add_stage = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(field(state.new_column_name, "New stage (e.g. QA)…").w(px(170.)))
        .child(compact_action(
            "+ Add stage",
            Variant::Secondary,
            move |_| {
                let name = state.new_column_name.get_untracked();
                if name.trim().is_empty() {
                    return;
                }
                state.new_column_name.set(String::new());
                ctx.edit(move |d| {
                    // New stages go before the last column (usually Review).
                    let before =
                        (d.columns.len() > 1).then(|| d.columns.last().unwrap().id.clone());
                    stages::add_column(d, &name, before.as_deref()).map(|_| ())
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
            move |lane, _| stage_lane(ctx, lane),
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
                                .child(icon_tinted(Icon::Trigger, ACCENT).w(px(15.)).h(px(15.)))
                                .child(label("Stages").font_weight(FontWeight::SEMIBOLD))
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
                                        format!("{} stages · {} steps", g.lanes.len(), g.step_count)
                                    })
                                })),
                        )
                        .child(muted(
                            "Click a step to configure it · drag ⋮⋮ to reorder or move it to another stage · each stage ends with its exit.",
                        )),
                )
                .child(add_stage),
        )
        .child(when(
            move || !ctx.error.get().is_empty(),
            move || notice(move || ctx.error.get(), Tone::Attention),
        ))
        .child(scroll().horizontal().w_full().child(lanes_row))
        .into_element()
}

fn drop_line(ctx: Ctx, lane: String, after: Option<String>) -> impl IntoElement {
    when(
        move || {
            !ctx.drag.dragging.get().is_empty()
                && ctx.drag.target.get() == Some((lane.clone(), after.clone()))
        },
        move || view().h(px(3.)).w_full().rounded(px(2.)).bg(ACCENT),
    )
}

fn stage_lane(ctx: Ctx, lane: ReadSignal<LaneModel>) -> impl IntoElement {
    let lane_id = lane.with_untracked(|l| l.id.clone());
    let lane_ref = element_ref();
    ctx.drag.lanes.with(|m| m.insert(lane_id.clone(), lane_ref));

    let (id_rename, id_confirm, id_left, id_right, id_del, id_add) = (
        lane_id.clone(),
        lane_id.clone(),
        lane_id.clone(),
        lane_id.clone(),
        lane_id.clone(),
        lane_id.clone(),
    );
    let renaming = {
        let id = lane_id.clone();
        computed(move || ctx.renaming.get() == id)
    };
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
                .flex_1()
                .child(
                    view()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .bg(SURFACE)
                        .border_1()
                        .border_color(BORDER)
                        .child(
                            muted(move || format!("{}", lane.with(|l| l.index) + 1))
                                .text_size(px(11.)),
                        ),
                )
                .child(when(
                    move || !renaming.get(),
                    move || {
                        let id = id_rename.clone();
                        press_target()
                            .min_w(px(0.))
                            .accessibility_label(|| Some("Rename stage".into()))
                            .on_press(move |_: &()| {
                                ctx.rename_text.set(lane.with_untracked(|l| l.name.clone()));
                                ctx.renaming.set(id.clone());
                            })
                            .child(
                                label(move || lane.with(|l| l.name.clone()))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate(),
                            )
                    },
                ))
                .child(when(
                    move || renaming.get(),
                    move || {
                        let id = id_confirm.clone();
                        let commit = move || {
                            let name = ctx.rename_text.get_untracked();
                            let id = id.clone();
                            ctx.renaming.set(String::new());
                            ctx.edit(move |d| stages::rename_column(d, &id, &name));
                        };
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(field(ctx.rename_text, "Stage name").w(px(130.)).on_submit({
                                let commit = commit.clone();
                                move |_: &str| commit()
                            }))
                            .child(compact_action("Save", Variant::Primary, move |_| commit()))
                    },
                )),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_0p5()
                .child(when(
                    move || lane.with(|l| l.index > 0),
                    move || {
                        let id = id_left.clone();
                        compact_action("←", Variant::Quiet, move |_| {
                            let id = id.clone();
                            let to = lane.with_untracked(|l| l.index.saturating_sub(1));
                            ctx.edit(move |d| stages::move_column(d, &id, to));
                        })
                    },
                ))
                .child(when(
                    move || lane.with(|l| !l.last),
                    move || {
                        let id = id_right.clone();
                        compact_action("→", Variant::Quiet, move |_| {
                            let id = id.clone();
                            let to = lane.with_untracked(|l| l.index + 1);
                            ctx.edit(move |d| stages::move_column(d, &id, to));
                        })
                    },
                ))
                .child(when(
                    move || ctx.graph.with(|g| g.lanes.len() > 1),
                    move || {
                        let id = id_del.clone();
                        let confirm_id = id.clone();
                        compact_action(
                            move || {
                                if ctx.confirm_delete.get() == confirm_id {
                                    "Delete stage?"
                                } else {
                                    "×"
                                }
                            },
                            Variant::Quiet,
                            move |_| {
                                if ctx.confirm_delete.get_untracked() != id {
                                    ctx.confirm_delete.set(id.clone());
                                    return;
                                }
                                ctx.confirm_delete.set(String::new());
                                let id = id.clone();
                                ctx.edit(move |d| stages::remove_column(d, &id));
                            },
                        )
                    },
                )),
        );

    let lane_for_nodes = lane_id.clone();
    let lane_for_end = lane_id.clone();
    let body = view()
        .gap_1p5()
        .flex_1()
        .child(when(
            move || lane.with(|l| l.nodes.is_empty()),
            move || {
                view()
                    .py_3()
                    .px_3()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(BORDER)
                    .bg(BACKGROUND)
                    .opacity(0.68)
                    .items_center()
                    .justify_center()
                    .child(muted(move || {
                        if lane.with(|l| l.entry) {
                            "No steps yet"
                        } else {
                            "No automation · Cards wait here"
                        }
                    }))
            },
        ))
        .child(each(
            move || lane.with(|l| l.nodes.clone()),
            |n| n.id.clone(),
            move |node, _| step_card(ctx, node, lane_for_nodes.clone()),
        ))
        .child(dynamic(move || {
            let last = lane.with(|l| l.nodes.last().map(|n| n.id.clone()));
            drop_line(ctx, lane_for_end.clone(), last)
        }));

    let adding = {
        let id = lane_id.clone();
        computed(move || ctx.adding.get() == id)
    };
    let add_step = view()
        .gap_1p5()
        .child(compact_action(
            move || if adding.get() { "Cancel" } else { "+ Add step" },
            Variant::Quiet,
            move |_| {
                if adding.get_untracked() {
                    ctx.adding.set(String::new());
                } else {
                    ctx.adding.set(id_add.clone());
                }
            },
        ))
        .child(when(
            move || adding.get(),
            move || {
                let mut grid = view().flex_row().flex_wrap().gap_1();
                for (kind, title) in stages::TEMPLATES {
                    let kind = *kind;
                    grid = grid.child(compact_action(*title, Variant::Secondary, move |_| {
                        let lane_id = lane.with_untracked(|l| l.id.clone());
                        let after = lane.with_untracked(|l| l.nodes.last().map(|n| n.id.clone()));
                        ctx.adding.set(String::new());
                        ctx.edit(move |d| {
                            let step = stages::template(kind)
                                .ok_or_else(|| anyhow::anyhow!("Unknown step type {kind}"))?;
                            let after = insertion_anchor(d, &lane_id, after, "");
                            let id = stages::insert_step(d, &lane_id, after.as_deref(), step)?;
                            ctx.ui.workflows.selected_step.set(id);
                            Ok(())
                        });
                    }));
                }
                grid
            },
        ));

    let exit_row = exit_controls(ctx, lane);

    let lane_card = view()
        .element_ref(&lane_ref)
        .w(px(272.))
        .flex_1()
        .min_w(px(260.))
        .p_3()
        .gap_2p5()
        .rounded(px(RADIUS))
        .bg(INSET)
        .border_1()
        .border_color(move || {
            let id = lane.with(|l| l.id.clone());
            if ctx
                .drag
                .target
                .with(|t| t.as_ref().is_some_and(|(l, _)| *l == id))
            {
                ACCENT
            } else {
                BORDER
            }
        })
        .child(header)
        .child(body)
        .child(add_step)
        .child(exit_row);

    view()
        .flex_row()
        .items_stretch()
        .gap_2p5()
        .flex_1()
        .min_w(px(268.))
        .child(when(
            move || lane.with(|l| l.index > 0),
            move || {
                view()
                    .justify_center()
                    .items_center()
                    .child(icon(Icon::Forward).w(px(14.)).h(px(14.)).opacity(0.55))
            },
        ))
        .child(lane_card)
}

fn exit_controls(ctx: Ctx, lane: ReadSignal<LaneModel>) -> impl IntoElement {
    let pick = move |exit: Exit| {
        let id = lane.with_untracked(|l| l.id.clone());
        ctx.edit(move |d| stages::set_exit(d, &id, exit));
    };
    view()
        .gap_1()
        .pt_1p5()
        .border_t_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(icon(Icon::Forward).w(px(12.)).h(px(12.)).opacity(0.7))
                .child(muted("Then").text_size(px(12.)))
                .child(
                    label(move || lane.with(|l| l.exit_label.clone()))
                        .text_size(px(12.))
                        .truncate(),
                ),
        )
        .child(
            segmented()
                .child(when(
                    move || lane.with(|l| !l.last),
                    move || {
                        compact_segment(
                            "Next stage",
                            None,
                            move || lane.with(|l| l.exit == Exit::Next),
                            move |_| pick(Exit::Next),
                        )
                    },
                ))
                .child(when(
                    move || lane.with(|l| matches!(l.exit, Exit::Column(_))),
                    move || {
                        compact_segment(
                            lane.with_untracked(|l| l.exit_label.clone()),
                            None,
                            || true,
                            |_| {},
                        )
                    },
                ))
                .child(compact_segment(
                    "Stay",
                    None,
                    move || lane.with(|l| l.exit == Exit::Stay),
                    move |_| pick(Exit::Stay),
                )),
        )
}

fn step_card(ctx: Ctx, node: ReadSignal<StepModel>, lane_id: String) -> impl IntoElement {
    let state = ctx.ui.workflows;
    let id = node.with_untracked(|n| n.id.clone());
    let reference = element_ref();
    ctx.drag
        .cards
        .with(|m| m.insert(id.clone(), (lane_id.clone(), reference)));
    let x = animated_value(0.);
    let y = animated_value(0.);
    let expanded = {
        let id = id.clone();
        computed(move || state.selected_step.get() == id)
    };
    let lifted = {
        let id = id.clone();
        computed(move || ctx.drag.dragging.get() == id)
    };
    let (id_press, id_drag, id_toggle) = (id.clone(), id.clone(), id.clone());
    let prev = node.with_untracked(|n| n.prev.clone());

    let handle = view()
        .px_0p5()
        .py_1()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(INSET))
        .label("Drag to reorder or move to another stage")
        .child(icon(Icon::Grip).w(px(12.)).h(px(12.)).opacity(0.7))
        .pan_values(Some(&x), Some(&y))
        .on_press_down(move |e: &PressEvent| ctx.drag.press(e, x, y))
        .on_pan_start(move |_: &PanEvent| ctx.drag.start(id_drag.clone(), x, y))
        .on_pan_end(move |_: &PanEvent| ctx.drag.end(ctx, x, y));

    let title_button = press_target()
        .flex_1()
        .min_w(px(0.))
        .accessibility_label(move || Some(node.with(|n| format!("Configure {}", n.title)).into()))
        .on_press(move |_: &()| {
            if expanded.get_untracked() {
                state.selected_step.set(String::new());
            } else {
                state.selected_step.set(id_press.clone());
            }
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .min_w(px(0.))
                .child(
                    icon_tinted(node.with_untracked(|n| n.icon), move || {
                        node.with(|n| n.tint.color())
                    })
                    .w(px(14.))
                    .h(px(14.)),
                )
                .child(
                    label(move || node.with(|n| n.title.clone()))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .min_w(px(0.))
                        .flex_1(),
                )
                .child(badge(move || node.with(|n| n.badge.clone()))),
        );

    let toggle = compact_action(
        move || {
            if node.with(|n| n.enabled) {
                "On"
            } else {
                "Off"
            }
        },
        Variant::Quiet,
        move |_| {
            let id = id_toggle.clone();
            let enabled = !node.with_untracked(|n| n.enabled);
            ctx.edit(move |d| stages::set_step_enabled(d, &id, enabled));
        },
    );

    let card = view()
        .element_ref(&reference)
        .w_full()
        .p_2p5()
        .gap_1p5()
        .rounded(px(RADIUS))
        .bg(move || if expanded.get() { SELECTED } else { SURFACE })
        .border_1()
        .border_color(move || {
            if expanded.get() || lifted.get() {
                ACCENT
            } else {
                BORDER
            }
        })
        .opacity(move || {
            if lifted.get() {
                0.85_f32
            } else if node.with(|n| n.enabled) {
                1.0
            } else {
                0.55
            }
        })
        .bind_translate_x(&x)
        .bind_translate_y(&y)
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .gap_1()
                .child(handle)
                .child(title_button)
                .child(toggle),
        )
        .child(
            muted(move || {
                node.with(|n| {
                    if n.enabled {
                        n.summary.clone()
                    } else {
                        format!("Disabled · skipped · {}", n.summary)
                    }
                })
            })
            .text_size(px(12.))
            .truncate()
            .w_full()
            .min_w(px(0.)),
        )
        .child(when(
            move || node.with(|n| !n.pills.is_empty()),
            move || {
                view().flex_row().flex_wrap().gap_1().child(each(
                    move || node.with(|n| n.pills.clone()),
                    |p| p.clone(),
                    move |pill, _| {
                        view()
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .bg(INSET)
                            .border_1()
                            .border_color(BORDER)
                            .child(
                                label(move || pill.get())
                                    .text_size(px(11.))
                                    .text_color(MUTED),
                            )
                    },
                ))
            },
        ))
        .child(when(
            move || expanded.get(),
            move || step_settings(ctx, node),
        ));

    view()
        .w_full()
        .gap_1()
        .child(drop_line(ctx, lane_id, prev))
        .child(when(
            move || node.with(|n| n.branch.is_some()),
            move || {
                muted(move || node.with(|n| format!("{} ↓", n.branch.clone().unwrap_or_default())))
                    .text_size(px(11.))
            },
        ))
        .child(card)
}

fn setting_row(title: &'static str, control: impl IntoElement) -> View {
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1p5()
        .child(muted(title).text_size(px(12.)))
        .child(control)
}

/// Text input bound to one step value; commits on every edit after mount.
fn text_setting(
    ctx: Ctx,
    id: String,
    initial: String,
    placeholder: &'static str,
    apply: impl Fn(Ctx, &str, String) + Copy + 'static,
) -> impl IntoElement {
    let value = signal(initial);
    let mounted = stored(false);
    effect(move || {
        let v = value.get();
        if !mounted.replace(true) {
            return;
        }
        apply(ctx, &id, v);
    });
    field(value, placeholder).flex_1().min_w(px(140.))
}

fn choice<T: Clone + PartialEq + 'static>(
    options: Vec<(String, T)>,
    current: impl Fn() -> T + Copy + 'static,
    pick: impl Fn(T) + Clone + 'static,
) -> impl IntoElement {
    let mut seg = segmented();
    for (title, value) in options {
        let (sel, chosen) = (value.clone(), value);
        let pick = pick.clone();
        seg = seg.child(compact_segment(
            title,
            None,
            move || current() == sel,
            move |_| pick(chosen.clone()),
        ));
    }
    seg
}

fn step_settings(ctx: Ctx, node: ReadSignal<StepModel>) -> AnyElement {
    let ui = ctx.ui;
    let model = node.get_untracked();
    let id = model.id.clone();
    let mut form = view().gap_2().pt_1p5().border_t_1().border_color(BORDER);

    form = form.child(setting_row(
        "Name",
        text_setting(
            ctx,
            id.clone(),
            model.name.clone(),
            "Display name",
            |ctx, id, v| {
                let id = id.to_owned();
                ctx.edit(move |d| stages::set_step_name(d, &id, Some(v)));
            },
        ),
    ));

    let rework = {
        let id = id.clone();
        move || {
            let id = id.clone();
            dynamic(move || {
                let targets = ctx.graph.with(|g| g.rework_targets.clone());
                let mut options = vec![("None".to_owned(), String::new())];
                options.extend(targets.into_iter().map(|(tid, title)| (title, tid)));
                let id = id.clone();
                setting_row(
                    "Rework →",
                    choice(
                        options,
                        move || node.with(|n| n.rework.clone()),
                        move |t: String| {
                            let id = id.clone();
                            ctx.edit(move |d| {
                                stages::set_rework(d, &id, Some(t).filter(|t| !t.is_empty()))
                            });
                        },
                    ),
                )
            })
        }
    };

    match model.uses.as_str() {
        "agent" => {
            let pid = id.clone();
            form = form
                .child(dynamic(move || {
                    let profiles = ui.profiles.get();
                    let mut seg = view().flex_row().flex_wrap().gap_1();
                    for prof in profiles {
                        let (sel, chosen) = (prof.id.clone(), prof.id.clone());
                        let (mark, _, _) = crate::profiles::effective_provider(
                            &prof,
                            ui.current_project().as_ref(),
                        );
                        let pid = pid.clone();
                        seg = seg.child(compact_segment(
                            format!("@{}", prof.id),
                            Some(mark),
                            move || node.with(|n| n.profile == sel),
                            move |_| ctx.set_with(&pid, "profile", Some(json!(chosen.clone()))),
                        ));
                    }
                    view()
                        .gap_1()
                        .child(muted("Agent").text_size(px(12.)))
                        .child(seg)
                }))
                .child(
                    view().p_2().rounded(px(RADIUS)).bg(INSET).child(
                        muted(move || {
                            node.with(|n| {
                                n.instruction.lines().take(4).collect::<Vec<_>>().join("\n")
                            })
                        })
                        .text_size(px(12.)),
                    ),
                )
                .child({
                    let (open_id, reset_id) = (id.clone(), id.clone());
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .child(compact_action(
                            "Edit prompt…",
                            Variant::Primary,
                            move |_| ctx.prompt.set(open_id.clone()),
                        ))
                        .child(when(
                            move || node.with(|n| !n.default_prompt),
                            move || {
                                let reset_id = reset_id.clone();
                                compact_action("Reset to default", Variant::Secondary, move |_| {
                                    ctx.set_with(&reset_id, "instruction", None);
                                })
                            },
                        ))
                        .child(when(
                            move || node.with(|n| n.default_prompt),
                            move || status("Default prompt", Tone::Neutral),
                        ))
                });
        }
        "admit" => {
            let (slot_id, ready_id) = (id.clone(), id.clone());
            form = form
                .child(setting_row(
                    "Wait for free Agent",
                    choice(
                        vec![("On".into(), true), ("Off".into(), false)],
                        move || node.with(|n| n.agent_slot),
                        move |on| {
                            let id = slot_id.clone();
                            let ready = node.with_untracked(|n| n.max_ready);
                            ctx.edit(move |d| {
                                stages::set_step_with(
                                    d,
                                    &id,
                                    "agent_slot",
                                    (!on).then_some(json!(false)),
                                )?;
                                // A gate needs at least one condition.
                                if !on && ready.is_none() {
                                    stages::set_step_with(
                                        d,
                                        &id,
                                        "max_ready",
                                        Some(json!(source::DEFAULT_BACKLOG_MAX_READY)),
                                    )?;
                                }
                                Ok(())
                            });
                        },
                    ),
                ))
                .child(dynamic(move || {
                    let current = node.with(|n| n.max_ready);
                    let mut values = vec![1u32, 2, 3, 5];
                    if let Some(k) = current
                        && !values.contains(&k)
                    {
                        values.push(k);
                        values.sort_unstable();
                    }
                    let mut options = vec![("Off".to_owned(), None)];
                    options.extend(values.into_iter().map(|k| (k.to_string(), Some(k))));
                    let id = ready_id.clone();
                    setting_row(
                        "Keep ≤K ready in stage",
                        choice(
                            options,
                            move || node.with(|n| n.max_ready),
                            move |k: Option<u32>| {
                                let id = id.clone();
                                let slot = node.with_untracked(|n| n.agent_slot);
                                ctx.edit(move |d| {
                                    stages::set_step_with(
                                        d,
                                        &id,
                                        "max_ready",
                                        k.map(|k| json!(k)),
                                    )?;
                                    if k.is_none() && !slot {
                                        stages::set_step_with(d, &id, "agent_slot", None)?;
                                    }
                                    Ok(())
                                });
                            },
                        ),
                    )
                }));
        }
        "deliver" => {
            let (sync_id, conflict_id, pid) = (id.clone(), id.clone(), id.clone());
            form = form
                .child(setting_row(
                    "Upstream",
                    choice(
                        vec![("Merge & sync".into(), true), ("Local only".into(), false)],
                        move || node.with(|n| n.sync_upstream),
                        move |on| ctx.set_with(&sync_id, "sync_upstream", Some(json!(on))),
                    ),
                ))
                .child(setting_row(
                    "Target branch",
                    text_setting(
                        ctx,
                        id.clone(),
                        model.target_branch.clone(),
                        "Project default (e.g. main)",
                        |ctx, id, v| {
                            let v = v.trim().to_owned();
                            ctx.set_with(id, "target_branch", (!v.is_empty()).then(|| json!(v)));
                        },
                    ),
                ))
                .child(setting_row(
                    "Conflict retries",
                    choice(
                        vec![
                            ("Off".into(), 0u32),
                            ("×1".into(), 1),
                            ("×3".into(), 3),
                            ("×5".into(), 5),
                        ],
                        move || node.with(|n| n.max_conflict),
                        move |k| {
                            ctx.set_with(
                                &conflict_id,
                                "max_conflict_attempts",
                                (k != canban_types::DEFAULT_MAX_CONFLICT_ATTEMPTS)
                                    .then(|| json!(k)),
                            )
                        },
                    ),
                ))
                .child(dynamic(move || {
                    if node.with(|n| n.max_conflict == 0) {
                        return view().into_element();
                    }
                    let profiles = ui.profiles.get();
                    let mut seg = view().flex_row().flex_wrap().gap_1();
                    for prof in profiles {
                        let (sel, chosen) = (prof.id.clone(), prof.id.clone());
                        let (mark, _, _) = crate::profiles::effective_provider(
                            &prof,
                            ui.current_project().as_ref(),
                        );
                        let pid = pid.clone();
                        seg = seg.child(compact_segment(
                            format!("@{}", prof.id),
                            Some(mark),
                            move || {
                                node.with(|n| {
                                    let active = if n.profile.is_empty() {
                                        "merger"
                                    } else {
                                        n.profile.as_str()
                                    };
                                    active == sel
                                })
                            },
                            move |_| {
                                ctx.set_with(
                                    &pid,
                                    "profile",
                                    (chosen != "merger").then(|| json!(chosen.clone())),
                                )
                            },
                        ));
                    }
                    view()
                        .gap_1()
                        .child(muted("Conflict Agent").text_size(px(12.)))
                        .child(seg)
                        .into_element()
                }))
                .child(rework());
        }
        "approval" => {
            form = form
                .child(setting_row(
                    "Message",
                    text_setting(
                        ctx,
                        id.clone(),
                        model.gate_title.clone(),
                        "Human approval is required",
                        |ctx, id, v| ctx.set_with(id, "title", Some(json!(v))),
                    ),
                ))
                .child(rework());
        }
        "notify" => {
            form = form.child(setting_row(
                "Message",
                text_setting(
                    ctx,
                    id.clone(),
                    model.message.clone(),
                    "Notification text",
                    |ctx, id, v| ctx.set_with(id, "message", Some(json!(v))),
                ),
            ));
        }
        "card.move" => {
            let mid = id.clone();
            form = form.child(dynamic(move || {
                let mut options = vec![("Next stage".to_owned(), NEXT_COLUMN.to_owned())];
                options.extend(
                    ctx.graph
                        .with(|g| g.columns.clone())
                        .into_iter()
                        .map(|c| (c.name, c.id)),
                );
                let mid = mid.clone();
                setting_row(
                    "Move to",
                    choice(
                        options,
                        move || node.with(|n| n.move_column.clone()),
                        move |c: String| ctx.set_with(&mid, "column", Some(json!(c))),
                    ),
                )
            }));
        }
        "condition" => {
            form = form.child(muted(move || {
                node.with(|n| format!("When {} · branches are edited in YAML Source", n.condition))
            }));
        }
        _ => {}
    }

    let del_id = id.clone();
    form.child(
        view()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .child(compact_action("Delete step", Variant::Danger, move |_| {
                let id = del_id.clone();
                ui.workflows.selected_step.set(String::new());
                ctx.edit(move |d| stages::remove_step(d, &id).map(|_| ()));
            }))
            .child(compact_action("Edit in YAML", Variant::Quiet, move |_| {
                ui.workflows.mode.set("yaml".into())
            })),
    )
    .into_element()
}

fn prompt_dialog(ctx: Ctx) -> impl IntoElement {
    let id = ctx.prompt.get_untracked();
    let model = ctx.graph.with_untracked(|g| {
        g.lanes
            .iter()
            .flat_map(|l| l.nodes.iter())
            .find(|n| n.id == id)
            .cloned()
    });
    let Some(model) = model else {
        ctx.prompt.set(String::new());
        return view().into_element();
    };
    let text = signal(model.instruction.clone());
    let editor = canban_ui_kit::markdown::MarkdownInput::new();
    let close = move || ctx.prompt.set(String::new());
    let (save_id, reset_profile) = (id.clone(), model.profile.clone());
    let apply_profile = reset_profile.clone();
    let reset_id = id.clone();
    super::workflow_dialog(
        view()
            .flex_1()
            .min_h(px(0.))
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(icon(Icon::Agent))
                    .child(heading(format!("Prompt · {}", model.title)))
                    .child(badge(format!(
                        "@{}",
                        if model.profile.is_empty() {
                            "default"
                        } else {
                            &model.profile
                        }
                    )))
                    .child(view().flex_1())
                    .child(chrome_button(
                        Icon::Close,
                        "Close prompt editor",
                        move |_| close(),
                    )),
            )
            .child(
                view()
                    .flex_1()
                    .min_h(px(240.))
                    .bg(INSET)
                    .rounded(px(RADIUS))
                    .p_2()
                    .child(
                        editor
                            .source(
                                text,
                                "Step Prompt",
                                "Write the step instruction for the Agent…",
                            )
                            .size_full(),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(action("Reset to default", Variant::Secondary, move |_| {
                        text.set(default_step_prompt(&reset_id, Some(&reset_profile)).to_owned())
                    }))
                    .child(view().flex_1())
                    .child(action("Cancel", Variant::Quiet, move |_| close()))
                    .child(action("Apply", Variant::Primary, move |_| {
                        let value = text.get_untracked();
                        let next_val =
                            (!is_default_step_prompt(&save_id, Some(&apply_profile), &value))
                                .then(|| json!(value));
                        ctx.set_with(&save_id, "instruction", next_val);
                        close();
                    })),
            ),
        close,
    )
    .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(edit: impl FnOnce(&mut Document)) -> GraphModel {
        let mut doc =
            source::default_document("delivery:test", canban_types::WorkspaceSettings::default());
        stages::normalize(&mut doc, &[]);
        edit(&mut doc);
        build_model(&doc)
    }

    #[test]
    fn lanes_are_generated_from_the_workflow_graph() {
        let g = model(|_| {});
        assert_eq!(
            g.lanes.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
            ["backlog", "develop", "review"]
        );
        assert!(g.lanes[0].nodes.is_empty());
        assert_eq!(g.lanes[0].exit, Exit::Stay);
        assert_eq!(g.lanes[1].nodes.len(), 1, "the exit move is not a card");
        assert_eq!(g.lanes[1].exit, Exit::Next);
        assert_eq!(g.lanes[1].exit_label, "Next stage · Review");
        let review: Vec<_> = g.lanes[2].nodes.iter().map(|n| n.uses.as_str()).collect();
        assert_eq!(review, ["condition", "approval", "deliver"]);
        assert_eq!(
            g.lanes[2].nodes[1].branch.as_deref(),
            Some("If External PR check")
        );
        assert_eq!(g.lanes[2].nodes[2].branch.as_deref(), Some("Otherwise"));
        assert!(g.lanes[2].last);
    }

    #[test]
    fn custom_columns_and_disabled_steps_render_without_special_cases() {
        let g = model(|d| {
            stages::add_column(d, "QA", Some("review")).unwrap();
            let id =
                stages::insert_step(d, "qa", None, stages::template("review").unwrap()).unwrap();
            stages::set_exit(d, "qa", Exit::Next).unwrap();
            stages::set_step_enabled(d, &id, false).unwrap();
        });
        let qa = g.lanes.iter().find(|l| l.id == "qa").unwrap();
        assert_eq!(qa.nodes.len(), 1);
        assert!(!qa.nodes[0].enabled);
        assert_eq!(qa.exit_label, "Next stage · Review");
        // Develop's relative exit now points at QA.
        assert_eq!(g.lanes[1].exit_label, "Next stage · QA");
    }

    #[test]
    fn insertion_anchor_skips_conditions() {
        let mut doc =
            source::default_document("delivery:test", canban_types::WorkspaceSettings::default());
        stages::normalize(&mut doc, &[]);
        assert_eq!(
            insertion_anchor(&doc, "review", Some("condition:external_pr".into()), ""),
            None
        );
        assert_eq!(
            insertion_anchor(&doc, "review", Some("deliver:review".into()), ""),
            Some("deliver:review".into())
        );
    }
}
