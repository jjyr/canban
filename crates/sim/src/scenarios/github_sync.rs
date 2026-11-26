//! Scenario #3 — GitHub synchronization:
//! no duplicate syncs/cards, ETag 304 reuse, zero card-revision churn on idle syncs,
//! coalesced rapid sync requests, multi-page pagination (`Link: rel="next"`),
//! stale response recovery (`inject_stale_response_once`), primary (403) & secondary (429)
//! rate limits, collaborator vs `external_pr` trust gating, `github.browse`/`github.detail`
//! fail-closed transport + restart cache persistence, `github.reference.*`,
//! `review.github_pr_file` contents & blob fallback, branch PR discovery + fast-fail CI +
//! PR activity hydration, cardless GitHub issue events + self-action receipt dedup,
//! `github.review.publish` authorization, and Kanban intake suppression / remote-closure edges.
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    github::{SimCheckRun, SimComment, SimIssue, SimReview},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{Card, CardContent, Event, EventSubscription};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

const CI_WATCHER_YAML: &str = r#"schema: 12
id: ci-watcher
name: CI Watcher
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  ci_fix:
    event: github.pr.ci_failed
    start_at: notify_ci_fail
steps:
  - id: notify_ci_fail
    uses: notify
    with:
      message: "CI failure noticed"
    next: done
  - id: done
    uses: complete
"#;

const CARDLESS_ISSUE_TRIAGE_YAML: &str = r#"schema: 12
id: issue-auto-triage
name: Issue Auto Triage
enabled: true
on:
  on_issue_opened:
    event: github.issue.opened
    cardless: true
    match:
      exclude_self: true
    start_at: filter
steps:
  - id: filter
    uses: script
    with:
      run: echo triage-issue
    next: check
  - id: check
    uses: condition
    with:
      pointer: /outputs/filter/matched
      equals: "true"
      then: alert
      otherwise: done
  - id: alert
    uses: notify
    with:
      message: "Matched new issue"
    next: done
  - id: done
    uses: complete
"#;

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

        // =========================================================================
        // 7. Fail-closed `github_transport` (`github.cli.status`, `card.create` author),
        //    `github.browse` pagination & restart cache persistence without mirroring bodies,
        //    `github.detail` (`overview`/`files`/`checks`/404), `github.reference.{add,remove}`,
        //    and `review.github_pr_file` (contents base64 + git blob fallback).
        // =========================================================================
        {
            // 7a. Fail-closed CLI status & `card.create` author without spawning `gh`.
            let cli_status = world.client_call("github.cli.status", json!({}), Some(&pid))?;
            ensure!(
                cli_status["installed"] == false && cli_status["state"] == "missing",
                "github.cli.status must report disabled/missing under injected transport: {cli_status}"
            );
            let local_ref_card = world.client_call(
                "card.create",
                json!({
                    "view": "full",
                    "project_id": pid,
                    "title": "Local Ref Card",
                    "body": "Local context",
                }),
                Some(&pid),
            )?;
            let ref_cid = local_ref_card["id"].as_str().unwrap().to_owned();
            let raw_ref_cid = canban_types::split_qualified_host_id(&ref_cid)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| ref_cid.clone());
            ensure!(
                local_ref_card["created_by"] == "Local",
                "card.create without GitHub credentials must default created_by to Local without spawning gh: {local_ref_card}"
            );

            // 7b. `github.browse` notifications + multi-page issues without creating cards or mirroring bodies.
            world.github.set_notifications(vec![json!({
                "id": "n1",
                "unread": true,
                "repository": { "full_name": "acme/browse-only" },
                "subject": {
                    "type": "Issue",
                    "title": "Browse notification",
                    "url": "https://api.github.com/repos/acme/browse-only/issues/1"
                }
            })]);
            let notif_page = world.client_call(
                "github.browse",
                json!({ "kind": "notifications", "state": "inbox", "relation": "all", "page": 1 }),
                Some(&pid),
            )?;
            ensure!(
                notif_page["items"]
                    .as_array()
                    .is_some_and(|a| a.len() == 1 && a[0]["item"]["id"] == "n1"),
                "github.browse(notifications) must route through SimGitHub: {notif_page}"
            );

            let browse_repo = "acme/browse-only";
            world.github.add_repo(browse_repo, 3003, "main");
            for num in 1..=31u64 {
                let mut issue = make_issue(num, "REMOTE_BODY_SENTINEL", "alice", false);
                issue.body = "REMOTE_BODY_SENTINEL body prose".into();
                issue.updated_at = format!("2026-06-01T00:{:02}:00Z", num);
                world.github.upsert_issue(browse_repo, issue);
            }
            let browse_q = |page: u64| {
                json!({
                    "kind": "issues",
                    "repository": browse_repo,
                    "relation": "all",
                    "state": "open",
                    "query": "",
                    "page": page,
                })
            };
            let first_browse = world.client_call("github.browse", browse_q(1), Some(&pid))?;
            ensure!(
                first_browse["items"]
                    .as_array()
                    .is_some_and(|a| a.len() == 30)
                    && first_browse["has_next"] == true,
                "github.browse page 1 expected 30 items + has_next=true: {first_browse}"
            );
            let second_browse = world.client_call("github.browse", browse_q(2), Some(&pid))?;
            ensure!(
                second_browse["items"]
                    .as_array()
                    .is_some_and(|a| a.len() == 1)
                    && second_browse["has_next"] == false,
                "github.browse page 2 expected 1 item + has_next=false: {second_browse}"
            );
            // Zero cards created for browse-only repo and zero remote sentinel bodies mirrored in SQLite.
            {
                let srv_a = world.server("remote-a").unwrap();
                ensure!(
                    !srv_a.snapshot()?.cards.iter().any(|c| {
                        c.github_issue()
                            .is_some_and(|s| s.repository == browse_repo)
                    }),
                    "github.browse must never create cards"
                );
                let conn = rusqlite::Connection::open(srv_a.home.join("canban.sqlite"))?;
                let mut stmt = conn.prepare(
                    "SELECT data FROM projects UNION ALL SELECT data FROM cards UNION ALL SELECT data FROM events",
                )?;
                let all_rows: Vec<String> = stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                ensure!(
                    !all_rows.iter().any(|t| t.contains("REMOTE_BODY_SENTINEL")),
                    "github.browse must never mirror remote issue bodies into SQLite"
                );
            }
            // Restart server and verify cached browse page survives even with host GitHub offline!
            world.bounce_server("remote-a")?;
            world.heal_and_quiesce(15_000)?;
            world.github.set_host_offline("remote-a", true);
            let cached_after_bounce =
                world.client_call("github.browse", browse_q(1), Some(&pid))?;
            world.github.set_host_offline("remote-a", false);
            ensure!(
                cached_after_bounce["items"] == first_browse["items"]
                    && cached_after_bounce["cached"] == true,
                "github.browse cache must survive AppServer restart without hitting GitHub"
            );

            // 7c. `github.detail` routes through SimGitHub and fails closed on 404.
            let detail_ok = world.client_call(
                "github.detail",
                json!({
                    "reference": {
                        "repository": browse_repo,
                        "kind": "issue",
                        "number": 1,
                        "url": format!("https://github.com/{browse_repo}/issues/1"),
                    },
                    "section": "overview",
                    "refresh": true,
                }),
                Some(&pid),
            )?;
            ensure!(
                detail_ok["data"]["number"] == 1,
                "github.detail overview mismatch: {detail_ok}"
            );
            let detail_404 = world.client_call(
                "github.detail",
                json!({
                    "reference": {
                        "repository": browse_repo,
                        "kind": "issue",
                        "number": 9999,
                        "url": format!("https://github.com/{browse_repo}/issues/9999"),
                    },
                    "section": "overview",
                    "refresh": true,
                }),
                Some(&pid),
            );
            ensure!(
                detail_404.is_err(),
                "github.detail on missing issue must fail closed without falling back to gh"
            );

            // 7d. `github.reference.add` & `github.reference.remove`: normalization, local identity,
            //     optimistic revision check, and zero agent spawns.
            let ref_rev = local_ref_card["revision"].as_u64().unwrap();
            let added_ref = world.client_call(
                "github.reference.add",
                json!({
                    "view": "full",
                    "card_id": ref_cid,
                    "revision": ref_rev,
                    "url": "https://github.com/acme/canban-sim/pull/3/files",
                }),
                Some(&pid),
            )?;
            let saved_ref_card: Card = serde_json::from_value(added_ref)?;
            ensure!(
                saved_ref_card.content == CardContent::Local
                    && saved_ref_card.github_links.len() == 1
                    && saved_ref_card.github_links[0].url
                        == "https://github.com/acme/canban-sim/pull/3",
                "github.reference.add failed normalization or altered local identity: {saved_ref_card:?}"
            );
            // Stale revision must be rejected.
            let stale_ref = world.client_call(
                "github.reference.add",
                json!({
                    "card_id": ref_cid,
                    "revision": ref_rev,
                    "url": "https://github.com/acme/canban-sim/issues/2",
                }),
                Some(&pid),
            );
            ensure!(
                stale_ref.is_err(),
                "github.reference.add must reject stale revision"
            );
            ensure!(
                world
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .terminals
                    .iter()
                    .all(|t| t.agent.is_none()),
                "Adding GitHub reference must never spawn an Agent"
            );
            let removed_ref = world.client_call(
                "github.reference.remove",
                json!({
                    "view": "full",
                    "card_id": ref_cid,
                    "revision": saved_ref_card.revision,
                    "reference": saved_ref_card.github_links[0],
                }),
                Some(&pid),
            )?;
            let removed_card: Card = serde_json::from_value(removed_ref)?;
            ensure!(
                removed_card.github_links.is_empty(),
                "github.reference.remove did not clear reference: {removed_card:?}"
            );
            let _ = world.client_call(
                "card.remove",
                json!({ "card_id": ref_cid, "revision": removed_card.revision }),
                Some(&pid),
            )?;
            let _ = raw_ref_cid;

            // 7e. `review.github_pr_file`: decodes `/contents` base64 AND falls back to `/git/blobs/<sha>`.
            let file_sha = "1234567890abcdef1234567890abcdef12345678";
            let blob_sha = "abcdef1234567890abcdef1234567890abcdef12";
            // "alpha\nbeta\ngamma\n" in base64 with trailing newline
            world.github.set_file_contents(
                browse_repo,
                file_sha,
                "src/main.rs",
                json!({
                    "encoding": "base64",
                    "content": "YWxwaGEKYmV0YQpnYW1tYQo=\n",
                }),
            );
            let small_file = world.client_call(
                "review.github_pr_file",
                json!({
                    "repository": browse_repo,
                    "sha": file_sha,
                    "path": "src/main.rs",
                }),
                Some(&pid),
            )?;
            ensure!(
                small_file["total_lines"] == 3
                    && small_file["lines"] == json!(["alpha", "beta", "gamma"]),
                "review.github_pr_file /contents decode mismatch: {small_file}"
            );
            // Large file: /contents omits base64 (`encoding: "none"`) -> falls back to /git/blobs/<blob_sha>.
            world.github.set_file_contents(
                browse_repo,
                file_sha,
                "src/big.rs",
                json!({
                    "encoding": "none",
                    "sha": blob_sha,
                }),
            );
            // "row1\nrow2\n" in base64
            world.github.set_git_blob(
                browse_repo,
                blob_sha,
                json!({
                    "encoding": "base64",
                    "content": "cm93MQpyb3cyCg==",
                }),
            );
            let big_file = world.client_call(
                "review.github_pr_file",
                json!({
                    "repository": browse_repo,
                    "sha": file_sha,
                    "path": "src/big.rs",
                }),
                Some(&pid),
            )?;
            ensure!(
                big_file["total_lines"] == 2 && big_file["lines"] == json!(["row1", "row2"]),
                "review.github_pr_file /git/blobs fallback mismatch: {big_file}"
            );

            // Also exercise `github.detail` `files` and `checks` sections on a PR.
            let mut files_pr = make_issue(50, "PR with files", "alice", true);
            files_pr.head_sha = file_sha.into();
            world.github.upsert_issue(browse_repo, files_pr);
            world.github.set_pr_files(
                browse_repo,
                50,
                vec![json!({
                    "filename": "src/main.rs",
                    "status": "modified",
                    "additions": 3,
                    "deletions": 1,
                    "patch": "@@ -1 +1,3 @@\n+alpha\n+beta\n+gamma",
                })],
            );
            let pr_files_detail = world.client_call(
                "github.detail",
                json!({
                    "reference": {
                        "repository": browse_repo,
                        "kind": "pr",
                        "number": 50,
                        "url": format!("https://github.com/{browse_repo}/pull/50"),
                    },
                    "section": "files",
                    "head": file_sha,
                }),
                Some(&pid),
            )?;
            ensure!(
                pr_files_detail["data"]
                    .as_array()
                    .is_some_and(|a| a.len() == 1 && a[0]["filename"] == "src/main.rs"),
                "github.detail(files) mismatch: {pr_files_detail}"
            );
            let pr_checks_detail = world.client_call(
                "github.detail",
                json!({
                    "reference": {
                        "repository": browse_repo,
                        "kind": "pr",
                        "number": 50,
                        "url": format!("https://github.com/{browse_repo}/pull/50"),
                    },
                    "section": "checks",
                    "head": file_sha,
                }),
                Some(&pid),
            )?;
            ensure!(
                pr_checks_detail["data"]["check_runs"].is_array(),
                "github.detail(checks) mismatch: {pr_checks_detail}"
            );

            world.hit("github.browse_and_references");
        }

        // =========================================================================
        // 8. Branch PR discovery + Option B fast-fail CI + workflow trigger + dedup + rerun,
        //    PR activity updates + ephemeral `projected_body` hydration,
        //    cardless GitHub issue events (`opened` vs `discovered`, `exclude_self`, script env isolation),
        //    self-action receipt dedup & linked-PR double-fire avoidance,
        //    and `github.review.publish` step authorization & empty-draft skip.
        // =========================================================================
        {
            // Configure GitHub credential login `"bot-owner"` on `remote-a` so `self_action` /
            // `exclude_self` run against a real authenticated identity while HTTP stays 100% on `SimGitHub`.
            {
                let srv_home = world.server("remote-a").unwrap().home.clone();
                let cred_dir = srv_home.join("credentials");
                std::fs::create_dir_all(&cred_dir)?;
                let cred_path = cred_dir.join("github.json");
                let cred_bytes = serde_json::to_vec(&json!({
                    "client_id": canban_github::auth::CLIENT_ID,
                    "login": "bot-owner",
                    "access_token": "sim-fixture-token",
                    "refresh_token": null,
                    "expires_at": null,
                }))?;
                std::fs::write(&cred_path, &cred_bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&cred_dir, std::fs::Permissions::from_mode(0o700))?;
                    std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600))?;
                }
            }
            world.bounce_server("remote-a")?;
            world.heal_and_quiesce(15_000)?;

            let exec = world
                .exec_runner("remote-a")
                .context("remote-a SimExec unavailable")?;

            // 8a. Branch PR discovery, fast-fail CI, dedup, rerun, workflow trigger, & ephemeral checks_summary.
            let ci_repo = "acme/ci-flow";
            world.github.add_repo(ci_repo, 4004, "main");
            let ci_dir = world.project_dir("gh-ci-flow")?;
            let mut ci_git = SimGitRepo::default();
            ci_git
                .remotes
                .insert("origin".into(), format!("https://github.com/{ci_repo}.git"));
            exec.register_git_repo(&ci_dir, ci_git);

            let ci_proj = world.client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "CI Flow", "directory": ci_dir }),
                None,
            )?;
            let pid_ci = ci_proj["id"].as_str().unwrap().to_owned();
            let _ = world.client_call(
                "project.github",
                json!({
                    "project_id": pid_ci,
                    "repository": ci_repo,
                    "intake": "off",
                    "pr_intake": "off",
                }),
                Some(&pid_ci),
            )?;
            let ci_sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid_ci }),
                Some(&pid_ci),
            )?;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid_ci,
                    "revision": ci_sources["revision"],
                    "sources": [{ "filename": "development.yaml", "source": CI_WATCHER_YAML }],
                }),
                Some(&pid_ci),
            )?;

            let wt_ci = world.client_call(
                "worktree.create",
                json!({
                    "project_id": pid_ci,
                    "branch": "feat/track-pr-ci",
                    "launch": "none",
                }),
                Some(&pid_ci),
            )?;
            let wt_ci_id = wt_ci["worktree"]["id"].as_str().unwrap().to_owned();
            let ci_card = world.client_call(
                "worktree.promote",
                json!({
                    "worktree_id": wt_ci_id,
                    "title": "Track PR CI",
                    "body": "Verify PR CI tracking",
                }),
                Some(&pid_ci),
            )?;
            let cid_ci = ci_card["id"].as_str().unwrap().to_owned();
            let raw_cid_ci = canban_types::split_qualified_host_id(&cid_ci)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_ci.clone());
            let ci_branch = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_cid_ci)
                .and_then(|c| c.branch)
                .context("worktree.promote did not assign card branch")?;

            let sha1 = "1111111111111111111111111111111111111111";
            let sha2 = "2222222222222222222222222222222222222222";
            let mut pr148 = make_issue(148, "PR: Track CI", "bot-owner", true);
            pr148.head_ref = ci_branch.clone();
            pr148.head_sha = sha1.into();
            world.github.upsert_issue(ci_repo, pr148.clone());
            // Option B fast-fail: clippy failed while test is still in_progress.
            world.github.set_check_runs(
                ci_repo,
                sha1,
                vec![
                    SimCheckRun {
                        id: 901,
                        name: "clippy".into(),
                        status: "completed".into(),
                        conclusion: Some("failure".into()),
                    },
                    SimCheckRun {
                        id: 902,
                        name: "test".into(),
                        status: "in_progress".into(),
                        conclusion: None,
                    },
                ],
            );

            let _ = world.client_call(
                "github.sync",
                json!({ "project_id": pid_ci }),
                Some(&pid_ci),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.workflow_runs.iter().any(|r| {
                    r.card_id.as_deref() == Some(raw_cid_ci.as_str())
                        && r.workflow_id == "ci-watcher"
                }))
            })?;

            {
                let srv = world.server("remote-a").unwrap();
                let snap = srv.snapshot()?;
                let c1 = snap.cards.iter().find(|c| c.id == raw_cid_ci).unwrap();
                let pr = c1.pull_request.as_ref().context("PR #148 not linked")?;
                ensure!(pr.number == 148 && pr.checks == "failure");
                let summary = pr
                    .checks_summary
                    .as_ref()
                    .context("missing checks_summary")?;
                ensure!(
                    summary.failed == 1
                        && summary.running == 1
                        && summary.failed_checks.len() == 1
                        && summary.failed_checks[0].name == "clippy",
                    "fast-fail checks_summary mismatch: {summary:?}"
                );
                ensure!(
                    c1.attention.is_none(),
                    "Automated CI workflow must clear passive CI attention banner, got {:?}",
                    c1.attention
                );
                let wf_run = snap
                    .workflow_runs
                    .iter()
                    .find(|r| {
                        r.card_id.as_deref() == Some(raw_cid_ci.as_str())
                            && r.workflow_id == "ci-watcher"
                    })
                    .unwrap();
                ensure!(
                    wf_run.inputs["trigger_event"]["kind"] == "github.pr.ci_failed"
                        && wf_run.inputs["trigger_event"]["data"]["number"] == 148,
                    "ci-watcher trigger payload mismatch: {:?}",
                    wf_run.inputs
                );
                let evs = srv.events()?;
                ensure!(
                    evs.iter()
                        .filter(|e| e.card_id == raw_cid_ci && e.kind == "github.pr.ci_failed")
                        .count()
                        == 1
                        && evs
                            .iter()
                            .any(|e| e.card_id == raw_cid_ci && e.kind == "github.pr_linked"),
                    "Expected exactly 1 github.pr.ci_failed and github.pr_linked"
                );
            }

            // Repeated refresh on same SHA & outcome must NOT emit duplicate ci_failed.
            let _ = world.client_call(
                "github.pr.refresh",
                json!({ "card_id": cid_ci }),
                Some(&pid_ci),
            )?;
            ensure!(
                world
                    .server("remote-a")
                    .unwrap()
                    .events()?
                    .iter()
                    .filter(|e| e.card_id == raw_cid_ci && e.kind == "github.pr.ci_failed")
                    .count()
                    == 1,
                "Repeated github.pr.refresh emitted duplicate github.pr.ci_failed"
            );

            // Re-run transitions to `running`, then `success` -> emits `github.pr.ci_succeeded`.
            world.github.set_check_runs(
                ci_repo,
                sha1,
                vec![
                    SimCheckRun {
                        id: 901,
                        name: "clippy".into(),
                        status: "in_progress".into(),
                        conclusion: None,
                    },
                    SimCheckRun {
                        id: 902,
                        name: "test".into(),
                        status: "in_progress".into(),
                        conclusion: None,
                    },
                ],
            );
            let _ = world.client_call(
                "github.pr.refresh",
                json!({ "card_id": cid_ci }),
                Some(&pid_ci),
            )?;
            ensure!(
                world
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .cards
                    .iter()
                    .find(|c| c.id == raw_cid_ci)
                    .unwrap()
                    .pull_request
                    .as_ref()
                    .unwrap()
                    .checks
                    == "running"
            );

            world.github.set_check_runs(
                ci_repo,
                sha1,
                vec![
                    SimCheckRun {
                        id: 901,
                        name: "clippy".into(),
                        status: "completed".into(),
                        conclusion: Some("success".into()),
                    },
                    SimCheckRun {
                        id: 902,
                        name: "test".into(),
                        status: "completed".into(),
                        conclusion: Some("success".into()),
                    },
                ],
            );
            let _ = world.client_call(
                "github.pr.refresh",
                json!({ "card_id": cid_ci }),
                Some(&pid_ci),
            )?;
            {
                let srv = world.server("remote-a").unwrap();
                let snap = srv.snapshot()?;
                let pr_ok = snap
                    .cards
                    .iter()
                    .find(|c| c.id == raw_cid_ci)
                    .unwrap()
                    .pull_request
                    .as_ref()
                    .unwrap();
                ensure!(pr_ok.checks == "success");
                ensure!(
                    srv.events()?
                        .iter()
                        .filter(|e| e.card_id == raw_cid_ci && e.kind == "github.pr.ci_succeeded")
                        .count()
                        == 1
                );
                // Durable SQLite `cards` JSON must NOT store ephemeral `checks_summary`.
                let conn = rusqlite::Connection::open(srv.home.join("canban.sqlite"))?;
                let raw_card: String =
                    conn.query_row("SELECT data FROM cards WHERE id=?1", [&raw_cid_ci], |r| {
                        r.get(0)
                    })?;
                ensure!(
                    !raw_card.contains("checks_summary"),
                    "Durable SQLite cards JSON must not persist checks_summary"
                );
            }

            // 8b. PR activity updates: new head SHA + `/compare` commits + issue comment +
            //     inline review comment + CHANGES_REQUESTED review + ephemeral `projected_body` hydration.
            let mut pr148_v2 = pr148;
            pr148_v2.head_sha = sha2.into();
            pr148_v2.updated_at = "2026-06-01T02:00:00Z".into();
            world.github.upsert_issue(ci_repo, pr148_v2);
            world.github.set_check_runs(
                ci_repo,
                sha2,
                vec![SimCheckRun {
                    id: 903,
                    name: "test".into(),
                    status: "completed".into(),
                    conclusion: Some("success".into()),
                }],
            );
            world.github.set_compare(
                ci_repo,
                sha1,
                sha2,
                json!([
                    {"sha": sha2, "commit": {"message": "Fix edge case"}, "author": {"login": "alice"}}
                ]),
            );
            let _ = world.github.add_issue_comment(
                ci_repo,
                148,
                "alice",
                "Remote PR discussion body should stay ephemeral",
                "2026-06-01T02:01:00Z",
            );
            let _ = world.github.add_pr_review_comment(
                ci_repo,
                148,
                SimComment {
                    id: 9002,
                    issue_number: 148,
                    author: "bob".into(),
                    body: "Inline review comment body".into(),
                    updated_at: "2026-06-01T02:02:00Z".into(),
                    path: Some("crates/ui/src/overview.rs".into()),
                    line: Some(42),
                    side: Some("RIGHT".into()),
                    in_reply_to_id: None,
                },
            );
            let _ = world.github.add_pr_review(
                ci_repo,
                148,
                SimReview {
                    id: 9003,
                    author: "bob".into(),
                    state: "CHANGES_REQUESTED".into(),
                    body: "Please address inline note".into(),
                    commit_id: sha2.into(),
                    submitted_at: "2026-06-01T02:03:00Z".into(),
                },
            );

            let _ = world.client_call(
                "github.pr.refresh",
                json!({ "card_id": cid_ci }),
                Some(&pid_ci),
            )?;
            {
                let srv = world.server("remote-a").unwrap();
                let updated_card = srv
                    .snapshot()?
                    .cards
                    .into_iter()
                    .find(|c| c.id == raw_cid_ci)
                    .unwrap();
                ensure!(
                    updated_card
                        .attention
                        .as_deref()
                        .is_some_and(|a| a.starts_with("Changes requested on PR #148")),
                    "Expected Changes requested attention banner, got {:?}",
                    updated_card.attention
                );
            }
            let tl_page = world.client_call(
                "timeline.list",
                json!({ "card_id": cid_ci, "view": "full", "tail": true, "limit": 100 }),
                Some(&pid_ci),
            )?;
            let tl_events: Vec<Event> = serde_json::from_value(tl_page["events"].clone())?;
            let upd_ev = tl_events
                .iter()
                .find(|e| e.kind == "github.pr.updated")
                .context("missing github.pr.updated")?;
            ensure!(
                upd_ev.data["commits"][0].as_str() == Some("2222222 · Fix edge case"),
                "github.pr.updated compare commit mismatch: {:?}",
                upd_ev.data
            );
            let issue_cmt = tl_events
                .iter()
                .find(|e| e.kind == "github.pr.commented" && e.data["kind"] == "issue_comment")
                .context("missing issue_comment github.pr.commented")?;
            ensure!(
                issue_cmt.data["projected_body"].as_str()
                    == Some("Remote PR discussion body should stay ephemeral"),
                "issue_comment projected_body mismatch: {:?}",
                issue_cmt.data
            );
            let inline_cmt = tl_events
                .iter()
                .find(|e| e.kind == "github.pr.commented" && e.data["kind"] == "review_comment")
                .context("missing review_comment github.pr.commented")?;
            ensure!(
                inline_cmt.data["projected_body"].as_str() == Some("Inline review comment body")
                    && inline_cmt.data["line"].as_u64() == Some(42),
                "review_comment projected_body/line mismatch: {:?}",
                inline_cmt.data
            );
            let rev_ev = tl_events
                .iter()
                .find(|e| e.kind == "github.pr.reviewed")
                .context("missing github.pr.reviewed")?;
            ensure!(
                rev_ev.data["state"].as_str() == Some("CHANGES_REQUESTED")
                    && rev_ev.data["projected_body"].as_str() == Some("Please address inline note"),
                "github.pr.reviewed mismatch: {:?}",
                rev_ev.data
            );
            // Durable SQLite `events` JSON must NEVER mirror remote prose bodies or `projected_body`.
            {
                let srv = world.server("remote-a").unwrap();
                let conn = rusqlite::Connection::open(srv.home.join("canban.sqlite"))?;
                let mut stmt = conn.prepare("SELECT data FROM events WHERE card_id=?1")?;
                let raw_evs: Vec<String> = stmt
                    .query_map([&raw_cid_ci], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for raw in raw_evs {
                    ensure!(
                        !raw.contains("Remote PR discussion body should stay ephemeral")
                            && !raw.contains("Inline review comment body")
                            && !raw.contains("Please address inline note")
                            && !raw.contains("projected_body"),
                        "Durable SQLite event mirrored remote PR prose: {raw}"
                    );
                }
            }

            // 8c. Cardless GitHub issue events (`opened` vs `discovered`, `exclude_self`,
            //     `script -> condition -> notify -> complete`, env isolation) +
            //     self-action receipt dedup & linked-PR double-fire avoidance.
            let widgets_repo = "acme/widgets";
            world.github.add_repo(widgets_repo, 9001, "main");
            let widgets_dir = world.project_dir("gh-widgets")?;
            let mut widgets_git = SimGitRepo::default();
            widgets_git.remotes.insert(
                "origin".into(),
                format!("https://github.com/{widgets_repo}.git"),
            );
            exec.register_git_repo(&widgets_dir, widgets_git);

            let widgets_proj = world.client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Cardless Repo", "directory": widgets_dir }),
                None,
            )?;
            let pid_w = widgets_proj["id"].as_str().unwrap().to_owned();
            let raw_pid_w = canban_types::split_qualified_host_id(&pid_w)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| pid_w.clone());
            let _ = world.client_call(
                "project.github",
                json!({
                    "project_id": pid_w,
                    "repository": widgets_repo,
                    "intake": "off",
                    "pr_intake": "off",
                }),
                Some(&pid_w),
            )?;

            // Seed pre-baseline issues #10 and #50 (older than SIM_EPOCH_BASE_MS `2023-11-14`)
            // and PR #77 linked to a Card on `acme/widgets`.
            let mut old_issue_10 = make_issue(10, "Old issue", "alice", false);
            old_issue_10.updated_at = "2023-01-01T00:00:00Z".into();
            world.github.upsert_issue(widgets_repo, old_issue_10);
            world
                .github
                .set_issue_created_at(widgets_repo, 10, "2023-01-01T00:00:00Z");

            let mut issue_50 = make_issue(50, "Receipt target issue", "alice", false);
            issue_50.updated_at = "2023-01-01T00:00:00Z".into();
            world.github.upsert_issue(widgets_repo, issue_50);
            world
                .github
                .set_issue_created_at(widgets_repo, 50, "2023-01-01T00:00:00Z");

            let wt_pr77 = world.client_call(
                "worktree.create",
                json!({
                    "project_id": pid_w,
                    "branch": "feat/pr-77",
                    "launch": "none",
                }),
                Some(&pid_w),
            )?;
            let wt_pr77_id = wt_pr77["worktree"]["id"].as_str().unwrap().to_owned();
            let pr77_card = world.client_call(
                "worktree.promote",
                json!({
                    "worktree_id": wt_pr77_id,
                    "title": "PR #77",
                    "body": "Linked PR dedup check",
                }),
                Some(&pid_w),
            )?;
            let cid_pr77 = pr77_card["id"].as_str().unwrap().to_owned();
            let raw_cid_pr77 = canban_types::split_qualified_host_id(&cid_pr77)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_pr77.clone());
            let pr77_branch = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_cid_pr77)
                .and_then(|c| c.branch)
                .context("PR #77 card missing branch")?;
            let mut pr77 = make_issue(77, "Linked PR #77", "alice", true);
            pr77.head_ref = pr77_branch;
            pr77.head_sha = "7777777777777777777777777777777777777777".into();
            pr77.updated_at = "2023-01-01T00:00:00Z".into();
            world.github.upsert_issue(widgets_repo, pr77);
            world
                .github
                .set_issue_created_at(widgets_repo, 77, "2023-01-01T00:00:00Z");

            // Apply cardless issue workflow and scripted `filter` output.
            exec.set_script_default(
                "filter",
                SimShellOutcome::ok("filtered issue\n").with_output("matched", "true"),
            );
            let w_sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid_w }),
                Some(&pid_w),
            )?;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid_w,
                    "revision": w_sources["revision"],
                    "sources": [{ "filename": "issue-triage.yaml", "source": CARDLESS_ISSUE_TRIAGE_YAML }],
                }),
                Some(&pid_w),
            )?;

            // Verify authoring bound `selector.repository` and `exclude_self: true`.
            let subs: Vec<EventSubscription> = serde_json::from_value(world.client_call(
                "subscription.list",
                json!({}),
                Some(&pid_w),
            )?)?;
            let sub = subs
                .iter()
                .find(|s| s.id.contains("issue-auto-triage"))
                .context("missing compiled issue-auto-triage subscription")?;
            ensure!(
                sub.selector.repository.as_deref() == Some(widgets_repo)
                    && sub.selector.exclude_self,
                "cardless subscription selector mismatch: {:?}",
                sub.selector
            );

            // 1st sync: establishes repo watermark (`baseline_at`) + emits `discovered` for #10 + links PR #77.
            let _ =
                world.client_call("github.sync", json!({ "project_id": pid_w }), Some(&pid_w))?;
            world.advance(Duration::from_millis(200))?;

            // Establish discussion baseline on issue #50 before writing a comment via Canban.
            let _ = world.client_call(
                "github.detail",
                json!({
                    "reference": {
                        "repository": widgets_repo,
                        "kind": "issue",
                        "number": 50,
                        "url": format!("https://github.com/{widgets_repo}/issues/50"),
                    },
                    "section": "discussion",
                    "refresh": true,
                }),
                Some(&pid_w),
            )?;

            exec.clear_recorded_commands();

            // Now create post-baseline issues #11 (`alice`) and #12 (`bot-owner` -> self_action),
            // post a Canban-written comment on #50 under a DIFFERENT GitHub login (`secondary-pat-bot`),
            // and add an external comment on linked PR #77!
            let mut issue_11 = make_issue(11, "Brand new issue", "alice", false);
            issue_11.updated_at = "2026-06-01T00:05:00Z".into();
            world.github.upsert_issue(widgets_repo, issue_11);
            world
                .github
                .set_issue_created_at(widgets_repo, 11, "2026-06-01T00:05:00Z");

            let mut issue_12 = make_issue(12, "Self-created issue", "bot-owner", false);
            issue_12.updated_at = "2026-06-01T00:06:00Z".into();
            world.github.upsert_issue(widgets_repo, issue_12);
            world
                .github
                .set_issue_created_at(widgets_repo, 12, "2026-06-01T00:06:00Z");

            world.github.set_login("secondary-pat-bot");
            let remote_cmt = world.client_call(
                "github.remote.comment",
                json!({
                    "reference": {
                        "repository": widgets_repo,
                        "kind": "issue",
                        "number": 50,
                        "url": format!("https://github.com/{widgets_repo}/issues/50"),
                    },
                    "body": "Automated reply",
                    "request_key": format!("rcpt-cmt-{seed}"),
                }),
                Some(&pid_w),
            )?;
            world.github.set_login("bot");
            ensure!(
                remote_cmt["state"] == "published",
                "github.remote.comment expected published: {remote_cmt}"
            );
            let receipt_comment_id = remote_cmt["id"]
                .as_u64()
                .context("github.remote.comment missing published id")?;

            let _ = world.github.add_issue_comment(
                widgets_repo,
                77,
                "reviewer",
                "External PR #77 comment",
                "2026-06-01T01:05:00Z",
            );

            // 2nd sync: polls post-baseline issues & repo comments and fires cardless workflow!
            let _ =
                world.client_call("github.sync", json!({ "project_id": pid_w }), Some(&pid_w))?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.workflow_runs.iter().any(|r| {
                    r.is_cardless()
                        && r.workflow_id == "issue-auto-triage"
                        && r.state == "completed"
                }))
            })?;

            {
                let srv = world.server("remote-a").unwrap();
                let snap = srv.snapshot()?;
                let evs = srv.events()?;
                ensure!(
                    evs.iter().any(|e| {
                        e.kind == "github.issue.discovered"
                            && e.data["repository"] == widgets_repo
                            && e.data["number"] == 10
                    }) && !evs.iter().any(|e| {
                        e.kind == "github.issue.opened"
                            && e.data["repository"] == widgets_repo
                            && e.data["number"] == 10
                    }),
                    "Pre-baseline issue #10 must emit discovered, not opened"
                );
                let opened_11 = evs
                    .iter()
                    .find(|e| {
                        e.kind == "github.issue.opened"
                            && e.data["repository"] == widgets_repo
                            && e.data["number"] == 11
                    })
                    .context("post-baseline issue #11 must emit github.issue.opened")?;
                ensure!(
                    opened_11.actor == "github:alice" && opened_11.data["self_action"] == false,
                    "issue #11 opened actor/self_action mismatch: {opened_11:?}"
                );
                let opened_12 = evs
                    .iter()
                    .find(|e| {
                        e.kind == "github.issue.opened"
                            && e.data["repository"] == widgets_repo
                            && e.data["number"] == 12
                    })
                    .context(
                        "self issue #12 must emit github.issue.opened with self_action=true",
                    )?;
                ensure!(
                    opened_12.actor == "github:bot-owner" && opened_12.data["self_action"] == true,
                    "issue #12 opened actor/self_action mismatch: {opened_12:?}"
                );

                // Exactly ONE cardless WorkflowRun (for #11; #10 discovered, #12 suppressed by exclude_self).
                let cardless_runs: Vec<_> = snap
                    .workflow_runs
                    .iter()
                    .filter(|r| r.is_cardless() && r.workflow_id == "issue-auto-triage")
                    .collect();
                ensure!(
                    cardless_runs.len() == 1 && cardless_runs[0].state == "completed",
                    "Expected exactly 1 completed cardless WorkflowRun, got {cardless_runs:?}"
                );
                ensure!(
                    cardless_runs[0]
                        .outputs
                        .get("filter")
                        .and_then(|m| m.get("matched"))
                        .and_then(serde_json::Value::as_str)
                        == Some("true"),
                    "cardless script output missing matched=true"
                );
                ensure!(
                    snap.notices
                        .iter()
                        .any(|n| n.project_id == raw_pid_w && n.message == "Matched new issue"),
                    "Condition step must branch to alert Notify step"
                );

                // Verify script env isolation via recorded SimExec CommandSpec!
                let script_cmd = exec
                    .recorded_commands()
                    .into_iter()
                    .find(|c| {
                        c.env_set.get("CANBAN_STEP_ID").map(String::as_str) == Some("filter")
                            && c.env_set.get("CANBAN_WORKFLOW").map(String::as_str)
                                == Some("issue-auto-triage")
                    })
                    .context("missing recorded CommandSpec for cardless filter script")?;
                ensure!(
                    script_cmd
                        .env_set
                        .get("CANBAN_ISSUE_NUMBER")
                        .map(String::as_str)
                        == Some("11")
                        && !script_cmd.env_set.contains_key("CANBAN_PR_NUMBER")
                        && script_cmd
                            .env_set
                            .get("CANBAN_GITHUB_REPO")
                            .map(String::as_str)
                            == Some(widgets_repo)
                        && script_cmd
                            .env_set
                            .get("CANBAN_SOURCE_TRUST")
                            .map(String::as_str)
                            == Some("external"),
                    "Cardless github.issue.* script env isolation mismatch: {:?}",
                    script_cmd.env_set
                );

                // Self-action receipt dedup: comment posted under `secondary-pat-bot` is marked `self_action: true`!
                let rcpt_ev = evs
                    .iter()
                    .find(|e| {
                        e.kind == "github.issue.comment_created"
                            && e.data["repository"] == widgets_repo
                            && e.data["item_id"] == receipt_comment_id
                    })
                    .context("missing github.issue.comment_created for receipt-tracked comment")?;
                ensure!(
                    rcpt_ev.actor == "github:secondary-pat-bot"
                        && rcpt_ev.data["self_action"] == true,
                    "Receipt-tracked comment must mark self_action=true across logins: {rcpt_ev:?}"
                );
                // PR #77 with linked PR Card must NOT emit duplicate `github.pr.comment_created`.
                ensure!(
                    !evs.iter().any(|e| {
                        e.kind == "github.pr.comment_created"
                            && e.data["repository"] == widgets_repo
                            && e.data["number"] == 77
                    }),
                    "Linked PR Card #77 must not double-fire github.pr.comment_created"
                );
            }

            // 8d. `github.review.publish` step authorization & empty-draft skip.
            let agent_pub_err = world.client_call(
                "github.review.publish",
                json!({
                    "card_id": cid_ci,
                    "execution": { "agent": true, "profile_id": "reviewer" },
                }),
                Some(&pid_ci),
            );
            ensure!(
                agent_pub_err.as_ref().is_err_and(|e| e
                    .to_string()
                    .contains("Review publishing is only permitted")),
                "Untrusted agent execution must be rejected by github.review.publish: {agent_pub_err:?}"
            );
            let bad_run_err = world.client_call(
                "github.review.publish",
                json!({
                    "card_id": cid_ci,
                    "workflow_run_id": "nonexistent-run",
                }),
                Some(&pid_ci),
            );
            ensure!(
                bad_run_err
                    .as_ref()
                    .is_err_and(|e| e.to_string().contains("Workflow run not found")),
                "Bogus workflow_run_id must be rejected by github.review.publish: {bad_run_err:?}"
            );
            let skipped_no_draft = world.client_call(
                "github.review.publish",
                json!({ "card_id": cid_ci }),
                Some(&pid_ci),
            )?;
            ensure!(
                skipped_no_draft["state"] == "skipped",
                "github.review.publish with no draft must skip cleanly: {skipped_no_draft}"
            );

            world.hit("github.ci_fast_fail_and_cardless_issue");
        }

        // =========================================================================
        // 9. Kanban GitHub intake edge cases:
        //    - PR-as-source without copying remote prose into durable SQLite `cards`/`github_issues`
        //    - Local removal suppresses auto-sync re-import; explicit `github.item.add` restores
        //      same Card ID, number, local instructions, and `card.removed` history
        //    - Remote issue closure preserves local stage, local instructions, non-archived status,
        //      and canonical SQLite title (`repo#number`)
        //    - `@codex` inside remote comments NEVER spawns an Agent
        // =========================================================================
        {
            let intake_repo = "acme/intake-edge";
            world.github.add_repo(intake_repo, 5005, "main");
            world
                .github
                .set_collaborators(intake_repo, vec!["alice".into(), "bot-owner".into()]);
            world
                .github
                .upsert_issue(intake_repo, make_issue(1, "Remote Issue 1", "alice", false));
            let _ = world.github.add_issue_comment(
                intake_repo,
                1,
                "alice",
                "@codex remote text must never run an Agent",
                "2026-06-01T00:01:00Z",
            );
            world
                .github
                .upsert_issue(intake_repo, make_issue(2, "Remote Issue 2", "alice", false));
            world
                .github
                .upsert_issue(intake_repo, make_issue(3, "Remote PR 3", "alice", true));

            let intake_dir = world.project_dir("gh-intake-edge")?;
            let p_intake = world.client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Intake Edge", "directory": intake_dir }),
                None,
            )?;
            let pid_intake = p_intake["id"].as_str().unwrap().to_owned();
            let raw_pid_intake = canban_types::split_qualified_host_id(&pid_intake)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| pid_intake.clone());
            let _ = world.client_call(
                "project.github",
                json!({
                    "project_id": pid_intake,
                    "repository": intake_repo,
                    "intake": "all",
                    "pr_intake": "all",
                    "include_drafts": true,
                }),
                Some(&pid_intake),
            )?;
            let _ = world.client_call(
                "github.sync",
                json!({ "project_id": pid_intake }),
                Some(&pid_intake),
            )?;
            world.advance(Duration::from_millis(200))?;

            let intake_cards: Vec<Card> = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .filter(|c| c.project_id == raw_pid_intake)
                .collect();
            ensure!(
                intake_cards.len() == 3,
                "Expected 3 intake-edge cards, got {}",
                intake_cards.len()
            );
            let c1 = intake_cards
                .iter()
                .find(|c| c.github_issue().is_some_and(|s| s.number == 1))
                .unwrap()
                .clone();
            let c2 = intake_cards
                .iter()
                .find(|c| c.github_issue().is_some_and(|s| s.number == 2))
                .unwrap()
                .clone();
            let c3 = intake_cards
                .iter()
                .find(|c| c.github_issue().is_some_and(|s| s.number == 3))
                .unwrap()
                .clone();
            ensure!(c3.is_github_pr(), "PR #3 must be classified as github_pr");
            // Check raw SQLite row for PR #3 keeps canonical `repo#3` title and zero durable `github_issues` rows.
            {
                let srv = world.server("remote-a").unwrap();
                let conn = rusqlite::Connection::open(srv.home.join("canban.sqlite"))?;
                let raw_c3: String =
                    conn.query_row("SELECT data FROM cards WHERE id=?1", [&c3.id], |r| r.get(0))?;
                let persisted_c3: Card = serde_json::from_str(&raw_c3)?;
                ensure!(
                    persisted_c3.title == "acme/intake-edge#3",
                    "Persisted PR source card title must stay canonical repo#number, got {:?}",
                    persisted_c3.title
                );
                let gh_issue_rows: u64 = conn.query_row(
                    "SELECT count(*) FROM github_issues WHERE project_id=?1",
                    [&raw_pid_intake],
                    |r| r.get(0),
                )?;
                ensure!(
                    gh_issue_rows == 0,
                    "Durable SQLite github_issues must stay empty, got {gh_issue_rows}"
                );
                ensure!(
                    srv.snapshot()?.terminals.iter().all(|t| t.agent.is_none()),
                    "@codex in remote GitHub comment must never spawn an Agent"
                );
            }

            // 9a. Local removal suppresses auto-sync; explicit `github.item.add` restores ID, number,
            //     local instructions ("Keep context"), and `card.removed` history.
            let q_c1 = canban_types::qualify_host_id("remote-a", &c1.id);
            let c1_upd = world.client_call(
                "card.update",
                json!({
                    "view": "full",
                    "card_id": q_c1,
                    "revision": c1.revision,
                    "body": "Keep context",
                }),
                Some(&pid_intake),
            )?;
            let c1_rev = c1_upd["revision"].as_u64().unwrap();
            let _ = world.client_call(
                "card.remove",
                json!({ "card_id": q_c1, "revision": c1_rev }),
                Some(&pid_intake),
            )?;
            let _ = world.client_call(
                "github.sync",
                json!({ "project_id": pid_intake }),
                Some(&pid_intake),
            )?;
            ensure!(
                !world
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .cards
                    .iter()
                    .any(|c| c.id == c1.id && c.active()),
                "Local removal must suppress automatic GitHub sync re-import of issue #1"
            );
            let restored_val = world.client_call(
                "github.item.add",
                json!({
                    "view": "full",
                    "project_id": pid_intake,
                    "reference": {
                        "repository": intake_repo,
                        "kind": "issue",
                        "number": 1,
                        "url": format!("https://github.com/{intake_repo}/issues/1"),
                    },
                    "instructions": "",
                }),
                Some(&pid_intake),
            )?;
            let restored_c1: Card = serde_json::from_value(restored_val)?;
            let raw_restored_id = canban_types::split_qualified_host_id(&restored_c1.id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| restored_c1.id.clone());
            ensure!(
                raw_restored_id == c1.id
                    && restored_c1.number == c1.number
                    && restored_c1.body == "Keep context"
                    && restored_c1.active(),
                "Explicit re-add did not restore exact card identity & local context: {restored_c1:?}"
            );
            ensure!(
                world
                    .server("remote-a")
                    .unwrap()
                    .events()?
                    .iter()
                    .any(|e| e.card_id == c1.id && e.kind == "card.removed"),
                "card.removed event history must survive explicit re-add"
            );

            // 9b. Remote closure keeps local stage (`review`), local instructions (`Local requirements`),
            //     canonical SQLite title (`acme/intake-edge#2`), and `archived_at == None`.
            let q_c2 = canban_types::qualify_host_id("remote-a", &c2.id);
            let c2_body = world.client_call(
                "card.update",
                json!({
                    "view": "full",
                    "card_id": q_c2,
                    "revision": c2.revision,
                    "body": "Local requirements",
                }),
                Some(&pid_intake),
            )?;
            let c2_rev = c2_body["revision"].as_u64().unwrap();
            let _ = world.client_call(
                "card.move",
                json!({
                    "view": "full",
                    "card_id": q_c2,
                    "revision": c2_rev,
                    "stage": "review",
                }),
                Some(&pid_intake),
            )?;
            let mut closed_2 = make_issue(2, "Updated remotely", "alice", false);
            closed_2.state = "closed".into();
            closed_2.updated_at = "2026-06-01T03:00:00Z".into();
            world.github.upsert_issue(intake_repo, closed_2);
            let _ = world.client_call(
                "github.sync",
                json!({ "project_id": pid_intake }),
                Some(&pid_intake),
            )?;
            {
                let srv = world.server("remote-a").unwrap();
                let conn = rusqlite::Connection::open(srv.home.join("canban.sqlite"))?;
                let raw_c2: String =
                    conn.query_row("SELECT data FROM cards WHERE id=?1", [&c2.id], |r| r.get(0))?;
                let persisted_c2: Card = serde_json::from_str(&raw_c2)?;
                ensure!(
                    persisted_c2.stage == "review"
                        && persisted_c2.body == "Local requirements"
                        && persisted_c2.title == "acme/intake-edge#2"
                        && persisted_c2.archived_at.is_none(),
                    "Remote issue closure mutated local stage/instructions/archive state: {persisted_c2:?}"
                );
            }

            world.hit("github.intake_suppression_and_closure");
        }

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
