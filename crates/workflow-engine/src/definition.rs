use anyhow::{Result, ensure};
use canban_types::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub fn validate_env_map(env: &BTreeMap<String, String>, scope: &str) -> Result<()> {
    for (k, v) in env {
        ensure!(
            !k.is_empty()
                && !k.contains('=')
                && !k.contains('\0')
                && !k.chars().any(char::is_whitespace),
            "Invalid environment variable name `{k}` in {scope}"
        );
        ensure!(
            !v.contains('\0'),
            "Environment variable `{k}` in {scope} cannot contain NUL bytes"
        );
    }
    Ok(())
}

pub fn validate_definition(
    flow: &WorkflowDefinition,
    columns: &[String],
    profiles: &[String],
) -> Result<()> {
    ensure!(
        !flow.id.trim().is_empty() && !flow.name.trim().is_empty(),
        "Workflow id/name required"
    );
    ensure!(
        !flow.entries.is_empty() && !flow.steps.is_empty(),
        "Workflow needs entries and steps"
    );
    ensure!(
        flow.steps.len() <= 256 && (1..=1000).contains(&flow.max_steps),
        "Workflow size/step limit exceeded"
    );
    ensure!(
        !flow.workspace.sync_base || flow.workspace.create_worktree,
        "sync_base requires create_worktree"
    );
    validate_env_map(&flow.env, "workflow")?;
    let mut declared_cols = BTreeSet::new();
    for col in &flow.columns {
        ensure!(
            !col.id.trim().is_empty() && !col.name.trim().is_empty(),
            "Workflow column id and name are required"
        );
        ensure!(
            col.id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
            "Invalid workflow column id {}",
            col.id
        );
        ensure!(
            declared_cols.insert(col.id.clone()),
            "Duplicate workflow column {}",
            col.id
        );
    }
    let effective_columns: Vec<String> = if flow.columns.is_empty() {
        columns.to_vec()
    } else {
        flow.columns.iter().map(|c| c.id.clone()).collect()
    };
    for (entry, target) in &flow.entries {
        ensure!(
            !entry.is_empty() && flow.steps.contains_key(target),
            "Invalid entry {entry}: {target}"
        );
    }
    for (id, step) in &flow.steps {
        ensure!(!id.is_empty(), "Empty step ID");
        let mut targets = Vec::new();
        if let Some(next) = step.next() {
            targets.push(next);
        }
        match step {
            WorkflowStep::Agent {
                profile,
                instruction,
                env,
                ..
            } => {
                ensure!(!instruction.trim().is_empty(), "Empty instruction at {id}");
                validate_env_map(env, &format!("step {id}"))?;
                if let Some(profile) = profile {
                    ensure!(
                        profiles.contains(profile),
                        "Unknown agent @{profile} at {id}"
                    );
                }
            }
            WorkflowStep::Script {
                run,
                shell,
                working_directory,
                timeout_minutes,
                timeout_seconds,
                env,
                on_failure,
                ..
            } => {
                ensure!(
                    !run.trim().is_empty(),
                    "Script needs a `run` command at {id}"
                );
                validate_env_map(env, &format!("step {id}"))?;
                if let Some(sh) = shell {
                    ensure!(
                        !sh.trim().is_empty()
                            && !sh.contains('\n')
                            && !sh.contains('\r')
                            && !sh.contains('\0'),
                        "Invalid `shell` at {id}"
                    );
                }
                if let Some(wd) = working_directory {
                    ensure!(!wd.trim().is_empty(), "Empty `working_directory` at {id}");
                    ensure!(
                        !wd.starts_with('/') && !wd.starts_with('\\') && !wd.starts_with('~'),
                        "working_directory must be relative to the Card workspace at {id}"
                    );
                    let path = Path::new(wd);
                    ensure!(
                        !path.is_absolute()
                            && path.components().all(|c| {
                                !matches!(
                                    c,
                                    Component::ParentDir
                                        | Component::RootDir
                                        | Component::Prefix(_)
                                )
                            }),
                        "working_directory cannot escape the Card workspace at {id}"
                    );
                }
                if let Some(mins) = timeout_minutes {
                    ensure!(
                        (1..=1440).contains(mins),
                        "timeout_minutes must be between 1 and 1440 at {id}"
                    );
                }
                if let Some(secs) = timeout_seconds {
                    ensure!(
                        (1..=86_400).contains(secs),
                        "timeout_seconds must be between 1 and 86400 at {id}"
                    );
                }
                if let Some(to) = on_failure {
                    ensure!(
                        flow.steps.contains_key(to),
                        "Unknown on_failure target {to} at {id}"
                    );
                }
            }
            WorkflowStep::Move { column, .. } => {
                ensure!(
                    effective_columns.contains(column),
                    "Unknown column {column} at {id}"
                )
            }
            WorkflowStep::Approval { title, rework, .. } => {
                ensure!(!title.trim().is_empty(), "Approval needs title at {id}");
                if let Some(to) = rework {
                    ensure!(flow.steps.contains_key(to), "Unknown rework target {to}");
                }
            }
            WorkflowStep::Deliver {
                profile,
                batch_size,
                test_command,
                rework,
                ..
            } => {
                if let Some(profile) = profile {
                    ensure!(
                        profiles.contains(profile),
                        "Unknown agent @{profile} at {id}"
                    );
                }
                ensure!(
                    (1..=50).contains(batch_size),
                    "batch_size must be between 1 and 50 at {id}"
                );
                if let Some(cmd) = test_command {
                    ensure!(
                        !cmd.trim().is_empty(),
                        "test_command must be non-empty when set at {id}"
                    );
                }
                if let Some(to) = rework {
                    ensure!(flow.steps.contains_key(to), "Unknown rework target {to}");
                }
            }
            WorkflowStep::Admit {
                agent_slot,
                max_ready,
                ..
            } => {
                ensure!(
                    max_ready.is_none_or(|k| (1..=100).contains(&k)),
                    "max_ready must be between 1 and 100 at {id}"
                );
                ensure!(
                    *agent_slot || max_ready.is_some(),
                    "Admit needs agent_slot or max_ready at {id}"
                );
            }
            WorkflowStep::Condition {
                pointer,
                then,
                otherwise,
                ..
            } => {
                ensure!(
                    pointer.is_empty() || pointer.starts_with('/'),
                    "Condition requires a JSON pointer at {id}"
                );
                targets.extend([then.as_str(), otherwise.as_str()]);
            }
            _ => {}
        }
        for to in targets {
            ensure!(flow.steps.contains_key(to), "Unknown target {to} at {id}");
        }
    }
    // Rework edges require an explicit decision; automatic cycles are rejected.
    fn visit<'a>(
        id: &'a str,
        flow: &'a WorkflowDefinition,
        stack: &mut BTreeSet<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        if done.contains(id) {
            return Ok(());
        }
        ensure!(stack.insert(id), "Automatic workflow cycle at {id}");
        let step = &flow.steps[id];
        if let Some(to) = step.next() {
            visit(to, flow, stack, done)?;
        }
        if let WorkflowStep::Condition {
            then, otherwise, ..
        } = step
        {
            visit(then, flow, stack, done)?;
            visit(otherwise, flow, stack, done)?;
        }
        stack.remove(id);
        done.insert(id);
        Ok(())
    }
    let mut done = BTreeSet::new();
    for id in flow.steps.keys() {
        visit(id, flow, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

pub fn matches(selector: &EventSelector, event: &Event) -> bool {
    let data = &event.data;
    if selector.project_id.as_deref().is_some_and(|v| {
        v != event.project_id
            && !data["related_cards"]
                .as_array()
                .is_some_and(|a| a.iter().any(|r| r["project_id"] == v))
    }) || selector.actor.as_deref().is_some_and(|v| v != event.actor)
        || selector
            .repository
            .as_deref()
            .is_some_and(|v| data["repository"].as_str() != Some(v))
        || selector
            .subject
            .as_deref()
            .is_some_and(|v| data["subject"].as_str() != Some(v))
        || selector
            .origin
            .as_deref()
            .is_some_and(|v| data["origin"].as_str() != Some(v))
        || selector
            .source_kind
            .as_deref()
            .is_some_and(|v| data["source_kind"].as_str() != Some(v))
        || selector
            .reason
            .as_deref()
            .is_some_and(|v| data["reason"].as_str() != Some(v))
    {
        return false;
    }
    if let Some(card) = &selector.card_id {
        let related = data["related_cards"].as_array().is_some_and(|a| {
            a.iter().any(|r| {
                r["card_id"] == *card && (selector.include_related || r["kind"] == "source")
            })
        });
        if event.card_id != *card && !related {
            return false;
        }
    }
    if selector
        .reply_to
        .as_deref()
        .is_some_and(|v| data["reply_to"].as_str() != Some(v))
    {
        return false;
    }
    let entered = matches!(
        event.kind.as_str(),
        "card.created" | "card.restored" | "card.stage_changed"
    );
    let left = matches!(
        event.kind.as_str(),
        "card.archived" | "card.removed" | "card.stage_changed"
    );
    let type_match = selector.types.is_empty()
        || selector.types.iter().any(|t| {
            t == "*"
                || t == &event.kind
                || t.strip_suffix(".*")
                    .is_some_and(|prefix| event.kind.starts_with(&format!("{prefix}.")))
                || (t == "card.column_entered"
                    && entered
                    && selector
                        .column
                        .as_ref()
                        .is_none_or(|v| data["to"] == *v || data["stage"] == *v))
                || (t == "card.column_left"
                    && left
                    && selector
                        .column
                        .as_ref()
                        .is_none_or(|v| data["from"] == *v || data["stage"] == *v))
                || (t == "card.content_changed" && event.kind == "card.updated")
        });
    type_match
        && (selector.column.is_none()
            || selector
                .types
                .iter()
                .any(|t| t == "card.column_entered" || t == "card.column_left")
            || selector
                .column
                .as_ref()
                .is_some_and(|v| data["stage"] == *v || data["to"] == *v))
        && (selector.changed_fields.is_empty()
            || data["changed_fields"].as_array().is_some_and(|a| {
                selector
                    .changed_fields
                    .iter()
                    .any(|v| a.iter().any(|f| f == v))
            }))
}
