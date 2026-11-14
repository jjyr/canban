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
fn files(_home: &Path, db: Db<'_>, project: &Project) -> Result<Value> {
    let mut active: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|s| s.project_id == project.id)
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
    Ok(json!({"project_id":project.id,"revision":revision,"sources":rows,"active":active}))
}
fn validate(
    db: Db<'_>,
    config: &Config,
    project: &Project,
    inputs: &[Value],
) -> Result<Vec<(String, String, source::Document, WorkflowDefinition)>> {
    ensure!(
        !inputs.is_empty() && inputs.len() <= 64,
        "A Project needs 1..64 Workflow sources; use enabled: false to disable one"
    );
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
    let effective_columns: Vec<String> = declared_columns
        .as_ref()
        .map(|cols| cols.iter().map(|c| c.id.clone()).collect())
        .unwrap_or_else(|| {
            project
                .workflow
                .stages
                .iter()
                .map(|s| s.id.clone())
                .collect()
        });
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
            if let Ok(old) = super::workflow::published(db, &doc.id) {
                ensure!(
                    old.project_id == project.id,
                    "Workflow ID belongs to another Project"
                );
            }
            for rule in doc.on.values() {
                ensure!(!rule.event.is_empty(), "Trigger event is required");
                ensure!(
                    rule.selector
                        .project_id
                        .as_deref()
                        .is_none_or(|id| id == project.id),
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
fn publish(
    _home: &Path,
    db: Db<'_>,
    project: &Project,
    config: &Config,
    inputs: &[Value],
) -> Result<()> {
    let compiled = validate(db, config, project, inputs)?;
    let old: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == project.id)
        .collect();
    // Retiring a source disables future triggers; captured receipts/runs keep their revision.
    for mut sub in db
        .list::<EventSubscription>()?
        .into_iter()
        .filter(|s| s.id.starts_with(&format!("source:{}:", project.id)))
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
            run.updated_at = now();
            db.save(&run)?;
            let card: Card = db.get(&run.card_id)?;
            log(
                db,
                &card,
                "workflow.stop_scheduled",
                "system",
                "Current step was removed; the run stops after it finishes",
                run.terminal_id.clone(),
                json!({"workflow_run_id":run.id,"step":run.step}),
            )?;
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
                source_yaml: Some(raw.clone()),
                id: format!("{}:{revision}", definition.id),
                project_id: project.id.clone(),
                workflow_id: definition.id.clone(),
                revision,
                definition: definition.clone(),
                created_at: now(),
            })?;
        }
        let file = WorkflowSource {
            id: format!("{}:{name}", project.id),
            project_id: project.id.clone(),
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
            if rule.event == "manual" {
                continue;
            }
            let sub_id = format!("source:{}:{}:{key}", project.id, definition.id);
            let old_sub = db.get::<EventSubscription>(&sub_id).ok();
            let mut selector = rule.selector.clone();
            selector.project_id = Some(project.id.clone());
            selector.types = vec![rule.event.clone()];
            if matches!(
                rule.event.as_str(),
                "card.column_entered" | "card.column_left"
            ) && selector.origin.is_none()
            {
                selector.origin = Some("local".into());
            }
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
                created_at: old_sub.map_or(now(), |s| s.created_at),
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
    // Synchronize declared YAML columns and primary workspace settings to Project.
    let active_sources: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == project.id)
        .collect();
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
                )?;
            }
        }
    }
    Ok(())
}
/// Initialize a newly created Project with either an explicit YAML source document
/// or the native default `development.yaml` workflow.
pub(super) fn initialize_new(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
    workflow_yaml: Option<&str>,
) -> Result<()> {
    let raw = if let Some(yaml) = workflow_yaml {
        yaml.to_owned()
    } else {
        let mut doc = source::default_document(
            format!("delivery:{}", project.id),
            project.workflow.workspace.clone(),
        );
        for step in &mut doc.steps {
            if step.uses == "agent"
                && step
                    .with
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| !config.profiles.iter().any(|p| p.id == id))
            {
                step.with.remove("profile");
            }
        }
        source::encode(&doc)?
    };
    publish(
        home,
        db,
        project,
        config,
        &[json!({"filename":"development.yaml","source":raw})],
    )
}

/// Ensure every Project has an initialized SQL YAML workflow collection,
/// and migrate any active workflow sources on older schema versions to `CURRENT`.
pub(super) fn initialize(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
) -> Result<()> {
    let mut existing: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|f| f.project_id == project.id)
        .collect();
    if existing.is_empty() {
        return initialize_new(home, db, config, project, None);
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
        publish(home, db, project, config, &inputs)?;
    }
    Ok(())
}
impl AppServer {
    pub(super) fn authoring_call(&self, method: &str, p: Value) -> Result<Value> {
        if method == "workflow.sources" {
            let mut store = self.store.lock().unwrap();
            return store.write(|db| {
                let project: Project = db.get(strp(&p, "project_id")?)?;
                initialize(&self.home, db, &self.config(), &project)?;
                files(&self.home, db, &project)
            });
        }
        if method == "workflow.prompt" {
            let project: Project = self
                .store
                .lock()
                .unwrap()
                .db()
                .get(strp(&p, "project_id")?)?;
            return Ok(json!({"prompt":self.workflow_prompt(&project)}));
        }
        self.mutate(method, &p, |db| {
            let project: Project = db.get(strp(&p, "project_id")?)?;
            let current = files(&self.home, db, &project)?;
            ensure!(
                p["revision"] == current["revision"],
                "revision_conflict: workflows changed; compare or reload the latest sources"
            );
            let inputs = p["sources"]
                .as_array()
                .context("sources must contain the entire Project collection")?;
            validate(db, &self.config(), &project, inputs)?;
            if method == "workflow.check" {
                return Ok(json!({"valid":true}));
            }
            publish(&self.home, db, &project, &self.config(), inputs)?;
            files(&self.home, db, &project)
        })
    }
}
