//! Linear workflow YAML schema migrations, analogous to `canban_storage::migrations`.
//!
//! Runtime code (`source`, `stages`, `definition`, and Studio UI) targets only [`CURRENT`].
//! Historical workflow defaults or structural rewrites live exclusively in sequential
//! [`Migration`] steps here so core authoring/runtime modules never accumulate legacy branches.
use crate::source::Document;
use anyhow::{Result, ensure};

pub const BASELINE: u32 = 1;
pub const CURRENT: u32 = 5;

pub struct Migration {
    pub version: u32,
    pub up: fn(&mut Document) -> Result<()>,
}

const STEPS: &[Migration] = &[
    Migration {
        version: 2,
        up: v2_omit_builtin_default_prompts,
    },
    Migration {
        version: 3,
        up: v3_decision_aware_default_prompts,
    },
    Migration {
        version: 4,
        up: v4_deliver_merger_role,
    },
    Migration {
        version: 5,
        up: v5_deliver_merge_strategy,
    },
];

/// Migrate a parsed [`Document`] from any supported [`BASELINE`]..=[`CURRENT`] version to [`CURRENT`].
/// Returns `Ok(true)` if `doc` was upgraded from an older schema version.
pub fn upgrade(doc: &mut Document) -> Result<bool> {
    ensure!(
        CURRENT == STEPS.last().map_or(BASELINE, |s| s.version),
        "Workflow migration registry and CURRENT schema disagree"
    );
    for (i, step) in STEPS.iter().enumerate() {
        ensure!(
            step.version == BASELINE + i as u32 + 1,
            "Workflow migration versions must be consecutive"
        );
    }
    ensure!(
        (BASELINE..=CURRENT).contains(&doc.schema),
        "Unsupported workflow schema {}; supported {BASELINE}..={CURRENT}",
        doc.schema
    );
    if doc.schema == CURRENT {
        return Ok(false);
    }
    let before = doc.schema;
    for step in STEPS
        .iter()
        .filter(|s| s.version > before && s.version <= CURRENT)
    {
        (step.up)(doc)?;
        doc.schema = step.version;
    }
    Ok(true)
}

/// Upgrade raw YAML text to [`CURRENT`] while preserving leading `#` header comment lines
/// when structural re-encoding is needed. Returns `Ok(Some(upgraded_yaml))` when `raw` was
/// on an older schema version, or `Ok(None)` when already at [`CURRENT`].
pub fn upgrade_yaml(raw: &str) -> Result<Option<String>> {
    ensure!(
        raw.len() <= 2 * 1024 * 1024,
        "Workflow source exceeds 2 MiB"
    );
    let mut doc: Document = serde_yaml_ng::from_str(raw)?;
    if !upgrade(&mut doc)? {
        return Ok(None);
    }
    let leading_comments: Vec<&str> = raw
        .lines()
        .take_while(|l| {
            let t = l.trim();
            t.is_empty() || t.starts_with('#')
        })
        .collect();
    let encoded = crate::source::encode(&doc)?;
    if leading_comments.iter().any(|l| l.trim().starts_with('#')) {
        Ok(Some(format!("{}\n{encoded}", leading_comments.join("\n"))))
    } else {
        Ok(Some(encoded))
    }
}

/// Schema 2: Built-in default step prompts are resolved at compile time instead of being
/// materialized verbatim in YAML. Strip unmodified v1 built-in prompts so future default
/// prompt refinements take effect automatically while preserving user-customized prompts.
fn v2_omit_builtin_default_prompts(doc: &mut Document) -> Result<()> {
    const V1_BUILTINS: &[&str] = &[
        "Analyze the Card task, inspect relevant repository files, and write a concise implementation plan with files to change, risks, and validation steps in your report. Do not modify code in this planning step.",
        "Implement the card task in the supplied working directory. Explain your changes and validation in your report.",
        "Implement the card task in the supplied working directory, commit your changes on the Card branch, and explain your changes and validation in your report.",
        "Review the work for this card. Inspect the code and earlier results, then write an ordinary Markdown review report. Report findings and any remaining concerns. Do not implement unrelated changes.",
        "Perform pre-merge verification for this card. Run tests and checks in the working directory, verify there are no blockers or unintended changes, and summarize readiness in your report.",
    ];
    for step in &mut doc.steps {
        if step.uses != "agent" {
            continue;
        }
        let strip = step
            .with
            .get("instruction")
            .and_then(|v| v.as_str())
            .is_some_and(|instr| {
                let trimmed = instr.trim();
                trimmed.is_empty() || V1_BUILTINS.contains(&trimmed)
            });
        if strip {
            step.with.remove("instruction");
        }
    }
    Ok(())
}

/// Schema 3: Default step prompts become decision-aware (allowing `--state backlog` when a Card
/// is already implemented or obsolete). Advancing the schema triggers recompilation of active
/// `WorkflowRevision`s on startup and strips any materialized v2 default prompt strings.
fn v3_decision_aware_default_prompts(doc: &mut Document) -> Result<()> {
    v2_omit_builtin_default_prompts(doc)
}

/// Schema 4: `deliver` steps gain dedicated `@merger` conflict resolution (`with.profile`,
/// defaulting to `@merger` at runtime while keeping `on_changes_requested` for human rework).
/// Strips explicit `profile: merger` on `deliver` steps so the built-in default stays clean.
fn v4_deliver_merger_role(doc: &mut Document) -> Result<()> {
    for step in &mut doc.steps {
        if step.uses == "deliver"
            && step
                .with
                .get("profile")
                .and_then(|v| v.as_str())
                .is_some_and(|p| p.trim().is_empty() || p.trim() == "merger")
        {
            step.with.remove("profile");
        }
    }
    Ok(())
}

/// Schema 5: `deliver` steps gain configurable `with.strategy` (`rebase` default, `squash`,
/// or `merge`). Strips explicit `strategy: rebase` (or blank strings) so the built-in default
/// remains clean in YAML.
fn v5_deliver_merge_strategy(doc: &mut Document) -> Result<()> {
    for step in &mut doc.steps {
        if step.uses == "deliver"
            && step
                .with
                .get("strategy")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.trim().is_empty() || s.trim() == "rebase")
        {
            step.with.remove("strategy");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_v1_builtin_prompts_and_preserves_custom_prompts_and_header_comments() {
        let v1 = r#"# Team workflow header
schema: 1
id: delivery:demo
name: Development
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory. Explain your changes and validation in your report.
    next: agent:custom
  - id: agent:custom
    uses: agent
    with:
      profile: reviewer
      instruction: Check security invariants specifically.
    next: __end
  - id: __end
    uses: complete
"#;
        let upgraded = upgrade_yaml(v1).unwrap().expect("v1 should upgrade to v2");
        assert!(upgraded.starts_with("# Team workflow header\n"));
        let doc = crate::source::parse(&upgraded).unwrap();
        assert_eq!(doc.schema, CURRENT);
        assert!(
            !doc.steps[0].with.contains_key("instruction"),
            "unmodified v1 default prompt is stripped"
        );
        assert_eq!(
            doc.steps[1].with["instruction"].as_str(),
            Some("Check security invariants specifically."),
            "custom prompt is preserved verbatim"
        );
        assert!(
            upgrade_yaml(&upgraded).unwrap().is_none(),
            "already-current YAML is untouched"
        );
    }

    #[test]
    fn v5_strips_redundant_default_rebase_strategy_and_keeps_squash_or_merge() {
        let v4 = r#"schema: 4
id: delivery:demo
name: Development
on:
  review:
    event: card.column_entered
    start_at: deliver:review
    match:
      column: review
steps:
  - id: deliver:review
    uses: deliver
    with:
      strategy: rebase
      sync_upstream: true
    next: deliver:squash
  - id: deliver:squash
    uses: deliver
    with:
      strategy: squash
    next: __end
  - id: __end
    uses: complete
"#;
        let upgraded = upgrade_yaml(v4).unwrap().expect("v4 should upgrade to v5");
        let doc = crate::source::parse(&upgraded).unwrap();
        assert_eq!(doc.schema, CURRENT);
        assert!(
            !doc.steps[0].with.contains_key("strategy"),
            "default rebase strategy is stripped"
        );
        assert_eq!(
            doc.steps[1].with.get("strategy").and_then(|v| v.as_str()),
            Some("squash"),
            "non-default strategy is preserved"
        );
    }
}
