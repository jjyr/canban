//! Scenario #3 — GitHub synchronization:
//! no duplicate syncs/cards, ETag 304 reuse, zero card-revision churn on idle syncs,
//! coalesced rapid sync requests, multi-page pagination (`Link: rel="next"`),
//! stale response recovery (`inject_stale_response_once`), primary (403) & secondary (429)
//! rate limits, and collaborator vs `external_pr` trust gating.
use crate::{
    github::SimIssue,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

fn make_issue(number: u64, title: &str, author: &str, is_pr: bool) -> SimIssue {
    SimIssue {
        number,
        title: title.into(),
        body: format!("Body for #{number}"),
        state: "open".into(),
        author: author.into(),
        labels: vec![if is_pr { "pr" } else { "bug" }.into()],
        updated_at: "2026-06-01T00:00:00Z".into(),
        is_pr,
        draft: false,
        merged: false,
        head_sha: if is_pr {
            format!("sha-pr-{number}-v1")
        } else {
            String::new()
        },
        head_ref: if is_pr {
            format!("feat/pr-{number}")
        } else {
            String::new()
        },
        base_ref: if is_pr { "main".into() } else { String::new() },
        check_conclusion: is_pr.then(|| "success".into()),
    }
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "github_sync");
    cfg.remote_hosts = vec!["remote-a".into()];

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let repo = "acme/canban-sim";
        world.github.add_repo(repo, 1001, "main");
        world.github.set_collaborators(repo, vec!["alice".into()]);
        world
            .github
            .upsert_issue(repo, make_issue(1, "Fix sync dedup", "alice", false));
        world
            .github
            .upsert_issue(repo, make_issue(2, "Add DST coverage", "alice", false));
        world
            .github
            .upsert_issue(repo, make_issue(3, "PR: Unified client", "alice", true));

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
        oracle.check_step_invariants(world)?;

        // 1. Idle syncs reuse ETag 304 with zero card-revision churn and zero duplicate events.
        let snap_after_initial = world.server("remote-a").unwrap().snapshot()?;
        let rev_map_before: BTreeMap<String, u64> = snap_after_initial
            .cards
            .iter()
            .map(|c| (c.id.clone(), c.revision))
            .collect();
        let events_before = world.server("remote-a").unwrap().events()?.len();

        world.github.reset_stats();
        for _ in 0..3 {
            let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
            world.advance(Duration::from_millis(200))?;
        }
        let idle_stats = world.github.stats();
        let snap_after_idle = world.server("remote-a").unwrap().snapshot()?;
        let events_after_idle = world.server("remote-a").unwrap().events()?.len();
        let rev_map_after: BTreeMap<String, u64> = snap_after_idle
            .cards
            .iter()
            .map(|c| (c.id.clone(), c.revision))
            .collect();
        ensure!(
            rev_map_after == rev_map_before,
            "Idle syncs mutated revisions!"
        );
        ensure!(
            events_after_idle == events_before,
            "Idle syncs emitted duplicate events!"
        );
        ensure!(idle_stats.responses_304 > 0, "Expected ETag 304 responses!");
        ensure!(idle_stats.total_requests <= 45);
        world.hit("github.idle_304_quiet");

        // 2. Rapid `github.sync.request` burst coalescing.
        world.github.reset_stats();
        for _ in 0..5 {
            let _ = world.client_call(
                "github.sync.request",
                json!({ "project_id": pid }),
                Some(&pid),
            )?;
        }
        world.advance(Duration::from_secs(2))?;
        ensure!(world.github.stats().total_requests <= 20);

        // 3. Primary rate limit (HTTP 403 + reset epoch) AND Secondary rate limit (HTTP 429 + Retry-After).
        let reset_epoch = (world.now_ms() / 1000) + 60;
        world.github.inject_primary_rate_limit(reset_epoch);
        let err_403 = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid));
        ensure!(err_403.is_err(), "Expected error on primary 403 rate limit");
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            ensure!(snap.cards.len() == 3);
            let proj_gh = snap
                .projects
                .iter()
                .find(|p| p.id == raw_pid)
                .unwrap()
                .github
                .as_ref()
                .unwrap();
            ensure!(proj_gh.error.is_some());
        }
        world.hit("github.rate_limit_primary");

        world.github.inject_secondary_rate_limit(429, 30);
        let err_429 = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid));
        ensure!(
            err_429.is_err(),
            "Expected error on secondary 429 rate limit"
        );
        ensure!(world.server("remote-a").unwrap().snapshot()?.cards.len() == 3);
        world.hit("github.rate_limit_secondary");

        // 4. Stale response injection (`inject_stale_response_once`):
        //    Add issue #40 on GitHub, inject a stale empty issues response once, sync ->
        //    existing 3 cards must NEVER be dropped, then next sync imports #40 cleanly!
        world
            .github
            .upsert_issue(repo, make_issue(40, "After stale read", "alice", false));
        world
            .github
            .inject_stale_response_once("/issues", json!([]));
        let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
        ensure!(
            world.server("remote-a").unwrap().snapshot()?.cards.len() >= 3,
            "Stale empty GitHub response must not delete imported cards"
        );
        // Real sync imports #40 and clears any prior error.
        let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
        ensure!(
            world.server("remote-a").unwrap().snapshot()?.cards.len() == 4,
            "Expected issue #40 imported after stale response cleared"
        );
        world.hit("github.stale_response_recovery");

        // 5. Multi-page pagination (`Link: rel="next"`): add 33 more open issues (total 37 > 30 per_page)
        //    and verify all pages are imported cleanly without duplicates!
        for num in 100..133 {
            world.github.upsert_issue(
                repo,
                make_issue(num, &format!("Paged issue #{num}"), "alice", false),
            );
        }
        let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
        let paged_count = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .filter(|c| c.project_id == raw_pid)
            .count();
        ensure!(
            paged_count == 37,
            "Multi-page GitHub pagination expected 37 cards on repo, got {paged_count}"
        );
        world.hit("github.pagination_multi_page");

        // Mutate PR #3 in-place and confirm zero duplicates.
        let mut pr3_upd = make_issue(3, "PR: Unified client (updated)", "alice", true);
        pr3_upd.updated_at = "2026-06-01T01:00:00Z".into();
        pr3_upd.head_sha = "sha-pr-3-v2".into();
        pr3_upd.check_conclusion = Some("failure".into());
        world.github.upsert_issue(repo, pr3_upd);
        let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .filter(|c| c.project_id == raw_pid)
                .count()
                == 37
        );

        // 6. Collaborator trust gating & `external_pr` classification:
        //    Bind a second project with `intake: "collaborators"` & `pr_intake: "collaborators"`.
        //    Trusted collaborator `alice` passes; untrusted `mallory` MUST NOT pass!
        let trust_repo = "acme/trusted-only";
        world.github.add_repo(trust_repo, 2002, "main");
        world
            .github
            .set_collaborators(trust_repo, vec!["alice".into()]);
        world.github.upsert_issue(
            trust_repo,
            make_issue(1, "Trusted Collaborator Issue", "alice", false),
        );
        world.github.upsert_issue(
            trust_repo,
            make_issue(2, "Trusted Collaborator PR", "alice", true),
        );
        world.github.upsert_issue(
            trust_repo,
            make_issue(3, "Untrusted External Issue", "mallory", false),
        );
        world.github.upsert_issue(
            trust_repo,
            make_issue(4, "Untrusted External PR", "mallory", true),
        );

        let dir_trust = world.project_dir("gh-trust")?;
        let p_trust = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Trust Gated Repo", "directory": dir_trust }),
            None,
        )?;
        let pid_trust = p_trust["id"].as_str().unwrap().to_owned();
        let raw_pid_trust = canban_types::split_qualified_host_id(&pid_trust)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid_trust.clone());

        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_trust,
                "repository": trust_repo,
                "intake": "collaborators",
                "pr_intake": "collaborators",
                "include_drafts": true,
            }),
            Some(&pid_trust),
        )?;
        let _ = world.client_call(
            "github.sync",
            json!({ "project_id": pid_trust }),
            Some(&pid_trust),
        )?;
        world.advance(Duration::from_millis(200))?;

        let trust_cards: Vec<_> = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .filter(|c| c.project_id == raw_pid_trust)
            .collect();
        ensure!(
            trust_cards.len() == 2,
            "Collaborators intake must admit ONLY alice's 2 items (rejecting mallory), got {:?}",
            trust_cards.iter().map(|c| &c.title).collect::<Vec<_>>()
        );
        ensure!(
            !trust_cards.iter().any(|c| c.title.contains("Untrusted")),
            "Untrusted author mallory bypassed Collaborators intake!"
        );
        let alice_pr = trust_cards
            .iter()
            .find(|c| c.title == "Trusted Collaborator PR")
            .unwrap();
        ensure!(
            alice_pr.source_trust.as_deref() == Some("collaborator"),
            "Collaborator PR must have source_trust == collaborator, got {:?}",
            alice_pr.source_trust
        );
        world.hit("github.collaborator_trust_gating");

        // Opening `pr_intake: "all"` admits mallory's PR #4, tagged strictly as `external_pr`!
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_trust,
                "repository": trust_repo,
                "intake": "collaborators",
                "pr_intake": "all",
                "include_drafts": true,
            }),
            Some(&pid_trust),
        )?;
        let _ = world.client_call(
            "github.sync",
            json!({ "project_id": pid_trust }),
            Some(&pid_trust),
        )?;
        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        let final_snap = world.server("remote-a").unwrap().snapshot()?;
        let mallory_pr = final_snap
            .cards
            .iter()
            .find(|c| c.project_id == raw_pid_trust && c.title == "Untrusted External PR")
            .expect("Expected mallory's PR imported after switching pr_intake=all");
        ensure!(
            mallory_pr.source_trust.as_deref() == Some("external_pr"),
            "External PR must carry source_trust == external_pr, got {:?}",
            mallory_pr.source_trust
        );
        ensure!(
            !final_snap
                .cards
                .iter()
                .any(|c| c.project_id == raw_pid_trust && c.title == "Untrusted External Issue"),
            "Untrusted issue must stay excluded while issue intake == collaborators"
        );
        world.hit("github.external_pr_trust");

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

        Ok(finalize_digest(world))
    })
}
