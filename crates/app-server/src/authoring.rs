//! SQLite is the only Workflow source of truth. Original YAML, immutable
//! revisions, active references and subscriptions commit in one transaction.
use super::*;
use canban_workflow_engine::source;
use std::collections::{BTreeMap, BTreeSet};

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
    let mut out = Vec::new();
    for input in inputs {
        let name = strp(input, "filename")?;
        filename(name)?;
        ensure!(names.insert(name), "Duplicate filename {name}");
        let raw = strp(input, "source")?;
        let parsed = (|| -> Result<_> {
            let doc = source::parse(raw)?;
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
                        project.workflow.stages.iter().any(|s| &s.id == column),
                        "Unknown trigger column {column}"
                    );
                }
            }
            let flow = doc.compile()?;
            canban_workflow_engine::validate_definition(
                &flow,
                &project
                    .workflow
                    .stages
                    .iter()
                    .map(|s| s.id.clone())
                    .collect::<Vec<_>>(),
                &config
                    .profiles
                    .iter()
                    .map(|p| p.id.clone())
                    .collect::<Vec<_>>(),
            )?;
            Ok((doc, flow))
        })()
        .with_context(|| name.to_owned())?;
        out.push((name.into(), raw.into(), parsed.0, parsed.1));
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
    Ok(())
}
/// Convert each Project once, including existing JSON revisions. No running
/// assignment or human decision is changed by this source migration.
pub(super) fn initialize(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
) -> Result<()> {
    if db
        .list::<WorkflowSource>()?
        .iter()
        .any(|f| f.project_id == project.id)
    {
        return Ok(());
    }
    let binding = db.get::<WorkflowBinding>(&project.id).ok();
    let mut defs: BTreeMap<String, WorkflowRevision> = BTreeMap::new();
    for w in db
        .list::<WorkflowRevision>()?
        .into_iter()
        .filter(|w| w.project_id == project.id)
    {
        if defs
            .get(&w.workflow_id)
            .is_none_or(|v| v.revision < w.revision)
        {
            defs.insert(w.workflow_id.clone(), w);
        }
    }
    let mut default = legacy_definition(&project.workflow, format!("delivery:{}", project.id));
    if default.entries.is_empty() {
        default.entries.insert("manual".into(), "__end".into());
        default.steps.insert("__end".into(), WorkflowStep::Complete);
    }
    let default_id = binding
        .as_ref()
        .map(|b| b.workflow_id.clone())
        .unwrap_or(default.id.clone());
    if binding.is_none() {
        defs.entry(default.id.clone()).or_insert(WorkflowRevision {
            source_yaml: None,
            id: format!("{}:0", default.id),
            project_id: project.id.clone(),
            workflow_id: default.id.clone(),
            revision: 0,
            definition: default,
            created_at: now(),
        });
    }
    let mut inputs = Vec::new();
    for (index, w) in defs.into_values().enumerate() {
        let mut rules = BTreeMap::new();
        for (entry, step) in &w.definition.entries {
            let column = w.workflow_id == default_id
                && project.workflow.stages.iter().any(|s| s.id == *entry);
            rules.insert(
                entry.clone(),
                WorkflowRule {
                    event: if column {
                        "card.column_entered"
                    } else {
                        "manual"
                    }
                    .into(),
                    start_at: step.clone(),
                    selector: EventSelector {
                        column: column.then(|| entry.clone()),
                        ..Default::default()
                    },
                    enabled: true,
                },
            );
        }
        let name = if w.workflow_id == default_id {
            "development.yaml".into()
        } else {
            format!("imported-{index}.yaml")
        };
        let raw = w
            .source_yaml
            .clone()
            .unwrap_or(source::export(&w.definition, rules)?);
        inputs.push(json!({"filename":name,"source":raw}));
    }
    if !inputs.is_empty() {
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

/// JSON compatibility is an import into the same file collection, not a second
/// editable source of truth. Require the current publication revision.
pub(super) fn import(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
    flow: &WorkflowDefinition,
    expected: u64,
) -> Result<WorkflowRevision> {
    let current = super::workflow::published(db, &flow.id).ok();
    ensure!(
        current.as_ref().map_or(0, |w| w.revision) == expected,
        "revision_conflict"
    );
    let collection = files(home, db, project)?;
    let existing = db
        .list::<WorkflowSource>()?
        .into_iter()
        .find(|f| f.workflow_id == flow.id && f.project_id == project.id);
    let name = existing
        .as_ref()
        .map(|f| f.filename.clone())
        .unwrap_or_else(|| {
            format!(
                "imported-{}.yaml",
                &canban_context::digest(flow.id.as_bytes())[..12]
            )
        });
    let rules = existing.map(|f| f.rules).unwrap_or_else(|| {
        flow.entries
            .iter()
            .map(|(key, step)| {
                (
                    key.clone(),
                    WorkflowRule {
                        event: "manual".into(),
                        start_at: step.clone(),
                        selector: Default::default(),
                        enabled: true,
                    },
                )
            })
            .collect()
    });
    let raw = source::export(flow, rules)?;
    let mut inputs = collection["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["filename"] != name)
        .map(|v| json!({"filename":v["filename"],"source":v["source"]}))
        .collect::<Vec<_>>();
    inputs.push(json!({"filename":name,"source":raw}));
    publish(home, db, project, config, &inputs)?;
    super::workflow::published(db, &flow.id)
}

/// Legacy bind translates to visible named trigger rules in SQL YAML sources.
pub(super) fn bind(
    home: &Path,
    db: Db<'_>,
    config: &Config,
    project: &Project,
    workflow_id: &str,
) -> Result<()> {
    let current = files(home, db, project)?;
    let old = db
        .get::<WorkflowBinding>(&project.id)
        .ok()
        .map(|b| b.workflow_id)
        .unwrap_or_else(|| format!("delivery:{}", project.id));
    let mut inputs = Vec::new();
    for item in current["sources"].as_array().unwrap() {
        let mut doc = source::parse(item["source"].as_str().unwrap())?;
        if doc.id == workflow_id {
            for (key, rule) in &mut doc.on {
                if project.workflow.stages.iter().any(|s| s.id == *key) {
                    rule.event = "card.column_entered".into();
                    rule.selector.column = Some(key.clone());
                    rule.enabled = true;
                }
            }
            doc.enabled = true;
        } else if doc.id == old {
            for rule in doc
                .on
                .values_mut()
                .filter(|r| r.event == "card.column_entered")
            {
                rule.enabled = false;
            }
        }
        let raw = if doc.id == workflow_id || doc.id == old {
            source::encode(&doc)?
        } else {
            item["source"].as_str().unwrap().into()
        };
        inputs.push(json!({"filename":item["filename"],"source":raw}));
    }
    publish(home, db, project, config, &inputs)
}
