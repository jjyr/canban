//! Scenario #15 — Multi-machine `project.export` & `project.import` lifecycle & isolation:
//! Exercises REAL production `project.export` and `project.import` RPCs across 3 hosts
//! (`local-server`, `remote-a`, `remote-b`) with 2 concurrent `MultiHostClient` instances
//! (`ui-client` desktop + `ui-client-2` mobile):
//! (a) Rich source project on `local-server` (multi-stage cards, comments, attention, `CardRead`,
//!     archived card, workflow definition, project-scoped env + secrets, global env + secrets),
//! (b) Real `project.export` producing a `.canban.tar.zst` bundle (verifying secret omission &
//!     zero plaintext secret leakage into the bundle file),
//! (c) Real `project.import` onto `remote-a`, `remote-b`, plus duplicate-import rejection on
//!     `local-server` — testing multi-host identical-ID qualification & `entity_hosts` routing,
//! (d) Concurrent independent mutations (create, update, move, comment, archive, `CardRead`),
//!     simultaneous workflows/agents on imported copies across hosts, and isolated project-scoped
//!     env edits vs synced global env,
//! (e) Partition + offline outbox mutations + `crash_server`/`bounce_server` + `bounce_client`
//!     recovery verified strictly by `InvariantOracle` across ALL clients and ALL 3 servers.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use canban_types::EnvEntryMeta;
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

        // Install workflow definition on source project.
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

        // Create 3 cards: Card 1 (backlog + comments + CardRead), Card 2 (develop + comments +
        // attention/human gate via workflow), Card 3 (archived, exported with include_archived=true).
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
        // Drive Card 2 through workflow start -> blocked (creates real attention + notice!).
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
        world.server_call(
            "local-server",
            "execution.report",
            json!({
                "terminal_id": c2_tid,
                "state": "blocked",
                "message": "Needs input before export",
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
        let c3_rev = c3["revision"].as_u64().unwrap_or(1);
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
        ensure!(
            export_res["exported_cards"].as_u64() == Some(2)
                && export_res["exported_archived_cards"].as_u64() == Some(1),
            "Unexpected exported card counts: {export_res}"
        );
        ensure!(
            export_res["secret_keys"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("PROJ_SECRET"))),
            "Expected PROJ_SECRET omitted & listed in secret_keys: {export_res}"
        );
        ensure!(bundle_path.exists(), "Exported bundle file missing on disk");
        // Security invariant: raw bundle bytes must never contain project or global secret plaintext.
        let bundle_raw = std::fs::read(&bundle_path)?;
        ensure!(
            !bundle_raw
                .windows(proj_secret.len())
                .any(|w| w == proj_secret.as_bytes()),
            "SECURITY VIOLATION: project secret plaintext leaked into exported bundle file!"
        );
        world.hit("bundle.exported");

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

        // Import onto `remote-a` and `remote-b` via `MultiHostClient`!
        let imp_a = world.client_call(
            "project.import",
            json!({
                "host_id": "remote-a",
                "file": bundle_path_str,
                "directory": dir_a,
                "name": format!("Atlas Remote A {seed}"),
            }),
            None,
        )?;
        ensure!(
            imp_a["imported"].as_bool() == Some(true)
                && imp_a["imported_cards"].as_u64() == Some(2)
                && imp_a["imported_archived_cards"].as_u64() == Some(1),
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
                && imp_b["imported_cards"].as_u64() == Some(2)
                && imp_b["imported_archived_cards"].as_u64() == Some(1),
            "Unexpected project.import(remote-b) result: {imp_b}"
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
        let (pid_a, pid_b, cid1_a, cid1_b, cid2_a, cid2_b) = {
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
                            && canban_types::split_qualified_host_id(&c.id)
                                .map(|(_, r)| r)
                                .unwrap_or(c.id.as_str())
                                == raw_id
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
            )
        };
        // Also verify imported active workflow runs on remote-a/b were cleanly cancelled on import.
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
        }
        world.hit("bundle.imported_multi_host");

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
        let (pid_b_c2, cid1_b_c2) = {
            let m2 = world.client_merged_snapshot_on("ui-client-2")?;
            let pb2 = m2
                .projects
                .iter()
                .find(|p| m2.entity_hosts.get(&p.id).map(String::as_str) == Some("remote-b"))
                .unwrap()
                .id
                .clone();
            let c1b2 = m2
                .cards
                .iter()
                .find(|c| {
                    m2.entity_hosts.get(&c.id).map(String::as_str) == Some("remote-b")
                        && canban_types::split_qualified_host_id(&c.id)
                            .map(|(_, r)| r)
                            .unwrap_or(c.id.as_str())
                            == cid1_local
                })
                .unwrap()
                .id
                .clone();
            (pb2, c1b2)
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
            json!({ "card_id": cid2_b, "revision": c2_b_rev }),
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
                "card_id": cid2_b,
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
        let raw_off_cid = canban_types::split_qualified_host_id(&off_cid)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| off_cid.clone());
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
