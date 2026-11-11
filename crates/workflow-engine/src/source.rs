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
            if kind == "approval" {
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
}
