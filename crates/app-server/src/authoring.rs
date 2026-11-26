//! SQLite is the only Workflow source of truth. Original YAML, immutable
//! revisions, active references and subscriptions commit in one transaction.
use super::*;
use canban_workflow_engine::source;
use std::collections::BTreeSet;

fn filename(name: &str) -> Result<()> {
    ensure!(
        !name.starts_with('.')
            && name.len() <= 100
            && name.ends_with(".yaml")
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "Use a simple .yaml filename"
    );
    Ok(())
}
fn files(_home: &Path, db: Db<'_>, scope_id: &str) -> Result<Value> {
    let mut active: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|s| s.project_id == scope_id)
        .collect();
    active.sort_by(|a, b| a.filename.cmp(&b.filename));
    let mut rows = Vec::new();
    for item in &active {
        let revision: WorkflowRevision = db.get(&item.revision_id)?;
        let raw = revision.source_yaml.context("Published YAML is missing")?;
        ensure!(
            canban_context::digest(raw.as_bytes()) == item.hash,
            "Stored YAML digest mismatch"
        );
        rows.push(json!({"filename":item.filename,"source":raw,"hash":item.hash,"active":item}));
    }
    let revision = canban_context::digest(&serde_json::to_vec(&active)?);
    Ok(json!({"project_id":scope_id,"revision":revision,"sources":rows,"active":active}))
}
fn validate(
    db: Db<'_>,
    config: &Config,
    scope_id: &str,
    project_opt: Option<&Project>,
    inputs: &[Value],
) -> Result<Vec<(String, String, source::Document, WorkflowDefinition)>> {
    let is_global = project_opt.is_none();
    if is_global {
        ensure!(
            inputs.len() <= 64,
            "Global scope supports at most 64 Workflow sources"
        );
    } else {
        ensure!(
            !inputs.is_empty() && inputs.len() <= 64,
            "A Project needs 1..64 Workflow sources; use enabled: false to disable one"
        );
    }
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut parsed_docs = Vec::new();
    let mut declared_columns: Option<Vec<WorkflowColumn>> = None;
    for input in inputs {
        let name = strp(input, "filename")?;
        filename(name)?;
        ensure!(names.insert(name), "Duplicate filename {name}");
        let raw = strp(input, "source")?;
        let doc = source::parse(raw).with_context(|| name.to_owned())?;
        let canonical = canban_workflow_engine::migrations::upgrade_yaml(raw)
            .with_context(|| name.to_owned())?
            .unwrap_or_else(|| raw.to_owned());
        if !doc.columns.is_empty() && (declared_columns.is_none() || name == "development.yaml") {
            declared_columns = Some(doc.columns.clone());
        }
        parsed_docs.push((name.to_owned(), canonical, doc));
    }
    let effective_columns: Vec<String> = if is_global {
        Vec::new()
    } else {
        declared_columns
            .as_ref()
            .map(|cols| cols.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_else(|| {
                project_opt
                    .map(|p| {
                        p.workflow
                            .stages
                            .iter()
                            .map(|s| s.id.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
    };
    // Removed columns are allowed: their active Cards return to the first column on publish.
    let mut out = Vec::new();
    for (name, raw, doc) in parsed_docs {
        let parsed = (|| -> Result<_> {
            ensure!(
                doc.id.len() <= 200 && !doc.id.contains('/') && !doc.id.contains(".."),
                "Invalid workflow ID"
            );
            ensure!(
                ids.insert(doc.id.clone()),
                "Duplicate workflow ID {}",
                doc.id
            );
            if is_global {
                ensure!(
                    doc.scope.is_global(),
                    "Global workflows must declare `scope: global`"
                );
            } else {
                ensure!(
                    !doc.scope.is_global(),
                    "Global-scoped workflow (`scope: global`) must be published under Global scope (`__global`)"
                );
            }
            if let Ok(old) = super::workflow::published(db, &doc.id) {
                ensure!(
                    old.project_id == scope_id,
                    "Workflow ID belongs to another scope"
                );
            }
            for rule in doc.on.values() {
                ensure!(
                    rule.is_schedule() || !rule.event.is_empty(),
                    "Trigger event or cron expression is required"
                );
                ensure!(
                    rule.selector
                        .project_id
                        .as_deref()
                        .is_none_or(|id| id == scope_id),
                    "Trigger cannot target another Project"
                );
                ensure!(
                    rule.selector.types.is_empty(),
                    "Use event rather than match.types"
                );
                if let Some(column) = &rule.selector.column {
                    ensure!(
                        effective_columns.contains(column),
                        "Unknown trigger column {column}"
                    );
                }
            }
            let flow = doc.compile()?;
            source::validate_schedules(&doc, &flow, &effective_columns)?;
            canban_workflow_engine::validate_definition(
                &flow,
                &effective_columns,
                &config
                    .profiles
                    .iter()
                    .map(|p| p.id.clone())
                    .collect::<Vec<_>>(),
            )?;
            Ok((doc, flow))
        })()
        .with_context(|| name.clone())?;
        out.push((name, raw, parsed.0, parsed.1));
    }
    Ok(out)
}
pub(super) fn publish_collection(
    home: &Path,
    db: Db<'_>,
    project: &Project,
    config: &Config,
    inputs: &[Value],
    now_ms: u64,
) -> Result<()> {
    publish_scope(home, db, &project.id, Some(project), config, inputs, now_ms)
}

pub(super) fn publish_global_collection(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    inputs: &[Value],
    now_ms: u64,
) -> Result<()> {
    publish_scope(home, db, GLOBAL_PROJECT_ID, None, config, inputs, now_ms)
}

fn publish_scope(
    _home: &Path,
    db: Db<'_>,
    scope_id: &str,
    project_opt: Option<&Project>,
    config: &Config,
    inputs: &[Value],
    now_ms: u64,
) -> Result<()> {
    let compiled = validate(db, config, scope_id, project_opt, inputs)?;
    let old: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == scope_id)
        .collect();
    // Retiring a source disables future triggers; captured receipts/runs keep their revision.
    for mut sub in db
        .list::<EventSubscription>()?
        .into_iter()
        .filter(|s| s.id.starts_with(&format!("source:{scope_id}:")))
    {
        sub.enabled = false;
        db.save(&sub)?;
    }
    for (name, raw, doc, definition) in compiled {
        // Runs currently at a step deleted from the source finish it, then stop.
        let existing_ids: BTreeSet<&str> = doc.steps.iter().map(|s| s.id.as_str()).collect();
        for mut run in db.list::<WorkflowRun>()?.into_iter().filter(|r| {
            r.active()
                && r.workflow_id == definition.id
                && r.step.as_deref().is_some_and(|s| !existing_ids.contains(s))
                && r.inputs["stop_after_step"].is_null()
        }) {
            run.inputs["stop_after_step"] = json!(run.step);
            run.updated_at = now_ms;
            db.save(&run)?;
            if let Some(card_id) = run.card_id.as_deref() {
                let card: Card = db.get(card_id)?;
                log(
                    db,
                    &card,
                    "workflow.stop_scheduled",
                    "system",
                    "Current step was removed; the run stops after it finishes",
                    run.terminal_id.clone(),
                    json!({"workflow_run_id":run.id,"step":run.step}),
                    now_ms,
                )?;
            } else {
                db.append(canban_timeline::event_at(
                    &run.project_id,
                    "",
                    "workflow.stop_scheduled",
                    "system",
                    "Current step was removed; the run stops after it finishes",
                    json!({"workflow_run_id":run.id,"step":run.step}),
                    now_ms,
                ))?;
            }
        }
        let hash = canban_context::digest(raw.as_bytes());
        let existing = old
            .iter()
            .find(|f| f.filename == name && f.workflow_id == definition.id);
        let latest = super::workflow::published(db, &definition.id).ok();
        let revision = if existing.is_some_and(|f| f.hash == hash) {
            existing.unwrap().revision
        } else {
            latest.as_ref().map_or(1, |w| w.revision + 1)
        };
        if !latest.as_ref().is_some_and(|w| w.revision == revision) {
            db.save(&WorkflowRevision {
                host_id: String::new(),
                source_yaml: Some(raw.clone()),
                id: format!("{}:{revision}", definition.id),
                project_id: scope_id.to_owned(),
                workflow_id: definition.id.clone(),
                revision,
                definition: definition.clone(),
                created_at: now_ms,
            })?;
        }
        let file = WorkflowSource {
            host_id: String::new(),
            id: format!("{scope_id}:{name}"),
            project_id: scope_id.to_owned(),
            filename: name,
            workflow_id: definition.id.clone(),
            revision,
            hash,
            revision_id: format!("{}:{revision}", definition.id),
            enabled: doc.enabled,
            columns: doc.columns.clone(),
            rules: doc.on,
        };
        for (key, rule) in &file.rules {
            if rule.event == "manual" || rule.is_schedule() {
                continue;
            }
            let sub_id = format!("source:{scope_id}:{}:{key}", definition.id);
            let old_sub = db.get::<EventSubscription>(&sub_id).ok();
            let project_repo = project_opt
                .and_then(|p| p.github.as_ref())
                .map(|g| g.repository.as_str());
            let selector =
                canban_workflow_engine::compile_rule_selector(scope_id, rule, project_repo);
            let enabled = file.enabled && rule.enabled;
            db.save(&EventSubscription {
                id: sub_id,
                revision: old_sub.as_ref().map_or(1, |s| s.revision + 1),
                selector,
                target: SubscriptionTarget::Workflow {
                    workflow_id: definition.id.clone(),
                    entry: key.clone(),
                    card_id: None,
                },
                enabled,
                cursor: old_sub
                    .as_ref()
                    .map_or(super::events::highwater(db)?, |s| s.cursor),
                created_at: old_sub.map_or(now_ms, |s| s.created_at),
            })?;
        }
        db.save(&file)?;
    }
    let names: BTreeSet<_> = inputs
        .iter()
        .filter_map(|v| v["filename"].as_str())
        .collect();
    for f in old.iter().filter(|f| !names.contains(f.filename.as_str())) {
        // Historical YAML revisions and captured runs remain available in SQL.
        db.conn
            .execute("DELETE FROM workflow_sources WHERE id=?1", [&f.id])?;
    }
    let active_sources: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == scope_id)
        .collect();
    if let Some(project) = project_opt {
        // Synchronize declared YAML columns and primary workspace settings to Project.
        if let Some(primary) = active_sources
            .iter()
            .find(|f| f.filename == "development.yaml")
            .or_else(|| active_sources.iter().find(|f| !f.columns.is_empty()))
            .or_else(|| active_sources.first())
        {
            let mut updated: Project = db.get(&project.id)?;
            let mut changed = false;
            if let Ok(rev) = db.get::<WorkflowRevision>(&primary.revision_id)
                && updated.workflow.workspace != rev.definition.workspace
            {
                updated.workflow.workspace = rev.definition.workspace;
                changed = true;
            }
            if !primary.columns.is_empty() {
                let new_stages: Vec<Stage> = primary
                    .columns
                    .iter()
                    .map(|col| {
                        if let Some(mut existing) = updated
                            .workflow
                            .stages
                            .iter()
                            .find(|s| s.id == col.id)
                            .cloned()
                        {
                            existing.name = col.name.clone();
                            existing
                        } else {
                            Stage {
                                id: col.id.clone(),
                                name: col.name.clone(),
                                execute: false,
                                profile: None,
                                instruction: String::new(),
                                on_completed: None,
                                require_human_review: false,
                            }
                        }
                    })
                    .collect();
                if updated.workflow.stages != new_stages {
                    updated.workflow.stages = new_stages;
                    changed = true;
                }
            }
            if changed {
                db.save(&updated)?;
            }
            // Cards in removed columns return to the first column.
            if let Some(first) = primary.columns.first() {
                let ids: BTreeSet<_> = primary.columns.iter().map(|c| c.id.as_str()).collect();
                for mut card in db.list::<Card>()?.into_iter().filter(|c| {
                    c.project_id == project.id && c.active() && !ids.contains(c.stage.as_str())
                }) {
                    let from = std::mem::replace(&mut card.stage, first.id.clone());
                    card.board_order = None;
                    canban_kanban::order::place(db, &mut card, None)?;
                    card.revision += 1;
                    db.save(&card)?;
                    log(
                        db,
                        &card,
                        "card.stage_changed",
                        "system",
                        &format!("{from} → {} (column removed)", card.stage),
                        None,
                        json!({"from":from,"to":card.stage,"reason":"column_removed"}),
                        now_ms,
                    )?;
                }
            }
        }
        let updated_project: Project = db.get(&project.id)?;
        super::schedules::sync_scope_schedules(
            db,
            &updated_project.id,
            Some(&updated_project),
            &active_sources,
            now_ms,
        )?;
    } else {
        super::schedules::sync_scope_schedules(
            db,
            GLOBAL_PROJECT_ID,
            None,
            &active_sources,
            now_ms,
        )?;
    }
    Ok(())
}
/// How a newly created Project initializes its workflow source collection.
pub(super) enum NewProjectWorkflow<'a> {
    /// Use saved global default template (`workflow_templates` `"default"`) if configured,
    /// otherwise fall back to the built-in default `development.yaml`.
    Default,
    /// Always use the built-in default `development.yaml` even when a custom global default exists.
    Builtin,
    /// Explicit single-file `development.yaml` string (`project.create` `workflow_yaml`).
    ExplicitYaml(&'a str),
    /// Explicit multi-file workflow collection (`project.create` `workflow_sources`).
    ExplicitSources(Vec<WorkflowTemplateFile>),
    /// Clone an existing Project's active `WorkflowSource` collection.
    FromProject(String),
}

pub(super) fn scope_template_files(
    db: Db<'_>,
    scope_id: &str,
) -> Result<Vec<WorkflowTemplateFile>> {
    let mut active: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|s| s.project_id == scope_id)
        .collect();
    active.sort_by(|a, b| a.filename.cmp(&b.filename));
    let mut out = Vec::with_capacity(active.len());
    for item in active {
        let rev: WorkflowRevision = db.get(&item.revision_id)?;
        let raw = rev.source_yaml.context("Published YAML is missing")?;
        let canonical = canban_workflow_engine::migrations::upgrade_yaml(&raw)?.unwrap_or(raw);
        out.push(WorkflowTemplateFile {
            filename: item.filename,
            source: canonical,
        });
    }
    Ok(out)
}

pub(super) fn project_template_files(
    db: Db<'_>,
    project_id: &str,
) -> Result<Vec<WorkflowTemplateFile>> {
    let out = scope_template_files(db, project_id)?;
    ensure!(
        !out.is_empty(),
        "Source Project has no published workflow sources"
    );
    Ok(out)
}

/// Inspects the primary workspace remote for a new Project before `Project` creation
/// so `detect_project_github` uses the effective template/copied remote when `workspace` is not overridden.
pub(super) fn effective_initial_remote(
    db: Db<'_>,
    mode: &NewProjectWorkflow<'_>,
    explicit_workspace: Option<&WorkspaceSettings>,
) -> Result<String> {
    if let Some(ws) = explicit_workspace {
        return Ok(ws.remote.clone());
    }
    let files = match mode {
        NewProjectWorkflow::Default => db
            .get::<WorkflowTemplate>(DEFAULT_WORKFLOW_TEMPLATE_ID)
            .ok()
            .map(|t| t.sources),
        NewProjectWorkflow::Builtin => None,
        NewProjectWorkflow::ExplicitYaml(raw) => Some(vec![WorkflowTemplateFile {
            filename: "development.yaml".into(),
            source: (*raw).to_owned(),
        }]),
        NewProjectWorkflow::ExplicitSources(files) => Some(files.clone()),
        NewProjectWorkflow::FromProject(pid) => Some(project_template_files(db, pid)?),
    };
    if let Some(files) = files
        && let Some(primary) = files
            .iter()
            .find(|f| f.filename == "development.yaml")
            .or_else(|| files.first())
    {
        let doc = source::parse(&primary.source)?;
        return Ok(doc.workspace.remote);
    }
    Ok(WorkspaceSettings::default().remote)
}

fn validate_template_files(config: &Config, inputs: &[Value]) -> Result<Vec<WorkflowTemplateFile>> {
    ensure!(
        !inputs.is_empty() && inputs.len() <= 64,
        "A workflow template needs 1..64 Workflow sources"
    );
    let mut names = BTreeSet::new();
    let mut raw_files = Vec::with_capacity(inputs.len());
    for input in inputs {
        let name = strp(input, "filename")?;
        filename(name)?;
        ensure!(names.insert(name.to_owned()), "Duplicate filename {name}");
        let raw = strp(input, "source")?;
        let canonical = canban_workflow_engine::migrations::upgrade_yaml(raw)
            .with_context(|| name.to_owned())?
            .unwrap_or_else(|| raw.to_owned());
        raw_files.push(WorkflowTemplateFile {
            filename: name.to_owned(),
            source: canonical,
        });
    }
    let profiles: Vec<String> = config.profiles.iter().map(|p| p.id.clone()).collect();
    let instantiated =
        source::instantiate_collection("templatecheck", &raw_files, &profiles, None)?;
    let mut declared_columns: Option<Vec<WorkflowColumn>> = None;
    let mut parsed_docs = Vec::with_capacity(instantiated.len());
    for file in &instantiated {
        let doc = source::parse(&file.source).with_context(|| file.filename.clone())?;
        if !doc.columns.is_empty()
            && (declared_columns.is_none() || file.filename == "development.yaml")
        {
            declared_columns = Some(doc.columns.clone());
        }
        parsed_docs.push((file.filename.clone(), doc));
    }
    let effective_columns: Vec<String> = declared_columns
        .map(|cols| cols.into_iter().map(|c| c.id).collect())
        .unwrap_or_else(|| {
            Workflow::default()
                .stages
                .into_iter()
                .map(|s| s.id)
                .collect()
        });
    for (name, doc) in parsed_docs {
        (|| -> Result<()> {
            ensure!(
                doc.id.len() <= 200 && !doc.id.contains('/') && !doc.id.contains(".."),
                "Invalid workflow ID"
            );
            for rule in doc.on.values() {
                ensure!(
                    rule.is_schedule() || !rule.event.is_empty(),
                    "Trigger event or cron expression is required"
                );
                ensure!(
                    rule.selector.types.is_empty(),
                    "Use event rather than match.types"
                );
                if let Some(column) = &rule.selector.column {
                    ensure!(
                        effective_columns.contains(column),
                        "Unknown trigger column {column}"
                    );
                }
            }
            let flow = doc.compile()?;
            source::validate_schedules(&doc, &flow, &effective_columns)?;
            canban_workflow_engine::validate_definition(&flow, &effective_columns, &profiles)?;
            Ok(())
        })()
        .with_context(|| name)?;
    }
    raw_files.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(raw_files)
}

/// Initialize a newly created Project from explicit YAML, a copied Project,
/// the saved global default template, or the native built-in `development.yaml`.
pub(super) fn initialize_new(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
    mode: NewProjectWorkflow<'_>,
    explicit_workspace: Option<&WorkspaceSettings>,
    now_ms: u64,
) -> Result<()> {
    let profiles: Vec<String> = config.profiles.iter().map(|p| p.id.clone()).collect();
    let inputs: Vec<Value> = match mode {
        NewProjectWorkflow::ExplicitYaml(yaml) => {
            vec![json!({"filename":"development.yaml","source":yaml})]
        }
        NewProjectWorkflow::ExplicitSources(raw_files) => {
            source::instantiate_collection(&project.id, &raw_files, &profiles, explicit_workspace)?
                .into_iter()
                .map(|f| json!({"filename": f.filename, "source": f.source}))
                .collect()
        }
        NewProjectWorkflow::FromProject(source_pid) => {
            let raw_files = project_template_files(db, &source_pid)?;
            source::instantiate_collection(&project.id, &raw_files, &profiles, explicit_workspace)?
                .into_iter()
                .map(|f| json!({"filename": f.filename, "source": f.source}))
                .collect()
        }
        NewProjectWorkflow::Default => {
            if let Ok(tpl) = db.get::<WorkflowTemplate>(DEFAULT_WORKFLOW_TEMPLATE_ID)
                && !tpl.sources.is_empty()
            {
                source::instantiate_collection(
                    &project.id,
                    &tpl.sources,
                    &profiles,
                    explicit_workspace,
                )?
                .into_iter()
                .map(|f| json!({"filename": f.filename, "source": f.source}))
                .collect()
            } else {
                let mut doc = source::default_document(
                    format!("delivery:{}", project.id),
                    project.workflow.workspace.clone(),
                );
                strip_unknown_agent_steps(&mut doc, &profiles);
                vec![json!({"filename":"development.yaml","source":source::encode(&doc)?})]
            }
        }
        NewProjectWorkflow::Builtin => {
            let mut doc = source::default_document(
                format!("delivery:{}", project.id),
                project.workflow.workspace.clone(),
            );
            strip_unknown_agent_steps(&mut doc, &profiles);
            vec![json!({"filename":"development.yaml","source":source::encode(&doc)?})]
        }
    };
    publish_scope(
        home,
        db,
        &project.id,
        Some(project),
        config,
        &inputs,
        now_ms,
    )
}

fn strip_unknown_agent_steps(doc: &mut source::Document, profiles: &[String]) {
    for step in &mut doc.steps {
        if step.uses == "agent" {
            for key in ["default_agent", "override"] {
                if step
                    .with
                    .get(key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| !profiles.iter().any(|p| p == id))
                {
                    step.with.remove(key);
                }
            }
        }
    }
}

/// Migrate any active Global (`__global`) workflow sources on older schema versions to `CURRENT`
/// and synchronize global `ScheduleState` timers.
pub(super) fn initialize_global(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    now_ms: u64,
) -> Result<()> {
    let mut existing: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == GLOBAL_PROJECT_ID)
        .collect();
    if existing.is_empty() {
        super::schedules::sync_scope_schedules(db, GLOBAL_PROJECT_ID, None, &existing, now_ms)?;
        return Ok(());
    }
    existing.sort_by(|a, b| a.filename.cmp(&b.filename));
    let mut needs_upgrade = false;
    let mut inputs = Vec::with_capacity(existing.len());
    for item in &existing {
        let rev: WorkflowRevision = db.get(&item.revision_id)?;
        let raw = rev.source_yaml.context("Published YAML is missing")?;
        if let Some(upgraded) = canban_workflow_engine::migrations::upgrade_yaml(&raw)? {
            needs_upgrade = true;
            inputs.push(json!({"filename": item.filename, "source": upgraded}));
        } else {
            inputs.push(json!({"filename": item.filename, "source": raw}));
        }
    }
    if needs_upgrade {
        publish_scope(home, db, GLOBAL_PROJECT_ID, None, config, &inputs, now_ms)?;
    } else {
        super::schedules::sync_scope_schedules(db, GLOBAL_PROJECT_ID, None, &existing, now_ms)?;
    }
    Ok(())
}

/// Ensure every Project has an initialized SQL YAML workflow collection,
/// and migrate any active workflow sources on older schema versions to `CURRENT`.
pub(super) fn initialize(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
    now_ms: u64,
) -> Result<()> {
    let mut existing: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == project.id)
        .collect();
    if existing.is_empty() {
        return initialize_new(
            home,
            db,
            config,
            project,
            NewProjectWorkflow::Builtin,
            None,
            now_ms,
        );
    }
    existing.sort_by(|a, b| a.filename.cmp(&b.filename));
    let mut needs_upgrade = false;
    let mut inputs = Vec::with_capacity(existing.len());
    for item in &existing {
        let rev: WorkflowRevision = db.get(&item.revision_id)?;
        let raw = rev.source_yaml.context("Published YAML is missing")?;
        if let Some(upgraded) = canban_workflow_engine::migrations::upgrade_yaml(&raw)? {
            needs_upgrade = true;
            inputs.push(json!({"filename": item.filename, "source": upgraded}));
        } else {
            inputs.push(json!({"filename": item.filename, "source": raw}));
        }
    }
    if needs_upgrade {
        publish_scope(
            home,
            db,
            &project.id,
            Some(project),
            config,
            &inputs,
            now_ms,
        )?;
    } else {
        super::schedules::sync_scope_schedules(db, &project.id, Some(project), &existing, now_ms)?;
    }
    Ok(())
}

fn scope_from_params(db: Db<'_>, p: &Value) -> Result<(String, Option<Project>)> {
    if p["scope"]
        .as_str()
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("global"))
    {
        return Ok((GLOBAL_PROJECT_ID.into(), None));
    }
    let sid = super::resolve_workflow_scope(db, strp(p, "project_id")?)?;
    if is_global_project_id(&sid) {
        Ok((sid, None))
    } else {
        let proj: Project = db.get(&sid)?;
        Ok((sid, Some(proj)))
    }
}

impl AppServer {
    pub(super) fn authoring_call(&self, method: &str, p: Value) -> Result<Value> {
        if method == "workflow.default.get" {
            let store = self.store.lock().unwrap();
            let template = store
                .db()
                .get::<WorkflowTemplate>(DEFAULT_WORKFLOW_TEMPLATE_ID)
                .ok();
            return Ok(json!({"template": template}));
        }
        if method == "workflow.default.set" {
            ensure!(
                p["execution"].is_null(),
                "Card Agent executions cannot overwrite the global default workflow"
            );
            return self.mutate(method, &p, |db| {
                let config = self.config();
                let (sources, source_project_name) = if let Some(arr) = p["sources"].as_array() {
                    let project_name = p["source_project_name"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    (validate_template_files(&config, arr)?, project_name)
                } else {
                    let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
                    let project: Project = db.get(&pid)?;
                    initialize(&self.home, db, &config, &project, self.now_ms())?;
                    let current = project_template_files(db, &project.id)?;
                    let inputs: Vec<Value> = current
                        .iter()
                        .map(|f| json!({"filename": f.filename, "source": f.source}))
                        .collect();
                    (
                        validate_template_files(&config, &inputs)?,
                        Some(project.name),
                    )
                };
                let name = p["name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Default")
                    .to_owned();
                let template = WorkflowTemplate {
                    id: DEFAULT_WORKFLOW_TEMPLATE_ID.into(),
                    name,
                    source_project_name,
                    sources,
                    updated_at: self.now_ms(),
                };
                db.save(&template)?;
                Ok(serde_json::to_value(template)?)
            });
        }
        if method == "workflow.default.reset" {
            ensure!(
                p["execution"].is_null(),
                "Card Agent executions cannot reset the global default workflow"
            );
            return self.mutate(method, &p, |db| {
                let removed = db.conn.execute(
                    "DELETE FROM workflow_templates WHERE id=?1",
                    [DEFAULT_WORKFLOW_TEMPLATE_ID],
                )?;
                Ok(json!({"reset": removed > 0}))
            });
        }
        if method == "workflow.sources" {
            let mut store = self.store.lock().unwrap();
            let now_ms = self.now_ms();
            return store.write(|db| {
                let (sid, project_opt) = scope_from_params(db, &p)?;
                if let Some(project) = &project_opt {
                    initialize(&self.home, db, &self.config(), project, now_ms)?;
                } else {
                    initialize_global(&self.home, db, &self.config(), now_ms)?;
                }
                files(&self.home, db, &sid)
            });
        }
        if method == "workflow.prompt" {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let (_, project_opt) = scope_from_params(db, &p)?;
            let prompt = match project_opt.as_ref() {
                Some(project) => self.workflow_prompt(project),
                None => self.global_workflow_prompt(),
            };
            return Ok(json!({"prompt": prompt}));
        }
        self.mutate(method, &p, |db| {
            let (sid, project_opt) = scope_from_params(db, &p)?;
            let current = files(&self.home, db, &sid)?;
            ensure!(
                p["revision"] == current["revision"],
                "revision_conflict: workflows changed; compare or reload the latest sources"
            );
            let inputs = p["sources"]
                .as_array()
                .context("sources must contain the entire workflow collection")?;
            validate(db, &self.config(), &sid, project_opt.as_ref(), inputs)?;
            if method == "workflow.check" {
                return Ok(json!({"valid":true}));
            }
            publish_scope(
                &self.home,
                db,
                &sid,
                project_opt.as_ref(),
                &self.config(),
                inputs,
                self.now_ms(),
            )?;
            files(&self.home, db, &sid)
        })
    }
}
