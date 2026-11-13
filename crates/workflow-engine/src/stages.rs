//! Stage-oriented structural editing over YAML workflow Documents.
//!
//! Studio presents each Board column as a Stage: an ordered pipeline of steps plus
//! an exit (`card.move`). These pure functions edit the flat YAML graph by local
//! edge splicing, so branches (`condition`) and rework edges survive edits.
use crate::source::{
    Document, NEXT_COLUMN, STAGE_ENTRY, Step, ensure_terminal, pin_explicit_nexts, step_stages,
};
use anyhow::{Context, Result, bail, ensure};
use canban_types::{EventSelector, WorkflowColumn, WorkflowRule};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Where a Stage sends the Card after its pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Relative: the next Board column (`column: $next`).
    Next,
    /// A fixed column.
    Column(String),
    /// Stay in this column; the run ends.
    Stay,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageView {
    pub column: String,
    pub name: String,
    /// Pipeline steps in display order (excludes the exit move and terminals).
    pub steps: Vec<String>,
    pub exit: Exit,
    /// The `card.move` step implementing the exit.
    pub exit_step: Option<String>,
    /// Column entry rule key, when the Stage runs automatically on entry.
    pub entry_rule: Option<String>,
}

fn index(doc: &Document, id: &str) -> Option<usize> {
    doc.steps.iter().position(|s| s.id == id)
}

/// Effective successors used for display ordering.
fn successors(doc: &Document, idx: usize) -> Vec<String> {
    let step = &doc.steps[idx];
    if step.uses == "condition" {
        ["then", "otherwise"]
            .iter()
            .filter_map(|k| step.with.get(*k).and_then(|v| v.as_str()).map(str::to_owned))
            .collect()
    } else {
        doc.raw_next(idx)
            .filter(|n| n != STAGE_ENTRY)
            .into_iter()
            .collect()
    }
}

/// Stage views in Board column order.
pub fn stages(doc: &Document) -> Vec<StageView> {
    let stage_of = step_stages(doc);
    doc.columns
        .iter()
        .map(|col| {
            let entry = doc.column_entry(&col.id);
            let mut ordered = Vec::new();
            let mut seen = BTreeSet::new();
            let mut stack: Vec<String> = entry.iter().map(|(_, s)| s.clone()).collect();
            // Depth-first in YAML successor order; only this Stage's steps are listed.
            while let Some(id) = stack.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let Some(i) = index(doc, &id) else { continue };
                if stage_of.get(&id) != Some(&col.id) {
                    continue;
                }
                ordered.push(id.clone());
                let mut next = successors(doc, i);
                next.reverse();
                stack.extend(next);
            }
            for s in &doc.steps {
                if stage_of.get(&s.id) == Some(&col.id) && !ordered.contains(&s.id) {
                    ordered.push(s.id.clone());
                }
            }
            ordered.retain(|id| index(doc, id).is_some_and(|i| doc.steps[i].uses != "complete"));
            let exit_step = ordered
                .iter()
                .find(|id| doc.steps[index(doc, id).unwrap()].uses == "card.move")
                .cloned();
            let exit = match &exit_step {
                None => Exit::Stay,
                Some(id) => {
                    let s = &doc.steps[index(doc, id).unwrap()];
                    match s.with.get("column").and_then(|v| v.as_str()) {
                        Some(NEXT_COLUMN) | None => Exit::Next,
                        Some(c) => Exit::Column(c.to_owned()),
                    }
                }
            };
            ordered.retain(|id| Some(id) != exit_step.as_ref());
            StageView {
                column: col.id.clone(),
                name: col.name.clone(),
                steps: ordered,
                exit,
                exit_step,
                entry_rule: entry.map(|(k, _)| k),
            }
        })
        .collect()
}

/// Makes structure explicit before editing: pins `next`, writes `stage` on every
/// step, and rewrites moves to the following column into `$next` / `$entry`.
pub fn normalize(doc: &mut Document, fallback_columns: &[WorkflowColumn]) {
    if doc.columns.is_empty() {
        doc.columns = fallback_columns.to_vec();
    }
    pin_explicit_nexts(doc);
    let stage_of = step_stages(doc);
    for i in 0..doc.steps.len() {
        let id = doc.steps[i].id.clone();
        if doc.steps[i].stage.is_none() && doc.steps[i].uses != "complete" {
            doc.steps[i].stage = stage_of.get(&id).cloned();
        }
        if doc.steps[i].uses != "card.move" {
            continue;
        }
        let stage = doc.steps[i].stage.clone();
        let target = doc.steps[i]
            .with
            .get("column")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let Some(target) = target else { continue };
        if target != NEXT_COLUMN
            && stage
                .as_deref()
                .and_then(|s| doc.next_column(s))
                .is_some_and(|n| n == target)
        {
            doc.steps[i].with.insert("column".into(), json!(NEXT_COLUMN));
        }
        let resolved = doc.move_target(&doc.steps[i], stage.as_deref());
        if let Some(col) = resolved {
            let entry = doc.column_entry(&col).map(|(_, s)| s);
            if doc.steps[i].next.is_some() && doc.steps[i].next == entry {
                doc.steps[i].next = Some(STAGE_ENTRY.into());
            }
        }
    }
}

fn unique_id(doc: &Document, base: &str) -> String {
    if index(doc, base).is_none() {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| index(doc, c).is_none())
        .unwrap()
}

/// Replaces every reference to `from` with `to`.
fn retarget(doc: &mut Document, from: &str, to: &str) {
    for rule in doc.on.values_mut() {
        if rule.start_at == from {
            rule.start_at = to.to_owned();
        }
    }
    for step in &mut doc.steps {
        if step.next.as_deref() == Some(from) {
            step.next = Some(to.to_owned());
        }
        if step.on_changes_requested.as_deref() == Some(from) {
            step.on_changes_requested = Some(to.to_owned());
        }
        for key in ["then", "otherwise"] {
            if step.with.get(key).and_then(|v| v.as_str()) == Some(from) {
                step.with.insert(key.into(), json!(to));
            }
        }
    }
}

/// Successor that replaces `id` when it is unlinked (terminal when none).
fn bypass(doc: &mut Document, id: &str) -> String {
    let i = index(doc, id).unwrap();
    let step = &doc.steps[i];
    let to = if step.uses == "condition" {
        step.with
            .get("otherwise")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    } else {
        step.next.clone().filter(|n| n != STAGE_ENTRY)
    };
    to.unwrap_or_else(|| ensure_terminal(doc))
}

fn ensure_entry_rule(doc: &mut Document, column: &str, start: &str) {
    if let Some((key, _)) = doc.column_entry(column) {
        doc.on.get_mut(&key).unwrap().start_at = start.to_owned();
        return;
    }
    let key = if doc.on.contains_key(column) {
        format!("{column}-entry")
    } else {
        column.to_owned()
    };
    doc.on.insert(
        key,
        WorkflowRule {
            event: "card.column_entered".into(),
            start_at: start.to_owned(),
            selector: EventSelector {
                column: Some(column.to_owned()),
                ..Default::default()
            },
            enabled: true,
        },
    );
}

fn view(doc: &Document, column: &str) -> Result<StageView> {
    stages(doc)
        .into_iter()
        .find(|s| s.column == column)
        .with_context(|| format!("Unknown stage {column}"))
}

/// Inserts `step` into `column`'s pipeline after `after` (`None` = first).
/// Returns the (possibly de-duplicated) step ID.
pub fn insert_step(
    doc: &mut Document,
    column: &str,
    after: Option<&str>,
    mut step: Step,
) -> Result<String> {
    pin_explicit_nexts(doc);
    let stage = view(doc, column)?;
    step.id = unique_id(doc, &step.id);
    step.stage = Some(column.to_owned());
    let id = step.id.clone();
    match after {
        Some(a) => {
            let i = index(doc, a).with_context(|| format!("Unknown step {a}"))?;
            ensure!(
                doc.steps[i].uses != "condition",
                "Insert into a condition branch instead of after the condition"
            );
            ensure!(
                doc.steps[i].uses != "card.move",
                "Steps go before the Stage exit"
            );
            step.next = Some(bypass(doc, a));
            doc.steps[i].next = Some(id.clone());
            doc.steps.insert(i + 1, step);
        }
        None => {
            let old_start = stage
                .steps
                .first()
                .cloned()
                .or_else(|| stage.exit_step.clone());
            step.next = Some(old_start.clone().unwrap_or_else(|| ensure_terminal(doc)));
            if let Some(old) = &old_start {
                // Predecessors outside this Stage (explicit moves into it) now enter here.
                let stage_of = step_stages(doc);
                for s in &mut doc.steps {
                    if s.next.as_deref() == Some(old.as_str())
                        && stage_of.get(&s.id).map(String::as_str) != Some(column)
                    {
                        s.next = Some(id.clone());
                    }
                }
            }
            let at = old_start
                .and_then(|o| index(doc, &o))
                .unwrap_or(doc.steps.len());
            doc.steps.insert(at, step);
            ensure_entry_rule(doc, column, &id);
        }
    }
    Ok(id)
}

/// Unlinks and deletes a step; predecessors continue at its successor.
pub fn remove_step(doc: &mut Document, id: &str) -> Result<Step> {
    pin_explicit_nexts(doc);
    index(doc, id).with_context(|| format!("Unknown step {id}"))?;
    let to = bypass(doc, id);
    retarget(doc, id, &to);
    let i = index(doc, id).unwrap();
    Ok(doc.steps.remove(i))
}

/// Moves a step to `column` after `after` (`None` = first), keeping its settings.
/// Only the linear chain is spliced; rework and condition-branch references keep
/// pointing at the moved step.
pub fn move_step(doc: &mut Document, id: &str, column: &str, after: Option<&str>) -> Result<()> {
    ensure!(after != Some(id), "A step cannot follow itself");
    pin_explicit_nexts(doc);
    let i = index(doc, id).with_context(|| format!("Unknown step {id}"))?;
    ensure!(
        doc.steps[i].uses != "condition",
        "Move a condition in YAML Source; its branches would be split"
    );
    let to = bypass(doc, id);
    for rule in doc.on.values_mut() {
        if rule.start_at == id {
            rule.start_at = to.clone();
        }
    }
    for step in &mut doc.steps {
        if step.next.as_deref() == Some(id) {
            step.next = Some(to.clone());
        }
    }
    let i = index(doc, id).unwrap();
    let mut step = doc.steps.remove(i);
    step.next = None;
    // Insert under a temporary ID so it cannot collide, then restore the original.
    let placeholder = unique_id(doc, &format!("{id}~moving"));
    let original = std::mem::replace(&mut step.id, placeholder.clone());
    let new_id = insert_step(doc, column, after, step)?;
    retarget_links(doc, &new_id, &original);
    let i = index(doc, &new_id).unwrap();
    doc.steps[i].id = original;
    Ok(())
}

/// Linear references only (`next`, rule starts).
fn retarget_links(doc: &mut Document, from: &str, to: &str) {
    for rule in doc.on.values_mut() {
        if rule.start_at == from {
            rule.start_at = to.to_owned();
        }
    }
    for step in &mut doc.steps {
        if step.next.as_deref() == Some(from) {
            step.next = Some(to.to_owned());
        }
    }
}

pub fn set_step_enabled(doc: &mut Document, id: &str, enabled: bool) -> Result<()> {
    let i = index(doc, id).with_context(|| format!("Unknown step {id}"))?;
    doc.steps[i].enabled = enabled;
    Ok(())
}

/// Sets (`Some`) or clears (`None`) a `with` argument.
pub fn set_step_with(doc: &mut Document, id: &str, key: &str, value: Option<Value>) -> Result<()> {
    let i = index(doc, id).with_context(|| format!("Unknown step {id}"))?;
    match value {
        Some(v) => doc.steps[i].with.insert(key.into(), v),
        None => doc.steps[i].with.remove(key),
    };
    Ok(())
}

pub fn set_step_name(doc: &mut Document, id: &str, name: Option<String>) -> Result<()> {
    let i = index(doc, id).with_context(|| format!("Unknown step {id}"))?;
    doc.steps[i].name = name.filter(|n| !n.trim().is_empty());
    Ok(())
}

pub fn set_rework(doc: &mut Document, id: &str, target: Option<String>) -> Result<()> {
    let i = index(doc, id).with_context(|| format!("Unknown step {id}"))?;
    ensure!(
        matches!(doc.steps[i].uses.as_str(), "approval" | "deliver"),
        "Only approval and deliver steps have a rework path"
    );
    if let Some(t) = &target {
        ensure!(index(doc, t).is_some(), "Unknown rework target {t}");
    }
    doc.steps[i].on_changes_requested = target;
    Ok(())
}

/// Changes where the Stage sends the Card after its pipeline.
pub fn set_exit(doc: &mut Document, column: &str, exit: Exit) -> Result<()> {
    pin_explicit_nexts(doc);
    let stage = view(doc, column)?;
    if let Exit::Column(c) = &exit {
        ensure!(doc.columns.iter().any(|x| &x.id == c), "Unknown column {c}");
        ensure!(c != column, "A Stage cannot exit into itself");
    }
    if exit == Exit::Next {
        ensure!(
            doc.next_column(column).is_some(),
            "{column} is the last column; choose Stay"
        );
    }
    let target = match &exit {
        Exit::Next => NEXT_COLUMN.to_owned(),
        Exit::Column(c) => c.clone(),
        Exit::Stay => {
            if let Some(m) = stage.exit_step {
                remove_step(doc, &m)?;
            }
            return Ok(());
        }
    };
    if let Some(m) = stage.exit_step {
        let i = index(doc, &m).unwrap();
        doc.steps[i].with.insert("column".into(), json!(target));
        doc.steps[i].next = Some(STAGE_ENTRY.into());
        return Ok(());
    }
    let terminal = ensure_terminal(doc);
    let id = unique_id(doc, &format!("move:{column}"));
    let step = Step {
        id: id.clone(),
        name: None,
        uses: "card.move".into(),
        with: BTreeMap::from([("column".into(), json!(target))]),
        next: Some(STAGE_ENTRY.into()),
        on_changes_requested: None,
        stage: Some(column.to_owned()),
        enabled: true,
    };
    // Tails: this Stage's steps that end the run.
    let stage_of = step_stages(doc);
    let tails: Vec<usize> = (0..doc.steps.len())
        .filter(|&i| {
            let s = &doc.steps[i];
            stage_of.get(&s.id).map(String::as_str) == Some(column)
                && !matches!(s.uses.as_str(), "condition" | "complete" | "card.move")
                && s.next.as_deref().is_none_or(|n| {
                    index(doc, n).is_some_and(|j| doc.steps[j].uses == "complete")
                })
        })
        .collect();
    let at = tails.last().map_or(doc.steps.len(), |t| t + 1);
    for t in tails {
        doc.steps[t].next = Some(id.clone());
    }
    let has_steps = !stage.steps.is_empty();
    doc.steps.insert(at.min(doc.steps.len()), step);
    if !has_steps {
        ensure_entry_rule(doc, column, &id);
    }
    let _ = terminal;
    Ok(())
}

fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "stage".into() } else { s }
}

/// Appends a new column (Stage) before `before` (`None` = at the end). Returns its ID.
pub fn add_column(doc: &mut Document, name: &str, before: Option<&str>) -> Result<String> {
    ensure!(!name.trim().is_empty(), "Stage name is required");
    let base = slug(name);
    let id = if doc.columns.iter().any(|c| c.id == base) {
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|c| !doc.columns.iter().any(|x| &x.id == c))
            .unwrap()
    } else {
        base
    };
    let at = before
        .and_then(|b| doc.columns.iter().position(|c| c.id == b))
        .unwrap_or(doc.columns.len());
    doc.columns.insert(
        at,
        WorkflowColumn {
            id: id.clone(),
            name: name.trim().to_owned(),
        },
    );
    Ok(id)
}

/// Removes a column with its Stage steps and entry rules. Fixed exits into it are
/// removed (those Stages then stay); relative exits follow the new column order.
/// Active Cards in the column return to the first column on publish.
pub fn remove_column(doc: &mut Document, column: &str) -> Result<()> {
    ensure!(doc.columns.len() > 1, "A Board needs at least one column");
    ensure!(
        doc.columns.iter().any(|c| c.id == column),
        "Unknown column {column}"
    );
    pin_explicit_nexts(doc);
    let stage_of = step_stages(doc);
    let doomed: Vec<String> = doc
        .steps
        .iter()
        .filter(|s| stage_of.get(&s.id).map(String::as_str) == Some(column) && s.uses != "complete")
        .map(|s| s.id.clone())
        .collect();
    for id in doomed {
        remove_step(doc, &id)?;
    }
    let fixed: Vec<String> = doc
        .steps
        .iter()
        .filter(|s| {
            s.uses == "card.move" && s.with.get("column").and_then(|v| v.as_str()) == Some(column)
        })
        .map(|s| s.id.clone())
        .collect();
    for id in fixed {
        remove_step(doc, &id)?;
    }
    doc.on.retain(|_, r| r.selector.column.as_deref() != Some(column));
    doc.columns.retain(|c| c.id != column);
    // A relative exit from the new last column has nowhere to go.
    let last = doc.columns.last().map(|c| c.id.clone());
    if let Some(last) = last
        && let Some(m) = view(doc, &last)?.exit_step
        && doc.steps[index(doc, &m).unwrap()].with.get("column").and_then(|v| v.as_str())
            == Some(NEXT_COLUMN)
    {
        remove_step(doc, &m)?;
    }
    if doc.on.is_empty() {
        bail!("Removing {column} would leave the workflow without triggers");
    }
    Ok(())
}

pub fn rename_column(doc: &mut Document, column: &str, name: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "Stage name is required");
    let c = doc
        .columns
        .iter_mut()
        .find(|c| c.id == column)
        .with_context(|| format!("Unknown column {column}"))?;
    c.name = name.trim().to_owned();
    Ok(())
}

/// Moves a column to position `to` (clamped).
pub fn move_column(doc: &mut Document, column: &str, to: usize) -> Result<()> {
    let from = doc
        .columns
        .iter()
        .position(|c| c.id == column)
        .with_context(|| format!("Unknown column {column}"))?;
    let col = doc.columns.remove(from);
    doc.columns.insert(to.min(doc.columns.len()), col);
    // Relative exits from the new last column would point nowhere.
    if let Some(last) = doc.columns.last().map(|c| c.id.clone())
        && let Some(m) = view(doc, &last)?.exit_step
        && doc.steps[index(doc, &m).unwrap()].with.get("column").and_then(|v| v.as_str())
            == Some(NEXT_COLUMN)
    {
        remove_step(doc, &m)?;
    }
    Ok(())
}

/// Step templates offered by "Add step".
pub fn template(kind: &str) -> Option<Step> {
    let (id, name, uses, with): (&str, &str, &str, Vec<(&str, Value)>) = match kind {
        "agent" => (
            "agent",
            "Agent",
            "agent",
            vec![("instruction", json!(crate::source::DEFAULT_DEVELOP_PROMPT))],
        ),
        "plan" => (
            "agent:plan",
            "Plan task",
            "agent",
            vec![
                ("profile", json!("developer")),
                ("instruction", json!(crate::source::DEFAULT_PLAN_PROMPT)),
            ],
        ),
        "review" => (
            "agent:review",
            "Review",
            "agent",
            vec![
                ("profile", json!("reviewer")),
                ("instruction", json!(crate::source::DEFAULT_REVIEW_PROMPT)),
            ],
        ),
        "approval" => (
            "approval",
            "Human approval",
            "approval",
            vec![("title", json!("Human approval is required before continuing"))],
        ),
        "deliver" => ("deliver", "Queue merge", "deliver", vec![]),
        "wait_agent" => ("admit:agent", "Wait for free Agent", "admit", vec![]),
        "plan_ahead" => (
            "admit:ready",
            "Plan-ahead limit",
            "admit",
            vec![
                ("agent_slot", json!(false)),
                ("max_ready", json!(crate::source::DEFAULT_BACKLOG_MAX_READY)),
            ],
        ),
        "notify" => (
            "notify",
            "Notify",
            "notify",
            vec![("message", json!("Card reached this step"))],
        ),
        _ => return None,
    };
    Some(Step {
        id: id.into(),
        name: Some(name.into()),
        uses: uses.into(),
        with: with.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        next: None,
        on_changes_requested: None,
        stage: None,
        enabled: true,
    })
}

/// Template kinds in "Add step" order.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("agent", "Agent"),
    ("plan", "Plan (Agent)"),
    ("review", "Review (Agent)"),
    ("approval", "Human approval"),
    ("deliver", "Queue merge"),
    ("wait_agent", "Wait for free Agent"),
    ("plan_ahead", "Plan-ahead limit"),
    ("notify", "Notify"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::default_document;
    use canban_types::{WorkflowStep, WorkspaceSettings};

    fn doc() -> Document {
        let mut d = default_document("st:test", WorkspaceSettings::default());
        normalize(&mut d, &[]);
        d
    }
    fn compiles(d: &Document) -> canban_types::WorkflowDefinition {
        let flow = d.compile().unwrap();
        crate::definition::validate_definition(
            &flow,
            &[],
            &["developer".into(), "reviewer".into()],
        )
        .unwrap();
        flow
    }

    #[test]
    fn default_document_maps_to_stages_with_relative_exits() {
        let d = doc();
        let s = stages(&d);
        assert_eq!(
            s.iter().map(|s| s.column.as_str()).collect::<Vec<_>>(),
            ["backlog", "develop", "review"]
        );
        assert!(s[0].steps.is_empty() && s[0].exit == Exit::Stay);
        assert_eq!(s[1].steps, ["agent:develop"]);
        assert_eq!(s[1].exit, Exit::Next);
        assert_eq!(
            s[2].steps,
            ["condition:external_pr", "approval:review", "deliver:review"]
        );
        let flow = compiles(&d);
        // `$next` / `$entry` resolve to the concrete Review column and its entry.
        assert!(matches!(
            &flow.steps["move:develop"],
            WorkflowStep::Move { column, next: Some(n) } if column == "review" && n == "condition:external_pr"
        ));
    }

    #[test]
    fn insert_move_remove_and_disable_keep_the_graph_valid() {
        let mut d = doc();
        let plan = insert_step(&mut d, "backlog", None, template("plan").unwrap()).unwrap();
        set_exit(&mut d, "backlog", Exit::Next).unwrap();
        let s = stages(&d);
        assert_eq!(s[0].steps, std::slice::from_ref(&plan));
        assert_eq!(s[0].exit, Exit::Next);
        let flow = compiles(&d);
        assert_eq!(flow.entries["backlog"], plan);
        assert!(matches!(
            &flow.steps["move:backlog"],
            WorkflowStep::Move { column, next: Some(n) } if column == "develop" && n == "agent:develop"
        ));

        // Review agent after Implement, then drag it into Review before the condition.
        let review =
            insert_step(&mut d, "develop", Some("agent:develop"), template("review").unwrap())
                .unwrap();
        assert_eq!(stages(&d)[1].steps, ["agent:develop", review.as_str()]);
        compiles(&d);
        move_step(&mut d, &review, "review", None).unwrap();
        let s = stages(&d);
        assert_eq!(s[1].steps, ["agent:develop"]);
        assert_eq!(s[2].steps[0], review);
        let flow = compiles(&d);
        assert_eq!(flow.entries["review"], review);
        // Implement's exit still continues at Review's (new) entry.
        assert!(matches!(
            &flow.steps["move:develop"],
            WorkflowStep::Move { next: Some(n), .. } if *n == review
        ));

        // Disabling skips at compile time; references jump to the successor.
        set_step_enabled(&mut d, &review, false).unwrap();
        let flow = compiles(&d);
        assert!(!flow.steps.contains_key(&review));
        assert_eq!(flow.entries["review"], "condition:external_pr");

        remove_step(&mut d, &review).unwrap();
        assert!(!d.steps.iter().any(|s| s.id == review));
        compiles(&d);
    }

    #[test]
    fn disabling_a_tail_step_ends_the_run_and_conditions_follow_otherwise() {
        let mut d = doc();
        set_step_enabled(&mut d, "condition:external_pr", false).unwrap();
        let flow = compiles(&d);
        assert_eq!(flow.entries["review"], "deliver:review");
        set_step_enabled(&mut d, "deliver:review", false).unwrap();
        let flow = compiles(&d);
        // The skipped tail resolves to a terminal rather than a dangling reference.
        assert!(matches!(
            flow.steps.get(flow.entries["review"].as_str()),
            Some(WorkflowStep::Complete)
        ));
    }

    #[test]
    fn moving_a_step_keeps_rework_and_branch_references() {
        let mut d = doc();
        // Review's approval and deliver rework to Implement; move Implement into Review.
        move_step(&mut d, "agent:develop", "review", Some("approval:review")).unwrap();
        let step = |id: &str| d.steps.iter().find(|s| s.id == id).unwrap().clone();
        assert_eq!(step("approval:review").on_changes_requested.as_deref(), Some("agent:develop"));
        assert_eq!(step("deliver:review").on_changes_requested.as_deref(), Some("agent:develop"));
        assert_eq!(step("approval:review").next.as_deref(), Some("agent:develop"));
        assert!(stages(&d)[1].steps.is_empty());
        compiles(&d);
        // Moving within the same Stage to the front.
        move_step(&mut d, "agent:develop", "review", None).unwrap();
        assert_eq!(stages(&d)[2].steps[0], "agent:develop");
        compiles(&d);
        assert!(move_step(&mut d, "condition:external_pr", "develop", None).is_err());
    }

    #[test]
    fn columns_add_rename_reorder_and_remove() {
        let mut d = doc();
        let qa = add_column(&mut d, "QA", Some("review")).unwrap();
        assert_eq!(qa, "qa");
        compiles(&d);
        // Relative exit now targets QA (it follows Develop).
        let flow = d.compile().unwrap();
        assert!(matches!(&flow.steps["move:develop"], WorkflowStep::Move { column, .. } if column == "qa"));
        rename_column(&mut d, "qa", "Quality").unwrap();
        assert_eq!(d.columns[2].name, "Quality");
        move_column(&mut d, "qa", 3).unwrap();
        let flow = d.compile().unwrap();
        assert!(matches!(&flow.steps["move:develop"], WorkflowStep::Move { column, .. } if column == "review"));
        remove_column(&mut d, "review").unwrap();
        assert!(!d.columns.iter().any(|c| c.id == "review"));
        assert!(!d.steps.iter().any(|s| s.stage.as_deref() == Some("review")));
        compiles(&d);
        assert!(remove_column(&mut d, "nope").is_err());
    }
}
