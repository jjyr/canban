//! Generic declarative definition/run viewer, shared with the native Gallery.
use super::*;

pub fn run_view(run: WorkflowRun) -> impl IntoElement {
    let mut content = panel()
        .gap_2()
        .child(label(run.definition.name.clone()))
        .child(muted(format!(
            "Revision {} · {}",
            run.definition_revision,
            run.state.replace('_', " ")
        )));
    for (entry, target) in &run.definition.entries {
        content = content.child(muted(format!("Entry {entry} → {target}")));
    }
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
        order.push(id.to_owned());
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
    let root = run
        .history
        .iter()
        .find_map(|h| h["step"].as_str())
        .or_else(|| run.definition.entries.get("start").map(String::as_str))
        .or_else(|| run.definition.entries.values().next().map(String::as_str));
    if let Some(root) = root {
        walk(root, &run.definition, &mut seen, &mut order);
    }
    for id in run.definition.entries.values() {
        walk(id, &run.definition, &mut seen, &mut order);
    }
    for id in run.definition.steps.keys() {
        walk(id, &run.definition, &mut seen, &mut order);
    }
    for id in &order {
        let step = &run.definition.steps[id];
        let current = run.step.as_ref() == Some(id) && run.active();
        let done = run.outputs.contains_key(id)
            || run
                .history
                .iter()
                .any(|h| h["step"] == *id && h["event"] == "completed");
        let kind = match step {
            WorkflowStep::Agent { profile, .. } => {
                format!("Agent · @{}", profile.as_deref().unwrap_or("default"))
            }
            WorkflowStep::Move { column, .. } => format!("Move to {column}"),
            WorkflowStep::Approval { title, .. } => title.clone(),
            WorkflowStep::Deliver { target_branch, .. } => {
                if let Some(branch) = target_branch {
                    format!("Auto-merge into {branch}")
                } else {
                    "Auto-merge queue".into()
                }
            }
            WorkflowStep::Condition {
                then, otherwise, ..
            } => format!("Condition → {then} / {otherwise}"),
            WorkflowStep::Notify { message, .. } => format!("Notify · {message}"),
            WorkflowStep::Complete => "Complete".into(),
        };
        let marker = if current {
            "●"
        } else if done {
            "✓"
        } else {
            "·"
        };
        let mut row = view()
            .gap_1()
            .child(label(format!("{marker} {id} · {kind}")));
        if let WorkflowStep::Agent { instruction, .. } = step {
            row = row.child(muted(instruction.clone()));
        }
        if let Some(next) = step.next() {
            row = row.child(muted(format!("Next → {next}")));
        }
        if let WorkflowStep::Approval {
            rework: Some(to), ..
        }
        | WorkflowStep::Deliver {
            rework: Some(to), ..
        } = step
        {
            row = row.child(muted(format!("Changes requested → {to}")));
        }
        if let Some(result) = run.outputs.get(id).and_then(|r| r["message"].as_str()) {
            row = row.child(muted(result.to_owned()));
        }
        content = content.child(row);
    }
    if let Some(error) = run.error {
        content = content.child(label(error).text_color(DANGER));
    }
    content
}
