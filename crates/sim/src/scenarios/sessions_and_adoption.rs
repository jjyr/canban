//! Scenario #16 — Interactive Sessions, `@mention` Question Delivery & Crash Recovery,
//! `card.adopt`, `execution.retry`, SQLite Fault Injection (`SQLITE_BUSY` / `SQLITE_IOERR`),
//! Corrupt `.outbox.json` / `hosts.json` Client Recovery, and Full Durability (`FsDurability::Full`
//! + `PRAGMA synchronous = FULL`):
//!
//! - **Full Durability Mode**: runs with `FsDurability::Full` and `sqlite_synchronous_off = false`
//!   (`PRAGMA synchronous = FULL`) verified across all hosts (`storage.full_sync_verified`).
//! - **Part A — Interactive Sessions (`agent.{open,bind,reply,interrupt,end}`)**: opens a real
//!   interactive session on `remote-a`, binds a provider session ID, interrupts it (verifying
//!   `SimHerdr::interrupted_terminals()` + idempotent `request_key`), replies to questions, and
//!   ends the session cleanly with zero leaked live terminals (`sessions.agent_lifecycle`).
//! - **Part B — `@mention` `Question` Delivery + Abrupt Crash Mid-`delivering` Recovery**:
//!   posts a real `@<profile>` mention comment delivered to the active terminal and answered via
//!   `agent.reply` (`sessions.mention_question_delivered`), then arms a deterministic post-send
//!   storage fault right after `Question.state = "delivering"` commits, abruptly crashes + bounces
//!   `remote-a`, and verifies `run_startup_recovery()` cleanly transitions the `"delivering"`
//!   question to `"delivery_unknown"` and `oracle.check_post_bounce_recovery` passes
//!   non-vacuously (`sessions.mention_crash_recovery`).
//! - **Part C — `card.adopt` & `execution.retry`**: opens a cardless project agent terminal,
//!   adopts it onto a Card via real `card.adopt` (`adoption.rs`) with idempotent replay
//!   (`sessions.card_adopted`), terminates the PTY with non-zero exit code to transition the
//!   execution to `"failed"`, retries it cleanly via real `execution.retry`, verifies a fresh PTY
//!   launches with incremented attempt number & preserved session binding, and completes the
//!   workflow cleanly (`sessions.execution_retried`).
//! - **Part D — Storage Fault Injection & Corrupt Client Store Reload**:
//!   injects `StorageFaultKind::Busy` before a live `card.create` RPC -> clean error + zero DB
//!   corruption + retry succeeds (`storage.fault_busy_rollback`);
//!   injects `StorageFaultKind::IoError` before a live `comment.add` RPC with explicit
//!   `request_key` -> clean rollback of both event and `operations` receipt + idempotent retry
//!   succeeds without duplicate side effects (`storage.fault_ioerror_rollback`);
//!   queues an offline mutation on secondary client `"ui-client-2"`, crashes it, truncates/corrupts
//!   its `.outbox.json`, snapshot cache, and `hosts.json`, bounces `"ui-client-2"`, and verifies
//!   graceful recovery and full convergence without panic or stuck state
//!   (`storage.corrupt_outbox_recovery`).
use crate::{
    exec::SimGitRepo,
    herdr::RunFault,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{FsDurability, SimWorldConfig, StorageFaultKind, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{
    Conversation, ExecutionTrigger, ManagedWorktree, TerminalEntry, qualify_host_id,
    unqualified_entity_id,
};
use rand::Rng;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};

const ADOPT_FLOW_YAML: &str = r#"schema: 12
id: sessions-adopt-flow
name: Sessions Adoption Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  develop:
    event: card.column_entered
    start_at: work
    match:
      column: develop
steps:
  - id: work
    uses: agent
    with:
      profile: codex
      instruction: Complete adopted interactive session
    next: finish
  - id: finish
    uses: complete
"#;

fn count_operation_rows(db_path: &std::path::Path, request_key: &str) -> Result<i64> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {} read-only", db_path.display()))?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM operations WHERE id=?1",
        [request_key],
        |r| r.get(0),
    )?;
    Ok(count)
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "sessions_and_adoption");
    cfg.remote_hosts = vec!["remote-a".into(), "remote-b".into()];
    cfg.extra_clients = vec!["ui-client-2".into()];
    cfg.fs_durability = FsDurability::Full;
    cfg.sqlite_synchronous_off = false;
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 16;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // ---------------------------------------------------------------------
        // Verify Full Durability mode (`FsDurability::Full` + `synchronous = FULL`)
        // and per-host `StorageFaultInjector` wiring on all 3 hosts.
        // ---------------------------------------------------------------------
        ensure!(
            world.config.fs_durability == FsDurability::Full
                && !world.config.sqlite_synchronous_off,
            "sessions_and_adoption must run with FsDurability::Full + sqlite_synchronous_off=false"
        );
        for host in ["local-server", "remote-a", "remote-b"] {
            ensure!(
                world.storage_faults(host).is_some(),
                "Expected per-host StorageFaultInjector wired for '{host}'"
            );
        }
        InvariantOracle::check_sqlite_integrity(world)?;
        world.hit("storage.full_sync_verified");

        // Derive seed-sensitive profile & payload choices so distinct seeds exercise
        // distinct valid paths and produce distinct canonical digests while staying
        // 100% deterministic on same-seed replay.
        let profiles = ["codex", "developer"];
        let primary_idx = (world.rng.random::<u64>() as usize) % profiles.len();
        let primary_profile = profiles[primary_idx];
        let adopt_profile = profiles[(primary_idx + 1) % profiles.len()];

        // Create project on `remote-a` and install adoption workflow.
        let dir_a = world.project_dir(&format!("sessions-repo-{seed}"))?;
        let proj = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Sessions Project {seed}"),
                "directory": dir_a,
            }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();
        let raw_pid = unqualified_entity_id(&pid).to_owned();

        let wf_sources =
            world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid,
                "revision": wf_sources["revision"],
                "sources": [{ "filename": "development.yaml", "source": ADOPT_FLOW_YAML }],
            }),
            Some(&pid),
        )?;

        // ---------------------------------------------------------------------
        // Part A — Interactive Sessions (`agent.{open,bind,interrupt}` + `reply` + `end`)
        // ---------------------------------------------------------------------
        let card_a = world.client_call(
            "card.create",
            json!({
                "project_id": pid,
                "title": format!("Interactive Session Card {seed}"),
                "body": format!("Primary profile={primary_profile} seed={seed}"),
            }),
            Some(&pid),
        )?;
        let cid_a = card_a["id"].as_str().unwrap().to_owned();
        let raw_cid_a = unqualified_entity_id(&cid_a).to_owned();
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_a,
            &raw_pid,
            card_a["revision"].as_u64().unwrap_or(1),
        );

        // 1. `agent.open` -> queues interactive execution and launches PTY in SimHerdr.
        let opened = world.client_call(
            "agent.open",
            json!({
                "card_id": cid_a,
                "profile_id": primary_profile,
                "request_key": format!("open-a-{seed}"),
            }),
            Some(&pid),
        )?;
        ensure!(
            opened["reused"].as_bool() == Some(false),
            "First agent.open should enqueue a fresh terminal: {opened}"
        );
        let raw_tid_a = unqualified_entity_id(
            opened["terminal_id"]
                .as_str()
                .context("agent.open missing terminal_id")?,
        )
        .to_owned();
        let q_tid_a = qualify_host_id("remote-a", &raw_tid_a);

        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == raw_tid_a
                    && t.agent.as_ref().is_some_and(|a| {
                        a.state == "running"
                            && a.terminal.is_some()
                            && a.connection_state == "connected"
                    })
            }))
        })?;

        // Second `agent.open` while live must reuse the active terminal without launching a second PTY.
        let reused = world.client_call(
            "agent.open",
            json!({
                "card_id": cid_a,
                "profile_id": primary_profile,
                "request_key": format!("open-a-reuse-{seed}"),
            }),
            Some(&pid),
        )?;
        ensure!(
            reused["reused"].as_bool() == Some(true)
                && unqualified_entity_id(reused["terminal_id"].as_str().unwrap_or("")) == raw_tid_a,
            "Second agent.open must reuse live interactive terminal: {reused}"
        );

        let herdr_a = world.herdr("remote-a").unwrap();
        let herdr_addr_a = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == raw_tid_a)
            .and_then(|t| t.agent)
            .and_then(|a| a.terminal)
            .context("Interactive terminal missing Herdr address")?;

        // 2. `agent.bind` -> binds provider session ID to both Conversation and live AgentRun.
        let sess_id_a = format!("sim-provider-sess-{primary_profile}-{seed}");
        let bound = world.client_call(
            "agent.bind",
            json!({
                "terminal_id": q_tid_a,
                "session_id": sess_id_a,
                "request_key": format!("bind-a-{seed}"),
            }),
            Some(&pid),
        )?;
        ensure!(
            bound["bound"].as_bool() == Some(true)
                && bound["session_id"].as_str() == Some(sess_id_a.as_str()),
            "Unexpected agent.bind response: {bound}"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let conv = snap
                .conversations
                .iter()
                .find(|c| c.id == raw_tid_a)
                .context("Missing Conversation after agent.bind")?;
            ensure!(
                conv.provider_session_id.as_deref() == Some(sess_id_a.as_str()),
                "Conversation provider_session_id mismatch: {conv:?}"
            );
            let term = snap
                .terminals
                .iter()
                .find(|t| t.id == raw_tid_a)
                .and_then(|t| t.agent.as_ref())
                .context("Missing AgentRun after agent.bind")?;
            ensure!(
                term.provider_session_id.as_deref() == Some(sess_id_a.as_str()),
                "AgentRun provider_session_id mismatch: {term:?}"
            );
        }
        // Attempting to rebind a different session ID to the same conversation must fail cleanly.
        let bad_bind = world.client_call(
            "agent.bind",
            json!({
                "terminal_id": q_tid_a,
                "session_id": format!("conflicting-sess-{seed}"),
            }),
            Some(&pid),
        );
        ensure!(
            bad_bind.is_err(),
            "Rebinding a different provider session ID should have failed"
        );
        // Cross-card duplicate provider session ID binding must also be rejected!
        let dup_card = world.client_call(
            "card.create",
            json!({
                "project_id": pid,
                "title": format!("Duplicate Session Reject Card {seed}"),
            }),
            Some(&pid),
        )?;
        let dup_cid = dup_card["id"].as_str().unwrap().to_owned();
        let dup_opened = world.client_call(
            "agent.open",
            json!({
                "card_id": dup_cid,
                "profile_id": primary_profile,
                "request_key": format!("open-dup-sess-{seed}"),
            }),
            Some(&pid),
        )?;
        let dup_tid = qualify_host_id(
            "remote-a",
            unqualified_entity_id(dup_opened["terminal_id"].as_str().unwrap()),
        );
        let cross_card_bind = world.client_call(
            "agent.bind",
            json!({
                "terminal_id": dup_tid,
                "session_id": sess_id_a,
            }),
            Some(&pid),
        );
        ensure!(
            cross_card_bind.is_err(),
            "Binding the same provider session ID onto a second card must be rejected"
        );
        let _ = world.client_call("agent.end", json!({ "terminal_id": dup_tid }), Some(&pid));

        // 3. `agent.interrupt` -> interrupts working PTY, transitions working -> idle,
        //    and idempotent replay with same `request_key` does NOT interrupt twice.
        let intr_key = format!("intr-a-{seed}");
        let intr_res = world.client_call(
            "agent.interrupt",
            json!({
                "terminal_id": q_tid_a,
                "request_key": intr_key,
            }),
            Some(&pid),
        )?;
        ensure!(
            intr_res["state"].as_str() == Some("sent"),
            "Expected agent.interrupt state='sent', got {intr_res}"
        );
        let interrupted_after_first = herdr_a.interrupted_terminals();
        ensure!(
            interrupted_after_first.contains(&herdr_addr_a),
            "SimHerdr::interrupted_terminals() missing {herdr_addr_a}: {interrupted_after_first:?}"
        );
        // Idempotent replay with exact same request_key must return cached receipt & not re-interrupt.
        let intr_replay = world.client_call(
            "agent.interrupt",
            json!({
                "terminal_id": q_tid_a,
                "request_key": intr_key,
            }),
            Some(&pid),
        )?;
        ensure!(
            intr_replay["state"].as_str() == Some("sent")
                && herdr_a.interrupted_terminals().len() == interrupted_after_first.len(),
            "Idempotent agent.interrupt replayed physical interrupt! replay={intr_replay}"
        );
        world.wait_until(5_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == raw_tid_a
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.runtime_status_at(now) == "idle")
            }))
        })?;
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // Part B — `@mention` `Question` Delivery + Abrupt Crash Mid-`delivering` Recovery
        // ---------------------------------------------------------------------
        // B1: Real `@<profile>` mention comment -> Question delivered to live agent PTY ->
        //     `agent.reply` marks Question `"replied"` and persists reply artifact & timeline event.
        let mention_body_1 = format!("@{primary_profile} please review architecture seed={seed}");
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_a,
                "body": mention_body_1,
                "request_key": format!("mention-1-{seed}"),
            }),
            Some(&pid),
        )?;
        oracle.record_committed_comment("remote-a", &raw_cid_a, mention_body_1.clone());

        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.questions.iter().any(|q| {
                q.card_id == raw_cid_a
                    && q.profile_id == primary_profile
                    && q.body == mention_body_1
                    && q.state == "delivered"
            }))
        })?;
        let q1_id = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .questions
            .into_iter()
            .find(|q| q.card_id == raw_cid_a && q.body == mention_body_1)
            .context("Delivered mention Question missing")?
            .id;
        ensure!(
            herdr_a
                .sent_inputs()
                .iter()
                .any(|s| s.terminal == herdr_addr_a && s.text.contains(&q1_id)),
            "Expected SimHerdr terminal.send to deliver Question {q1_id} to {herdr_addr_a}"
        );
        world.hit("sessions.mention_question_delivered");

        // Reply to Question 1 via real `agent.reply` (routed through `world.client_call`).
        let reply_body = format!("Reviewed and verified seed={seed}");
        let reply_key = format!("reply-q1-{seed}");
        let reply_res = world.client_call(
            "agent.reply",
            json!({
                "card_id": cid_a,
                "question_id": q1_id,
                "body": reply_body,
                "request_key": reply_key,
            }),
            Some(&pid),
        )?;
        ensure!(
            reply_res["accepted"].as_bool() == Some(true),
            "agent.reply failed: {reply_res}"
        );
        // Idempotent replay with same `request_key` returns accepted without error.
        let reply_replay = world.client_call(
            "agent.reply",
            json!({
                "card_id": cid_a,
                "question_id": q1_id,
                "body": reply_body,
                "request_key": reply_key,
            }),
            Some(&pid),
        )?;
        ensure!(
            reply_replay["accepted"].as_bool() == Some(true),
            "agent.reply idempotent replay failed: {reply_replay}"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let q1 = snap
                .questions
                .iter()
                .find(|q| q.id == q1_id)
                .context("Question 1 vanished after agent.reply")?;
            ensure!(
                q1.state == "replied" && q1.reply_path.is_some(),
                "Expected Question 1 state='replied' with reply_path, got {q1:?}"
            );
            let reply_file = world.servers["remote-a"]
                .home
                .join(q1.reply_path.as_deref().unwrap());
            ensure!(
                std::fs::read_to_string(&reply_file)
                    .with_context(|| format!("read reply file {}", reply_file.display()))?
                    == reply_body,
                "Reply artifact file contents mismatch"
            );
        }

        // B2: Induce abrupt `crash_server` + `bounce_server` while a Question is genuinely
        //     committed in SQLite with `state == "delivering"`, then verify `run_startup_recovery()`
        //     transitions it cleanly out of `"delivering"` -> `"delivery_unknown"`!
        //
        // Hold delivery while we post the comment by marking the terminal `"blocked"`.
        herdr_a.set_status(&herdr_addr_a, "blocked", Some(2));
        world.wait_until(5_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == raw_tid_a
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.runtime_status_at(now) == "blocked")
            }))
        })?;

        let mention_body_2 = format!("@{primary_profile} mid-delivery crash probe seed={seed}");
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_a,
                "body": mention_body_2,
                "request_key": format!("mention-2-{seed}"),
            }),
            Some(&pid),
        )?;
        oracle.record_committed_comment("remote-a", &raw_cid_a, mention_body_2.clone());

        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.questions.iter().any(|q| {
                q.card_id == raw_cid_a && q.body == mention_body_2 && q.state == "pending"
            }))
        })?;
        let q2_id = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .questions
            .into_iter()
            .find(|q| q.card_id == raw_cid_a && q.body == mention_body_2)
            .context("Pending mention Question 2 missing")?
            .id;

        // Arm a one-shot hook inside `SimHerdr::terminal.send`: at the exact instant
        // `deliver_questions` sends `q2` to Herdr, `q2.state = "delivering"` has JUST committed
        // to SQLite and the very next write transaction is `delivering -> delivered`. Injecting
        // `StorageFaultKind::IoError` there rolls back the post-send commit so `q2` remains
        // genuinely persisted as `state == "delivering"` right as we SIGKILL `remote-a`!
        let remote_a_faults = Arc::clone(world.storage_faults("remote-a").unwrap());
        herdr_a.set_on_next_send(move |_addr, _text| {
            remote_a_faults.inject_next(StorageFaultKind::IoError);
        });
        herdr_a.set_status(&herdr_addr_a, "idle", Some(3));

        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .questions
                .iter()
                .any(|q| q.id == q2_id && q.state == "delivering"))
        })?;

        // Abruptly crash (`SIGKILL`) and bounce `remote-a` while `q2.state == "delivering"`.
        world.crash_server("remote-a")?;
        world.bounce_server("remote-a")?;

        // Non-vacuous post-bounce recovery verification (`q.state != "delivering"`).
        InvariantOracle::check_post_bounce_recovery(world, "remote-a")?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let q2 = snap
                .questions
                .iter()
                .find(|q| q.id == q2_id)
                .context("Question 2 vanished across crash/bounce")?;
            ensure!(
                q2.state == "delivery_unknown"
                    && q2
                        .error
                        .as_deref()
                        .is_some_and(|e| e.contains("restarted during delivery")),
                "Expected run_startup_recovery to transition delivering -> delivery_unknown, got {q2:?}"
            );
            let card = snap
                .cards
                .iter()
                .find(|c| c.id == raw_cid_a)
                .context("Card A missing after bounce")?;
            ensure!(
                card.attention.is_some(),
                "Expected Card A attention set after mid-delivery crash recovery"
            );
        }
        world.hit("sessions.mention_crash_recovery");

        // Wait for bounced `remote-a` to reattach to live Herdr PTY & clients to reconnect,
        // then end Card A's interactive session via real `agent.end`!
        world.heal_and_quiesce(20_000)?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == raw_tid_a
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.connection_state == "connected" && !a.finished())
            }))
        })?;

        let end_key = format!("end-a-{seed}");
        let end_res = world.client_call(
            "agent.end",
            json!({
                "terminal_id": q_tid_a,
                "request_key": end_key,
            }),
            Some(&pid),
        )?;
        ensure!(
            end_res["state"].as_str() == Some("sent"),
            "Expected agent.end state='sent', got {end_res}"
        );
        ensure!(
            herdr_a.killed_terminals().contains(&herdr_addr_a),
            "SimHerdr::killed_terminals() missing {herdr_addr_a}"
        );
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .terminals
                .iter()
                .any(|t| t.id == raw_tid_a && t.agent.as_ref().is_some_and(|a| a.finished())))
        })?;
        ensure!(
            herdr_a
                .terminal_record(&herdr_addr_a)
                .and_then(|r| r["state"].as_str().map(str::to_owned))
                .as_deref()
                == Some("finished"),
            "Interactive session PTY {herdr_addr_a} leaked after agent.end"
        );
        world.hit("sessions.agent_lifecycle");
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // Part C — `card.adopt` & `execution.retry`
        // ---------------------------------------------------------------------
        let card_c = world.client_call(
            "card.create",
            json!({
                "project_id": pid,
                "title": format!("Adopt & Retry Card {seed}"),
                "body": format!("Adopt profile={adopt_profile} seed={seed}"),
            }),
            Some(&pid),
        )?;
        let cid_c = card_c["id"].as_str().unwrap().to_owned();
        let raw_cid_c = unqualified_entity_id(&cid_c).to_owned();
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_c,
            &raw_pid,
            card_c["revision"].as_u64().unwrap_or(1),
        );

        // Pre-bind provider session ID on Card C so `card.adopt` inherits it onto the adopted run!
        let adopt_sess_id = format!("adopt-provider-sess-{adopt_profile}-{seed}");
        let pre_bind = world.client_call(
            "agent.bind",
            json!({
                "card_id": cid_c,
                "profile_id": adopt_profile,
                "session_id": adopt_sess_id,
                "request_key": format!("bind-c-{seed}"),
            }),
            Some(&pid),
        )?;
        ensure!(
            pre_bind["bound"].as_bool() == Some(true),
            "Pre-adopt agent.bind failed: {pre_bind}"
        );

        // Launch an independent cardless project agent terminal on `remote-a`.
        let manual_term = world.client_call(
            "terminal.create",
            json!({
                "project_id": pid,
                "agent": true,
                "profile_id": adopt_profile,
                "cwd": dir_a,
                "name": format!("Cardless Adopt PTY {seed}"),
            }),
            Some(&pid),
        )?;
        let raw_manual_tid = unqualified_entity_id(
            manual_term["id"]
                .as_str()
                .context("terminal.create missing id")?,
        )
        .to_owned();
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == raw_manual_tid
                    && t.runtime
                        .as_ref()
                        .is_some_and(|r| r.state == "running" && r.terminal.is_some())
            }))
        })?;
        let (manual_herdr_addr, pty_count_before_adopt) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let addr = snap
                .terminals
                .iter()
                .find(|t| t.id == raw_manual_tid)
                .and_then(|t| t.runtime.as_ref())
                .and_then(|r| r.terminal.clone())
                .context("Manual terminal missing Herdr address")?;
            (addr, herdr_a.terminal_addresses().len())
        };

        // Call REAL `card.adopt` to take over the existing live PTY onto Card C's `develop` stage!
        let adopt_params = json!({
            "view": "full",
            "card_id": cid_c,
            "stage": "develop",
            "profile_id": adopt_profile,
            "cwd": dir_a,
            "runtime_terminal": manual_herdr_addr,
            "runtime_session": "default",
            "request_key": format!("adopt-c-{seed}"),
        });
        let adopted = world.client_call("card.adopt", adopt_params.clone(), Some(&pid))?;
        ensure!(
            adopted["adopted"].as_bool() == Some(true)
                && adopted["card"]["stage"].as_str() == Some("develop"),
            "card.adopt failed: {adopted}"
        );
        // Idempotent replay with identical request_key returns identical receipt and spawns no extra PTY.
        let adopted_replay = world.client_call("card.adopt", adopt_params, Some(&pid))?;
        ensure!(
            adopted_replay == adopted,
            "card.adopt replay diverged: first={adopted} replay={adopted_replay}"
        );
        ensure!(
            herdr_a.terminal_addresses().len() == pty_count_before_adopt,
            "card.adopt must reuse the existing PTY without launching a duplicate terminal"
        );

        let expected_adopted_tid = Conversation::key(&raw_cid_c, adopt_profile);
        let q_adopted_tid = qualify_host_id("remote-a", &expected_adopted_tid);
        let wf_run_id = adopted["workflow_run"]["id"]
            .as_str()
            .map(unqualified_entity_id)
            .context("card.adopt missing workflow_run.id")?
            .to_owned();
        oracle.record_committed_workflow_run(
            "remote-a",
            &wf_run_id,
            &raw_pid,
            "sessions-adopt-flow",
            1,
        );
        let initial_attempt_num = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let adopted_entry = snap
                .terminals
                .iter()
                .find(|t| t.id == expected_adopted_tid)
                .context("Adopted conversation terminal missing")?;
            let run = adopted_entry
                .agent
                .as_ref()
                .context("Adopted terminal missing AgentRun")?;
            ensure!(
                run.state == "running"
                    && run.trigger == ExecutionTrigger::Interactive
                    && run.terminal.as_deref() == Some(manual_herdr_addr.as_str())
                    && run.provider_session_id.as_deref() == Some(adopt_sess_id.as_str())
                    && run.workflow_run_id.as_deref() == Some(wf_run_id.as_str()),
                "Adopted AgentRun state mismatch: {run:?}"
            );
            let detached = snap
                .terminals
                .iter()
                .find(|t| t.id == raw_manual_tid)
                .context("Detached manual terminal missing")?;
            let det_rt = detached
                .runtime
                .as_ref()
                .context("Detached terminal missing runtime")?;
            ensure!(
                det_rt.state == "finished" && det_rt.terminal.is_none(),
                "Detached manual terminal should be finished with terminal=None, got {det_rt:?}"
            );
            run.number
        };
        world.hit("sessions.card_adopted");
        oracle.check_step_invariants(world)?;

        // Now fail the adopted execution by terminating its PTY with non-zero exit code,
        // wait until AppServer observes `state == "failed"` + `observed["state"] == "finished"`,
        // and recover via REAL `execution.retry`!
        herdr_a.finish_terminal(&manual_herdr_addr, 1, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == expected_adopted_tid
                    && t.agent.as_ref().is_some_and(|a| {
                        a.state == "failed"
                            && a.observed
                                .as_ref()
                                .is_some_and(|v| v["state"] == "finished")
                    })
            }))
        })?;

        let retry_res = world.client_call(
            "execution.retry",
            json!({
                "terminal_id": q_adopted_tid,
                "request_key": format!("retry-c-{seed}"),
            }),
            Some(&pid),
        )?;
        ensure!(
            unqualified_entity_id(retry_res["id"].as_str().unwrap_or("")) == expected_adopted_tid,
            "Unexpected execution.retry response: {retry_res}"
        );

        // Scheduler launches a fresh Herdr PTY with incremented attempt number & preserved session.
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == expected_adopted_tid
                    && t.agent.as_ref().is_some_and(|a| {
                        a.state == "running"
                            && a.number == initial_attempt_num + 1
                            && a.resume_session
                            && a.provider_session_id.as_deref() == Some(adopt_sess_id.as_str())
                            && a.terminal.is_some()
                            && a.terminal.as_deref() != Some(manual_herdr_addr.as_str())
                            && a.connection_state == "connected"
                    })
            }))
        })?;
        let retried_herdr_addr = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == expected_adopted_tid)
            .and_then(|t| t.agent)
            .and_then(|a| a.terminal)
            .context("Retried AgentRun missing fresh Herdr address")?;

        // Complete the retried execution cleanly and exit its PTY so zero live terminals leak.
        let _ = world.client_call(
            "execution.report",
            json!({
                "terminal_id": q_adopted_tid,
                "state": "completed",
                "message": format!("Retried execution succeeded seed={seed}"),
                "request_key": format!("report-retry-c-{seed}"),
            }),
            Some(&pid),
        )?;
        herdr_a.finish_terminal(&retried_herdr_addr, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let wf_done = snap
                .workflow_runs
                .iter()
                .any(|r| r.id == wf_run_id && r.state == "completed");
            let term_done = snap.terminals.iter().any(|t| {
                t.id == expected_adopted_tid && t.agent.as_ref().is_some_and(|a| a.finished())
            });
            Ok(wf_done && term_done)
        })?;
        // Verify zero live terminals remain leaked in SimHerdr on `remote-a`.
        for addr in herdr_a.terminal_addresses() {
            let state = herdr_a
                .terminal_record(&addr)
                .and_then(|r| r["state"].as_str().map(str::to_owned));
            ensure!(
                state.as_deref() == Some("finished"),
                "Leaked live terminal {addr} on remote-a (state={state:?})"
            );
        }
        world.hit("sessions.execution_retried");
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // Part D — Storage Fault Injection (`SQLITE_BUSY` & `SQLITE_IOERR`) +
        //          Corrupt `.outbox.json` / `hosts.json` Reload
        // ---------------------------------------------------------------------
        // Let background workers settle so injected faults target our live write RPCs cleanly.
        world.heal_and_quiesce(15_000)?;
        let remote_a_db = world.servers["remote-a"].home.join("canban.sqlite");
        let faults = Arc::clone(world.storage_faults("remote-a").unwrap());

        // D1: Inject `StorageFaultKind::Busy` before a live `card.create` RPC ->
        //     clean error + zero partial state + zero DB corruption + retry succeeds!
        let busy_key = format!("busy-create-{seed}");
        let busy_title = format!("Busy Rollback Card {seed}");
        let busy_params = json!({
            "project_id": pid,
            "title": busy_title,
            "body": format!("Recovered after SQLITE_BUSY seed={seed}"),
            "request_key": busy_key,
        });
        let triggers_before_busy = faults.triggered_count();
        let cards_before_busy = world.server("remote-a").unwrap().snapshot()?.cards.len();
        faults.inject_next(StorageFaultKind::Busy);
        let busy_err = world
            .client_call("card.create", busy_params.clone(), Some(&pid))
            .expect_err("Expected SQLITE_BUSY error on live card.create");
        ensure!(
            format!("{busy_err:#}").contains("SQLITE_BUSY")
                || format!("{busy_err:#}")
                    .to_ascii_lowercase()
                    .contains("database is locked"),
            "Expected SQLITE_BUSY error, got: {busy_err:#}"
        );
        ensure!(
            faults.triggered_count() == triggers_before_busy + 1,
            "Expected StorageFaultInjector::triggered_count to increment on SQLITE_BUSY"
        );
        ensure!(
            world.server("remote-a").unwrap().snapshot()?.cards.len() == cards_before_busy
                && count_operation_rows(&remote_a_db, &busy_key)? == 0,
            "SQLITE_BUSY left partial card or operation receipt behind"
        );
        InvariantOracle::check_sqlite_integrity(world)?;

        let created_after_busy = world.client_call("card.create", busy_params, Some(&pid))?;
        let cid_d = created_after_busy["id"].as_str().unwrap().to_owned();
        let raw_cid_d = unqualified_entity_id(&cid_d).to_owned();
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_d,
            &raw_pid,
            created_after_busy["revision"].as_u64().unwrap_or(1),
        );
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .any(|c| c.id == raw_cid_d && c.title == busy_title),
            "Retry after SQLITE_BUSY did not commit Card D"
        );
        world.hit("storage.fault_busy_rollback");

        // D2: Inject `StorageFaultKind::IoError` before a live `comment.add` RPC with explicit
        //     `request_key` -> full transaction rollback (no event & no operations row!) +
        //     retry with the exact same `request_key` succeeds without duplicate side effects!
        let io_key = format!("ioerr-comment-{seed}");
        let io_event_id = format!("ioev-{seed}");
        let io_body = format!("Durable comment after SQLITE_IOERR rollback seed={seed}");
        let io_params = json!({
            "card_id": cid_d,
            "id": io_event_id,
            "body": io_body,
            "request_key": io_key,
        });
        let triggers_before_io = faults.triggered_count();
        faults.inject_next(StorageFaultKind::IoError);
        let io_err = world
            .client_call("comment.add", io_params.clone(), Some(&pid))
            .expect_err("Expected SQLITE_IOERR error on live comment.add");
        ensure!(
            format!("{io_err:#}").contains("SQLITE_IOERR")
                || format!("{io_err:#}")
                    .to_ascii_lowercase()
                    .contains("disk i/o error"),
            "Expected SQLITE_IOERR error, got: {io_err:#}"
        );
        ensure!(
            faults.triggered_count() == triggers_before_io + 1,
            "Expected StorageFaultInjector::triggered_count to increment on SQLITE_IOERR"
        );
        // Verify ZERO half-committed event or operation receipt persisted in SQLite!
        ensure!(
            count_operation_rows(&remote_a_db, &io_key)? == 0
                && !world
                    .server("remote-a")
                    .unwrap()
                    .events()?
                    .iter()
                    .any(|e| e.id == io_event_id
                        || e.data["body"].as_str() == Some(io_body.as_str())),
            "SQLITE_IOERR failed to roll back uncommitted comment or operation receipt!"
        );
        InvariantOracle::check_sqlite_integrity(world)?;

        // Retry with the exact same `request_key` succeeds cleanly; replaying once more is idempotent.
        let io_retry_1 = world.client_call("comment.add", io_params.clone(), Some(&pid))?;
        let io_retry_2 = world.client_call("comment.add", io_params, Some(&pid))?;
        ensure!(
            io_retry_1["id"] == io_retry_2["id"],
            "Idempotent replay after SQLITE_IOERR recovery returned different receipt: {io_retry_1} vs {io_retry_2}"
        );
        let matching_comments = world
            .server("remote-a")
            .unwrap()
            .events()?
            .into_iter()
            .filter(|e| {
                e.card_id == raw_cid_d && e.kind == "comment.created" && e.id == io_event_id
            })
            .count();
        ensure!(
            matching_comments == 1 && count_operation_rows(&remote_a_db, &io_key)? == 1,
            "Expected exactly 1 committed comment & 1 operations receipt after retry+replay, got comments={matching_comments}"
        );
        oracle.record_committed_comment("remote-a", &raw_cid_d, io_body);
        world.hit("storage.fault_ioerror_rollback");
        oracle.check_step_invariants(world)?;

        // D3: Corrupt `.outbox.json`, snapshot cache, and `hosts.json` crash reload on
        //     secondary client `"ui-client-2"` -> clean recovery without panic or stuck client!
        world.heal_and_quiesce(15_000)?;
        let c2_dir = world
            .client_data_dir_named("ui-client-2")
            .context("ui-client-2 data_dir missing")?
            .to_path_buf();
        let c2_outbox_path = canban_client::store::outbox_path(&c2_dir, "remote-a");
        let c2_cache_path = canban_client::store::cache_path(&c2_dir, "remote-a");
        let c2_hosts_path = c2_dir.join("hosts.json");

        // Partition `"ui-client-2"` from `"remote-a"` and queue an offline mutation so a real
        // `.outbox.json` file is written on disk before we abruptly crash `"ui-client-2"`.
        world.partition("ui-client-2", "remote-a");
        world.wait_until(45_000, |w| {
            Ok(w.client_host_summaries_on("ui-client-2")
                .iter()
                .find(|s| s.id == "remote-a")
                .is_some_and(|s| !s.is_connected()))
        })?;
        let c2_pid = world
            .client_merged_snapshot_on("ui-client-2")?
            .projects
            .into_iter()
            .find(|p| unqualified_entity_id(&p.id) == raw_pid)
            .context("ui-client-2 missing project before offline queue")?
            .id;
        let c2_cid_d = world
            .client_merged_snapshot_on("ui-client-2")?
            .cards
            .into_iter()
            .find(|c| unqualified_entity_id(&c.id) == raw_cid_d)
            .context("ui-client-2 missing Card D before offline queue")?
            .id;
        let _ = world.client_call_on(
            "ui-client-2",
            "comment.add",
            json!({
                "card_id": c2_cid_d,
                "body": format!("doomed-before-power-loss-truncation-{seed}"),
                "request_key": format!("trunc-outbox-{seed}"),
            }),
            Some(&c2_pid),
        )?;
        ensure!(
            c2_outbox_path.exists() && c2_hosts_path.exists(),
            "Expected ui-client-2 outbox & hosts.json on disk before crash"
        );

        // Crash #1: truncate `.outbox.json` and snapshot cache while keeping `hosts.json` intact.
        world.crash_client_named("ui-client-2")?;
        std::fs::write(
            &c2_outbox_path,
            br#"{"ops":[{"id":"truncated-mid-write","host_id":"remote-a""#,
        )?;
        std::fs::write(&c2_cache_path, br#"{"saved_at":1,"snapshot":{"cards":["#)?;
        world.repair("ui-client-2", "remote-a");
        world.bounce_client_named("ui-client-2")?;
        world.heal_and_quiesce(20_000)?;
        ensure!(
            world.client_sync_items_on("ui-client-2").is_empty(),
            "Corrupt .outbox.json must recover to empty queue without stuck items"
        );

        // Crash #2: truncate `hosts.json` AND `.outbox.json` together, bounce `"ui-client-2"`,
        // and verify clean reload + fresh live mutation from `"ui-client-2"` succeeds!
        world.crash_client_named("ui-client-2")?;
        std::fs::write(
            &c2_hosts_path,
            br#"{"hosts":[{"id":"remote-a","name":"Truncated"#,
        )?;
        std::fs::write(&c2_outbox_path, b"\x00\xff\xfecorrupt-outbox")?;
        world.bounce_client_named("ui-client-2")?;
        world.heal_and_quiesce(20_000)?;

        let recovered_body = format!("post-corrupt-reload-comment-{seed}");
        let c2_pid_post = world
            .client_merged_snapshot_on("ui-client-2")?
            .projects
            .into_iter()
            .find(|p| unqualified_entity_id(&p.id) == raw_pid)
            .context("ui-client-2 missing project after corrupt hosts.json recovery")?
            .id;
        let c2_cid_post = world
            .client_merged_snapshot_on("ui-client-2")?
            .cards
            .into_iter()
            .find(|c| unqualified_entity_id(&c.id) == raw_cid_d)
            .context("ui-client-2 missing Card D after corrupt hosts.json recovery")?
            .id;
        let _ = world.client_call_on(
            "ui-client-2",
            "comment.add",
            json!({
                "card_id": c2_cid_post,
                "body": recovered_body,
                "request_key": format!("post-corrupt-ok-{seed}"),
            }),
            Some(&c2_pid_post),
        )?;
        oracle.record_committed_comment("remote-a", &raw_cid_d, recovered_body);
        // Verify rewritten hosts.json & snapshot cache on disk parse as valid JSON again.
        let hosts_text = std::fs::read_to_string(&c2_hosts_path)?;
        let _: serde_json::Value =
            serde_json::from_str(&hosts_text).context("Recovered hosts.json is not valid JSON")?;
        world.hit("storage.corrupt_outbox_recovery");

        // ---------------------------------------------------------------------
        // Part E — 2-Phase `card.adopt`, Unusable PTY Rejection (all 7 cases),
        //          Savepoint Owner Preservation, `terminal.rename`/`context`,
        //          CWD Inheritance (Worktree/Subdir/Live Inventory/Core Fence),
        //          Archive Worktree Protection, Manual Labels & Ephemeral Agent
        // ---------------------------------------------------------------------
        let herdr_b = world.herdr("remote-b").unwrap();
        let exec_b = world
            .exec_runner("remote-b")
            .context("remote-b SimExec unavailable")?;

        // E0: First terminal accepts empty inheritance before any project exists on `remote-b`!
        {
            let first_bare = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "inherit_terminal_id": "" }),
            )?;
            ensure!(
                first_bare["runtime"]["state"] == "running"
                    && first_bare["name"] == "Terminal 1"
                    && first_bare["runtime"]["terminal"].is_string()
                    && first_bare["conversation_id"].is_null(),
                "Empty inheritance first terminal mismatch: {first_bare}"
            );
            let bare_tid = first_bare["id"].as_str().unwrap().to_owned();
            let bare_addr = first_bare["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": bare_tid }),
            )?;
            herdr_b.finish_terminal(&bare_addr, 0, "exited");
        }

        // Create two git-backed projects on `remote-b` for adoption, CWD inheritance & archive protection.
        let b_dir1 = world.project_dir(&format!("adopt-wt-p1-{seed}"))?;
        let b_dir1_sub = PathBuf::from(&b_dir1).join("docs");
        std::fs::create_dir_all(&b_dir1_sub)?;
        exec_b.register_git_repo(&b_dir1, SimGitRepo::default());
        let b_dir2 = world.project_dir(&format!("adopt-wt-p2-{seed}"))?;
        exec_b.register_git_repo(&b_dir2, SimGitRepo::default());

        let pb1 = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-b",
                "name": format!("AdoptWT1 {seed}"),
                "directory": b_dir1,
                "target_branch": "main",
                "workspace": { "create_worktree": true, "sync_base": false },
            }),
            None,
        )?;
        let pid_b1 = pb1["id"].as_str().unwrap().to_owned();
        let raw_pid_b1 = unqualified_entity_id(&pid_b1).to_owned();
        let pb2 = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-b",
                "name": format!("AdoptWT2 {seed}"),
                "directory": b_dir2,
                "target_branch": "main",
                "workspace": { "create_worktree": true, "sync_base": false },
            }),
            None,
        )?;
        let pid_b2 = pb2["id"].as_str().unwrap().to_owned();
        let raw_pid_b2 = unqualified_entity_id(&pid_b2).to_owned();
        let b1_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_b1 }),
            Some(&pid_b1),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_b1,
                "revision": b1_sources["revision"],
                "sources": [{ "filename": "adopt-flow.yaml", "source": ADOPT_FLOW_YAML }],
            }),
            Some(&pid_b1),
        )?;

        // E1: 2-phase `card.adopt` (workflow-only adopt -> bind manual PTY -> replay without duplicates).
        {
            let c2p = world.client_call(
                "card.create",
                json!({ "project_id": pid_b1, "title": "Two-Phase Adopt" }),
                Some(&pid_b1),
            )?;
            let raw_c2p = unqualified_entity_id(c2p["id"].as_str().unwrap()).to_owned();
            let manual_term = world.server_call(
                "remote-b",
                "terminal.create",
                json!({
                    "card_id": raw_c2p,
                    "agent": true,
                    "profile_id": "codex",
                    "name": "My session",
                }),
            )?;
            let manual_addr = manual_term["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            let c2p_wt = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_c2p)
                .unwrap()
                .worktree
                .unwrap_or_else(|| b_dir1.clone());
            let terms_before_adopt = herdr_b.terminal_addresses().len();

            // Phase 1: workflow-only adoption (no runtime_terminal).
            let first = world.server_call(
                "remote-b",
                "card.adopt",
                json!({
                    "view": "full",
                    "card_id": raw_c2p,
                    "profile_id": "codex",
                    "request_key": format!("2ph-no-term-{seed}"),
                }),
            )?;
            ensure!(
                first["workflow_run"]["terminal_id"].is_null(),
                "Phase 1 workflow-only adopt must leave terminal_id null: {first}"
            );
            // Phase 2: bind existing manual PTY onto same workflow run.
            let adopt_params = json!({
                "view": "full",
                "card_id": raw_c2p,
                "profile_id": "codex",
                "cwd": c2p_wt,
                "runtime_terminal": manual_addr,
                "runtime_session": "default",
                "request_key": format!("2ph-bind-once-{seed}"),
            });
            let second = world.server_call("remote-b", "card.adopt", adopt_params.clone())?;
            ensure!(
                first["workflow_run"]["id"] == second["workflow_run"]["id"]
                    && second["workflow_run"]["terminal_id"]
                        == Conversation::key(&raw_c2p, "codex"),
                "Phase 2 adopt must bind existing PTY to same workflow_run: {second}"
            );
            // Phase 3: repeat with new request_key -> no duplicate agent terminal or PTY launch!
            let mut again = adopt_params;
            again["request_key"] = json!(format!("2ph-bind-again-{seed}"));
            let _ = world.server_call("remote-b", "card.adopt", again)?;
            let snap = world.server("remote-b").unwrap().snapshot()?;
            let c2p_agents = snap
                .terminals
                .iter()
                .filter(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c2p))
                .count();
            ensure!(
                c2p_agents == 1 && herdr_b.terminal_addresses().len() == terms_before_adopt,
                "2-phase adopt replay created duplicate terminal or PTY"
            );
            // Clean up workflow + PTY.
            let _ = world.server_call(
                "remote-b",
                "workflow.stop",
                json!({ "run_id": second["workflow_run"]["id"] }),
            )?;
            herdr_b.finish_terminal(&manual_addr, 0, "exited");
        }

        // E2: Unusable PTY rejection across all 7 failure modes (`missing`, `unknown`, `session`,
        //     `cwd`, `profile`, `ambiguous`, `core`) keeps workflow and manual runtime intact!
        let remote_b_db = world.servers["remote-b"].home.join("canban.sqlite");
        for failure in [
            "missing",
            "unknown",
            "session",
            "cwd",
            "profile",
            "ambiguous",
            "core",
        ] {
            let cu = world.client_call(
                "card.create",
                json!({ "project_id": pid_b1, "title": format!("Adopt Unusable {failure}") }),
                Some(&pid_b1),
            )?;
            let raw_cu = unqualified_entity_id(cu["id"].as_str().unwrap()).to_owned();
            let src_val = if failure == "profile" {
                // Plain shell terminal has no matching agent engine for `profile_id: "claude"`.
                world.server_call(
                    "remote-b",
                    "terminal.create",
                    json!({
                        "card_id": raw_cu,
                        "name": format!("Manual {failure}"),
                    }),
                )?
            } else {
                world.server_call(
                    "remote-b",
                    "terminal.create",
                    json!({
                        "card_id": raw_cu,
                        "agent": true,
                        "profile_id": "codex",
                        "name": format!("Manual {failure}"),
                    }),
                )?
            };
            let src: TerminalEntry = serde_json::from_value(src_val)?;
            let src_addr = src.runtime.as_ref().unwrap().terminal.clone().unwrap();
            let cu_wt = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_cu)
                .unwrap()
                .worktree
                .unwrap_or_else(|| b_dir1.clone());

            let mut params = json!({
                "view": "full",
                "card_id": raw_cu,
                "profile_id": "codex",
                "cwd": cu_wt,
                "runtime_terminal": src_addr,
                "runtime_session": "default",
                "request_key": format!("adopt-fail-{failure}-{seed}"),
            });
            let mut inserted_dup_id: Option<String> = None;
            match failure {
                "missing" => {
                    params.as_object_mut().unwrap().remove("runtime_terminal");
                }
                "unknown" => params["runtime_terminal"] = json!("t404-missing"),
                "session" => params["runtime_session"] = json!("another-core"),
                "cwd" => params["cwd"] = json!(b_dir2),
                "profile" => params["profile_id"] = json!("claude"),
                "ambiguous" => {
                    let dup_id = format!("ambig-dup-{seed}");
                    let mut dup = src.clone();
                    dup.id = dup_id.clone();
                    dup.number += 1000;
                    let conn = Connection::open(&remote_b_db)?;
                    conn.execute(
                        "INSERT INTO terminals(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            dup.id,
                            dup.origin_project_id,
                            dup.origin_card_id,
                            "running",
                            dup.number as i64,
                            serde_json::to_string(&dup)?
                        ],
                    )?;
                    inserted_dup_id = Some(dup_id);
                }
                "core" => {
                    let new_core = format!("core-adopt-replaced-{seed}");
                    herdr_b.restart_core(new_core.clone());
                    world.wait_until(5_000, |w| {
                        let obs = w
                            .server("remote-b")
                            .unwrap()
                            .call("runtime.observation", json!({}))?;
                        Ok(obs["core_id"].as_str() == Some(new_core.as_str()))
                    })?;
                }
                _ => unreachable!(),
            }
            let expected_src_runtime = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| t.id == src.id)
                .unwrap()
                .runtime;
            let adopted = world.server_call("remote-b", "card.adopt", params)?;
            if let Some(dup_id) = inserted_dup_id {
                let conn = Connection::open(&remote_b_db)?;
                conn.execute("DELETE FROM terminals WHERE id=?1", [&dup_id])?;
            }
            ensure!(
                adopted["card"]["stage"] == "develop"
                    && adopted["workflow_run"]["terminal_id"].is_null(),
                "[{failure}] Expected workflow-only adopt without binding: {adopted}"
            );
            let snap = world.server("remote-b").unwrap().snapshot()?;
            ensure!(
                !snap
                    .terminals
                    .iter()
                    .any(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cu)),
                "[{failure}] Unusable PTY adopt must not create an AgentRun"
            );
            ensure!(
                snap.terminals
                    .iter()
                    .find(|t| t.id == src.id)
                    .unwrap()
                    .runtime
                    == expected_src_runtime,
                "[{failure}] Manual runtime must stay unchanged"
            );
            ensure!(
                !snap.conversations.iter().any(|c| c.card_id == raw_cu),
                "[{failure}] Unusable PTY adopt must not create a Conversation"
            );
            let _ = world.server_call(
                "remote-b",
                "workflow.stop",
                json!({ "run_id": adopted["workflow_run"]["id"] }),
            )?;
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": src.id }),
            );
            herdr_b.finish_terminal(&src_addr, 0, "exited");
        }
        // Let observer refresh after the `"core"` restart.
        world.advance(Duration::from_millis(300))?;

        // E3: Savepoint owner preservation — existing `workflow_run.terminal_id` owner is preserved!
        {
            let co = world.client_call(
                "card.create",
                json!({ "project_id": pid_b1, "title": "Savepoint Owner Preservation" }),
                Some(&pid_b1),
            )?;
            let raw_co = unqualified_entity_id(co["id"].as_str().unwrap()).to_owned();
            // Move card to `develop` -> workflow starts and assigns a real agent owner (`terminal_id`)!
            let _ = world.server_call(
                "remote-b",
                "card.move",
                json!({
                    "card_id": raw_co,
                    "revision": co["revision"],
                    "stage": "develop",
                }),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.workflow_runs.iter().any(|r| {
                    r.card_id.as_deref() == Some(raw_co.as_str())
                        && r.active()
                        && r.terminal_id.is_some()
                }))
            })?;
            let (wf_owner_id, existing_owner_tid) = {
                let snap = world.server("remote-b").unwrap().snapshot()?;
                let r = snap
                    .workflow_runs
                    .iter()
                    .find(|r| r.card_id.as_deref() == Some(raw_co.as_str()) && r.active())
                    .unwrap();
                (r.id.clone(), r.terminal_id.clone().unwrap())
            };
            // Open an extra manual terminal with a DIFFERENT profile (`claude`) on the same card and
            // call `card.adopt` -> savepoint must reject binding and preserve existing owner (`codex`)!
            let manual_co_val = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "card_id": raw_co, "agent": true, "profile_id": "claude" }),
            )?;
            let manual_co: TerminalEntry = serde_json::from_value(manual_co_val)?;
            let manual_co_addr = manual_co
                .runtime
                .as_ref()
                .unwrap()
                .terminal
                .clone()
                .unwrap();
            let co_wt = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_co)
                .unwrap()
                .worktree
                .unwrap_or_else(|| b_dir1.clone());
            let adopt_owner = world.server_call(
                "remote-b",
                "card.adopt",
                json!({
                    "view": "full",
                    "card_id": raw_co,
                    "profile_id": "claude",
                    "cwd": co_wt,
                    "runtime_terminal": manual_co_addr,
                    "runtime_session": "default",
                    "request_key": format!("adopt-preserve-owner-{seed}"),
                }),
            )?;
            ensure!(
                adopt_owner["workflow_run"]["terminal_id"].as_str()
                    == Some(existing_owner_tid.as_str()),
                "Savepoint must preserve existing workflow owner {existing_owner_tid}, got {adopt_owner}"
            );
            ensure!(
                world
                    .server("remote-b")
                    .unwrap()
                    .snapshot()?
                    .terminals
                    .iter()
                    .find(|t| t.id == manual_co.id)
                    .unwrap()
                    .runtime
                    == manual_co.runtime,
                "Manual terminal runtime must stay untouched when workflow already has an owner"
            );
            let _ = world.server_call(
                "remote-b",
                "workflow.stop",
                json!({ "run_id": wf_owner_id }),
            )?;
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": manual_co.id }),
            )?;
            herdr_b.finish_terminal(&manual_co_addr, 0, "exited");
            if let Some(addr) = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| t.id == existing_owner_tid)
                .and_then(|t| t.agent)
                .and_then(|a| a.terminal)
            {
                herdr_b.finish_terminal(&addr, 0, "exited");
            }
        }

        // E4: `terminal.rename` (custom + empty reset), `terminal.context`, `terminal.interrupt`
        //     idempotency, manual labels (`max + 1` per project without durable counter),
        //     ephemeral agent `origin_card_id`, CWD inheritance (worktree/subdir/live inventory/offline/core fence),
        //     agent directory context immutability, and archive worktree protection!
        {
            // Manual labels (`max + 1` per project) + `terminal.rename` + `terminal.context`.
            let c_lbl = world.client_call(
                "card.create",
                json!({ "project_id": pid_b1, "title": "Labels & Archive Card" }),
                Some(&pid_b1),
            )?;
            let raw_c_lbl = unqualified_entity_id(c_lbl["id"].as_str().unwrap()).to_owned();
            // Card-role agents must NOT advance independent terminal labels.
            let _ = world.server_call(
                "remote-b",
                "agent.open",
                json!({ "card_id": raw_c_lbl, "profile_id": "codex" }),
            )?;
            let t1 = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b1 }),
            )?;
            let t2 = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b1 }),
            )?;
            let p2_t1 = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b2 }),
            )?;
            ensure!(
                t1["name"] == "Terminal 1"
                    && t2["name"] == "Terminal 2"
                    && p2_t1["name"] == "Terminal 1"
            );
            let t1_id = t1["id"].as_str().unwrap().to_owned();
            let t2_id = t2["id"].as_str().unwrap().to_owned();
            let p2_t1_id = p2_t1["id"].as_str().unwrap().to_owned();
            // `terminal.rename` custom name, then empty string resets to `"Terminal N"`.
            let ren_custom = world.server_call(
                "remote-b",
                "terminal.rename",
                json!({ "terminal_id": t2_id, "name": "Build Shell" }),
            )?;
            ensure!(ren_custom["name"] == "Build Shell");
            let ren_reset = world.server_call(
                "remote-b",
                "terminal.rename",
                json!({ "terminal_id": t2_id, "name": "" }),
            )?;
            ensure!(
                ren_reset["name"] == "Terminal 2",
                "Empty terminal.rename must restore default name: {ren_reset}"
            );
            let ctx1 = world.server_call(
                "remote-b",
                "terminal.context",
                json!({ "terminal_id": t1_id }),
            )?;
            ensure!(ctx1["project_id"].as_str() == Some(raw_pid_b1.as_str()));

            // Closing Terminal 2 -> next is Terminal 2; closing Terminal 1 while Terminal 2 live -> next is Terminal 3;
            // closing all resets back to Terminal 1; zero `terminal-manual-number` counter in SQLite!
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": t2_id }),
            )?;
            let t2_reborn = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b1 }),
            )?;
            ensure!(t2_reborn["name"] == "Terminal 2");
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": t1_id }),
            )?;
            let t3 = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b1 }),
            )?;
            ensure!(t3["name"] == "Terminal 3");
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": t2_reborn["id"] }),
            )?;
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": t3["id"] }),
            )?;
            let t_reset = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b1 }),
            )?;
            ensure!(t_reset["name"] == "Terminal 1");
            {
                let conn =
                    Connection::open_with_flags(&remote_b_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                let cnt: i64 = conn.query_row(
                    "SELECT count(*) FROM counters WHERE id='terminal-manual-number'",
                    [],
                    |r| r.get(0),
                )?;
                ensure!(
                    cnt == 0,
                    "Must not store durable terminal-manual-number counter"
                );
            }
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": t_reset["id"] }),
            )?;
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": p2_t1_id }),
            )?;

            // Ephemeral agent terminal records `origin_card_id` without Conversation or AgentRun!
            let eph_val = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b1, "card_id": raw_c_lbl, "agent": true }),
            )?;
            let eph: TerminalEntry = serde_json::from_value(eph_val)?;
            ensure!(
                eph.id.starts_with("agent-")
                    && eph.origin_card_id.as_deref() == Some(raw_c_lbl.as_str())
                    && eph.conversation_id.is_none()
                    && eph.agent.is_none(),
                "Ephemeral agent terminal fields mismatch: {eph:?}"
            );
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": eph.id }),
            )?;

            // CWD inheritance: managed worktree root & subdir, card worktree subdir, project subdir,
            // deleted subdir fallback, live inventory cwd, independent terminal observed cwd & offline cache!
            let wt_res = world.server_call(
                "remote-b",
                "worktree.create",
                json!({
                    "project_id": raw_pid_b1,
                    "branch": format!("feat/cwd-inherit-{seed}"),
                    "launch": "shell",
                    "request_key": format!("cwd-inherit-wt-{seed}"),
                }),
            )?;
            let wt: ManagedWorktree = serde_json::from_value(wt_res["worktree"].clone())?;
            let wt_tid = wt_res["terminal"]["id"].as_str().unwrap().to_owned();
            let wt_addr = wt_res["terminal"]["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            let wt_sub = PathBuf::from(&wt.path).join("nested-sub");
            std::fs::create_dir_all(&wt_sub)?;

            // Inherit managed worktree root.
            let inh_wt_root = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "inherit_terminal_id": wt_tid }),
            )?;
            let inh_wt_root_addr = inh_wt_root["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            ensure!(
                herdr_b.terminal_record(&inh_wt_root_addr).unwrap()["cwd"].as_str()
                    == Some(wt.path.as_str()),
                "Expected inherited CWD == managed worktree root"
            );
            // Move foreground into managed worktree subdir -> wait until observer caches `directory_root`
            // and `cwd == wt_sub`, then inherit live cwd and verify deleted-subdir fallback to `wt.path`!
            let wt_canon = std::fs::canonicalize(&wt.path)?;
            herdr_b.set_foreground(&wt_addr, Some(json!({ "cwd": wt_sub, "name": "zsh" })));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.terminal_contexts.iter().any(|c| {
                    c.terminal_id == wt_tid
                        && c.cwd.as_deref() == wt_sub.to_str()
                        && c.directory_root
                            .as_deref()
                            .and_then(|d| std::fs::canonicalize(d).ok())
                            .as_deref()
                            == Some(wt_canon.as_path())
                }))
            })?;
            let inh_wt_sub = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "inherit_terminal_id": wt_tid }),
            )?;
            let inh_wt_sub_addr = inh_wt_sub["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            ensure!(
                herdr_b.terminal_record(&inh_wt_sub_addr).unwrap()["cwd"].as_str()
                    == wt_sub.to_str(),
                "Expected inherited CWD == managed worktree subdirectory"
            );
            // Delete subdir -> falls back cleanly to managed worktree root!
            std::fs::remove_dir_all(&wt_sub)?;
            let inh_wt_fb = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "inherit_terminal_id": wt_tid }),
            )?;
            let inh_wt_fb_addr = inh_wt_fb["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            let inh_wt_fb_cwd = herdr_b.terminal_record(&inh_wt_fb_addr).unwrap()["cwd"]
                .as_str()
                .unwrap()
                .to_owned();
            ensure!(
                std::fs::canonicalize(&inh_wt_fb_cwd)? == wt_canon,
                "Deleted worktree subdir must fall back to worktree root: got={inh_wt_fb_cwd}, expected={}",
                wt_canon.display()
            );

            // Project subdir inheritance & independent terminal observed CWD + offline cache vs `terminal.context`.
            let ind_t = world.server_call("remote-b", "terminal.create", json!({}))?;
            let ind_tid = ind_t["id"].as_str().unwrap().to_owned();
            let ind_addr = ind_t["runtime"]["terminal"].as_str().unwrap().to_owned();
            herdr_b.set_foreground(&ind_addr, Some(json!({ "cwd": b_dir1_sub, "name": "zsh" })));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.terminal_contexts.iter().any(|c| {
                    c.terminal_id == ind_tid && c.project_id.as_deref() == Some(raw_pid_b1.as_str())
                }))
            })?;
            let ind_inh = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "inherit_terminal_id": ind_tid }),
            )?;
            let ind_inh_addr = ind_inh["runtime"]["terminal"].as_str().unwrap().to_owned();
            ensure!(
                herdr_b.terminal_record(&ind_inh_addr).unwrap()["cwd"].as_str()
                    == b_dir1_sub.to_str(),
                "Independent terminal inheritance must use observed foreground project subdir"
            );
            // Move foreground outside all projects -> `project_id` clears; go offline -> cached cwd stays while `terminal.context` reports null!
            let outside_dir = world.project_dir(&format!("outside-cwd-{seed}"))?;
            herdr_b.set_foreground(
                &ind_addr,
                Some(json!({ "cwd": outside_dir, "name": "zsh" })),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.terminal_contexts.iter().any(|c| {
                    c.terminal_id == ind_tid
                        && c.project_id.is_none()
                        && c.cwd.as_deref() == Some(outside_dir.as_str())
                }))
            })?;
            herdr_b.set_offline(true);
            world.advance(Duration::from_millis(300))?;
            {
                let snap = world.server("remote-b").unwrap().snapshot()?;
                ensure!(
                    snap.terminal_contexts.iter().any(|c| {
                        c.terminal_id == ind_tid && c.cwd.as_deref() == Some(outside_dir.as_str())
                    }),
                    "Offline Herdr must retain last-known terminal_contexts"
                );
                let live_ctx = world.server_call(
                    "remote-b",
                    "terminal.context",
                    json!({ "terminal_id": ind_tid }),
                )?;
                ensure!(
                    live_ctx["cwd"].is_null() && live_ctx["project_id"].is_null(),
                    "Explicit terminal.context while offline must report null: {live_ctx}"
                );
            }
            herdr_b.set_offline(false);
            world.advance(Duration::from_millis(300))?;

            // Agent directory context moving to Project 2 MUST NOT reassign its Card/Conversation!
            world.wait_until(8_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_c_lbl && a.terminal.is_some() && !a.finished()
                    })
                }))
            })?;
            let (lbl_agent_tid, lbl_agent_addr) = {
                let snap = world.server("remote-b").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c_lbl))
                    .unwrap();
                (
                    t.id.clone(),
                    t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                )
            };
            herdr_b.set_foreground(
                &lbl_agent_addr,
                Some(json!({ "cwd": b_dir2, "name": "codex" })),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.terminal_contexts.iter().any(|c| {
                    c.terminal_id == lbl_agent_tid
                        && c.project_id.as_deref() == Some(raw_pid_b2.as_str())
                }))
            })?;
            {
                let snap = world.server("remote-b").unwrap().snapshot()?;
                let conv = snap
                    .conversations
                    .iter()
                    .find(|c| c.card_id == raw_c_lbl)
                    .unwrap();
                ensure!(
                    conv.project_id == raw_pid_b1 && conv.card_id == raw_c_lbl,
                    "Agent foreground directory change must never reassign Conversation project/card"
                );
            }

            // Archive worktree protection by an unrelated independent terminal whose foreground cwd is inside the worktree!
            let c_arch = world.client_call(
                "card.create",
                json!({ "project_id": pid_b2, "title": "Archive Protected Worktree" }),
                Some(&pid_b2),
            )?;
            let raw_c_arch = unqualified_entity_id(c_arch["id"].as_str().unwrap()).to_owned();
            let card_sh = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "project_id": raw_pid_b2, "card_id": raw_c_arch }),
            )?;
            let card_sh_id = card_sh["id"].as_str().unwrap().to_owned();
            let arch_wt = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_c_arch)
                .unwrap()
                .worktree
                .context("Card shell terminal must create card worktree")?;
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": card_sh_id }),
            )?;
            // Move unrelated independent terminal `ind_tid` into `arch_wt`, then archive card -> worktree survives!
            herdr_b.set_foreground(&ind_addr, Some(json!({ "cwd": arch_wt, "name": "zsh" })));
            world.advance(Duration::from_millis(250))?;
            let arch_rev = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_c_arch)
                .unwrap()
                .revision;
            let _ = world.server_call(
                "remote-b",
                "card.archive",
                json!({ "card_id": raw_c_arch, "revision": arch_rev }),
            )?;
            ensure!(
                std::path::Path::new(&arch_wt).is_dir(),
                "card.archive deleted worktree still occupied by unrelated independent terminal!"
            );

            // Manual plain-shell `terminal.interrupt` idempotency (routes to `terminal.keys` `["ctrl+c"]`)
            // + Core identity fence rejecting `terminal.end` without bumping card rev!
            let keys_before = herdr_b.sent_keys().len();
            let intr_before = herdr_b.interrupted_terminals().len();
            let intr_args = json!({
                "terminal_id": ind_tid,
                "request_key": format!("man-intr-once-{seed}"),
            });
            let _ = world.server_call("remote-b", "terminal.interrupt", intr_args.clone())?;
            let _ = world.server_call("remote-b", "terminal.interrupt", intr_args)?;
            let keys_after = herdr_b.sent_keys();
            ensure!(
                keys_after.len() == keys_before + 1
                    && keys_after.last() == Some(&(ind_addr.clone(), vec!["ctrl+c".to_owned()]))
                    && herdr_b.interrupted_terminals().len() == intr_before,
                "Idempotent plain-shell terminal.interrupt must send exactly 1 ctrl+c via terminal.keys (got keys={:?}, interrupts={})",
                &keys_after[keys_before..],
                herdr_b
                    .interrupted_terminals()
                    .len()
                    .saturating_sub(intr_before),
            );
            let card_rev_before_fence = world
                .server("remote-b")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_c_lbl)
                .unwrap()
                .revision;
            herdr_b.restart_core(format!("core-fence-final-{seed}"));
            world.wait_until(8_000, |w| {
                let snap = w.server("remote-b").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == ind_tid
                        && t.runtime
                            .as_ref()
                            .is_some_and(|r| r.connection == "replaced")
                }))
            })?;
            ensure!(
                world
                    .server_call(
                        "remote-b",
                        "terminal.end",
                        json!({ "terminal_id": ind_tid })
                    )
                    .is_err(),
                "terminal.end must be fenced off after Core identity replacement"
            );
            // Core fence for `inherit_terminal_id`: mismatched Core must not supply old subdir cwd!
            let fenced_inh = world.server_call(
                "remote-b",
                "terminal.create",
                json!({ "inherit_terminal_id": wt_tid }),
            )?;
            let fenced_addr = fenced_inh["runtime"]["terminal"]
                .as_str()
                .unwrap()
                .to_owned();
            ensure!(
                herdr_b.terminal_record(&fenced_addr).unwrap()["cwd"].as_str() != wt_sub.to_str(),
                "Core identity fence must not inherit stale subdir cwd from replaced Core"
            );
            ensure!(
                world
                    .server("remote-b")
                    .unwrap()
                    .snapshot()?
                    .cards
                    .into_iter()
                    .find(|c| c.id == raw_c_lbl)
                    .unwrap()
                    .revision
                    == card_rev_before_fence,
                "Manual terminal controls and core fence must never advance unrelated Card revisions"
            );
            // Close newly spawned fenced terminal cleanly.
            let _ = world.server_call(
                "remote-b",
                "terminal.close",
                json!({ "terminal_id": fenced_inh["id"] }),
            )?;
            let _ = world.server_call(
                "remote-b",
                "worktree.remove",
                json!({ "worktree_id": wt.id, "force": true }),
            );
        }
        world.hit("sessions.two_phase_adopt_and_cwd_rename");

        // ---------------------------------------------------------------------
        // Part F — Configuration Sessions (`workflow.prompt`, `configuration.open`,
        //          `configuration.bind`, `auto_fresh`, generation guard, & closing
        //          `queued` and `unknown` configuration terminals)
        // ---------------------------------------------------------------------
        {
            let cfg_dir = world.project_dir(&format!("cfg-sessions-{seed}"))?;
            let cfg_proj = world.client_call(
                "project.create",
                json!({
                    "host_id": "remote-a",
                    "name": format!("Config Sessions {seed}"),
                    "directory": cfg_dir,
                }),
                None,
            )?;
            let cfg_pid = cfg_proj["id"].as_str().unwrap().to_owned();
            let raw_cfg_pid = unqualified_entity_id(&cfg_pid).to_owned();
            let cards_before = world.server("remote-a").unwrap().snapshot()?.cards.len();

            // 1. `workflow.prompt` must use English instructions with zero CJK characters.
            let prompt_res = world.client_call(
                "workflow.prompt",
                json!({ "project_id": cfg_pid }),
                Some(&cfg_pid),
            )?;
            let prompt_str = prompt_res["prompt"]
                .as_str()
                .context("workflow.prompt missing prompt")?
                .to_owned();
            ensure!(
                prompt_str.contains("Workflow Configuration Assistant")
                    && !prompt_str
                        .chars()
                        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
                "workflow.prompt must be English without CJK characters"
            );

            // 2. `configuration.open` project ownership, reuse (`reused: true`), `configuration.bind`,
            //    stale generation rejection, exact history resume (`generation == 2`), & created_at refresh!
            let open_params = json!({
                "project_id": cfg_pid,
                "profile_id": "codex",
                "prompt": prompt_str,
            });
            let first =
                world.client_call("configuration.open", open_params.clone(), Some(&cfg_pid))?;
            let again =
                world.client_call("configuration.open", open_params.clone(), Some(&cfg_pid))?;
            let cfg_tid = unqualified_entity_id(first["terminal_id"].as_str().unwrap()).to_owned();
            ensure!(
                unqualified_entity_id(again["terminal_id"].as_str().unwrap()) == cfg_tid
                    && again["reused"] == true,
                "Second configuration.open must reuse active session: first={first} again={again}"
            );
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == cfg_tid
                        && t.runtime
                            .as_ref()
                            .is_some_and(|r| r.state == "running" && r.terminal.is_some())
                }))
            })?;
            let (cfg_addr_1, gen1_created_at) = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                ensure!(
                    snap.cards.len() == cards_before,
                    "configuration.open must never create a business Card"
                );
                let t = snap.terminals.iter().find(|t| t.id == cfg_tid).unwrap();
                ensure!(t.agent.is_none());
                (
                    t.runtime.as_ref().unwrap().terminal.clone().unwrap(),
                    t.created_at,
                )
            };
            let bind_ok = world.server_call(
                "remote-a",
                "configuration.bind",
                json!({
                    "configuration_id": cfg_tid,
                    "generation": 1,
                    "session_id": format!("exact-cfg-history-{seed}"),
                }),
            )?;
            ensure!(
                bind_ok["bound"] == true,
                "configuration.bind failed: {bind_ok}"
            );
            // Conflicting session_id on same generation rejected.
            ensure!(
                world
                    .server_call(
                        "remote-a",
                        "configuration.bind",
                        json!({
                            "configuration_id": cfg_tid,
                            "generation": 1,
                            "session_id": "wrong-session",
                        }),
                    )
                    .is_err(),
                "Conflicting configuration.bind session_id must fail"
            );

            // Finish gen 1 PTY -> advance virtual clock -> reopen -> resumes at gen 2 with preserved history!
            herdr_a.finish_terminal(&cfg_addr_1, 0, "exited");
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .configuration_sessions
                    .iter()
                    .any(|s| s.id == cfg_tid && s.state == "finished"))
            })?;
            world.advance(Duration::from_millis(50))?;
            let resumed_open =
                world.client_call("configuration.open", open_params, Some(&cfg_pid))?;
            ensure!(resumed_open["reused"] == false);
            // Reject stale generation 1 bind against generation 2!
            ensure!(
                world
                    .server_call(
                        "remote-a",
                        "configuration.bind",
                        json!({
                            "configuration_id": cfg_tid,
                            "generation": 1,
                            "session_id": "stale-gen-1",
                        }),
                    )
                    .is_err(),
                "Stale generation configuration.bind must fail"
            );
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == cfg_tid
                        && t.runtime.as_ref().is_some_and(|r| {
                            r.state == "running"
                                && r.terminal.as_deref().is_some_and(|a| a != cfg_addr_1)
                        })
                }))
            })?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let sess = snap
                    .configuration_sessions
                    .iter()
                    .find(|s| s.id == cfg_tid)
                    .unwrap();
                let term = snap.terminals.iter().find(|t| t.id == cfg_tid).unwrap();
                ensure!(
                    sess.generation == 2
                        && sess.provider_session_id.as_deref()
                            == Some(&format!("exact-cfg-history-{seed}"))
                        && term.created_at > gen1_created_at,
                    "Resumed configuration session mismatch: sess={sess:?} created_at={}",
                    term.created_at
                );
                let cfg_addr_2 = term.runtime.as_ref().unwrap().terminal.clone().unwrap();
                herdr_a.finish_terminal(&cfg_addr_2, 0, "exited");
            }
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .configuration_sessions
                    .iter()
                    .any(|s| s.id == cfg_tid && s.state == "finished"))
            })?;

            // 3. Default agent (`developer`) + `auto_fresh` fallback when exact history was never bound!
            let expected_def_key = format!("configuration:{raw_cfg_pid}:workflows:developer");
            let def_open = world.client_call(
                "configuration.open",
                json!({ "project_id": cfg_pid, "auto_fresh": true }),
                Some(&cfg_pid),
            )?;
            ensure!(
                unqualified_entity_id(def_open["terminal_id"].as_str().unwrap())
                    == expected_def_key
                    && def_open["reused"] == false
            );
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == expected_def_key
                        && t.runtime
                            .as_ref()
                            .is_some_and(|r| r.state == "running" && r.terminal.is_some())
                }))
            })?;
            let def_addr_1 = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| t.id == expected_def_key)
                .unwrap()
                .runtime
                .unwrap()
                .terminal
                .unwrap();
            herdr_a.finish_terminal(&def_addr_1, 0, "exited");
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .configuration_sessions
                    .iter()
                    .any(|s| s.id == expected_def_key && s.state == "finished"))
            })?;
            // Without `auto_fresh` or `fresh`, missing provider history must error!
            ensure!(
                world
                    .client_call(
                        "configuration.open",
                        json!({ "project_id": cfg_pid }),
                        Some(&cfg_pid),
                    )
                    .is_err(),
                "Ended unbound configuration session must reject open without auto_fresh"
            );
            let def_restarted = world.client_call(
                "configuration.open",
                json!({ "project_id": cfg_pid, "auto_fresh": true }),
                Some(&cfg_pid),
            )?;
            ensure!(def_restarted["reused"] == false);
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == expected_def_key
                        && t.runtime.as_ref().is_some_and(|r| {
                            r.state == "running"
                                && r.terminal.as_deref().is_some_and(|a| a != def_addr_1)
                        })
                }))
            })?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let sess = snap
                    .configuration_sessions
                    .iter()
                    .find(|s| s.id == expected_def_key)
                    .unwrap();
                ensure!(
                    sess.profile_id == "developer"
                        && sess.generation == 2
                        && sess.provider_session_id.is_none()
                );
                let addr = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == expected_def_key)
                    .unwrap()
                    .runtime
                    .as_ref()
                    .unwrap()
                    .terminal
                    .clone()
                    .unwrap();
                herdr_a.finish_terminal(&addr, 0, "exited");
            }
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .configuration_sessions
                    .iter()
                    .any(|s| s.id == expected_def_key && s.state == "finished"))
            })?;

            // 4. Closing a `queued` configuration session cancels launch without creating a Card!
            herdr_a.set_offline(true);
            let q_open = world.client_call(
                "configuration.open",
                json!({ "project_id": cfg_pid, "profile_id": "planner" }),
                Some(&cfg_pid),
            )?;
            let q_cfg_tid =
                unqualified_entity_id(q_open["terminal_id"].as_str().unwrap()).to_owned();
            let _ = world.server_call(
                "remote-a",
                "terminal.close",
                json!({ "terminal_id": q_cfg_tid }),
            )?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let s = snap
                    .configuration_sessions
                    .iter()
                    .find(|c| c.id == q_cfg_tid)
                    .unwrap();
                ensure!(s.state == "cancelled" && snap.cards.len() == cards_before);
            }

            // 5. Closing an `unknown` configuration session (triggered via `RunFault::FailBeforeCreate`)
            //    cancels session & finishes terminal runtime while preserving error!
            herdr_a.set_offline(false);
            world.advance(Duration::from_millis(1_000))?;
            herdr_a.inject_next_run_fault(RunFault::FailBeforeCreate(
                "No space left on device".into(),
            ));
            let u_open = world.client_call(
                "configuration.open",
                json!({ "project_id": cfg_pid, "profile_id": "reviewer" }),
                Some(&cfg_pid),
            )?;
            let u_cfg_tid =
                unqualified_entity_id(u_open["terminal_id"].as_str().unwrap()).to_owned();
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .configuration_sessions
                    .iter()
                    .any(|c| c.id == u_cfg_tid && c.state == "unknown"))
            })?;
            let closed_u = world.server_call(
                "remote-a",
                "terminal.close",
                json!({
                    "terminal_id": u_cfg_tid,
                    "request_key": format!("close-unknown-cfg-{seed}"),
                }),
            )?;
            ensure!(
                closed_u == json!({ "state": "closed", "terminal_id": u_cfg_tid }),
                "Closing unknown configuration session returned unexpected receipt: {closed_u}"
            );
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let s = snap
                    .configuration_sessions
                    .iter()
                    .find(|c| c.id == u_cfg_tid)
                    .unwrap();
                let rt = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == u_cfg_tid)
                    .and_then(|t| t.runtime.as_ref())
                    .context("Unknown closed config terminal missing runtime")?;
                ensure!(
                    s.state == "cancelled"
                        && rt.state == "finished"
                        && rt
                            .error
                            .as_deref()
                            .is_some_and(|e| e.contains("No space left on device")),
                    "Closed unknown configuration state mismatch: sess={s:?} rt={rt:?}"
                );
            }
        }
        world.hit("sessions.configuration_sessions");

        // Final heal, quiesce, full invariants + SQLite integrity + canonical digest.
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
