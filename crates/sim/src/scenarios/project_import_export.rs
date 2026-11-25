//! Scenario #15 — Multi-machine `project.export` & `project.import` lifecycle & isolation:
//! Exercises REAL production `project.export`, `project.import`, `project.archive`,
//! `project.archived.list`, and `project.restore` RPCs across 3 hosts
//! (`local-server`, `remote-a`, `remote-b`) with 2 concurrent `MultiHostClient` instances
//! (`ui-client` desktop + `ui-client-2` mobile):
//! (a) Rich source project on `local-server` (multi-stage cards, comments, attention, `CardRead`,
//!     real managed attachments + agent report file + archived-card attachment archive + Project
//!     Note, workflow definition with active & paused cron schedules, `SimGitHub` repository
//!     binding with synced Issue/PR + real removed GitHub tombstone, project-scoped env + secrets,
//!     and global env + secrets),
//! (b) Real `project.export` producing a `.canban.tar.zst` bundle (`files_count > 0`,
//!     `exported_tombstones == 1`, secret omission & zero plaintext secret leakage in raw archive,
//!     decompressed `manifest.json`, `records.json`, or any bundled managed file),
//! (c) Real `project.import` onto `remote-a` (`pause_github: true`) and `remote-b`
//!     (`pause_github: false`), plus duplicate-import rejection on `local-server` — verifying
//!     multi-host identical-ID qualification, extracted managed file integrity & isolation,
//!     GitHub `pause_github`/tombstone preservation without duplicate intake, and imported cron
//!     schedules (`ScheduleState`) firing independently across hosts,
//! (d) Concurrent independent mutations (create, update, move, comment, archive, `CardRead`),
//!     simultaneous workflows/agents on imported copies across hosts, isolated project-scoped
//!     env edits vs synced global env, and full `project.archive` / `project.archived.list` /
//!     `project.restore` lifecycle on `remote-b`,
//! (e) Partition + offline outbox mutations + `crash_server`/`bounce_server` + `bounce_client`
//!     recovery verified strictly by `InvariantOracle` across ALL clients and ALL 3 servers.
use crate::{
    github::SimIssue,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorld, SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{Card, EnvEntryMeta, unqualified_entity_id};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::time::Duration;

const BUNDLE_WORKFLOW_YAML: &str = r#"schema: 12
id: bundle-flow
name: Exported Bundle Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: work
  bundle_cron_active:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    enabled: true
    start_at: finish
    card:
      column: backlog
      title: "Scheduled Copy Card #{{run_number}}"
      body: "Triggered by imported active cron"
  bundle_cron_paused:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    enabled: false
    start_at: finish
    card:
      column: backlog
      title: "Scheduled Paused Card #{{run_number}}"
      body: "Triggered when re-enabled"
steps:
  - id: work
    uses: agent
    with:
      profile: developer
      instruction: Work on imported project copy
    next: gate
  - id: gate
    uses: approval
    with:
      title: Review imported work
    next: finish
    on_changes_requested: work
  - id: finish
    uses: complete
"#;

fn make_gh_issue(number: u64, title: &str, author: &str, is_pr: bool) -> SimIssue {
    SimIssue {
        number,
        title: title.into(),
        body: format!("GitHub body for #{number}"),
        state: "open".into(),
        author: author.into(),
        labels: vec![if is_pr { "pr" } else { "bug" }.into()],
        updated_at: "2026-06-01T00:00:00Z".into(),
        is_pr,
        draft: false,
        merged: false,
        head_sha: if is_pr {
            format!("sha-bundle-pr-{number}")
        } else {
            String::new()
        },
        head_ref: if is_pr {
            format!("feat/bundle-pr-{number}")
        } else {
            String::new()
        },
        base_ref: if is_pr { "main".into() } else { String::new() },
        check_conclusion: is_pr.then(|| "success".into()),
    }
}

/// Reads all Card rows (including `removed_at.is_some()` tombstones filtered out of `Snapshot`)
/// directly from a simulated server's SQLite database read-only.
fn read_all_db_cards(world: &SimWorld, host_name: &str) -> Result<Vec<Card>> {
    let db_path = world.servers[host_name].home.join("canban.sqlite");
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("[{host_name}] open {} read-only", db_path.display()))?;
    let mut stmt = conn.prepare("SELECT data FROM cards ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut cards = Vec::new();
    for row in rows {
        cards.push(serde_json::from_str::<Card>(&row?)?);
    }
    Ok(cards)
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "project_import_export");
    cfg.remote_hosts = vec!["remote-a".into(), "remote-b".into()];
    cfg.extra_clients = vec!["ui-client-2".into()];
    cfg.min_latency_ms = 3;
    cfg.max_latency_ms = 20;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // ---------------------------------------------------------------------
        // (a) Create rich source project on `local-server`
        // ---------------------------------------------------------------------
        let gh_repo = format!("acme/atlas-bundle-{seed}");
        let gh_repo_id = 5_000 + (seed % 1_000);
        world.github.add_repo(&gh_repo, gh_repo_id, "main");
        world
            .github
            .set_collaborators(&gh_repo, vec!["alice".into(), "bot".into()]);
        world.github.upsert_issue(
            &gh_repo,
            make_gh_issue(10, "GH Active Issue", "alice", false),
        );
        world
            .github
            .upsert_issue(&gh_repo, make_gh_issue(11, "GH Open PR", "alice", true));
        world.github.upsert_issue(
            &gh_repo,
            make_gh_issue(12, "GH Tombstone Issue", "alice", false),
        );

        let dir_local = world.project_dir(&format!("bundle-local-{seed}"))?;
        let dir_a = world.project_dir(&format!("bundle-remote-a-{seed}"))?;
        let dir_b = world.project_dir(&format!("bundle-remote-b-{seed}"))?;

        let proj_local = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Atlas Source {seed}"),
                "directory": dir_local,
            }),
            None,
        )?;
        let pid_local = proj_local["id"].as_str().unwrap().to_owned();

        // Install workflow definition (including active + paused cron schedules) on source project.
        let wf_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_local }),
            Some(&pid_local),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_local,
                "revision": wf_sources["revision"],
                "sources": [{ "filename": "bundle-flow.yaml", "source": BUNDLE_WORKFLOW_YAML }],
            }),
            Some(&pid_local),
        )?;

        // Confirm both cron schedules were registered on `local-server`.
        let local_scheds = world.client_call(
            "schedule.list",
            json!({ "project_id": pid_local }),
            Some(&pid_local),
        )?;
        let local_sched_arr = local_scheds["schedules"]
            .as_array()
            .context("schedule.list missing schedules array")?;
        ensure!(
            local_sched_arr
                .iter()
                .any(|s| s["rule_id"] == "bundle_cron_active"
                    && s["enabled"] == true
                    && s["next_run_at"].as_u64().is_some())
                && local_sched_arr
                    .iter()
                    .any(|s| s["rule_id"] == "bundle_cron_paused"
                        && s["enabled"] == false
                        && s["next_run_at"].is_null()),
            "Unexpected source project schedules before export: {local_scheds}"
        );

        // Bind source project to SimGitHub, sync Issue #10, PR #11, Issue #12, then remove #12
        // via real `card.remove` to create a durable GitHub tombstone before export!
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_local,
                "repository": gh_repo,
                "intake": "all",
                "pr_intake": "all",
                "include_drafts": true,
            }),
            Some(&pid_local),
        )?;
        world.wait_until(15_000, |w| {
            let snap = w.server("local-server").unwrap().snapshot()?;
            let synced = snap
                .projects
                .iter()
                .find(|p| p.id == pid_local)
                .and_then(|p| p.github.as_ref())
                .and_then(|g| g.synced_at)
                .is_some();
            let gh_count = snap
                .cards
                .iter()
                .filter(|c| c.project_id == pid_local && c.github_issue().is_some())
                .count();
            Ok(synced && gh_count == 3)
        })?;
        let (gh12_cid, gh12_rev) = {
            let snap = world.server("local-server").unwrap().snapshot()?;
            let c = snap
                .cards
                .iter()
                .find(|c| {
                    c.project_id == pid_local && c.github_issue().is_some_and(|r| r.number == 12)
                })
                .context("Missing synced GitHub Issue #12 on local-server")?;
            (c.id.clone(), c.revision)
        };
        let rem_res = world.client_call(
            "card.remove",
            json!({ "card_id": gh12_cid, "revision": gh12_rev }),
            Some(&pid_local),
        )?;
        ensure!(
            rem_res["removed"].as_bool() == Some(true),
            "card.remove failed for GitHub Issue #12: {rem_res}"
        );
        // Confirm immediate `github.sync` on source does not resurrect tombstoned #12.
        let _ = world.client_call(
            "github.sync",
            json!({ "project_id": pid_local }),
            Some(&pid_local),
        )?;
        ensure!(
            world
                .server("local-server")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .filter(|c| c.project_id == pid_local && c.github_issue().is_some())
                .count()
                == 2,
            "GitHub tombstone #12 was resurrected on local-server before export"
        );

        // Create 3 local cards:
        // - Card 1: backlog + comment + CardRead + real managed attachment
        // - Card 2: develop + comment + workflow blocked report file (managed report + human_gate)
        // - Card 3: attachment + archived (produces real `archives/<pid>/...` managed file)
        // Plus a real Project Note (`note.save`).
        let c1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": "Backlog Feature",
                "body": format!("backlog-seed-{seed}"),
            }),
            Some(&pid_local),
        )?;
        let cid1_local = c1["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid1_local, "body": "initial-backlog-comment" }),
            Some(&pid_local),
        )?;
        let _ = world.client_call(
            "notifications.read",
            json!({ "card_id": cid1_local, "sequence": 1 }),
            Some(&pid_local),
        )?;
        let spec_text = format!("atlas-spec-payload-seed-{seed}\nmanaged-attachment-line-2\n");
        let spec_bytes = spec_text.as_bytes().to_vec();
        let att1 = world.client_call(
            "attachment.add",
            json!({
                "card_id": cid1_local,
                "name": "spec-doc.txt",
                "media_type": "text/plain",
                "bytes": spec_bytes,
            }),
            Some(&pid_local),
        )?;
        let art1_path = att1["path"]
            .as_str()
            .context("attachment.add missing path")?
            .to_owned();

        let note_body = format!("Multi-host note body seed={seed}");
        let note_res = world.client_call(
            "note.save",
            json!({
                "project_id": pid_local,
                "title": "Bundle Design Note",
                "body": note_body,
            }),
            Some(&pid_local),
        )?;
        ensure!(
            note_res["title"].as_str() == Some("Bundle Design Note"),
            "Unexpected note.save receipt: {note_res}"
        );

        let c2 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": "Active Attention Card",
                "body": format!("active-seed-{seed}"),
            }),
            Some(&pid_local),
        )?;
        let cid2_local = c2["id"].as_str().unwrap().to_owned();
        let c2_rev = c2["revision"].as_u64().unwrap_or(1);
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid2_local, "revision": c2_rev, "stage": "develop" }),
            Some(&pid_local),
        )?;
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid2_local, "body": "attention-context-comment" }),
            Some(&pid_local),
        )?;
        // Drive Card 2 through workflow start -> blocked with report `body` (writes a real
        // managed report `.md` file + creates attention & `human_gate`).
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "bundle-flow",
                "card_id": cid2_local,
                "entry": "start",
                "request_key": format!("bundle-src-wf-{seed}"),
            }),
            Some(&pid_local),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local-server").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid2_local && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        let c2_tid = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid2_local))
            .unwrap()
            .id;
        let rep2_body = format!("# Blocked Diagnostic Report\nseed={seed}\nneeds-human-review\n");
        world.server_call(
            "local-server",
            "execution.report",
            json!({
                "terminal_id": c2_tid,
                "state": "blocked",
                "message": "Needs input before export",
                "body": rep2_body,
                "request_key": format!("bundle-src-blk-{seed}"),
            }),
        )?;
        world.wait_until(5_000, |w| {
            let snap = w.server("local-server").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .any(|c| c.id == cid2_local && c.attention.is_some()))
        })?;
        let rep2_path = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .reports
            .into_iter()
            .find(|r| r.card_id == cid2_local)
            .and_then(|r| r.path)
            .context("Expected managed report file path on local-server for Card 2")?;

        let c3 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": "Archived History Card",
                "body": "will-be-archived",
            }),
            Some(&pid_local),
        )?;
        let cid3_local = c3["id"].as_str().unwrap().to_owned();
        let archived_att_text = format!("archived-evidence-seed-{seed}");
        let att3 = world.client_call(
            "attachment.add",
            json!({
                "card_id": cid3_local,
                "name": "archived-evidence.txt",
                "media_type": "text/plain",
                "bytes": archived_att_text.as_bytes().to_vec(),
            }),
            Some(&pid_local),
        )?;
        let art3_path = att3["path"]
            .as_str()
            .context("attachment.add on c3 missing path")?
            .to_owned();
        let c3_rev = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == cid3_local)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.archive",
            json!({ "card_id": cid3_local, "revision": c3_rev }),
            Some(&pid_local),
        )?;

        // Save project-scoped env + secret on `local` and global env + secret on Desktop.
        let proj_secret = format!("sk-proj-bundle-{seed}");
        let global_secret = format!("sk-global-bundle-{seed}");
        oracle.track_secret(proj_secret.clone());
        oracle.track_secret(global_secret.clone());

        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "project",
                "target_id": pid_local,
                "entries": {
                    "PROJ_TIER": { "kind": "plain", "value": "source-local" },
                    "PROJ_SECRET": { "kind": "secret", "value": proj_secret },
                },
            }),
            Some(&pid_local),
        )?;
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "GLOBAL_SYNC_KEY": { "kind": "plain", "value": format!("g-v1-{seed}") },
                    "GLOBAL_SECRET": { "kind": "secret", "value": global_secret },
                },
            }),
            None,
        )?;
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(15_000)?;
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // (b) Call REAL `project.export` on `local-server`
        // ---------------------------------------------------------------------
        let bundle_path = world.temp_file_path(&format!("atlas-{seed}.canban.tar.zst"))?;
        let bundle_path_str = bundle_path.to_string_lossy().into_owned();
        let export_res = world.client_call(
            "project.export",
            json!({
                "project_id": pid_local,
                "output": bundle_path_str,
                "include_archived": true,
                "overwrite": true,
            }),
            Some(&pid_local),
        )?;
        ensure!(
            export_res["exported"].as_bool() == Some(true),
            "project.export failed: {export_res}"
        );
        // Active cards: c1, c2, GH #10, GH #11 (4); Archived: c3 (1); Tombstones: GH #12 (1).
        ensure!(
            export_res["exported_cards"].as_u64() == Some(4)
                && export_res["exported_archived_cards"].as_u64() == Some(1)
                && export_res["exported_tombstones"].as_u64() == Some(1),
            "Unexpected exported card/tombstone counts: {export_res}"
        );
        let exported_files_count = export_res["files_count"].as_u64().unwrap_or(0);
        ensure!(
            exported_files_count >= 3,
            "Expected >= 3 real managed files in exported bundle (attachment + report + archived card archive), got {export_res}"
        );
        ensure!(
            export_res["secret_keys"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("PROJ_SECRET"))),
            "Expected PROJ_SECRET omitted & listed in secret_keys: {export_res}"
        );
        ensure!(bundle_path.exists(), "Exported bundle file missing on disk");

        // Security & integrity invariant: inspect raw compressed archive bytes AND decompressed
        // `manifest.json`, `records.json`, AND every bundled managed file (`files.len() > 0`).
        let bundle_raw = std::fs::read(&bundle_path)?;
        let loaded_bundle = canban_context::project_bundle::read_project_bundle(&bundle_path)?;
        ensure!(
            loaded_bundle.manifest.exported_cards == 4
                && loaded_bundle.manifest.exported_archived_cards == 1
                && loaded_bundle.manifest.exported_tombstones == 1
                && loaded_bundle.manifest.files.len() as u64 == exported_files_count
                && !loaded_bundle.manifest.files.is_empty()
                && loaded_bundle
                    .manifest
                    .secret_keys
                    .iter()
                    .any(|k| k == "PROJ_SECRET"),
            "Unexpected decompressed bundle manifest: {:?}",
            loaded_bundle.manifest
        );
        ensure!(
            loaded_bundle
                .manifest
                .files
                .iter()
                .any(|f| f.path == art1_path)
                && loaded_bundle
                    .manifest
                    .files
                    .iter()
                    .any(|f| f.path == rep2_path)
                && loaded_bundle
                    .manifest
                    .files
                    .iter()
                    .any(|f| f.path.starts_with(&format!("archives/{pid_local}/"))),
            "Exported bundle manifest missing expected managed files (art1={art1_path}, rep2={rep2_path}): {:?}",
            loaded_bundle.manifest.files
        );
        let manifest_json = serde_json::to_string(&loaded_bundle.manifest)?;
        let records_text = String::from_utf8(loaded_bundle.records_json.clone())
            .context("decompressed bundle records.json must be valid UTF-8")?;
        for (label, secret) in [
            ("project secret", proj_secret.as_str()),
            ("global secret", global_secret.as_str()),
        ] {
            ensure!(
                !bundle_raw
                    .windows(secret.len())
                    .any(|w| w == secret.as_bytes()),
                "SECURITY VIOLATION: {label} plaintext leaked into raw exported bundle bytes!"
            );
            ensure!(
                !manifest_json.contains(secret),
                "SECURITY VIOLATION: {label} plaintext leaked into decompressed bundle manifest.json!"
            );
            ensure!(
                !records_text.contains(secret),
                "SECURITY VIOLATION: {label} plaintext leaked into decompressed bundle records.json!"
            );
        }
        let inspect_dir = world.project_dir(&format!("bundle-inspect-{seed}"))?;
        let inspect_root = std::path::PathBuf::from(&inspect_dir);
        let allowed_prefixes: Vec<String> = loaded_bundle
            .manifest
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let extracted_inspect =
            canban_context::project_bundle::extract_project_bundle_files_with_durability(
                &bundle_path,
                &inspect_root,
                &allowed_prefixes,
                canban_context::FsDurability::NoSync,
            )?;
        ensure!(
            extracted_inspect.len() == loaded_bundle.manifest.files.len(),
            "Expected all {} bundled managed files extracted during inspection, got {}",
            loaded_bundle.manifest.files.len(),
            extracted_inspect.len()
        );
        for path in &extracted_inspect {
            let bytes = std::fs::read(path)?;
            for (label, secret) in [
                ("project secret", proj_secret.as_str()),
                ("global secret", global_secret.as_str()),
            ] {
                ensure!(
                    !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                    "SECURITY VIOLATION: {label} plaintext leaked into bundled file {}!",
                    path.display()
                );
            }
        }
        world.hit("bundle.exported");

        // Pause GitHub intake on source `local-server` after export (cutover hygiene) so we can
        // deterministically verify `pause_github` behavior across the 3 copies below.
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_local,
                "repository": gh_repo,
                "intake": "all",
                "pr_intake": "all",
                "include_drafts": true,
                "paused": true,
            }),
            Some(&pid_local),
        )?;

        // ---------------------------------------------------------------------
        // (c) Call REAL `project.import` onto `remote-a`, `remote-b`, and test duplicate on `local`
        // ---------------------------------------------------------------------
        // Duplicate import onto the SAME host where `pid_local` already exists must be rejected!
        let dup_dir = world.project_dir(&format!("bundle-dup-local-{seed}"))?;
        let dup_err = world
            .client_call(
                "project.import",
                json!({
                    "host_id": "local",
                    "file": bundle_path_str,
                    "directory": dup_dir,
                    "name": "Duplicate Local",
                }),
                None,
            )
            .expect_err("Importing duplicate project_id onto the same host must be rejected");
        ensure!(
            dup_err.to_string().contains("already exists on this host"),
            "Unexpected duplicate import error: {dup_err:#}"
        );

        // Import onto `remote-a` with `pause_github: true` and onto `remote-b` with active GitHub
        // intake (`pause_github: false` omitted) via `MultiHostClient`!
        let imp_a = world.client_call(
            "project.import",
            json!({
                "host_id": "remote-a",
                "file": bundle_path_str,
                "directory": dir_a,
                "name": format!("Atlas Remote A {seed}"),
                "pause_github": true,
            }),
            None,
        )?;
        ensure!(
            imp_a["imported"].as_bool() == Some(true)
                && imp_a["imported_cards"].as_u64() == Some(4)
                && imp_a["imported_archived_cards"].as_u64() == Some(1)
                && imp_a["imported_tombstones"].as_u64() == Some(1)
                && imp_a["github_paused"].as_bool() == Some(true),
            "Unexpected project.import(remote-a) result: {imp_a}"
        );
        let imp_b = world.client_call(
            "project.import",
            json!({
                "host_id": "remote-b",
                "file": bundle_path_str,
                "directory": dir_b,
                "name": format!("Atlas Remote B {seed}"),
            }),
            None,
        )?;
        ensure!(
            imp_b["imported"].as_bool() == Some(true)
                && imp_b["imported_cards"].as_u64() == Some(4)
                && imp_b["imported_archived_cards"].as_u64() == Some(1)
                && imp_b["imported_tombstones"].as_u64() == Some(1)
                && imp_b["github_paused"].as_bool() == Some(false),
            "Unexpected project.import(remote-b) result: {imp_b}"
        );
        ensure!(
            imp_b["warnings"]
                .as_array()
                .is_some_and(|w| w.iter().any(|s| s
                    .as_str()
                    .is_some_and(|m| m.contains("GitHub intake is active")))),
            "Expected active-GitHub warning on unpaused remote-b import: {imp_b}"
        );

        // Wait until BOTH `ui-client` (3 copies: local, remote-a, remote-b) and `ui-client-2`
        // (2 remote copies: remote-a, remote-b) observe all imported copies!
        world.wait_until(15_000, |w| {
            let s1 = w.client_merged_snapshot_on("ui-client")?;
            let s2 = w.client_merged_snapshot_on("ui-client-2")?;
            Ok(s1.projects.len() == 3 && s2.projects.len() == 2)
        })?;
        oracle.check_step_invariants(world)?;

        // Resolve the exact client-visible (qualified where colliding!) project & card IDs per host.
        let (pid_a, pid_b, cid1_a, cid1_b, cid2_a, cid2_b, cid3_a) = {
            let merged = world.client_merged_snapshot_on("ui-client")?;
            let find_proj = |host: &str| -> String {
                merged
                    .projects
                    .iter()
                    .find(|p| merged.entity_hosts.get(&p.id).map(String::as_str) == Some(host))
                    .expect("missing host project copy")
                    .id
                    .clone()
            };
            let pa = find_proj("remote-a");
            let pb = find_proj("remote-b");
            ensure!(
                pa != pid_local && pb != pid_local && pa != pb,
                "Multi-host identical imported project IDs must be uniquely qualified: local={pid_local} a={pa} b={pb}"
            );
            let find_card = |host: &str, raw_id: &str| -> String {
                merged
                    .cards
                    .iter()
                    .find(|c| {
                        merged.entity_hosts.get(&c.id).map(String::as_str) == Some(host)
                            && unqualified_entity_id(&c.id) == raw_id
                    })
                    .expect("missing host card copy")
                    .id
                    .clone()
            };
            (
                pa,
                pb,
                find_card("remote-a", &cid1_local),
                find_card("remote-b", &cid1_local),
                find_card("remote-a", &cid2_local),
                find_card("remote-b", &cid2_local),
                find_card("remote-a", &cid3_local),
            )
        };
        let (pid_b_c2, cid1_b_c2, cid2_b_c2) = {
            let m2 = world.client_merged_snapshot_on("ui-client-2")?;
            let pb2 = m2
                .projects
                .iter()
                .find(|p| m2.entity_hosts.get(&p.id).map(String::as_str) == Some("remote-b"))
                .unwrap()
                .id
                .clone();
            let find_c2 = |raw_id: &str| -> String {
                m2.cards
                    .iter()
                    .find(|c| {
                        m2.entity_hosts.get(&c.id).map(String::as_str) == Some("remote-b")
                            && unqualified_entity_id(&c.id) == raw_id
                    })
                    .unwrap()
                    .id
                    .clone()
            };
            (pb2, find_c2(&cid1_local), find_c2(&cid2_local))
        };

        // Verify imported active workflow runs on remote-a/b were cleanly cancelled on import,
        // and verify archived card `c3` was imported as archived on both target hosts.
        for host in ["remote-a", "remote-b"] {
            let srv_snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                srv_snap
                    .workflow_runs
                    .iter()
                    .all(|r| !r.active() && r.state == "cancelled"),
                "[{host}] imported active workflow run must be cancelled on import: {:?}",
                srv_snap.workflow_runs
            );
            let archived_c3 = srv_snap
                .cards
                .iter()
                .find(|c| c.id == cid3_local)
                .with_context(|| {
                    format!("[{host}] missing imported archived card c3 ({cid3_local})")
                })?;
            ensure!(
                archived_c3.archived_at.is_some()
                    && !archived_c3.active()
                    && archived_c3.archive.is_some(),
                "[{host}] imported archived card c3 must remain archived with archive path: {archived_c3:?}"
            );
        }

        // ---------------------------------------------------------------------
        // Requirement 1: Verify real managed files extracted into each host's own
        // isolated project storage dir, readable via `resource.read`, and isolated
        // when modified/added on `remote-a`!
        // ---------------------------------------------------------------------
        for (srv_host, host_pid, host_cid1, host_cid2) in [
            ("remote-a", pid_a.as_str(), cid1_a.as_str(), cid2_a.as_str()),
            ("remote-b", pid_b.as_str(), cid1_b.as_str(), cid2_b.as_str()),
        ] {
            let home = &world.servers[srv_host].home;
            let extracted_att1 = home.join(&art1_path);
            let extracted_rep2 = home.join(&rep2_path);
            ensure!(
                extracted_att1.is_file(),
                "[{srv_host}] extracted attachment file missing on disk: {}",
                extracted_att1.display()
            );
            ensure!(
                extracted_rep2.is_file(),
                "[{srv_host}] extracted report file missing on disk: {}",
                extracted_rep2.display()
            );
            ensure!(
                std::fs::read(&extracted_att1)? == spec_bytes,
                "[{srv_host}] extracted attachment bytes diverged from original"
            );
            ensure!(
                std::fs::read(&extracted_rep2)? == rep2_body.as_bytes(),
                "[{srv_host}] extracted report bytes diverged from original"
            );

            let att_read = world.client_call(
                "resource.read",
                json!({ "card_id": host_cid1, "path": art1_path }),
                Some(host_pid),
            )?;
            ensure!(
                att_read["body"].as_str() == Some(spec_text.as_str()),
                "[{srv_host}] resource.read attachment mismatch: {att_read}"
            );
            let rep_read = world.client_call(
                "resource.read",
                json!({ "card_id": host_cid2, "path": rep2_path }),
                Some(host_pid),
            )?;
            ensure!(
                rep_read["body"].as_str() == Some(rep2_body.as_str()),
                "[{srv_host}] resource.read report mismatch: {rep_read}"
            );

            let notes_val = world.client_call(
                "note.list",
                json!({ "project_id": host_pid }),
                Some(host_pid),
            )?;
            ensure!(
                notes_val.as_array().is_some_and(|arr| arr.iter().any(|n| {
                    n["title"].as_str() == Some("Bundle Design Note")
                        && n["body"].as_str() == Some(note_body.as_str())
                })),
                "[{srv_host}] imported project missing Bundle Design Note: {notes_val}"
            );
        }

        // Round-trip imported archived card `c3` through `card.restore` on `remote-a` only,
        // verifying both its metadata AND its archived attachment restore cleanly while
        // `local-server` & `remote-b` copies stay archived!
        let c3_a_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid3_local)
            .unwrap()
            .revision;
        let restored_c3 = world.client_call(
            "card.restore",
            json!({ "card_id": cid3_a, "revision": c3_a_rev }),
            Some(&pid_a),
        )?;
        let restored_c3_rev = restored_c3["revision"].as_u64().unwrap_or(0);
        ensure!(
            restored_c3["archived_at"].is_null() && restored_c3_rev > c3_a_rev,
            "Unexpected card.restore receipt on remote-a: {restored_c3}"
        );
        oracle.record_committed_card("remote-a", &cid3_a, &pid_a, restored_c3_rev);
        let restored_c3_get = world.client_call(
            "card.get",
            json!({ "card_id": cid3_a, "view": "full" }),
            Some(&pid_a),
        )?;
        ensure!(
            restored_c3_get["title"].as_str() == Some("Archived History Card")
                && restored_c3_get["body"].as_str() == Some("will-be-archived")
                && restored_c3_get["archived_at"].is_null(),
            "Restored archived card c3 failed round-trip card.get: {restored_c3_get}"
        );
        let restored_c3_att = world.client_call(
            "resource.read",
            json!({ "card_id": cid3_a, "path": art3_path }),
            Some(&pid_a),
        )?;
        ensure!(
            restored_c3_att["body"].as_str() == Some(archived_att_text.as_str()),
            "Restored archived card c3 attachment failed round-trip resource.read: {restored_c3_att}"
        );
        for host in ["local-server", "remote-b"] {
            let still_archived = world
                .server(host)
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == cid3_local)
                .with_context(|| {
                    format!("[{host}] missing archived card c3 after remote-a restore")
                })?;
            ensure!(
                still_archived.archived_at.is_some() && !still_archived.active(),
                "[{host}] card.restore on remote-a unexpectedly affected sibling copy: {still_archived:?}"
            );
        }

        // Add a brand-new attachment & note ONLY on `remote-a`, and verify zero clobber or leak
        // into `local-server` or `remote-b` storage dirs or DB records!
        let remote_a_only_text = format!("remote-a-only-attachment-seed-{seed}");
        let att_a_only = world.client_call(
            "attachment.add",
            json!({
                "card_id": cid1_a,
                "name": "remote-a-only.txt",
                "media_type": "text/plain",
                "bytes": remote_a_only_text.as_bytes().to_vec(),
            }),
            Some(&pid_a),
        )?;
        let art_a_only_path = att_a_only["path"]
            .as_str()
            .context("remote-a attachment.add missing path")?
            .to_owned();
        let _ = world.client_call(
            "note.save",
            json!({
                "project_id": pid_a,
                "title": "Remote-A Exclusive Note",
                "body": "isolated-to-remote-a",
            }),
            Some(&pid_a),
        )?;
        ensure!(
            world.servers["remote-a"]
                .home
                .join(&art_a_only_path)
                .is_file(),
            "New remote-a attachment missing from remote-a home"
        );
        let att_a_only_read = world.client_call(
            "resource.read",
            json!({ "card_id": cid1_a, "path": art_a_only_path }),
            Some(&pid_a),
        )?;
        ensure!(
            att_a_only_read["body"].as_str() == Some(remote_a_only_text.as_str()),
            "resource.read failed for new remote-a attachment: {att_a_only_read}"
        );
        for (sibling_host, sibling_pid) in [("local-server", &pid_local), ("remote-b", &pid_b)] {
            ensure!(
                !world.servers[sibling_host]
                    .home
                    .join(&art_a_only_path)
                    .exists(),
                "[{sibling_host}] remote-a managed attachment leaked onto sibling disk!"
            );
            ensure!(
                std::fs::read(world.servers[sibling_host].home.join(&art1_path))? == spec_bytes,
                "[{sibling_host}] sibling original attachment was clobbered!"
            );
            let sib_notes = world.client_call(
                "note.list",
                json!({ "project_id": sibling_pid }),
                Some(sibling_pid),
            )?;
            ensure!(
                !sib_notes.to_string().contains("Remote-A Exclusive Note"),
                "[{sibling_host}] remote-a note leaked into sibling project: {sib_notes}"
            );
        }
        world.hit("bundle.managed_files_roundtrip");
        world.hit("bundle.archived_card_restored");
        world.hit("bundle.imported_multi_host");

        // ---------------------------------------------------------------------
        // Requirement 3: GitHub-linked multi-machine import (`pause_github`, tombstones,
        // no duplicate GitHub intake or tombstone corruption across machines)
        // ---------------------------------------------------------------------
        for host in ["local-server", "remote-a", "remote-b"] {
            let db_cards = read_all_db_cards(world, host)?;
            ensure!(
                db_cards.iter().any(|c| {
                    c.project_id == pid_local
                        && c.removed_at.is_some()
                        && c.github_issue().is_some_and(|r| r.number == 12)
                }),
                "[{host}] GitHub tombstone for Issue #12 missing from SQLite cards table!"
            );
        }
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .projects
                .iter()
                .find(|p| p.id == pid_local)
                .and_then(|p| p.github.as_ref())
                .is_some_and(|g| g.paused),
            "remote-a project.github must be paused after pause_github=true import"
        );
        ensure!(
            world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .projects
                .iter()
                .find(|p| p.id == pid_local)
                .and_then(|p| p.github.as_ref())
                .is_some_and(|g| !g.paused),
            "remote-b project.github must remain unpaused after default import"
        );

        // Publish a brand-new Issue #13 on SimGitHub after import.
        world.github.upsert_issue(
            &gh_repo,
            make_gh_issue(13, "GH Post-Import Issue", "alice", false),
        );

        // Paused hosts (`local-server` & `remote-a`) must perform ZERO GitHub requests on
        // `github.sync`, must NOT pick up #13, and must NOT resurrect tombstoned #12.
        world.github.reset_stats();
        let _ = world.client_call(
            "github.sync",
            json!({ "project_id": pid_local }),
            Some(&pid_local),
        )?;
        let _ = world.client_call("github.sync", json!({ "project_id": pid_a }), Some(&pid_a))?;
        ensure!(
            world.github.stats().total_requests == 0,
            "Paused GitHub copies unexpectedly polled GitHub: {:?}",
            world.github.stats()
        );
        for host in ["local-server", "remote-a"] {
            let gh_nums: Vec<u64> = world
                .server(host)
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .filter(|c| c.project_id == pid_local)
                .filter_map(|c| c.github_issue().map(|r| r.number))
                .collect();
            ensure!(
                gh_nums.len() == 2 && gh_nums.contains(&10) && gh_nums.contains(&11),
                "[{host}] paused copy should keep exactly GH #10 and #11, got {gh_nums:?}"
            );
        }

        // Active `remote-b` syncs #13 cleanly ONCE, with zero duplication of #10/#11 and zero
        // resurrection of tombstoned #12!
        let _ = world.client_call("github.sync", json!({ "project_id": pid_b }), Some(&pid_b))?;
        let rb_gh_nums: Vec<u64> = world
            .server("remote-b")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .filter(|c| c.project_id == pid_local)
            .filter_map(|c| c.github_issue().map(|r| r.number))
            .collect();
        ensure!(
            rb_gh_nums.len() == 3
                && rb_gh_nums.contains(&10)
                && rb_gh_nums.contains(&11)
                && rb_gh_nums.contains(&13)
                && !rb_gh_nums.contains(&12),
            "[remote-b] unexpected GitHub issue set after sync (duplicates or resurrected #12): {rb_gh_nums:?}"
        );

        // Re-enable GitHub on `remote-a` (`paused: false`), sync, verify #13 is imported ONCE
        // without duplicating #10/#11 or resurrecting #12, then pause `remote-a` again.
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_a,
                "repository": gh_repo,
                "intake": "all",
                "pr_intake": "all",
                "include_drafts": true,
                "paused": false,
            }),
            Some(&pid_a),
        )?;
        let _ = world.client_call("github.sync", json!({ "project_id": pid_a }), Some(&pid_a))?;
        let ra_gh_nums: Vec<u64> = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .filter(|c| c.project_id == pid_local)
            .filter_map(|c| c.github_issue().map(|r| r.number))
            .collect();
        ensure!(
            ra_gh_nums.len() == 3
                && ra_gh_nums.contains(&10)
                && ra_gh_nums.contains(&11)
                && ra_gh_nums.contains(&13)
                && !ra_gh_nums.contains(&12),
            "[remote-a] unexpected GitHub issue set after re-enabling sync: {ra_gh_nums:?}"
        );
        // Pause `remote-a` again; leave `remote-b` unpaused so `project.restore` later verifies
        // the production default `pause_github: true` transition!
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_a,
                "repository": gh_repo,
                "intake": "all",
                "pr_intake": "all",
                "include_drafts": true,
                "paused": true,
            }),
            Some(&pid_a),
        )?;
        world.hit("bundle.github_linked_import");

        // ---------------------------------------------------------------------
        // Requirement 2: Imported Cron Schedules (`ScheduleState`) across multiple machines
        // ---------------------------------------------------------------------
        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            let active_st = snap
                .schedule_states
                .iter()
                .find(|s| s.project_id == pid_local && s.rule_id == "bundle_cron_active")
                .with_context(|| {
                    format!("[{host}] missing imported bundle_cron_active schedule")
                })?;
            let paused_st = snap
                .schedule_states
                .iter()
                .find(|s| s.project_id == pid_local && s.rule_id == "bundle_cron_paused")
                .with_context(|| {
                    format!("[{host}] missing imported bundle_cron_paused schedule")
                })?;
            ensure!(
                active_st.enabled
                    && active_st.queued_triggers == 0
                    && active_st.next_run_at.is_some(),
                "[{host}] unexpected imported active schedule state: {active_st:?}"
            );
            ensure!(
                !paused_st.enabled
                    && paused_st.queued_triggers == 0
                    && paused_st.next_run_at.is_none(),
                "[{host}] unexpected imported paused schedule state: {paused_st:?}"
            );
        }

        // Verify MultiHostClient merged snapshot uniquely qualifies colliding ScheduleState IDs
        // across all 3 hosts!
        let (sched_active_l, sched_active_a, sched_active_b_c2, sched_paused_b_c2) = {
            let m1 = world.client_merged_snapshot_on("ui-client")?;
            let mut active_ids = Vec::new();
            for h in ["local", "remote-a", "remote-b"] {
                let st = m1
                    .schedule_states
                    .iter()
                    .find(|s| {
                        m1.entity_hosts.get(&s.id).map(String::as_str) == Some(h)
                            && s.rule_id == "bundle_cron_active"
                    })
                    .with_context(|| {
                        format!("ui-client merged snapshot missing bundle_cron_active for {h}")
                    })?;
                active_ids.push(st.id.clone());
            }
            ensure!(
                active_ids[0] != active_ids[1]
                    && active_ids[0] != active_ids[2]
                    && active_ids[1] != active_ids[2],
                "Colliding imported ScheduleState IDs must be uniquely qualified across hosts: {active_ids:?}"
            );

            let m2 = world.client_merged_snapshot_on("ui-client-2")?;
            let find_b_sched = |rule: &str| -> Result<String> {
                m2.schedule_states
                    .iter()
                    .find(|s| {
                        m2.entity_hosts.get(&s.id).map(String::as_str) == Some("remote-b")
                            && s.rule_id == rule
                    })
                    .map(|s| s.id.clone())
                    .with_context(|| format!("ui-client-2 missing remote-b schedule rule {rule}"))
            };
            (
                active_ids[0].clone(),
                active_ids[1].clone(),
                find_b_sched("bundle_cron_active")?,
                find_b_sched("bundle_cron_paused")?,
            )
        };

        // Independently flip `remote-b`'s schedules via `ui-client-2` (`bundle_cron_active` OFF,
        // `bundle_cron_paused` ON) while keeping `local-server` and `remote-a` untouched!
        let _ = world.client_call_on(
            "ui-client-2",
            "schedule.toggle",
            json!({ "schedule_id": sched_active_b_c2, "enabled": false }),
            Some(&pid_b_c2),
        )?;
        let _ = world.client_call_on(
            "ui-client-2",
            "schedule.toggle",
            json!({ "schedule_id": sched_paused_b_c2, "enabled": true }),
            Some(&pid_b_c2),
        )?;

        // Compute the next UTC minute boundary across all 3 servers and advance cleanly past ONE
        // cron boundary (never crossing a second boundary) in small steps checking invariants!
        let next_due_ms = ["local-server", "remote-a", "remote-b"]
            .into_iter()
            .filter_map(|h| {
                world
                    .server(h)
                    .unwrap()
                    .snapshot()
                    .ok()?
                    .schedule_states
                    .into_iter()
                    .filter(|s| s.project_id == pid_local && s.enabled)
                    .filter_map(|s| s.next_run_at)
                    .min()
            })
            .max()
            .context("Expected at least one enabled schedule next_run_at")?;
        let target_ms = next_due_ms.saturating_add(5_000);
        while world.now_ms() < target_ms {
            let remaining = target_ms.saturating_sub(world.now_ms());
            world.advance(Duration::from_millis(remaining.min(2_000)))?;
            oracle.check_step_invariants(world)?;
        }

        // Verify each host's scheduler fired ONLY its own enabled schedule ONCE with zero
        // duplicates or cross-host corruption!
        for (host, expected_rule, expected_title, forbidden_rule) in [
            (
                "local-server",
                "bundle_cron_active",
                "Scheduled Copy Card #1",
                "bundle_cron_paused",
            ),
            (
                "remote-a",
                "bundle_cron_active",
                "Scheduled Copy Card #1",
                "bundle_cron_paused",
            ),
            (
                "remote-b",
                "bundle_cron_paused",
                "Scheduled Paused Card #1",
                "bundle_cron_active",
            ),
        ] {
            let snap = world.server(host).unwrap().snapshot()?;
            let expected_runs: Vec<_> = snap
                .scheduled_runs
                .iter()
                .filter(|r| r.project_id == pid_local && r.rule_id == expected_rule)
                .collect();
            let forbidden_runs: Vec<_> = snap
                .scheduled_runs
                .iter()
                .filter(|r| r.project_id == pid_local && r.rule_id == forbidden_rule)
                .collect();
            ensure!(
                expected_runs.len() == 1 && expected_runs[0].state == "completed",
                "[{host}] expected exactly 1 completed ScheduledRun for {expected_rule}, got {:?}",
                snap.scheduled_runs
            );
            ensure!(
                forbidden_runs.is_empty(),
                "[{host}] disabled schedule {forbidden_rule} unexpectedly fired: {forbidden_runs:?}"
            );
            let cron_cards: Vec<_> = snap
                .cards
                .iter()
                .filter(|c| c.project_id == pid_local && c.title == expected_title)
                .collect();
            ensure!(
                cron_cards.len() == 1,
                "[{host}] expected exactly 1 scheduled card titled {expected_title:?}, got {}",
                cron_cards.len()
            );
        }

        // Disable all cron schedules across all 3 hosts before workflow/archive/fault phases so
        // later virtual-time advances cannot race with extra minute ticks.
        let _ = world.client_call(
            "schedule.toggle",
            json!({ "schedule_id": sched_active_l, "enabled": false }),
            Some(&pid_local),
        )?;
        let _ = world.client_call(
            "schedule.toggle",
            json!({ "schedule_id": sched_active_a, "enabled": false }),
            Some(&pid_a),
        )?;
        let _ = world.client_call_on(
            "ui-client-2",
            "schedule.toggle",
            json!({ "schedule_id": sched_paused_b_c2, "enabled": false }),
            Some(&pid_b_c2),
        )?;
        world.hit("bundle.schedules_multi_host");

        // ---------------------------------------------------------------------
        // (d) Concurrent independent ops across ALL 3 imported copies & 2 clients
        // ---------------------------------------------------------------------
        // 1. Independently update the SAME original exported Card 1 on all 3 hosts concurrently!
        let (rev1_l, rev1_a, rev1_b) = {
            let m = world.client_merged_snapshot_on("ui-client")?;
            let get_rev = |id: &str| {
                m.cards
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.revision)
                    .unwrap_or(1)
            };
            (get_rev(&cid1_local), get_rev(&cid1_a), get_rev(&cid1_b))
        };

        let h_upd_l = world.client_spawn_on(
            "ui-client",
            "card.update",
            json!({ "card_id": cid1_local, "revision": rev1_l, "title": "Local Copy Title" }),
            Some(&pid_local),
        )?;
        let h_upd_a = world.client_spawn_on(
            "ui-client",
            "card.update",
            json!({ "card_id": cid1_a, "revision": rev1_a, "title": "Remote-A Copy Title" }),
            Some(&pid_a),
        )?;
        let h_upd_b = world.client_spawn_on(
            "ui-client-2",
            "card.update",
            json!({ "card_id": cid1_b_c2, "revision": rev1_b, "title": "Remote-B Copy Title" }),
            Some(&pid_b_c2),
        )?;
        let r_l = world.await_call(h_upd_l, 20_000)?;
        let r_a = world.await_call(h_upd_a, 20_000)?;
        let r_b = world.await_call(h_upd_b, 20_000)?;
        oracle.record_committed_card(
            "local-server",
            &cid1_local,
            &pid_local,
            r_l["revision"].as_u64().unwrap(),
        );
        oracle.record_committed_card(
            "remote-a",
            &cid1_a,
            &pid_a,
            r_a["revision"].as_u64().unwrap(),
        );
        oracle.record_committed_card(
            "remote-b",
            &cid1_b,
            &pid_b,
            r_b["revision"].as_u64().unwrap(),
        );

        // Create new cards, move, comment, and archive independently on each copy!
        let new_l = world.client_call(
            "card.create",
            json!({ "project_id": pid_local, "title": "New On Local Only" }),
            Some(&pid_local),
        )?;
        let new_a = world.client_call(
            "card.create",
            json!({ "project_id": pid_a, "title": "New On Remote-A Only" }),
            Some(&pid_a),
        )?;
        let new_b = world.client_call_on(
            "ui-client-2",
            "card.create",
            json!({ "project_id": pid_b_c2, "title": "New On Remote-B Only" }),
            Some(&pid_b_c2),
        )?;
        let new_l_id = new_l["id"].as_str().unwrap().to_owned();
        let new_a_id = new_a["id"].as_str().unwrap().to_owned();
        let new_b_id = new_b["id"].as_str().unwrap().to_owned();
        oracle.record_committed_card(
            "local-server",
            &new_l_id,
            &pid_local,
            new_l["revision"].as_u64().unwrap(),
        );
        oracle.record_committed_card(
            "remote-a",
            &new_a_id,
            &pid_a,
            new_a["revision"].as_u64().unwrap(),
        );
        oracle.record_committed_card(
            "remote-b",
            &new_b_id,
            &pid_b,
            new_b["revision"].as_u64().unwrap(),
        );

        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid1_a, "body": "comment-only-on-remote-a" }),
            Some(&pid_a),
        )?;
        oracle.record_committed_comment("remote-a", &cid1_a, "comment-only-on-remote-a");
        let _ = world.client_call_on(
            "ui-client-2",
            "comment.add",
            json!({ "card_id": cid1_b_c2, "body": "comment-only-on-remote-b" }),
            Some(&pid_b_c2),
        )?;
        oracle.record_committed_comment("remote-b", &cid1_b, "comment-only-on-remote-b");

        // Move and archive a card ONLY on remote-a; verify local & remote-b copies are untouched!
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": new_a_id, "revision": new_a["revision"], "stage": "review" }),
            Some(&pid_a),
        )?;
        let new_a_after_move = world.client_call(
            "card.get",
            json!({ "card_id": new_a_id, "view": "full" }),
            Some(&pid_a),
        )?;
        let _ = world.client_call(
            "card.archive",
            json!({ "card_id": new_a_id, "revision": new_a_after_move["revision"] }),
            Some(&pid_a),
        )?;

        // Card 2 was exported while blocked with a `human_gate`: import cancels the source host's
        // active WorkflowRun while preserving `human_gate` with `terminal_id = None` so automation
        // cannot silently bypass human review on the target machine. Resolve the imported human gate
        // cleanly via `card.continue` on `remote-a` (ui-client) and `remote-b` (ui-client-2),
        // verifying `local-server`'s live blocked Card 2 stays untouched, then start fresh workflows
        // simultaneously on BOTH remote copies!
        let (c2_a_rev, c2_b_rev) = {
            let sa = world.server("remote-a").unwrap().snapshot()?;
            let sb = world.server("remote-b").unwrap().snapshot()?;
            let ca = sa.cards.iter().find(|c| c.id == cid2_local).unwrap();
            let cb = sb.cards.iter().find(|c| c.id == cid2_local).unwrap();
            ensure!(
                ca.human_gate
                    .as_ref()
                    .is_some_and(|g| g.terminal_id.is_none())
                    && cb
                        .human_gate
                        .as_ref()
                        .is_some_and(|g| g.terminal_id.is_none()),
                "Expected imported Card 2 to preserve human_gate with terminal_id=None"
            );
            ensure!(
                sa.workflow_runs.iter().all(|r| !r.active())
                    && sb.workflow_runs.iter().all(|r| !r.active()),
                "Expected imported active workflow runs to be cancelled on target hosts"
            );
            (ca.revision, cb.revision)
        };
        let _ = world.client_call(
            "card.continue",
            json!({ "card_id": cid2_a, "revision": c2_a_rev }),
            Some(&pid_a),
        )?;
        let _ = world.client_call_on(
            "ui-client-2",
            "card.continue",
            json!({ "card_id": cid2_b_c2, "revision": c2_b_rev }),
            Some(&pid_b_c2),
        )?;
        // Verify local-server's Card 2 still retains its original human_gate & live terminal!
        {
            let sl = world.server("local-server").unwrap().snapshot()?;
            let cl = sl.cards.iter().find(|c| c.id == cid2_local).unwrap();
            ensure!(
                cl.human_gate
                    .as_ref()
                    .is_some_and(|g| g.terminal_id.is_some()),
                "Local Card 2 human_gate was unexpectedly affected by remote card.continue!"
            );
        }

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "bundle-flow",
                "card_id": cid2_a,
                "entry": "start",
                "request_key": format!("bundle-wf-a-{seed}"),
            }),
            Some(&pid_a),
        )?;
        let _ = world.client_call_on(
            "ui-client-2",
            "workflow.start",
            json!({
                "workflow_id": "bundle-flow",
                "card_id": cid2_b_c2,
                "entry": "start",
                "request_key": format!("bundle-wf-b-{seed}"),
            }),
            Some(&pid_b_c2),
        )?;
        world.wait_until(12_000, |w| {
            let sa = w.server("remote-a").unwrap().snapshot()?;
            let sb = w.server("remote-b").unwrap().snapshot()?;
            let a_running = sa.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid2_local && a.terminal.is_some() && a.state == "running"
                })
            });
            let b_running = sb.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid2_local && a.terminal.is_some() && a.state == "running"
                })
            });
            Ok(a_running && b_running)
        })?;
        // Complete both imported-copy agent runs and approve both gates cleanly!
        for (srv_host, host_pid, tag) in [("remote-a", &pid_a, "a"), ("remote-b", &pid_b, "b")] {
            let tid = world
                .server(srv_host)
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid2_local))
                .unwrap()
                .id;
            world.server_call(
                srv_host,
                "execution.report",
                json!({
                    "terminal_id": tid,
                    "state": "completed",
                    "message": format!("Completed on {srv_host}"),
                    "request_key": format!("bundle-wf-done-{tag}-{seed}"),
                }),
            )?;
            world.wait_until(5_000, |w| {
                let s = w.server(srv_host).unwrap().snapshot()?;
                Ok(s.workflow_runs.iter().any(|r| {
                    r.card_id.as_deref() == Some(cid2_local.as_str())
                        && r.state == "waiting_approval"
                }))
            })?;
            let (wf_id, wf_visit, card_rev) = {
                let s = world.server(srv_host).unwrap().snapshot()?;
                let r = s
                    .workflow_runs
                    .iter()
                    .find(|r| {
                        r.card_id.as_deref() == Some(cid2_local.as_str())
                            && r.state == "waiting_approval"
                    })
                    .unwrap();
                let c = s.cards.iter().find(|c| c.id == cid2_local).unwrap();
                (r.id.clone(), r.visit, c.revision)
            };
            let q_wf_id = canban_types::qualify_host_id(srv_host, &wf_id);
            let _ = world.client_call(
                "workflow.decide",
                json!({
                    "run_id": q_wf_id,
                    "visit": wf_visit,
                    "revision": card_rev,
                    "decision": "approve",
                    "feedback": format!("Approved on {srv_host}"),
                    "request_key": format!("bundle-wf-app-{tag}-{seed}"),
                }),
                Some(host_pid),
            )?;
            world.wait_until(5_000, |w| {
                let s = w.server(srv_host).unwrap().snapshot()?;
                Ok(s.workflow_runs
                    .iter()
                    .any(|r| r.id == wf_id && r.state == "completed"))
            })?;
        }

        // Edit project-scoped env on `remote-a` and `remote-b` independently; assert sibling copies
        // remain isolated while global env DOES sync to all hosts!
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "project",
                "target_id": pid_a,
                "entries": {
                    "PROJ_TIER": { "kind": "plain", "value": "remote-a-isolated" },
                },
            }),
            Some(&pid_a),
        )?;
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "project",
                "target_id": pid_b,
                "entries": {
                    "PROJ_TIER": { "kind": "plain", "value": "remote-b-isolated" },
                },
            }),
            Some(&pid_b),
        )?;
        let global_v2 = format!("g-v2-{seed}");
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "GLOBAL_SYNC_KEY": { "kind": "plain", "value": global_v2 },
                },
            }),
            None,
        )?;
        oracle.record_committed_env(
            "local-server",
            "GLOBAL_SYNC_KEY",
            EnvEntryMeta::Plain {
                value: global_v2.clone(),
            },
        );
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(15_000)?;

        // Assert exact per-host project-env isolation AND global-env convergence across all 3 hosts!
        for (srv_host, expected_tier) in [
            ("local-server", "source-local"),
            ("remote-a", "remote-a-isolated"),
            ("remote-b", "remote-b-isolated"),
        ] {
            let preview =
                world.server_call(srv_host, "env.preview", json!({ "project_id": pid_local }))?;
            let tier_val = preview["rows"]
                .as_array()
                .and_then(|rows| rows.iter().find(|r| r["key"].as_str() == Some("PROJ_TIER")))
                .and_then(|r| r["display_value"].as_str());
            ensure!(
                tier_val == Some(expected_tier),
                "[{srv_host}] project-scoped env cross-talk! Expected PROJ_TIER={expected_tier:?}, got {tier_val:?}"
            );
            let snap = world.server(srv_host).unwrap().snapshot()?;
            ensure!(
                snap.global_env.get("GLOBAL_SYNC_KEY")
                    == Some(&EnvEntryMeta::Plain {
                        value: global_v2.clone(),
                    }),
                "[{srv_host}] global env failed to sync: {:?}",
                snap.global_env
            );
        }

        // Assert Card 1 titles, client-routed `card.list`, `timeline.list`, and `notifications.read`
        // remain strictly isolated per host even though all 3 copies share identical raw IDs!
        for (
            srv_host,
            client_pid,
            client_cid1,
            expected_title,
            expected_only_card,
            forbidden_comment,
        ) in [
            (
                "local-server",
                pid_local.as_str(),
                cid1_local.as_str(),
                "Local Copy Title",
                "New On Local Only",
                "comment-only-on-remote-a",
            ),
            (
                "remote-a",
                pid_a.as_str(),
                cid1_a.as_str(),
                "Remote-A Copy Title",
                "Remote-A Copy Title",
                "comment-only-on-remote-b",
            ),
            (
                "remote-b",
                pid_b.as_str(),
                cid1_b.as_str(),
                "Remote-B Copy Title",
                "New On Remote-B Only",
                "comment-only-on-remote-a",
            ),
        ] {
            let snap = world.server(srv_host).unwrap().snapshot()?;
            let card = snap.cards.iter().find(|c| c.id == cid1_local).unwrap();
            ensure!(
                card.title == expected_title,
                "[{srv_host}] Card 1 cross-host title corruption: expected {expected_title:?}, got {:?}",
                card.title
            );
            let page = world.client_call(
                "card.list",
                json!({ "project_id": client_pid, "limit": 20 }),
                Some(client_pid),
            )?;
            ensure!(
                page.to_string().contains(expected_only_card),
                "[{srv_host}] client-routed card.list missed expected host-specific card {expected_only_card:?}: {page}"
            );
            let tl = world.client_call(
                "timeline.list",
                json!({ "card_id": client_cid1, "tail": true, "limit": 50, "view": "full" }),
                Some(client_pid),
            )?;
            ensure!(
                !tl.to_string().contains(forbidden_comment),
                "[{srv_host}] client-routed timeline cross-talk detected: found {forbidden_comment:?} in {tl}"
            );
            let _ = world.client_call(
                "notifications.read",
                json!({ "card_id": client_cid1, "sequence": 2 }),
                Some(client_pid),
            )?;
        }
        world.hit("bundle.cross_host_ops_isolated");
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // Requirement 4: `project.archive` / `project.archived.list` / `project.restore`
        // lifecycle on `remote-b` while `local-server` and `remote-a` remain active!
        // ---------------------------------------------------------------------
        let live_rb_addrs: Vec<String> = world
            .server("remote-b")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .filter_map(|t| {
                t.agent
                    .filter(|a| a.project_id == pid_local && !a.finished())
                    .and_then(|a| a.terminal)
            })
            .collect();
        if !live_rb_addrs.is_empty() {
            // Live terminal must block `project.archive` cleanly without removing the project
            // or leaving behind a partial archive file.
            let blocked_arch = world
                .client_call(
                    "project.archive",
                    json!({ "project_id": pid_b }),
                    Some(&pid_b),
                )
                .expect_err("Expected live terminal on remote-b to block project.archive");
            ensure!(
                blocked_arch.to_string().contains("live terminal"),
                "Unexpected blocker error on remote-b project.archive: {blocked_arch:#}"
            );
            let herdr_b = world.herdr("remote-b").unwrap();
            for addr in &live_rb_addrs {
                herdr_b.finish_terminal(addr, 0, "exited");
            }
            world.wait_until(10_000, |w| {
                let s = w.server("remote-b").unwrap().snapshot()?;
                Ok(s.terminals
                    .iter()
                    .all(|t| t.agent.as_ref().is_none_or(|a| a.finished())))
            })?;
        }

        let arch_b_res = world.client_call(
            "project.archive",
            json!({ "project_id": pid_b }),
            Some(&pid_b),
        )?;
        ensure!(
            arch_b_res["archived"].as_bool() == Some(true)
                && unqualified_entity_id(arch_b_res["project_id"].as_str().unwrap_or(""))
                    == pid_local
                && arch_b_res["files_count"].as_u64().unwrap_or(0) >= 3
                && arch_b_res["tombstones"].as_u64() == Some(1),
            "Unexpected project.archive(remote-b) receipt: {arch_b_res}"
        );

        let rb_archive_file = world.servers["remote-b"]
            .home
            .join("project-archives")
            .join(format!("{pid_local}.canban.tar.zst"));
        ensure!(
            rb_archive_file.is_file(),
            "Expected cold archive file on remote-b at {}",
            rb_archive_file.display()
        );
        ensure!(
            !world.servers["remote-b"]
                .home
                .join("projects")
                .join(&pid_local)
                .exists(),
            "Archived project metadata dir must be cleaned up on remote-b"
        );

        // Wait until BOTH clients observe `remote-b` removed from active projects while
        // `local-server` and `remote-a` remain active and completely unaffected!
        world.wait_until(15_000, |w| {
            let s1 = w.client_merged_snapshot_on("ui-client")?;
            let s2 = w.client_merged_snapshot_on("ui-client-2")?;
            Ok(s1.projects.len() == 2 && s2.projects.len() == 1)
        })?;
        ensure!(
            world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .projects
                .is_empty()
                && world
                    .server("remote-b")
                    .unwrap()
                    .snapshot()?
                    .cards
                    .is_empty(),
            "remote-b must have 0 active projects/cards while archived"
        );
        for host in ["local-server", "remote-a"] {
            ensure!(
                world
                    .server(host)
                    .unwrap()
                    .snapshot()?
                    .projects
                    .iter()
                    .any(|p| p.id == pid_local),
                "[{host}] sibling project unexpectedly affected by remote-b project.archive"
            );
        }

        // `project.archived.list` on `remote-b` shows the archived copy; `local` and `remote-a`
        // have no archived projects.
        let archived_list_b = world.client_call(
            "project.archived.list",
            json!({ "host_id": "remote-b" }),
            None,
        )?;
        let archived_projs_b = archived_list_b["projects"]
            .as_array()
            .context("project.archived.list(remote-b) missing projects array")?;
        ensure!(
            archived_projs_b.len() == 1
                && unqualified_entity_id(archived_projs_b[0]["project_id"].as_str().unwrap_or(""))
                    == pid_local
                && archived_projs_b[0]["name"].as_str() == Some(&format!("Atlas Remote B {seed}"))
                && archived_projs_b[0]["files_count"].as_u64().unwrap_or(0) >= 3,
            "Unexpected project.archived.list(remote-b) result: {archived_list_b}"
        );
        for other_host in ["local", "remote-a"] {
            let other_list = world.client_call(
                "project.archived.list",
                json!({ "host_id": other_host }),
                None,
            )?;
            ensure!(
                other_list["projects"].as_array().is_some_and(Vec::is_empty),
                "[{other_host}] expected 0 archived projects, got {other_list}"
            );
        }

        // Restore `remote-b` via real `project.restore` and confirm full state convergence
        // across BOTH clients (`ui-client` + `ui-client-2`)!
        let restored_b = world.client_call(
            "project.restore",
            json!({ "host_id": "remote-b", "project_id": pid_local }),
            None,
        )?;
        ensure!(
            restored_b["restored"].as_bool() == Some(true)
                && unqualified_entity_id(restored_b["project_id"].as_str().unwrap_or(""))
                    == pid_local
                && restored_b["imported_tombstones"].as_u64() == Some(1)
                && restored_b["github_paused"].as_bool() == Some(true),
            "Unexpected project.restore(remote-b) receipt: {restored_b}"
        );
        ensure!(
            !rb_archive_file.exists(),
            "Archive file should be deleted after successful project.restore on remote-b"
        );
        let archived_list_b_after = world.client_call(
            "project.archived.list",
            json!({ "host_id": "remote-b" }),
            None,
        )?;
        ensure!(
            archived_list_b_after["projects"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "project.archived.list(remote-b) must be empty after restore: {archived_list_b_after}"
        );

        world.wait_until(15_000, |w| {
            let s1 = w.client_merged_snapshot_on("ui-client")?;
            let s2 = w.client_merged_snapshot_on("ui-client-2")?;
            Ok(s1.projects.len() == 3 && s2.projects.len() == 2)
        })?;
        // Verify cards, workflows, env, managed files, notes, and GitHub tombstone returned intact!
        let restored_rb_snap = world.server("remote-b").unwrap().snapshot()?;
        ensure!(
            restored_rb_snap
                .cards
                .iter()
                .any(|c| c.id == cid1_local && c.title == "Remote-B Copy Title")
                && restored_rb_snap
                    .cards
                    .iter()
                    .any(|c| c.id == cid3_local && c.archived_at.is_some())
                && restored_rb_snap
                    .cards
                    .iter()
                    .any(|c| c.title == "New On Remote-B Only")
                && restored_rb_snap
                    .cards
                    .iter()
                    .any(|c| c.title == "Scheduled Paused Card #1")
                && restored_rb_snap
                    .workflow_sources
                    .iter()
                    .any(|w| w.project_id == pid_local && w.filename == "bundle-flow.yaml")
                && restored_rb_snap
                    .projects
                    .iter()
                    .find(|p| p.id == pid_local)
                    .and_then(|p| p.github.as_ref())
                    .is_some_and(|g| g.paused),
            "Restored remote-b snapshot missing expected cards, workflow, or paused GitHub state"
        );
        let restored_rb_db_cards = read_all_db_cards(world, "remote-b")?;
        ensure!(
            restored_rb_db_cards.iter().any(|c| {
                c.project_id == pid_local
                    && c.removed_at.is_some()
                    && c.github_issue().is_some_and(|r| r.number == 12)
            }),
            "GitHub tombstone #12 was lost across remote-b project.archive -> project.restore!"
        );
        let restored_env_b = world.server_call(
            "remote-b",
            "env.preview",
            json!({ "project_id": pid_local }),
        )?;
        let restored_tier_b = restored_env_b["rows"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["key"].as_str() == Some("PROJ_TIER")))
            .and_then(|r| r["display_value"].as_str());
        ensure!(
            restored_tier_b == Some("remote-b-isolated"),
            "Restored remote-b lost project-scoped env PROJ_TIER: {restored_env_b}"
        );
        ensure!(
            std::fs::read(world.servers["remote-b"].home.join(&art1_path))? == spec_bytes
                && std::fs::read(world.servers["remote-b"].home.join(&rep2_path))?
                    == rep2_body.as_bytes(),
            "Managed files on remote-b did not survive project.archive -> project.restore!"
        );
        let restored_att_b = world.client_call(
            "resource.read",
            json!({ "card_id": cid1_b, "path": art1_path }),
            Some(&pid_b),
        )?;
        ensure!(
            restored_att_b["body"].as_str() == Some(spec_text.as_str()),
            "resource.read on restored remote-b card failed: {restored_att_b}"
        );
        let restored_notes_b =
            world.client_call("note.list", json!({ "project_id": pid_b }), Some(&pid_b))?;
        ensure!(
            restored_notes_b
                .as_array()
                .is_some_and(|arr| arr.iter().any(|n| {
                    n["title"].as_str() == Some("Bundle Design Note")
                        && n["body"].as_str() == Some(note_body.as_str())
                })),
            "Restored remote-b missing Bundle Design Note: {restored_notes_b}"
        );
        world.hit("bundle.project_archive_restore");
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // (e) Fault injection: partition + offline outbox + server & client bounces
        // ---------------------------------------------------------------------
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(120))?;

        let off_card = world.client_call(
            "card.create",
            json!({
                "project_id": pid_a,
                "title": "Offline Imported Copy Card",
                "body": "queued-while-partitioned",
            }),
            Some(&pid_a),
        )?;
        let off_cid = off_card["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid1_a, "body": "offline-comment-imported-a" }),
            Some(&pid_a),
        )?;
        ensure!(
            !world.client_sync_items().is_empty(),
            "Expected queued offline outbox mutations on partitioned remote-a imported copy"
        );

        // Simultaneously mutate remote-b from `ui-client-2` while remote-a is partitioned!
        let c2_live_card = world.client_call_on(
            "ui-client-2",
            "card.create",
            json!({
                "project_id": pid_b_c2,
                "title": "Client-2 Concurrent On Remote-B",
                "body": "while-a-partitioned",
            }),
            Some(&pid_b_c2),
        )?;
        let c2_live_id = c2_live_card["id"].as_str().unwrap().to_owned();
        oracle.record_committed_card(
            "remote-b",
            &c2_live_id,
            &pid_b,
            c2_live_card["revision"].as_u64().unwrap(),
        );

        // Abruptly crash & bounce `remote-a` AND crash & bounce primary `ui-client`!
        world.crash_server("remote-a")?;
        world.crash_client()?;
        world.advance(Duration::from_millis(200))?;
        world.bounce_server("remote-a")?;
        world.bounce_client()?;

        // Persisted outbox items for the imported remote-a project must survive client bounce!
        ensure!(
            !world.client_sync_items().is_empty(),
            "Persisted outbox items for imported project vanished across client crash/bounce!"
        );

        // Heal partition & wait for full convergence across ALL 3 servers and BOTH clients!
        world.heal_and_quiesce(30_000)?;
        oracle.record_committed_comment("remote-a", &cid1_a, "offline-comment-imported-a");

        // Verify offline card landed ONLY on `remote-a`, never on `local-server` or `remote-b`!
        let snap_l = world.server("local-server").unwrap().snapshot()?;
        let snap_a = world.server("remote-a").unwrap().snapshot()?;
        let snap_b = world.server("remote-b").unwrap().snapshot()?;
        let raw_off_cid = unqualified_entity_id(&off_cid).to_owned();
        ensure!(
            snap_a
                .cards
                .iter()
                .any(|c| c.id == raw_off_cid && c.title == "Offline Imported Copy Card"),
            "Offline card failed to drain onto remote-a"
        );
        ensure!(
            !snap_l
                .cards
                .iter()
                .any(|c| c.title == "Offline Imported Copy Card")
                && !snap_b
                    .cards
                    .iter()
                    .any(|c| c.title == "Offline Imported Copy Card"),
            "Offline remote-a card leaked onto sibling hosts!"
        );
        world.hit("bundle.fault_recovery");

        // Strict quiescent invariant verification across ALL clients and ALL 3 servers!
        oracle.check_quiescent_invariants(world)?;

        for host in ["local-server", "remote-a", "remote-b"] {
            ensure!(
                world.servers[host].exec.blocked_commands().is_empty(),
                "[{host}] unexpected blocked OS commands: {:?}",
                world.servers[host].exec.blocked_commands()
            );
        }

        Ok(finalize_digest(world))
    })
}
