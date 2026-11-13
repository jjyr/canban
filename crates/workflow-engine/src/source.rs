//! YAML authoring compiles into the single interpreter. Pure descriptions are
//! shared by native UI and CLI; no server-generated hover prose.
use anyhow::{Context, Result, bail, ensure};
use canban_types::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<WorkflowColumn>,
    pub on: BTreeMap<String, WorkflowRule>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub workspace: WorkspaceSettings,
    #[serde(default = "limit")]
    pub max_steps: u32,
}
fn yes() -> bool {
    true
}
fn limit() -> u32 {
    100
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub uses: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub with: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_changes_requested: Option<String>,
    /// Board column (Stage) this step belongs to in Studio. Inferred when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// `false` skips the step at compile time; references continue to its successor.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
}
fn is_yes(v: &bool) -> bool {
    *v
}
pub fn parse(source: &str) -> Result<Document> {
    ensure!(
        source.len() <= 2 * 1024 * 1024,
        "Workflow source exceeds 2 MiB"
    );
    let mut doc: Document = serde_yaml_ng::from_str(source)?;
    crate::migrations::upgrade(&mut doc)?;
    ensure!(!doc.on.is_empty(), "Workflow needs at least one on rule");
    Ok(doc)
}
/// Relative `card.move` column: the column after the step's Stage.
pub const NEXT_COLUMN: &str = "$next";
/// `card.move` continuation: the target column's entry rule start (or end of run).
pub const STAGE_ENTRY: &str = "$entry";
const SKIPPED_END: &str = "__skipped_end";

impl Document {
    /// Successor by list position when `next` is omitted (`None` = end of run).
    pub(crate) fn raw_next(&self, idx: usize) -> Option<String> {
        let step = &self.steps[idx];
        if matches!(step.uses.as_str(), "condition" | "complete") {
            return None;
        }
        step.next
            .clone()
            .or_else(|| self.steps.get(idx + 1).map(|s| s.id.clone()))
    }
    /// Follows disabled steps to the first enabled one (`None` = end of run).
    pub(crate) fn resolve_enabled(&self, id: &str) -> Option<String> {
        let mut cur = id.to_owned();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(cur.clone()) {
                return None;
            }
            // Unknown targets stay as-is so validation reports them.
            let Some(idx) = self.steps.iter().position(|s| s.id == cur) else {
                return Some(cur);
            };
            let step = &self.steps[idx];
            if step.enabled {
                return Some(cur);
            }
            let to = if step.uses == "condition" {
                step.with
                    .get("otherwise")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            } else {
                self.raw_next(idx)
            };
            cur = to?;
        }
    }
    fn column_ids(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.id.clone()).collect()
    }
    /// The enabled `card.column_entered` rule start for a column, if any.
    pub fn column_entry(&self, column: &str) -> Option<(String, String)> {
        self.on
            .iter()
            .find(|(k, r)| {
                r.enabled
                    && r.event == "card.column_entered"
                    && (r.selector.column.as_deref() == Some(column)
                        || (r.selector.column.is_none() && *k == column))
            })
            .map(|(k, r)| (k.clone(), r.start_at.clone()))
    }
    /// Column after `column` in declared order.
    pub fn next_column(&self, column: &str) -> Option<String> {
        let ids = self.column_ids();
        let pos = ids.iter().position(|c| c == column)?;
        ids.get(pos + 1).cloned()
    }
    /// Concrete target of a `card.move` step given its Stage.
    pub fn move_target(&self, step: &Step, stage: Option<&str>) -> Option<String> {
        let column = step.with.get("column").and_then(|v| v.as_str())?;
        if column == NEXT_COLUMN {
            stage.and_then(|s| self.next_column(s))
        } else {
            Some(column.to_owned())
        }
    }

    pub fn compile(&self) -> Result<WorkflowDefinition> {
        let stages = step_stages(self);
        let mut steps = BTreeMap::new();
        let mut need_end = false;
        let mut target = |id: Option<String>, required: bool| -> Option<String> {
            let resolved = id.and_then(|id| self.resolve_enabled(&id));
            if resolved.is_none() && required {
                need_end = true;
                return Some(SKIPPED_END.into());
            }
            resolved
        };
        let mut seen = BTreeSet::new();
        for (i, step) in self.steps.iter().enumerate() {
            ensure!(seen.insert(&step.id), "Duplicate step ID {}", step.id);
            if !step.enabled {
                continue;
            }
            let mut args: serde_json::Map<String, Value> = step.with.clone().into_iter().collect();
            let kind = match step.uses.as_str() {
                "agent" => "agent",
                "card.move" => "move",
                "approval" => "approval",
                "deliver" => "deliver",
                "admit" => "admit",
                "condition" => "condition",
                "notify" => "notify",
                "complete" => "complete",
                other => bail!("Unknown uses: {other}"),
            };
            args.insert("type".into(), json!(kind));
            if kind == "agent"
                && args
                    .get("instruction")
                    .and_then(|v| v.as_str())
                    .is_none_or(|s| s.trim().is_empty())
            {
                let prof = args.get("profile").and_then(|v| v.as_str());
                args.insert(
                    "instruction".into(),
                    json!(default_step_prompt(&step.id, prof)),
                );
            }
            let stage = stages.get(&step.id).map(String::as_str);
            let mut raw = self.raw_next(i);
            if kind == "move" {
                let column = self.move_target(step, stage).with_context(|| {
                    format!(
                        "Step {} moves to the next column, but its Stage has no next column",
                        step.id
                    )
                })?;
                args.insert("column".into(), json!(column));
                if raw.as_deref() == Some(STAGE_ENTRY) {
                    raw = self.column_entry(&column).map(|(_, start)| start);
                }
            } else {
                ensure!(
                    raw.as_deref() != Some(STAGE_ENTRY),
                    "Only card.move steps may continue at {STAGE_ENTRY} ({})",
                    step.id
                );
            }
            if !matches!(kind, "condition" | "complete") {
                args.insert("next".into(), json!(target(raw, false)));
            }
            if kind == "condition" {
                for key in ["then", "otherwise"] {
                    let to = step
                        .with
                        .get(key)
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                    args.insert(key.into(), json!(target(to, true)));
                }
            }
            if matches!(kind, "approval" | "deliver") {
                args.insert(
                    "rework".into(),
                    json!(
                        target(step.on_changes_requested.clone(), true)
                            .filter(|_| step.on_changes_requested.is_some())
                    ),
                );
            }
            let compiled: WorkflowStep = serde_json::from_value(Value::Object(args))?;
            steps.insert(step.id.clone(), compiled);
        }
        let mut entries = BTreeMap::new();
        for (id, rule) in &self.on {
            entries.insert(
                id.clone(),
                target(Some(rule.start_at.clone()), true).unwrap(),
            );
        }
        if need_end {
            ensure!(
                !steps.contains_key(SKIPPED_END),
                "Step ID {SKIPPED_END} is reserved"
            );
            steps.insert(SKIPPED_END.into(), WorkflowStep::Complete);
        }
        Ok(WorkflowDefinition {
            id: self.id.clone(),
            name: self.name.clone(),
            columns: self.columns.clone(),
            entries,
            steps,
            workspace: self.workspace.clone(),
            max_steps: self.max_steps,
        })
    }
}

/// Stage (Board column) of every step: explicit `stage` wins; otherwise inferred by
/// walking each column's entry rule and switching lanes after `card.move`.
/// Steps unreachable from any column rule fall back to the first column.
pub fn step_stages(doc: &Document) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let idx: BTreeMap<&str, usize> = doc
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let mut order: Vec<String> = doc.column_ids();
    for r in doc.on.values() {
        if let Some(c) = &r.selector.column
            && !order.contains(c)
        {
            order.push(c.clone());
        }
    }
    fn walk(
        doc: &Document,
        idx: &BTreeMap<&str, usize>,
        id: &str,
        lane: &str,
        out: &mut BTreeMap<String, String>,
    ) {
        let Some(&i) = idx.get(id) else {
            return;
        };
        if out.contains_key(id) {
            return;
        }
        let step = &doc.steps[i];
        let lane = step.stage.clone().unwrap_or_else(|| lane.to_owned());
        out.insert(id.to_owned(), lane.clone());
        let mut after = lane.clone();
        if step.uses == "card.move"
            && let Some(t) = doc.move_target(step, Some(&lane))
        {
            after = t;
        }
        if step.uses == "condition" {
            for key in ["then", "otherwise"] {
                if let Some(to) = step.with.get(key).and_then(|v| v.as_str()) {
                    walk(doc, idx, to, &lane, out);
                }
            }
        } else if let Some(to) = doc.raw_next(i)
            && to != STAGE_ENTRY
        {
            walk(doc, idx, &to, &after, out);
        }
        if let Some(to) = &step.on_changes_requested {
            walk(doc, idx, to, &lane, out);
        }
    }
    // Explicit annotations first so inference never overrides them.
    for s in &doc.steps {
        if let Some(stage) = &s.stage {
            out.insert(s.id.clone(), stage.clone());
        }
    }
    let explicit = out.clone();
    for col in &order {
        for r in doc.on.values() {
            let rule_col = r.selector.column.as_deref();
            if rule_col == Some(col.as_str()) {
                let mut tmp = BTreeMap::new();
                walk(doc, &idx, &r.start_at, col, &mut tmp);
                for (k, v) in tmp {
                    out.entry(k).or_insert(v);
                }
            }
        }
    }
    let first = order.first().cloned().unwrap_or_else(|| "backlog".into());
    for s in &doc.steps {
        out.entry(s.id.clone()).or_insert_with(|| first.clone());
    }
    for (k, v) in explicit {
        out.insert(k, v);
    }
    out
}
pub fn column_details(
    column: &str,
    files: &[WorkflowSource],
    revisions: &[WorkflowRevision],
    subscriptions: &[EventSubscription],
) -> Vec<String> {
    let mut result = Vec::new();
    for f in files.iter().filter(|f| f.enabled) {
        let Some(w) = revisions
            .iter()
            .find(|w| w.workflow_id == f.workflow_id && w.revision == f.revision)
        else {
            continue;
        };
        let mut lines = Vec::new();
        for (name, rule) in f.rules.iter().filter(|(_, r)| r.enabled) {
            let event = rule.event.as_str();
            if rule.selector.column.as_deref() == Some(column) {
                let text = match event {
                    "card.column_entered" => "Entering this column",
                    "card.column_left" => "Leaving this column",
                    _ => event,
                };
                lines.push(format!("{text} → {} ({name})", rule.start_at));
            }
        }
        for sub in subscriptions
            .iter()
            .filter(|s| s.enabled && s.selector.column.as_deref() == Some(column))
        {
            if let SubscriptionTarget::Workflow {
                workflow_id, entry, ..
            } = &sub.target
                && workflow_id == &f.workflow_id
                && !sub.id.starts_with("source:")
            {
                lines.push(format!("{} → {entry}", sub.selector.types.join(", ")));
            }
        }
        // Only show effects alongside a trigger association; global-only workflows
        // remain on the Project page rather than placing an icon on every column.
        if !lines.is_empty() {
            let moves: BTreeSet<_> = w
                .definition
                .steps
                .iter()
                .filter_map(|(id, s)| match s {
                    WorkflowStep::Move { column: c, .. } if c == column => Some(id.clone()),
                    _ => None,
                })
                .collect();
            for step in moves {
                lines.push(format!("Flow effect: {step} moves the Card here"));
            }
            result.push(format!(
                "{} · {}\n{}",
                w.definition.name,
                f.filename,
                lines.join("\n")
            ));
        }
    }
    result
}
pub fn encode(document: &Document) -> Result<String> {
    Ok(serde_yaml_ng::to_string(document)?)
}

pub const DEFAULT_PLAN_PROMPT: &str = "Analyze the Card against the codebase without modifying files. If it is already implemented or obsolete, report `--state backlog` with a clear explanation; if requirements need clarification, report `--state blocked`; otherwise submit a concise implementation plan (files, risks, validation) with `--state completed`.";
pub const DEFAULT_DEVELOP_PROMPT: &str = "Evaluate the Card against the current codebase before editing:\n- Actionable: implement and validate the changes on the Card branch, commit deliverables, and report `--state completed`.\n- Already implemented, obsolete, or not applicable: make no unnecessary edits and report `--state backlog` explaining why.\n- Ambiguous or awaiting a human decision: report `--state blocked` with specific questions.";
pub const DEFAULT_REVIEW_PROMPT: &str = "Review the Card diff and earlier results without unrelated edits. Verify correctness, edge cases, and validation coverage, then report findings and remaining concerns with `--state completed` (or `--state blocked` if critical issues require human attention).";
pub const DEFAULT_MERGE_PROMPT: &str = "Run pre-merge checks and tests in the working directory, verify there are no blockers or unintended changes, and summarize readiness in your report.";

pub fn default_step_prompt(step_id: &str, profile: Option<&str>) -> &'static str {
    let lower = step_id.to_ascii_lowercase();
    if lower.contains("plan")
        || lower.contains("backlog")
        || lower.contains("design")
        || profile == Some("planner")
    {
        DEFAULT_PLAN_PROMPT
    } else if lower.contains("merge")
        || lower.contains("deliver")
        || lower.contains("queue")
        || profile == Some("merger")
    {
        DEFAULT_MERGE_PROMPT
    } else if lower.contains("review") || lower.contains("qa") || profile == Some("reviewer") {
        DEFAULT_REVIEW_PROMPT
    } else {
        DEFAULT_DEVELOP_PROMPT
    }
}

pub fn is_default_step_prompt(step_id: &str, profile: Option<&str>, instruction: &str) -> bool {
    let trimmed = instruction.trim();
    trimmed.is_empty() || trimmed == default_step_prompt(step_id, profile).trim()
}

/// Default workflow document for newly created Projects:
/// 1. Backlog: manual intake (no auto-analysis, no auto-move)
/// 2. Develop: automatic `@developer` implementation → moves to Review
/// 3. Review: Queue Merge (`deliver:review`) with `sync_upstream: true` (no `@reviewer` agent step;
///    external untrusted PRs pause at `approval:review` before merging).
pub fn default_document(workflow_id: impl Into<String>, workspace: WorkspaceSettings) -> Document {
    Document {
        schema: crate::migrations::CURRENT,
        id: workflow_id.into(),
        name: "Development".into(),
        enabled: true,
        columns: vec![
            WorkflowColumn {
                id: "backlog".into(),
                name: "Backlog".into(),
            },
            WorkflowColumn {
                id: "develop".into(),
                name: "Develop".into(),
            },
            WorkflowColumn {
                id: "review".into(),
                name: "Review".into(),
            },
        ],
        on: BTreeMap::from([
            (
                "develop".into(),
                WorkflowRule {
                    event: "card.column_entered".into(),
                    start_at: "agent:develop".into(),
                    selector: EventSelector {
                        column: Some("develop".into()),
                        ..Default::default()
                    },
                    enabled: true,
                },
            ),
            (
                "review".into(),
                WorkflowRule {
                    event: "card.column_entered".into(),
                    start_at: "condition:external_pr".into(),
                    selector: EventSelector {
                        column: Some("review".into()),
                        ..Default::default()
                    },
                    enabled: true,
                },
            ),
        ]),
        steps: vec![
            Step {
                id: "agent:develop".into(),
                name: Some("Implement".into()),
                uses: "agent".into(),
                with: BTreeMap::from([("profile".into(), json!("developer"))]),
                next: Some("move:develop".into()),
                on_changes_requested: None,
                stage: None,
                enabled: true,
            },
            Step {
                id: "move:develop".into(),
                name: Some("Move to Review".into()),
                uses: "card.move".into(),
                with: BTreeMap::from([("column".into(), json!("review"))]),
                next: Some("condition:external_pr".into()),
                on_changes_requested: None,
                stage: None,
                enabled: true,
            },
            Step {
                id: "condition:external_pr".into(),
                name: Some("External PR check".into()),
                uses: "condition".into(),
                with: BTreeMap::from([
                    ("pointer".into(), json!("/card/source_trust")),
                    ("equals".into(), json!("external_pr")),
                    ("then".into(), json!("approval:review")),
                    ("otherwise".into(), json!("deliver:review")),
                ]),
                next: None,
                on_changes_requested: None,
                stage: None,
                enabled: true,
            },
            Step {
                id: "deliver:review".into(),
                name: Some("Queue merge".into()),
                uses: "deliver".into(),
                with: BTreeMap::from([("sync_upstream".into(), json!(true))]),
                next: Some("__end".into()),
                on_changes_requested: Some("agent:develop".into()),
                stage: None,
                enabled: true,
            },
            Step {
                id: "approval:review".into(),
                name: Some("Human approval".into()),
                uses: "approval".into(),
                with: BTreeMap::from([(
                    "title".into(),
                    json!("External PR; human approval is required before merging"),
                )]),
                next: Some("__end".into()),
                on_changes_requested: Some("agent:develop".into()),
                stage: None,
                enabled: true,
            },
            Step {
                id: "__end".into(),
                name: None,
                uses: "complete".into(),
                with: BTreeMap::new(),
                next: None,
                on_changes_requested: None,
                stage: None,
                enabled: true,
            },
        ],
        workspace,
        max_steps: 100,
    }
}

/// UI-customizable workflow options derived locally from the YAML graph.
/// Always derivable even when the user adds custom columns, custom steps, or custom prompts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowOptions {
    /// Backlog new Card auto-analysis (`agent:plan`).
    pub backlog_analyze: bool,
    /// Backlog new Card auto-processing (moves to next active stage; waits for analysis if enabled).
    pub backlog_auto_develop: bool,
    /// Review requires manual human approval instead of Queue Merge (`deliver`).
    pub review_manual: bool,
    /// Queue Merge syncs the merged target branch to the upstream Git remote (`sync_upstream: true` by default;
    /// `false` = merge locally only without syncing upstream).
    pub sync_upstream: bool,
    /// External PR trust rule: `"require_review"` | `"same"` | `"manual_only"`.
    pub external_pr: String,
    /// Backlog waits for a free automatic Agent slot (`admit:develop`) before moving on.
    pub backlog_wait_for_agent: bool,
    /// Keep at most K planned Cards waiting in Backlog (`admit:plan` `max_ready`).
    pub backlog_max_ready: Option<u32>,
}

/// Default plan-ahead window when Backlog starts work only when an Agent is free.
pub const DEFAULT_BACKLOG_MAX_READY: u32 = 2;
const ADMIT_PLAN: &str = "admit:plan";
const ADMIT_DEVELOP: &str = "admit:develop";

impl Default for WorkflowOptions {
    fn default() -> Self {
        Self {
            backlog_analyze: false,
            backlog_auto_develop: false,
            review_manual: false,
            sync_upstream: true,
            external_pr: "require_review".into(),
            backlog_wait_for_agent: false,
            backlog_max_ready: None,
        }
    }
}

fn step_index_map(doc: &Document) -> BTreeMap<String, usize> {
    doc.steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect()
}

fn effective_next(doc: &Document, idx: usize) -> Option<&str> {
    let step = &doc.steps[idx];
    if matches!(step.uses.as_str(), "condition" | "complete") {
        None
    } else {
        step.next
            .as_deref()
            .or_else(|| doc.steps.get(idx + 1).map(|s| s.id.as_str()))
    }
}

pub(crate) fn ensure_terminal(doc: &mut Document) -> String {
    if let Some(existing) = doc
        .steps
        .iter()
        .find(|s| s.uses == "complete")
        .map(|s| s.id.clone())
    {
        return existing;
    }
    // Pin explicit next pointers on existing tail steps before appending `__end`
    // so sequential fallthrough semantics do not shift unexpectedly.
    for i in 0..doc.steps.len() {
        if !matches!(doc.steps[i].uses.as_str(), "condition" | "complete")
            && doc.steps[i].next.is_none()
            && let Some(next_id) = doc.steps.get(i + 1).map(|s| s.id.clone())
        {
            doc.steps[i].next = Some(next_id);
        }
    }
    let id = std::iter::successors(Some("__end".to_string()), |s| Some(format!("{s}_")))
        .find(|cand| !doc.steps.iter().any(|s| &s.id == cand))
        .unwrap();
    doc.steps.push(Step {
        id: id.clone(),
        name: None,
        uses: "complete".into(),
        with: BTreeMap::new(),
        next: None,
        on_changes_requested: None,
        stage: None,
        enabled: true,
    });
    id
}

pub(crate) fn pin_explicit_nexts(doc: &mut Document) {
    for i in 0..doc.steps.len() {
        if !matches!(doc.steps[i].uses.as_str(), "condition" | "complete")
            && doc.steps[i].next.is_none()
        {
            doc.steps[i].next = doc.steps.get(i + 1).map(|s| s.id.clone());
        }
    }
}

fn backlog_column_id(doc: &Document) -> String {
    doc.columns
        .iter()
        .find(|c| c.id == "backlog")
        .or_else(|| doc.columns.first())
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "backlog".into())
}

fn next_stage_after_backlog(doc: &Document) -> (String, String) {
    let backlog = backlog_column_id(doc);
    if let Some(pos) = doc.columns.iter().position(|c| c.id == backlog)
        && let Some(next_col) = doc.columns.get(pos + 1)
    {
        return (next_col.id.clone(), next_col.name.clone());
    }
    if let Some(dev) = doc.columns.iter().find(|c| c.id == "develop") {
        return (dev.id.clone(), dev.name.clone());
    }
    ("develop".into(), "Develop".into())
}

fn review_column_id(doc: &Document) -> String {
    doc.columns
        .iter()
        .find(|c| c.id == "review")
        .or_else(|| doc.columns.last())
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "review".into())
}

fn entry_step_for_column(doc: &Document, column_id: &str) -> Option<String> {
    doc.on
        .get(column_id)
        .filter(|r| r.enabled)
        .map(|r| r.start_at.clone())
        .or_else(|| {
            doc.on
                .values()
                .find(|r| r.enabled && r.selector.column.as_deref() == Some(column_id))
                .map(|r| r.start_at.clone())
        })
}

fn is_referenced_outside(
    doc: &Document,
    target_id: &str,
    ignore_steps: &[&str],
    ignore_rules: &[&str],
) -> bool {
    for (r_key, r) in &doc.on {
        if !ignore_rules.contains(&r_key.as_str()) && r.start_at == target_id {
            return true;
        }
    }
    for (idx, s) in doc.steps.iter().enumerate() {
        if ignore_steps.contains(&s.id.as_str()) {
            continue;
        }
        if effective_next(doc, idx) == Some(target_id) {
            return true;
        }
        if s.on_changes_requested.as_deref() == Some(target_id) {
            return true;
        }
        if s.uses == "condition"
            && (s.with.get("then").and_then(|v| v.as_str()) == Some(target_id)
                || s.with.get("otherwise").and_then(|v| v.as_str()) == Some(target_id))
        {
            return true;
        }
    }
    false
}

pub fn derive_options(doc: &Document) -> WorkflowOptions {
    let idx_map = step_index_map(doc);
    let backlog_col = backlog_column_id(doc);
    let backlog_rule = doc.on.get(&backlog_col).filter(|r| r.enabled).or_else(|| {
        doc.on.values().find(|r| {
            r.enabled
                && (r.selector.column.as_deref() == Some(backlog_col.as_str())
                    || r.event == "card.created")
        })
    });

    let mut backlog_analyze = false;
    let mut backlog_auto_develop = false;
    let mut backlog_wait_for_agent = false;
    let mut backlog_max_ready = None;
    if let Some(rule) = backlog_rule {
        let mut cur = Some(rule.start_at.as_str());
        let mut seen = BTreeSet::new();
        while let Some(step_id) = cur {
            if !seen.insert(step_id) {
                break;
            }
            let Some(&idx) = idx_map.get(step_id) else {
                break;
            };
            let step = &doc.steps[idx];
            if step.uses == "complete" {
                break;
            }
            if step.uses == "admit" {
                if step
                    .with
                    .get("agent_slot")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
                {
                    backlog_wait_for_agent = true;
                }
                if let Some(k) = step.with.get("max_ready").and_then(|v| v.as_u64()) {
                    backlog_max_ready = Some(k as u32);
                }
            }
            if step.uses == "card.move" {
                let target = step
                    .with
                    .get("column")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if target != backlog_col && !target.is_empty() {
                    backlog_auto_develop = true;
                }
                break;
            }
            if step.uses == "agent" {
                backlog_analyze = true;
            }
            cur = effective_next(doc, idx);
        }
    }

    let review_col = review_column_id(doc);
    let mut internal_has_deliver = false;
    if let Some(start) = entry_step_for_column(doc, &review_col) {
        let mut cur = Some(start.as_str());
        let mut seen = BTreeSet::new();
        while let Some(step_id) = cur {
            if !seen.insert(step_id) {
                break;
            }
            let Some(&idx) = idx_map.get(step_id) else {
                break;
            };
            let step = &doc.steps[idx];
            match step.uses.as_str() {
                "deliver" => {
                    internal_has_deliver = true;
                    break;
                }
                "approval" | "complete" => break,
                "condition" => {
                    // Follow the internal (`otherwise`) path when checking `external_pr`.
                    if step.with.get("pointer").and_then(|v| v.as_str())
                        == Some("/card/source_trust")
                        && step.with.get("equals").and_then(|v| v.as_str()) == Some("external_pr")
                    {
                        cur = step.with.get("otherwise").and_then(|v| v.as_str());
                    } else {
                        cur = step
                            .with
                            .get("then")
                            .and_then(|v| v.as_str())
                            .or_else(|| step.with.get("otherwise").and_then(|v| v.as_str()));
                    }
                }
                _ => {
                    cur = effective_next(doc, idx);
                }
            }
        }
    } else {
        internal_has_deliver = doc.steps.iter().any(|s| s.uses == "deliver");
    }

    let sync_upstream = doc
        .steps
        .iter()
        .find(|s| s.uses == "deliver")
        .and_then(|s| s.with.get("sync_upstream"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let external_pr = if doc
        .on
        .values()
        .any(|r| r.enabled && r.selector.source_kind.as_deref() == Some("issue"))
    {
        "manual_only".into()
    } else if doc.steps.iter().any(|s| {
        s.uses == "condition"
            && s.with.get("pointer").and_then(|v| v.as_str()) == Some("/card/source_trust")
            && s.with.get("equals").and_then(|v| v.as_str()) == Some("external_pr")
    }) {
        "require_review".into()
    } else {
        "same".into()
    };

    WorkflowOptions {
        backlog_analyze,
        backlog_auto_develop,
        review_manual: !internal_has_deliver,
        sync_upstream,
        external_pr,
        backlog_wait_for_agent: backlog_wait_for_agent && backlog_auto_develop,
        backlog_max_ready,
    }
}

/// Removes the managed Backlog admission gates, reconnecting their predecessors.
fn strip_backlog_admits(doc: &mut Document) {
    pin_explicit_nexts(doc);
    for gate in [ADMIT_PLAN, ADMIT_DEVELOP] {
        let Some(pos) = doc
            .steps
            .iter()
            .position(|s| s.id == gate && s.uses == "admit")
        else {
            continue;
        };
        let target = doc.steps[pos].next.clone().unwrap_or_default();
        doc.steps.remove(pos);
        for rule in doc.on.values_mut() {
            if rule.start_at == gate {
                rule.start_at = target.clone();
            }
        }
        for step in &mut doc.steps {
            if step.next.as_deref() == Some(gate) {
                step.next = Some(target.clone());
            }
            if step.on_changes_requested.as_deref() == Some(gate) {
                step.on_changes_requested = Some(target.clone());
            }
            for key in ["then", "otherwise"] {
                if step.with.get(key).and_then(|v| v.as_str()) == Some(gate) {
                    step.with.insert(key.into(), json!(target));
                }
            }
        }
    }
}

/// Inserts `admit:develop` before the Backlog move and `admit:plan` before planning.
fn apply_backlog_admission(doc: &mut Document, wait_for_agent: bool, max_ready: Option<u32>) {
    if !wait_for_agent {
        return;
    }
    pin_explicit_nexts(doc);
    let backlog_col = backlog_column_id(doc);
    let Some(rule_key) = doc
        .on
        .iter()
        .find(|(k, r)| **k == backlog_col || r.selector.column.as_deref() == Some(&backlog_col))
        .map(|(k, _)| k.clone())
    else {
        return;
    };
    let start = doc.on[&rule_key].start_at.clone();
    // Walk the Backlog chain to the first move out of Backlog.
    let mut chain = Vec::new();
    let mut move_id = None;
    let mut cur = Some(start.clone());
    while let Some(id) = cur {
        if chain.contains(&id) {
            break;
        }
        let Some(step) = doc.steps.iter().find(|s| s.id == id) else {
            break;
        };
        if step.uses == "card.move" {
            move_id = Some(id);
            break;
        }
        if matches!(step.uses.as_str(), "complete" | "condition") {
            break;
        }
        chain.push(id);
        cur = step.next.clone();
    }
    let Some(move_id) = move_id else {
        return;
    };
    for step in doc.steps.iter_mut().filter(|s| chain.contains(&s.id)) {
        if step.next.as_deref() == Some(move_id.as_str()) {
            step.next = Some(ADMIT_DEVELOP.into());
        }
    }
    let rule = doc.on.get_mut(&rule_key).unwrap();
    if rule.start_at == move_id {
        rule.start_at = ADMIT_DEVELOP.into();
    }
    let at = doc.steps.iter().position(|s| s.id == move_id).unwrap_or(0);
    doc.steps.insert(
        at,
        Step {
            id: ADMIT_DEVELOP.into(),
            name: Some("Wait for free Agent".into()),
            uses: "admit".into(),
            with: BTreeMap::new(),
            next: Some(move_id),
            on_changes_requested: None,
            stage: None,
            enabled: true,
        },
    );
    let plans = chain
        .iter()
        .any(|id| doc.steps.iter().any(|s| &s.id == id && s.uses == "agent"));
    if let Some(k) = max_ready.filter(|_| plans) {
        let rule = doc.on.get_mut(&rule_key).unwrap();
        let first = std::mem::replace(&mut rule.start_at, ADMIT_PLAN.into());
        doc.steps.insert(
            0,
            Step {
                id: ADMIT_PLAN.into(),
                name: Some("Plan-ahead limit".into()),
                uses: "admit".into(),
                with: BTreeMap::from([
                    ("agent_slot".into(), json!(false)),
                    ("max_ready".into(), json!(k)),
                ]),
                next: Some(first),
                on_changes_requested: None,
                stage: None,
                enabled: true,
            },
        );
    }
}

/// Backlog starts work only when an automatic Agent slot is free; with analysis
/// enabled, at most `max_ready` planned Cards wait in Backlog (planning pauses beyond).
/// `wait_for_agent == true` implies auto-start of the next stage.
pub fn set_backlog_admission(doc: &mut Document, wait_for_agent: bool, max_ready: Option<u32>) {
    let current = derive_options(doc);
    let max_ready = max_ready.map(|k| k.clamp(1, 100));
    strip_backlog_admits(doc);
    if wait_for_agent && !current.backlog_auto_develop {
        set_backlog_flow(doc, current.backlog_analyze, true);
    }
    apply_backlog_admission(
        doc,
        wait_for_agent,
        max_ready.or(Some(DEFAULT_BACKLOG_MAX_READY)),
    );
}

/// Surgically updates Backlog intake behavior (auto-analyze and/or auto-move to next stage)
/// without clobbering custom columns, custom middle stages, or custom prompts. Existing
/// admission gates are preserved while auto-start remains enabled.
pub fn set_backlog_options(doc: &mut Document, analyze: bool, auto_develop: bool) {
    let current = derive_options(doc);
    strip_backlog_admits(doc);
    set_backlog_flow(doc, analyze, auto_develop);
    apply_backlog_admission(
        doc,
        current.backlog_wait_for_agent && auto_develop,
        current
            .backlog_max_ready
            .or(Some(DEFAULT_BACKLOG_MAX_READY)),
    );
}

fn set_backlog_flow(doc: &mut Document, analyze: bool, auto_develop: bool) {
    pin_explicit_nexts(doc);
    let end_id = ensure_terminal(doc);
    let backlog_col = backlog_column_id(doc);
    let (next_col_id, next_col_name) = next_stage_after_backlog(doc);
    let next_stage_entry =
        entry_step_for_column(doc, &next_col_id).unwrap_or_else(|| end_id.clone());

    let (plan_profile, plan_prompt) = doc
        .steps
        .iter()
        .find(|s| matches!(s.id.as_str(), "agent:plan" | "plan") && s.uses == "agent")
        .map(|s| {
            let prof = s
                .with
                .get("profile")
                .and_then(|v| v.as_str())
                .unwrap_or("developer")
                .to_owned();
            let instr = s
                .with
                .get("instruction")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            (prof, instr)
        })
        .unwrap_or_else(|| ("developer".into(), None));

    let existing_approval = doc
        .steps
        .iter()
        .find(|s| matches!(s.id.as_str(), "approval:plan" | "approve_plan") && s.uses == "approval")
        .map(|s| s.id.clone());

    // Determine existing custom rule key for Backlog if not named `"backlog"`.
    let rule_key = if doc.on.contains_key(&backlog_col) {
        backlog_col.clone()
    } else {
        doc.on
            .iter()
            .find(|(_, r)| r.selector.column.as_deref() == Some(backlog_col.as_str()))
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| backlog_col.clone())
    };

    if !analyze && !auto_develop {
        // Keep workflow valid (`!doc.on.is_empty()`): only remove the Backlog rule when other rules exist.
        if doc.on.len() > 1 {
            doc.on.remove(&rule_key);
        } else if let Some(r) = doc.on.get_mut(&rule_key) {
            r.enabled = false;
        }
        let candidates = ["agent:plan", "approval:plan", "move:backlog"];
        for cand in candidates {
            if !is_referenced_outside(doc, cand, &candidates, &[rule_key.as_str()]) {
                doc.steps.retain(|s| s.id != cand);
            }
        }
        return;
    }

    let source_filter = doc
        .on
        .get("develop")
        .and_then(|r| r.selector.source_kind.clone());

    let move_step_id = doc
        .steps
        .iter()
        .find(|s| {
            s.uses == "card.move"
                && (s.id == "move:backlog"
                    || s.id == "enter_develop"
                    || s.with.get("column").and_then(|v| v.as_str()) == Some(next_col_id.as_str()))
                && s.id != "move:develop"
        })
        .map(|s| s.id.clone())
        .unwrap_or_else(|| "move:backlog".into());

    let plan_step_id = doc
        .steps
        .iter()
        .find(|s| matches!(s.id.as_str(), "agent:plan" | "plan") && s.uses == "agent")
        .map(|s| s.id.clone())
        .unwrap_or_else(|| "agent:plan".into());

    if auto_develop {
        let move_step = Step {
            id: move_step_id.clone(),
            name: Some(format!("Move to {next_col_name}")),
            uses: "card.move".into(),
            with: BTreeMap::from([("column".into(), json!(next_col_id))]),
            next: Some(next_stage_entry),
            on_changes_requested: None,
            stage: None,
            enabled: true,
        };
        if let Some(existing) = doc.steps.iter_mut().find(|s| s.id == move_step_id) {
            *existing = move_step;
        } else {
            let insert_at = doc
                .steps
                .iter()
                .position(|s| s.id == plan_step_id)
                .map(|i| i + 1)
                .unwrap_or(0);
            doc.steps.insert(insert_at, move_step);
        }
    } else if !is_referenced_outside(
        doc,
        &move_step_id,
        &[plan_step_id.as_str(), "approval:plan", "approve_plan"],
        &[rule_key.as_str()],
    ) {
        doc.steps.retain(|s| s.id != move_step_id);
    }

    if analyze {
        let after_plan = if let Some(ref app_id) = existing_approval {
            let after_app = if auto_develop {
                move_step_id.clone()
            } else {
                end_id.clone()
            };
            if let Some(app_step) = doc.steps.iter_mut().find(|s| &s.id == app_id) {
                app_step.next = Some(after_app);
            }
            app_id.clone()
        } else if auto_develop {
            move_step_id.clone()
        } else {
            end_id.clone()
        };

        if let Some(existing) = doc.steps.iter_mut().find(|s| s.id == plan_step_id) {
            existing.next = Some(after_plan);
        } else {
            let mut with = BTreeMap::from([("profile".into(), json!(plan_profile))]);
            if let Some(prompt) = plan_prompt {
                with.insert("instruction".into(), json!(prompt));
            }
            doc.steps.insert(
                0,
                Step {
                    id: plan_step_id.clone(),
                    name: Some("Analyze task".into()),
                    uses: "agent".into(),
                    with,
                    next: Some(after_plan),
                    on_changes_requested: None,
                    stage: None,
                    enabled: true,
                },
            );
        }

        doc.on.insert(
            rule_key,
            WorkflowRule {
                event: "card.column_entered".into(),
                start_at: plan_step_id,
                selector: EventSelector {
                    column: Some(backlog_col),
                    source_kind: source_filter,
                    ..Default::default()
                },
                enabled: true,
            },
        );
    } else {
        // `!analyze && auto_develop`: move immediately into next stage on entering Backlog.
        let candidates = [plan_step_id.as_str(), "approval:plan", "approve_plan"];
        for cand in candidates {
            if !is_referenced_outside(doc, cand, &candidates, &[rule_key.as_str()]) {
                doc.steps.retain(|s| s.id != cand);
            }
        }
        doc.on.insert(
            rule_key,
            WorkflowRule {
                event: "card.column_entered".into(),
                start_at: move_step_id,
                selector: EventSelector {
                    column: Some(backlog_col),
                    source_kind: source_filter,
                    ..Default::default()
                },
                enabled: true,
            },
        );
    }
}

fn find_rework_target(doc: &Document) -> Option<String> {
    doc.steps
        .iter()
        .find(|s| {
            matches!(s.id.as_str(), "agent:develop" | "develop" | "implement") && s.uses == "agent"
        })
        .or_else(|| doc.steps.iter().find(|s| s.uses == "agent"))
        .map(|s| s.id.clone())
}

fn retarget_moves_to_column(doc: &mut Document, col_id: &str, new_entry: &str) {
    for step in &mut doc.steps {
        if step.uses == "card.move"
            && step.with.get("column").and_then(|v| v.as_str()) == Some(col_id)
        {
            step.next = Some(new_entry.to_owned());
        }
    }
}

/// Surgically switches the Review stage between Queue Merge (`manual == false`) and
/// Manual Human Review (`manual == true`), preserving any custom `@reviewer` / pre-merge
/// agent steps if the user added them in their template, as well as custom columns & prompts.
pub fn set_review_manual(doc: &mut Document, manual: bool) {
    pin_explicit_nexts(doc);
    let end_id = ensure_terminal(doc);
    let review_col = review_column_id(doc);
    let current_opts = derive_options(doc);
    let rework_target = find_rework_target(doc);

    // Check if the user explicitly configured a review/pre-merge agent step in Review.
    let existing_review_agent = doc
        .steps
        .iter()
        .find(|s| {
            s.uses == "agent"
                && matches!(
                    s.id.as_str(),
                    "agent:review" | "review" | "agent:merge" | "merge_verify"
                )
        })
        .map(|s| s.id.clone());

    let rule_key = if doc.on.contains_key(&review_col) {
        review_col.clone()
    } else {
        doc.on
            .iter()
            .find(|(_, r)| r.selector.column.as_deref() == Some(review_col.as_str()))
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| review_col.clone())
    };
    let source_filter = doc
        .on
        .get(&rule_key)
        .and_then(|r| r.selector.source_kind.clone())
        .or_else(|| {
            doc.on
                .get("develop")
                .and_then(|r| r.selector.source_kind.clone())
        });

    if manual {
        // Ensure `approval:review` exists.
        let approval_id = doc
            .steps
            .iter()
            .find(|s| {
                s.uses == "approval"
                    && matches!(s.id.as_str(), "approval:review" | "approve_review")
            })
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "approval:review".into());

        if let Some(app_step) = doc.steps.iter_mut().find(|s| s.id == approval_id) {
            app_step.with.insert(
                "title".into(),
                json!("Human review is required before merging"),
            );
            app_step.next = Some(end_id.clone());
            if app_step.on_changes_requested.is_none() {
                app_step.on_changes_requested = rework_target.clone();
            }
        } else {
            let end_pos = doc
                .steps
                .iter()
                .position(|s| s.id == end_id)
                .unwrap_or(doc.steps.len());
            doc.steps.insert(
                end_pos,
                Step {
                    id: approval_id.clone(),
                    name: Some("Human approval".into()),
                    uses: "approval".into(),
                    with: BTreeMap::from([(
                        "title".into(),
                        json!("Human review is required before merging"),
                    )]),
                    next: Some(end_id.clone()),
                    on_changes_requested: rework_target,
                    stage: None,
                    enabled: true,
                },
            );
        }

        let new_entry = if let Some(ref rev_agent) = existing_review_agent {
            if let Some(s) = doc.steps.iter_mut().find(|s| &s.id == rev_agent) {
                s.next = Some(approval_id.clone());
            }
            rev_agent.clone()
        } else {
            approval_id.clone()
        };

        doc.on.insert(
            rule_key.clone(),
            WorkflowRule {
                event: "card.column_entered".into(),
                start_at: new_entry.clone(),
                selector: EventSelector {
                    column: Some(review_col.clone()),
                    source_kind: source_filter,
                    ..Default::default()
                },
                enabled: true,
            },
        );
        retarget_moves_to_column(doc, &review_col, &new_entry);

        // Clean up `deliver:review` and `condition:external_pr` if no longer referenced.
        let deliver_ids: Vec<String> = doc
            .steps
            .iter()
            .filter(|s| s.uses == "deliver")
            .map(|s| s.id.clone())
            .collect();
        let mut ignore: Vec<&str> = vec!["condition:external_pr"];
        for d in &deliver_ids {
            ignore.push(d.as_str());
        }
        if !is_referenced_outside(doc, "condition:external_pr", &ignore, &[]) {
            doc.steps.retain(|s| s.id != "condition:external_pr");
        }
        for d_id in &deliver_ids {
            if !is_referenced_outside(doc, d_id, &[], &[]) {
                doc.steps.retain(|s| &s.id != d_id);
            }
        }
    } else {
        // Queue Merge mode (`deliver:review`).
        let deliver_id = doc
            .steps
            .iter()
            .find(|s| s.uses == "deliver")
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "deliver:review".into());

        if let Some(del_step) = doc.steps.iter_mut().find(|s| s.id == deliver_id) {
            del_step
                .with
                .entry("sync_upstream".into())
                .or_insert_with(|| json!(current_opts.sync_upstream));
            if del_step.next.is_none() {
                del_step.next = Some(end_id.clone());
            }
            if del_step.on_changes_requested.is_none() {
                del_step.on_changes_requested = rework_target.clone();
            }
        } else {
            let end_pos = doc
                .steps
                .iter()
                .position(|s| s.id == end_id)
                .unwrap_or(doc.steps.len());
            doc.steps.insert(
                end_pos,
                Step {
                    id: deliver_id.clone(),
                    name: Some("Queue merge".into()),
                    uses: "deliver".into(),
                    with: BTreeMap::from([(
                        "sync_upstream".into(),
                        json!(current_opts.sync_upstream),
                    )]),
                    next: Some(end_id.clone()),
                    on_changes_requested: rework_target.clone(),
                    stage: None,
                    enabled: true,
                },
            );
        }

        let internal_target = if let Some(ref rev_agent) = existing_review_agent {
            if let Some(s) = doc.steps.iter_mut().find(|s| &s.id == rev_agent) {
                s.next = Some(deliver_id.clone());
            }
            rev_agent.clone()
        } else {
            deliver_id.clone()
        };

        let new_entry = if current_opts.external_pr == "require_review" {
            // Ensure `approval:review` exists for external PRs.
            let approval_id = doc
                .steps
                .iter()
                .find(|s| {
                    s.uses == "approval"
                        && matches!(s.id.as_str(), "approval:review" | "approve_review")
                })
                .map(|s| s.id.clone())
                .unwrap_or_else(|| "approval:review".into());
            if let Some(app_step) = doc.steps.iter_mut().find(|s| s.id == approval_id) {
                app_step.with.insert(
                    "title".into(),
                    json!("External PR; human approval is required before merging"),
                );
            } else {
                let end_pos = doc
                    .steps
                    .iter()
                    .position(|s| s.id == end_id)
                    .unwrap_or(doc.steps.len());
                doc.steps.insert(
                    end_pos,
                    Step {
                        id: approval_id.clone(),
                        name: Some("Human approval".into()),
                        uses: "approval".into(),
                        with: BTreeMap::from([(
                            "title".into(),
                            json!("External PR; human approval is required before merging"),
                        )]),
                        next: Some(end_id.clone()),
                        on_changes_requested: rework_target,
                        stage: None,
                        enabled: true,
                    },
                );
            }
            let cond_step = Step {
                id: "condition:external_pr".into(),
                name: Some("External PR check".into()),
                uses: "condition".into(),
                with: BTreeMap::from([
                    ("pointer".into(), json!("/card/source_trust")),
                    ("equals".into(), json!("external_pr")),
                    ("then".into(), json!(approval_id)),
                    ("otherwise".into(), json!(internal_target)),
                ]),
                next: None,
                on_changes_requested: None,
                stage: None,
                enabled: true,
            };
            if let Some(existing) = doc
                .steps
                .iter_mut()
                .find(|s| s.id == "condition:external_pr")
            {
                *existing = cond_step;
            } else {
                let target_pos = doc
                    .steps
                    .iter()
                    .position(|s| s.id == deliver_id)
                    .unwrap_or(doc.steps.len());
                doc.steps.insert(target_pos, cond_step);
            }
            "condition:external_pr".to_owned()
        } else {
            // Remove unused `approval:review` if switching from Manual to Queue Merge with `same`/`manual_only`.
            for app_cand in ["approval:review", "approve_review"] {
                if !is_referenced_outside(
                    doc,
                    app_cand,
                    &["condition:external_pr"],
                    &[rule_key.as_str()],
                ) {
                    doc.steps.retain(|s| s.id != app_cand);
                }
            }
            if !is_referenced_outside(doc, "condition:external_pr", &[], &[rule_key.as_str()]) {
                doc.steps.retain(|s| s.id != "condition:external_pr");
            }
            internal_target
        };

        doc.on.insert(
            rule_key,
            WorkflowRule {
                event: "card.column_entered".into(),
                start_at: new_entry.clone(),
                selector: EventSelector {
                    column: Some(review_col.clone()),
                    source_kind: source_filter,
                    ..Default::default()
                },
                enabled: true,
            },
        );
        retarget_moves_to_column(doc, &review_col, &new_entry);
    }
}

/// Surgically updates `sync_upstream` on all `deliver` steps in the workflow.
pub fn set_sync_upstream(doc: &mut Document, sync_upstream: bool) {
    let mut updated_any = false;
    for step in &mut doc.steps {
        if step.uses == "deliver" {
            step.with
                .insert("sync_upstream".into(), json!(sync_upstream));
            updated_any = true;
        }
    }
    if !updated_any {
        // If user toggled upstream sync while in manual mode, switch Review to Queue Merge with the chosen sync setting.
        set_review_manual(doc, false);
        for step in &mut doc.steps {
            if step.uses == "deliver" {
                step.with
                    .insert("sync_upstream".into(), json!(sync_upstream));
            }
        }
    }
}

/// Surgically updates the External PR handling rule (`"require_review"` | `"same"` | `"manual_only"`).
pub fn set_external_pr_rule(doc: &mut Document, mode: &str) {
    pin_explicit_nexts(doc);
    let end_id = ensure_terminal(doc);
    let review_col = review_column_id(doc);
    let opts = derive_options(doc);
    let rework_target = find_rework_target(doc);

    let source_filter = if mode == "manual_only" {
        Some("issue".to_owned())
    } else {
        None
    };
    for rule in doc.on.values_mut() {
        if rule.event == "card.column_entered" {
            rule.selector.source_kind = source_filter.clone();
        }
    }

    if opts.review_manual {
        // When Review is already manual for all Cards, remove redundant external_pr condition if present.
        if let Some(cond_then) = doc
            .steps
            .iter()
            .find(|s| s.id == "condition:external_pr")
            .and_then(|s| s.with.get("then").and_then(|v| v.as_str()))
            .map(str::to_owned)
        {
            for rule in doc.on.values_mut() {
                if rule.start_at == "condition:external_pr" {
                    rule.start_at = cond_then.clone();
                }
            }
            retarget_moves_to_column(doc, &review_col, &cond_then);
            doc.steps.retain(|s| s.id != "condition:external_pr");
        }
        return;
    }

    // Queue merge is active on the internal path; find what the internal path starts at.
    let current_review_start = entry_step_for_column(doc, &review_col);
    let internal_start = doc
        .steps
        .iter()
        .find(|s| s.id == "condition:external_pr")
        .and_then(|s| s.with.get("otherwise").and_then(|v| v.as_str()))
        .map(str::to_owned)
        .or(current_review_start)
        .unwrap_or_else(|| "deliver:review".into());

    if mode == "require_review" {
        let approval_id = doc
            .steps
            .iter()
            .find(|s| {
                s.uses == "approval"
                    && matches!(s.id.as_str(), "approval:review" | "approve_review")
            })
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "approval:review".into());
        if !doc.steps.iter().any(|s| s.id == approval_id) {
            let end_pos = doc
                .steps
                .iter()
                .position(|s| s.id == end_id)
                .unwrap_or(doc.steps.len());
            doc.steps.insert(
                end_pos,
                Step {
                    id: approval_id.clone(),
                    name: Some("Human approval".into()),
                    uses: "approval".into(),
                    with: BTreeMap::from([(
                        "title".into(),
                        json!("External PR; human approval is required before merging"),
                    )]),
                    next: Some(end_id),
                    on_changes_requested: rework_target,
                    stage: None,
                    enabled: true,
                },
            );
        }
        let cond_step = Step {
            id: "condition:external_pr".into(),
            name: Some("External PR check".into()),
            uses: "condition".into(),
            with: BTreeMap::from([
                ("pointer".into(), json!("/card/source_trust")),
                ("equals".into(), json!("external_pr")),
                ("then".into(), json!(approval_id)),
                ("otherwise".into(), json!(internal_start)),
            ]),
            next: None,
            on_changes_requested: None,
            stage: None,
            enabled: true,
        };
        if let Some(existing) = doc
            .steps
            .iter_mut()
            .find(|s| s.id == "condition:external_pr")
        {
            *existing = cond_step;
        } else {
            let pos = doc
                .steps
                .iter()
                .position(|s| s.id == internal_start)
                .unwrap_or(doc.steps.len());
            doc.steps.insert(pos, cond_step);
        }
        for rule in doc.on.values_mut() {
            if rule.selector.column.as_deref() == Some(review_col.as_str()) {
                rule.start_at = "condition:external_pr".into();
            }
        }
        retarget_moves_to_column(doc, &review_col, "condition:external_pr");
    } else {
        for rule in doc.on.values_mut() {
            if rule.selector.column.as_deref() == Some(review_col.as_str())
                && rule.start_at == "condition:external_pr"
            {
                rule.start_at = internal_start.clone();
            }
        }
        retarget_moves_to_column(doc, &review_col, &internal_start);
        doc.steps.retain(|s| s.id != "condition:external_pr");
        for app_cand in ["approval:review", "approve_review"] {
            if !is_referenced_outside(doc, app_cand, &[], &[]) {
                doc.steps.retain(|s| s.id != app_cand);
            }
        }
    }
}

pub fn column_summary(
    column: &str,
    files: &[WorkflowSource],
    revisions: &[WorkflowRevision],
) -> Option<String> {
    for f in files.iter().filter(|f| f.enabled) {
        let Some(w) = revisions
            .iter()
            .find(|w| w.workflow_id == f.workflow_id && w.revision == f.revision)
        else {
            continue;
        };
        for rule in f.rules.values().filter(|r| r.enabled) {
            if rule.selector.column.as_deref() != Some(column) {
                continue;
            }
            let Some(first) = w.definition.steps.get(&rule.start_at) else {
                continue;
            };
            let has_deliver = w
                .definition
                .steps
                .values()
                .any(|s| matches!(s, WorkflowStep::Deliver { .. }));
            match first {
                WorkflowStep::Agent { profile, next, .. } => {
                    let agent = profile
                        .as_deref()
                        .map(|p| format!("@{p}"))
                        .unwrap_or_else(|| "Agent".into());
                    let following = next.as_deref().and_then(|n| w.definition.steps.get(n));
                    return Some(match following {
                        Some(WorkflowStep::Deliver { .. }) => format!("{agent} → Auto-merge"),
                        Some(WorkflowStep::Approval { .. }) => format!("{agent} · Review"),
                        _ => agent,
                    });
                }
                WorkflowStep::Condition { .. } if has_deliver => {
                    return Some(if sync_label(w) {
                        "Queue merge · Sync".into()
                    } else {
                        "Queue merge · Local".into()
                    });
                }
                WorkflowStep::Deliver { sync_upstream, .. } => {
                    return Some(if *sync_upstream {
                        "Queue merge · Sync".into()
                    } else {
                        "Queue merge · Local".into()
                    });
                }
                WorkflowStep::Approval { .. } => {
                    return Some("Approval".into());
                }
                _ => return Some("Automated".into()),
            }
        }
    }
    None
}

fn sync_label(w: &WorkflowRevision) -> bool {
    w.definition
        .steps
        .values()
        .find_map(|s| match s {
            WorkflowStep::Deliver { sync_upstream, .. } => Some(*sync_upstream),
            _ => None,
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_sequence_rework_and_duplicates() {
        let source = "schema: 1\nid: test\nname: Test\non:\n  manual:\n    event: manual\n    start_at: plan\nsteps:\n  - id: plan\n    uses: agent\n    with:\n      profile: codex\n      instruction: |\n        Read and explain.\n  - id: gate\n    uses: approval\n    with:\n      title: Approve\n    on_changes_requested: plan\n";
        let doc = parse(source).unwrap();
        let f = doc.compile().unwrap();
        assert!(matches!(&f.steps["plan"], WorkflowStep::Agent {next:Some(n),..} if n=="gate"));
        assert!(
            matches!(&f.steps["gate"], WorkflowStep::Approval {rework:Some(n),..} if n=="plan")
        );
        assert!(parse(&source.replace("schema: 1", "schema: 1\nschema: 1")).is_err());
        assert!(
            parse(&source.replace("event: manual", "event: manual\n    event: manual")).is_err()
        );
        assert!(parse(&source.replace("schema: 1", "schema: 99")).is_err());
        assert!(
            parse(&source.replace("uses: agent", "uses: arbitrary"))
                .unwrap()
                .compile()
                .is_err()
        );
    }

    #[test]
    fn default_document_and_derived_options_preserve_custom_stages_steps_and_prompts() {
        let default_doc = default_document("delivery:test", WorkspaceSettings::default());
        assert!(!default_doc.on.contains_key("backlog"));
        assert!(!default_doc.steps.iter().any(|s| s.id == "agent:review"));
        let opts = derive_options(&default_doc);
        assert_eq!(
            opts,
            WorkflowOptions {
                backlog_analyze: false,
                backlog_auto_develop: false,
                review_manual: false,
                sync_upstream: true,
                external_pr: "require_review".into(),
                backlog_wait_for_agent: false,
                backlog_max_ready: None,
            }
        );
        let compiled_default = default_doc.compile().unwrap();
        assert!(matches!(
            compiled_default.steps.get("deliver:review"),
            Some(WorkflowStep::Deliver {
                sync_upstream: true,
                ..
            })
        ));

        // Start from a custom 4-column workflow with a custom QA notify step and custom prompt.
        let mut doc = parse(
            r#"schema: 1
id: dev
name: Custom 4-Column
columns:
  - id: backlog
    name: Backlog
  - id: design
    name: Design
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  design:
    event: card.column_entered
    start_at: agent:design
    match:
      column: design
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:design
    uses: agent
    with:
      profile: developer
      instruction: Custom design spec prompt
    next: notify:qa
  - id: notify:qa
    uses: notify
    with:
      message: Design ready
    next: __end
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Custom develop prompt
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: deliver:review
  - id: deliver:review
    uses: deliver
    with:
      sync_upstream: true
    next: __end
  - id: __end
    uses: complete
"#,
        )
        .unwrap();

        // 1. Enable Backlog auto-analyze + Backlog auto-process (routes into `design`!).
        set_backlog_options(&mut doc, true, true);
        let o1 = derive_options(&doc);
        assert!(o1.backlog_analyze);
        assert!(o1.backlog_auto_develop);
        assert_eq!(doc.columns.len(), 4);
        assert!(doc.steps.iter().any(|s| s.id == "notify:qa"));
        let move_backlog = doc.steps.iter().find(|s| s.id == "move:backlog").unwrap();
        assert_eq!(move_backlog.with["column"].as_str(), Some("design"));
        assert_eq!(move_backlog.next.as_deref(), Some("agent:design"));

        // 2. Disable auto-analyze while keeping auto-process (moves directly to `design`).
        set_backlog_options(&mut doc, false, true);
        let o2 = derive_options(&doc);
        assert!(!o2.backlog_analyze);
        assert!(o2.backlog_auto_develop);
        assert_eq!(doc.on["backlog"].start_at, "move:backlog");

        // 3. Toggle merge-only without upstream sync.
        set_sync_upstream(&mut doc, false);
        let o3 = derive_options(&doc);
        assert!(!o3.sync_upstream);
        assert!(matches!(
            doc.compile().unwrap().steps.get("deliver:review"),
            Some(WorkflowStep::Deliver {
                sync_upstream: false,
                ..
            })
        ));

        // 4. Switch Review to manual and back to Queue Merge; custom prompt & notify step stay intact.
        set_review_manual(&mut doc, true);
        let o4 = derive_options(&doc);
        assert!(o4.review_manual);
        set_review_manual(&mut doc, false);
        let o5 = derive_options(&doc);
        assert!(!o5.review_manual);
        assert!(doc.steps.iter().any(|s| s.id == "notify:qa"));
        let dev_step = doc.steps.iter().find(|s| s.id == "agent:develop").unwrap();
        assert_eq!(
            dev_step.with["instruction"].as_str(),
            Some("Custom develop prompt")
        );
        doc.compile().unwrap();
    }

    #[test]
    fn backlog_admission_gates_round_trip_and_survive_backlog_toggles() {
        let mut doc = default_document("adm:test", WorkspaceSettings::default());
        set_backlog_options(&mut doc, true, true);
        set_backlog_admission(&mut doc, true, Some(3));
        let o = derive_options(&doc);
        assert!(o.backlog_analyze && o.backlog_auto_develop && o.backlog_wait_for_agent);
        assert_eq!(o.backlog_max_ready, Some(3));
        let flow = doc.compile().unwrap();
        assert_eq!(flow.entries["backlog"], "admit:plan");
        assert!(matches!(
            &flow.steps["admit:plan"],
            WorkflowStep::Admit { agent_slot: false, max_ready: Some(3), next: Some(n) } if n == "agent:plan"
        ));
        assert!(matches!(
            &flow.steps["admit:develop"],
            WorkflowStep::Admit { agent_slot: true, max_ready: None, next: Some(n) } if n == "move:backlog"
        ));
        assert_eq!(flow.steps["agent:plan"].next(), Some("admit:develop"));
        crate::definition::validate_definition(
            &flow,
            &[],
            &["developer".into(), "reviewer".into()],
        )
        .unwrap();

        // Changing K is idempotent (no duplicate gates).
        set_backlog_admission(&mut doc, true, Some(1));
        assert_eq!(doc.steps.iter().filter(|s| s.uses == "admit").count(), 2);
        assert_eq!(derive_options(&doc).backlog_max_ready, Some(1));

        // Toggling analysis keeps the Agent gate; the plan gate needs a planning step.
        set_backlog_options(&mut doc, false, true);
        let o = derive_options(&doc);
        assert!(o.backlog_wait_for_agent && !o.backlog_analyze);
        assert!(!doc.steps.iter().any(|s| s.id == "admit:plan"));
        assert_eq!(doc.compile().unwrap().entries["backlog"], "admit:develop");
        set_backlog_options(&mut doc, true, true);
        assert_eq!(
            derive_options(&doc).backlog_max_ready,
            Some(DEFAULT_BACKLOG_MAX_READY)
        );

        // "Immediately" and "Stay" remove gates and reconnect the flow.
        set_backlog_admission(&mut doc, false, None);
        let o = derive_options(&doc);
        assert!(!o.backlog_wait_for_agent && o.backlog_auto_develop);
        assert!(!doc.steps.iter().any(|s| s.uses == "admit"));
        assert_eq!(
            doc.compile().unwrap().steps["agent:plan"].next(),
            Some("move:backlog")
        );
        set_backlog_admission(&mut doc, true, Some(2));
        set_backlog_options(&mut doc, true, false);
        assert!(!doc.steps.iter().any(|s| s.uses == "admit"));
        doc.compile().unwrap();

        // Choosing "When Agent free" from "Stay" also enables auto-start.
        set_backlog_admission(&mut doc, true, Some(2));
        let o = derive_options(&doc);
        assert!(o.backlog_auto_develop && o.backlog_wait_for_agent);
    }
}
