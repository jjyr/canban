//! Scenario #4 — Page data fetching & mutations at the `canban-client` / UI boundary:
//! write receipt hydration, multi-host routing, colliding raw IDs across hosts,
//! `card.list` & `timeline.list` cursor pagination + `cursor_stale` detection,
//! offline read-cache + pending comment merge, and Desktop + Mobile topology parity.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    // Phase A: Desktop mode (local + remote-a + remote-b).
    let desktop_cfg = SimWorldConfig::desktop_default(seed, "client_pages");
    let desktop_digest = run_with_world(desktop_cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let dir_local = world.project_dir("local")?;
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

        // 1. Online write receipt hydration: immediate host routing without waiting on resync.
        let c_a = world.client_call(
            "card.create",
            json!({ "project_id": pid_a, "title": "Page Card A", "body": "initial" }),
            Some(&pid_a),
        )?;
        let cid_a = c_a["id"].as_str().unwrap().to_owned();
        ensure!(c_a["revision"].as_u64().unwrap_or(0) >= 1);
        let routed =
            world
                .client()
                .resolve_target_host("card.update", &json!({ "card_id": cid_a }), None);
        ensure!(routed == "remote-a", "Got routed={routed:?}");

        let c_b = world.client_call(
            "card.create",
            json!({ "project_id": pid_b, "title": "Page Card B", "body": "initial" }),
            Some(&pid_b),
        )?;
        let cid_b = c_b["id"].as_str().unwrap().to_owned();
        let c_local = world.client_call(
            "card.create",
            json!({ "project_id": pid_local, "title": "Page Card Local", "body": "initial" }),
            Some(&pid_local),
        )?;
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
            json!({ "card_id": cid_a, "tail": true, "limit": 200, "view": "full" }),
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

        // Local & remote-b remain operational while remote-a is partitioned.
        let local_rev = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_local)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid_local, "revision": local_rev, "stage": "review" }),
            Some(&pid_local),
        )?;
        let b_rev = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_b)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.update",
            json!({ "card_id": cid_b, "revision": b_rev, "title": "Page Card B Updated" }),
            Some(&pid_b),
        )?;

        oracle.check_step_invariants(world)?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

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
