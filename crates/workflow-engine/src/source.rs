//! YAML authoring compiles into the single interpreter. Pure descriptions are
//! shared by native UI and CLI; no server-generated hover prose.
use anyhow::{Result, bail, ensure};
use canban_types::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
                "deliver" | "card.deliver" => "deliver",
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
/// Convert existing definitions once; original user-authored YAML is never regenerated.
pub fn export(flow: &WorkflowDefinition, rules: BTreeMap<String, WorkflowRule>) -> Result<String> {
    let terminal = std::iter::successors(Some("__end".to_string()), |s| Some(format!("{s}_")))
        .find(|s| !flow.steps.contains_key(s))
        .unwrap();
    let steps = flow
        .steps
        .iter()
        .map(|(id, s)| {
            let mut value = serde_json::to_value(s).unwrap();
            let kind = value.as_object_mut().unwrap().remove("type").unwrap();
            let next = value
                .as_object_mut()
                .unwrap()
                .remove("next")
                .and_then(|v| v.as_str().map(str::to_owned));
            let rework = value
                .as_object_mut()
                .unwrap()
                .remove("rework")
                .and_then(|v| v.as_str().map(str::to_owned));
            // Explicit terminal nodes prevent alphabetical export from implying a successor.
            let next = if !matches!(s, WorkflowStep::Complete | WorkflowStep::Condition { .. }) {
                Some(next.unwrap_or_else(|| terminal.clone()))
            } else {
                None
            };
            Step {
                id: id.clone(),
                name: None,
                uses: if kind == "move" {
                    "card.move".into()
                } else {
                    kind.as_str().unwrap().into()
                },
                with: serde_json::from_value(value).unwrap(),
                next,
                on_changes_requested: rework,
            }
        })
        .chain(std::iter::once(Step {
            id: terminal.clone(),
            name: None,
            uses: "complete".into(),
            with: BTreeMap::new(),
            next: None,
            on_changes_requested: None,
        }))
        .collect();
    Ok(serde_yaml_ng::to_string(&Document {
        schema: 1,
        id: flow.id.clone(),
        name: flow.name.clone(),
        enabled: true,
        columns: flow.columns.clone(),
        on: rules,
        steps,
        workspace: flow.workspace.clone(),
        max_steps: flow.max_steps,
    })?)
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinPolicy {
    /// "manual" | "confirm" | "auto"
    pub planning: String,
    /// "human_review" | "auto_merge"
    pub delivery: String,
    /// "same" | "require_review" | "manual_only"
    pub external_pr: String,
}

impl Default for BuiltinPolicy {
    fn default() -> Self {
        Self {
            planning: "manual".into(),
            delivery: "human_review".into(),
            external_pr: "require_review".into(),
        }
    }
}

pub fn detect_policy(doc: &Document) -> Option<BuiltinPolicy> {
    let has_deliver = doc
        .steps
        .iter()
        .any(|s| matches!(s.uses.as_str(), "deliver" | "card.deliver"));
    let has_develop = doc
        .steps
        .iter()
        .any(|s| s.id == "agent:develop" || s.id == "develop");
    let has_review_or_merge = doc.steps.iter().any(|s| {
        matches!(
            s.id.as_str(),
            "agent:review" | "review" | "agent:merge" | "merge_verify" | "deliver:review" | "deliver"
        )
    });
    if !has_develop && !has_review_or_merge && !has_deliver {
        return None;
    }
    // Check if there are custom steps outside the standard policy set
    let known_ids = [
        "agent:plan",
        "approval:plan",
        "move:backlog",
        "agent:develop",
        "develop",
        "move:develop",
        "condition:external_pr",
        "agent:review",
        "review",
        "approval:review",
        "agent:merge",
        "merge_verify",
        "deliver:review",
        "deliver",
        "__end",
        "complete",
    ];
    if doc
        .steps
        .iter()
        .any(|s| !known_ids.contains(&s.id.as_str()) && !s.id.starts_with("__end"))
    {
        return None;
    }
    let planning = if let Some(rule) = doc.on.get("backlog")
        && rule.enabled
    {
        if doc.steps.iter().any(|s| s.id == "approval:plan") {
            "confirm"
        } else {
            "auto"
        }
    } else {
        "manual"
    };
    let delivery = if has_deliver {
        "auto_merge"
    } else {
        "human_review"
    };
    let external_pr = if doc
        .on
        .get("develop")
        .is_some_and(|r| r.selector.source_kind.as_deref() == Some("issue"))
    {
        "manual_only"
    } else if doc.steps.iter().any(|s| s.id == "condition:external_pr") {
        "require_review"
    } else {
        "same"
    };
    Some(BuiltinPolicy {
        planning: planning.into(),
        delivery: delivery.into(),
        external_pr: external_pr.into(),
    })
}

pub fn apply_policy(doc: &mut Document, policy: &BuiltinPolicy) {
    let find_agent = |doc: &Document, ids: &[&str], default_profile: &str, default_prompt: &str| {
        for id in ids {
            if let Some(step) = doc.steps.iter().find(|s| s.id == *id && s.uses == "agent") {
                let profile = step
                    .with
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .unwrap_or(default_profile)
                    .to_owned();
                let instruction = step
                    .with
                    .get("instruction")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(default_prompt)
                    .to_owned();
                return (profile, instruction);
            }
        }
        (default_profile.to_owned(), default_prompt.to_owned())
    };
    let (plan_profile, plan_prompt) = find_agent(
        doc,
        &["agent:plan", "plan"],
        "developer",
        DEFAULT_PLAN_PROMPT,
    );
    let (dev_profile, dev_prompt) = find_agent(
        doc,
        &["agent:develop", "develop"],
        "developer",
        DEFAULT_DEVELOP_PROMPT,
    );
    let (rev_profile, rev_prompt) = find_agent(
        doc,
        &["agent:review", "review"],
        "reviewer",
        DEFAULT_REVIEW_PROMPT,
    );
    let (merge_profile, merge_prompt) = find_agent(
        doc,
        &["agent:merge", "merge_verify"],
        "reviewer",
        DEFAULT_MERGE_PROMPT,
    );

    if doc.columns.is_empty() {
        doc.columns = vec![
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
        ];
    }

    let mut on = BTreeMap::new();
    let mut steps = Vec::new();
    let source_filter = if policy.external_pr == "manual_only" {
        Some("issue".to_string())
    } else {
        None
    };

    if policy.planning != "manual" {
        on.insert(
            "backlog".into(),
            WorkflowRule {
                event: "card.column_entered".into(),
                start_at: "agent:plan".into(),
                selector: EventSelector {
                    column: Some("backlog".into()),
                    source_kind: Some("issue".into()),
                    ..Default::default()
                },
                enabled: true,
            },
        );
        let after_plan = if policy.planning == "confirm" {
            "approval:plan"
        } else {
            "move:backlog"
        };
        steps.push(Step {
            id: "agent:plan".into(),
            name: Some("Plan task".into()),
            uses: "agent".into(),
            with: BTreeMap::from([
                ("profile".into(), json!(plan_profile)),
                ("instruction".into(), json!(plan_prompt)),
            ]),
            next: Some(after_plan.into()),
            on_changes_requested: None,
        });
        if policy.planning == "confirm" {
            steps.push(Step {
                id: "approval:plan".into(),
                name: Some("Confirm plan".into()),
                uses: "approval".into(),
                with: BTreeMap::from([(
                    "title".into(),
                    json!("Plan ready; confirm to start development"),
                )]),
                next: Some("move:backlog".into()),
                on_changes_requested: Some("agent:plan".into()),
            });
        }
        steps.push(Step {
            id: "move:backlog".into(),
            name: Some("Move to Develop".into()),
            uses: "card.move".into(),
            with: BTreeMap::from([("column".into(), json!("develop"))]),
            next: Some("agent:develop".into()),
            on_changes_requested: None,
        });
    }

    on.insert(
        "develop".into(),
        WorkflowRule {
            event: "card.column_entered".into(),
            start_at: "agent:develop".into(),
            selector: EventSelector {
                column: Some("develop".into()),
                source_kind: source_filter.clone(),
                ..Default::default()
            },
            enabled: true,
        },
    );

    steps.push(Step {
        id: "agent:develop".into(),
        name: Some("Implement".into()),
        uses: "agent".into(),
        with: BTreeMap::from([
            ("profile".into(), json!(dev_profile)),
            ("instruction".into(), json!(dev_prompt)),
        ]),
        next: Some("move:develop".into()),
        on_changes_requested: None,
    });

    let review_entry = if policy.delivery == "auto_merge" {
        if policy.external_pr == "require_review" {
            "condition:external_pr"
        } else {
            "agent:merge"
        }
    } else {
        "agent:review"
    };

    steps.push(Step {
        id: "move:develop".into(),
        name: Some("Move to Review".into()),
        uses: "card.move".into(),
        with: BTreeMap::from([("column".into(), json!("review"))]),
        next: Some(review_entry.into()),
        on_changes_requested: None,
    });

    on.insert(
        "review".into(),
        WorkflowRule {
            event: "card.column_entered".into(),
            start_at: review_entry.into(),
            selector: EventSelector {
                column: Some("review".into()),
                source_kind: source_filter,
                ..Default::default()
            },
            enabled: true,
        },
    );

    if policy.delivery == "auto_merge" {
        if policy.external_pr == "require_review" {
            steps.push(Step {
                id: "condition:external_pr".into(),
                name: Some("External PR check".into()),
                uses: "condition".into(),
                with: BTreeMap::from([
                    ("pointer".into(), json!("/card/source_trust")),
                    ("equals".into(), json!("external_pr")),
                    ("then".into(), json!("agent:review")),
                    ("otherwise".into(), json!("agent:merge")),
                ]),
                next: None,
                on_changes_requested: None,
            });
        }
        steps.push(Step {
            id: "agent:merge".into(),
            name: Some("Pre-merge check".into()),
            uses: "agent".into(),
            with: BTreeMap::from([
                ("profile".into(), json!(merge_profile)),
                ("instruction".into(), json!(merge_prompt)),
            ]),
            next: Some("deliver:review".into()),
            on_changes_requested: None,
        });
        steps.push(Step {
            id: "deliver:review".into(),
            name: Some("Auto-merge queue".into()),
            uses: "deliver".into(),
            with: BTreeMap::new(),
            next: Some("__end".into()),
            on_changes_requested: Some("agent:develop".into()),
        });
        if policy.external_pr == "require_review" {
            steps.push(Step {
                id: "agent:review".into(),
                name: Some("Review external PR".into()),
                uses: "agent".into(),
                with: BTreeMap::from([
                    ("profile".into(), json!(rev_profile)),
                    ("instruction".into(), json!(rev_prompt)),
                ]),
                next: Some("approval:review".into()),
                on_changes_requested: None,
            });
            steps.push(Step {
                id: "approval:review".into(),
                name: Some("Human approval".into()),
                uses: "approval".into(),
                with: BTreeMap::from([(
                    "title".into(),
                    json!("External PR reviewed; human approval is required before merging"),
                )]),
                next: Some("__end".into()),
                on_changes_requested: Some("agent:develop".into()),
            });
        }
    } else {
        steps.push(Step {
            id: "agent:review".into(),
            name: Some("Review".into()),
            uses: "agent".into(),
            with: BTreeMap::from([
                ("profile".into(), json!(rev_profile)),
                ("instruction".into(), json!(rev_prompt)),
            ]),
            next: Some("approval:review".into()),
            on_changes_requested: None,
        });
        steps.push(Step {
            id: "approval:review".into(),
            name: Some("Human approval".into()),
            uses: "approval".into(),
            with: BTreeMap::from([(
                "title".into(),
                json!("Agent report ready; human review is required before continuing"),
            )]),
            next: Some("__end".into()),
            on_changes_requested: Some("agent:develop".into()),
        });
    }

    steps.push(Step {
        id: "__end".into(),
        name: None,
        uses: "complete".into(),
        with: BTreeMap::new(),
        next: None,
        on_changes_requested: None,
    });

    doc.on = on;
    doc.steps = steps;
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
                    return Some("Auto-merge · Ext Review".into());
                }
                WorkflowStep::Deliver { .. } => {
                    return Some("Auto-merge".into());
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
    fn builtin_policy_preserves_custom_prompts_and_columns() {
        let mut doc = parse("schema: 1\nid: dev\nname: Development\ncolumns:\n  - id: backlog\n    name: Backlog\n  - id: design\n    name: Design\n  - id: develop\n    name: Develop\n  - id: review\n    name: Review\non:\n  develop:\n    event: card.column_entered\n    start_at: agent:develop\n    match:\n      column: develop\nsteps:\n  - id: agent:develop\n    uses: agent\n    with:\n      profile: developer\n      instruction: Custom develop prompt\n").unwrap();
        let policy = BuiltinPolicy {
            planning: "confirm".into(),
            delivery: "auto_merge".into(),
            external_pr: "require_review".into(),
        };
        apply_policy(&mut doc, &policy);
        assert_eq!(doc.columns.len(), 4);
        assert_eq!(detect_policy(&doc), Some(policy));
        let dev_step = doc.steps.iter().find(|s| s.id == "agent:develop").unwrap();
        assert_eq!(
            dev_step.with["instruction"].as_str(),
            Some("Custom develop prompt")
        );
        let compiled = doc.compile().unwrap();
        assert!(matches!(
            compiled.steps.get("deliver:review"),
            Some(WorkflowStep::Deliver { .. })
        ));
        assert!(matches!(
            compiled.steps.get("condition:external_pr"),
            Some(WorkflowStep::Condition { pointer, equals, .. })
                if pointer == "/card/source_trust" && equals == "external_pr"
        ));
    }
}
