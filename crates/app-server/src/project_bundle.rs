//! Project-level export and import (`*.canban.tar.zst`) for cross-host migration and backup.

use super::*;
use canban_context::{
    CardBranchHint, ProjectBundleFileMeta, ProjectBundleManifest, ProjectGitHints, WorktreeHint,
    cleanup_extracted_files, digest, extract_project_bundle_files, read_project_bundle,
    validate_relative_bundle_path, write_project_bundle,
};
use canban_herdr::host::Host;
use canban_storage::env::{SCOPE_PROJECT, apply_env_writes};
use canban_types::WorkflowStep;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExportedDraft {
    id: String,
    revision: u64,
    updated_at: u64,
    data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExportedEnvEntry {
    key: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExportedObservation {
    id: String,
    data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectBundleRecords {
    project: Project,
    #[serde(default)]
    cards: Vec<Card>,
    #[serde(default)]
    events: Vec<Event>,
    #[serde(default)]
    card_reads: Vec<CardRead>,
    #[serde(default)]
    reports: Vec<Report>,
    #[serde(default)]
    artifacts: Vec<Artifact>,
    #[serde(default)]
    questions: Vec<Question>,
    #[serde(default)]
    workflow_sources: Vec<WorkflowSource>,
    #[serde(default)]
    workflow_revisions: Vec<WorkflowRevision>,
    #[serde(default)]
    workflow_runs: Vec<WorkflowRun>,
    #[serde(default)]
    schedule_states: Vec<ScheduleState>,
    #[serde(default)]
    scheduled_runs: Vec<ScheduledRun>,
    #[serde(default)]
    notifications: Vec<Notice>,
    #[serde(default)]
    event_subscriptions: Vec<EventSubscription>,
    #[serde(default)]
    editor_drafts: Vec<ExportedDraft>,
    #[serde(default)]
    plain_env: Vec<ExportedEnvEntry>,
    #[serde(default)]
    counters: BTreeMap<String, u64>,
    #[serde(default)]
    notes: Vec<Note>,
    #[serde(default)]
    note_revisions: Vec<NoteRevision>,
    #[serde(default)]
    note_events: Vec<NoteEvent>,
    #[serde(default)]
    source_observations: Vec<ExportedObservation>,
    #[serde(default)]
    activity_segments: Vec<ActivitySegment>,
}

fn expand_absolute_path(host: &Host, raw: &str, label: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    ensure!(!trimmed.is_empty(), "{label} is required");
    let expanded = if trimmed == "~" {
        host.home()?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = host.home()?;
        if home == "/" {
            format!("/{rest}")
        } else {
            format!("{home}/{rest}")
        }
    } else {
        trimmed.to_owned()
    };
    let path = PathBuf::from(&expanded);
    ensure!(
        path.is_absolute(),
        "{label} must be an absolute path: {expanded}"
    );
    Ok(path)
}

fn collect_dir_files(home: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_dir_files(home, &path, out)?;
        } else if meta.is_file() {
            let rel = canban_context::relative(home, &path)?.replace('\\', "/");
            if validate_relative_bundle_path(&rel).is_ok() {
                out.insert(rel, path);
            }
        }
    }
    Ok(())
}

fn maybe_add_managed_file(
    home: &Path,
    raw_rel: Option<&str>,
    allowed_prefix: &str,
    out: &mut BTreeMap<String, PathBuf>,
) {
    let Some(rel) = raw_rel.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let normalized = rel.replace('\\', "/");
    if !normalized.starts_with(allowed_prefix)
        || validate_relative_bundle_path(&normalized).is_err()
    {
        return;
    }
    let abs = home.join(&normalized);
    if let Ok(meta) = std::fs::symlink_metadata(&abs)
        && !meta.file_type().is_symlink()
        && meta.is_file()
    {
        out.insert(normalized, abs);
    }
}

fn read_counter_opt(db: Db<'_>, key: &str) -> Result<Option<u64>> {
    use rusqlite::OptionalExtension;
    db.conn
        .query_row("SELECT value FROM counters WHERE id=?1", [key], |r| {
            r.get::<_, u64>(0)
        })
        .optional()
        .map_err(Into::into)
}

fn prune_empty_created_dirs(home: &Path, created_files: &[PathBuf], roots: &[PathBuf]) {
    let mut dirs = BTreeSet::new();
    for file in created_files {
        let mut cur = file.parent();
        while let Some(dir) = cur {
            if !roots.iter().any(|r| dir.starts_with(r)) {
                break;
            }
            dirs.insert(dir.to_path_buf());
            if dir == home {
                break;
            }
            cur = dir.parent();
        }
    }
    for dir in dirs.into_iter().rev() {
        let _ = std::fs::remove_dir(&dir);
    }
}

impl AppServer {
    pub(super) fn export_project(&self, p: Value) -> Result<Value> {
        let raw_pid = strp(&p, "project_id")?;
        let raw_output = strp(&p, "output")?;
        let include_archived = p["include_archived"].as_bool().unwrap_or(false);
        let overwrite = p["overwrite"].as_bool().unwrap_or(false);
        let host = Host::new(self.config());
        let output_path = expand_absolute_path(&host, raw_output, "output")?;
        if !overwrite && output_path.exists() {
            bail!(
                "Output file already exists ({}); choose a new path or pass --overwrite",
                output_path.display()
            );
        }

        struct ExportData {
            records: ProjectBundleRecords,
            manifest: ProjectBundleManifest,
            files: Vec<(String, PathBuf)>,
            warnings: Vec<String>,
        }

        let data: ExportData = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let pid = super::resolve_project_id(db, raw_pid)?;
            let project: Project = db.get(&pid)?;

            let all_cards: Vec<Card> = db
                .list::<Card>()?
                .into_iter()
                .filter(|c| c.project_id == pid)
                .collect();

            let mut exported_cards_vec = Vec::new();
            let mut active_count = 0usize;
            let mut archived_count = 0usize;
            let mut tombstone_count = 0usize;
            let mut skipped_archived_cards = 0usize;
            let mut skipped_archived_github_cards = 0usize;
            let mut max_all_card_number = 0u64;

            for card in all_cards {
                max_all_card_number = max_all_card_number.max(card.number);
                if card.removed_at.is_some() {
                    if card.github_issue().is_some() {
                        tombstone_count += 1;
                        exported_cards_vec.push(card);
                    }
                } else if card.archived_at.is_some() {
                    if include_archived {
                        archived_count += 1;
                        exported_cards_vec.push(card);
                    } else {
                        skipped_archived_cards += 1;
                        if card.github_issue().is_some() {
                            skipped_archived_github_cards += 1;
                        }
                    }
                } else {
                    active_count += 1;
                    exported_cards_vec.push(card);
                }
            }

            let content_cards: Vec<&Card> = exported_cards_vec
                .iter()
                .filter(|c| c.removed_at.is_none())
                .collect();
            let content_card_ids: HashSet<String> =
                content_cards.iter().map(|c| c.id.clone()).collect();

            let events: Vec<Event> = {
                let mut q = db
                    .conn
                    .prepare("SELECT data FROM events WHERE project_id=?1 ORDER BY rowid")?;
                let rows = q.query_map([&pid], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for row in rows {
                    let ev: Event = serde_json::from_str(&row?)?;
                    if (!ev.card_id.is_empty() && content_card_ids.contains(&ev.card_id))
                        || (ev.card_id.is_empty() && include_archived)
                    {
                        out.push(ev);
                    }
                }
                out
            };

            let card_reads: Vec<CardRead> = db
                .list::<CardRead>()?
                .into_iter()
                .filter(|r| content_card_ids.contains(&r.card_id))
                .collect();
            let reports: Vec<Report> = db
                .list::<Report>()?
                .into_iter()
                .filter(|r| content_card_ids.contains(&r.card_id))
                .collect();
            let artifacts: Vec<Artifact> = db
                .list::<Artifact>()?
                .into_iter()
                .filter(|a| content_card_ids.contains(&a.card_id))
                .collect();
            let questions: Vec<Question> = db
                .list::<Question>()?
                .into_iter()
                .filter(|q| content_card_ids.contains(&q.card_id))
                .collect();
            let notifications: Vec<Notice> = db
                .list::<Notice>()?
                .into_iter()
                .filter(|n| content_card_ids.contains(&n.card_id))
                .collect();

            let live_terminals_count = db
                .list::<TerminalEntry>()?
                .into_iter()
                .filter(|t| {
                    let belongs = t.agent.as_ref().is_some_and(|a| a.project_id == pid)
                        || t.origin_project_id.as_deref() == Some(pid.as_str())
                        || t.id.starts_with(&format!("merge-queue:{pid}:"))
                        || t.id.starts_with(&format!("configuration:{pid}:"))
                        || t.id.starts_with(&format!("scheduled:{pid}:"));
                    belongs
                        && (t.agent.as_ref().is_some_and(|a| !a.finished())
                            || t.runtime.as_ref().is_some_and(|r| r.state != "finished"))
                })
                .count();

            let workflow_sources: Vec<WorkflowSource> = db
                .list::<WorkflowSource>()?
                .into_iter()
                .filter(|s| s.project_id == pid)
                .collect();
            let workflow_revisions: Vec<WorkflowRevision> = db
                .list::<WorkflowRevision>()?
                .into_iter()
                .filter(|r| r.project_id == pid)
                .collect();
            let workflow_runs: Vec<WorkflowRun> = db
                .list::<WorkflowRun>()?
                .into_iter()
                .filter(|r| {
                    r.project_id == pid
                        && r.card_id
                            .as_deref()
                            .is_none_or(|cid| content_card_ids.contains(cid))
                })
                .collect();
            let schedule_states: Vec<ScheduleState> = db
                .list::<ScheduleState>()?
                .into_iter()
                .filter(|s| s.project_id == pid)
                .collect();
            let scheduled_runs: Vec<ScheduledRun> = db
                .list::<ScheduledRun>()?
                .into_iter()
                .filter(|r| {
                    r.project_id == pid
                        && r.card_id
                            .as_deref()
                            .is_none_or(|cid| content_card_ids.contains(cid))
                })
                .collect();
            let sub_prefix = format!("source:{pid}:");
            let event_subscriptions: Vec<EventSubscription> = db
                .list::<EventSubscription>()?
                .into_iter()
                .filter(|s| {
                    s.id.starts_with(&sub_prefix) || s.selector.project_id.as_deref() == Some(&pid)
                })
                .collect();

            let active_merges = db
                .list::<MergeBatch>()?
                .into_iter()
                .filter(|b| b.project_id == pid && b.active())
                .count()
                + self
                    .merging_cards
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|cid| {
                        content_card_ids.contains(cid.as_str())
                            || db.get::<Card>(cid).is_ok_and(|c| c.project_id == pid)
                    })
                    .count();
            let active_configs = db
                .list::<ConfigurationSession>()?
                .into_iter()
                .filter(|s| {
                    s.project_id == pid && !matches!(s.state.as_str(), "finished" | "cancelled")
                })
                .count();

            let mut editor_drafts = Vec::new();
            {
                let mut draft_ids = vec![format!("new-card:{pid}")];
                for cid in &content_card_ids {
                    draft_ids.push(format!("card:{cid}"));
                    draft_ids.push(format!("review-draft:{cid}"));
                }
                let mut q = db.conn.prepare(
                    "SELECT id, revision, updated_at, data FROM editor_drafts WHERE id=?1",
                )?;
                for did in draft_ids {
                    use rusqlite::OptionalExtension;
                    if let Some(row) = q
                        .query_row([&did], |r| {
                            Ok(ExportedDraft {
                                id: r.get(0)?,
                                revision: r.get::<_, i64>(1)? as u64,
                                updated_at: r.get::<_, i64>(2)? as u64,
                                data: r.get(3)?,
                            })
                        })
                        .optional()?
                    {
                        editor_drafts.push(row);
                    }
                }
            }

            let plain_env: Vec<ExportedEnvEntry> = {
                let mut q = db.conn.prepare(
                    "SELECT key, kind, value, updated_at FROM env_entries \
                     WHERE scope=?1 AND target_id=?2 AND deleted=0 AND kind IN ('plain','unset') \
                     ORDER BY key",
                )?;
                q.query_map(rusqlite::params![SCOPE_PROJECT, &pid], |r| {
                    Ok(ExportedEnvEntry {
                        key: r.get(0)?,
                        kind: r.get(1)?,
                        value: r.get(2)?,
                        updated_at: r.get::<_, i64>(3)? as u64,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let secret_keys: Vec<String> = {
                let mut q = db.conn.prepare(
                    "SELECT key FROM env_entries \
                     WHERE scope=?1 AND target_id=?2 AND deleted=0 AND kind='secret' \
                     ORDER BY key",
                )?;
                q.query_map(rusqlite::params![SCOPE_PROJECT, &pid], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };

            let mut counters = BTreeMap::new();
            let card_counter_key = format!("cards:{pid}");
            let card_counter_val = read_counter_opt(db, &card_counter_key)?
                .unwrap_or(0)
                .max(max_all_card_number);
            counters.insert(card_counter_key, card_counter_val);
            for cid in &content_card_ids {
                let tl_key = format!("timeline:{cid}");
                let max_seq = events
                    .iter()
                    .filter(|e| e.card_id == *cid)
                    .map(|e| e.sequence)
                    .max()
                    .unwrap_or(0);
                let val = read_counter_opt(db, &tl_key)?.unwrap_or(0).max(max_seq);
                if val > 0 {
                    counters.insert(tl_key, val);
                }
            }

            let (notes, note_revisions, note_events, source_observations, activity_segments) =
                if include_archived {
                    let notes: Vec<Note> = db
                        .list::<Note>()?
                        .into_iter()
                        .filter(|n| n.project_id == pid)
                        .collect();
                    let note_revisions: Vec<NoteRevision> = db
                        .list::<NoteRevision>()?
                        .into_iter()
                        .filter(|n| n.project_id == pid)
                        .collect();
                    let note_events: Vec<NoteEvent> = db
                        .list::<NoteEvent>()?
                        .into_iter()
                        .filter(|n| n.project_id == pid)
                        .collect();
                    let source_observations: Vec<ExportedObservation> = {
                        let mut q = db.conn.prepare(
                            "SELECT id, data FROM source_observations WHERE id=?1 OR id LIKE ?2 ORDER BY id",
                        )?;
                        q.query_map(
                            rusqlite::params![format!("github:{pid}"), format!("github:{pid}:%")],
                            |r| {
                                Ok(ExportedObservation {
                                    id: r.get(0)?,
                                    data: r.get(1)?,
                                })
                            },
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                    };
                    let mut activity_segments = Vec::new();
                    db.for_each_activity_segment(0, i64::MAX as u64, Some(&pid), None, |seg| {
                        activity_segments.push(seg);
                        Ok(())
                    })?;
                    (
                        notes,
                        note_revisions,
                        note_events,
                        source_observations,
                        activity_segments,
                    )
                } else {
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())
                };

            let mut required_profiles_set = BTreeSet::new();
            for stage in &project.workflow.stages {
                if let Some(prof) = stage.profile.as_deref().filter(|s| !s.trim().is_empty()) {
                    required_profiles_set.insert(prof.to_owned());
                }
            }
            let active_rev_ids: HashSet<&str> = workflow_sources
                .iter()
                .map(|s| s.revision_id.as_str())
                .collect();
            for rev in &workflow_revisions {
                if !active_rev_ids.contains(rev.id.as_str()) {
                    continue;
                }
                for step in rev.definition.steps.values() {
                    match step {
                        WorkflowStep::Agent {
                            profile: Some(prof),
                            ..
                        }
                        | WorkflowStep::Deliver {
                            profile: Some(prof),
                            ..
                        } if !prof.trim().is_empty() => {
                            required_profiles_set.insert(prof.clone());
                        }
                        _ => {}
                    }
                }
            }

            let managed_worktrees: Vec<WorktreeHint> = db
                .list::<ManagedWorktree>()?
                .into_iter()
                .filter(|w| w.project_id == pid)
                .map(|w| WorktreeHint {
                    branch: w.branch,
                    base_branch: w.base_branch,
                    base_commit: w.base_commit,
                })
                .collect();
            let card_branches: Vec<CardBranchHint> = exported_cards_vec
                .iter()
                .filter(|c| c.removed_at.is_none() && c.archived_at.is_none())
                .filter_map(|c| {
                    c.branch.as_ref().map(|branch| CardBranchHint {
                        card_number: c.number,
                        card_id: c.id.clone(),
                        branch: branch.clone(),
                        base_commit: c.base_commit.clone(),
                    })
                })
                .collect();
            let remotes = self
                .ops()
                .remote_urls(&project.directory, &project.workflow.workspace.remote);

            let git_hints = ProjectGitHints {
                remotes,
                target_branch: project.target_branch.clone(),
                card_branches,
                managed_worktrees,
            };

            let mut file_map: BTreeMap<String, PathBuf> = BTreeMap::new();
            let project_files_prefix = format!("projects/{pid}/");
            for card in &content_cards {
                let cdir = canban_context::card_dir(&self.home, &pid, card.number);
                collect_dir_files(&self.home, &cdir, &mut file_map)?;
            }
            for art in &artifacts {
                maybe_add_managed_file(
                    &self.home,
                    Some(&art.path),
                    &project_files_prefix,
                    &mut file_map,
                );
            }
            for rep in &reports {
                maybe_add_managed_file(
                    &self.home,
                    rep.path.as_deref(),
                    &project_files_prefix,
                    &mut file_map,
                );
            }
            for q in &questions {
                maybe_add_managed_file(
                    &self.home,
                    q.reply_path.as_deref(),
                    &project_files_prefix,
                    &mut file_map,
                );
            }
            if include_archived {
                let arch_dir = self.home.join("archives").join(&pid);
                collect_dir_files(&self.home, &arch_dir, &mut file_map)?;
                let archive_prefix = format!("archives/{pid}/");
                for card in &content_cards {
                    maybe_add_managed_file(
                        &self.home,
                        card.archive.as_deref(),
                        &archive_prefix,
                        &mut file_map,
                    );
                }
            }

            let mut file_metas = Vec::with_capacity(file_map.len());
            let mut files = Vec::with_capacity(file_map.len());
            for (rel, abs) in file_map {
                let bytes = std::fs::read(&abs)
                    .with_context(|| format!("Failed reading managed file {}", abs.display()))?;
                file_metas.push(ProjectBundleFileMeta {
                    path: rel.clone(),
                    bytes: bytes.len() as u64,
                    digest: digest(&bytes),
                });
                files.push((rel, abs));
            }

            let mut warnings = Vec::new();
            if live_terminals_count > 0 || active_merges > 0 || active_configs > 0 {
                warnings.push(format!(
                    "Project has live runtime activity ({live_terminals_count} live terminal(s), {active_merges} active merge(s), {active_configs} configuration session(s)); stop active runs and pause GitHub intake before cutting over to another host."
                ));
            }
            if !include_archived && skipped_archived_github_cards > 0 {
                warnings.push(format!(
                    "Excluded {skipped_archived_github_cards} archived GitHub Issue/PR card(s); if any remain open on GitHub, target github.sync may re-import them unless exported with --include-archived."
                ));
            }
            if !secret_keys.is_empty() {
                warnings.push(format!(
                    "Omitted {} project secret(s) ({}) from the bundle; reconfigure them on the target host after import.",
                    secret_keys.len(),
                    secret_keys.join(", ")
                ));
            }

            let manifest = ProjectBundleManifest {
                format_version: canban_context::PROJECT_BUNDLE_FORMAT_VERSION,
                codec: "zstd".into(),
                sqlite_schema: canban_storage::migrations::CURRENT,
                include_archived,
                created_at: now(),
                source_server_id: self.identity.clone(),
                project_id: pid.clone(),
                project_name: project.name.clone(),
                source_directory: project.directory.clone(),
                exported_cards: active_count,
                exported_archived_cards: archived_count,
                exported_tombstones: tombstone_count,
                skipped_archived_cards,
                skipped_archived_github_cards,
                required_profiles: required_profiles_set.into_iter().collect(),
                secret_keys,
                git_hints,
                files: file_metas,
            };

            let records = ProjectBundleRecords {
                project,
                cards: exported_cards_vec,
                events,
                card_reads,
                reports,
                artifacts,
                questions,
                workflow_sources,
                workflow_revisions,
                workflow_runs,
                schedule_states,
                scheduled_runs,
                notifications,
                event_subscriptions,
                editor_drafts,
                plain_env,
                counters,
                notes,
                note_revisions,
                note_events,
                source_observations,
                activity_segments,
            };

            ExportData {
                records,
                manifest,
                files,
                warnings,
            }
        };

        let records_json = serde_json::to_vec_pretty(&data.records)?;
        let bundle_bytes = write_project_bundle(
            &output_path,
            overwrite,
            &data.manifest,
            &records_json,
            &data.files,
        )?;

        Ok(json!({
            "exported": true,
            "project_id": data.manifest.project_id,
            "name": data.manifest.project_name,
            "output": output_path.to_string_lossy(),
            "bytes": bundle_bytes,
            "include_archived": data.manifest.include_archived,
            "exported_cards": data.manifest.exported_cards,
            "exported_archived_cards": data.manifest.exported_archived_cards,
            "exported_tombstones": data.manifest.exported_tombstones,
            "skipped_archived_cards": data.manifest.skipped_archived_cards,
            "skipped_archived_github_cards": data.manifest.skipped_archived_github_cards,
            "files_count": data.manifest.files.len(),
            "secret_keys": data.manifest.secret_keys,
            "required_profiles": data.manifest.required_profiles,
            "git_hints": data.manifest.git_hints,
            "warnings": data.warnings,
        }))
    }

    pub(super) fn import_project(&self, p: Value) -> Result<Value> {
        let raw_file = strp(&p, "file")?;
        let host = Host::new(self.config());
        let archive_path = expand_absolute_path(&host, raw_file, "file")?;
        let target_directory = self.resolve_project_directory(&p)?;
        let name_override = p
            .get("name")
            .and_then(Value::as_str)
            .map(|s| {
                let trimmed = s.trim();
                ensure!(!trimmed.is_empty(), "Project name must not be empty");
                Ok::<String, anyhow::Error>(trimmed.to_owned())
            })
            .transpose()?;
        let pause_github = p["pause_github"].as_bool().unwrap_or(false);

        let loaded = read_project_bundle(&archive_path)?;
        ensure!(
            loaded.manifest.sqlite_schema <= canban_storage::migrations::CURRENT,
            "Bundle SQLite schema v{} is newer than this daemon (v{}); upgrade the target Canban daemon first",
            loaded.manifest.sqlite_schema,
            canban_storage::migrations::CURRENT
        );
        let records: ProjectBundleRecords = serde_json::from_slice(&loaded.records_json)
            .context("Failed decoding records.json from project bundle")?;
        let pid = records.project.id.clone();
        ensure!(
            pid == loaded.manifest.project_id,
            "Bundle manifest project_id ({}) does not match records.json ({pid})",
            loaded.manifest.project_id
        );
        let imported_card_ids: HashSet<String> =
            records.cards.iter().map(|c| c.id.clone()).collect();
        ensure!(
            records.cards.iter().all(|c| c.project_id == pid),
            "Bundle contains card(s) belonging to another project"
        );
        ensure!(
            records.events.iter().all(|e| e.project_id == pid),
            "Bundle contains event(s) belonging to another project"
        );
        ensure!(
            records.card_reads.iter().all(|r| r.project_id == pid),
            "Bundle contains card_read(s) belonging to another project"
        );
        ensure!(
            records.reports.iter().all(|r| r.project_id == pid),
            "Bundle contains report(s) belonging to another project"
        );
        ensure!(
            records.artifacts.iter().all(|a| a.project_id == pid),
            "Bundle contains artifact(s) belonging to another project"
        );
        ensure!(
            records.questions.iter().all(|q| q.project_id == pid),
            "Bundle contains question(s) belonging to another project"
        );
        ensure!(
            records.notifications.iter().all(|n| n.project_id == pid),
            "Bundle contains notification(s) belonging to another project"
        );
        ensure!(
            records.workflow_sources.iter().all(|s| s.project_id == pid),
            "Bundle contains workflow_source(s) belonging to another project"
        );
        ensure!(
            records
                .workflow_revisions
                .iter()
                .all(|r| r.project_id == pid),
            "Bundle contains workflow_revision(s) belonging to another project"
        );
        ensure!(
            records.workflow_runs.iter().all(|r| r.project_id == pid),
            "Bundle contains workflow_run(s) belonging to another project"
        );
        ensure!(
            records.schedule_states.iter().all(|s| s.project_id == pid),
            "Bundle contains schedule_state(s) belonging to another project"
        );
        ensure!(
            records.scheduled_runs.iter().all(|r| r.project_id == pid),
            "Bundle contains scheduled_run(s) belonging to another project"
        );
        let sub_prefix = format!("source:{pid}:");
        ensure!(
            records.event_subscriptions.iter().all(|s| {
                s.id.starts_with(&sub_prefix) || s.selector.project_id.as_deref() == Some(&pid)
            }),
            "Bundle contains event_subscription(s) outside project {pid}"
        );
        ensure!(
            records.notes.iter().all(|n| n.project_id == pid),
            "Bundle contains note(s) belonging to another project"
        );
        ensure!(
            records.note_revisions.iter().all(|n| n.project_id == pid),
            "Bundle contains note_revision(s) belonging to another project"
        );
        ensure!(
            records.note_events.iter().all(|n| n.project_id == pid),
            "Bundle contains note_event(s) belonging to another project"
        );
        ensure!(
            records
                .activity_segments
                .iter()
                .all(|seg| seg.project_id.as_deref() == Some(pid.as_str())),
            "Bundle contains activity_segment(s) belonging to another project"
        );
        let new_card_draft_id = format!("new-card:{pid}");
        ensure!(
            records.editor_drafts.iter().all(|d| {
                d.id == new_card_draft_id
                    || d.id
                        .strip_prefix("card:")
                        .is_some_and(|cid| imported_card_ids.contains(cid))
                    || d.id
                        .strip_prefix("review-draft:")
                        .is_some_and(|cid| imported_card_ids.contains(cid))
            }),
            "Bundle contains editor_draft(s) outside project {pid}"
        );
        let obs_exact = format!("github:{pid}");
        let obs_prefix = format!("github:{pid}:");
        ensure!(
            records
                .source_observations
                .iter()
                .all(|o| o.id == obs_exact || o.id.starts_with(&obs_prefix)),
            "Bundle contains source_observation(s) outside project {pid}"
        );
        let card_counter_key = format!("cards:{pid}");
        ensure!(
            records.counters.keys().all(|key| {
                key == &card_counter_key
                    || key
                        .strip_prefix("timeline:")
                        .is_some_and(|cid| imported_card_ids.contains(cid))
            }),
            "Bundle contains out-of-scope counter key(s)"
        );

        {
            let store = self.store.lock().unwrap();
            let db = store.db();
            ensure!(
                db.get::<Project>(&pid).is_err(),
                "Project {pid} already exists on this host"
            );
            for rev in &records.workflow_revisions {
                ensure!(
                    db.get::<WorkflowRevision>(&rev.id).is_err(),
                    "Workflow revision {} already exists on this host",
                    rev.id
                );
                let exists: bool = db.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workflow_revisions WHERE json_extract(data,'$.workflow_id')=?1 AND json_extract(data,'$.revision')=?2)",
                    rusqlite::params![&rev.workflow_id, rev.revision],
                    |r| r.get(0),
                )?;
                ensure!(
                    !exists,
                    "Workflow revision {}:{} already exists on this host",
                    rev.workflow_id,
                    rev.revision
                );
            }
            for rep in &records.reports {
                let exists: bool = db.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM reports WHERE json_extract(data,'$.request_key')=?1)",
                    [&rep.request_key],
                    |r| r.get(0),
                )?;
                ensure!(
                    !exists,
                    "Report request_key {} already exists on this host",
                    rep.request_key
                );
            }
        }

        let detected_github = self.detect_project_github(
            &target_directory,
            &records.project.workflow.workspace.remote,
        );

        let mut allowed_prefixes = vec![format!("projects/{pid}/")];
        if loaded.manifest.include_archived {
            allowed_prefixes.push(format!("archives/{pid}/"));
        }
        let created_files =
            extract_project_bundle_files(&archive_path, &self.home, &allowed_prefixes)?;

        let cleanup_roots = vec![
            self.home.join("projects").join(&pid),
            self.home.join("archives").join(&pid),
        ];

        let mut summary_imported_active = 0usize;
        let mut summary_imported_archived = 0usize;
        let mut summary_imported_tombstones = 0usize;
        let mut summary_github_paused = false;
        let mut summary_github_active = false;
        let mut final_project_name = records.project.name.clone();

        let tx_result = self.mutate("project.import", &p, |db| {
            ensure!(
                db.get::<Project>(&pid).is_err(),
                "Project {pid} already exists on this host"
            );
            let now_ms = now();
            let highwater = super::events::highwater(db)?;

            let mut project = records.project.clone();
            project.directory = target_directory.clone();
            if let Some(name) = &name_override {
                project.name = name.clone();
            }
            final_project_name = project.name.clone();
            if let Some(detected) = &detected_github {
                project.detected_github = Some(detected.clone());
            }
            if let Some(g) = &mut project.github {
                g.error = None;
                if pause_github {
                    g.paused = true;
                }
                summary_github_paused = g.paused;
                summary_github_active = !g.paused
                    && (!matches!(g.intake, GitHubIntake::Off)
                        || !matches!(g.pr_intake, GitHubPrIntake::Off));
            }
            db.save(&project)?;

            for rev in &records.workflow_revisions {
                db.save(rev)?;
            }
            for src in &records.workflow_sources {
                db.save(src)?;
            }

            let mut max_card_number = 0u64;
            let mut max_event_seq: BTreeMap<String, u64> = BTreeMap::new();
            for mut card in records.cards.clone() {
                max_card_number = max_card_number.max(card.number);
                card.worktree = None;
                card.workspace = None;
                card.workspace_history.clear();
                card.archive_error = None;
                if let Some(gate) = &mut card.human_gate {
                    gate.terminal_id = None;
                }
                if card.removed_at.is_some() {
                    summary_imported_tombstones += 1;
                } else if card.archived_at.is_some() {
                    summary_imported_archived += 1;
                } else {
                    summary_imported_active += 1;
                }
                db.save(&card)?;
            }

            for ev in &records.events {
                if !ev.card_id.is_empty() {
                    max_event_seq
                        .entry(ev.card_id.clone())
                        .and_modify(|s| *s = (*s).max(ev.sequence))
                        .or_insert(ev.sequence);
                }
                // Save directly to `events` without inserting into `event_journal`
                // so historical events never replay into target subscriptions.
                db.save(ev)?;
            }

            for read in &records.card_reads {
                db.save(read)?;
            }
            for art in &records.artifacts {
                db.save(art)?;
            }
            for mut rep in records.reports.clone() {
                rep.terminal_id = None;
                db.save(&rep)?;
            }
            for mut q in records.questions.clone() {
                q.terminal_id = None;
                db.save(&q)?;
            }
            for n in &records.notifications {
                db.save(n)?;
            }

            for mut run in records.workflow_runs.clone() {
                run.terminal_id = None;
                if run.active() {
                    run.state = "cancelled".into();
                    run.error = Some("Cancelled during project import from another host".into());
                    run.updated_at = now_ms;
                }
                db.save(&run)?;
            }

            for mut sched in records.schedule_states.clone() {
                sched.queued_triggers = 0;
                if !sched.enabled {
                    sched.next_run_at = None;
                } else if sched.next_run_at.is_none_or(|due| due <= now_ms) {
                    sched.next_run_at = canban_scheduler::timers::compute_next_run_at(
                        &sched.cron,
                        sched.timezone.as_deref(),
                        now_ms,
                    );
                }
                sched.updated_at = now_ms;
                db.save(&sched)?;
            }

            for mut srun in records.scheduled_runs.clone() {
                srun.terminal_id = None;
                if srun.active() {
                    srun.state = "cancelled".into();
                    srun.error = Some("Cancelled during project import from another host".into());
                    srun.updated_at = now_ms;
                }
                db.save(&srun)?;
            }

            for mut sub in records.event_subscriptions.clone() {
                sub.cursor = highwater;
                db.save(&sub)?;
            }

            for draft in &records.editor_drafts {
                db.conn.execute(
                    "INSERT OR REPLACE INTO editor_drafts(id, revision, updated_at, data) VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![&draft.id, draft.revision as i64, draft.updated_at as i64, &draft.data],
                )?;
            }

            let exported_card_counter = records
                .counters
                .get(&card_counter_key)
                .copied()
                .unwrap_or(0)
                .max(max_card_number);
            if exported_card_counter > 0 {
                db.conn.execute(
                    "INSERT INTO counters(id,value) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=max(value,excluded.value)",
                    rusqlite::params![&card_counter_key, exported_card_counter as i64],
                )?;
            }
            for (cid, seq) in max_event_seq {
                let key = format!("timeline:{cid}");
                let val = records.counters.get(&key).copied().unwrap_or(0).max(seq);
                if val > 0 {
                    db.conn.execute(
                        "INSERT INTO counters(id,value) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=max(value,excluded.value)",
                        rusqlite::params![&key, val as i64],
                    )?;
                }
            }
            for cid in &imported_card_ids {
                let key = format!("timeline:{cid}");
                if let Some(&val) = records.counters.get(&key)
                    && val > 0
                {
                    db.conn.execute(
                        "INSERT INTO counters(id,value) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=max(value,excluded.value)",
                        rusqlite::params![&key, val as i64],
                    )?;
                }
            }

            if !records.plain_env.is_empty() {
                let mut writes = BTreeMap::new();
                for entry in &records.plain_env {
                    match entry.kind.as_str() {
                        "plain" => {
                            writes.insert(
                                entry.key.clone(),
                                EnvWriteEntry::Plain {
                                    value: entry.value.clone().unwrap_or_default(),
                                },
                            );
                        }
                        "unset" => {
                            writes.insert(entry.key.clone(), EnvWriteEntry::Unset);
                        }
                        _ => {}
                    }
                }
                if !writes.is_empty() {
                    apply_env_writes(db.conn, &self.home, SCOPE_PROJECT, &pid, &writes, now_ms)?;
                }
            }

            for note in &records.notes {
                db.save(note)?;
            }
            for rev in &records.note_revisions {
                db.save(rev)?;
            }
            for ev in &records.note_events {
                db.save(ev)?;
            }
            for obs in &records.source_observations {
                db.conn.execute(
                    "INSERT OR REPLACE INTO source_observations(id, data) VALUES(?1, ?2)",
                    rusqlite::params![&obs.id, &obs.data],
                )?;
            }
            if !records.activity_segments.is_empty() {
                db.insert_activity_segments(&records.activity_segments)?;
            }

            Ok(json!({
                "imported": true,
                "project_id": pid,
            }))
        });

        if let Err(err) = tx_result {
            cleanup_extracted_files(&created_files);
            prune_empty_created_dirs(&self.home, &created_files, &cleanup_roots);
            return Err(err);
        }

        if summary_github_active {
            self.trigger_github_sync();
        }

        let configured_profiles: HashSet<String> = self
            .config()
            .profiles
            .iter()
            .map(|prof| prof.id.clone())
            .collect();
        let missing_profiles: Vec<String> = loaded
            .manifest
            .required_profiles
            .iter()
            .filter(|id| !configured_profiles.contains(*id))
            .cloned()
            .collect();

        let mut warnings = Vec::new();
        if !missing_profiles.is_empty() {
            warnings.push(format!(
                "Target host is missing profile(s) referenced by workflows: {} (configure them or use 'Copy profiles from This Mac' in Host Settings).",
                missing_profiles.join(", ")
            ));
        }
        if !loaded.manifest.secret_keys.is_empty() {
            warnings.push(format!(
                "Project secret(s) were not included in the bundle ({}); configure them on this host before running workflows that need them.",
                loaded.manifest.secret_keys.join(", ")
            ));
        }
        if !loaded.manifest.include_archived && loaded.manifest.skipped_archived_github_cards > 0 {
            warnings.push(format!(
                "Source export omitted {} archived GitHub Issue/PR card(s); still-open items on GitHub may be re-imported on sync.",
                loaded.manifest.skipped_archived_github_cards
            ));
        }
        if summary_github_active {
            warnings.push(
                "GitHub intake is active on the imported project; pause or delete the project on the source host to prevent duplicate polling and card updates."
                    .into(),
            );
        }

        Ok(json!({
            "imported": true,
            "project_id": pid,
            "name": final_project_name,
            "directory": target_directory,
            "include_archived": loaded.manifest.include_archived,
            "imported_cards": summary_imported_active,
            "imported_archived_cards": summary_imported_archived,
            "imported_tombstones": summary_imported_tombstones,
            "skipped_archived_cards": loaded.manifest.skipped_archived_cards,
            "missing_profiles": missing_profiles,
            "missing_secrets": loaded.manifest.secret_keys,
            "github_paused": summary_github_paused,
            "git_hints": loaded.manifest.git_hints,
            "warnings": warnings,
        }))
    }
}
