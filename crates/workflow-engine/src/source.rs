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
    #[serde(default, skip_serializing_if = "is_default_workflow_scope")]
    pub scope: WorkflowScope,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<WorkflowColumn>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "deserialize_env_map"
    )]
    pub env: BTreeMap<String, String>,
    #[serde(deserialize_with = "deserialize_on_map")]
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
    /// Action type (omitted when using the top-level `run:` script shorthand).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uses: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_on_error: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "deserialize_env_map"
    )]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub with: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_changes_requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
    /// Board column (Stage) this step belongs to in Studio. Inferred when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// `false` skips the step at compile time; references continue to its successor.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
}
impl Default for Step {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: None,
            uses: String::new(),
            run: None,
            shell: None,
            working_directory: None,
            timeout_minutes: None,
            timeout_seconds: None,
            continue_on_error: None,
            env: BTreeMap::new(),
            with: BTreeMap::new(),
            next: None,
            on_changes_requested: None,
            on_failure: None,
            stage: None,
            enabled: true,
        }
    }
}
impl Step {
    /// Canonical step kind (`"script"` when `uses` is omitted and `run` is set).
    pub fn kind(&self) -> &str {
        let uses = self.uses.trim();
        if uses.is_empty() && self.run.is_some() {
            "script"
        } else {
            uses
        }
    }

    /// Script command body from either top-level `run` or `with.run`.
    pub fn script_run(&self) -> Option<&str> {
        self.run
            .as_deref()
            .or_else(|| self.with.get("run").and_then(|v| v.as_str()))
    }
}
fn is_yes(v: &bool) -> bool {
    *v
}
fn scalar_env_value(key: &str, value: Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => bail!("Environment variable {key} must have a scalar value"),
    }
}
fn value_map_to_env_map(raw: BTreeMap<String, Value>) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        out.insert(k.clone(), scalar_env_value(&k, v)?);
    }
    Ok(out)
}
fn value_to_env_map(value: Value, context: &str) -> Result<BTreeMap<String, String>> {
    let obj = value
        .as_object()
        .cloned()
        .with_context(|| format!("{context} must be a mapping"))?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        out.insert(k.clone(), scalar_env_value(&k, v)?);
    }
    Ok(out)
}
fn deserialize_env_map<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<BTreeMap<String, Value>>::deserialize(deserializer)?.unwrap_or_default();
    value_map_to_env_map(raw).map_err(serde::de::Error::custom)
}
fn deserialize_on_map<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, WorkflowRule>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RuleOrScheduleList {
        Rule(Box<WorkflowRule>),
        ScheduleList(Vec<WorkflowRule>),
    }
    let raw = BTreeMap::<String, RuleOrScheduleList>::deserialize(deserializer)?;
    let mut out = BTreeMap::new();
    for (key, entry) in raw {
        match entry {
            RuleOrScheduleList::Rule(mut rule) => {
                if rule.cron.is_some() {
                    rule.event = "schedule".into();
                }
                out.insert(key, *rule);
            }
            RuleOrScheduleList::ScheduleList(items) => {
                if items.is_empty() {
                    return Err(serde::de::Error::custom(format!(
                        "`on.{key}` schedule list cannot be empty"
                    )));
                }
                let single = items.len() == 1;
                for (idx, mut rule) in items.into_iter().enumerate() {
                    rule.event = "schedule".into();
                    let name = if single && !out.contains_key(&key) {
                        key.clone()
                    } else {
                        let mut cand = format!("{key}_{}", idx + 1);
                        let mut n = idx + 2;
                        while out.contains_key(&cand) {
                            cand = format!("{key}_{n}");
                            n += 1;
                        }
                        cand
                    };
                    out.insert(name, rule);
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn parse_raw(source: &str) -> Result<Document> {
    let mut doc: Document = serde_yaml_ng::from_str(source)?;
    let first_step = doc.steps.first().map(|s| s.id.clone());
    for rule in doc.on.values_mut() {
        if rule.cron.is_some() {
            rule.event = "schedule".into();
        }
        if rule.is_schedule()
            && rule.start_at.trim().is_empty()
            && let Some(first) = &first_step
        {
            rule.start_at = first.clone();
        }
    }
    Ok(doc)
}

pub fn parse(source: &str) -> Result<Document> {
    ensure!(
        source.len() <= 2 * 1024 * 1024,
        "Workflow source exceeds 2 MiB"
    );
    let mut doc = parse_raw(source)?;
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
            let raw_uses = step.uses.trim();
            if raw_uses.is_empty() && step.run.is_none() {
                bail!("Step {} must specify `uses` or `run`", step.id);
            }
            let kind = match step.kind() {
                "agent" => "agent",
                "script" => "script",
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
            if kind != "script" {
                ensure!(
                    step.run.is_none()
                        && step.shell.is_none()
                        && step.working_directory.is_none()
                        && step.timeout_minutes.is_none()
                        && step.timeout_seconds.is_none()
                        && step.continue_on_error.is_none()
                        && step.on_failure.is_none(),
                    "Step {} ({kind}) cannot define script-only fields",
                    step.id
                );
            }
            if !matches!(kind, "agent" | "script") {
                ensure!(
                    step.env.is_empty() && !args.contains_key("env"),
                    "Step {} ({kind}) does not accept `env`",
                    step.id
                );
            } else {
                let mut merged_env = match args.remove("env") {
                    Some(v) => value_to_env_map(v, &format!("Step {} `with.env`", step.id))?,
                    None => BTreeMap::new(),
                };
                for (k, v) in &step.env {
                    if let Some(prev) = merged_env.get(k)
                        && prev != v
                    {
                        bail!(
                            "Step {} specifies conflicting `env.{k}` and `with.env.{k}`",
                            step.id
                        );
                    }
                    merged_env.insert(k.clone(), v.clone());
                }
                if !merged_env.is_empty() {
                    args.insert("env".into(), json!(merged_env));
                }
            }
            if kind == "script" {
                let with_run = match args.remove("run") {
                    Some(Value::String(s)) => Some(s),
                    Some(_) => bail!("Step {} `with.run` must be a string", step.id),
                    None => None,
                };
                let run = match (&step.run, with_run) {
                    (Some(a), Some(b)) if a != &b => {
                        bail!(
                            "Step {} specifies conflicting `run` and `with.run`",
                            step.id
                        )
                    }
                    (Some(a), _) => a.clone(),
                    (None, Some(b)) => b,
                    (None, None) => bail!("Script step {} requires `run`", step.id),
                };
                args.insert("run".into(), json!(run));

                let pick_str = |top: &Option<String>,
                                with_val: Option<Value>,
                                field: &str|
                 -> Result<Option<String>> {
                    let from_with = match with_val {
                        Some(Value::String(s)) => Some(s),
                        Some(_) => bail!("Step {} `with.{field}` must be a string", step.id),
                        None => None,
                    };
                    match (top, from_with) {
                        (Some(a), Some(b)) if a != &b => bail!(
                            "Step {} specifies conflicting `{field}` and `with.{field}`",
                            step.id
                        ),
                        (Some(a), _) => Ok(Some(a.clone())),
                        (None, Some(b)) => Ok(Some(b)),
                        (None, None) => Ok(None),
                    }
                };
                if let Some(shell) = pick_str(&step.shell, args.remove("shell"), "shell")? {
                    args.insert("shell".into(), json!(shell));
                }
                if let Some(wd) = pick_str(
                    &step.working_directory,
                    args.remove("working_directory"),
                    "working_directory",
                )? {
                    args.insert("working_directory".into(), json!(wd));
                }

                let pick_u64 = |top: Option<u64>,
                                with_val: Option<Value>,
                                field: &str|
                 -> Result<Option<u64>> {
                    let from_with = match with_val {
                        Some(v) => Some(v.as_u64().with_context(|| {
                            format!(
                                "Step {} `with.{field}` must be a non-negative integer",
                                step.id
                            )
                        })?),
                        None => None,
                    };
                    match (top, from_with) {
                        (Some(a), Some(b)) if a != b => bail!(
                            "Step {} specifies conflicting `{field}` and `with.{field}`",
                            step.id
                        ),
                        (Some(a), _) => Ok(Some(a)),
                        (None, Some(b)) => Ok(Some(b)),
                        (None, None) => Ok(None),
                    }
                };
                if let Some(mins) = pick_u64(
                    step.timeout_minutes.map(u64::from),
                    args.remove("timeout_minutes"),
                    "timeout_minutes",
                )? {
                    let mins_u32 = u32::try_from(mins).with_context(|| {
                        format!("Step {} `timeout_minutes` is too large", step.id)
                    })?;
                    args.insert("timeout_minutes".into(), json!(mins_u32));
                }
                if let Some(secs) = pick_u64(
                    step.timeout_seconds,
                    args.remove("timeout_seconds"),
                    "timeout_seconds",
                )? {
                    args.insert("timeout_seconds".into(), json!(secs));
                }

                let from_with_coe = match args.remove("continue_on_error") {
                    Some(v) => Some(v.as_bool().with_context(|| {
                        format!(
                            "Step {} `with.continue_on_error` must be a boolean",
                            step.id
                        )
                    })?),
                    None => None,
                };
                let coe = match (step.continue_on_error, from_with_coe) {
                    (Some(a), Some(b)) if a != b => bail!(
                        "Step {} specifies conflicting `continue_on_error` and `with.continue_on_error`",
                        step.id
                    ),
                    (Some(a), _) | (None, Some(a)) => a,
                    (None, None) => false,
                };
                args.insert("continue_on_error".into(), json!(coe));
                args.insert(
                    "on_failure".into(),
                    json!(
                        target(step.on_failure.clone(), true).filter(|_| step.on_failure.is_some())
                    ),
                );
            }
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
            scope: self.scope,
            columns: self.columns.clone(),
            env: self.env.clone(),
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
        if let Some(to) = &step.on_failure {
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

pub const DEFAULT_PLAN_PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/workflow/step_plan.txt"
))
.trim_ascii_end();
pub const DEFAULT_DEVELOP_PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/workflow/step_develop.txt"
))
.trim_ascii_end();
pub const DEFAULT_REVIEW_PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/workflow/step_review.txt"
))
.trim_ascii_end();
pub const DEFAULT_MERGE_PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/workflow/step_merge.txt"
))
.trim_ascii_end();

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
        scope: WorkflowScope::Project,
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
        env: BTreeMap::new(),
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
                    ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
            },
            Step {
                id: "move:develop".into(),
                name: Some("Move to Review".into()),
                uses: "card.move".into(),
                with: BTreeMap::from([("column".into(), json!("review"))]),
                next: Some("condition:external_pr".into()),
                ..Default::default()
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
                ..Default::default()
            },
            Step {
                id: "deliver:review".into(),
                name: Some("Queue merge".into()),
                uses: "deliver".into(),
                with: BTreeMap::from([("sync_upstream".into(), json!(true))]),
                next: Some("__end".into()),
                on_changes_requested: Some("agent:develop".into()),
                ..Default::default()
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
                ..Default::default()
            },
            Step {
                id: "__end".into(),
                uses: "complete".into(),
                ..Default::default()
            },
        ],
        workspace,
        max_steps: 100,
    }
}

fn is_project_id_suffix(s: &str) -> bool {
    matches!(s.len(), 8 | 32) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn base_workflow_id(raw_id: &str, filename: &str) -> String {
    let trimmed = raw_id.trim();
    if let Some(rest) = trimmed.strip_prefix("delivery:")
        && (rest.is_empty() || is_project_id_suffix(rest) || filename == "development.yaml")
    {
        return "delivery".into();
    }
    if filename == "development.yaml" && trimmed == "delivery" {
        return "delivery".into();
    }
    let mut cur = trimmed;
    while let Some((head, tail)) = cur.rsplit_once(':') {
        if !head.is_empty() && is_project_id_suffix(tail) {
            cur = head;
        } else {
            break;
        }
    }
    if cur.is_empty() {
        filename
            .strip_suffix(".yaml")
            .unwrap_or("workflow")
            .to_owned()
    } else {
        cur.to_owned()
    }
}

/// Rewrites `doc.id`, clears project-specific rule selectors, and strips missing Profile
/// references so a reusable workflow collection can be published cleanly onto `project_id`.
pub fn instantiate_collection(
    project_id: &str,
    files: &[WorkflowTemplateFile],
    profiles: &[String],
    explicit_workspace: Option<&WorkspaceSettings>,
) -> Result<Vec<WorkflowTemplateFile>> {
    ensure!(
        !files.is_empty() && files.len() <= 64,
        "A Project needs 1..64 Workflow sources"
    );
    let mut sorted: Vec<&WorkflowTemplateFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.filename.cmp(&b.filename));
    let primary_idx = sorted
        .iter()
        .position(|f| f.filename == "development.yaml")
        .unwrap_or(0);
    let mut used_ids = BTreeSet::new();
    let mut out = Vec::with_capacity(sorted.len());
    for (idx, file) in sorted.into_iter().enumerate() {
        let mut doc = parse(&file.source).with_context(|| file.filename.clone())?;
        let base = base_workflow_id(&doc.id, &file.filename);
        let candidate_prefix = if base == "delivery" {
            format!("delivery:{project_id}")
        } else {
            let max_base = 200usize.saturating_sub(project_id.len() + 4);
            let truncated = if base.len() > max_base {
                &base[..max_base]
            } else {
                base.as_str()
            };
            format!("{truncated}:{project_id}")
        };
        let mut next_id = candidate_prefix.clone();
        let mut suffix = 2usize;
        while !used_ids.insert(next_id.clone()) {
            next_id = format!("{candidate_prefix}-{suffix}");
            suffix += 1;
        }
        doc.id = next_id;
        for rule in doc.on.values_mut() {
            rule.selector.project_id = None;
        }
        for step in &mut doc.steps {
            if matches!(step.kind(), "agent" | "deliver")
                && step
                    .with
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| !id.is_empty() && !profiles.iter().any(|p| p == id))
            {
                step.with.remove("profile");
            }
        }
        if idx == primary_idx
            && let Some(ws) = explicit_workspace
        {
            doc.workspace = ws.clone();
        }
        out.push(WorkflowTemplateFile {
            filename: file.filename.clone(),
            source: encode(&doc)?,
        });
    }
    Ok(out)
}

/// Canonicalizes a collection of YAML files for project-agnostic equality comparison
/// (normalizing `id` and clearing `rule.selector.project_id`).
pub fn normalized_collection_signature(
    files: &[WorkflowTemplateFile],
) -> Option<Vec<(String, Document)>> {
    if files.is_empty() {
        return None;
    }
    let mut normalized = Vec::with_capacity(files.len());
    for file in files {
        let mut doc = parse(&file.source).ok()?;
        doc.id = base_workflow_id(&doc.id, &file.filename);
        for rule in doc.on.values_mut() {
            rule.selector.project_id = None;
        }
        normalized.push((file.filename.clone(), doc));
    }
    normalized.sort_by(|a, b| a.0.cmp(&b.0));
    Some(normalized)
}

/// Human-readable one-line summary of a workflow source collection (columns + file count).
pub fn summarize_collection(files: &[WorkflowTemplateFile]) -> String {
    let mut docs = Vec::new();
    for f in files {
        if let Ok(doc) = parse(&f.source) {
            docs.push((f.filename.as_str(), doc));
        }
    }
    let columns: Vec<String> = docs
        .iter()
        .find(|(name, doc)| *name == "development.yaml" && !doc.columns.is_empty())
        .or_else(|| docs.iter().find(|(_, doc)| !doc.columns.is_empty()))
        .map(|(_, doc)| doc.columns.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_else(|| vec!["Backlog".into(), "Develop".into(), "Review".into()]);
    let stages = columns.join(" → ");
    if files.len() > 1 {
        format!("{stages} · {} workflows", files.len())
    } else {
        stages
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
    /// Queue Merge integration strategy (`rebase` default, `squash`, or forward `merge`).
    pub merge_strategy: MergeStrategy,
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
            merge_strategy: MergeStrategy::Rebase,
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
        uses: "complete".into(),
        ..Default::default()
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
        if s.on_failure.as_deref() == Some(target_id) {
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

    let merge_strategy = doc
        .steps
        .iter()
        .find(|s| s.uses == "deliver")
        .and_then(|s| s.with.get("strategy"))
        .and_then(|v| v.as_str())
        .and_then(MergeStrategy::parse)
        .unwrap_or_default();

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
        merge_strategy,
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
            if step.on_failure.as_deref() == Some(gate) {
                step.on_failure = Some(target.clone());
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
            next: Some(move_id),
            ..Default::default()
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
                ..Default::default()
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
            ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
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
                        ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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

/// Surgically updates `strategy` on all `deliver` steps in the workflow, omitting the key when
/// equal to the default [`MergeStrategy::Rebase`].
pub fn set_merge_strategy(doc: &mut Document, strategy: MergeStrategy) {
    let mut updated_any = false;
    for step in &mut doc.steps {
        if step.uses == "deliver" {
            if strategy == MergeStrategy::Rebase {
                step.with.remove("strategy");
            } else {
                step.with.insert("strategy".into(), json!(strategy));
            }
            updated_any = true;
        }
    }
    if !updated_any {
        set_review_manual(doc, false);
        for step in &mut doc.steps {
            if step.uses == "deliver" {
                if strategy == MergeStrategy::Rebase {
                    step.with.remove("strategy");
                } else {
                    step.with.insert("strategy".into(), json!(strategy));
                }
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
                    ..Default::default()
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
            ..Default::default()
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

/// Concise Board column-header automation summary + hover tooltip text derived from
/// active Project workflow sources, revisions, and column-scoped subscriptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnAutomation {
    /// Short pill label (e.g. `"Auto develop"`, `"Auto merge"`, `"Auto plan"`).
    pub label: String,
    /// Multi-line plain-text summary shown in the hover tooltip.
    pub tooltip: String,
}

fn rule_targets_column(key: &str, rule: &WorkflowRule, column: &str) -> bool {
    rule.enabled
        && rule.event != "manual"
        && (rule.selector.column.as_deref() == Some(column)
            || (rule.event == "card.column_entered"
                && rule.selector.column.is_none()
                && key == column))
}

fn column_display_name<'a>(column: &'a str, def: &'a WorkflowDefinition) -> &'a str {
    def.columns
        .iter()
        .find(|c| c.id == column)
        .map(|c| c.name.as_str())
        .unwrap_or(column)
}

fn agent_auto_phrase(step_id: &str, profile: Option<&str>, column: &str) -> String {
    let lower_step = step_id.to_ascii_lowercase();
    let lower_col = column.to_ascii_lowercase();
    let prof = profile.map(str::trim).filter(|p| !p.is_empty());
    if prof == Some("planner")
        || lower_step.contains("plan")
        || lower_step.contains("analyze")
        || lower_step.contains("design")
        || lower_col == "backlog"
        || lower_col == "design"
        || lower_col == "plan"
    {
        "Auto plan".into()
    } else if prof == Some("developer")
        || lower_step.contains("develop")
        || lower_step.contains("implement")
        || lower_step.contains("build")
        || lower_col == "develop"
    {
        "Auto develop".into()
    } else if prof == Some("reviewer") || lower_step.contains("review") || lower_col == "review" {
        "Auto review".into()
    } else if let Some(p) = prof {
        format!("Auto @{p}")
    } else {
        "Auto run".into()
    }
}

fn walk_stage_summary(
    column: &str,
    start_id: &str,
    def: &WorkflowDefinition,
) -> (Option<String>, Vec<String>) {
    let mut cur = Some(start_id);
    let mut seen = BTreeSet::new();
    let mut has_deliver = false;
    let mut first_agent: Option<(String, Option<String>)> = None;
    let mut has_script = false;
    let mut moves_to: Option<String> = None;
    let mut has_notify = false;
    let mut details = Vec::new();

    while let Some(step_id) = cur {
        if !seen.insert(step_id) {
            break;
        }
        let Some(step) = def.steps.get(step_id) else {
            break;
        };
        match step {
            WorkflowStep::Complete => break,
            WorkflowStep::Condition {
                pointer,
                equals,
                then,
                otherwise,
            } => {
                if pointer == "/card/source_trust" && equals == "external_pr" {
                    if let Some(WorkflowStep::Approval { .. }) = def.steps.get(then) {
                        details.push("• External PR → Requires human approval".into());
                    } else {
                        details.push(format!("• External PR → {then}"));
                    }
                    cur = Some(otherwise.as_str());
                } else {
                    details.push(format!("• Condition ({pointer}) → {otherwise}"));
                    cur = Some(otherwise.as_str());
                }
            }
            WorkflowStep::Admit {
                agent_slot,
                max_ready,
                next,
            } => {
                let gate = match (*agent_slot, *max_ready) {
                    (true, Some(k)) => {
                        format!("Waits for a free Agent slot (keeps ≤{k} ready)")
                    }
                    (true, None) => "Waits for a free Agent slot".into(),
                    (false, Some(k)) => format!("Plans ahead (keeps ≤{k} ready)"),
                    (false, None) => "Waits for admission".into(),
                };
                details.push(format!("• {gate}"));
                cur = next.as_deref();
            }
            WorkflowStep::Agent { profile, next, .. } => {
                if first_agent.is_none() {
                    first_agent = Some((step_id.to_owned(), profile.clone()));
                }
                let who = profile
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .map(|p| format!("@{p}"))
                    .unwrap_or_else(|| "default Agent".into());
                details.push(format!("• Runs {who}"));
                cur = next.as_deref();
            }
            WorkflowStep::Script { run, next, .. } => {
                has_script = true;
                let preview = run
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("custom script");
                details.push(format!("• Runs script: {preview}"));
                cur = next.as_deref();
            }
            WorkflowStep::Deliver {
                profile,
                target_branch,
                strategy,
                sync_upstream,
                batch_size,
                test_command,
                max_conflict_attempts,
                ..
            } => {
                has_deliver = true;
                let branch = target_branch
                    .as_deref()
                    .filter(|b| !b.is_empty())
                    .unwrap_or("target branch");
                let strategy_desc = match strategy {
                    MergeStrategy::Rebase => "",
                    MergeStrategy::Squash => " (squash)",
                    MergeStrategy::Merge => " (merge commit)",
                };
                let sync_desc = if *sync_upstream {
                    " + syncs upstream"
                } else {
                    " (local only)"
                };
                let batch_desc = if *batch_size != canban_types::DEFAULT_MERGE_BATCH_SIZE {
                    format!(" · batch {batch_size}")
                } else {
                    String::new()
                };
                details.push(format!(
                    "• Queue merge{strategy_desc} into {branch}{sync_desc}{batch_desc}"
                ));
                if let Some(cmd) = test_command.as_deref().filter(|c| !c.trim().is_empty()) {
                    details.push(format!("• Batch test: `{cmd}`"));
                }
                if *max_conflict_attempts > 0 {
                    let merger = profile
                        .as_deref()
                        .filter(|p| !p.is_empty())
                        .unwrap_or("merger");
                    details.push(format!(
                        "• Merge conflicts → @{merger} (up to {max_conflict_attempts}×)"
                    ));
                }
                break;
            }
            WorkflowStep::Move { column: to, .. } => {
                if to != column && !to.is_empty() {
                    moves_to = Some(to.clone());
                    let dest = column_display_name(to, def);
                    details.push(format!("• Moves Card to {dest}"));
                }
                break;
            }
            WorkflowStep::Approval { title, .. } => {
                if has_deliver || first_agent.is_some() || moves_to.is_some() || has_notify {
                    details.push(format!("• Pauses for approval: {title}"));
                }
                break;
            }
            WorkflowStep::Notify { message, next } => {
                has_notify = true;
                details.push(format!("• Posts notification: {message}"));
                cur = next.as_deref();
            }
        }
    }

    let label = if has_deliver {
        Some("Auto merge".into())
    } else if let Some((step_id, profile)) = first_agent {
        Some(agent_auto_phrase(&step_id, profile.as_deref(), column))
    } else if has_script {
        Some("Auto script".into())
    } else if moves_to.is_some() {
        Some(if column == "backlog" {
            "Auto start".into()
        } else {
            "Auto move".into()
        })
    } else if has_notify {
        Some("Auto notify".into())
    } else {
        None
    };

    if label.is_none() {
        details.clear();
    }
    (label, details)
}

pub fn column_automation(
    column: &str,
    files: &[WorkflowSource],
    revisions: &[WorkflowRevision],
    subscriptions: &[EventSubscription],
) -> Option<ColumnAutomation> {
    let mut ordered: Vec<&WorkflowSource> = files.iter().filter(|f| f.enabled).collect();
    ordered.sort_by_key(|f| {
        (
            f.filename != "development.yaml",
            f.columns.is_empty(),
            f.filename.as_str(),
        )
    });

    let mut primary_label: Option<String> = None;
    let mut extra_count: usize = 0;
    let mut blocks: Vec<String> = Vec::new();

    for f in ordered {
        let Some(w) = revisions
            .iter()
            .find(|w| w.workflow_id == f.workflow_id && w.revision == f.revision)
        else {
            continue;
        };
        let def = &w.definition;
        let mut wf_lines: Vec<String> = Vec::new();
        let mut wf_has_auto = false;

        // Prioritize `card.column_entered` rules first.
        let mut rules: Vec<(&String, &WorkflowRule)> = f
            .rules
            .iter()
            .filter(|(k, r)| rule_targets_column(k, r, column))
            .collect();
        rules.sort_by_key(|(k, r)| (r.event != "card.column_entered", k.as_str()));

        for (key, rule) in rules {
            let start_id = def
                .entries
                .get(key.as_str())
                .map(String::as_str)
                .unwrap_or(rule.start_at.as_str());
            if rule.event == "card.column_entered" {
                let (lbl, mut step_lines) = walk_stage_summary(column, start_id, def);
                if let Some(lbl) = lbl {
                    if primary_label.is_none() {
                        primary_label = Some(lbl);
                    } else if wf_lines.is_empty() {
                        extra_count += 1;
                    }
                    wf_has_auto = true;
                    if let Some(kind) = &rule.selector.source_kind {
                        wf_lines.push(format!("• Only for source: {kind}"));
                    }
                    wf_lines.append(&mut step_lines);
                }
            } else {
                let (lbl, _) = walk_stage_summary(column, start_id, def);
                if lbl.is_some() {
                    if primary_label.is_none() {
                        primary_label = lbl;
                    } else if wf_lines.is_empty() {
                        extra_count += 1;
                    }
                    wf_has_auto = true;
                    let ev = match rule.event.as_str() {
                        "card.column_left" => "Leaving column",
                        other => other,
                    };
                    wf_lines.push(format!("• On {ev} → {start_id}"));
                }
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
                let start_id = def
                    .entries
                    .get(entry.as_str())
                    .map(String::as_str)
                    .unwrap_or(entry.as_str());
                let (lbl, _) = walk_stage_summary(column, start_id, def);
                let fallback = lbl.unwrap_or_else(|| "Auto".into());
                if primary_label.is_none() {
                    primary_label = Some(fallback);
                } else if !wf_has_auto {
                    extra_count += 1;
                }
                wf_has_auto = true;
                wf_lines.push(format!("• On {} → {entry}", sub.selector.types.join(", ")));
            }
        }

        if wf_has_auto && !wf_lines.is_empty() {
            blocks.push(format!(
                "{} ({})\n{}",
                def.name,
                f.filename,
                wf_lines.join("\n")
            ));
        }
    }

    let mut label = primary_label?;
    if extra_count > 0 {
        label.push_str(&format!(" +{extra_count}"));
    }
    Some(ColumnAutomation {
        label,
        tooltip: blocks.join("\n\n"),
    })
}

pub fn column_summary(
    column: &str,
    files: &[WorkflowSource],
    revisions: &[WorkflowRevision],
) -> Option<String> {
    column_automation(column, files, revisions, &[]).map(|a| a.label)
}

/// Validates scheduled `on:` rules against the compiled `WorkflowDefinition` and Board columns.
pub fn validate_schedules(
    doc: &Document,
    flow: &WorkflowDefinition,
    effective_columns: &[String],
) -> Result<()> {
    if doc.scope.is_global() {
        ensure!(
            doc.columns.is_empty(),
            "Global workflows cannot declare Board `columns`"
        );
        for (rule_id, rule) in &doc.on {
            ensure!(
                rule.is_schedule() || rule.event == "manual",
                "Global workflow rule `{rule_id}` only supports `schedule` or `manual` triggers"
            );
            ensure!(
                rule.card.is_none(),
                "Global scheduled rule `{rule_id}` cannot configure `card:`; choose a Project to create Board Cards"
            );
        }
    }
    for (rule_id, rule) in &doc.on {
        if !rule.is_schedule() {
            ensure!(
                rule.cron.is_none()
                    && rule.timezone.is_none()
                    && rule.overlap == ScheduleOverlap::Skip
                    && rule.card.is_none(),
                "Rule `{rule_id}` ({}) cannot define schedule-only fields (`cron`, `timezone`, `overlap`, `card`)",
                rule.event
            );
            continue;
        }
        let cron = rule
            .cron
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .with_context(|| format!("Scheduled rule `{rule_id}` requires a non-empty `cron`"))?;
        canban_types::cron::parse_schedule(cron, rule.timezone.as_deref())
            .map_err(|e| anyhow::anyhow!("Invalid schedule in `{rule_id}`: {e}"))?;
        ensure!(
            rule.selector == EventSelector::default(),
            "Scheduled rule `{rule_id}` does not accept `match` selectors"
        );
        let start_id = flow
            .entries
            .get(rule_id)
            .with_context(|| format!("Scheduled rule `{rule_id}` is missing an entry step"))?;

        if let Some(card_cfg) = &rule.card {
            if let Some(col) = card_cfg.column.as_deref().filter(|c| !c.trim().is_empty()) {
                ensure!(
                    effective_columns.iter().any(|c| c == col),
                    "Scheduled rule `{rule_id}` targets unknown Card column `{col}`"
                );
            }
        } else {
            // Cardless Terminal Agent mode: walk reachable steps from `start_id` and ensure only
            // `script`, `agent` (at most one terminal Agent step), and `complete` are used.
            let mut cur = Some(start_id.as_str());
            let mut seen = BTreeSet::new();
            let mut agent_count = 0usize;
            while let Some(step_id) = cur {
                if !seen.insert(step_id) {
                    break;
                }
                let Some(step) = flow.steps.get(step_id) else {
                    break;
                };
                match step {
                    WorkflowStep::Complete => break,
                    WorkflowStep::Script {
                        next, on_failure, ..
                    } => {
                        if let Some(fail_to) = on_failure {
                            ensure!(
                                matches!(flow.steps.get(fail_to), Some(WorkflowStep::Agent { .. })),
                                "Cardless schedule `{rule_id}` script `on_failure` must target an `agent` step (or configure `card:`)"
                            );
                        }
                        cur = next.as_deref();
                    }
                    WorkflowStep::Agent { next, .. } => {
                        agent_count += 1;
                        if let Some(next_id) = next
                            && !matches!(flow.steps.get(next_id), Some(WorkflowStep::Complete))
                        {
                            bail!(
                                "Cardless schedule `{rule_id}` runs a single Terminal Agent session (`{step_id}`) and cannot continue to `{next_id}`; add `card:` to run multi-step Card pipelines"
                            );
                        }
                        break;
                    }
                    other => {
                        let kind = match other {
                            WorkflowStep::Move { .. } => "card.move",
                            WorkflowStep::Deliver { .. } => "deliver",
                            WorkflowStep::Approval { .. } => "approval",
                            WorkflowStep::Admit { .. } => "admit",
                            WorkflowStep::Condition { .. } => "condition",
                            WorkflowStep::Notify { .. } => "notify",
                            _ => "step",
                        };
                        bail!(
                            "Cardless schedule `{rule_id}` cannot execute `{kind}` step `{step_id}`; configure `card:` on the schedule rule to create a Card for each run"
                        );
                    }
                }
            }
            ensure!(
                agent_count > 0
                    || seen
                        .iter()
                        .any(|id| matches!(flow.steps.get(*id), Some(WorkflowStep::Script { .. }))),
                "Cardless schedule `{rule_id}` needs an `agent` or `script` step at `{start_id}`"
            );
        }
    }
    Ok(())
}

/// Returns the first `agent` step profile reachable from `start_at` in `def`, if any.
pub fn schedule_agent_profile(def: &WorkflowDefinition, rule_id: &str) -> Option<String> {
    let mut cur = def.entries.get(rule_id).map(String::as_str);
    let mut seen = BTreeSet::new();
    while let Some(step_id) = cur {
        if !seen.insert(step_id) {
            break;
        }
        match def.steps.get(step_id)? {
            WorkflowStep::Agent { profile, .. } => return profile.clone(),
            WorkflowStep::Script {
                next, on_failure, ..
            } => {
                if let Some(next_id) = next {
                    cur = Some(next_id.as_str());
                } else if let Some(fail_id) = on_failure {
                    cur = Some(fail_id.as_str());
                } else {
                    break;
                }
            }
            other => cur = other.next(),
        }
    }
    None
}

/// Returns `true` when `rule_id` in `def` starts directly at a `WorkflowStep::Script` step.
pub fn schedule_starts_with_script(def: &WorkflowDefinition, rule_id: &str) -> bool {
    def.entries
        .get(rule_id)
        .and_then(|sid| def.steps.get(sid))
        .is_some_and(|s| matches!(s, WorkflowStep::Script { .. }))
}

/// Execution mode template for creating a new scheduled workflow document.
#[derive(Clone, Debug)]
pub enum ScheduledWorkflowMode {
    ScriptOnly {
        run: String,
        shell: Option<String>,
        working_directory: Option<String>,
        timeout_minutes: Option<u32>,
    },
    ScriptWithAgentFallback {
        run: String,
        shell: Option<String>,
        working_directory: Option<String>,
        timeout_minutes: Option<u32>,
        profile: Option<String>,
        instruction: String,
    },
    AgentOnly {
        profile: Option<String>,
        instruction: String,
    },
    CreateCard {
        column: Option<String>,
        title: Option<String>,
        body: Option<String>,
        profile: Option<String>,
        instruction: String,
    },
}

#[derive(Clone, Debug)]
pub struct ScheduledWorkflowSpec {
    pub workflow_id: String,
    pub name: String,
    pub scope: WorkflowScope,
    pub cron: String,
    pub timezone: Option<String>,
    pub overlap: ScheduleOverlap,
    pub enabled: bool,
    pub mode: ScheduledWorkflowMode,
}

/// Builds a scheduled workflow YAML [`Document`] supporting all 4 execution modes and Global/Project scope.
pub fn build_scheduled_workflow_document(spec: ScheduledWorkflowSpec) -> Document {
    let mut steps = Vec::new();
    let mut card_cfg = None;
    let start_at;

    match spec.mode {
        ScheduledWorkflowMode::ScriptOnly {
            run,
            shell,
            working_directory,
            timeout_minutes,
        } => {
            start_at = "run_script".to_owned();
            steps.push(Step {
                id: "run_script".into(),
                name: Some("Run Script".into()),
                uses: "script".into(),
                run: Some(run),
                shell: shell.filter(|s| !s.trim().is_empty()),
                working_directory: working_directory.filter(|s| !s.trim().is_empty()),
                timeout_minutes,
                next: Some("__end".into()),
                ..Default::default()
            });
        }
        ScheduledWorkflowMode::ScriptWithAgentFallback {
            run,
            shell,
            working_directory,
            timeout_minutes,
            profile,
            instruction,
        } => {
            start_at = "run_script".to_owned();
            steps.push(Step {
                id: "run_script".into(),
                name: Some("Check Script".into()),
                uses: "script".into(),
                run: Some(run),
                shell: shell.filter(|s| !s.trim().is_empty()),
                working_directory: working_directory.filter(|s| !s.trim().is_empty()),
                timeout_minutes,
                next: Some("__end".into()),
                on_failure: Some("ask_agent".into()),
                ..Default::default()
            });
            let mut with = BTreeMap::new();
            if let Some(p) = profile.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                with.insert("profile".into(), json!(p));
            }
            let inst = if instruction.trim().is_empty() {
                "The scheduled script failed or flagged items needing review. Inspect the preceding script output and environment, resolve safe items, or report `--state blocked` to ask the user in this terminal.".to_owned()
            } else {
                instruction
            };
            with.insert("instruction".into(), json!(inst));
            steps.push(Step {
                id: "ask_agent".into(),
                name: Some("Inspect & Ask User".into()),
                uses: "agent".into(),
                with,
                next: Some("__end".into()),
                ..Default::default()
            });
        }
        ScheduledWorkflowMode::AgentOnly {
            profile,
            instruction,
        } => {
            start_at = "agent:run".to_owned();
            let mut with = BTreeMap::new();
            if let Some(p) = profile.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                with.insert("profile".into(), json!(p));
            }
            let inst = if instruction.trim().is_empty() {
                "Inspect the workspace health, run relevant checks, and summarize findings in your report.".to_owned()
            } else {
                instruction
            };
            with.insert("instruction".into(), json!(inst));
            steps.push(Step {
                id: "agent:run".into(),
                name: Some("Scheduled Agent".into()),
                uses: "agent".into(),
                with,
                next: Some("__end".into()),
                ..Default::default()
            });
        }
        ScheduledWorkflowMode::CreateCard {
            column,
            title,
            body,
            profile,
            instruction,
        } => {
            start_at = "agent:run".to_owned();
            card_cfg = Some(ScheduleCardConfig {
                column: column.filter(|s| !s.trim().is_empty()),
                title: title.filter(|s| !s.trim().is_empty()),
                body: body.filter(|s| !s.trim().is_empty()),
            });
            let mut with = BTreeMap::new();
            if let Some(p) = profile.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                with.insert("profile".into(), json!(p));
            }
            let inst = if instruction.trim().is_empty() {
                "Evaluate and complete this scheduled Card task, then report your outcome."
                    .to_owned()
            } else {
                instruction
            };
            with.insert("instruction".into(), json!(inst));
            steps.push(Step {
                id: "agent:run".into(),
                name: Some("Scheduled Card Agent".into()),
                uses: "agent".into(),
                with,
                next: Some("__end".into()),
                ..Default::default()
            });
        }
    }
    steps.push(Step {
        id: "__end".into(),
        uses: "complete".into(),
        ..Default::default()
    });

    Document {
        schema: crate::migrations::CURRENT,
        id: spec.workflow_id,
        name: spec.name,
        scope: spec.scope,
        enabled: true,
        columns: Vec::new(),
        env: BTreeMap::new(),
        on: BTreeMap::from([(
            "schedule".into(),
            WorkflowRule {
                event: "schedule".into(),
                start_at,
                cron: Some(spec.cron),
                timezone: spec.timezone.filter(|s| !s.trim().is_empty()),
                overlap: spec.overlap,
                card: card_cfg,
                enabled: spec.enabled,
                ..Default::default()
            },
        )]),
        steps,
        workspace: WorkspaceSettings::default(),
        max_steps: 100,
    }
}

/// Builds a fresh standalone scheduled workflow YAML document.
pub fn default_scheduled_document(
    workflow_id: impl Into<String>,
    name: impl Into<String>,
    profile: Option<&str>,
) -> Document {
    build_scheduled_workflow_document(ScheduledWorkflowSpec {
        workflow_id: workflow_id.into(),
        name: name.into(),
        scope: WorkflowScope::Project,
        cron: "0 9 * * 1-5".into(),
        timezone: None,
        overlap: ScheduleOverlap::Skip,
        enabled: true,
        mode: ScheduledWorkflowMode::AgentOnly {
            profile: profile.map(str::to_owned),
            instruction: "Inspect the repository health, run relevant checks, and summarize findings in your report.".into(),
        },
    })
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
                merge_strategy: MergeStrategy::Rebase,
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

    #[test]
    fn column_automation_derives_concise_labels_tooltips_and_hides_manual_columns() {
        let make_pair = |filename: &str, doc: &Document| -> (WorkflowSource, WorkflowRevision) {
            let def = doc.compile().unwrap();
            let rev_id = format!("{}:1", def.id);
            let src = WorkflowSource {
                id: format!("proj:{filename}"),
                project_id: "proj".into(),
                filename: filename.into(),
                workflow_id: def.id.clone(),
                revision: 1,
                hash: "h".into(),
                revision_id: rev_id.clone(),
                enabled: doc.enabled,
                columns: doc.columns.clone(),
                rules: doc.on.clone(),
            };
            let rev = WorkflowRevision {
                source_yaml: Some(encode(doc).unwrap()),
                id: rev_id,
                project_id: "proj".into(),
                workflow_id: def.id.clone(),
                revision: 1,
                definition: def,
                created_at: 0,
            };
            (src, rev)
        };

        // 1. Default workflow: Backlog quiet (`None`), Develop -> `Auto develop`, Review -> `Auto merge`.
        let mut doc = default_document("delivery:test", WorkspaceSettings::default());
        let (s0, r0) = make_pair("development.yaml", &doc);
        let srcs0 = [s0];
        let revs0 = [r0];
        assert_eq!(column_automation("backlog", &srcs0, &revs0, &[]), None);
        let dev = column_automation("develop", &srcs0, &revs0, &[]).unwrap();
        assert_eq!(dev.label, "Auto develop");
        assert!(dev.tooltip.contains("Development (development.yaml)"));
        assert!(dev.tooltip.contains("Runs @developer"));
        assert!(dev.tooltip.contains("Moves Card to Review"));
        let rev = column_automation("review", &srcs0, &revs0, &[]).unwrap();
        assert_eq!(rev.label, "Auto merge");
        assert!(
            rev.tooltip
                .contains("External PR → Requires human approval")
        );
        assert!(
            rev.tooltip
                .contains("Queue merge into target branch + syncs upstream")
        );
        assert!(rev.tooltip.contains("Merge conflicts → @merger (up to 3×)"));

        // 2. Backlog auto-plan + admission gate, and Review switched to manual approval (must hide!).
        set_backlog_options(&mut doc, true, true);
        set_backlog_admission(&mut doc, true, Some(2));
        set_review_manual(&mut doc, true);
        let (s1, r1) = make_pair("development.yaml", &doc);
        let srcs1 = [s1];
        let revs1 = [r1];
        let backlog = column_automation("backlog", &srcs1, &revs1, &[]).unwrap();
        assert_eq!(backlog.label, "Auto plan");
        assert!(backlog.tooltip.contains("Plans ahead (keeps ≤2 ready)"));
        assert!(backlog.tooltip.contains("Runs @developer"));
        assert!(backlog.tooltip.contains("Waits for a free Agent slot"));
        assert!(backlog.tooltip.contains("Moves Card to Develop"));
        assert_eq!(
            column_automation("review", &srcs1, &revs1, &[]),
            None,
            "approval-only manual review columns must stay blank"
        );

        // 3. Backlog auto-start without planning (`Auto start`), and local-only merge (`Auto merge`).
        set_backlog_options(&mut doc, false, true);
        set_review_manual(&mut doc, false);
        set_sync_upstream(&mut doc, false);
        let (s2, r2) = make_pair("development.yaml", &doc);
        let srcs2 = [s2];
        let revs2 = [r2];
        let backlog_start = column_automation("backlog", &srcs2, &revs2, &[]).unwrap();
        assert_eq!(backlog_start.label, "Auto start");
        let rev_local = column_automation("review", &srcs2, &revs2, &[]).unwrap();
        assert_eq!(rev_local.label, "Auto merge");
        assert!(rev_local.tooltip.contains("(local only)"));

        // 4. Custom QA column (`@qa` -> `Auto @qa`), review agent without deliver (`Auto review`),
        // disabled steps skipped, and multiple workflows (`+1` suffix + combined tooltip).
        let custom = parse(
            r#"schema: 2
id: custom
name: Custom Pipeline
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: qa
    name: QA
  - id: review
    name: Review
on:
  qa:
    event: card.column_entered
    start_at: skip_me
    match:
      column: qa
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: skip_me
    uses: approval
    enabled: false
    with:
      title: Disabled gate
    next: qa_run
  - id: qa_run
    uses: agent
    with:
      profile: qa
      instruction: Run QA checks
    next: move_review
  - id: move_review
    uses: card.move
    with:
      column: review
    next: $entry
  - id: agent:review
    uses: agent
    with:
      profile: reviewer
    next: final_gate
  - id: final_gate
    uses: approval
    with:
      title: Final sign-off
"#,
        )
        .unwrap();
        let extra = parse(
            r#"schema: 2
id: extra-qa
name: Extra QA Notifier
on:
  qa_ping:
    event: card.column_entered
    start_at: ping
    match:
      column: qa
steps:
  - id: ping
    uses: notify
    with:
      message: Entered QA
"#,
        )
        .unwrap();
        let (sc, rc) = make_pair("development.yaml", &custom);
        let (se, re) = make_pair("extra.yaml", &extra);
        let qa = column_automation("qa", &[se, sc.clone()], &[re, rc.clone()], &[]).unwrap();
        assert_eq!(qa.label, "Auto @qa +1");
        assert!(qa.tooltip.contains("Custom Pipeline (development.yaml)"));
        assert!(qa.tooltip.contains("Extra QA Notifier (extra.yaml)"));
        let rev_agent = column_automation("review", &[sc], &[rc], &[]).unwrap();
        assert_eq!(rev_agent.label, "Auto review");
        assert!(rev_agent.tooltip.contains("Runs @reviewer"));
        assert!(
            rev_agent
                .tooltip
                .contains("Pauses for approval: Final sign-off")
        );
    }

    #[test]
    fn compiles_run_shorthand_and_uses_script_equivalently_with_env() {
        let yaml = r#"schema: 6
id: script:demo
name: Script Demo
env:
  GLOBAL_FLAG: 1
  LOG_LEVEL: info
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:develop
    uses: agent
    env:
      AGENT_ONLY: "yes"
    next: test:shorthand
  - id: test:shorthand
    run: cargo test --lib
    shell: bash
    working_directory: crates/app
    timeout_minutes: 5
    continue_on_error: true
    env:
      STEP_FLAG: enabled
    on_failure: agent:develop
    next: test:explicit
  - id: test:explicit
    uses: script
    with:
      run: echo ok
      shell: zsh
      timeout_seconds: 30
    next: __end
  - id: __end
    uses: complete
"#;
        let doc = parse(yaml).unwrap();
        assert_eq!(doc.env.get("GLOBAL_FLAG").map(String::as_str), Some("1"));
        let def = doc.compile().unwrap();
        assert_eq!(def.env.get("LOG_LEVEL").map(String::as_str), Some("info"));
        assert!(matches!(
            &def.steps["agent:develop"],
            WorkflowStep::Agent { env, .. } if env.get("AGENT_ONLY").map(String::as_str) == Some("yes")
        ));
        assert_eq!(
            def.steps["test:shorthand"],
            WorkflowStep::Script {
                run: "cargo test --lib".into(),
                shell: Some("bash".into()),
                working_directory: Some("crates/app".into()),
                timeout_minutes: Some(5),
                timeout_seconds: None,
                continue_on_error: true,
                env: BTreeMap::from([("STEP_FLAG".into(), "enabled".into())]),
                next: Some("test:explicit".into()),
                on_failure: Some("agent:develop".into()),
            }
        );
        assert_eq!(
            def.steps["test:explicit"],
            WorkflowStep::Script {
                run: "echo ok".into(),
                shell: Some("zsh".into()),
                working_directory: None,
                timeout_minutes: None,
                timeout_seconds: Some(30),
                continue_on_error: false,
                env: BTreeMap::new(),
                next: Some("__end".into()),
                on_failure: None,
            }
        );
    }

    #[test]
    fn rejects_conflicting_run_or_script_fields_on_non_script_steps() {
        let conflicting = parse(
            r#"schema: 6
id: bad
name: Bad
on:
  develop:
    event: card.column_entered
    start_at: check
steps:
  - id: check
    uses: script
    run: echo a
    with:
      run: echo b
"#,
        )
        .unwrap();
        assert!(conflicting.compile().is_err());

        let misplaced = parse(
            r#"schema: 6
id: bad2
name: Bad2
on:
  develop:
    event: card.column_entered
    start_at: agent:dev
steps:
  - id: agent:dev
    uses: agent
    run: echo nope
"#,
        )
        .unwrap();
        assert!(misplaced.compile().is_err());
    }

    #[test]
    fn parses_named_and_github_actions_schedule_rules_and_validates_modes() {
        let gha = r#"schema: 8
id: nightly:audit
name: Nightly Audit
on:
  schedule:
    - cron: "0 2 * * *"
      timezone: Asia/Shanghai
steps:
  - id: precheck
    run: echo ok
  - id: agent:audit
    uses: agent
    with:
      profile: developer
      instruction: Audit repository health.
"#;
        let doc = parse(gha).unwrap();
        let rule = doc.on.get("schedule").expect("normalized schedule rule");
        assert_eq!(rule.event, "schedule");
        assert_eq!(rule.cron.as_deref(), Some("0 2 * * *"));
        assert_eq!(rule.timezone.as_deref(), Some("Asia/Shanghai"));
        assert_eq!(rule.start_at, "precheck");
        let flow = doc.compile().unwrap();
        validate_schedules(&doc, &flow, &["backlog".into(), "develop".into()]).unwrap();
        assert_eq!(
            schedule_agent_profile(&flow, "schedule").as_deref(),
            Some("developer")
        );

        // Cardless schedule cannot use `card.move` without `card:`.
        let invalid_cardless = r#"schema: 8
id: bad:sched
name: Bad Schedule
on:
  nightly:
    cron: "0 2 * * *"
    start_at: move:dev
steps:
  - id: move:dev
    uses: card.move
    with:
      column: develop
"#;
        let bad_doc = parse(invalid_cardless).unwrap();
        let bad_flow = bad_doc.compile().unwrap();
        assert!(
            validate_schedules(&bad_doc, &bad_flow, &["backlog".into(), "develop".into()]).is_err()
        );

        // Adding `card:` makes `card.move` valid!
        let valid_card = r#"schema: 8
id: ok:sched
name: Daily Card
on:
  nightly:
    cron: "0 2 * * *"
    start_at: move:dev
    card:
      column: backlog
      title: "Daily Triage · {{date}}"
steps:
  - id: move:dev
    uses: card.move
    with:
      column: develop
"#;
        let ok_doc = parse(valid_card).unwrap();
        let ok_flow = ok_doc.compile().unwrap();
        validate_schedules(&ok_doc, &ok_flow, &["backlog".into(), "develop".into()]).unwrap();
    }
}
