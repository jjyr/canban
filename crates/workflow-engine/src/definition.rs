use anyhow::{Result, ensure};
use canban_types::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub fn validate_env_map(env: &BTreeMap<String, String>, scope: &str) -> Result<()> {
    for (k, v) in env {
        ensure!(
            canban_types::is_valid_env_key(k),
            "Invalid environment variable name `{k}` in {scope}"
        );
        ensure!(
            !v.contains('\0'),
            "Environment variable `{k}` in {scope} cannot contain NUL bytes"
        );
        validate_env_expressions(v, k, scope)?;
    }
    Ok(())
}

/// Validate `${{ ... }}` expressions inside workflow/step `env:` values.
/// Only `${{ secrets.<KEY> }}` and `${{ vars.<KEY> }}` are supported.
pub fn validate_env_expressions(value: &str, key: &str, scope: &str) -> Result<()> {
    let mut rest = value;
    while let Some(start) = rest.find("${{") {
        let after_open = &rest[start + 3..];
        let Some(end) = after_open.find("}}") else {
            anyhow::bail!("Unclosed `${{{{` expression in environment variable `{key}` in {scope}");
        };
        let expr = after_open[..end].trim();
        let (ns, target_key) = expr.split_once('.').ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid expression `${{{{ {expr} }}}}` in `{key}` ({scope}); expected `secrets.<KEY>` or `vars.<KEY>`"
            )
        })?;
        let ns = ns.trim();
        let target_key = target_key.trim();
        ensure!(
            matches!(ns, "secrets" | "vars"),
            "Unsupported expression namespace `{ns}` in `{key}` ({scope}); expected `secrets` or `vars`"
        );
        ensure!(
            canban_types::is_valid_env_key(target_key),
            "Invalid variable name `{target_key}` in `${{{{ {expr} }}}}` (`{key}`, {scope})"
        );
        rest = &after_open[end + 2..];
    }
    Ok(())
}

pub fn env_value_references_secret(raw: &str) -> bool {
    let mut rest = raw;
    while let Some(start) = rest.find("${{") {
        let after_open = &rest[start + 3..];
        let Some(end) = after_open.find("}}") else {
            break;
        };
        let expr = after_open[..end].trim();
        if let Some((ns, _)) = expr.split_once('.')
            && ns.trim() == "secrets"
        {
            return true;
        }
        rest = &after_open[end + 2..];
    }
    false
}

/// Expand `${{ vars.KEY }}` and `${{ secrets.KEY }}` inside a workflow/step `env:` value.
/// Returns `(expanded_value, skipped_secret_keys_when_untrusted)`.
pub fn expand_env_templates(
    raw: &str,
    vars: &BTreeMap<String, String>,
    secrets: &BTreeMap<String, String>,
    allow_secrets: bool,
) -> (String, Vec<String>) {
    let mut out = String::with_capacity(raw.len());
    let mut skipped = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 3..];
        let Some(end) = after_open.find("}}") else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let expr = after_open[..end].trim();
        if let Some((ns, target)) = expr.split_once('.') {
            let ns = ns.trim();
            let target = target.trim();
            match ns {
                "vars" => {
                    if let Some(val) = vars.get(target) {
                        out.push_str(val);
                    }
                }
                "secrets" => {
                    if allow_secrets {
                        if let Some(val) = secrets.get(target) {
                            out.push_str(val);
                        }
                    } else if !skipped.iter().any(|s| s == target) {
                        skipped.push(target.to_owned());
                    }
                }
                _ => {
                    out.push_str(&rest[start..start + 3 + end + 2]);
                }
            }
        } else {
            out.push_str(&rest[start..start + 3 + end + 2]);
        }
        rest = &after_open[end + 2..];
    }
    out.push_str(rest);
    (out, skipped)
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

/// Compiles a `WorkflowRule` into its runtime `EventSelector` for `scope_id`.
/// For `github.*` or `cardless` rules without an explicit `match.repository`, binds
/// `project_repository` (when present) so unbound repo-level GitHub events match without
/// requiring `event.project_id` to be stamped on canonical remote facts.
pub fn compile_rule_selector(
    scope_id: &str,
    rule: &WorkflowRule,
    project_repository: Option<&str>,
) -> EventSelector {
    let mut selector = rule.selector.clone();
    selector.project_id = Some(scope_id.to_owned());
    selector.types = vec![rule.event.clone()];
    if matches!(
        rule.event.as_str(),
        "card.column_entered" | "card.column_left"
    ) && selector.origin.is_none()
    {
        selector.origin = Some("local".into());
    }
    if (rule.is_github_event() || rule.cardless)
        && selector
            .repository
            .as_deref()
            .is_none_or(|r| r.trim().is_empty())
    {
        selector.repository = project_repository
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned);
    }
    selector
}

pub fn matches(selector: &EventSelector, event: &Event) -> bool {
    let data = &event.data;
    if selector.exclude_self && data["self_action"].as_bool() == Some(true) {
        return false;
    }
    let has_related_cards = data["related_cards"]
        .as_array()
        .is_some_and(|a| !a.is_empty());
    let repo_matches = selector.repository.as_deref().is_some_and(|v| {
        data["repository"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(v))
    });
    let project_mismatch = selector.project_id.as_deref().is_some_and(|v| {
        let direct_or_related = v == event.project_id
            || data["related_cards"]
                .as_array()
                .is_some_and(|a| a.iter().any(|r| r["project_id"] == v));
        let unbound_repo_match = event.project_id.is_empty() && !has_related_cards && repo_matches;
        !direct_or_related && !unbound_repo_match
    });
    if project_mismatch
        || selector.actor.as_deref().is_some_and(|v| v != event.actor)
        || (selector.repository.is_some() && !repo_matches)
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
    // Transition directions remain authoritative even for historical facts
    // whose snapshot metadata still contains the source column.
    let (entered, left) = match event.kind.as_str() {
        "card.stage_changed" if data["from"] != data["to"] => {
            (data["to"].as_str(), data["from"].as_str())
        }
        "card.created" | "card.restored" => (data["stage"].as_str(), None),
        "card.archived" | "card.removed" => (None, data["stage"].as_str()),
        _ => (None, None),
    };
    let column_matches = |column: Option<&str>| {
        column.is_some_and(|column| selector.column.as_deref().is_none_or(|v| column == v))
    };
    let current_column_matches = selector
        .column
        .as_deref()
        .is_none_or(|v| data["stage"].as_str().or_else(|| data["to"].as_str()) == Some(v));
    let type_match = selector.types.is_empty()
        || selector.types.iter().any(|t| {
            if t == "card.column_entered" {
                column_matches(entered)
            } else if t == "card.column_left" {
                column_matches(left)
            } else {
                current_column_matches
                    && (t == "*"
                        || t == &event.kind
                        || t.strip_suffix(".*")
                            .is_some_and(|prefix| event.kind.starts_with(&format!("{prefix}.")))
                        || (t == "card.content_changed" && event.kind == "card.updated"))
            }
        });
    type_match
        && (!selector.types.is_empty() || current_column_matches)
        && (selector.changed_fields.is_empty()
            || data["changed_fields"].as_array().is_some_and(|a| {
                selector
                    .changed_fields
                    .iter()
                    .any(|v| a.iter().any(|f| f == v))
            }))
}

#[cfg(test)]
mod column_selector_tests {
    use super::*;
    use serde_json::json;

    fn check(kind: &str, data: serde_json::Value, entered: &[&str], left: &[&str]) {
        let event = canban_timeline::event("project", "card", kind, "user", "", data);
        for (view, expected) in [("card.column_entered", entered), ("card.column_left", left)] {
            for column in ["backlog", "develop", "review"] {
                let selector =
                    serde_json::from_value(json!({"types":[view],"column":column})).unwrap();
                assert_eq!(
                    matches(&selector, &event),
                    expected.contains(&column),
                    "{kind}: {view} {column}: {}",
                    event.data
                );
            }
        }
    }

    #[test]
    fn transition_views_use_direction_even_with_stale_snapshot_metadata() {
        for stage in ["develop", "review"] {
            check(
                "card.stage_changed",
                json!({"from":"develop","to":"review","stage":stage}),
                &["review"],
                &["develop"],
            );
            check(
                "card.stage_changed",
                json!({"from":"review","to":"develop","stage":stage}),
                &["develop"],
                &["review"],
            );
        }
        check(
            "card.stage_changed",
            json!({"from":"develop","to":"develop","stage":"develop"}),
            &[],
            &[],
        );
        check("card.stage_changed", json!({"stage":"develop"}), &[], &[]);
        check("card.order_changed", json!({"stage":"develop"}), &[], &[]);
    }

    #[test]
    fn membership_views_and_mixed_type_alternatives_keep_column_constraints() {
        for kind in ["card.created", "card.restored"] {
            check(
                kind,
                json!({"stage":"backlog","from":"review","to":"develop"}),
                &["backlog"],
                &[],
            );
        }
        for kind in ["card.archived", "card.removed"] {
            check(
                kind,
                json!({"stage":"review","from":"develop","to":"backlog"}),
                &[],
                &["review"],
            );
        }
        let event = canban_timeline::event(
            "project",
            "card",
            "card.updated",
            "user",
            "",
            json!({"stage":"develop"}),
        );
        for types in [
            json!([]),
            json!(["*"]),
            json!(["card.updated", "card.column_entered"]),
            json!(["card.content_changed", "card.column_left"]),
        ] {
            for column in ["develop", "review"] {
                let selector =
                    serde_json::from_value(json!({"types":types,"column":column})).unwrap();
                assert_eq!(matches(&selector, &event), column == "develop");
            }
        }
    }

    #[test]
    fn unbound_repo_events_match_project_subscription_and_respect_exclude_self() {
        let rule = WorkflowRule {
            event: "github.issue.comment_created".into(),
            start_at: "script:filter".into(),
            cardless: true,
            selector: EventSelector {
                exclude_self: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let selector = compile_rule_selector("proj-1", &rule, Some("Org/Repo"));
        assert_eq!(selector.project_id.as_deref(), Some("proj-1"));
        assert_eq!(selector.repository.as_deref(), Some("Org/Repo"));
        assert!(selector.exclude_self);

        let external_comment = canban_timeline::event(
            "",
            "",
            "github.issue.comment_created",
            "github:alice",
            "new comment",
            json!({
                "repository": "org/repo",
                "related_cards": [],
                "self_action": false
            }),
        );
        assert!(matches(&selector, &external_comment));

        let self_comment = canban_timeline::event(
            "",
            "",
            "github.issue.comment_created",
            "github:me",
            "my comment",
            json!({
                "repository": "org/repo",
                "related_cards": [],
                "self_action": true
            }),
        );
        assert!(!matches(&selector, &self_comment));

        let other_repo = canban_timeline::event(
            "",
            "",
            "github.issue.comment_created",
            "github:alice",
            "other repo",
            json!({
                "repository": "org/other",
                "related_cards": [],
                "self_action": false
            }),
        );
        assert!(!matches(&selector, &other_repo));

        let other_project_bound = canban_timeline::event(
            "proj-2",
            "card-9",
            "github.issue.comment_created",
            "github:alice",
            "bound elsewhere",
            json!({
                "repository": "org/repo",
                "self_action": false
            }),
        );
        assert!(!matches(&selector, &other_project_bound));

        let other_project_related = canban_timeline::event(
            "",
            "",
            "github.issue.comment_created",
            "github:alice",
            "related elsewhere",
            json!({
                "repository": "org/repo",
                "related_cards": [{"project_id": "proj-2", "card_id": "card-9"}],
                "self_action": false
            }),
        );
        assert!(!matches(&selector, &other_project_related));

        // Without repository bound on the selector, unbound events do NOT match a project selector.
        let no_repo_selector = EventSelector {
            project_id: Some("proj-1".into()),
            types: vec!["github.issue.comment_created".into()],
            ..Default::default()
        };
        assert!(!matches(&no_repo_selector, &external_comment));
    }

    #[test]
    fn env_expression_validation_and_expansion_handles_vars_secrets_and_untrusted_runs() {
        let valid = BTreeMap::from([
            (
                "AUTH_HEADER".into(),
                "Bearer ${{ secrets.DEPLOY_TOKEN }}".into(),
            ),
            (
                "URL".into(),
                "https://${{ vars.API_HOST }}/v1?k=${{ secrets.API_KEY }}".into(),
            ),
        ]);
        validate_env_map(&valid, "step deploy").unwrap();
        assert!(env_value_references_secret(&valid["AUTH_HEADER"]));
        assert!(!env_value_references_secret("https://${{ vars.API_HOST }}"));

        for bad in [
            BTreeMap::from([("A".into(), "${{ secrets.UNCLOSED".into())]),
            BTreeMap::from([("A".into(), "${{ github.token }}".into())]),
            BTreeMap::from([("A".into(), "${{ secrets.BAD KEY }}".into())]),
            BTreeMap::from([("A".into(), "${{ just_name }}".into())]),
        ] {
            assert!(validate_env_map(&bad, "step bad").is_err(), "{bad:?}");
        }

        let vars = BTreeMap::from([("API_HOST".into(), "api.example.com".into())]);
        let secrets = BTreeMap::from([
            ("DEPLOY_TOKEN".into(), "tok-secret".into()),
            ("API_KEY".into(), "key-secret".into()),
        ]);

        let (trusted_val, skipped_ok) = expand_env_templates(
            "https://${{ vars.API_HOST }}/v1?k=${{ secrets.API_KEY }}",
            &vars,
            &secrets,
            true,
        );
        assert_eq!(trusted_val, "https://api.example.com/v1?k=key-secret");
        assert!(skipped_ok.is_empty());

        let (untrusted_val, skipped) = expand_env_templates(
            "https://${{ vars.API_HOST }}/v1?k=${{ secrets.API_KEY }}",
            &vars,
            &secrets,
            false,
        );
        assert_eq!(untrusted_val, "https://api.example.com/v1?k=");
        assert_eq!(skipped, vec!["API_KEY".to_owned()]);
    }
}
