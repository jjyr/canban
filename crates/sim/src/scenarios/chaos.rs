//! Full-spectrum interleaved Chaos scenario + determinism self-test.
//!
//! Randomly interleaves operations across ALL 8 subsystems using `world.rng`:
//! 1. Card CRUD, moves, comments & workflow automation triggers
//! 2. Remote host network faults (`partition`, `partition_oneway`, `hold`/`release`, `fail_rate`,
//!    `crash_server`/`bounce_server`) & outbox replay/drain
//! 3. GitHub issue/PR mutations, syncs, and injected 403/500 errors
//! 4. Page queries (`merged_snapshot`, `merged_runtime_state`, `timeline.list`, `card.list`)
//! 5. Global & scoped env updates (with secret leak tracking!)
//! 6. Notification reads (`notifications.read`, `notifications.read_all`)
//! 7. Herdr status transitions (`working`/`idle`/`done`/`blocked`/stale-seq/`missed`/disconnect)
//! 8. Viewer presence toggles & virtual-time jumps for idle reclamation
use crate::{
    github::SimIssue,
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use canban_types::{SyncAction, SyncItemState};
use rand::Rng;
use serde_json::json;
use std::time::Duration;

const CHAOS_FLOW: &str = r#"schema: 1
id: chaos-flow
name: Chaos Flow
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
      profile: developer
      instruction: Chaos work
    next: finish
  - id: finish
    uses: complete
"#;

#[derive(Clone, Debug)]
pub struct ChaosSummary {
    pub seed: u64,
    pub steps: u64,
    pub faults: u64,
    pub operations: u64,
    pub digest: u64,
}

pub fn run(seed: u64, max_ops: usize) -> Result<ChaosSummary> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "chaos");
    // Introduce slight deterministic clock skew between hosts to stress LWW env sync.
    let skew_a = ((seed % 7) as i64 - 3) * 150;
    let skew_b = (((seed / 7) % 7) as i64 - 3) * 150;
    cfg.host_clock_skew_ms.insert("remote-a".into(), skew_a);
    cfg.host_clock_skew_ms.insert("remote-b".into(), skew_b);

    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    // Setup projects across local, remote-a, and remote-b.
    let dir_l = world.project_dir("chaos-local")?;
    let dir_a = world.project_dir("chaos-a")?;
    let dir_b = world.project_dir("chaos-b")?;

    let p_l = world.client_call(
        "project.create",
        json!({ "host_id": "local", "name": "Chaos Local", "directory": dir_l }),
        None,
    )?;
    let p_a = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Chaos Remote A", "directory": dir_a }),
        None,
    )?;
    let p_b = world.client_call(
        "project.create",
        json!({ "host_id": "remote-b", "name": "Chaos Remote B", "directory": dir_b }),
        None,
    )?;
    let pids = [
        ("local".to_owned(), p_l["id"].as_str().unwrap().to_owned()),
        (
            "remote-a".to_owned(),
            p_a["id"].as_str().unwrap().to_owned(),
        ),
        (
            "remote-b".to_owned(),
            p_b["id"].as_str().unwrap().to_owned(),
        ),
    ];

    // Configure lightweight workflow + 1m idle reclaim on remote-a and GitHub repo on remote-b.
    if let Some(srv_a) = world.server("remote-a") {
        let _ = srv_a.call("scheduler.configure", json!({ "idle_minutes": 1 }));
    }
    let sources_a = world.client_call(
        "workflow.sources",
        json!({ "project_id": pids[1].1 }),
        Some(&pids[1].1),
    )?;
    let _ = world.client_call(
        "workflow.apply",
        json!({
            "project_id": pids[1].1,
            "revision": sources_a["revision"],
            "sources": [{ "filename": "chaos-flow.yaml", "source": CHAOS_FLOW }],
        }),
        Some(&pids[1].1),
    )?;

    let gh_repo = "acme/chaos-repo";
    world.github.add_repo(gh_repo, 9001, "main");
    world.github.upsert_issue(
        gh_repo,
        SimIssue {
            number: 1,
            title: "Chaos Issue #1".into(),
            body: "Initial body".into(),
            state: "open".into(),
            author: "alice".into(),
            labels: vec!["chaos".into()],
            updated_at: "2026-06-01T00:00:00Z".into(),
            is_pr: false,
            draft: false,
            merged: false,
            head_sha: String::new(),
            head_ref: String::new(),
            base_ref: String::new(),
            check_conclusion: None,
        },
    );
    let _ = world.client_call(
        "project.github",
        json!({
            "project_id": pids[2].1,
            "repository": gh_repo,
            "intake": "all",
            "pr_intake": "all",
            "include_drafts": true,
        }),
        Some(&pids[2].1),
    )?;

    world.advance(Duration::from_millis(300))?;

    let mut card_seq = 0u64;
    let mut env_seq = 0u64;
    let mut gh_issue_seq = 1u64;
    let mut partitioned_a = false;
    let mut partitioned_b = false;
    let mut held_a = false;
    let mut crashed_b = false;
    let mut ops_done = 0u64;

    for op_idx in 0..max_ops {
        let roll: u32 = world.rng.random_range(0..100);
        ops_done += 1;

        match roll {
            // 0..24: Create a Card on a random host (even while partitioned -> outbox!).
            0..=24 => {
                let idx = world.rng.random_range(0..pids.len());
                card_seq += 1;
                let pid = pids[idx].1.clone();
                let title = format!("Chaos Card #{card_seq}");
                let _ = world.client_call(
                    "card.create",
                    json!({
                        "project_id": pid,
                        "title": title,
                        "body": format!("Generated at op {op_idx}"),
                    }),
                    Some(&pid),
                );
            }
            // 25..44: Update, comment on, or move an existing Card.
            25..=44 => {
                if let Ok(merged) = world.client_merged_snapshot()
                    && !merged.cards.is_empty()
                {
                    let c_idx = world.rng.random_range(0..merged.cards.len());
                    let card = &merged.cards[c_idx];
                    let pid = card.project_id.clone();
                    let cid = card.id.clone();
                    let rev = card.revision;
                    let sub: u32 = world.rng.random_range(0..3);
                    match sub {
                        0 => {
                            let _ = world.client_call(
                                "card.update",
                                json!({
                                    "card_id": cid,
                                    "revision": rev,
                                    "title": format!("Updated-{} op{}", card.number, op_idx),
                                }),
                                Some(&pid),
                            );
                        }
                        1 => {
                            let _ = world.client_call(
                                "comment.add",
                                json!({
                                    "card_id": cid,
                                    "body": format!("Chaos comment op={op_idx}"),
                                }),
                                Some(&pid),
                            );
                        }
                        _ => {
                            let stages = ["backlog", "develop", "review"];
                            let stage = stages[world.rng.random_range(0..stages.len())];
                            let _ = world.client_call(
                                "card.move",
                                json!({
                                    "card_id": cid,
                                    "revision": rev,
                                    "stage": stage,
                                }),
                                Some(&pid),
                            );
                        }
                    }
                }
            }
            // 45..54: Env & Secret updates + global sync trigger (secret tracked by oracle!).
            45..=54 => {
                env_seq += 1;
                let secret = format!("sk-chaos-{seed}-{env_seq}");
                oracle.track_secret(secret.clone());
                let _ = world.client_call(
                    "env.save",
                    json!({
                        "scope": "global",
                        "host_id": "local",
                        "entries": [
                            { "key": "CHAOS_MODE", "value": format!("v{env_seq}"), "secret": false },
                            { "key": "CHAOS_SECRET", "value": secret, "secret": true },
                        ]
                    }),
                    None,
                );
                world.client_sync_global_env_now();
            }
            // 55..63: GitHub issue update / sync / fault injection.
            55..=63 => {
                let sub: u32 = world.rng.random_range(0..3);
                match sub {
                    0 => {
                        gh_issue_seq += 1;
                        world.github.upsert_issue(
                            gh_repo,
                            SimIssue {
                                number: gh_issue_seq,
                                title: format!("Chaos Issue #{gh_issue_seq}"),
                                body: "Auto-generated in chaos".into(),
                                state: "open".into(),
                                author: "alice".into(),
                                labels: vec!["chaos".into()],
                                updated_at: format!("2026-06-01T00:{:02}:00Z", gh_issue_seq % 59),
                                is_pr: gh_issue_seq.is_multiple_of(3),
                                draft: false,
                                merged: false,
                                head_sha: format!("sha-{gh_issue_seq}"),
                                head_ref: format!("feat/{gh_issue_seq}"),
                                base_ref: "main".into(),
                                check_conclusion: Some("success".into()),
                            },
                        );
                    }
                    1 => {
                        world.github.inject_error(500, Vec::new());
                    }
                    _ => {
                        let pid_b = pids[2].1.clone();
                        let _ = world.client_call(
                            "github.sync.request",
                            json!({ "project_id": pid_b }),
                            Some(&pid_b),
                        );
                    }
                }
            }
            // 64..73: Herdr status/fault injection + independent terminal creation & close-fallback checks!
            64..=73 => {
                let sub_term: u32 = world.rng.random_range(0..10);
                if sub_term < 3 {
                    // Create an independent terminal in a random project and verify immediate
                    // project placement (never transiently landing in "Other").
                    let idx = world.rng.random_range(0..pids.len());
                    let pid = pids[idx].1.clone();
                    if let Ok(created) = world.client_call(
                        "terminal.create",
                        json!({ "project_id": pid }),
                        Some(&pid),
                    ) && let Some(tid) = created["id"].as_str()
                    {
                        let _ = world.advance(Duration::from_millis(120));
                        if world
                            .client_merged_snapshot()
                            .is_ok_and(|s| s.terminals.iter().any(|t| t.id == tid))
                        {
                            InvariantOracle::assert_terminal_in_project(&world, tid, &pid)?;
                        }
                    }
                } else if sub_term < 5 {
                    // Probe close/exit fallback priority across all currently live terminals,
                    // then optionally exit one shell via Herdr or `terminal.end`.
                    if let Ok(merged) = world.client_merged_snapshot() {
                        let rows = canban_ui::terminals::entries(&merged);
                        let live_ids: Vec<String> = rows
                            .iter()
                            .filter(|e| e.listed())
                            .map(|e| e.saved.id.clone())
                            .collect();
                        if !live_ids.is_empty() {
                            let target = &live_ids[world.rng.random_range(0..live_ids.len())];
                            let closing = std::collections::BTreeSet::new();
                            let _ = InvariantOracle::assert_close_fallback_priority(
                                &merged, &rows, &closing, target,
                            )?;
                            if live_ids.len() > 2 && world.rng.random_bool(0.5) {
                                let _ = world.client_call(
                                    "terminal.end",
                                    json!({ "terminal_id": target }),
                                    None,
                                );
                            }
                        }
                    }
                } else if let Some(herdr_a) = world.herdr("remote-a") {
                    let addrs = herdr_a.terminal_addresses();
                    if !addrs.is_empty() {
                        let addr = &addrs[world.rng.random_range(0..addrs.len())];
                        let sub: u32 = world.rng.random_range(0..5);
                        match sub {
                            0 => herdr_a.set_status(addr, "working", None),
                            1 => herdr_a.set_status(addr, "idle", None),
                            2 => herdr_a.set_status_detailed(
                                addr,
                                json!({
                                    "state": "blocked",
                                    "kind": "input",
                                    "title": "Chaos prompt",
                                    "msg": "Proceed?",
                                }),
                                None,
                            ),
                            3 => herdr_a.inject_stale_event(addr, 1, "done"),
                            _ => herdr_a.emit_missed_snapshot(),
                        }
                    } else if world.rng.random_bool(0.3) {
                        herdr_a.drop_subscribers();
                    }
                }
            }
            // 74..81: Notification center reads, page queries & visible-card completeness probes
            //         (exercises reconnect-window & instant-disconnect card readability continuously!).
            74..=81 => {
                let sub: u32 = world.rng.random_range(0..3);
                match sub {
                    0 => {
                        let _ = world.client_call_all("notifications.read_all", json!({}));
                    }
                    1 => {
                        if let Ok(merged) = world.client_merged_snapshot()
                            && !merged.cards.is_empty()
                        {
                            let c_idx = world.rng.random_range(0..merged.cards.len());
                            let card = &merged.cards[c_idx];
                            let cid = card.id.clone();
                            let pid = card.project_id.clone();
                            // Probe card.get & timeline.list on a visible card even mid-partition/reconnect!
                            if let Ok(raw_card) = world.client_call(
                                "card.get",
                                json!({ "card_id": cid, "view": "full" }),
                                Some(&pid),
                            ) {
                                let parsed: canban_types::Card = serde_json::from_value(raw_card)
                                    .map_err(|e| {
                                    anyhow::anyhow!(
                                        "chaos card.get({cid}) returned incomplete Card: {e}"
                                    )
                                })?;
                                ensure!(
                                    parsed.id == cid
                                        && !parsed.title.trim().is_empty()
                                        && !parsed.stage.trim().is_empty(),
                                    "chaos card.get({cid}) incomplete fields: {parsed:?}"
                                );
                            }
                            if let Ok(tl) = world.client_call(
                                "timeline.list",
                                json!({ "card_id": cid, "tail": true, "limit": 50, "view": "full" }),
                                Some(&pid),
                            ) {
                                ensure!(
                                    tl.get("events").and_then(|v| v.as_array()).is_some(),
                                    "chaos timeline.list({cid}) missing events array: {tl}"
                                );
                            }
                        }
                    }
                    _ => {
                        let _ = world.client_merged_snapshot();
                        let _ = world.client_merged_runtime();
                    }
                }
            }
            // 82..86: Concurrent external server-side mutation + instant-disconnect-after-sync!
            82..=86 => {
                let target_host = if world.rng.random_bool(0.5) {
                    "remote-a"
                } else {
                    "remote-b"
                };
                if let Some(srv) = world.server(target_host)
                    && let Ok(snap) = srv.snapshot()
                {
                    let sub: u32 = world.rng.random_range(0..3);
                    match sub {
                        0 if !snap.projects.is_empty() => {
                            // External server creates a card; let client observe it briefly then optionally partition!
                            let raw_pid = snap.projects[0].id.clone();
                            card_seq += 1;
                            let _ = srv.call(
                                "card.create",
                                json!({
                                    "view": "full",
                                    "project_id": raw_pid,
                                    "title": format!("Ext Server Card #{card_seq}"),
                                    "body": format!("ext-op-{op_idx}"),
                                    "stage": "backlog",
                                }),
                            );
                            world.advance(Duration::from_millis(120))?;
                            if target_host == "remote-a" && !partitioned_a {
                                world.partition("ui-client", "remote-a");
                                partitioned_a = true;
                            }
                        }
                        1 if !snap.cards.is_empty() => {
                            let c = &snap.cards[world.rng.random_range(0..snap.cards.len())];
                            let _ = srv.call(
                                "card.update",
                                json!({
                                    "view": "full",
                                    "card_id": c.id,
                                    "revision": c.revision,
                                    "title": format!("Ext Updated #{} op{op_idx}", c.number),
                                }),
                            );
                        }
                        _ if snap.cards.len() > 2 => {
                            let c = &snap.cards[world.rng.random_range(0..snap.cards.len())];
                            let _ = srv.call(
                                "card.archive",
                                json!({ "card_id": c.id, "revision": c.revision }),
                            );
                        }
                        _ => {}
                    }
                }
            }
            // 87..94: Network & server faults via Turmoil!
            87..=94 => {
                let sub: u32 = world.rng.random_range(0..6);
                match sub {
                    0 => {
                        if partitioned_a {
                            world.repair("ui-client", "remote-a");
                            partitioned_a = false;
                        } else {
                            world.partition("ui-client", "remote-a");
                            partitioned_a = true;
                        }
                    }
                    1 => {
                        if partitioned_b {
                            world.repair("ui-client", "remote-b");
                            partitioned_b = false;
                        } else {
                            world.partition_oneway("ui-client", "remote-b");
                            partitioned_b = true;
                        }
                    }
                    2 => {
                        if held_a {
                            world.release("ui-client", "remote-a");
                            held_a = false;
                        } else {
                            world.hold("ui-client", "remote-a");
                            held_a = true;
                        }
                    }
                    3 => {
                        let rate = if world.rng.random_bool(0.5) {
                            0.0
                        } else {
                            0.08
                        };
                        world.set_fail_rate(rate);
                    }
                    4 => {
                        if crashed_b {
                            world.bounce_server("remote-b")?;
                            crashed_b = false;
                        } else {
                            world.crash_server("remote-b")?;
                            crashed_b = true;
                        }
                    }
                    _ => {
                        // Advance virtual time to exercise timers, heartbeats, and idle reclaim.
                        let jump_secs = world.rng.random_range(1..=10);
                        world.advance(Duration::from_secs(jump_secs))?;
                    }
                }
            }
            // 95..=99: Trigger explicit outbox drain or resolve any conflict deterministically.
            _ => {
                for item in world.client_sync_items() {
                    if matches!(item.state, SyncItemState::Conflict) {
                        let _ = world.client_sync_action(
                            &item.host_id,
                            &item.mutation_id,
                            SyncAction::Discard,
                        );
                    }
                }
                world.advance(Duration::from_millis(250))?;
            }
        }

        world.advance(Duration::from_millis(100))?;
        if op_idx.is_multiple_of(4) {
            oracle.check_step_invariants(&world)?;
        }
    }

    // Heal every fault, resolve any cascading conflicts that occurred during partitions
    // (bounded loop so a genuinely stuck outbox still fails loudly), and check convergence!
    world.heal_and_quiesce(30_000)?;
    for _ in 0..16 {
        let unresolved: Vec<_> = world
            .client_sync_items()
            .into_iter()
            .filter(|item| matches!(item.state, SyncItemState::Conflict | SyncItemState::Failed))
            .collect();
        if unresolved.is_empty() {
            break;
        }
        for item in unresolved {
            let _ = world.client_sync_action(&item.host_id, &item.mutation_id, SyncAction::Discard);
        }
        world.heal_and_quiesce(20_000)?;
    }
    oracle.check_quiescent_invariants(&world)?;

    // Verify every active card visible in `merged_snapshot()` at quiescence is completely readable!
    let merged = world.client_merged_snapshot()?;
    let active_cards: Vec<(String, String)> = merged
        .cards
        .iter()
        .filter(|c| c.active())
        .map(|c| (c.id.clone(), c.project_id.clone()))
        .collect();
    for (cid, pid) in active_cards {
        let _ = InvariantOracle::assert_card_complete_and_readable(&mut world, &cid, &pid)?;
    }

    // Record deterministic structural summary into trace digest (no random UUIDs!).
    let merged = world.client_merged_snapshot()?;
    let mut card_count_by_host = std::collections::BTreeMap::<String, u64>::new();
    for c in &merged.cards {
        let host = merged
            .entity_hosts
            .get(&c.id)
            .cloned()
            .unwrap_or_else(|| "local".into());
        *card_count_by_host.entry(host).or_default() += 1;
    }
    for (host, cnt) in card_count_by_host {
        world.trace.mix_str(&host);
        world.trace.mix_u64(cnt);
    }

    let summary = ChaosSummary {
        seed,
        steps: world.step_count(),
        faults: world.fault_count(),
        operations: ops_done,
        digest: world.trace.digest(),
    };
    ensure!(summary.operations == max_ops as u64);
    Ok(summary)
}
