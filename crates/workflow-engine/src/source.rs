//! YAML authoring compiles into the single interpreter. Pure descriptions are
//! shared by native UI and CLI; no server-generated hover prose.
use anyhow::{Result, bail, ensure};
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
}
pub fn parse(source: &str) -> Result<Document> {
    ensure!(
        source.len() <= 2 * 1024 * 1024,
        "Workflow source exceeds 2 MiB"
    );
    let doc: Document = serde_yaml_ng::from_str(source)?;
    ensure!(
        doc.schema == 1,
        "Unsupported workflow schema {}",
        doc.schema
    );
    ensure!(!doc.on.is_empty(), "Workflow needs at least one on rule");
    Ok(doc)
}
impl Document {
    pub fn compile(&self) -> Result<WorkflowDefinition> {
        let mut steps = BTreeMap::new();
        for (i, step) in self.steps.iter().enumerate() {
            ensure!(
                !steps.contains_key(&step.id),
                "Duplicate step ID {}",
                step.id
            );
            let mut args: serde_json::Map<String, Value> = step.with.clone().into_iter().collect();
            let kind = match step.uses.as_str() {
                "agent" => "agent",
                "card.move" => "move",
                "approval" => "approval",
                "deliver" => "deliver",
                "condition" => "condition",
                "notify" => "notify",
                "complete" => "complete",
                other => bail!("Unknown uses: {other}"),
            };
            args.insert("type".into(), json!(kind));
            let next = step
                .next
                .clone()
                .or_else(|| self.steps.get(i + 1).map(|s| s.id.clone()));
            if !matches!(kind, "condition" | "complete") {
                args.insert("next".into(), json!(next));
            }
            if matches!(kind, "approval" | "deliver") {
                args.insert("rework".into(), json!(step.on_changes_requested));
            }
            let compiled: WorkflowStep = serde_json::from_value(Value::Object(args))?;
            steps.insert(step.id.clone(), compiled);
        }
        let entries = self
            .on
            .iter()
            .map(|(id, rule)| (id.clone(), rule.start_at.clone()))
            .collect();
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

pub const DEFAULT_PLAN_PROMPT: &str = "Analyze the Card task, inspect relevant repository files, and write a concise implementation plan with files to change, risks, and validation steps in your report. Do not modify code in this planning step.";
pub const DEFAULT_DEVELOP_PROMPT: &str = "Implement the card task in the supplied working directory. Explain your changes and validation in your report.";
pub const DEFAULT_REVIEW_PROMPT: &str = "Review the work for this card. Inspect the code and earlier results, then write an ordinary Markdown review report. Report findings and any remaining concerns. Do not implement unrelated changes.";
pub const DEFAULT_MERGE_PROMPT: &str = "Perform pre-merge verification for this card. Run tests and checks in the working directory, verify there are no blockers or unintended changes, and summarize readiness in your report.";

pub fn default_step_prompt(step_id: &str, profile: Option<&str>) -> &'static str {
    let lower = step_id.to_ascii_lowercase();
    if lower.contains("plan") || lower.contains("backlog") || lower.contains("design") {
        DEFAULT_PLAN_PROMPT
    } else if lower.contains("merge") || lower.contains("deliver") || lower.contains("queue") {
        DEFAULT_MERGE_PROMPT
    } else if lower.contains("review") || lower.contains("qa") || profile == Some("reviewer") {
        DEFAULT_REVIEW_PROMPT
    } else {
        DEFAULT_DEVELOP_PROMPT
    }
}

pub fn is_default_step_prompt(step_id: &str, profile: Option<&str>, instruction: &str) -> bool {
    let trimmed = instruction.trim();
    trimmed.is_empty()
        || trimmed == default_step_prompt(step_id, profile).trim()
        || trimmed == DEFAULT_PLAN_PROMPT
        || trimmed == DEFAULT_DEVELOP_PROMPT
        || trimmed == DEFAULT_REVIEW_PROMPT
        || trimmed == DEFAULT_MERGE_PROMPT
}

/// Default workflow document for newly created Projects:
/// 1. Backlog: manual intake (no auto-analysis, no auto-move)
/// 2. Develop: automatic `@developer` implementation → moves to Review
/// 3. Review: Queue Merge (`deliver:review`) with `sync_upstream: true` (no `@reviewer` agent step;
///    external untrusted PRs pause at `approval:review` before merging).
pub fn default_document(workflow_id: impl Into<String>, workspace: WorkspaceSettings) -> Document {
    Document {
        schema: 1,
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
                with: BTreeMap::from([
                    ("profile".into(), json!("developer")),
                    ("instruction".into(), json!(DEFAULT_DEVELOP_PROMPT)),
                ]),
                next: Some("move:develop".into()),
                on_changes_requested: None,
            },
            Step {
                id: "move:develop".into(),
                name: Some("Move to Review".into()),
                uses: "card.move".into(),
                with: BTreeMap::from([("column".into(), json!("review"))]),
                next: Some("condition:external_pr".into()),
                on_changes_requested: None,
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
            },
            Step {
                id: "deliver:review".into(),
                name: Some("Queue merge".into()),
                uses: "deliver".into(),
                with: BTreeMap::from([("sync_upstream".into(), json!(true))]),
                next: Some("__end".into()),
                on_changes_requested: Some("agent:develop".into()),
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
            },
            Step {
                id: "__end".into(),
                name: None,
                uses: "complete".into(),
                with: BTreeMap::new(),
                next: None,
                on_changes_requested: None,
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
}

impl Default for WorkflowOptions {
    fn default() -> Self {
        Self {
            backlog_analyze: false,
            backlog_auto_develop: false,
            review_manual: false,
            sync_upstream: true,
            external_pr: "require_review".into(),
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

fn ensure_terminal(doc: &mut Document) -> String {
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
    });
    id
}

fn pin_explicit_nexts(doc: &mut Document) {
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
    }
}

/// Surgically updates Backlog intake behavior (auto-analyze and/or auto-move to next stage)
/// without clobbering custom columns, custom middle stages, or custom prompts.
pub fn set_backlog_options(doc: &mut Document, analyze: bool, auto_develop: bool) {
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
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(DEFAULT_PLAN_PROMPT)
                .to_owned();
            (prof, instr)
        })
        .unwrap_or_else(|| ("developer".into(), DEFAULT_PLAN_PROMPT.into()));

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
            doc.steps.insert(
                0,
                Step {
                    id: plan_step_id.clone(),
                    name: Some("Analyze task".into()),
                    uses: "agent".into(),
                    with: BTreeMap::from([
                        ("profile".into(), json!(plan_profile)),
                        ("instruction".into(), json!(plan_prompt)),
                    ]),
                    next: Some(after_plan),
                    on_changes_requested: None,
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
        assert!(parse(&source.replace("schema: 1", "schema: 2")).is_err());
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
}
