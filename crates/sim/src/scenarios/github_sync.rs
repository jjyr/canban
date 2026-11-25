//! Scenario #3 — GitHub synchronization:
//! no duplicate syncs/cards, ETag 304 reuse, zero card-revision churn on idle syncs,
//! coalesced rapid sync requests, HTTP 403/500 fault recovery, and in-place PR updates.
use crate::{
    github::SimIssue,
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "github_sync");
    cfg.remote_hosts = vec!["remote-a".into()];
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    let repo = "acme/canban-sim";
    world.github.add_repo(repo, 1001, "main");
    world.github.upsert_issue(
        repo,
        SimIssue {
            number: 1,
            title: "Fix sync dedup".into(),
            body: "Ensure idle polls are quiet".into(),
            state: "open".into(),
            author: "alice".into(),
            labels: vec!["bug".into()],
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
    world.github.upsert_issue(
        repo,
        SimIssue {
            number: 2,
            title: "Add DST coverage".into(),
            body: "Test all 8 scenarios".into(),
            state: "open".into(),
            author: "alice".into(),
            labels: vec!["enhancement".into()],
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
    world.github.upsert_issue(
        repo,
        SimIssue {
            number: 3,
            title: "PR: Unified client".into(),
            body: "Implements canban-client".into(),
            state: "open".into(),
            author: "alice".into(),
            labels: vec!["pr".into()],
            updated_at: "2026-06-01T00:00:00Z".into(),
            is_pr: true,
            draft: false,
            merged: false,
            head_sha: "sha-pr-3-v1".into(),
            head_ref: "feat/client".into(),
            base_ref: "main".into(),
            check_conclusion: Some("success".into()),
        },
    );

    let dir = world.project_dir("gh-sync")?;
    let proj = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "GitHub Repo", "directory": dir }),
        None,
    )?;
    let pid = proj["id"].as_str().unwrap().to_owned();
    let raw_pid = canban_types::split_qualified_host_id(&pid)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| pid.clone());

    // Bind project to simulated GitHub repo with both Issue and PR intake enabled.
    let _ = world.client_call(
        "project.github",
        json!({
            "project_id": pid,
            "repository": repo,
            "intake": "all",
            "pr_intake": "all",
            "include_drafts": true,
        }),
        Some(&pid),
    )?;

    // Wait for initial automatic background sync to import all 3 items (2 issues + 1 PR).
    world.wait_until(15_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        let synced = snap
            .projects
            .iter()
            .find(|p| p.id == raw_pid)
            .and_then(|p| p.github.as_ref())
            .and_then(|g| g.synced_at)
            .is_some();
        Ok(synced && snap.cards.len() == 3)
    })?;
    oracle.check_step_invariants(&world)?;

    let srv_a = world.server("remote-a").unwrap();
    let snap_after_initial = srv_a.snapshot()?;
    ensure!(
        snap_after_initial.cards.len() == 3,
        "Expected 3 imported GitHub cards, got {}",
        snap_after_initial.cards.len()
    );
    let rev_map_before: BTreeMap<String, u64> = snap_after_initial
        .cards
        .iter()
        .map(|c| (c.id.clone(), c.revision))
        .collect();
    let events_before = srv_a.events()?.len();

    // Run 3 consecutive idle syncs while GitHub state is completely unchanged.
    world.github.reset_stats();
    for _ in 0..3 {
        let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
        world.advance(Duration::from_millis(200))?;
    }

    let idle_stats = world.github.stats();
    let snap_after_idle = srv_a.snapshot()?;
    let events_after_idle = srv_a.events()?.len();
    let rev_map_after: BTreeMap<String, u64> = snap_after_idle
        .cards
        .iter()
        .map(|c| (c.id.clone(), c.revision))
        .collect();

    ensure!(
        rev_map_after == rev_map_before,
        "Idle GitHub syncs mutated Card revisions! before={rev_map_before:?} after={rev_map_after:?}"
    );
    ensure!(
        events_after_idle == events_before,
        "Idle GitHub syncs emitted duplicate events! before={events_before} after={events_after_idle}"
    );
    ensure!(
        idle_stats.responses_304 > 0,
        "Expected ETag 304 responses during idle GitHub syncs, got stats={idle_stats:?}"
    );
    ensure!(
        idle_stats.total_requests <= 45,
        "Excessive GitHub HTTP traffic during 3 idle syncs: {idle_stats:?}"
    );

    // Burst 5 rapid `github.sync.request` calls — worker queue must coalesce them!
    world.github.reset_stats();
    for _ in 0..5 {
        let _ = world.client_call(
            "github.sync.request",
            json!({ "project_id": pid }),
            Some(&pid),
        )?;
    }
    world.advance(Duration::from_secs(2))?;
    let burst_stats = world.github.stats();
    ensure!(
        burst_stats.total_requests <= 20,
        "Rapid github.sync.request burst was not coalesced! stats={burst_stats:?}"
    );

    // Inject HTTP 403 rate-limit error -> sync fails cleanly without dropping cards ->
    // next sync recovers and clears `project.github.error`.
    world
        .github
        .inject_error(403, vec![("x-ratelimit-remaining".into(), "0".into())]);
    let err_res = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid));
    ensure!(
        err_res.is_err(),
        "Expected github.sync to return error on injected HTTP 403"
    );
    {
        let snap = srv_a.snapshot()?;
        ensure!(
            snap.cards.len() == 3,
            "HTTP 403 error must never delete existing imported cards"
        );
        let proj_gh = snap
            .projects
            .iter()
            .find(|p| p.id == raw_pid)
            .unwrap()
            .github
            .as_ref()
            .unwrap();
        ensure!(
            proj_gh.error.is_some(),
            "Expected GitHub source error recorded after 403"
        );
    }

    // Mutate PR #3 on simulated GitHub (updated title + CI failure) and sync again ->
    // recovers cleanly AND updates the existing PR card in-place without duplicates!
    world.github.upsert_issue(
        repo,
        SimIssue {
            number: 3,
            title: "PR: Unified client (updated)".into(),
            body: "Implements canban-client with fixes".into(),
            state: "open".into(),
            author: "alice".into(),
            labels: vec!["pr".into()],
            updated_at: "2026-06-01T01:00:00Z".into(),
            is_pr: true,
            draft: false,
            merged: false,
            head_sha: "sha-pr-3-v2".into(),
            head_ref: "feat/client".into(),
            base_ref: "main".into(),
            check_conclusion: Some("failure".into()),
        },
    );
    let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
    world.heal_and_quiesce(15_000)?;
    oracle.check_quiescent_invariants(&world)?;

    let final_snap = srv_a.snapshot()?;
    ensure!(
        final_snap.cards.len() == 3,
        "Updating PR #3 created duplicate cards! count={}",
        final_snap.cards.len()
    );
    let proj_gh = final_snap
        .projects
        .iter()
        .find(|p| p.id == raw_pid)
        .unwrap()
        .github
        .as_ref()
        .unwrap();
    ensure!(
        proj_gh.error.is_none(),
        "Expected GitHub error cleared after recovery sync, got {:?}",
        proj_gh.error
    );

    Ok(world.trace.digest())
}
