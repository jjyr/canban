//! Scenario #4 — Page data fetching & mutations at the `canban-client` / UI boundary:
//! write receipt hydration, multi-host routing, colliding raw IDs across hosts,
//! `card.list` & `timeline.list` cursor pagination + `cursor_stale` detection,
//! offline read-cache + pending comment merge, Desktop + Mobile topology parity,
//! and real `canban-ui` presenter/projection DST verification (`CommittedWrites`,
//! timeline pagination + synthetic pending events, `apply_runtime_state_to_snapshot`,
//! `apply_host_summaries`, `SnapshotIndex`/`QueueIndex`, `overview`, `quick_merge`,
//! `plan_backend_events` / `is_runtime_only_event_batch`, and draft error watch).
use crate::{
    exec::SimGitRepo,
    github::SimIssue,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{PRIMARY_CLIENT, SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{
    ActivityBucket, ActivityDetail, ActivityKind, ActivitySegment, AgentBucketStat, AppEvent,
    Delivery, DeliveryMode, DwellBucketTotals, DwellLane, EnvWriteEntry, Event, EventDelivery,
    HostConnectionState, HumanGate, ProjectActivityTotals, Report, RuntimeState, Snapshot,
    TerminalContext,
};
use canban_ui::{
    BackendRefreshAction,
    backend::BackendEvent,
    card::quick_merge,
    index::{KnownTagsIndex, LinkLookup, SnapshotIndex, build_card_status},
    is_runtime_only_event_batch,
    overview::{
        Icon, Tone, describe_row_at, describe_standalone_terminal, describe_unread_update,
        running_agents_count_from, standalone_terminal_is_running, standalone_terminal_needs_you,
    },
    plan_backend_events,
    presenter::{
        CardTimelineState, CommittedWrites, apply_around_page, apply_host_summaries,
        apply_incremental_page, apply_older_page, apply_runtime_state_to_snapshot_at,
        apply_tail_page, confirmed_max_sequence, parse_timeline_page, submission,
    },
    projects::selection::{PendingRemoteRestore, restore as restore_project_selection},
    utilities::{should_apply_usage_push, should_apply_utilities_push},
    with_ui_projection,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    time::Duration,
};

pub fn run(seed: u64) -> Result<u64> {
    // Phase A: Desktop mode (local + remote-a + remote-b).
    let desktop_cfg = SimWorldConfig::desktop_default(seed, "client_pages");
    let desktop_digest = run_with_world(desktop_cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let dir_local = world.project_dir("local")?;
        world
            .exec_runner("local-server")
            .unwrap()
            .register_git_repo(&dir_local, SimGitRepo::default());
        let dir_a = world.project_dir("a")?;
        let dir_b = world.project_dir("b")?;
        let p_local = world.client_call(
            "project.create",
            json!({ "host_id": "local", "name": "Local Proj", "directory": dir_local }),
            None,
        )?;
        let p_a = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Remote A Proj", "directory": dir_a }),
            None,
        )?;
        let p_b = world.client_call(
            "project.create",
            json!({ "host_id": "remote-b", "name": "Remote B Proj", "directory": dir_b }),
            None,
        )?;
        let pid_local = p_local["id"].as_str().unwrap().to_owned();
        let pid_a = p_a["id"].as_str().unwrap().to_owned();
        let pid_b = p_b["id"].as_str().unwrap().to_owned();
        let raw_pid_a = canban_types::split_qualified_host_id(&pid_a)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid_a.clone());
        let raw_pid_b = canban_types::split_qualified_host_id(&pid_b)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid_b.clone());
        world.advance(Duration::from_millis(200))?;

        // Track real `apply_host_summaries` (`HostSummary.reconnect_count -> connect_epoch`) across
        // initial connect, steady polling, partition (`Offline`), heal (`Connected`), and single-tick jump.
        let mut ui_host_epochs: BTreeMap<String, u64> = BTreeMap::new();
        let initial_hosts = world.client_host_summaries();
        let initial_bumped = apply_host_summaries(&[], &initial_hosts, &mut ui_host_epochs);
        ensure!(
            initial_bumped.iter().any(|h| h == "remote-a")
                && ui_host_epochs.get("remote-a").copied() == Some(1)
                && ui_host_epochs.get("local").copied().unwrap_or(0) == 0,
            "initial Connected remote-a must bump connect_epoch to 1 and leave local at 0: bumped={initial_bumped:?} epochs={ui_host_epochs:?}"
        );
        // Routine RTT/summary refresh while staying Connected at same reconnect_count must NOT bump.
        let steady_hosts = world.client_host_summaries();
        let steady_bumped =
            apply_host_summaries(&initial_hosts, &steady_hosts, &mut ui_host_epochs);
        ensure!(
            steady_bumped.is_empty() && ui_host_epochs.get("remote-a").copied() == Some(1),
            "routine host summary tick must not bump connect_epoch: bumped={steady_bumped:?} epochs={ui_host_epochs:?}"
        );
        let mut prev_hosts = steady_hosts;

        // 1. Online write receipt hydration: immediate host routing without waiting on resync.
        let c_a = world.client_call(
            "card.create",
            with_ui_projection(
                "card.create",
                json!({ "project_id": pid_a, "title": "Page Card A", "body": "initial" }),
            ),
            Some(&pid_a),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.create", "remote-a", &c_a);
        let cid_a = c_a["id"].as_str().unwrap().to_owned();
        ensure!(c_a["revision"].as_u64().unwrap_or(0) >= 1);
        let routed =
            world
                .client()
                .resolve_target_host("card.update", &json!({ "card_id": cid_a }), None);
        ensure!(routed == "remote-a", "Got routed={routed:?}");

        let c_b = world.client_call(
            "card.create",
            with_ui_projection(
                "card.create",
                json!({ "project_id": pid_b, "title": "Page Card B", "body": "initial" }),
            ),
            Some(&pid_b),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.create", "remote-b", &c_b);
        let cid_b = c_b["id"].as_str().unwrap().to_owned();
        let c_local = world.client_call(
            "card.create",
            with_ui_projection(
                "card.create",
                json!({ "project_id": pid_local, "title": "Page Card Local", "body": "initial" }),
            ),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.create", "local", &c_local);
        let cid_local = c_local["id"].as_str().unwrap().to_owned();

        // 2. Colliding raw Card IDs across `remote-a` and `remote-b`:
        //    Both hosts store raw ID `"colliding-raw-card"`; multi-host qualification must keep
        //    them strictly distinct and routable!
        let shared_raw = "colliding-raw-card";
        let _ = world.server_call(
            "remote-a",
            "card.create",
            json!({ "id": shared_raw, "project_id": raw_pid_a, "title": "Collision on A", "body": "host-a" }),
        )?;
        let _ = world.server_call(
            "remote-b",
            "card.create",
            json!({ "id": shared_raw, "project_id": raw_pid_b, "title": "Collision on B", "body": "host-b" }),
        )?;
        world.wait_until(10_000, |w| {
            let merged = w.client_merged_snapshot()?;
            let has_a = merged.cards.iter().any(|c| c.title == "Collision on A");
            let has_b = merged.cards.iter().any(|c| c.title == "Collision on B");
            Ok(has_a && has_b)
        })?;
        let (id_on_a, id_on_b) = {
            let merged = world.client_merged_snapshot()?;
            let a = merged
                .cards
                .iter()
                .find(|c| c.title == "Collision on A")
                .unwrap()
                .id
                .clone();
            let b = merged
                .cards
                .iter()
                .find(|c| c.title == "Collision on B")
                .unwrap()
                .id
                .clone();
            (a, b)
        };
        ensure!(
            id_on_a != id_on_b && (id_on_a.contains("::") || id_on_b.contains("::")),
            "Colliding raw Card IDs across remote-a and remote-b were not disambiguated: a={id_on_a:?} b={id_on_b:?}"
        );
        // Mutate the remote-a copy via client and confirm remote-b copy is untouched!
        let _ = world.client_call(
            "card.update",
            json!({ "card_id": id_on_a, "revision": 1, "title": "Collision on A (edited)" }),
            Some(&pid_a),
        )?;
        world.advance(Duration::from_millis(150))?;
        let srv_b_col = world
            .server("remote-b")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == shared_raw)
            .unwrap();
        ensure!(
            srv_b_col.title == "Collision on B",
            "Editing colliding raw ID on remote-a leaked into remote-b!"
        );
        world.hit("client_pages.colliding_raw_ids");

        // 3. `card.list` cursor pagination + `cursor_stale` detection on concurrent mutation.
        for i in 1..=3 {
            let _ = world.client_call(
                "card.create",
                json!({ "project_id": pid_a, "title": format!("Paged Card {i}"), "body": "paged" }),
                Some(&pid_a),
            )?;
        }
        world.advance(Duration::from_millis(150))?;
        let mut p = json!({ "project_id": pid_a, "stage": "backlog", "limit": 2 });
        let first_page = world.client_call("card.list", p.clone(), Some(&pid_a))?;
        ensure!(first_page["items"].as_array().unwrap().len() == 2);
        let saved_cursor = first_page["next_cursor"].clone();
        ensure!(
            !saved_cursor.is_null(),
            "Expected non-null next_cursor on page 1"
        );

        let mut paged_ids = BTreeSet::new();
        let mut cur_page = first_page;
        loop {
            for item in cur_page["items"].as_array().unwrap() {
                let id = item["id"].as_str().unwrap().to_owned();
                ensure!(
                    paged_ids.insert(id),
                    "Duplicate card ID across card.list pages"
                );
            }
            if cur_page["next_cursor"].is_null() {
                break;
            }
            p["cursor"] = cur_page["next_cursor"].clone();
            cur_page = world.client_call("card.list", p.clone(), Some(&pid_a))?;
        }
        ensure!(
            paged_ids.len() == 5,
            "Expected 5 backlog cards on remote-a across pages, got {}",
            paged_ids.len()
        );
        world.hit("client_pages.card_list_pagination");

        // Mutate a card on remote-a and verify reusing old `saved_cursor` detects `cursor_stale`.
        let rev_a_now = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_a)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.update",
            json!({ "card_id": cid_a, "revision": rev_a_now, "title": "Page Card A Mutated" }),
            Some(&pid_a),
        )?;
        let stale_err = world.client_call(
            "card.list",
            json!({ "project_id": pid_a, "stage": "backlog", "limit": 2, "cursor": saved_cursor }),
            Some(&pid_a),
        );
        ensure!(
            stale_err
                .as_ref()
                .is_err_and(|e| e.to_string().contains("cursor_stale")),
            "Expected cursor_stale error after mutation, got {stale_err:?}"
        );
        world.hit("client_pages.card_list_cursor_stale");

        // 4. `timeline.list` forward cursor pagination (`after_sequence` + `next_cursor`) +
        //    offline read-cache merged with pending comments!
        for i in 1..=4 {
            let _ = world.client_call(
                "comment.add",
                json!({ "card_id": cid_a, "body": format!("online-comment-{i}") }),
                Some(&pid_a),
            )?;
        }
        world.advance(Duration::from_millis(150))?;

        let mut seen_seq = 0u64;
        let mut collected_comments = Vec::new();
        loop {
            let page = world.client_call(
                "timeline.list",
                json!({
                    "card_id": cid_a,
                    "after_sequence": seen_seq,
                    "limit": 2,
                    "view": "full",
                }),
                Some(&pid_a),
            )?;
            let evs = page["events"].as_array().unwrap();
            for e in evs {
                let seq = e["sequence"].as_u64().unwrap_or(0);
                ensure!(
                    seq > seen_seq,
                    "timeline.list sequence not strictly increasing"
                );
                seen_seq = seq;
                if e["kind"] == "comment.created"
                    && let Some(body) = e["body"]
                        .as_str()
                        .or_else(|| e["summary"].as_str())
                        .or_else(|| e["data"]["body"].as_str())
                {
                    collected_comments.push(body.to_owned());
                }
            }
            if page["next_cursor"].is_null() {
                break;
            }
        }
        ensure!(
            collected_comments
                == vec![
                    "online-comment-1",
                    "online-comment-2",
                    "online-comment-3",
                    "online-comment-4"
                ],
            "timeline.list cursor pagination mismatch: {collected_comments:?}"
        );
        world.hit("client_pages.timeline_pagination");

        // Warm tail cache online before partition.
        let _ = world.client_call(
            "timeline.list",
            json!({ "card_id": cid_a, "tail": true, "limit": 200, "view": "full" }),
            Some(&pid_a),
        )?;

        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;

        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_a, "body": "offline-pending-comment" }),
            Some(&pid_a),
        )?;
        let tl_offline = world.client_call(
            "timeline.list",
            with_ui_projection(
                "timeline.list",
                json!({ "card_id": cid_a, "tail": true, "limit": 200 }),
            ),
            Some(&pid_a),
        )?;
        let offline_events = tl_offline["events"].as_array().unwrap();
        let mut seen_ids = BTreeSet::new();
        for ev in offline_events {
            let eid = ev["id"].as_str().unwrap_or("");
            ensure!(seen_ids.insert(eid.to_owned()), "Duplicate event ID {eid}");
        }
        ensure!(
            offline_events.iter().any(|e| {
                e["body"] == "online-comment-1" || e["data"]["body"] == "online-comment-1"
            }) && offline_events.iter().any(|e| {
                e["body"] == "offline-pending-comment"
                    || e["data"]["body"] == "offline-pending-comment"
            }),
            "Offline timeline.list must merge cached committed + pending comments"
        );
        world.hit("client_pages.offline_timeline_merge");

        // Real UI presenter check: pending synthetic events (`data.pending == true`,
        // `sequence == u64::MAX`) from offline `timeline.list` MUST NOT advance `highwater`!
        let offline_page = parse_timeline_page(&tl_offline);
        let mut offline_tl_writes = CommittedWrites::default();
        let mut offline_tl_state = CardTimelineState::default();
        let confirmed_before_heal = confirmed_max_sequence(&offline_page.events);
        ensure!(
            confirmed_before_heal > 0 && confirmed_before_heal < u64::MAX,
            "confirmed_max_sequence must ignore pending synthetic u64::MAX events: {confirmed_before_heal}"
        );
        ensure!(
            offline_page
                .events
                .iter()
                .any(|e| e.data["pending"] == true && e.sequence == u64::MAX),
            "expected synthetic pending event with sequence=u64::MAX in real offline timeline.list"
        );
        apply_tail_page(
            &mut offline_tl_state,
            offline_page.events,
            offline_page.highwater,
            offline_page.has_more_before,
            offline_page.oldest_sequence,
            &mut offline_tl_writes,
            &cid_a,
        );
        ensure!(
            offline_tl_state.highwater == confirmed_before_heal
                && offline_tl_state.highwater < u64::MAX,
            "apply_tail_page must not advance highwater to u64::MAX on pending synthetic event: highwater={} confirmed={confirmed_before_heal}",
            offline_tl_state.highwater
        );

        // Wait until client marks `remote-a` Offline, and confirm `apply_host_summaries` does NOT bump epoch.
        let _ = world.wait_until(8_000, |w| {
            Ok(w.client_host_summaries()
                .iter()
                .any(|h| h.id == "remote-a" && !h.connection.is_connected()))
        });
        let offline_hosts = world.client_host_summaries();
        let offline_bumped = apply_host_summaries(&prev_hosts, &offline_hosts, &mut ui_host_epochs);
        ensure!(
            !offline_bumped.iter().any(|h| h == "remote-a")
                && ui_host_epochs.get("remote-a").copied() == Some(1),
            "going Offline must not bump connect_epoch: bumped={offline_bumped:?} epochs={ui_host_epochs:?}"
        );
        prev_hosts = offline_hosts;

        // Local & remote-b remain operational while remote-a is partitioned.
        let local_rev = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_local)
            .unwrap()
            .revision;
        let moved_local = world.client_call(
            "card.move",
            with_ui_projection(
                "card.move",
                json!({ "card_id": cid_local, "revision": local_rev, "stage": "review" }),
            ),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.move", "local", &moved_local);
        let b_rev = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_b)
            .unwrap()
            .revision;
        let upd_b = world.client_call(
            "card.update",
            with_ui_projection(
                "card.update",
                json!({ "card_id": cid_b, "revision": b_rev, "title": "Page Card B Updated" }),
            ),
            Some(&pid_b),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.update", "remote-b", &upd_b);

        oracle.check_step_invariants(world)?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        // Reconnect after partition must bump `remote-a` connect_epoch 1 -> 2, and a single-tick
        // `reconnect_count` jump while staying Connected must bump 2 -> 3 (`local` stays 0).
        let reconnected_hosts = world.client_host_summaries();
        let reconn_bumped =
            apply_host_summaries(&prev_hosts, &reconnected_hosts, &mut ui_host_epochs);
        ensure!(
            reconn_bumped.iter().any(|h| h == "remote-a")
                && ui_host_epochs.get("remote-a").copied() == Some(2),
            "reconnecting remote-a must bump connect_epoch 1 -> 2: bumped={reconn_bumped:?} epochs={ui_host_epochs:?}"
        );
        let mut single_tick_hosts = reconnected_hosts.clone();
        if let Some(ra) = single_tick_hosts.iter_mut().find(|h| h.id == "remote-a") {
            ra.reconnect_count = ra.reconnect_count.saturating_add(1);
        }
        let jump_bumped =
            apply_host_summaries(&reconnected_hosts, &single_tick_hosts, &mut ui_host_epochs);
        ensure!(
            jump_bumped == vec!["remote-a".to_owned()]
                && ui_host_epochs.get("remote-a").copied() == Some(3)
                && ui_host_epochs.get("local").copied().unwrap_or(0) == 0,
            "single-tick reconnect_count jump must bump remote-a 2 -> 3 and keep local at 0: bumped={jump_bumped:?} epochs={ui_host_epochs:?}"
        );

        // Incremental refresh after heal advances `offline_tl_state.highwater` and reconciles the
        // offline comment onto its real finite server sequence.
        let tl_inc_after_heal = world.client_call(
            "timeline.list",
            with_ui_projection(
                "timeline.list",
                json!({ "card_id": cid_a, "after_sequence": offline_tl_state.highwater, "limit": 50 }),
            ),
            Some(&pid_a),
        )?;
        let inc_heal_page = parse_timeline_page(&tl_inc_after_heal);
        apply_incremental_page(
            &mut offline_tl_state,
            inc_heal_page.events,
            inc_heal_page.highwater,
            &mut offline_tl_writes,
            &cid_a,
        );
        ensure!(
            offline_tl_state.highwater > confirmed_before_heal
                && offline_tl_state.highwater < u64::MAX,
            "post-heal incremental page must advance highwater finitely: before={confirmed_before_heal} after={}",
            offline_tl_state.highwater
        );
        let settled_offline = offline_tl_state
            .events
            .iter()
            .filter(|e| e.kind == "comment.created" && e.message == "offline-pending-comment")
            .collect::<Vec<_>>();
        ensure!(
            settled_offline.len() == 1 && settled_offline[0].sequence < u64::MAX,
            "reconciled offline comment must appear once with finite sequence: {settled_offline:?}"
        );

        let tl_after = world.client_call(
            "timeline.list",
            json!({ "card_id": cid_a, "tail": true, "limit": 200, "view": "full" }),
            Some(&pid_a),
        )?;
        let comment_msgs: Vec<&str> = tl_after["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "comment.created")
            .filter_map(|e| {
                e["body"]
                    .as_str()
                    .or_else(|| e["summary"].as_str())
                    .or_else(|| e["data"]["body"].as_str())
            })
            .collect();
        ensure!(
            comment_msgs
                == vec![
                    "online-comment-1",
                    "online-comment-2",
                    "online-comment-3",
                    "online-comment-4",
                    "offline-pending-comment"
                ],
            "Post-heal timeline.list comments mismatch: {comment_msgs:?}"
        );

        let merged = world.client_merged_snapshot()?;
        ensure!(merged.projects.len() == 3);
        ensure!(merged.cards.len() == 8);
        let _rt = world.client_merged_runtime()?;

        // =========================================================================
        // 5. Real UI Presenter Timeline Pagination, Optimistic Comment Overlay,
        //    In-Place `comment.updated` / `comment.deleted` Folding, `older` & `around`.
        // =========================================================================
        let tail_raw = world.client_call(
            "timeline.list",
            with_ui_projection(
                "timeline.list",
                json!({ "card_id": cid_a, "tail": true, "limit": 3 }),
            ),
            Some(&pid_a),
        )?;
        let tail_parsed = parse_timeline_page(&tail_raw);
        let mut tl_writes = CommittedWrites::default();
        let mut tl_state = CardTimelineState::default();
        apply_tail_page(
            &mut tl_state,
            tail_parsed.events,
            tail_parsed.highwater,
            tail_parsed.has_more_before,
            tail_parsed.oldest_sequence,
            &mut tl_writes,
            &cid_a,
        );
        ensure!(
            tl_state.initialized
                && tl_state.has_more_before
                && tl_state.oldest_seq.is_some()
                && tl_state.events.len() == 3,
            "tail page state mismatch: init={} has_more={} oldest={:?} len={}",
            tl_state.initialized,
            tl_state.has_more_before,
            tl_state.oldest_seq,
            tl_state.events.len()
        );
        let tail_comment_ids: Vec<String> = tl_state
            .events
            .iter()
            .filter(|e| e.kind == "comment.created")
            .map(|e| e.id.clone())
            .collect();
        ensure!(
            tail_comment_ids.len() >= 2,
            "expected at least 2 comments in 3-item tail page, got {tail_comment_ids:?}"
        );
        let edit_comment_id = tail_comment_ids[0].clone();
        let delete_comment_id = tail_comment_ids[1].clone();

        // Optimistic `comment.add` overlays immediately before incremental refresh arrives.
        let opt_reply = world.client_call(
            "comment.add",
            with_ui_projection(
                "comment.add",
                json!({ "card_id": cid_a, "body": "optimistic-tail-comment", "mentions": [] }),
            ),
            Some(&pid_a),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "comment.add", "remote-a", &opt_reply);
        let opt_comment_id = opt_reply["id"].as_str().unwrap().to_owned();
        tl_writes.record("comment.add", &opt_reply, "remote-a");
        let stale_rev_before_comment = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_a)
            .unwrap()
            .revision
            .saturating_sub(1);
        let mut stale_comment_snap = world.client_merged_snapshot()?;
        if let Some(c) = stale_comment_snap.cards.iter_mut().find(|c| c.id == cid_a) {
            c.revision = stale_rev_before_comment;
        }
        tl_writes.reconcile(&mut stale_comment_snap);
        if let Some(expected_card_rev) = opt_reply["card_revision"].as_u64() {
            let projected_rev = stale_comment_snap
                .cards
                .iter()
                .find(|c| c.id == cid_a)
                .unwrap()
                .revision;
            ensure!(
                projected_rev >= expected_card_rev,
                "comment.add receipt card_revision={expected_card_rev} failed to advance stale card rev={projected_rev}"
            );
        }
        tl_writes.overlay_card_events(&cid_a, &mut tl_state.events);
        ensure!(
            tl_state
                .events
                .iter()
                .any(|e| e.id == opt_comment_id && e.message == "optimistic-tail-comment"),
            "optimistic comment.add did not overlay onto CardTimelineState"
        );

        // Real `comment.update` & `comment.delete` on earlier tail comments.
        let updated_c = world.client_call(
            "comment.update",
            with_ui_projection(
                "comment.update",
                json!({
                    "card_id": cid_a,
                    "comment_id": edit_comment_id,
                    "body": "Edited tail body",
                    "mentions": [],
                }),
            ),
            Some(&pid_a),
        )?;
        ensure!(updated_c["body"] == json!("Edited tail body"));
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "comment.update", "remote-a", &updated_c);
        tl_writes.record("comment.update", &updated_c, "remote-a");
        let mut optimistic_events = tl_state.events.clone();
        tl_writes.overlay_card_events(&cid_a, &mut optimistic_events);
        ensure!(
            optimistic_events
                .iter()
                .find(|e| e.id == edit_comment_id)
                .is_some_and(|e| e.message == "Edited tail body" && e.data["revision"] == json!(2)),
            "optimistic comment.update overlay mismatch"
        );

        let deleted_c = world.client_call(
            "comment.delete",
            json!({ "card_id": cid_a, "comment_id": delete_comment_id }),
            Some(&pid_a),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "comment.delete", "remote-a", &deleted_c);
        tl_writes.record("comment.delete", &deleted_c, "remote-a");
        tl_writes.overlay_card_events(&cid_a, &mut optimistic_events);
        ensure!(
            optimistic_events
                .iter()
                .find(|e| e.id == delete_comment_id)
                .is_some_and(|e| e.kind == "comment.deleted"),
            "optimistic comment.delete tombstone overlay mismatch"
        );

        // Incremental refresh folds `comment.updated` & `comment.deleted` in place and reconciles.
        let inc_raw = world.client_call(
            "timeline.list",
            with_ui_projection(
                "timeline.list",
                json!({ "card_id": cid_a, "after_sequence": tl_state.highwater, "limit": 50 }),
            ),
            Some(&pid_a),
        )?;
        let inc_parsed = parse_timeline_page(&inc_raw);
        let prev_hw = tl_state.highwater;
        apply_incremental_page(
            &mut tl_state,
            inc_parsed.events,
            inc_parsed.highwater,
            &mut tl_writes,
            &cid_a,
        );
        ensure!(
            tl_state.highwater > prev_hw,
            "incremental timeline refresh must advance highwater: prev={prev_hw} now={}",
            tl_state.highwater
        );
        ensure!(
            tl_state
                .events
                .iter()
                .find(|e| e.id == edit_comment_id)
                .is_some_and(|e| e.message == "Edited tail body"),
            "apply_incremental_page failed to fold comment.updated in place"
        );
        ensure!(
            tl_state
                .events
                .iter()
                .find(|e| e.id == delete_comment_id)
                .is_some_and(|e| e.kind == "comment.deleted"),
            "apply_incremental_page failed to fold comment.deleted in place"
        );
        ensure!(
            tl_writes.pending_comment_count() == 0
                && tl_writes.pending_deleted_comment_count() == 0,
            "comment receipts did not drain after authoritative incremental page: comments={} deleted={}",
            tl_writes.pending_comment_count(),
            tl_writes.pending_deleted_comment_count()
        );

        // Load older activity prepends earlier sequences and clears `has_more_before`.
        let oldest_before = tl_state.oldest_seq.unwrap();
        let older_raw = world.client_call(
            "timeline.list",
            with_ui_projection(
                "timeline.list",
                json!({ "card_id": cid_a, "before_sequence": oldest_before, "limit": 50 }),
            ),
            Some(&pid_a),
        )?;
        let older_parsed = parse_timeline_page(&older_raw);
        apply_older_page(
            &mut tl_state,
            older_parsed.events,
            older_parsed.has_more_before,
            older_parsed.oldest_sequence,
            &mut tl_writes,
            &cid_a,
        );
        ensure!(
            !tl_state.has_more_before
                && tl_state.oldest_seq.is_some_and(|s| s < oldest_before)
                && tl_state
                    .events
                    .windows(2)
                    .all(|w| w[0].sequence <= w[1].sequence),
            "apply_older_page mismatch: has_more={} oldest={:?} prev_oldest={oldest_before}",
            tl_state.has_more_before,
            tl_state.oldest_seq
        );
        let mut around_state = CardTimelineState::default();
        let mid_events: Vec<Event> = tl_state.events.iter().take(3).cloned().collect();
        apply_around_page(&mut around_state, mid_events, false, &mut tl_writes, &cid_a);
        ensure!(
            around_state.events.len() == 3
                && around_state.oldest_seq == Some(around_state.events[0].sequence),
            "apply_around_page state mismatch"
        );
        world.hit("client_pages.ui_presenter_pagination_checked");

        // =========================================================================
        // 6. Real `CommittedWrites` + `with_ui_projection` Lifecycle Across Real RPCs
        //    (projection guards, resolve/reopen, hold_auto_merge, reads, review drafts,
        //     managed worktrees, card.remove, env.save scopes, terminals + hints/contexts).
        // =========================================================================
        ensure!(
            with_ui_projection("comment.update", json!({"fields": ["body"], "body": "x"}))
                .get("view")
                .is_none(),
            "explicit `fields` must not collide with injected `view: full`"
        );
        for m in [
            "config.agents.save",
            "utilities.configure",
            "utilities.awake",
            "scheduler.configure",
            "host.env.refresh",
        ] {
            ensure!(submission::config_only(m), "{m} must be config_only");
        }
        for m in [
            "card.move",
            "card.hold_auto_merge",
            "card.resolve",
            "card.reopen",
            "comment.update",
            "comment.delete",
            "notifications.read",
            "schedule.toggle",
            "terminal.create",
            "review.run_agent",
            "review.send_agent",
            "review.draft.set",
            "review.draft.clear",
            "agent.open",
            "execution.retry",
            "configuration.open",
            "worktree.create",
            "worktree.promote",
        ] {
            ensure!(submission::immediate(m), "{m} must be immediate");
        }

        let mut life_writes = CommittedWrites::default();
        let stale_before_life = world.client_merged_snapshot()?;

        // Real `card.hold_auto_merge`, `card.resolve`, `card.reopen`, `notifications.read`.
        let rev_local_now = stale_before_life
            .cards
            .iter()
            .find(|c| c.id == cid_local)
            .unwrap()
            .revision;
        let held_local = world.client_call(
            "card.hold_auto_merge",
            with_ui_projection(
                "card.hold_auto_merge",
                json!({ "card_id": cid_local, "revision": rev_local_now, "hold": true }),
            ),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(
            PRIMARY_CLIENT,
            "card.hold_auto_merge",
            "local",
            &held_local,
        );
        life_writes.record("card.hold_auto_merge", &held_local, "local");
        let mut stale_held = stale_before_life.clone();
        life_writes.reconcile(&mut stale_held);
        ensure!(
            stale_held
                .cards
                .iter()
                .find(|c| c.id == cid_local)
                .is_some_and(|c| c.hold_auto_merge && c.revision > rev_local_now),
            "card.hold_auto_merge receipt did not overlay immediately on stale snapshot"
        );

        let rev_after_hold = held_local["revision"].as_u64().unwrap();
        let resolved_local = world.client_call(
            "card.resolve",
            with_ui_projection(
                "card.resolve",
                json!({ "card_id": cid_local, "revision": rev_after_hold, "reason": "verified" }),
            ),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.resolve", "local", &resolved_local);
        life_writes.record("card.resolve", &resolved_local, "local");
        let mut stale_res = stale_before_life.clone();
        life_writes.overlay(&mut stale_res);
        ensure!(
            stale_res
                .cards
                .iter()
                .find(|c| c.id == cid_local)
                .is_some_and(|c| c.resolved()),
            "card.resolve receipt did not overlay immediately"
        );

        let rev_after_res = resolved_local["revision"].as_u64().unwrap();
        let reopened_local = world.client_call(
            "card.reopen",
            with_ui_projection(
                "card.reopen",
                json!({ "card_id": cid_local, "revision": rev_after_res }),
            ),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.reopen", "local", &reopened_local);
        life_writes.record("card.reopen", &reopened_local, "local");
        let mut stale_reopen = stale_res.clone();
        life_writes.overlay(&mut stale_reopen);
        ensure!(
            stale_reopen
                .cards
                .iter()
                .find(|c| c.id == cid_local)
                .is_some_and(|c| !c.resolved() && c.revision > rev_after_res),
            "card.reopen receipt did not overlay immediately"
        );

        // Real `notifications.read` receipt overlay & reconcile.
        let read_rcpt = world.client_call(
            "notifications.read",
            json!({ "card_id": cid_a, "sequence": tl_state.highwater }),
            Some(&pid_a),
        )?;
        oracle.record_ui_write_receipt(
            PRIMARY_CLIENT,
            "notifications.read",
            "remote-a",
            &read_rcpt,
        );
        life_writes.record("notifications.read", &read_rcpt, "remote-a");
        let mut stale_read = stale_before_life.clone();
        life_writes.overlay(&mut stale_read);
        ensure!(
            stale_read
                .card_reads
                .iter()
                .any(|r| r.card_id == cid_a && r.sequence >= tl_state.highwater),
            "notifications.read receipt failed to overlay on stale snapshot"
        );

        // Real `review.draft.set` -> overlay -> `review.draft.clear` -> overlay hides draft.
        let draft_set = world.client_call(
            "review.draft.set",
            json!({
                "card_id": cid_local,
                "summary": "Check lock order",
                "verdict": "comment",
                "comments": [],
            }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "review.draft.set", "local", &draft_set);
        life_writes.record("review.draft.set", &draft_set, "local");
        let mut stale_draft = stale_before_life.clone();
        life_writes.reconcile(&mut stale_draft);
        ensure!(
            stale_draft
                .review_drafts
                .iter()
                .any(|d| d.card_id == cid_local && d.summary == "Check lock order"),
            "review.draft.set receipt failed to overlay"
        );
        let draft_clear = world.client_call(
            "review.draft.clear",
            json!({ "card_id": cid_local }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "review.draft.clear", "local", &draft_clear);
        life_writes.record("review.draft.clear", &draft_clear, "local");
        life_writes.overlay(&mut stale_draft);
        ensure!(
            stale_draft
                .review_drafts
                .iter()
                .all(|d| d.card_id != cid_local),
            "review.draft.clear failed to hide overlaid draft"
        );

        // Real `worktree.create` -> overlay -> `worktree.promote` -> removes worktree & adds Card!
        let wt_created = world.client_call(
            "worktree.create",
            json!({
                "project_id": pid_local,
                "branch": "feat/ui-dst-wt",
                "launch": "none",
            }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "worktree.create", "local", &wt_created);
        let wt_id = wt_created["worktree"]["id"].as_str().unwrap().to_owned();
        life_writes.record("worktree.create", &wt_created, "local");
        let mut stale_wt = stale_before_life.clone();
        life_writes.reconcile(&mut stale_wt);
        ensure!(
            stale_wt.worktrees.iter().any(|w| w.id == wt_id),
            "worktree.create receipt failed to overlay ManagedWorktree"
        );
        let wt_promoted = world.client_call(
            "worktree.promote",
            json!({
                "worktree_id": wt_id,
                "title": "Promoted Worktree Card",
                "body": "Investigate #perf and [#lru-cache](tag:lru-cache) bottlenecks",
            }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "worktree.promote", "local", &wt_promoted);
        let cid_wt = wt_promoted["id"].as_str().unwrap().to_owned();
        life_writes.record("worktree.promote", &wt_promoted, "local");
        life_writes.overlay(&mut stale_wt);
        ensure!(
            stale_wt.worktrees.iter().all(|w| w.id != wt_id)
                && stale_wt.cards.iter().any(|c| c.id == cid_wt),
            "worktree.promote must replace ManagedWorktree with promoted Card"
        );

        // Real `card.create` + `card.remove` must NOT resurrect removed Card from create receipt!
        let temp_created = world.client_call(
            "card.create",
            with_ui_projection(
                "card.create",
                json!({ "project_id": pid_local, "title": "Ephemeral Card" }),
            ),
            Some(&pid_local),
        )?;
        let temp_cid = temp_created["id"].as_str().unwrap().to_owned();
        let temp_rev = temp_created["revision"].as_u64().unwrap();
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.create", "local", &temp_created);
        life_writes.record("card.create", &temp_created, "local");
        let temp_removed = world.client_call(
            "card.remove",
            json!({ "card_id": temp_cid, "revision": temp_rev }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.remove", "local", &temp_removed);
        life_writes.record("card.remove", &temp_removed, "local");
        let mut stale_removed = Snapshot::default();
        life_writes.reconcile(&mut stale_removed);
        ensure!(
            stale_removed.cards.iter().all(|c| c.id != temp_cid),
            "card.remove must evict create receipt and never resurrect removed Card"
        );

        // Real `env.save` (global + remote agent scope + project scope, then clear remote agent scope).
        let plain_entries = BTreeMap::from([(
            "UI_DST_KEY".to_owned(),
            EnvWriteEntry::Plain { value: "v1".into() },
        )]);
        let env_global = world.client_call(
            "env.save",
            json!({ "scope": "global", "entries": plain_entries }),
            None,
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "env.save", "local", &env_global);
        life_writes.record("env.save", &env_global, "local");
        let env_agent = world.client_call(
            "env.save",
            json!({
                "host_id": "remote-a",
                "scope": "agent",
                "target_id": "codex",
                "entries": plain_entries,
            }),
            None,
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "env.save", "remote-a", &env_agent);
        life_writes.record("env.save", &env_agent, "remote-a");
        let env_proj = world.client_call(
            "env.save",
            json!({
                "scope": "project",
                "target_id": pid_a,
                "entries": plain_entries,
            }),
            Some(&pid_a),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "env.save", "remote-a", &env_proj);
        life_writes.record("env.save", &env_proj, "remote-a");
        let mut stale_env = Snapshot::default();
        life_writes.overlay(&mut stale_env);
        ensure!(
            stale_env.global_env.contains_key("UI_DST_KEY")
                && stale_env
                    .agent_env("remote-a", "codex")
                    .contains_key("UI_DST_KEY")
                && stale_env.project_env(&pid_a).contains_key("UI_DST_KEY"),
            "env.save receipts did not overlay global/agent/project scopes"
        );
        // Clear remote agent scope with empty map and verify immediate removal + clean reconcile.
        let env_agent_clear = world.client_call(
            "env.save",
            json!({
                "host_id": "remote-a",
                "scope": "agent",
                "target_id": "codex",
                "entries": BTreeMap::<String, EnvWriteEntry>::new(),
            }),
            None,
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "env.save", "remote-a", &env_agent_clear);
        life_writes.record("env.save", &env_agent_clear, "remote-a");
        life_writes.overlay(&mut stale_env);
        ensure!(
            !stale_env.scoped_env.contains_key("agent:remote-a:codex"),
            "clearing empty remote agent env scope must remove key immediately"
        );

        // Real `terminal.create` + `record_terminal_hint` + `reconcile_runtime` + `last_terminal_contexts`
        // + `terminal.end` drain.
        let term_rcpt = world.client_call(
            "terminal.create",
            json!({ "project_id": pid_local, "name": "UI DST Shell", "cwd": dir_local }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "terminal.create", "local", &term_rcpt);
        let term_id = term_rcpt["id"].as_str().unwrap().to_owned();
        let mut term_writes = CommittedWrites::default();
        // Also seed a durable card receipt so we verify `reconcile_runtime` never evicts it!
        term_writes.record("card.reopen", &reopened_local, "local");
        term_writes.record("terminal.create", &term_rcpt, "local");
        let mut rt_probe = Snapshot::default();
        term_writes.reconcile_runtime(&mut rt_probe);
        ensure!(
            rt_probe.terminals.iter().any(|t| t.id == term_id)
                && term_writes.pending_card_count() == 1,
            "reconcile_runtime must overlay terminal while preserving durable card receipts"
        );
        // Wait until real terminal binds to Herdr and gets observed with real cwd.
        world.wait_until(10_000, |w| {
            let s = w.client_merged_snapshot()?;
            Ok(s.terminals.iter().any(|t| {
                t.id == term_id
                    && t.runtime
                        .as_ref()
                        .is_some_and(|r| r.core_id.is_some() && r.terminal.is_some())
            }) && s
                .terminal_contexts
                .iter()
                .any(|c| c.terminal_id == term_id && c.cwd.is_some()))
        })?;
        let live_term_snap = world.client_merged_snapshot()?;
        let live_term_entry = live_term_snap
            .terminals
            .iter()
            .find(|t| t.id == term_id)
            .unwrap()
            .clone();
        let live_rt = live_term_entry.runtime.as_ref().unwrap();
        let hint = TerminalContext {
            terminal_id: term_id.clone(),
            core_id: live_rt.core_id.clone().unwrap(),
            terminal: live_rt.terminal.clone().unwrap(),
            cwd: Some(dir_local.clone()),
            directory_root: Some(dir_local.clone()),
            project_id: Some(pid_local.clone()),
            ..Default::default()
        };
        let mut hint_writes = CommittedWrites::default();
        hint_writes.record_terminal_hint(hint.clone());
        let mut unknown_cwd_snap = live_term_snap.clone();
        for ctx in &mut unknown_cwd_snap.terminal_contexts {
            if ctx.terminal_id == term_id {
                ctx.cwd = None;
                ctx.directory_root = None;
                ctx.project_id = None;
            }
        }
        hint_writes.reconcile(&mut unknown_cwd_snap);
        ensure!(
            unknown_cwd_snap
                .terminal_contexts
                .iter()
                .any(|c| c.terminal_id == term_id && c.cwd.as_deref() == Some(dir_local.as_str()))
                && hint_writes.pending_terminal_hint_count() == 1,
            "terminal placement hint must survive cwd: None until observed"
        );
        // Real observed snapshot clears the placement hint and populates `last_terminal_contexts`.
        let mut obs_snap = live_term_snap.clone();
        hint_writes.reconcile(&mut obs_snap);
        ensure!(
            hint_writes.pending_terminal_hint_count() == 0
                && hint_writes.last_terminal_context_count() >= 1,
            "observed cwd must clear hint and retain last_terminal_context"
        );
        // Transient drop of cwd/context on a still-running terminal retains last observed context!
        let mut transient_snap = live_term_snap.clone();
        transient_snap
            .terminal_contexts
            .retain(|c| c.terminal_id != term_id);
        hint_writes.reconcile(&mut transient_snap);
        ensure!(
            transient_snap
                .terminal_contexts
                .iter()
                .any(|c| c.terminal_id == term_id && c.cwd.is_some()),
            "live terminal must retain last observed context across transient context omission"
        );
        // Ending the real terminal clears retained context cleanly once finished!
        let _ = world.client_call(
            "terminal.end",
            json!({ "terminal_id": term_id }),
            Some(&pid_local),
        )?;
        world.wait_until(10_000, |w| {
            let s = w.client_merged_snapshot()?;
            Ok(s.terminals.iter().any(|t| {
                t.id == term_id && t.runtime.as_ref().is_some_and(|r| r.state == "finished")
            }))
        })?;
        let mut finished_snap = world.client_merged_snapshot()?;
        finished_snap
            .terminal_contexts
            .retain(|c| c.terminal_id != term_id);
        hint_writes.reconcile(&mut finished_snap);
        ensure!(
            finished_snap
                .terminal_contexts
                .iter()
                .all(|c| c.terminal_id != term_id),
            "finished terminal must clear retained last_terminal_context"
        );

        world.heal_and_quiesce(20_000)?;
        let mut caught_up_life = world.client_merged_snapshot()?;
        life_writes.reconcile(&mut caught_up_life);
        ensure!(
            life_writes.pending_card_count() == 0
                && life_writes.pending_card_read_count() == 0
                && life_writes.pending_review_draft_count() == 0
                && life_writes.pending_worktree_count() == 0
                && life_writes.pending_env_update_count() == 0,
            "CommittedWrites did not drain cleanly at quiescence: cards={} reads={} drafts={} wt={} env={}",
            life_writes.pending_card_count(),
            life_writes.pending_card_read_count(),
            life_writes.pending_review_draft_count(),
            life_writes.pending_worktree_count(),
            life_writes.pending_env_update_count(),
        );

        // =========================================================================
        // 7. Real `quick_merge`, `SnapshotIndex`/`KnownTagsIndex`/`LinkLookup`/
        //    `QueueIndex` (`build_card_status`), & `apply_runtime_state_to_snapshot`.
        // =========================================================================
        let snap_for_idx = world.client_merged_snapshot()?;
        let wt_card = snap_for_idx
            .cards
            .iter()
            .find(|c| c.id == cid_wt)
            .unwrap()
            .clone();
        let local_proj = snap_for_idx
            .projects
            .iter()
            .find(|p| p.id == pid_local)
            .unwrap();
        ensure!(
            quick_merge::eligible(&wt_card, Some(local_proj))
                && quick_merge::message(&wt_card)
                    == format!("Canban #{}: {}", wt_card.number, wt_card.title),
            "promoted worktree card must be quick_merge eligible"
        );
        let no_wt_card = snap_for_idx
            .cards
            .iter()
            .find(|c| c.id == cid_local)
            .unwrap();
        ensure!(!quick_merge::eligible(no_wt_card, Some(local_proj)));
        let mut pr_mode_card = wt_card.clone();
        pr_mode_card.delivery_mode = Some(DeliveryMode::GitHubPr);
        ensure!(!quick_merge::eligible(&pr_mode_card, Some(local_proj)));
        let mut archived_card = wt_card.clone();
        archived_card.archived_at = Some(1);
        ensure!(!quick_merge::eligible(&archived_card, Some(local_proj)));
        let mut delivered_card = wt_card.clone();
        delivered_card.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "abc123".into(),
            request_key: "req-1".into(),
            merged_at: 1,
        });
        ensure!(!quick_merge::eligible(&delivered_card, Some(local_proj)));

        // `SnapshotIndex`, `KnownTagsIndex` (`#perf` + `[#lru-cache](tag:lru-cache)`), and
        // `LinkLookup` stability across `card_summaries` update (`comment.add`).
        let idx1 = SnapshotIndex::reuse_or_build(None, &snap_for_idx);
        let known_tags = KnownTagsIndex::build(
            &idx1,
            &BTreeSet::from(["perf".to_owned(), "local-tag".to_owned()]),
        );
        let wt_tags = known_tags.tags_for_card(&cid_wt);
        ensure!(
            wt_tags.contains(&"perf".to_owned()) && wt_tags.contains(&"lru-cache".to_owned()),
            "KnownTagsIndex must extract #perf and explicit tag:lru-cache from real Card body: {wt_tags:?}"
        );
        let lookup1 = LinkLookup::build(&idx1);
        ensure!(
            lookup1.resolve_card_number(&pid_local, wt_card.number) == Some(cid_wt.as_str()),
            "LinkLookup must resolve real Card number to ID"
        );
        let c_sum = world.client_call(
            "comment.add",
            with_ui_projection(
                "comment.add",
                json!({ "card_id": cid_wt, "body": "summary bump", "mentions": [] }),
            ),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "comment.add", "local", &c_sum);
        world.heal_and_quiesce(15_000)?;
        let snap_after_sum = world.client_merged_snapshot()?;
        let idx2 = SnapshotIndex::reuse_or_build(Some(&idx1), &snap_after_sum);
        ensure!(
            !Rc::ptr_eq(&idx1, &idx2) && idx1 != idx2 && idx2.reply_count(&cid_wt) >= 1,
            "comment.add card_summaries bump must rebuild SnapshotIndex and advance reply_count"
        );
        let lookup2 = LinkLookup::build(&idx2);
        ensure!(
            lookup1 == lookup2,
            "LinkLookup must remain equal across card_summaries-only changes"
        );

        // `build_card_status` (`QueueIndex`) FIFO + hold + pause parity across projects.
        let mut q_snap = snap_after_sum.clone();
        for (i, c) in q_snap
            .cards
            .iter_mut()
            .filter(|c| c.active())
            .take(3)
            .enumerate()
        {
            c.stage = "review".into();
            q_snap.workflow_runs.push(
                serde_json::from_value(json!({
                    "id": format!("sim-q-run-{i}"),
                    "project_id": c.project_id,
                    "card_id": c.id,
                    "workflow_id": "delivery",
                    "definition_revision": 1,
                    "definition": {
                        "id": "delivery", "name": "Delivery",
                        "entries": { "review": "deliver" },
                        "steps": {
                            "deliver": { "type": "deliver", "sync_upstream": true, "max_conflict_attempts": 3, "next": "done" },
                            "done": { "type": "complete" }
                        }
                    },
                    "state": "waiting_delivery",
                    "step": "deliver",
                    "visit": 1,
                    "trigger_event_id": null,
                    "input_digest": "d",
                    "inputs": { "queued_at": 10 - i },
                    "outputs": {},
                    "history": [],
                    "error": null,
                    "created_at": 0,
                    "updated_at": 0
                }))
                .unwrap(),
            );
        }
        for paused in [false, true] {
            if let Some(r) = q_snap.workflow_runs.last_mut() {
                r.inputs["queue_pause_reason"] = if paused {
                    json!("Checkout busy")
                } else {
                    Value::Null
                };
            }
            if let Some(c) = q_snap.cards.first_mut() {
                c.hold_auto_merge = paused;
            }
            let status_map = build_card_status(&q_snap);
            for c in q_snap.cards.iter().filter(|c| c.active()) {
                let d = &status_map[&c.id];
                ensure!(
                    d.is_merging == canban_ui::components::activity::is_merging(&q_snap, c)
                        && d.specs_normal
                            == canban_ui::components::activity::card_status_specs(
                                &q_snap, c, false
                            )
                        && d.specs_ui_merging
                            == canban_ui::components::activity::card_status_specs(&q_snap, c, true),
                    "build_card_status parity failed under paused={paused} for Card {}",
                    c.id
                );
            }
        }

        // Real `agent.open` on `cid_wt` -> real `client_merged_runtime()` -> `apply_runtime_state_to_snapshot`
        // + real `SimHerdr` permission wait -> `reminders` / `sound_items_with_reminders` / `describe_row`!
        let base_snap_rt = world.client_merged_snapshot()?;
        let base_idx_rt = SnapshotIndex::reuse_or_build(None, &base_snap_rt);
        let agent_open = world.client_call(
            "agent.open",
            json!({ "card_id": cid_wt, "profile_id": "codex" }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "agent.open", "local", &agent_open);
        world.wait_until(10_000, |w| {
            let s = w.client_merged_snapshot()?;
            Ok(s.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == cid_wt && a.terminal.is_some())
            }))
        })?;
        let live_rt_state = world.client_merged_runtime()?;
        let mut rt_writes = CommittedWrites::default();
        let applied_rt = apply_runtime_state_to_snapshot_at(
            &base_snap_rt,
            live_rt_state.clone(),
            &mut rt_writes,
            world.now_ms(),
        )
        .expect("apply_runtime_state_to_snapshot_at must succeed for known cards");
        let idx_after_rt = SnapshotIndex::reuse_or_build(Some(&base_idx_rt), &applied_rt);
        ensure!(
            Rc::ptr_eq(&base_idx_rt, &idx_after_rt)
                && base_idx_rt.generation == idx_after_rt.generation,
            "apply_runtime_state_to_snapshot must keep SnapshotIndex Rc & generation unchanged"
        );
        ensure!(
            applied_rt
                .host_summaries
                .get("local")
                .is_some_and(|h| h.active_agents >= 1),
            "apply_runtime_state_to_snapshot must reflect local active_agents"
        );
        // `observed_at` + context churn while working keeps `build_card_status` equal.
        let status_working = build_card_status(&applied_rt);
        let mut churn_rt = live_rt_state;
        for t in &mut churn_rt.terminals {
            if let Some(a) = &mut t.agent {
                a.lifecycle.observed_at = a.lifecycle.observed_at.map(|v| v + 1_000);
            }
        }
        for c in &mut churn_rt.terminal_contexts {
            c.additions += 14;
        }
        let applied_churn = apply_runtime_state_to_snapshot_at(
            &applied_rt,
            churn_rt,
            &mut rt_writes,
            world.now_ms(),
        )
        .unwrap();
        ensure!(
            build_card_status(&applied_churn) == status_working,
            "observed_at + additions churn must keep build_card_status equal"
        );

        // Real Herdr permission block -> `apply_runtime_state_to_snapshot_at` reconciles reminders +
        // `sound_items_with_reminders` + `overview::describe_row_at` -> `Icon::ShieldAlert` / `"Permission required"`.
        let wt_agent_addr = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .terminals
            .iter()
            .find_map(|t| {
                t.agent
                    .as_ref()
                    .filter(|a| a.card_id == cid_wt)
                    .and_then(|a| a.terminal.clone())
            })
            .expect("agent herdr addr");
        world.herdr("local-server").unwrap().set_status_detailed(
            &wt_agent_addr,
            json!({
                "state": "blocked",
                "kind": "permission",
                "title": "Allow command?",
                "msg": "cargo test --workspace"
            }),
            Some(1),
        );
        world.wait_until(10_000, |w| {
            let s = w.client_merged_snapshot()?;
            Ok(s.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid_wt
                        && a.lifecycle
                            .waiting
                            .as_ref()
                            .is_some_and(|wt| wt.kind == "permission")
                })
            }))
        })?;
        let blocked_rt = world.client_merged_runtime()?;
        let blocked_now = world.now_ms();
        let applied_blocked = apply_runtime_state_to_snapshot_at(
            &applied_churn,
            blocked_rt,
            &mut rt_writes,
            blocked_now,
        )
        .expect("blocked RuntimeState must apply");
        let wait_id = applied_blocked
            .terminals
            .iter()
            .find_map(|t| {
                t.agent
                    .as_ref()
                    .filter(|a| a.card_id == cid_wt)
                    .and_then(|a| a.lifecycle.waiting.as_ref())
                    .map(|w| format!("wait:{}", w.id))
            })
            .expect("expected RuntimeWait id");
        let blocked_rems = canban_notifications::reminders(&applied_blocked);
        let sound_ids =
            canban_notifications::sound_items_with_reminders(&applied_blocked, &blocked_rems);
        ensure!(
            blocked_rems.iter().any(|r| r.id == wait_id)
                && sound_ids.iter().any(|id| id == &wait_id),
            "blocked RuntimeWait must reconcile into reminders & sound_items: wait_id={wait_id}"
        );
        let perm_rows = canban_notifications::overview_at(&applied_blocked, blocked_now);
        let perm_row = perm_rows
            .iter()
            .find(|r| r.card.id == cid_wt)
            .expect("expected overview row for blocked agent card");
        let perm_ev = describe_row_at(&applied_blocked, perm_row, blocked_now);
        ensure!(
            perm_ev.icon == Icon::ShieldAlert
                && perm_ev.tone == Tone::Attention
                && perm_ev.headline == "Permission required"
                && perm_ev.detail.as_deref() == Some("Allow command? · cargo test --workspace"),
            "permission-blocked describe_row mismatch: {perm_ev:?}"
        );

        // Unknown Card reference in `RuntimeState` must return `None` (escalating to full `app.state`).
        let unknown_rt = RuntimeState {
            terminals: vec![
                serde_json::from_value(json!({
                    "id": "unknown-card-term",
                    "number": 99,
                    "origin_card_id": "missing-card-id",
                    "created_at": 1
                }))
                .unwrap(),
            ],
            ..Default::default()
        };
        ensure!(
            apply_runtime_state_to_snapshot_at(
                &applied_blocked,
                unknown_rt,
                &mut rt_writes,
                blocked_now,
            )
            .is_none(),
            "unknown Card in RuntimeState must return None to escalate to full app.state"
        );

        // =========================================================================
        // 8. Real UI `overview` Classification Matrix: Review / Merge Conflict /
        //    Failed Gate / Unread Prefixes / 1h Automatic Unreported Idle / Standalone Shells.
        // =========================================================================
        {
            let mut gate_snap = applied_blocked.clone();
            let wt_tid = gate_snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid_wt))
                .unwrap()
                .id
                .clone();
            if let Some(t) = gate_snap.terminals.iter_mut().find(|t| t.id == wt_tid)
                && let Some(a) = &mut t.agent
            {
                a.state = "completed".into();
                a.terminal = None;
                a.lifecycle.waiting = None;
            }
            if let Some(c) = gate_snap.cards.iter_mut().find(|c| c.id == cid_wt) {
                c.stage = "review".into();
                c.human_gate = Some(HumanGate {
                    reason: "Agent report ready; human review required".into(),
                    terminal_id: Some(wt_tid.clone()),
                    next_stage: None,
                });
            }
            gate_snap.reports.push(Report {
                id: "sim-rep-1".into(),
                terminal_id: Some(wt_tid.clone()),
                workflow_run_id: None,
                actor: "agent:developer".into(),
                project_id: pid_local.clone(),
                card_id: cid_wt.clone(),
                state: "completed".into(),
                message: "Implemented event-first hierarchy and semantic icons".into(),
                path: None,
                request_key: "sim-rk-1".into(),
                fingerprint: "sim-fp-1".into(),
                created_at: world.now_ms(),
            });
            let r_rows = canban_notifications::overview_at(&gate_snap, world.now_ms());
            let r_row = r_rows.iter().find(|r| r.card.id == cid_wt).unwrap();
            let r_ev = describe_row_at(&gate_snap, r_row, world.now_ms());
            ensure!(
                r_ev.icon == Icon::Eye
                    && r_ev.tone == Tone::Attention
                    && r_ev.headline == "Ready for review"
                    && r_ev.detail.as_deref()
                        == Some("Implemented event-first hierarchy and semantic icons"),
                "review gate describe_row mismatch: {r_ev:?}"
            );

            // Merge conflict gate.
            gate_snap
                .cards
                .iter_mut()
                .find(|c| c.id == cid_wt)
                .unwrap()
                .human_gate
                .as_mut()
                .unwrap()
                .reason = "Merge conflict with main after 2 attempts".into();
            let m_rows = canban_notifications::overview_at(&gate_snap, world.now_ms());
            let m_ev = describe_row_at(
                &gate_snap,
                m_rows.iter().find(|r| r.card.id == cid_wt).unwrap(),
                world.now_ms(),
            );
            ensure!(
                m_ev.icon == Icon::GitMerge
                    && m_ev.headline == "Merge conflict"
                    && m_ev.detail.as_deref() == Some("Merge conflict with main after 2 attempts"),
                "merge conflict describe_row mismatch: {m_ev:?}"
            );

            // Failed agent gate strips `failed: `.
            gate_snap
                .terminals
                .iter_mut()
                .find(|t| t.id == wt_tid)
                .unwrap()
                .agent
                .as_mut()
                .unwrap()
                .state = "failed".into();
            gate_snap
                .cards
                .iter_mut()
                .find(|c| c.id == cid_wt)
                .unwrap()
                .human_gate
                .as_mut()
                .unwrap()
                .reason = "failed: Build failed due to missing symbol".into();
            let f_rows = canban_notifications::overview_at(&gate_snap, world.now_ms());
            let f_ev = describe_row_at(
                &gate_snap,
                f_rows.iter().find(|r| r.card.id == cid_wt).unwrap(),
                world.now_ms(),
            );
            ensure!(
                f_ev.icon == Icon::TriangleAlert
                    && f_ev.tone == Tone::Danger
                    && f_ev.headline == "Agent failed"
                    && f_ev.detail.as_deref() == Some("Build failed due to missing symbol"),
                "failed gate describe_row mismatch: {f_ev:?}"
            );

            // `describe_unread_update` strips machine prefixes & maps semantic icons.
            let mut rem_snap = Snapshot {
                cards: vec![wt_card.clone()],
                ..Default::default()
            };
            let ev_reply = Event {
                id: "sim-u1".into(),
                project_id: pid_local.clone(),
                card_id: cid_wt.clone(),
                sequence: 1,
                kind: "comment.replied".into(),
                actor: "agent:agy".into(),
                message: "Ready for confirmation".into(),
                data: json!({"question_id": "q1"}),
                created_at: 1,
            };
            rem_snap.reminders = canban_notifications::reminders_from_events_at(
                &rem_snap,
                &[ev_reply],
                world.now_ms(),
            );
            let u1 = describe_unread_update(canban_notifications::reminders(&rem_snap).first());
            ensure!(
                u1.icon == Icon::MessageSquare
                    && u1.tone == Tone::Info
                    && u1.headline == "@agy replied"
                    && u1.detail.as_deref() == Some("Ready for confirmation"),
                "comment.replied describe_unread_update mismatch: {u1:?}"
            );
            let ev_done = Event {
                id: "sim-u2".into(),
                project_id: pid_local.clone(),
                card_id: cid_wt.clone(),
                sequence: 2,
                kind: "execution.reported".into(),
                actor: "agent:developer".into(),
                message: "completed: All checks passed".into(),
                data: json!({"report_id": "r2", "state": "completed"}),
                created_at: 2,
            };
            rem_snap.reminders = canban_notifications::reminders_from_events_at(
                &rem_snap,
                &[ev_done],
                world.now_ms(),
            );
            let u2 = describe_unread_update(canban_notifications::reminders(&rem_snap).first());
            ensure!(
                u2.icon == Icon::Check
                    && u2.tone == Tone::Success
                    && u2.headline == "Stage completed · @developer"
                    && u2.detail.as_deref() == Some("All checks passed"),
                "completed describe_unread_update mismatch: {u2:?}"
            );
            let ev_backlog = Event {
                id: "sim-u3".into(),
                project_id: pid_local.clone(),
                card_id: cid_wt.clone(),
                sequence: 3,
                kind: "execution.reported".into(),
                actor: "agent:developer".into(),
                message: "Returned to Backlog: Already landed".into(),
                data: json!({"report_id": "r3", "state": "backlog"}),
                created_at: 3,
            };
            rem_snap.reminders = canban_notifications::reminders_from_events_at(
                &rem_snap,
                &[ev_backlog],
                world.now_ms(),
            );
            let u3 = describe_unread_update(canban_notifications::reminders(&rem_snap).first());
            ensure!(
                u3.icon == Icon::Undo
                    && u3.tone == Tone::Neutral
                    && u3.headline == "Returned to Backlog"
                    && u3.detail.as_deref() == Some("Already landed"),
                "backlog describe_unread_update mismatch: {u3:?}"
            );

            // Automatic unreported idle suppression until `UNREPORTED_IDLE_ALERT_MS` using deterministic `overview_at`.
            let now_ms = world.now_ms();
            let mut idle_snap = applied_blocked.clone();
            if let Some(t) = idle_snap.terminals.iter_mut().find(|t| t.id == wt_tid)
                && let Some(a) = &mut t.agent
            {
                a.trigger = canban_types::ExecutionTrigger::StageEntry;
                a.state = "running".into();
                a.observed = Some(json!({"state": "live", "turn": 1, "status": {"state": "idle"}}));
                a.lifecycle.waiting = None;
                a.lifecycle.observed_at = Some(now_ms);
                a.lifecycle.idle_since = Some(now_ms.saturating_sub(5 * 60_000));
            }
            let quiet_rows = canban_notifications::overview_at(&idle_snap, now_ms);
            let quiet_row = quiet_rows.iter().find(|r| r.card.id == cid_wt).unwrap();
            let quiet_ev = describe_row_at(&idle_snap, quiet_row, now_ms);
            ensure!(
                quiet_row.group == "Idle"
                    && !quiet_row.actionable()
                    && quiet_ev.tone == Tone::Neutral
                    && quiet_ev.headline == "Idle",
                "automatic unreported idle <1h must stay quiet Idle: row={:?} ev={quiet_ev:?}",
                quiet_row.group
            );
            let later_ms = now_ms + canban_notifications::UNREPORTED_IDLE_ALERT_MS + 1_000;
            if let Some(t) = idle_snap.terminals.iter_mut().find(|t| t.id == wt_tid)
                && let Some(a) = &mut t.agent
            {
                a.lifecycle.observed_at = Some(later_ms);
            }
            let stalled_rows = canban_notifications::overview_at(&idle_snap, later_ms);
            let stalled_row = stalled_rows.iter().find(|r| r.card.id == cid_wt).unwrap();
            let stalled_ev = describe_row_at(&idle_snap, stalled_row, later_ms);
            ensure!(
                stalled_row.group == "Needs attention"
                    && stalled_row.actionable()
                    && stalled_ev.icon == Icon::TriangleAlert
                    && stalled_ev.tone == Tone::Danger
                    && stalled_ev.headline == "Stopped before reporting"
                    && stalled_ev.detail.as_deref() == Some("Inspect terminal"),
                "automatic unreported idle >1h must surface Stopped before reporting: {stalled_ev:?}"
            );
        }

        // End the card agent so scheduler active_count returns to 0, then verify 3 real standalone
        // shells: plain shell, working `claude` shell, and permission-blocked `agy` shell!
        world
            .herdr("local-server")
            .unwrap()
            .finish_terminal(&wt_agent_addr, 0, "done");
        world.wait_until(10_000, |w| {
            let srv = w.server("local-server").unwrap();
            Ok(canban_scheduler::active_count_at(&srv.snapshot()?, srv.now_ms()) == 0)
        })?;
        let sh_plain = world.client_call(
            "terminal.create",
            json!({ "project_id": pid_local, "name": "Plain Shell", "cwd": dir_local }),
            Some(&pid_local),
        )?;
        let sh_claude = world.client_call(
            "terminal.create",
            json!({ "project_id": pid_local, "name": "Manual Claude", "cwd": dir_local }),
            Some(&pid_local),
        )?;
        let sh_agy = world.client_call(
            "terminal.create",
            json!({ "project_id": pid_local, "name": "Manual Agy", "cwd": dir_local }),
            Some(&pid_local),
        )?;
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "terminal.create", "local", &sh_plain);
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "terminal.create", "local", &sh_claude);
        oracle.record_ui_write_receipt(PRIMARY_CLIENT, "terminal.create", "local", &sh_agy);
        let id_plain = sh_plain["id"].as_str().unwrap().to_owned();
        let id_claude = sh_claude["id"].as_str().unwrap().to_owned();
        let id_agy = sh_agy["id"].as_str().unwrap().to_owned();
        world.wait_until(10_000, |w| {
            let s = w.server("local-server").unwrap().snapshot()?;
            Ok([&id_plain, &id_claude, &id_agy].iter().all(|tid| {
                s.terminals.iter().any(|t| {
                    t.id == **tid
                        && t.runtime
                            .as_ref()
                            .and_then(|r| r.terminal.as_ref())
                            .is_some()
                })
            }))
        })?;
        let srv_terms = world.server("local-server").unwrap().snapshot()?.terminals;
        let addr_of = |tid: &str| -> String {
            srv_terms
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.runtime.as_ref())
                .and_then(|r| r.terminal.clone())
                .unwrap()
        };
        let addr_claude = addr_of(&id_claude);
        let addr_agy = addr_of(&id_agy);
        let local_herdr = world.herdr("local-server").unwrap();
        local_herdr.upsert_terminal(&addr_claude, &dir_local, "working", 1, Some("claude"));
        local_herdr.upsert_terminal(&addr_agy, &dir_local, "blocked", 1, Some("agy"));
        local_herdr.set_status_detailed(
            &addr_agy,
            json!({
                "state": "blocked",
                "app": "agy",
                "kind": "permission",
                "title": "Allow command?",
                "msg": "git status"
            }),
            Some(1),
        );
        world.wait_until(10_000, |w| {
            let s = w.client_merged_snapshot()?;
            let c_ok = s.terminals.iter().any(|t| {
                t.id == id_claude
                    && t.runtime.as_ref().is_some_and(|r| {
                        r.observed_agent.as_deref() == Some("claude")
                            && r.agent_status.as_deref() == Some("working")
                    })
            });
            let a_ok = s.terminals.iter().any(|t| {
                t.id == id_agy
                    && t.runtime.as_ref().is_some_and(|r| {
                        r.observed_agent.as_deref() == Some("agy")
                            && r.agent_status.as_deref() == Some("blocked")
                            && r.waiting.as_ref().is_some_and(|wt| wt.kind == "permission")
                    })
            });
            Ok(c_ok && a_ok)
        })?;
        let sh_snap = world.client_merged_snapshot()?;
        ensure!(
            canban_scheduler::active_count_at(&sh_snap, world.now_ms()) == 0,
            "standalone manual shell agents must consume 0 Scheduler slots"
        );
        let sh_entries = canban_ui::terminals::entries(&sh_snap);
        let e_plain = sh_entries.iter().find(|e| e.saved.id == id_plain).unwrap();
        let e_claude = sh_entries.iter().find(|e| e.saved.id == id_claude).unwrap();
        let e_agy = sh_entries.iter().find(|e| e.saved.id == id_agy).unwrap();
        ensure!(
            !standalone_terminal_needs_you(&sh_snap, e_plain)
                && !standalone_terminal_is_running(&sh_snap, e_plain),
            "plain shell must be neither needs_you nor running"
        );
        ensure!(
            !standalone_terminal_needs_you(&sh_snap, e_claude)
                && standalone_terminal_is_running(&sh_snap, e_claude),
            "working claude shell must be running and not needs_you"
        );
        let claude_ev = describe_standalone_terminal(e_claude, false, None, None);
        ensure!(
            claude_ev.headline == "Working" && claude_ev.live,
            "working claude standalone terminal describe mismatch: {claude_ev:?}"
        );
        ensure!(
            standalone_terminal_needs_you(&sh_snap, e_agy)
                && !standalone_terminal_is_running(&sh_snap, e_agy),
            "permission-blocked agy shell must be needs_you and not running"
        );
        let agy_ev = describe_standalone_terminal(e_agy, false, None, None);
        ensure!(
            agy_ev.icon == Icon::ShieldAlert
                && agy_ev.tone == Tone::Attention
                && agy_ev.headline == "Permission required"
                && agy_ev.detail.as_deref() == Some("Allow command? · git status"),
            "blocked agy standalone terminal describe mismatch: {agy_ev:?}"
        );
        let sh_rows = canban_notifications::overview_at(&sh_snap, world.now_ms());
        ensure!(
            running_agents_count_from(&sh_snap, &sh_rows, &sh_entries, Some(&pid_local)) == 1,
            "expected exactly 1 running standalone agent (claude) in local project"
        );

        // =========================================================================
        // 9. Real UI Event-Batch Classification, Side-Channel Guards & Draft Error Watch.
        // =========================================================================
        let real_backend_events: Vec<BackendEvent> = world
            .drain_client_events()
            .into_iter()
            .map(BackendEvent::from_client_event)
            .collect();
        ensure!(
            !real_backend_events.is_empty(),
            "expected real MultiHostClient ClientEvents during desktop scenario"
        );
        let real_plan = plan_backend_events(real_backend_events, true);
        ensure!(
            real_plan.refresh_action != BackendRefreshAction::None,
            "real mixed ClientEvents must schedule a refresh action: {real_plan:?}"
        );

        let host_ev = |host: &str, topic: &'static str| BackendEvent::Host {
            host_id: host.into(),
            event: AppEvent::Changed {
                topic,
                card_id: None,
            },
        };
        ensure!(!is_runtime_only_event_batch(&[]));
        ensure!(is_runtime_only_event_batch(&[host_ev(
            "local",
            "terminals"
        )]));
        ensure!(is_runtime_only_event_batch(&[host_ev("local", "reclaim")]));
        ensure!(is_runtime_only_event_batch(&[
            host_ev("local", "terminals"),
            host_ev("local", "reclaim"),
        ]));
        for non_rt in [
            vec![
                host_ev("local", "terminals"),
                host_ev("local", "application"),
            ],
            vec![host_ev("local", "application")],
            vec![host_ev("local", "review")],
            vec![host_ev("local", "github")],
            vec![host_ev("local", "resync")],
            vec![host_ev("local", "runtime")],
            vec![BackendEvent::UtilitiesChanged {
                host_id: "local".into(),
                state: json!({}),
            }],
        ] {
            ensure!(
                !is_runtime_only_event_batch(&non_rt),
                "batch must not classify as runtime-only: {non_rt:?}"
            );
        }

        // Side-channel routing & optimistic pending guards (`UpdateChanged`, `UtilitiesChanged`, `UsageChanged`).
        let side_plan = plan_backend_events(
            vec![
                BackendEvent::UpdateChanged {
                    host_id: "remote-a".into(),
                    state: json!({"state": {"phase": "ignored"}}),
                },
                BackendEvent::UpdateChanged {
                    host_id: "local".into(),
                    state: json!({"state": {"phase": "downloading", "progress": 60}}),
                },
                BackendEvent::UtilitiesChanged {
                    host_id: "remote-a".into(),
                    state: json!({"preferences": {"theme": "ignored"}}),
                },
                BackendEvent::UtilitiesChanged {
                    host_id: "local".into(),
                    state: json!({"preferences": {"theme": "nord"}}),
                },
                BackendEvent::UsageChanged {
                    host_id: "other-host".into(),
                    state: json!({"readings": [{"provider": "ignored"}]}),
                },
                BackendEvent::UsageChanged {
                    host_id: "local".into(),
                    state: json!({"readings": [{"provider": "codex", "used": 17}]}),
                },
            ],
            true,
        );
        ensure!(
            side_plan.local_update_pushes.len() == 1
                && side_plan.local_update_pushes[0]["state"]["phase"] == "downloading"
                && side_plan.local_utility_pushes.len() == 1
                && side_plan.local_utility_pushes[0]["preferences"]["theme"] == "nord"
                && side_plan.usage_pushes.len() == 2
                && side_plan.refresh_action == BackendRefreshAction::None,
            "side-channel event plan mismatch: {side_plan:?}"
        );
        ensure!(should_apply_utilities_push(&BTreeSet::new()));
        ensure!(!should_apply_utilities_push(&BTreeSet::from([
            "utilities.configure"
        ])));
        ensure!(should_apply_usage_push("local", "local"));
        ensure!(!should_apply_usage_push("local", "other-host"));

        // `global_env_sync`-only bumps env_sync_epoch without refreshing; `resync` sets all flags;
        // runtime-only escalates to Full when `!snapshot_ready`.
        let env_only_plan = plan_backend_events(vec![host_ev("local", "global_env_sync")], true);
        ensure!(
            env_only_plan.bump_env_sync_epoch
                && env_only_plan.refresh_action == BackendRefreshAction::None
        );
        let rt_ready_plan = plan_backend_events(vec![host_ev("local", "terminals")], true);
        ensure!(rt_ready_plan.refresh_action == BackendRefreshAction::Runtime);
        let rt_unready_plan = plan_backend_events(vec![host_ev("local", "terminals")], false);
        ensure!(rt_unready_plan.refresh_action == BackendRefreshAction::Full);
        let resync_plan = plan_backend_events(vec![host_ev("local", "resync")], true);
        ensure!(
            resync_plan.notification_resync
                && resync_plan.refresh_local_side_channels
                && resync_plan.bump_github_epoch
                && resync_plan.bump_env_sync_epoch
                && resync_plan.refresh_action == BackendRefreshAction::Full
        );

        // Draft persistence error watch wakes subscribers immediately without polling.
        let mut draft_err_rx = canban_ui::drafts::persistence::subscribe_error_epoch();
        let before_err_epoch = *draft_err_rx.borrow();
        canban_ui::drafts::persistence::bump_error_epoch();
        ensure!(
            draft_err_rx.has_changed().unwrap_or(false)
                && *draft_err_rx.borrow_and_update() > before_err_epoch,
            "drafts::persistence error watch must wake consumer immediately on bump"
        );

        // 9. Assignee projection in Shadow UI `CommittedWrites` + `default_card_terminal` ranking.
        {
            let proj_card = world.client_call(
                "card.create",
                canban_ui::with_ui_projection(
                    "card.create",
                    json!({
                        "project_id": pid_local,
                        "title": "Shadow UI Assignee Card",
                        "assignee": "developer",
                    }),
                ),
                Some(&pid_local),
            )?;
            let proj_cid = proj_card["id"].as_str().unwrap().to_owned();
            let proj_rev = proj_card["revision"].as_u64().unwrap();
            oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.create", "local", &proj_card);
            let mut cw = canban_ui::presenter::CommittedWrites::default();
            cw.record("card.create", &proj_card, "local");
            let mut base_snap = world.client_merged_snapshot()?;
            cw.reconcile(&mut base_snap);
            ensure!(
                base_snap
                    .cards
                    .iter()
                    .find(|c| c.id == proj_cid)
                    .and_then(|c| c.assignee.as_deref())
                    == Some("developer"),
                "CommittedWrites card.create projection lost assignee=developer"
            );

            let updated_card = world.client_call(
                "card.update",
                canban_ui::with_ui_projection(
                    "card.update",
                    json!({
                        "card_id": proj_cid,
                        "revision": proj_rev,
                        "assignee": "reviewer",
                    }),
                ),
                Some(&pid_local),
            )?;
            oracle.record_ui_write_receipt(PRIMARY_CLIENT, "card.update", "local", &updated_card);
            cw.record("card.update", &updated_card, "local");
            cw.reconcile(&mut base_snap);
            ensure!(
                base_snap
                    .cards
                    .iter()
                    .find(|c| c.id == proj_cid)
                    .and_then(|c| c.assignee.as_deref())
                    == Some("reviewer"),
                "CommittedWrites card.update projection lost assignee=reviewer"
            );

            // `default_card_terminal` prioritizes working/running Agent (`@reviewer`) over shell & idle `@developer`.
            let mut tab_snap = Snapshot {
                projects: base_snap.projects.clone(),
                cards: vec![serde_json::from_value(json!({
                    "id": "sim-tab-card",
                    "project_id": pid_local,
                    "number": 900,
                    "title": "Tab Ranking Card",
                    "body": "",
                    "stage": "review",
                    "assignee": "developer",
                    "revision": 1,
                    "created_at": 1
                }))?],
                terminals: serde_json::from_value(json!([
                    {
                        "id": "sim-tab-shell",
                        "number": 1,
                        "origin_card_id": "sim-tab-card",
                        "runtime": { "core_id": "core", "state": "running", "connection": "connected", "terminal": "pty-s" },
                        "created_at": 1
                    },
                    {
                        "id": "sim-tab-card:developer",
                        "conversation_id": "sim-tab-card:developer",
                        "number": 2,
                        "origin_card_id": "sim-tab-card",
                        "runtime": { "core_id": "core", "state": "finished", "connection": "disconnected" },
                        "agent": {
                            "id": "sim-tab-card:developer",
                            "card_id": "sim-tab-card",
                            "project_id": pid_local,
                            "number": 2,
                            "profile": { "id": "developer", "name": "Developer", "provider": "codex", "command": ["codex"] },
                            "prompt": "dev",
                            "state": "completed",
                            "core_id": "core",
                            "created_at": 2
                        },
                        "created_at": 2
                    },
                    {
                        "id": "sim-tab-card:reviewer",
                        "conversation_id": "sim-tab-card:reviewer",
                        "number": 3,
                        "origin_card_id": "sim-tab-card",
                        "runtime": { "core_id": "core", "state": "running", "connection": "connected", "terminal": "pty-r", "agent_status": "working" },
                        "agent": {
                            "id": "sim-tab-card:reviewer",
                            "card_id": "sim-tab-card",
                            "project_id": pid_local,
                            "number": 3,
                            "profile": { "id": "reviewer", "name": "Reviewer", "provider": "claude", "command": ["claude"] },
                            "prompt": "review",
                            "state": "running",
                            "connection_state": "connected",
                            "core_id": "core",
                            "terminal": "pty-r",
                            "created_at": 3
                        },
                        "created_at": 3
                    }
                ]))?,
                ..Default::default()
            };
            ensure!(
                canban_ui::terminals::default_card_terminal(&tab_snap, "sim-tab-card", None)
                    .as_deref()
                    == Some("sim-tab-card:reviewer"),
                "Expected default_card_terminal to prefer working @reviewer over shell & idle @developer"
            );
            ensure!(
                canban_ui::terminals::default_card_terminal(
                    &tab_snap,
                    "sim-tab-card",
                    Some("sim-tab-shell"),
                )
                .as_deref()
                    == Some("sim-tab-shell"),
                "Expected default_card_terminal to honor still-live recent tab override"
            );
            // When @reviewer finishes, live shell falls ahead of non-live idle history.
            tab_snap.terminals.pop();
            ensure!(
                canban_ui::terminals::default_card_terminal(&tab_snap, "sim-tab-card", None)
                    .as_deref()
                    == Some("sim-tab-shell")
            );
        }
        world.hit("client_pages.ui_event_batch_checked");

        // =========================================================================
        // 10. Comment Views/Projection, `activity.record`, `insights.activity`,
        //     `insights.activity_buckets` Parity & Persistence across Restart,
        //     `insights.events` Filtering, and Cardless Event Pruning.
        // =========================================================================
        {
            // Close prior standalone shells from section 8 so agent-work tracking starts clean.
            for sh_id in [&id_plain, &id_claude, &id_agy] {
                let _ = world.client_call(
                    "terminal.close",
                    json!({ "terminal_id": sh_id }),
                    Some(&pid_local),
                )?;
            }
            world.advance(Duration::from_millis(200))?;

            // 10a. Comment receipt projection vs `view: "full"`, `kinds`/`fields` paging, and
            //      preserving raw stored event fields (`e.message == e.data["body"]`).
            let c_views = world.client_call(
                "card.create",
                json!({
                    "view": "full",
                    "project_id": pid_local,
                    "title": "Comment Views Card",
                    "body": "projection check",
                }),
                Some(&pid_local),
            )?;
            let cid_views = c_views["id"].as_str().unwrap().to_owned();
            let views_card_rev = c_views["revision"].as_u64().unwrap();
            for n in 0..5u64 {
                let p = json!({
                    "card_id": cid_views,
                    "body": format!("unique comment {n}"),
                    "mentions": [],
                    "request_key": format!("views-cmt-{seed}-{n}"),
                });
                let receipt = world.client_call("comment.add", p.clone(), Some(&pid_local))?;
                ensure!(
                    receipt.get("body").is_none()
                        && receipt.get("message").is_none()
                        && receipt["card_revision"] == views_card_rev,
                    "Default comment.add receipt leaked body/message or wrong card_revision: {receipt}"
                );
                let mut full_req = p;
                full_req["view"] = json!("full");
                let full_resp = world.client_call("comment.add", full_req, Some(&pid_local))?;
                let expected_body = format!("unique comment {n}");
                ensure!(
                    full_resp["id"] == receipt["id"]
                        && full_resp["body"] == expected_body
                        && full_resp.get("message").is_none()
                        && full_resp["data"].get("body").is_none(),
                    "Replayed view=full comment.add projection mismatch: {full_resp}"
                );
                ensure!(
                    serde_json::to_string(&full_resp)?
                        .matches(&expected_body)
                        .count()
                        == 1,
                    "view=full comment must not duplicate body text"
                );
                let typed: Event = serde_json::from_value(full_resp)?;
                ensure!(typed.message == expected_body);
            }
            let mut after_seq = 0u64;
            let mut paged_cmts = Vec::new();
            loop {
                let page = world.client_call(
                    "timeline.list",
                    json!({
                        "card_id": cid_views,
                        "kinds": ["comment.created"],
                        "after_sequence": after_seq,
                        "limit": 2,
                    }),
                    Some(&pid_local),
                )?;
                for ev in page["events"].as_array().unwrap() {
                    ensure!(
                        ev["kind"] == "comment.created" && ev.get("message").is_none(),
                        "kinds-filtered timeline event mismatch: {ev}"
                    );
                    paged_cmts.push(ev["id"].clone());
                }
                after_seq = page["last_sequence"].as_u64().unwrap();
                if page["next_cursor"].is_null() {
                    break;
                }
                ensure!(page["next_cursor"] == after_seq);
            }
            ensure!(paged_cmts.len() == 5);
            let empty_kinds = world.client_call(
                "timeline.list",
                json!({ "card_id": cid_views, "kinds": ["never.matches"] }),
                Some(&pid_local),
            )?;
            ensure!(
                empty_kinds["events"] == json!([]) && empty_kinds["last_sequence"] == after_seq
            );
            let selected_fields = world.client_call(
                "timeline.list",
                json!({ "card_id": cid_views, "fields": ["id"], "limit": 1 }),
                Some(&pid_local),
            )?;
            ensure!(
                selected_fields["last_sequence"].as_u64().is_some()
                    && selected_fields["events"][0].as_object().unwrap().len() == 1
            );
            {
                let stored_cmts: Vec<_> = world
                    .server("local-server")
                    .unwrap()
                    .events()?
                    .into_iter()
                    .filter(|e| e.card_id == cid_views && e.kind == "comment.created")
                    .collect();
                ensure!(
                    stored_cmts.len() == 5
                        && stored_cmts
                            .iter()
                            .all(|e| e.message == e.data["body"].as_str().unwrap_or("")),
                    "Stored comment.created events must retain message == data.body"
                );
            }

            // 10b. `activity.record`, real virtual-time open & closed agent work,
            //      `insights.activity`, `insights.activity_buckets` exact parity & restart persistence.
            let dir_local2 = world.project_dir("local-insights-2")?;
            let p_local2 = world.client_call(
                "project.create",
                json!({ "host_id": "local", "name": "Local Insights 2", "directory": dir_local2 }),
                None,
            )?;
            let pid_local2 = p_local2["id"].as_str().unwrap().to_owned();

            // Apply a simple manual agent workflow on `pid_local` to launch a real AgentRun.
            let ins_sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid_local }),
                Some(&pid_local),
            )?;
            let ins_wf = r#"schema: 12
id: insights-agent-flow
name: Insights Agent Flow
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
      instruction: Work for insights activity verification
    next: done
  - id: done
    uses: complete
"#;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid_local,
                    "revision": ins_sources["revision"],
                    "sources": [{ "filename": "insights-agent.yaml", "source": ins_wf }],
                }),
                Some(&pid_local),
            )?;

            // Start a standalone manual `codex` shell on `pid_local2` and record a 10s closed burst.
            let sh_codex = world.client_call(
                "terminal.create",
                json!({ "project_id": pid_local2, "name": "Manual Codex", "cwd": dir_local2 }),
                Some(&pid_local2),
            )?;
            let id_codex = sh_codex["id"].as_str().unwrap().to_owned();
            let addr_codex = sh_codex["runtime"]["terminal"].as_str().unwrap().to_owned();
            let local_herdr = world.herdr("local-server").unwrap();
            local_herdr.set_foreground(
                &addr_codex,
                Some(json!({ "pid": 401, "name": "codex", "argv": ["codex"], "cwd": dir_local2 })),
            );
            local_herdr.set_status_detailed(
                &addr_codex,
                json!({ "state": "working", "app": "codex" }),
                Some(1),
            );
            world.advance(Duration::from_secs(10))?;
            local_herdr.set_status_detailed(
                &addr_codex,
                json!({ "state": "idle", "app": "codex" }),
                Some(1),
            );
            world.advance(Duration::from_millis(500))?;

            // Launch a workflow `developer` agent on `cid_views` (`pid_local`) and keep it OPEN working!
            let _ = world.client_call(
                "workflow.start",
                json!({
                    "workflow_id": "insights-agent-flow",
                    "card_id": cid_views,
                    "entry": "start",
                    "request_key": format!("ins-wf-start-{seed}"),
                }),
                Some(&pid_local),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("local-server").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == cid_views && a.terminal.is_some())
                }))
            })?;
            let (open_tid, open_addr) = {
                let snap = world.server("local-server").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid_views))
                    .unwrap();
                (
                    t.id.clone(),
                    t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                )
            };
            local_herdr.set_status(&open_addr, "working", Some(1));
            world.advance(Duration::from_secs(12))?;

            let now_ts = world.server("local-server").unwrap().now_ms();
            let day_end = now_ts - 2_000;
            let day_start = day_end - 8 * 3_600_000;

            // Validation rejections for `activity.record`.
            let too_many: Vec<Value> = (0..201u64)
                .map(|i| {
                    json!({
                        "start_ms": day_start + i,
                        "end_ms": day_start + i + 100,
                        "lane": "other",
                        "page": "board",
                    })
                })
                .collect();
            ensure!(
                world
                    .client_call(
                        "activity.record",
                        json!({ "segments": too_many }),
                        Some(&pid_local)
                    )
                    .is_err()
            );
            ensure!(
                world
                    .client_call(
                        "activity.record",
                        json!({ "segments": [{ "start_ms": day_start + 500, "end_ms": day_start + 500, "lane": "card", "page": "card" }] }),
                        Some(&pid_local),
                    )
                    .is_err()
            );
            ensure!(
                world
                    .client_call(
                        "activity.record",
                        json!({ "segments": [{ "start_ms": day_start, "end_ms": day_start + 12 * 3_600_000 + 1, "lane": "card", "page": "card" }] }),
                        Some(&pid_local),
                    )
                    .is_err()
            );
            ensure!(
                world
                    .client_call(
                        "activity.record",
                        json!({ "segments": [{ "start_ms": now_ts, "end_ms": now_ts + 120_000, "lane": "card", "page": "card" }] }),
                        Some(&pid_local),
                    )
                    .is_err()
            );
            ensure!(
                world
                    .client_call(
                        "activity.record",
                        json!({ "segments": [{ "start_ms": day_start, "end_ms": day_start + 10_000, "lane": "bogus", "page": "card" }] }),
                        Some(&pid_local),
                    )
                    .is_err()
            );

            // Record valid dwell segments: straddling day_start, fully inside pid_local2,
            // straddling day_end, and fully outside after day_end.
            let rec_res = world.client_call(
                "activity.record",
                json!({
                    "segments": [
                        {
                            "start_ms": day_start - 30_000,
                            "end_ms": day_start + 90_000,
                            "lane": "card",
                            "page": "card",
                            "project_id": pid_local,
                            "card_id": cid_local,
                        },
                        {
                            "start_ms": day_start + 300_000,
                            "end_ms": day_start + 480_000,
                            "lane": "terminal",
                            "page": "terminals",
                            "project_id": pid_local2,
                            "terminal_id": id_codex,
                        },
                        {
                            "start_ms": day_end - 45_000,
                            "end_ms": day_end + 30_000,
                            "lane": "other",
                            "page": "board",
                        },
                        {
                            "start_ms": day_end + 10_000,
                            "end_ms": day_end + 40_000,
                            "lane": "card",
                            "page": "card",
                            "project_id": pid_local,
                            "card_id": cid_local,
                        },
                    ]
                }),
                Some(&pid_local),
            )?;
            ensure!(rec_res["recorded"] == 4);

            // Validation rejections for `insights.activity` and `insights.activity_buckets`.
            ensure!(
                world
                    .client_call(
                        "insights.activity",
                        json!({ "from_ms": day_start, "to_ms": day_start + 9 * 24 * 3_600_000 }),
                        Some(&pid_local),
                    )
                    .is_err()
            );
            ensure!(
                world
                    .client_call(
                        "insights.activity_buckets",
                        json!({ "boundaries": [day_start] }),
                        Some(&pid_local),
                    )
                    .is_err()
            );
            ensure!(
                world
                    .client_call(
                        "insights.activity_buckets",
                        json!({ "boundaries": [day_start + 100, day_start + 100] }),
                        Some(&pid_local),
                    )
                    .is_err()
            );

            let aggregate_window = |segments: &[ActivitySegment],
                                    win_from: u64,
                                    win_to: u64|
             -> ActivityBucket {
                let mut dwell = DwellBucketTotals::default();
                let mut agent_ms = 0u64;
                let mut by_project: BTreeMap<String, ProjectActivityTotals> = BTreeMap::new();
                let mut by_agent_map: BTreeMap<(String, String, String), u64> = BTreeMap::new();
                for seg in segments {
                    let dur = seg
                        .end_ms
                        .min(win_to)
                        .saturating_sub(seg.start_ms.max(win_from));
                    if dur == 0 {
                        continue;
                    }
                    let pid_key = seg.project_id.as_deref().filter(|s| !s.is_empty());
                    match (&seg.kind, &seg.data) {
                        (ActivityKind::Dwell, ActivityDetail::Dwell(d)) => {
                            match d.lane {
                                DwellLane::Card => dwell.card += dur,
                                DwellLane::Terminal => dwell.terminal += dur,
                                DwellLane::Other => dwell.other += dur,
                            }
                            if let Some(p) = pid_key {
                                by_project.entry(p.to_owned()).or_default().dwell_ms += dur;
                            }
                        }
                        (ActivityKind::Agent, ActivityDetail::Agent(a)) => {
                            agent_ms += dur;
                            if let Some(p) = pid_key {
                                by_project.entry(p.to_owned()).or_default().agent_ms += dur;
                            }
                            *by_agent_map
                                .entry((a.profile_id.clone(), a.provider.clone(), a.model.clone()))
                                .or_default() += dur;
                        }
                        _ => {}
                    }
                }
                let mut by_agent: Vec<AgentBucketStat> = by_agent_map
                    .into_iter()
                    .map(|((profile_id, provider, model), ms)| AgentBucketStat {
                        profile_id,
                        provider,
                        model,
                        ms,
                    })
                    .collect();
                by_agent.sort_by(|a, b| {
                    b.ms.cmp(&a.ms)
                        .then_with(|| a.profile_id.cmp(&b.profile_id))
                        .then_with(|| a.provider.cmp(&b.provider))
                        .then_with(|| a.model.cmp(&b.model))
                });
                ActivityBucket {
                    start_ms: win_from,
                    end_ms: win_to,
                    dwell,
                    agent_ms,
                    by_project,
                    by_agent,
                }
            };

            // Unscoped parity check: open workflow agent + closed manual codex + clipped dwell!
            let act_unscoped = world.client_call(
                "insights.activity",
                json!({ "from_ms": day_start, "to_ms": day_end }),
                Some(&pid_local),
            )?;
            let segs_unscoped: Vec<ActivitySegment> =
                serde_json::from_value(act_unscoped["segments"].clone())?;
            let open_agent = segs_unscoped
                .iter()
                .find(|s| s.open && s.terminal_id.as_deref() == Some(open_tid.as_str()))
                .context("Expected open working workflow agent in insights.activity")?;
            let open_info = open_agent.agent_data().unwrap();
            ensure!(
                open_info.profile_id == "developer" && !open_info.manual,
                "Open agent metadata mismatch: {open_info:?}"
            );
            let closed_manual = segs_unscoped
                .iter()
                .find(|s| {
                    s.kind == ActivityKind::Agent
                        && !s.open
                        && s.terminal_id.as_deref() == Some(id_codex.as_str())
                })
                .context("Expected closed manual codex agent segment in insights.activity")?;
            let manual_info = closed_manual.agent_data().unwrap();
            ensure!(
                manual_info.profile_id == "manual"
                    && manual_info.provider == "codex"
                    && manual_info.manual
                    && closed_manual.duration_ms() >= 9_000,
                "Closed manual agent metadata/duration mismatch: {closed_manual:?}"
            );

            let buckets_unscoped_res = world.client_call(
                "insights.activity_buckets",
                json!({ "boundaries": [day_start, day_end] }),
                Some(&pid_local),
            )?;
            let buckets_unscoped: Vec<ActivityBucket> =
                serde_json::from_value(buckets_unscoped_res["buckets"].clone())?;
            ensure!(buckets_unscoped.len() == 1);
            let expected_unscoped = aggregate_window(&segs_unscoped, day_start, day_end);
            ensure!(
                expected_unscoped.dwell == buckets_unscoped[0].dwell
                    && expected_unscoped.agent_ms == buckets_unscoped[0].agent_ms
                    && expected_unscoped.by_project == buckets_unscoped[0].by_project
                    && expected_unscoped.by_agent == buckets_unscoped[0].by_agent,
                "Unscoped insights.activity vs insights.activity_buckets parity mismatch: expected={expected_unscoped:?} actual={:?}",
                buckets_unscoped[0]
            );
            ensure!(
                buckets_unscoped[0].dwell.card == 90_000
                    && buckets_unscoped[0].dwell.terminal == 180_000
                    && buckets_unscoped[0].dwell.other == 45_000
                    && buckets_unscoped[0].agent_ms >= 18_000,
                "Unscoped bucket totals mismatch: {:?}",
                buckets_unscoped[0]
            );

            // Multi-bucket split boundary check across [day_start, day_start + 30_000, day_end].
            let split_res = world.client_call(
                "insights.activity_buckets",
                json!({
                    "boundaries": [day_start, day_start + 30_000, day_end],
                    "project_id": pid_local,
                }),
                Some(&pid_local),
            )?;
            let split_buckets: Vec<ActivityBucket> =
                serde_json::from_value(split_res["buckets"].clone())?;
            ensure!(
                split_buckets.len() == 2
                    && split_buckets[0].dwell.card == 30_000
                    && split_buckets[0].dwell.terminal == 0
                    && split_buckets[1].dwell.card == 60_000
                    && split_buckets[1].agent_ms >= 8_000,
                "Split bucket math mismatch: {split_buckets:?}"
            );

            // Project-scoped parity check on `pid_local`.
            let act_scoped = world.client_call(
                "insights.activity",
                json!({ "from_ms": day_start, "to_ms": day_end, "project_id": pid_local }),
                Some(&pid_local),
            )?;
            let segs_scoped: Vec<ActivitySegment> =
                serde_json::from_value(act_scoped["segments"].clone())?;
            let buckets_scoped_res = world.client_call(
                "insights.activity_buckets",
                json!({ "boundaries": [day_start, day_end], "project_id": pid_local }),
                Some(&pid_local),
            )?;
            let buckets_scoped: Vec<ActivityBucket> =
                serde_json::from_value(buckets_scoped_res["buckets"].clone())?;
            let expected_scoped = aggregate_window(&segs_scoped, day_start, day_end);
            ensure!(
                expected_scoped.dwell == buckets_scoped[0].dwell
                    && expected_scoped.agent_ms == buckets_scoped[0].agent_ms
                    && expected_scoped.by_project == buckets_scoped[0].by_project
                    && expected_scoped.by_agent == buckets_scoped[0].by_agent
                    && buckets_scoped[0].dwell.card == 90_000
                    && buckets_scoped[0].dwell.terminal == 0,
                "Project-scoped parity mismatch: expected={expected_scoped:?} actual={:?}",
                buckets_scoped[0]
            );

            // Report completion on `cid_views` (produces `execution.reported` with `path` & prose for 10c),
            // finish terminals, then gracefully stop & bounce `local-server` to verify activity persistence!
            let views_card_num = world
                .server("local-server")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .find(|c| c.id == cid_views)
                .unwrap()
                .number;
            let _ = world.client_call(
                "execution.report",
                json!({
                    "project_id": pid_local,
                    "card": views_card_num,
                    "state": "completed",
                    "message": "Long secret report prose",
                    "body": "Long secret report prose body",
                    "request_key": format!("ins-rep-{seed}"),
                }),
                Some(&pid_local),
            )?;
            let expected_persisted_agent_ms = buckets_unscoped[0].agent_ms;
            world.stop_server_graceful("local-server")?;
            world.bounce_server("local-server")?;
            world.heal_and_quiesce(15_000)?;

            // Verify dwell & closed agent segments persisted accurately across server restart!
            let post_bounce_buckets: Vec<ActivityBucket> = serde_json::from_value(
                world.client_call(
                    "insights.activity_buckets",
                    json!({ "boundaries": [day_start, day_end] }),
                    Some(&pid_local),
                )?["buckets"]
                    .clone(),
            )?;
            ensure!(
                post_bounce_buckets[0].dwell == buckets_unscoped[0].dwell
                    && post_bounce_buckets[0].agent_ms == expected_persisted_agent_ms,
                "Activity buckets drifted across graceful restart: before={:?} after={:?}",
                buckets_unscoped[0],
                post_bounce_buckets[0]
            );

            // 10c. `insights.events`: filters kinds, strips `message` and non-whitelisted `data` keys,
            //      and excludes cardless events!
            let views_rev_now = world
                .server("local-server")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .find(|c| c.id == cid_views)
                .unwrap()
                .revision;
            let _ = world.client_call(
                "card.update",
                json!({
                    "card_id": cid_views,
                    "revision": views_rev_now,
                    "title": "Comment Views Card (edited)",
                }),
                Some(&pid_local),
            )?;
            // Emit real cardless GitHub events on `pid_local2` for both 10c (`insights.events`) and 10d (pruning).
            let prune_repo = "acme/prune-sim";
            world.github.add_repo(prune_repo, 6006, "main");
            world.github.upsert_issue(
                prune_repo,
                SimIssue {
                    number: 1,
                    title: "Old cardless 1".into(),
                    body: String::new(),
                    state: "open".into(),
                    author: "alice".into(),
                    labels: Vec::new(),
                    updated_at: "2023-01-01T00:00:00Z".into(),
                    is_pr: false,
                    draft: false,
                    merged: false,
                    head_sha: String::new(),
                    head_ref: String::new(),
                    base_ref: String::new(),
                    check_conclusion: None,
                },
            );
            world.github.upsert_issue(
                prune_repo,
                SimIssue {
                    number: 2,
                    title: "Old cardless pending".into(),
                    body: String::new(),
                    state: "open".into(),
                    author: "alice".into(),
                    labels: Vec::new(),
                    updated_at: "2023-01-01T00:01:00Z".into(),
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
                    "project_id": pid_local2,
                    "repository": prune_repo,
                    "intake": "off",
                    "pr_intake": "off",
                }),
                Some(&pid_local2),
            )?;
            // Register a consumer subscription so `github_event_refresh` polls repo issues -> cardless events!
            let _ = world.client_call(
                "subscription.save",
                json!({
                    "subscription_id": "sub-prune-consumer",
                    "revision": 0,
                    "selector": { "types": ["github.issue.discovered"] },
                    "target": { "type": "consumer" },
                    "enabled": true,
                }),
                Some(&pid_local2),
            )?;
            let _ = world.client_call(
                "github.sync",
                json!({ "project_id": pid_local2 }),
                Some(&pid_local2),
            )?;
            world.wait_until(10_000, |w| {
                let evs = w.server("local-server").unwrap().events()?;
                Ok(evs
                    .iter()
                    .filter(|e| {
                        e.card_id.is_empty()
                            && e.kind == "github.issue.discovered"
                            && e.data["repository"] == prune_repo
                    })
                    .count()
                    >= 2)
            })?;

            let ins_ev_res = world.client_call(
                "insights.events",
                json!({ "project_id": pid_local }),
                Some(&pid_local),
            )?;
            let ins_events: Vec<Event> = serde_json::from_value(ins_ev_res["events"].clone())?;
            ensure!(
                ins_events.iter().all(|e| !e.card_id.is_empty())
                    && ins_events.iter().all(|e| e.message.is_empty())
                    && !ins_events.iter().any(|e| e.kind == "card.updated")
                    && !ins_events
                        .iter()
                        .any(|e| e.kind == "github.issue.discovered"),
                "insights.events leaked cardless/prose/non-insights events"
            );
            let rep_ev = ins_events
                .iter()
                .find(|e| e.card_id == cid_views && e.kind == "execution.reported")
                .context("execution.reported missing from insights.events")?;
            ensure!(
                rep_ev.data["state"] == json!("completed")
                    && rep_ev.data.get("terminal_id").is_some()
                    && rep_ev.data.get("run_number").is_some()
                    && rep_ev.data.get("path").is_none(),
                "insights.events execution.reported data filtering mismatch: {:?}",
                rep_ev.data
            );

            // 10d. Real virtual-time cardless event pruning (`CARDLESS_RETENTION_MS` = 7 days):
            //      preserves journal `highwater`, events with pending `EventDelivery`, recent
            //      cardless events, and card-bound events while pruning consumed old cardless events!
            let (ev_old_id, ev_pending_id, hw_before) = {
                let srv = world.server("local-server").unwrap();
                let evs = srv.events()?;
                let mut old_cardless: Vec<Event> = evs
                    .into_iter()
                    .filter(|e| {
                        e.card_id.is_empty()
                            && e.kind == "github.issue.discovered"
                            && e.data["repository"] == prune_repo
                    })
                    .collect();
                old_cardless.sort_by_key(|e| e.data["number"].as_u64().unwrap_or(0));
                ensure!(old_cardless.len() == 2);
                let old_id = old_cardless[0].id.clone();
                let pend_id = old_cardless[1].id.clone();
                // Wait until consumer deliveries exist for both, ack ONLY `old_id`, leaving
                // `pend_id`'s consumer delivery `pending`!
                let ev_list = srv.call("event.list", json!({ "after": 0, "limit": 500 }))?;
                let hw = ev_list["highwater"].as_u64().unwrap();
                (old_id, pend_id, hw)
            };
            // Wait for consumer deliveries to be created, then ack `ev_old_id` so ONLY `ev_pending_id` stays pending.
            world.wait_until(5_000, |w| {
                let delivs: Vec<EventDelivery> =
                    serde_json::from_value(w.server("local-server").unwrap().call(
                        "subscription.deliveries",
                        json!({ "subscription_id": "sub-prune-consumer" }),
                    )?)?;
                Ok(delivs.iter().any(|d| d.event_id == ev_old_id)
                    && delivs.iter().any(|d| d.event_id == ev_pending_id))
            })?;
            let old_deliv_id = format!("sub-prune-consumer:{ev_old_id}");
            let _ = world.client_call(
                "subscription.ack",
                json!({ "delivery_id": old_deliv_id }),
                Some(&pid_local2),
            )?;

            // Advance `local-server` virtual clock by 8 days (> 7d `CARDLESS_RETENTION_MS`) and emit
            // a Herdr heartbeat + a brand-new recent cardless event #3 so `pump_at_with_prune_state` runs!
            world.servers["local-server"]
                .clock
                .jump_ms(8 * 24 * 60 * 60 * 1_000);
            world.herdr("local-server").unwrap().emit_heartbeat();
            world.github.upsert_issue(
                prune_repo,
                SimIssue {
                    number: 3,
                    title: "Recent cardless 3".into(),
                    body: String::new(),
                    state: "open".into(),
                    author: "alice".into(),
                    labels: Vec::new(),
                    updated_at: "2026-06-01T00:10:00Z".into(),
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
                "github.sync",
                json!({ "project_id": pid_local2 }),
                Some(&pid_local2),
            )?;
            world.wait_until(10_000, |w| {
                let evs = w.server("local-server").unwrap().events()?;
                Ok(!evs.iter().any(|e| e.id == ev_old_id)
                    && evs.iter().any(|e| {
                        e.card_id.is_empty()
                            && e.data["repository"] == prune_repo
                            && e.data["number"] == 3
                    }))
            })?;
            {
                let srv = world.server("local-server").unwrap();
                let hw_after =
                    srv.call("event.list", json!({ "after": 0, "limit": 1 }))?["highwater"]
                        .as_u64()
                        .unwrap();
                ensure!(
                    hw_after >= hw_before,
                    "Journal highwater decreased after cardless pruning: {hw_after} < {hw_before}"
                );
                let remaining = srv.events()?;
                ensure!(
                    !remaining.iter().any(|e| e.id == ev_old_id),
                    "Consumed old cardless event must be pruned"
                );
                ensure!(
                    remaining.iter().any(|e| e.id == ev_pending_id),
                    "Old cardless event with pending delivery must NOT be pruned"
                );
                ensure!(
                    remaining.iter().any(|e| {
                        e.card_id.is_empty()
                            && e.data["repository"] == prune_repo
                            && e.data["number"] == 3
                    }),
                    "Recent cardless event must NOT be pruned"
                );
                ensure!(
                    remaining.iter().any(|e| e.card_id == cid_views),
                    "Card-bound events must NEVER be pruned by cardless pruning"
                );
            }
            // Ack the remaining pending consumer delivery and close manual shell cleanly.
            let _ = world.client_call(
                "subscription.ack",
                json!({ "delivery_id": format!("sub-prune-consumer:{ev_pending_id}") }),
                Some(&pid_local2),
            )?;
            let _ = world.client_call(
                "terminal.close",
                json!({ "terminal_id": id_codex }),
                Some(&pid_local2),
            )?;
            world
                .herdr("local-server")
                .unwrap()
                .finish_terminal(&open_addr, 0, "done");

            world.hit("client_pages.insights_and_activity");
        }

        // 13. Remote `last_project` desktop preference persistence & deferred UI restore.
        {
            // Both unqualified remote IDs (non-colliding) and qualified remote IDs (colliding with
            // local) must be accepted by the local AppServer.
            let saved_remote_unqualified = world.server_call(
                "local-server",
                "utilities.configure",
                json!({ "last_project": pid_a }),
            )?;
            ensure!(
                saved_remote_unqualified["preferences"]["last_project"].as_str()
                    == Some(pid_a.as_str()),
                "local AppServer must accept non-colliding remote last_project IDs: {saved_remote_unqualified}"
            );
            let qualified_remote_pid = canban_types::qualify_host_id("remote-a", &pid_local);
            let saved_remote_qualified = world.server_call(
                "local-server",
                "utilities.configure",
                json!({ "last_project": qualified_remote_pid }),
            )?;
            ensure!(
                saved_remote_qualified["preferences"]["last_project"].as_str()
                    == Some(qualified_remote_pid.as_str()),
                "local AppServer must accept qualified remote last_project IDs: {saved_remote_qualified}"
            );
            let fetched_state = world.server_call("local-server", "utilities.state", json!({}))?;
            ensure!(
                fetched_state["preferences"]["last_project"].as_str()
                    == Some(qualified_remote_pid.as_str()),
                "utilities.state must reflect saved remote last_project: {fetched_state}"
            );

            // Reject empty, control-char, oversized, and malformed `host::` IDs.
            let oversized = format!("host::remote-a::{}", "x".repeat(520));
            for invalid in [
                "",
                "bad\nid",
                "host::no-separator",
                "host::local::abc",
                "host::::abc",
                "host::remote-a::",
                "host::remote-a::host::remote-b::abc",
                "host::remote-a::bad\nid",
                oversized.as_str(),
            ] {
                let err = world.server_call(
                    "local-server",
                    "utilities.configure",
                    json!({ "last_project": invalid }),
                );
                ensure!(
                    err.is_err(),
                    "utilities.configure must reject invalid last_project {invalid:?}"
                );
            }

            // Local project ID and null clearing remain valid, then restore remote preference.
            let saved_local = world.server_call(
                "local-server",
                "utilities.configure",
                json!({ "last_project": pid_local }),
            )?;
            ensure!(
                saved_local["preferences"]["last_project"].as_str() == Some(pid_local.as_str()),
                "local last_project must still save: {saved_local}"
            );
            let cleared = world.server_call(
                "local-server",
                "utilities.configure",
                json!({ "last_project": Value::Null }),
            )?;
            ensure!(
                cleared["preferences"]["last_project"].is_null(),
                "null last_project must clear preference: {cleared}"
            );
            let re_saved = world.server_call(
                "local-server",
                "utilities.configure",
                json!({ "last_project": pid_a }),
            )?;
            ensure!(
                re_saved["preferences"]["last_project"].as_str() == Some(pid_a.as_str()),
                "re-saving remote last_project must succeed: {re_saved}"
            );

            // Deferred UI restore: startup sees only local projects, waits while remote-a is
            // Connecting, and restores pid_a once remote-a connects with its merged snapshot.
            let merged_snap = world.client_merged_snapshot()?;
            let live_hosts = world.client_host_summaries();
            let local_only_projects: Vec<_> = merged_snap
                .projects
                .iter()
                .filter(|p| merged_snap.project_host(&p.id) == "local")
                .cloned()
                .collect();
            let fallback = restore_project_selection(&local_only_projects, Some(&pid_a))
                .context("local fallback project expected")?;
            ensure!(fallback != pid_a);
            let mut pending = PendingRemoteRestore::from_bootstrap(
                &local_only_projects,
                Some(&pid_a),
                Some(&fallback),
            );
            ensure!(
                pending.is_some(),
                "startup with missing remote saved_project must arm PendingRemoteRestore"
            );

            let connecting_hosts: Vec<_> = live_hosts
                .iter()
                .cloned()
                .map(|mut h| {
                    if h.id == "remote-a" {
                        h.connection = HostConnectionState::Connecting;
                    }
                    h
                })
                .collect();
            ensure!(
                PendingRemoteRestore::resolve(
                    &mut pending,
                    &fallback,
                    &local_only_projects,
                    &connecting_hosts,
                )
                .is_none()
                    && pending.is_some(),
                "PendingRemoteRestore must wait while remote host is still Connecting"
            );

            let restored_remote = PendingRemoteRestore::resolve(
                &mut pending,
                &fallback,
                &merged_snap.projects,
                &live_hosts,
            );
            ensure!(
                restored_remote.as_deref() == Some(pid_a.as_str()) && pending.is_none(),
                "PendingRemoteRestore must restore remote project once merged: {restored_remote:?}"
            );

            // Manual user switch before remote connects cancels deferred restore.
            let mut pending_manual = PendingRemoteRestore::from_bootstrap(
                &local_only_projects,
                Some(&pid_a),
                Some(&fallback),
            );
            ensure!(
                PendingRemoteRestore::resolve(
                    &mut pending_manual,
                    "user-picked-other",
                    &merged_snap.projects,
                    &live_hosts,
                )
                .is_none()
                    && pending_manual.is_none(),
                "manual user selection must cancel PendingRemoteRestore"
            );

            // Connected remote host missing a deleted project clears PendingRemoteRestore cleanly.
            let mut pending_deleted = PendingRemoteRestore::from_bootstrap(
                &local_only_projects,
                Some("host::remote-a::deleted-proj"),
                Some(&fallback),
            );
            ensure!(
                PendingRemoteRestore::resolve(
                    &mut pending_deleted,
                    &fallback,
                    &merged_snap.projects,
                    &live_hosts,
                )
                .is_none()
                    && pending_deleted.is_none(),
                "connected remote host without saved project must clear PendingRemoteRestore"
            );

            // Cross-host archive blocker scoping: a remote configuration/merge/scheduled terminal
            // sharing the same raw project ID as a local project must ONLY block the remote host's
            // qualified project, never the local project.
            {
                let mut collision_snap = merged_snap.clone();
                let shared_raw_pid = pid_local.clone();
                let remote_shared_pid = canban_types::qualify_host_id("remote-a", &shared_raw_pid);
                if let Some(mut local_proj) = collision_snap
                    .projects
                    .iter()
                    .find(|p| p.id == shared_raw_pid)
                    .cloned()
                {
                    local_proj.id = remote_shared_pid.clone();
                    collision_snap.projects.push(local_proj);
                    collision_snap
                        .entity_hosts
                        .insert(remote_shared_pid.clone(), "remote-a".into());
                }
                let remote_cfg_tid = format!("configuration:{shared_raw_pid}:sess-1");
                let remote_term: canban_types::TerminalEntry = serde_json::from_value(json!({
                    "id": remote_cfg_tid,
                    "conversation_id": null,
                    "number": 1,
                    "name": "Remote Config",
                    "runtime": {
                        "state": "running",
                        "connection": "connected",
                        "terminal": "term-remote-cfg"
                    },
                    "created_at": 1
                }))?;
                collision_snap.terminals.push(remote_term);
                collision_snap
                    .entity_hosts
                    .insert(remote_cfg_tid, "remote-a".into());
                let baseline_local =
                    canban_ui::projects::active_blockers(&merged_snap, &shared_raw_pid);
                let local_blockers =
                    canban_ui::projects::active_blockers(&collision_snap, &shared_raw_pid);
                let remote_blockers =
                    canban_ui::projects::active_blockers(&collision_snap, &remote_shared_pid);
                ensure!(
                    local_blockers.live_terminals == baseline_local.live_terminals,
                    "remote terminal with colliding raw project ID must not block local archive: before={baseline_local:?} after={local_blockers:?}"
                );
                ensure!(
                    remote_blockers.live_terminals == 1,
                    "remote terminal must block its own remote project archive: {remote_blockers:?}"
                );
            }

            world.hit("client_pages.remote_last_project_preference");
        }

        // 14. Cross-host entity scoping: unified shared-ID remapping across terminals/sessions/
        //     batches/runs, `human_gate.terminal_id` & `event_subscriptions` remapping, qualified
        //     special-session recognition, workflow config lookup, column automation isolation,
        //     and `entries()` worktree host scoping.
        {
            let wf_snap = world.client_merged_snapshot()?;
            let base_proj = wf_snap
                .projects
                .first()
                .context("project expected in merged snapshot")?;
            let base_card = wf_snap
                .cards
                .first()
                .context("card expected in merged snapshot")?;
            let mut local_snap = Snapshot::default();
            let mut local_proj = base_proj.clone();
            local_proj.id = "p-shared".into();
            local_proj.name = "Shared Proj".into();
            local_proj.directory = "/shared/proj".into();
            local_snap.projects.push(local_proj);
            let mut local_card = base_card.clone();
            local_card.id = "c-shared".into();
            local_card.project_id = "p-shared".into();
            local_card.number = 1;
            local_card.title = "Local Shared Card".into();
            local_card.stage = "develop".into();
            local_card.human_gate = None;
            local_snap.cards.push(local_card);
            local_snap.worktrees.push(canban_types::ManagedWorktree {
                id: "wt-local".into(),
                project_id: "p-shared".into(),
                branch: "feat/local".into(),
                base_branch: "main".into(),
                base_commit: None,
                path: "/shared/wt".into(),
                directory: "/shared/proj".into(),
                created_at: 1,
            });
            local_snap.terminals.push(serde_json::from_value(json!({
                "id": "term-shared",
                "conversation_id": null,
                "number": 1,
                "name": "Local Term",
                "runtime": { "state": "running", "connection": "connected", "terminal": "pty-l1" },
                "created_at": 1
            }))?);
            local_snap
                .configuration_sessions
                .push(canban_types::ConfigurationSession {
                    id: "configuration:p-shared:workflows:claude".into(),
                    project_id: "p-shared".into(),
                    profile_id: "claude".into(),
                    provider_session_id: None,
                    generation: 1,
                    state: "finished".into(),
                    prompt: "local".into(),
                    error: None,
                });
            local_snap.merge_batches.push(canban_types::MergeBatch {
                id: "merge-queue:p-shared:b1".into(),
                project_id: "p-shared".into(),
                profile_id: "claude".into(),
                provider_session_id: None,
                target_branch: "main".into(),
                target_head: "abc".into(),
                strategy: Default::default(),
                sync_upstream: false,
                test_command: None,
                worktree: "/shared/wt-merge".into(),
                state: "completed".into(),
                failure_kind: None,
                conflict_card_number: None,
                conflict_files: Vec::new(),
                test_output: None,
                items: Vec::new(),
                prompt: String::new(),
                error: None,
                queued_at: None,
                started_at: None,
                created_at: 1,
                updated_at: 1,
            });
            let base_src = wf_snap
                .workflow_sources
                .iter()
                .find(|ws| ws.filename == "development.yaml")
                .context("default development.yaml workflow source expected")?;
            let base_rev = wf_snap
                .workflow_revisions
                .iter()
                .find(|wr| wr.id == base_src.revision_id)
                .context("default development.yaml workflow revision expected")?;
            let mut local_src = base_src.clone();
            local_src.host_id = "local".into();
            local_src.id = "ws-shared".into();
            local_src.project_id = "p-shared".into();
            local_src.workflow_id = "development".into();
            local_src.revision = 1;
            local_src.revision_id = "wr-shared".into();
            local_src.enabled = true;
            let mut local_rev = base_rev.clone();
            local_rev.host_id = "local".into();
            local_rev.id = "wr-shared".into();
            local_rev.project_id = "p-shared".into();
            local_rev.workflow_id = "development".into();
            local_rev.revision = 1;
            local_rev.definition.name = "Local Flow".into();
            local_snap.workflow_sources.push(local_src);
            local_snap.workflow_revisions.push(local_rev);
            local_snap
                .event_subscriptions
                .push(serde_json::from_value(json!({
                    "id": "sub-shared",
                    "revision": 1,
                    "selector": { "project_id": "p-shared", "column": "develop", "types": ["card.updated"] },
                    "target": { "type": "workflow", "workflow_id": "development", "entry": "develop", "card_id": "c-shared" },
                    "enabled": true,
                    "cursor": 0,
                    "created_at": 1
                }))?);
            local_snap.register_host_entities("local");

            let mut remote_snap = Snapshot::default();
            let mut remote_proj = base_proj.clone();
            remote_proj.id = "p-shared".into();
            remote_proj.name = "Shared Proj".into();
            remote_proj.directory = "/shared/proj".into();
            remote_snap.projects.push(remote_proj);
            let mut remote_card_src = base_card.clone();
            remote_card_src.id = "c-shared".into();
            remote_card_src.project_id = "p-shared".into();
            remote_card_src.number = 1;
            remote_card_src.title = "Remote Shared Card".into();
            remote_card_src.stage = "develop".into();
            remote_card_src.human_gate = Some(HumanGate {
                reason: "Approve".into(),
                next_stage: Some("review".into()),
                terminal_id: Some("term-shared".into()),
            });
            remote_snap.cards.push(remote_card_src);
            remote_snap.terminals.push(serde_json::from_value(json!({
                "id": "term-shared",
                "conversation_id": null,
                "number": 1,
                "name": "Remote Shared Term",
                "runtime": { "state": "running", "connection": "connected", "terminal": "pty-r1" },
                "created_at": 2
            }))?);
            // Remote special sessions whose IDs collide with local session/batch rows (not terminals)
            // plus a remote __global scheduled run + matching terminal.
            remote_snap
                .configuration_sessions
                .push(canban_types::ConfigurationSession {
                    id: "configuration:p-shared:workflows:claude".into(),
                    project_id: "p-shared".into(),
                    profile_id: "claude".into(),
                    provider_session_id: None,
                    generation: 2,
                    state: "running".into(),
                    prompt: "remote".into(),
                    error: None,
                });
            remote_snap.terminals.push(serde_json::from_value(json!({
                "id": "configuration:p-shared:workflows:claude",
                "conversation_id": null,
                "number": 2,
                "name": "Remote Config",
                "runtime": { "state": "running", "connection": "connected", "terminal": "pty-rcfg" },
                "created_at": 2
            }))?);
            remote_snap.merge_batches.push(canban_types::MergeBatch {
                id: "merge-queue:p-shared:b1".into(),
                project_id: "p-shared".into(),
                profile_id: "claude".into(),
                provider_session_id: None,
                target_branch: "main".into(),
                target_head: "def".into(),
                strategy: Default::default(),
                sync_upstream: false,
                test_command: None,
                worktree: "/shared/wt-merge".into(),
                state: "running".into(),
                failure_kind: None,
                conflict_card_number: None,
                conflict_files: Vec::new(),
                test_output: None,
                items: Vec::new(),
                prompt: String::new(),
                error: None,
                queued_at: None,
                started_at: None,
                created_at: 2,
                updated_at: 2,
            });
            remote_snap.terminals.push(serde_json::from_value(json!({
                "id": "merge-queue:p-shared:b1",
                "conversation_id": null,
                "number": 3,
                "name": "Remote Merge",
                "runtime": { "state": "running", "connection": "connected", "terminal": "pty-rmb" },
                "created_at": 2
            }))?);
            remote_snap.scheduled_runs.push(canban_types::ScheduledRun {
                host_id: String::new(),
                id: "scheduled:__global:r1".into(),
                schedule_id: "schedule:__global:wf:r1".into(),
                project_id: "__global".into(),
                workflow_id: "global:wf".into(),
                workflow_name: "Remote Global".into(),
                filename: "gc.yaml".into(),
                rule_id: "schedule".into(),
                cron: "0 * * * *".into(),
                number: 1,
                trigger_kind: "cron".into(),
                creates_card: false,
                card_id: None,
                card_number: None,
                workflow_run_id: None,
                start_step: None,
                current_step: None,
                profile_id: None,
                provider_session_id: None,
                terminal_id: Some("scheduled:__global:r1".into()),
                cwd: "/shared/proj".into(),
                state: "running".into(),
                summary: None,
                report_path: None,
                output_path: None,
                error: None,
                steps: Vec::new(),
                scheduled_at: 2,
                started_at: None,
                completed_at: None,
                close_requested_at: None,
                updated_at: 2,
            });
            remote_snap.terminals.push(serde_json::from_value(json!({
                "id": "scheduled:__global:r1",
                "conversation_id": null,
                "number": 4,
                "name": "Remote Global Sched",
                "runtime": { "state": "running", "connection": "connected", "terminal": "pty-rsched" },
                "created_at": 2
            }))?);
            let mut remote_src = base_src.clone();
            remote_src.host_id = "remote-a".into();
            remote_src.id = "ws-shared".into();
            remote_src.project_id = "p-shared".into();
            remote_src.workflow_id = "development".into();
            remote_src.revision = 1;
            remote_src.revision_id = "wr-shared".into();
            remote_src.enabled = true;
            let mut remote_rev = base_rev.clone();
            remote_rev.host_id = "remote-a".into();
            remote_rev.id = "wr-shared".into();
            remote_rev.project_id = "p-shared".into();
            remote_rev.workflow_id = "development".into();
            remote_rev.revision = 1;
            remote_rev.definition.name = "Remote Flow".into();
            remote_snap.workflow_sources.push(remote_src);
            remote_snap.workflow_revisions.push(remote_rev);
            remote_snap
                .event_subscriptions
                .push(serde_json::from_value(json!({
                    "id": "sub-shared",
                    "revision": 1,
                    "selector": { "project_id": "p-shared", "card_id": "c-shared", "column": "develop", "types": ["remote.custom_event"] },
                    "target": { "type": "workflow", "workflow_id": "development", "entry": "develop", "card_id": "c-shared" },
                    "enabled": true,
                    "cursor": 0,
                    "created_at": 2
                }))?);

            let remote_summary = world
                .client_host_summaries()
                .into_iter()
                .find(|h| h.id == "remote-a")
                .context("remote-a host summary expected")?;
            local_snap.merge_remote_snapshot(remote_snap, remote_summary);

            // 14a. Shared-ID alignment across terminals / sessions / merge batches / global runs.
            let q_cfg = "host::remote-a::configuration:p-shared:workflows:claude";
            let q_mb = "host::remote-a::merge-queue:p-shared:b1";
            let q_sched = "host::remote-a::scheduled:__global:r1";
            ensure!(
                local_snap
                    .configuration_sessions
                    .iter()
                    .any(|c| c.id == q_cfg)
                    && local_snap.terminals.iter().any(|t| t.id == q_cfg),
                "remote configuration session and its terminal must stay ID-aligned"
            );
            ensure!(
                local_snap.merge_batches.iter().any(|b| b.id == q_mb)
                    && local_snap.terminals.iter().any(|t| t.id == q_mb),
                "remote merge batch and its terminal must stay ID-aligned"
            );
            ensure!(
                local_snap.scheduled_runs.iter().any(|r| r.id == q_sched)
                    && local_snap.terminals.iter().any(|t| t.id == q_sched),
                "remote __global scheduled run and its terminal must stay ID-aligned"
            );

            // 14b. Card human_gate.terminal_id and event_subscriptions remapping.
            let remote_card = local_snap
                .cards
                .iter()
                .find(|c| c.id == "host::remote-a::c-shared")
                .context("qualified remote card expected")?;
            ensure!(
                remote_card
                    .human_gate
                    .as_ref()
                    .and_then(|g| g.terminal_id.as_deref())
                    == Some("host::remote-a::term-shared"),
                "remote card human_gate.terminal_id must be remapped on collision: {:?}",
                remote_card.human_gate
            );
            let remote_sub = local_snap
                .event_subscriptions
                .iter()
                .find(|sub| sub.id == "host::remote-a::sub-shared")
                .context("qualified remote event_subscription expected")?;
            ensure!(
                remote_sub.selector.project_id.as_deref() == Some("host::remote-a::p-shared")
                    && remote_sub.selector.card_id.as_deref() == Some("host::remote-a::c-shared")
                    && local_snap.host_of(&remote_sub.id) == "remote-a",
                "remote event_subscription refs/host must be remapped: {remote_sub:?}"
            );

            // 14c. Qualified special-session & automated-terminal recognition.
            for (tid, expected_kind) in [
                (q_cfg, canban_types::ManagedTerminalKind::Configuration),
                (q_mb, canban_types::ManagedTerminalKind::MergeQueue),
                (q_sched, canban_types::ManagedTerminalKind::Scheduled),
            ] {
                let term = local_snap
                    .terminals
                    .iter()
                    .find(|t| t.id == tid)
                    .context("qualified special terminal expected")?;
                ensure!(
                    term.is_special_session()
                        && term.managed_kind() == Some(expected_kind)
                        && canban_ui::activity::is_automated_terminal(&local_snap, tid),
                    "qualified special terminal {tid} must be recognized as {expected_kind:?}"
                );
            }

            // 14d. Workflow configuration session lookup by host/project.
            let local_cfg = canban_ui::workflows::find_workflow_configuration_session(
                &local_snap,
                "p-shared",
                "claude",
            )
            .context("local workflow config session expected")?;
            let remote_cfg = canban_ui::workflows::find_workflow_configuration_session(
                &local_snap,
                "host::remote-a::p-shared",
                "claude",
            )
            .context("remote workflow config session expected")?;
            ensure!(
                local_cfg.state == "finished" && remote_cfg.state == "running",
                "workflow config session lookup must isolate local vs remote colliding projects"
            );

            // 14e. Column automation isolation across hosts with identical workflow_id/revision.
            let local_auto =
                canban_ui::workflows::project_column_automation(&local_snap, "p-shared", "develop")
                    .context("local develop automation expected")?;
            let remote_auto = canban_ui::workflows::project_column_automation(
                &local_snap,
                "host::remote-a::p-shared",
                "develop",
            )
            .context("remote develop automation expected")?;
            ensure!(
                local_auto.tooltip.contains("Local Flow")
                    && !local_auto.tooltip.contains("Remote Flow")
                    && !local_auto.tooltip.contains("remote.custom_event"),
                "local column automation leaked remote workflow/subscription: {}",
                local_auto.tooltip
            );
            ensure!(
                remote_auto.tooltip.contains("Remote Flow")
                    && !remote_auto.tooltip.contains("Local Flow")
                    && remote_auto.tooltip.contains("remote.custom_event"),
                "remote column automation did not use remote workflow/subscription: {}",
                remote_auto.tooltip
            );

            // 14f. `entries()` must not match a remote terminal's cwd against a local managed worktree
            //      even when the remote terminal ID is not yet in `entity_hosts`.
            let optimistic_remote_tid = "shell-remote-optimistic".to_owned();
            local_snap.terminals.push(serde_json::from_value(json!({
                "id": optimistic_remote_tid,
                "conversation_id": null,
                "number": 99,
                "name": "Remote Optimistic",
                "runtime": { "state": "running", "connection": "connected", "terminal": "pty-opt" },
                "created_at": 3
            }))?);
            local_snap.terminal_contexts.push(TerminalContext {
                terminal_id: optimistic_remote_tid.clone(),
                core_id: "core-remote".into(),
                terminal: "pty-opt".into(),
                cwd: Some("/shared/wt".into()),
                directory_root: Some("/shared/wt".into()),
                project_id: Some("host::remote-a::p-shared".into()),
                card_id: Some("host::remote-a::c-shared".into()),
                ..Default::default()
            });
            let entries_now = canban_ui::terminals::entries(&local_snap);
            let opt_entry = entries_now
                .iter()
                .find(|e| e.saved.id == optimistic_remote_tid)
                .context("optimistic remote terminal entry expected")?;
            ensure!(
                opt_entry.host_id == "remote-a"
                    && opt_entry.card.as_ref().map(|c| c.id.as_str())
                        == Some("host::remote-a::c-shared"),
                "remote terminal cwd matching local worktree path must not drop remote card association: host={} card={:?}",
                opt_entry.host_id,
                opt_entry.card.as_ref().map(|c| &c.id)
            );

            world.hit("client_pages.cross_host_entity_scoping");
        }

        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })?;

    // Phase B: Mobile mode (`mobile_default` — remote-a + remote-b only, no local server).
    let mobile_cfg =
        SimWorldConfig::mobile_default(seed.wrapping_add(0x9E37_79B9), "client_pages_mobile");
    let mobile_digest = run_with_world(mobile_cfg, |world| {
        let mut oracle = InvariantOracle::default();

        ensure!(
            world.server("local-server").is_none(),
            "Mobile topology must have no local host"
        );
        let dir_a = world.project_dir("mob-a")?;
        let dir_b = world.project_dir("mob-b")?;
        let pa = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Mobile A", "directory": dir_a }),
            None,
        )?;
        let pb = world.client_call(
            "project.create",
            json!({ "host_id": "remote-b", "name": "Mobile B", "directory": dir_b }),
            None,
        )?;
        let pid_a = pa["id"].as_str().unwrap().to_owned();
        let pid_b = pb["id"].as_str().unwrap().to_owned();

        let _ = world.client_call(
            "card.create",
            json!({ "project_id": pid_a, "title": "Mobile Card A" }),
            Some(&pid_a),
        )?;
        let _ = world.client_call(
            "card.create",
            json!({ "project_id": pid_b, "title": "Mobile Card B" }),
            Some(&pid_b),
        )?;
        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        let list_a = world.client_call(
            "card.list",
            json!({ "project_id": pid_a, "limit": 10 }),
            Some(&pid_a),
        )?;
        ensure!(list_a["items"].as_array().unwrap().len() == 1);
        ensure!(world.client_merged_snapshot()?.cards.len() == 2);
        world.hit("client_pages.mobile_mode");

        Ok(finalize_digest(world))
    })?;

    Ok(desktop_digest ^ mobile_digest.rotate_left(17))
}
