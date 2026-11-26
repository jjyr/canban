//! Scenario #17 — Review, Worktree Merge, GitHub PR Delivery, Drafts Conflict, & Durable Event Subscriptions:
//! Exercises 100% real production code paths across four stateful business domains with ZERO OS
//! child processes spawned (backed exclusively by in-process `SimExec`, `SimGitRepo`, `SimHerdr`,
//! and `SimGitHub`):
//! (A) Review & Worktree Merge (`review.{preview,merge,send_agent,run_agent}`):
//!     git-backed project on `remote-a` with `create_worktree: true`, launching a diff-review
//!     agent (`review.run_agent`), inspecting `review.preview` diff/files/branch/token, steering
//!     review feedback (`review.send_agent` in both `"card"` and `"terminal"` modes via `SimHerdr`),
//!     rejecting stale review tokens, and merging cleanly via `review.merge` with target ref
//!     advancement and idempotent replay verification.
//! (B) GitHub PR Delivery (`github.pr.{publish,refresh,merge}` + `reconcile_remote_pr_merge`):
//!     `SimGitHub`-linked project with `SimGitRepo`, creating worktree-backed cards, publishing
//!     PRs via `github.pr.publish`, surviving transient `500` on `github.pr.refresh` and `409` on
//!     `github.pr.merge`, verifying check-run/review summary projection, merging cleanly via
//!     `github.pr.merge`, and reconciling out-of-band remote merges on `SimGitHub` via both
//!     `github.sync` (`reconcile_remote_pr_merge`) and `github.pr.refresh`.
//! (C) Drafts Revision Conflict & Persistence (`draft.{list,save}`):
//!     creating a draft, verifying idempotent retry & optimistic revision update, rejecting stale
//!     `draft_conflict`, persisting across `bounce_server`, tombstoning with `data: null`, and
//!     rejecting stale resurrection attempts.
//! (D) Durable Event Journal & Subscriptions (`event.list`, `subscription.{save,list,deliveries,ack,retry}`):
//!     registering explicit consumer & workflow subscriptions, listing paginated journal events
//!     via `event.list`, driving a workflow delivery to `failed` via virtual-time backoff, inducing
//!     an abrupt `crash_server` + `bounce_server` before acking/retrying, verifying zero lost or
//!     duplicate `(subscription_id, event_id)` deliveries across restart, retrying the failed
//!     workflow delivery via `subscription.retry` to completion, and acking consumer deliveries via
//!     `subscription.ack`.
use crate::{
    exec::SimGitRepo,
    github::{GitHubFault, SimCheckRun, SimIssue, SimReview},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorld, SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::Card;
use canban_ui::review::model::{
    DraftScopeInput, HydrateAction, MAX_SCOPED_REVIEW_DRAFTS, PendingInlineComment,
    ReviewDraftScope, ReviewDraftSnapshot, ReviewDraftStore, normalize_checkout_path,
    plan_server_draft_hydration, rail_pending_count_pure, resolve_draft_scope,
    switch_draft_scope_pure,
};
use rand::Rng;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;

const REVIEW_WORKTREE_YAML: &str = r#"schema: 12
id: review-worktree-flow
name: Review Worktree Flow
workspace:
  create_worktree: true
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: finish
steps:
  - id: finish
    stage: review
    uses: complete
"#;

const PR_WORKTREE_YAML: &str = r#"schema: 12
id: pr-delivery-worktree-flow
name: PR Delivery Worktree Flow
workspace:
  create_worktree: true
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: finish
steps:
  - id: finish
    stage: review
    uses: complete
"#;

const EVENT_SUB_YAML: &str = r#"schema: 12
id: event-sub-flow
name: Event Subscription Retry Flow
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  on_restore:
    event: manual
    start_at: notify_step
steps:
  - id: notify_step
    stage: develop
    uses: notify
    with:
      message: Retried durable workflow subscription succeeded
    next: done
  - id: done
    stage: review
    uses: complete
"#;

fn strip_host(id: &str) -> String {
    canban_types::split_qualified_host_id(id)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn find_card(world: &SimWorld, host: &str, raw_card_id: &str) -> Result<Card> {
    world
        .server(host)
        .with_context(|| format!("server '{host}' not running"))?
        .snapshot()?
        .cards
        .into_iter()
        .find(|c| c.id == raw_card_id)
        .with_context(|| format!("card '{raw_card_id}' not found on '{host}'"))
}

fn finish_all_live_herdr_terminals(world: &mut SimWorld, host: &str) -> Result<()> {
    let herdr = world
        .herdr(host)
        .with_context(|| format!("herdr '{host}' unavailable"))?;
    let addrs = herdr.terminal_addresses();
    for addr in addrs {
        let is_live = herdr
            .terminal_record(&addr)
            .is_some_and(|rec| rec["state"].as_str() == Some("live"));
        if is_live {
            herdr.finish_terminal(&addr, 0, "sim completed");
        }
    }
    world.wait_until(10_000, |w| {
        let snap = w.server(host).unwrap().snapshot()?;
        let any_active_terminal = snap.terminals.iter().any(|t| {
            t.runtime
                .as_ref()
                .is_some_and(|r| r.state == "running" || r.state == "starting")
                || t.agent
                    .as_ref()
                    .is_some_and(|a| !a.finished() && !a.agent_stopped())
        });
        Ok(!any_active_terminal)
    })?;
    Ok(())
}

fn assert_unique_deliveries(deliveries: &[Value], context: &str) -> Result<()> {
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for d in deliveries {
        let sub_id = d["subscription_id"]
            .as_str()
            .context("delivery missing subscription_id")?
            .to_owned();
        let event_id = d["event_id"]
            .as_str()
            .context("delivery missing event_id")?
            .to_owned();
        ensure!(
            seen.insert((sub_id.clone(), event_id.clone())),
            "{context}: duplicate (subscription_id={sub_id}, event_id={event_id}) delivery!"
        );
    }
    Ok(())
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "review_and_events");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 12;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // Seed-varied deterministic parameters (guarantees distinct canonical digests per seed
        // while keeping identical seeds 100% reproducible).
        let review_commit_count: usize = world.rng.random_range(1..=2);
        let focus_code: u32 = world.rng.random_range(100..=999);
        let draft_rev_note: u32 = world.rng.random_range(10..=99);

        let exec = world
            .exec_runner("remote-a")
            .context("remote-a SimExec unavailable")?;

        // =====================================================================
        // PART A — Review & Worktree Merge (`review.{preview,merge,send_agent,run_agent}`)
        // =====================================================================
        let review_dir = world.project_dir(&format!("review-repo-{seed}"))?;
        exec.register_git_repo(&review_dir, SimGitRepo::default());
        let initial_review_head = exec
            .head_sha(&review_dir)
            .context("SimGitRepo(review) missing initial HEAD")?;

        let proj_rev = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Review Repo {seed}"),
                "directory": review_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_rev = proj_rev["id"]
            .as_str()
            .context("project.create missing id")?
            .to_owned();

        let rev_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_rev }),
            Some(&pid_rev),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_rev,
                "revision": rev_sources["revision"],
                "sources": [{
                    "filename": "review-worktree-flow.yaml",
                    "source": REVIEW_WORKTREE_YAML,
                }],
            }),
            Some(&pid_rev),
        )?;

        let card_a1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_rev,
                "title": format!("Review Feature A1 ({seed})"),
                "body": format!("Verify diff review and clean worktree merge ({focus_code})"),
            }),
            Some(&pid_rev),
        )?;
        let cid_a1 = card_a1["id"]
            .as_str()
            .context("card.create A1 missing id")?
            .to_owned();
        let raw_cid_a1 = strip_host(&cid_a1);
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_a1,
            &strip_host(&pid_rev),
            card_a1["revision"].as_u64().unwrap_or(1),
        );

        // 1. Launch real `review.run_agent`: prepares the Card's Git worktree (`create_worktree: true`)
        //    and enqueues a real `diff_review` agent terminal on `SimHerdr`.
        let run_agent_res = world.client_call(
            "review.run_agent",
            json!({
                "card_id": cid_a1,
                "profile_id": "developer",
                "focus": format!("Check lock ordering and edge cases ({focus_code})"),
            }),
            Some(&pid_rev),
        )?;
        ensure!(
            run_agent_res["state"] == "queued",
            "Expected review.run_agent state=queued, got: {run_agent_res}"
        );

        let a1_review_tid = strip_host(
            run_agent_res["terminal_id"]
                .as_str()
                .context("review.run_agent missing terminal_id")?,
        );
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let card_ready = snap
                .cards
                .iter()
                .find(|c| c.id == raw_cid_a1)
                .is_some_and(|c| c.worktree.is_some() && c.branch.is_some());
            let term_running = snap.terminals.iter().any(|t| {
                t.id == a1_review_tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.state == "running" && a.terminal.is_some())
            });
            Ok(card_ready && term_running)
        })?;

        let a1_snap = find_card(world, "remote-a", &raw_cid_a1)?;
        let a1_wt = a1_snap
            .worktree
            .clone()
            .context("Card A1 missing prepared worktree after review.run_agent")?;
        let mut a1_head = initial_review_head.clone();
        for idx in 0..review_commit_count {
            a1_head = exec.commit_in_worktree(
                &a1_wt,
                &format!("feat(review): commit {} for seed {seed}", idx + 1),
            )?;
        }
        ensure!(
            a1_head != initial_review_head,
            "Card A1 worktree commit must advance HEAD past initial main head"
        );

        // Complete the diff_review agent terminal cleanly on SimHerdr.
        finish_all_live_herdr_terminals(world, "remote-a")?;

        // 2. Call real `review.preview` and verify diff, files, branch, head SHAs, and merge token.
        let preview_1 = world.client_call(
            "review.preview",
            json!({ "card_id": cid_a1 }),
            Some(&pid_rev),
        )?;
        ensure!(
            preview_1["target_branch"] == "main",
            "review.preview target_branch mismatch: {preview_1}"
        );
        ensure!(
            preview_1["target_head"] == initial_review_head,
            "review.preview target_head mismatch: {preview_1}"
        );
        ensure!(
            preview_1["card_head"] == a1_head,
            "review.preview card_head mismatch: {preview_1}"
        );
        ensure!(
            preview_1["merge_allowed"] == true,
            "review.preview must allow merge on clean diverged worktree: {preview_1}"
        );
        let preview_files = preview_1["files"]
            .as_array()
            .context("review.preview missing files array")?;
        ensure!(
            !preview_files.is_empty(),
            "review.preview must report committed diff files: {preview_1}"
        );
        let preview_patch = preview_1["patch"]
            .as_str()
            .context("review.preview missing patch string")?;
        ensure!(
            preview_patch.contains("diff --git"),
            "review.preview must include unified diff patch: {preview_1}"
        );
        let stale_preview_token = preview_1["token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("review.preview missing token")?
            .to_owned();
        world.hit("review.preview_checked");

        // 3. Steer review feedback via real `review.send_agent` (both `card` mode with inline
        //    comments + `move_to_develop`, and `terminal` mode launching a review terminal in SimHerdr).
        let send_card = world.client_call(
            "review.send_agent",
            json!({
                "mode": "card",
                "card_id": cid_a1,
                "profile_id": "developer",
                "move_to_develop": true,
                "context_label": format!("main <- card-a1 ({focus_code})"),
                "summary": format!("Tighten error handling for seed {seed}."),
                "comments": [{
                    "path": preview_files[0].as_str().unwrap_or("src/change_1.rs"),
                    "line": 2,
                    "side": "RIGHT",
                    "snippet": "// sim diff",
                    "body": "Add bounds assertion before merging."
                }],
                "request_key": format!("rev-send-card-{seed}"),
            }),
            Some(&pid_rev),
        )?;
        ensure!(
            send_card["state"] == "sent",
            "review.send_agent(card) expected state=sent: {send_card}"
        );
        ensure!(
            send_card["card_record"]["stage"] == "develop",
            "review.send_agent(card) should move card to develop: {send_card}"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            ensure!(
                snap.questions
                    .iter()
                    .any(|q| q.card_id == raw_cid_a1 && q.profile_id == "developer"),
                "review.send_agent(card) must create a @developer Question"
            );
        }

        let herdr_terms_before = world.herdr("remote-a").unwrap().terminal_addresses().len();
        let send_term = world.client_call(
            "review.send_agent",
            json!({
                "mode": "terminal",
                "project_id": pid_rev,
                "profile_id": "developer",
                "context_label": format!("Diff Review Terminal {focus_code}"),
                "summary": "Inspect worktree diff interactively.",
                "comments": [],
                "request_key": format!("rev-send-term-{seed}"),
            }),
            Some(&pid_rev),
        )?;
        ensure!(
            send_term["state"] == "running",
            "review.send_agent(terminal) expected state=running: {send_term}"
        );
        let herdr_terms_after = world.herdr("remote-a").unwrap().terminal_addresses().len();
        ensure!(
            herdr_terms_after > herdr_terms_before,
            "review.send_agent(terminal) must spawn a review terminal via SimHerdr"
        );
        world.advance(Duration::from_millis(120))?;
        finish_all_live_herdr_terminals(world, "remote-a")?;
        world.hit("review.agent_steered");

        // 4. Verify stale preview token rejection, refresh `review.preview`, and run real
        //    `review.merge` cleanly via `SimExec`/`SimGitRepo`!
        let a1_pre_merge = find_card(world, "remote-a", &raw_cid_a1)?;
        let stale_merge = world.client_call(
            "review.merge",
            json!({
                "card_id": cid_a1,
                "revision": a1_pre_merge.revision,
                "token": stale_preview_token,
                "strategy": "rebase",
                "request_key": format!("rev-merge-stale-{seed}"),
            }),
            Some(&pid_rev),
        );
        ensure!(
            stale_merge.is_err(),
            "review.merge must reject stale preview token after Card revision changed"
        );

        let preview_2 = world.client_call(
            "review.preview",
            json!({ "card_id": cid_a1 }),
            Some(&pid_rev),
        )?;
        let fresh_merge_token = preview_2["token"]
            .as_str()
            .context("refreshed review.preview missing token")?
            .to_owned();
        ensure!(
            fresh_merge_token != stale_preview_token,
            "Refreshed review.preview token must reflect updated Card revision"
        );

        let merge_req = json!({
            "card_id": cid_a1,
            "revision": a1_pre_merge.revision,
            "token": fresh_merge_token,
            "strategy": "rebase",
            "request_key": format!("rev-merge-ok-{seed}"),
        });
        let merged_a1 = world.client_call("review.merge", merge_req.clone(), Some(&pid_rev))?;
        ensure!(
            merged_a1["state"] == "merged",
            "review.merge expected state=merged, got: {merged_a1}"
        );
        ensure!(
            merged_a1["commit"] == a1_head,
            "review.merge commit mismatch: expected {a1_head}, got {merged_a1}"
        );
        ensure!(
            merged_a1["target_branch"] == "main",
            "review.merge target_branch mismatch: {merged_a1}"
        );

        // Idempotent replay of `review.merge` with the same `request_key` returns cached receipt.
        let merged_a1_replay = world.client_call("review.merge", merge_req, Some(&pid_rev))?;
        ensure!(
            merged_a1_replay["state"] == "merged" && merged_a1_replay["commit"] == a1_head,
            "Idempotent review.merge replay diverged: {merged_a1_replay}"
        );

        // Target branch ref in SimGitRepo must have advanced to `a1_head` with ZERO OS spawns!
        ensure!(
            exec.head_sha(&review_dir).as_deref() == Some(a1_head.as_str()),
            "Target checkout HEAD did not advance after review.merge"
        );
        ensure!(
            exec.branch_head_sha(&review_dir, "main").as_deref() == Some(a1_head.as_str()),
            "Target branch refs/heads/main did not advance after review.merge"
        );
        let a1_post_merge = find_card(world, "remote-a", &raw_cid_a1)?;
        ensure!(
            a1_post_merge
                .resolution
                .as_ref()
                .is_some_and(|r| r.kind == "merged"),
            "Card A1 resolution must be merged after review.merge: {:?}",
            a1_post_merge.resolution
        );
        ensure!(
            a1_post_merge
                .delivery
                .as_ref()
                .is_some_and(|d| d.commit == a1_head && d.target_branch == "main"),
            "Card A1 delivery metadata missing after review.merge: {:?}",
            a1_post_merge.delivery
        );
        ensure!(
            exec.blocked_commands().is_empty(),
            "Part A leaked unexpected OS commands: {:?}",
            exec.blocked_commands()
        );
        world.hit("review.merged_cleanly");
        oracle.check_step_invariants(world)?;

        // =====================================================================
        // PART B — GitHub PR Delivery (`github.pr.{publish,refresh,merge}` + `reconcile_remote_pr_merge`)
        // =====================================================================
        let gh_repo = "acme/canban-sim";
        world.github.add_repo(gh_repo, 7701, "main");
        world
            .github
            .set_collaborators(gh_repo, vec!["alice".into(), "bot".into()]);

        let pr_dir = world.project_dir(&format!("pr-repo-{seed}"))?;
        exec.register_git_repo(&pr_dir, SimGitRepo::default());

        let proj_pr = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("GitHub PR Repo {seed}"),
                "directory": pr_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_pr = proj_pr["id"]
            .as_str()
            .context("project.create(PR) missing id")?
            .to_owned();
        let raw_pid_pr = strip_host(&pid_pr);

        let pr_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_pr }),
            Some(&pid_pr),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_pr,
                "revision": pr_sources["revision"],
                "sources": [{
                    "filename": "pr-delivery-worktree-flow.yaml",
                    "source": PR_WORKTREE_YAML,
                }],
            }),
            Some(&pid_pr),
        )?;

        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_pr,
                "repository": gh_repo,
                "intake": "off",
                "pr_intake": "all",
                "include_drafts": true,
                "delivery_mode": "github_pr",
            }),
            Some(&pid_pr),
        )?;
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .projects
                .iter()
                .find(|p| p.id == raw_pid_pr)
                .and_then(|p| p.github.as_ref())
                .and_then(|g| g.synced_at)
                .is_some())
        })?;

        // Helper closure: create a worktree-backed Card via real `worktree.create` + `worktree.promote`.
        let create_wt_card = |world: &mut SimWorld,
                              branch: &str,
                              title: &str|
         -> Result<(String, String, String, u64)> {
            let wt_res = world.client_call(
                "worktree.create",
                json!({
                    "project_id": pid_pr,
                    "branch": branch,
                    "base_branch": "main",
                    "launch": "none",
                }),
                Some(&pid_pr),
            )?;
            let wt_id = wt_res["worktree"]["id"]
                .as_str()
                .context("worktree.create missing worktree.id")?
                .to_owned();
            let wt_path = wt_res["worktree"]["path"]
                .as_str()
                .context("worktree.create missing worktree.path")?
                .to_owned();
            let promoted = world.client_call(
                "worktree.promote",
                json!({
                    "worktree_id": wt_id,
                    "title": title,
                    "body": format!("PR delivery body for {branch}"),
                }),
                Some(&pid_pr),
            )?;
            let cid = promoted["id"]
                .as_str()
                .context("worktree.promote missing id")?
                .to_owned();
            let rev = promoted["revision"].as_u64().unwrap_or(1);
            Ok((cid, wt_path, branch.to_owned(), rev))
        };

        // 1. Card B1: commit -> `review.preview` -> `github.pr.publish`
        let b1_branch = format!("feat/pr-b1-{seed}");
        let (cid_b1, b1_wt, _, b1_rev) =
            create_wt_card(world, &b1_branch, &format!("PR Delivery B1 ({seed})"))?;
        let raw_cid_b1 = strip_host(&cid_b1);
        oracle.record_committed_card("remote-a", &raw_cid_b1, &raw_pid_pr, b1_rev);

        let b1_head = exec.commit_in_worktree(&b1_wt, "feat(pr): deliver card B1")?;
        world.github.set_branch_head(gh_repo, &b1_branch, &b1_head);

        let b1_preview = world.client_call(
            "review.preview",
            json!({ "card_id": cid_b1 }),
            Some(&pid_pr),
        )?;
        let b1_token = b1_preview["token"]
            .as_str()
            .context("Card B1 preview missing token")?
            .to_owned();

        let pub_b1 = world.client_call(
            "github.pr.publish",
            json!({
                "card_id": cid_b1,
                "revision": b1_rev,
                "token": b1_token,
                "request_key": format!("gh-pub-b1-{seed}"),
            }),
            Some(&pid_pr),
        )?;
        ensure!(
            pub_b1["state"] == "published",
            "github.pr.publish(B1) expected state=published: {pub_b1}"
        );
        let pr1_num = pub_b1["number"]
            .as_u64()
            .context("github.pr.publish(B1) missing number")?;
        ensure!(
            pub_b1["head"] == b1_head,
            "github.pr.publish(B1) head mismatch: expected {b1_head}, got {pub_b1}"
        );
        ensure!(
            exec.remote_ref_sha(&pr_dir, "origin", &b1_branch)
                .as_deref()
                == Some(b1_head.as_str()),
            "github.pr.publish must push branch head via SimExec"
        );
        world.hit("github.pr_published");

        // 2. `github.pr.refresh` with check runs + review + transient HTTP 500 resilience.
        world.github.set_check_runs(
            gh_repo,
            &b1_head,
            vec![SimCheckRun {
                id: 101,
                name: "ci/sim-build".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
            }],
        );
        world.github.add_pr_review(
            gh_repo,
            pr1_num,
            SimReview {
                id: 0,
                author: "alice".into(),
                state: "APPROVED".into(),
                body: "LGTM".into(),
                commit_id: b1_head.clone(),
                submitted_at: "2026-06-01T00:30:00Z".into(),
            },
        );

        // Inject transient HTTP 500 on `GET /pulls/{pr1_num}` -> refresh errors cleanly without
        // losing the PR binding, then retry succeeds with green checks & 1 approval!
        world.github.inject_fault_on(
            "GET",
            format!("/pulls/{pr1_num}"),
            GitHubFault::Http {
                status: 500,
                headers: Vec::new(),
                body: Some(r#"{"message":"transient 500"}"#.into()),
            },
        );
        let refresh_500 = world.client_call(
            "github.pr.refresh",
            json!({ "card_id": cid_b1 }),
            Some(&pid_pr),
        );
        ensure!(
            refresh_500.is_err(),
            "Expected transient HTTP 500 to fail github.pr.refresh cleanly"
        );

        let refreshed_b1 = world.client_call(
            "github.pr.refresh",
            json!({ "card_id": cid_b1 }),
            Some(&pid_pr),
        )?;
        ensure!(
            refreshed_b1["state"] == "open",
            "github.pr.refresh(B1) expected state=open: {refreshed_b1}"
        );
        ensure!(
            refreshed_b1["checks"] == "success",
            "github.pr.refresh(B1) expected checks=success: {refreshed_b1}"
        );
        ensure!(
            refreshed_b1["review_summary"]
                .as_str()
                .is_some_and(|s| s.contains("1 approvals")),
            "github.pr.refresh(B1) expected 1 approvals summary: {refreshed_b1}"
        );
        world.hit("github.pr_refreshed");

        // 3. `github.pr.merge` with transient HTTP 409 on `PUT /pulls/{pr1_num}/merge` ->
        //    uncertain `state == "unknown"` + attention -> `github.pr.refresh` -> clean merge!
        let b1_rev_before_merge = find_card(world, "remote-a", &raw_cid_b1)?.revision;
        world.github.inject_fault_on(
            "PUT",
            format!("/pulls/{pr1_num}/merge"),
            GitHubFault::Http {
                status: 409,
                headers: Vec::new(),
                body: Some(r#"{"message":"Head branch conflict (simulated 409)"}"#.into()),
            },
        );
        let merge_409_req = json!({
            "card_id": cid_b1,
            "revision": b1_rev_before_merge,
            "head": b1_head,
            "request_key": format!("gh-merge-b1-409-{seed}"),
        });
        let merge_409 =
            world.client_call("github.pr.merge", merge_409_req.clone(), Some(&pid_pr))?;
        ensure!(
            merge_409["state"] == "unknown",
            "409 on PUT /merge must yield uncertain state=unknown, got: {merge_409}"
        );
        // Replay with identical request_key returns cached uncertain receipt without re-hitting merge.
        let merge_409_replay =
            world.client_call("github.pr.merge", merge_409_req, Some(&pid_pr))?;
        ensure!(
            merge_409_replay["state"] == "unknown",
            "Cached uncertain merge receipt mismatch: {merge_409_replay}"
        );
        ensure!(
            find_card(world, "remote-a", &raw_cid_b1)?
                .attention
                .is_some(),
            "Card B1 should carry attention after 409 uncertain merge"
        );

        // Refresh PR state back to open and retry merge cleanly with a new request_key.
        let _ = world.client_call(
            "github.pr.refresh",
            json!({ "card_id": cid_b1 }),
            Some(&pid_pr),
        )?;
        let b1_rev_retry = find_card(world, "remote-a", &raw_cid_b1)?.revision;
        let merge_b1 = world.client_call(
            "github.pr.merge",
            json!({
                "card_id": cid_b1,
                "revision": b1_rev_retry,
                "head": b1_head,
                "request_key": format!("gh-merge-b1-ok-{seed}"),
            }),
            Some(&pid_pr),
        )?;
        ensure!(
            merge_b1["state"] == "merged",
            "github.pr.merge(B1) retry expected state=merged: {merge_b1}"
        );
        let b1_after_merge = find_card(world, "remote-a", &raw_cid_b1)?;
        ensure!(
            b1_after_merge
                .pull_request
                .as_ref()
                .is_some_and(|pr| pr.merged && pr.state == "closed"),
            "Card B1 pull_request must be merged+closed: {:?}",
            b1_after_merge.pull_request
        );
        ensure!(
            b1_after_merge
                .resolution
                .as_ref()
                .is_some_and(|r| r.kind == "merged"),
            "Card B1 resolution must be merged: {:?}",
            b1_after_merge.resolution
        );
        ensure!(
            b1_after_merge.attention.is_none(),
            "Card B1 attention must clear after successful merge"
        );
        world.hit("github.pr_merged");

        // 4. Out-of-band remote PR merges reconciled via BOTH `github.sync` (`reconcile_remote_pr_merge`)
        //    and `github.pr.refresh`!
        let b2_branch = format!("feat/pr-b2-{seed}");
        let (cid_b2, b2_wt, _, b2_rev) =
            create_wt_card(world, &b2_branch, &format!("PR Remote Merge B2 ({seed})"))?;
        let raw_cid_b2 = strip_host(&cid_b2);
        oracle.record_committed_card("remote-a", &raw_cid_b2, &raw_pid_pr, b2_rev);

        let b2_head = exec.commit_in_worktree(&b2_wt, "feat(pr): deliver card B2")?;
        world.github.set_branch_head(gh_repo, &b2_branch, &b2_head);
        let b2_preview = world.client_call(
            "review.preview",
            json!({ "card_id": cid_b2 }),
            Some(&pid_pr),
        )?;
        let pub_b2 = world.client_call(
            "github.pr.publish",
            json!({
                "card_id": cid_b2,
                "revision": b2_rev,
                "token": b2_preview["token"].as_str().context("B2 token missing")?,
                "request_key": format!("gh-pub-b2-{seed}"),
            }),
            Some(&pid_pr),
        )?;
        ensure!(
            pub_b2["state"] == "published",
            "github.pr.publish(B2) expected state=published: {pub_b2}"
        );
        let pr2_num = pub_b2["number"].as_u64().context("B2 PR number missing")?;

        // Also seed an external GitHub PR #30 on SimGitHub and import it via `github.sync`
        // so `github_pr_intake` exercises `reconcile_remote_pr_merge` directly on sync!
        let pr3_num = 30u64;
        world.github.upsert_issue(
            gh_repo,
            SimIssue {
                number: pr3_num,
                title: format!("External PR Remote Merge ({seed})"),
                body: "Merged out-of-band on GitHub".into(),
                state: "open".into(),
                author: "alice".into(),
                labels: vec!["pr".into()],
                updated_at: "2026-06-01T00:40:00Z".into(),
                is_pr: true,
                draft: false,
                merged: false,
                head_sha: format!("sha-ext-pr-{seed}"),
                head_ref: format!("feat/ext-{seed}"),
                base_ref: "main".into(),
                check_conclusion: Some("success".into()),
            },
        );
        let _ = world.client_call(
            "github.sync",
            json!({ "project_id": pid_pr }),
            Some(&pid_pr),
        )?;
        let raw_cid_b3 = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| {
                c.project_id == raw_pid_pr
                    && c.is_github_pr()
                    && c.github_issue().is_some_and(|s| s.number == pr3_num)
            })
            .context("Expected external PR #30 imported as a Card via github.sync")?
            .id;

        // Merge BOTH PR #2 (delivery PR) and PR #30 (imported PR) directly on SimGitHub!
        let pr2_merge_sha = world
            .github
            .merge_pr_remote(gh_repo, pr2_num)
            .context("merge_pr_remote(PR #2) failed")?;
        let pr3_merge_sha = world
            .github
            .merge_pr_remote(gh_repo, pr3_num)
            .context("merge_pr_remote(PR #30) failed")?;

        // `github.sync` runs `github_pr_intake` -> `reconcile_remote_pr_merge` for PR #30!
        let _ = world.client_call(
            "github.sync",
            json!({ "project_id": pid_pr }),
            Some(&pid_pr),
        )?;
        let b3_reconciled = find_card(world, "remote-a", &raw_cid_b3)?;
        ensure!(
            b3_reconciled
                .delivery
                .as_ref()
                .is_some_and(|d| d.commit == pr3_merge_sha),
            "reconcile_remote_pr_merge on github.sync did not record delivery for PR #30: {:?}",
            b3_reconciled.delivery
        );
        ensure!(
            b3_reconciled
                .resolution
                .as_ref()
                .is_some_and(|r| r.kind == "merged" && r.actor == "github"),
            "reconcile_remote_pr_merge must resolve PR #30 as merged by github: {:?}",
            b3_reconciled.resolution
        );

        // `github.pr.refresh` reconciles delivery PR #2 merged remotely on GitHub!
        let refreshed_b2 = world.client_call(
            "github.pr.refresh",
            json!({ "card_id": cid_b2 }),
            Some(&pid_pr),
        )?;
        ensure!(
            refreshed_b2["merged"] == true && refreshed_b2["state"] == "closed",
            "github.pr.refresh(B2) must observe remote merge: {refreshed_b2}"
        );
        let b2_reconciled = find_card(world, "remote-a", &raw_cid_b2)?;
        ensure!(
            b2_reconciled
                .pull_request
                .as_ref()
                .is_some_and(|pr| pr.merged && pr.state == "closed")
                && b2_reconciled
                    .delivery
                    .as_ref()
                    .is_some_and(|d| d.commit == pr2_merge_sha)
                && b2_reconciled
                    .resolution
                    .as_ref()
                    .is_some_and(|r| r.kind == "merged" && r.actor == "github"),
            "Card B2 remote PR merge was not cleanly reconciled: pr={:?} delivery={:?} resolution={:?}",
            b2_reconciled.pull_request,
            b2_reconciled.delivery,
            b2_reconciled.resolution
        );
        world.hit("github.pr_remote_merge_reconciled");
        oracle.check_step_invariants(world)?;

        // =====================================================================
        // PART B2 — Scoped Review Draft Isolation Across Worktrees, Projects, Hosts, & Cards
        // =====================================================================
        let card_s1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_rev,
                "title": format!("Scoped Draft Card 1 ({seed})"),
                "body": "Card with server-persisted ReviewDraft",
            }),
            Some(&pid_rev),
        )?;
        let cid_s1 = card_s1["id"]
            .as_str()
            .context("card.create S1 missing id")?
            .to_owned();
        let raw_cid_s1 = strip_host(&cid_s1);
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_s1,
            &strip_host(&pid_rev),
            card_s1["revision"].as_u64().unwrap_or(1),
        );

        let card_s2 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_rev,
                "title": format!("Scoped Draft Card 2 ({seed})"),
                "body": "Card without server ReviewDraft",
            }),
            Some(&pid_rev),
        )?;
        let cid_s2 = card_s2["id"]
            .as_str()
            .context("card.create S2 missing id")?
            .to_owned();
        let raw_cid_s2 = strip_host(&cid_s2);
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_s2,
            &strip_host(&pid_rev),
            card_s2["revision"].as_u64().unwrap_or(1),
        );

        let local_scope_dir = world.project_dir(&format!("local-scope-repo-{seed}"))?;
        let proj_local = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Local Scope Repo {seed}"),
                "directory": local_scope_dir,
            }),
            None,
        )?;
        let pid_local = proj_local["id"]
            .as_str()
            .context("project.create(local) missing id")?
            .to_owned();

        let _ = world.client_call(
            "review.draft.set",
            json!({
                "card_id": cid_s1,
                "summary": format!("Server draft summary ({seed})"),
                "verdict": "request_changes",
                "compare_sha": a1_head,
                "comments": [{
                    "id": format!("srv-c1-{seed}"),
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "snippet": "fn check()",
                    "body": format!("Server inline review comment ({seed})"),
                }],
            }),
            Some(&pid_rev),
        )?;

        world.wait_until(15_000, |w| {
            let snap = w.client_merged_snapshot()?;
            Ok(snap.review_drafts.iter().any(|d| d.card_id == cid_s1)
                && !snap.review_drafts.iter().any(|d| d.card_id == cid_s2))
        })?;

        let snap_with_draft = world.client_merged_snapshot()?;
        let server_draft_s1 = snap_with_draft
            .review_drafts
            .iter()
            .find(|d| d.card_id == cid_s1)
            .context("merged_snapshot missing Card S1 ReviewDraft")?
            .clone();

        // 1. Canonical main-checkout normalization & cross-worktree isolation + restoration.
        let mut store = ReviewDraftStore::default();
        let mut active_scope: Option<ReviewDraftScope> = None;
        let mut live = ReviewDraftSnapshot::default();

        let main_scope_empty = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_rev,
            project_dir: Some(&review_dir),
            mode: "git",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: "",
            pr_repository: None,
            pr_number: None,
        });
        let main_scope_explicit = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_rev,
            project_dir: Some(&review_dir),
            mode: "git",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: &format!("{review_dir}/"),
            pr_repository: None,
            pr_number: None,
        });
        ensure!(
            main_scope_empty == main_scope_explicit,
            "Main checkout empty path and project_dir/ must map to the same scope"
        );
        ensure!(
            normalize_checkout_path(&format!("{review_dir}/"), Some(&review_dir)).is_empty(),
            "normalize_checkout_path must collapse trailing-slash project_dir to empty string"
        );

        let initial_switched = switch_draft_scope_pure(
            &mut active_scope,
            &mut live,
            &mut store,
            None,
            main_scope_empty.clone(),
        );
        ensure!(
            !initial_switched,
            "Initial bind without prior scope must not count as a switch"
        );
        live.pending_comments = vec![
            PendingInlineComment {
                id: format!("main-1-{seed}"),
                path: "src/main.rs".into(),
                section: String::new(),
                line: 4,
                side: "RIGHT".into(),
                snippet: String::new(),
                body: "Main checkout comment 1".into(),
                author: None,
                edited_by_user: false,
            },
            PendingInlineComment {
                id: format!("main-2-{seed}"),
                path: "src/main.rs".into(),
                section: String::new(),
                line: 9,
                side: "RIGHT".into(),
                snippet: String::new(),
                body: "Main checkout comment 2".into(),
                author: None,
                edited_by_user: false,
            },
        ];
        live.submit_summary = format!("Main checkout notes ({seed})");
        ensure!(
            rail_pending_count_pure(
                active_scope.as_ref(),
                live.pending_comments.len(),
                &store,
                "remote-a",
                &pid_rev,
                "review",
            ) == 2,
            "Rail badge must show 2 pending comments on main checkout"
        );

        // Switching to another worktree clears active comments while preserving main's bucket.
        let wt_scope = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_rev,
            project_dir: Some(&review_dir),
            mode: "git",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: &a1_wt,
            pr_repository: None,
            pr_number: None,
        });
        ensure!(
            switch_draft_scope_pure(
                &mut active_scope,
                &mut live,
                &mut store,
                None,
                wt_scope.clone()
            ),
            "Switching from main checkout to worktree must switch draft scope"
        );
        ensure!(
            live.pending_comments.is_empty() && live.submit_summary.is_empty(),
            "Worktree switch leaked previous checkout's pending comments or summary"
        );
        ensure!(
            rail_pending_count_pure(
                active_scope.as_ref(),
                live.pending_comments.len(),
                &store,
                "remote-a",
                &pid_rev,
                "review",
            ) == 0,
            "Rail badge must drop to 0 on empty worktree"
        );

        live.pending_comments.push(PendingInlineComment {
            id: format!("wt-1-{seed}"),
            path: "src/wt.rs".into(),
            section: String::new(),
            line: 7,
            side: "RIGHT".into(),
            snippet: String::new(),
            body: "Worktree-only comment".into(),
            author: None,
            edited_by_user: false,
        });

        // Returning to main checkout via full `project_dir/` restores main's 2 comments & summary.
        ensure!(
            switch_draft_scope_pure(
                &mut active_scope,
                &mut live,
                &mut store,
                None,
                main_scope_explicit.clone(),
            ),
            "Switching back to main checkout must return switched=true"
        );
        ensure!(
            live.pending_comments.len() == 2
                && live.submit_summary == format!("Main checkout notes ({seed})"),
            "Returning to main checkout did not restore saved comments/summary"
        );

        // 2. Cross-project and cross-host isolation (including Board-level project switch rail badge).
        ensure!(
            rail_pending_count_pure(
                active_scope.as_ref(),
                live.pending_comments.len(),
                &store,
                "remote-a",
                &pid_pr,
                "board",
            ) == 0,
            "Switching project on Board must immediately isolate Rail pending comment badge"
        );
        let pr_proj_scope = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_pr,
            project_dir: Some(&pr_dir),
            mode: "git",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: "",
            pr_repository: None,
            pr_number: None,
        });
        ensure!(
            switch_draft_scope_pure(
                &mut active_scope,
                &mut live,
                &mut store,
                None,
                pr_proj_scope
            ),
            "Switching to another Project must switch draft scope"
        );
        ensure!(
            live.pending_comments.is_empty() && live.submit_summary.is_empty(),
            "Cross-project switch leaked previous project's comments"
        );

        let local_checkout_scope = resolve_draft_scope(DraftScopeInput {
            host: "local",
            project_id: &pid_local,
            project_dir: Some(&local_scope_dir),
            mode: "git",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: "/shared/same-rel",
            pr_repository: None,
            pr_number: None,
        });
        let remote_same_rel_scope = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_local,
            project_dir: Some(&local_scope_dir),
            mode: "git",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: "/shared/same-rel",
            pr_repository: None,
            pr_number: None,
        });
        ensure!(
            local_checkout_scope != remote_same_rel_scope,
            "Identical relative checkout paths on different hosts must stay distinct"
        );

        // 3. Card draft hydration & zero cross-card leakage (plus server clear detection).
        let card1_scope = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_rev,
            project_dir: Some(&review_dir),
            mode: "card_local",
            review_card_id: &cid_s1,
            ui_card_id: &cid_s1,
            card_host_project: Some(("remote-a", &pid_rev)),
            checkout_cwd: "",
            pr_repository: None,
            pr_number: None,
        });
        ensure!(
            switch_draft_scope_pure(
                &mut active_scope,
                &mut live,
                &mut store,
                None,
                card1_scope.clone()
            ),
            "Switching to Card S1 must switch draft scope"
        );
        match plan_server_draft_hydration(
            active_scope.as_ref(),
            &live.loaded_draft_key,
            &cid_s1,
            Some(&server_draft_s1),
            true,
        ) {
            HydrateAction::Apply(snap) => live = snap,
            other => anyhow::bail!("Expected HydrateAction::Apply for Card S1, got: {other:?}"),
        }
        ensure!(
            live.pending_comments.len() == 1
                && live.submit_gh_event == "REQUEST_CHANGES"
                && live.draft_anchor_sha.as_deref() == Some(a1_head.as_str()),
            "Card S1 hydration mismatch: {live:?}"
        );

        // Direct guard: hydrating Card S2 (no server draft) while Card S1 was previously loaded must clear!
        match plan_server_draft_hydration(
            Some(&card1_scope),
            &live.loaded_draft_key,
            &cid_s2,
            None,
            true,
        ) {
            HydrateAction::Apply(cleared) => ensure!(
                cleared.pending_comments.is_empty()
                    && cleared.submit_summary.is_empty()
                    && cleared.loaded_draft_key == (cid_s2.clone(), 0),
                "Expected empty cleared draft for Card S2"
            ),
            other => {
                anyhow::bail!("Expected HydrateAction::Apply(empty) for Card S2, got: {other:?}")
            }
        }

        let card2_scope = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_rev,
            project_dir: Some(&review_dir),
            mode: "card_local",
            review_card_id: &cid_s2,
            ui_card_id: &cid_s2,
            card_host_project: Some(("remote-a", &pid_rev)),
            checkout_cwd: "",
            pr_repository: None,
            pr_number: None,
        });
        ensure!(
            switch_draft_scope_pure(&mut active_scope, &mut live, &mut store, None, card2_scope),
            "Switching from Card S1 to Card S2 must switch draft scope"
        );
        ensure!(
            live.pending_comments.is_empty() && live.submit_summary.is_empty(),
            "Card S1 review draft leaked into Card S2"
        );

        // Switch back to Card S1, then clear its server draft via real RPC and verify non-forced hydration clears it.
        ensure!(
            switch_draft_scope_pure(
                &mut active_scope,
                &mut live,
                &mut store,
                None,
                card1_scope.clone()
            ),
            "Returning to Card S1 must switch draft scope"
        );
        ensure!(
            live.pending_comments.len() == 1,
            "Returning to Card S1 lost its saved comment"
        );
        let _ = world.client_call(
            "review.draft.clear",
            json!({ "card_id": cid_s1 }),
            Some(&pid_rev),
        )?;
        world.wait_until(15_000, |w| {
            let snap = w.client_merged_snapshot()?;
            Ok(!snap.review_drafts.iter().any(|d| d.card_id == cid_s1))
        })?;
        match plan_server_draft_hydration(
            active_scope.as_ref(),
            &live.loaded_draft_key,
            &cid_s1,
            None,
            false,
        ) {
            HydrateAction::Apply(cleared) => live = cleared,
            other => anyhow::bail!(
                "Expected HydrateAction::Apply(empty) after review.draft.clear, got: {other:?}"
            ),
        }
        ensure!(
            live.pending_comments.is_empty() && live.submit_summary.is_empty(),
            "Server-side review.draft.clear did not clear Card S1 live draft"
        );

        // 4. Standalone PR scope isolation & rail visibility rules.
        let standalone_pr_scope = resolve_draft_scope(DraftScopeInput {
            host: "remote-a",
            project_id: &pid_pr,
            project_dir: Some(&pr_dir),
            mode: "github_pr",
            review_card_id: "",
            ui_card_id: "",
            card_host_project: None,
            checkout_cwd: "",
            pr_repository: Some(gh_repo),
            pr_number: Some(999),
        });
        ensure!(
            !standalone_pr_scope.is_foreign_project("local", "unrelated")
                && !standalone_pr_scope.matches_project("remote-a", &pid_pr),
            "StandalonePr must not trigger foreign project reset or spoof project checkout badge"
        );
        ensure!(
            switch_draft_scope_pure(
                &mut active_scope,
                &mut live,
                &mut store,
                None,
                standalone_pr_scope.clone(),
            ),
            "Opening StandalonePr must switch scope"
        );
        live.pending_comments.push(PendingInlineComment {
            id: format!("pr-standalone-{seed}"),
            path: "README.md".into(),
            section: String::new(),
            line: 1,
            side: "RIGHT".into(),
            snippet: String::new(),
            body: "Standalone PR comment".into(),
            author: None,
            edited_by_user: false,
        });
        ensure!(
            rail_pending_count_pure(
                active_scope.as_ref(),
                live.pending_comments.len(),
                &store,
                "remote-a",
                &pid_pr,
                "review",
            ) == 1
                && rail_pending_count_pure(
                    active_scope.as_ref(),
                    live.pending_comments.len(),
                    &store,
                    "remote-a",
                    &pid_pr,
                    "board",
                ) == 0,
            "StandalonePr rail badge must only appear on review screen"
        );

        // 5. Async submit fencing (clearing off-screen submitted scope leaves active scope untouched)
        //    and bounded LRU capacity guard.
        store.clear_scope(&wt_scope);
        ensure!(
            store.peek(&wt_scope).is_none()
                && store
                    .peek(&main_scope_empty)
                    .is_some_and(|d| d.pending_comments.len() == 2)
                && live.pending_comments.len() == 1,
            "Clearing off-screen submitted scope disturbed unrelated or active scopes"
        );

        let mut cap_store = ReviewDraftStore::default();
        for idx in 0..(MAX_SCOPED_REVIEW_DRAFTS + 4) {
            cap_store.save(
                ReviewDraftScope::Checkout {
                    host: "local".into(),
                    project_id: format!("proj-{idx}"),
                    cwd: String::new(),
                },
                ReviewDraftSnapshot {
                    submit_summary: format!("draft-{idx}"),
                    ..Default::default()
                },
            );
        }
        ensure!(
            cap_store.len() <= MAX_SCOPED_REVIEW_DRAFTS,
            "ReviewDraftStore exceeded MAX_SCOPED_REVIEW_DRAFTS ({})",
            cap_store.len()
        );

        world.hit("review.draft_scope_isolated");
        oracle.check_step_invariants(world)?;

        // =====================================================================
        // PART C — Drafts Revision Conflict & Persistence (`draft.{list,save}`)
        // =====================================================================
        let draft_id = format!("new-card:sim-draft-{seed}");
        let draft_v1 = json!({
            "title": format!("Draft Title {seed}"),
            "body": format!("initial draft v1 ({draft_rev_note})"),
        });
        let draft_save_1 = json!({
            "host_id": "remote-a",
            "id": draft_id,
            "revision": 0,
            "data": draft_v1,
        });
        let saved_d1 = world.client_call("draft.save", draft_save_1.clone(), Some(&pid_rev))?;
        ensure!(
            saved_d1["revision"] == 1,
            "draft.save initial create expected revision=1: {saved_d1}"
        );
        // Identical retry at revision 0 is idempotent (`drafts.rs:46-48`).
        let saved_d1_retry = world.client_call("draft.save", draft_save_1, Some(&pid_rev))?;
        ensure!(
            saved_d1_retry["revision"] == 1,
            "draft.save identical retry expected revision=1: {saved_d1_retry}"
        );

        // Update with matching revision 1 -> revision 2.
        let draft_v2 = json!({
            "title": format!("Draft Title {seed} (edited)"),
            "body": format!("updated draft v2 ({draft_rev_note})"),
        });
        let saved_d2 = world.client_call(
            "draft.save",
            json!({
                "host_id": "remote-a",
                "id": draft_id,
                "revision": 1,
                "data": draft_v2,
            }),
            Some(&pid_rev),
        )?;
        ensure!(
            saved_d2["revision"] == 2,
            "draft.save update expected revision=2: {saved_d2}"
        );

        // Stale-revision update (`revision: 1`, distinct payload) MUST fail with `draft_conflict`!
        let stale_draft_err = world
            .client_call(
                "draft.save",
                json!({
                    "host_id": "remote-a",
                    "id": draft_id,
                    "revision": 1,
                    "data": { "title": "Stale window overwrite", "body": "must fail" },
                }),
                Some(&pid_rev),
            )
            .expect_err("Stale draft.save must be rejected with draft_conflict");
        ensure!(
            stale_draft_err.to_string().contains("draft_conflict"),
            "Expected draft_conflict error, got: {stale_draft_err:#}"
        );

        // Bounce `remote-a` and verify `draft.list` persists revision 2 & exact payload!
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        let drafts_after_bounce = world.client_call(
            "draft.list",
            json!({ "host_id": "remote-a" }),
            Some(&pid_rev),
        )?;
        let persisted = drafts_after_bounce
            .as_array()
            .context("draft.list must return an array")?
            .iter()
            .find(|d| d["id"] == draft_id)
            .context("Draft disappeared across bounce_server!")?;
        ensure!(
            persisted["revision"] == 2 && persisted["data"] == draft_v2,
            "Persisted draft diverged across bounce_server: {persisted}"
        );

        // Tombstone/delete draft with `data: null` at matching revision 2 -> revision 3,
        // and verify stale writes cannot resurrect a discarded tombstone!
        let tombstoned = world.client_call(
            "draft.save",
            json!({
                "host_id": "remote-a",
                "id": draft_id,
                "revision": 2,
                "data": Value::Null,
            }),
            Some(&pid_rev),
        )?;
        ensure!(
            tombstoned["revision"] == 3,
            "draft.save tombstone expected revision=3: {tombstoned}"
        );
        let resurrect_err = world
            .client_call(
                "draft.save",
                json!({
                    "host_id": "remote-a",
                    "id": draft_id,
                    "revision": 2,
                    "data": draft_v2,
                }),
                Some(&pid_rev),
            )
            .expect_err("Stale draft.save must not resurrect tombstoned draft");
        ensure!(
            resurrect_err.to_string().contains("draft_conflict"),
            "Expected draft_conflict on tombstone resurrection, got: {resurrect_err:#}"
        );
        let drafts_after_tombstone = world.client_call(
            "draft.list",
            json!({ "host_id": "remote-a" }),
            Some(&pid_rev),
        )?;
        let tomb_entry = drafts_after_tombstone
            .as_array()
            .context("draft.list array")?
            .iter()
            .find(|d| d["id"] == draft_id)
            .context("Tombstoned draft entry missing from draft.list")?;
        ensure!(
            tomb_entry["revision"] == 3 && tomb_entry["data"].is_null(),
            "Expected tombstoned draft at revision=3 with data=null, got: {tomb_entry}"
        );
        world.hit("drafts.conflict_and_persistence");
        oracle.check_step_invariants(world)?;

        // =====================================================================
        // PART D — Durable Event Journal & Subscriptions
        //          (`event.list`, `subscription.{save,list,deliveries,ack,retry}`)
        // =====================================================================
        let ev_dir = world.project_dir(&format!("events-proj-{seed}"))?;
        let proj_ev = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Events Proj {seed}"),
                "directory": ev_dir,
            }),
            None,
        )?;
        let pid_ev = proj_ev["id"]
            .as_str()
            .context("project.create(events) missing id")?
            .to_owned();
        let raw_pid_ev = strip_host(&pid_ev);

        let ev_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_ev }),
            Some(&pid_ev),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_ev,
                "revision": ev_sources["revision"],
                "sources": [{
                    "filename": "event-sub-flow.yaml",
                    "source": EVENT_SUB_YAML,
                }],
            }),
            Some(&pid_ev),
        )?;

        let card_d1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_ev,
                "title": format!("Event Consumer Card ({seed})"),
                "body": "Drives consumer subscription deliveries",
            }),
            Some(&pid_ev),
        )?;
        let cid_d1 = card_d1["id"].as_str().context("D1 id missing")?.to_owned();
        let raw_cid_d1 = strip_host(&cid_d1);
        oracle.record_committed_card(
            "remote-a",
            &raw_cid_d1,
            &raw_pid_ev,
            card_d1["revision"].as_u64().unwrap_or(1),
        );

        let card_d2 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_ev,
                "title": format!("Event Retry Card ({seed})"),
                "body": "Drives failed workflow delivery + crash + subscription.retry",
            }),
            Some(&pid_ev),
        )?;
        let cid_d2 = card_d2["id"].as_str().context("D2 id missing")?.to_owned();
        let raw_cid_d2 = strip_host(&cid_d2);
        let d2_rev = card_d2["revision"].as_u64().unwrap_or(1);
        oracle.record_committed_card("remote-a", &raw_cid_d2, &raw_pid_ev, d2_rev);

        // 1. Register durable consumer subscription & workflow subscription via `subscription.save`.
        let sub_consumer = world.client_call(
            "subscription.save",
            json!({
                "host_id": "remote-a",
                "revision": 0,
                "enabled": true,
                "selector": {
                    "card_id": cid_d1,
                    "types": ["comment.*"],
                },
                "target": { "type": "consumer" },
            }),
            Some(&pid_ev),
        )?;
        let sub_consumer_id = sub_consumer["id"]
            .as_str()
            .context("consumer subscription missing id")?
            .to_owned();
        ensure!(
            sub_consumer["revision"] == 1 && sub_consumer["enabled"] == true,
            "Unexpected consumer subscription receipt: {sub_consumer}"
        );

        // Verify stale revision conflict on `subscription.save`.
        let sub_conflict = world
            .client_call(
                "subscription.save",
                json!({
                    "host_id": "remote-a",
                    "subscription_id": sub_consumer_id,
                    "revision": 0,
                    "enabled": true,
                    "selector": { "card_id": cid_d1, "types": ["comment.*"] },
                    "target": { "type": "consumer" },
                }),
                Some(&pid_ev),
            )
            .expect_err("Stale subscription.save revision must be rejected");
        ensure!(
            sub_conflict.to_string().contains("revision_conflict"),
            "Expected revision_conflict on stale subscription.save, got: {sub_conflict:#}"
        );

        let sub_workflow = world.client_call(
            "subscription.save",
            json!({
                "host_id": "remote-a",
                "revision": 0,
                "enabled": true,
                "selector": {
                    "card_id": cid_d2,
                    "types": ["card.archived"],
                },
                "target": {
                    "type": "workflow",
                    "workflow_id": "event-sub-flow",
                    "entry": "on_restore",
                },
            }),
            Some(&pid_ev),
        )?;
        let sub_workflow_id = sub_workflow["id"]
            .as_str()
            .context("workflow subscription missing id")?
            .to_owned();

        // Confirm both subscriptions appear in `subscription.list`.
        let sub_list = world.client_call(
            "subscription.list",
            json!({ "host_id": "remote-a" }),
            Some(&pid_ev),
        )?;
        let sub_arr = sub_list
            .as_array()
            .context("subscription.list must return an array")?;
        ensure!(
            sub_arr
                .iter()
                .any(|s| s["id"] == sub_consumer_id && s["enabled"] == true)
                && sub_arr
                    .iter()
                    .any(|s| s["id"] == sub_workflow_id && s["enabled"] == true),
            "subscription.list missing registered subscriptions: {sub_list}"
        );

        // 2. Emit durable events: 2 comments on Card D1 + archive Card D2.
        let comment_1 = format!("durable-event-comment-1-{seed}");
        let comment_2 = format!("durable-event-comment-2-{seed}");
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_d1, "body": comment_1 }),
            Some(&pid_ev),
        )?;
        oracle.record_committed_comment("remote-a", &raw_cid_d1, comment_1);
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_d1, "body": comment_2 }),
            Some(&pid_ev),
        )?;
        oracle.record_committed_comment("remote-a", &raw_cid_d1, comment_2);

        let _ = world.client_call(
            "card.archive",
            json!({ "card_id": cid_d2, "revision": d2_rev }),
            Some(&pid_ev),
        )?;

        // 3. Query durable event journal via `event.list` (filtered + paginated cursor check).
        let journal_d1 = world.client_call(
            "event.list",
            json!({
                "host_id": "remote-a",
                "after": 0,
                "limit": 500,
                "selector": { "card_id": cid_d1, "types": ["comment.*"] },
            }),
            Some(&pid_ev),
        )?;
        let d1_events = journal_d1["events"]
            .as_array()
            .context("event.list missing events array")?;
        ensure!(
            d1_events.len() == 2,
            "Expected 2 journaled comment.* events on Card D1, got: {journal_d1}"
        );
        let first_offset = d1_events[0]["offset"]
            .as_u64()
            .context("event offset missing")?;
        let second_offset = d1_events[1]["offset"]
            .as_u64()
            .context("second event offset missing")?;
        let highwater = journal_d1["highwater"]
            .as_u64()
            .context("event.list highwater missing")?;
        ensure!(
            second_offset > first_offset && highwater >= second_offset,
            "Journal offsets must be strictly increasing and <= highwater: {journal_d1}"
        );
        let journal_page2 = world.client_call(
            "event.list",
            json!({
                "host_id": "remote-a",
                "after": first_offset,
                "limit": 500,
                "selector": { "card_id": cid_d1, "types": ["comment.*"] },
            }),
            Some(&pid_ev),
        )?;
        ensure!(
            journal_page2["events"]
                .as_array()
                .is_some_and(|evs| evs.len() == 1 && evs[0]["offset"] == second_offset),
            "Paginated event.list(after={first_offset}) must return only the second comment event: {journal_page2}"
        );
        world.hit("events.journal_listed");

        // 4. Wait until:
        //    - Consumer subscription has 2 pending deliveries (unacked!)
        //    - Workflow subscription delivery exhausts 5 backoff attempts on archived Card D2
        //      and transitions to `state == "failed"`.
        world.wait_until(60_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let c_deliv = srv.call(
                "subscription.deliveries",
                json!({ "subscription_id": sub_consumer_id }),
            )?;
            let w_deliv = srv.call(
                "subscription.deliveries",
                json!({ "subscription_id": sub_workflow_id }),
            )?;
            let c_ready = c_deliv
                .as_array()
                .is_some_and(|arr| arr.len() == 2 && arr.iter().all(|d| d["state"] == "pending"));
            let w_failed = w_deliv.as_array().is_some_and(|arr| {
                arr.len() == 1
                    && arr[0]["state"] == "failed"
                    && arr[0]["attempts"].as_u64() == Some(5)
            });
            Ok(c_ready && w_failed)
        })?;

        let pre_crash_consumer = world.client_call(
            "subscription.deliveries",
            json!({ "host_id": "remote-a", "subscription_id": sub_consumer_id }),
            Some(&pid_ev),
        )?;
        let pre_crash_consumer_arr = pre_crash_consumer
            .as_array()
            .context("pre-crash consumer deliveries array")?;
        assert_unique_deliveries(pre_crash_consumer_arr, "pre-crash consumer")?;
        let pre_crash_consumer_ids: BTreeSet<String> = pre_crash_consumer_arr
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        ensure!(pre_crash_consumer_ids.len() == 2);

        let pre_crash_workflow = world.client_call(
            "subscription.deliveries",
            json!({ "host_id": "remote-a", "subscription_id": sub_workflow_id }),
            Some(&pid_ev),
        )?;
        let pre_crash_wf_arr = pre_crash_workflow
            .as_array()
            .context("pre-crash workflow deliveries array")?;
        assert_unique_deliveries(pre_crash_wf_arr, "pre-crash workflow")?;
        let failed_delivery_id = pre_crash_wf_arr[0]["id"]
            .as_str()
            .context("failed delivery missing id")?
            .to_owned();
        ensure!(
            pre_crash_wf_arr[0]["error"]
                .as_str()
                .is_some_and(|e| e.contains("Restore the Card")),
            "Expected archived-card failure reason on workflow delivery, got: {}",
            pre_crash_wf_arr[0]
        );

        // 5. Abruptly SIGKILL (`crash_server`) + `bounce_server` BEFORE acking or retrying!
        world.crash_server("remote-a")?;
        world.advance(Duration::from_millis(200))?;
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;

        // Verify ALL pending consumer deliveries & failed workflow delivery survived crash intact
        // with zero lost events and zero duplicate `(subscription_id, event_id)` deliveries!
        let post_crash_consumer = world.client_call(
            "subscription.deliveries",
            json!({ "host_id": "remote-a", "subscription_id": sub_consumer_id }),
            Some(&pid_ev),
        )?;
        let post_crash_consumer_arr = post_crash_consumer
            .as_array()
            .context("post-crash consumer deliveries array")?;
        assert_unique_deliveries(post_crash_consumer_arr, "post-crash consumer")?;
        let post_crash_consumer_ids: BTreeSet<String> = post_crash_consumer_arr
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        ensure!(
            post_crash_consumer_ids == pre_crash_consumer_ids,
            "Pending consumer deliveries changed across abrupt crash/bounce: before={pre_crash_consumer_ids:?} after={post_crash_consumer_ids:?}"
        );
        ensure!(
            post_crash_consumer_arr
                .iter()
                .all(|d| d["state"] == "pending"),
            "Pending consumer deliveries lost pending state across crash: {post_crash_consumer}"
        );

        let post_crash_workflow = world.client_call(
            "subscription.deliveries",
            json!({ "host_id": "remote-a", "subscription_id": sub_workflow_id }),
            Some(&pid_ev),
        )?;
        let post_crash_wf_arr = post_crash_workflow
            .as_array()
            .context("post-crash workflow deliveries array")?;
        assert_unique_deliveries(post_crash_wf_arr, "post-crash workflow")?;
        ensure!(
            post_crash_wf_arr.len() == 1
                && post_crash_wf_arr[0]["id"] == failed_delivery_id
                && post_crash_wf_arr[0]["state"] == "failed",
            "Failed workflow delivery did not survive crash intact: {post_crash_workflow}"
        );

        // 6. Restore Card D2 and exercise real `subscription.retry` on the failed delivery!
        let d2_archived_rev = find_card(world, "remote-a", &raw_cid_d2)?.revision;
        let _ = world.client_call(
            "card.restore",
            json!({ "card_id": cid_d2, "revision": d2_archived_rev }),
            Some(&pid_ev),
        )?;
        let retried = world.client_call(
            "subscription.retry",
            json!({ "host_id": "remote-a", "delivery_id": failed_delivery_id }),
            Some(&pid_ev),
        )?;
        ensure!(
            retried["id"] == failed_delivery_id
                && (retried["state"] == "pending" || retried["state"] == "completed"),
            "Unexpected subscription.retry response: {retried}"
        );

        // Wait for the retried workflow delivery to execute and converge to `completed`!
        world.wait_until(15_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let list = srv.call(
                "subscription.deliveries",
                json!({ "subscription_id": sub_workflow_id }),
            )?;
            let deliv_done = list.as_array().is_some_and(|arr| {
                arr.len() == 1
                    && arr[0]["id"] == failed_delivery_id
                    && arr[0]["state"] == "completed"
                    && arr[0]["workflow_run_id"].is_string()
            });
            if !deliv_done {
                return Ok(false);
            }
            let run_id = list[0]["workflow_run_id"].as_str().unwrap().to_owned();
            let snap = srv.snapshot()?;
            let run_done = snap
                .workflow_runs
                .iter()
                .any(|r| r.id == run_id && r.state == "completed");
            Ok(run_done)
        })?;
        world.hit("events.subscription_crash_retry");

        // Emit a 3rd comment AFTER abrupt crash/bounce to prove the persisted subscription cursor
        // advances monotonically without rewinding or duplicating pre-crash deliveries!
        let comment_3 = format!("durable-event-comment-3-post-crash-{seed}");
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_d1, "body": comment_3 }),
            Some(&pid_ev),
        )?;
        oracle.record_committed_comment("remote-a", &raw_cid_d1, comment_3);
        world.wait_until(15_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let c_deliv = srv.call(
                "subscription.deliveries",
                json!({ "subscription_id": sub_consumer_id }),
            )?;
            Ok(c_deliv
                .as_array()
                .is_some_and(|arr| arr.len() == 3 && arr.iter().all(|d| d["state"] == "pending")))
        })?;
        let post_crash_all_consumer = world.client_call(
            "subscription.deliveries",
            json!({ "host_id": "remote-a", "subscription_id": sub_consumer_id }),
            Some(&pid_ev),
        )?;
        let post_crash_all_arr = post_crash_all_consumer
            .as_array()
            .context("post-crash 3rd-comment consumer deliveries array")?;
        assert_unique_deliveries(post_crash_all_arr, "post-crash + new event consumer")?;
        let post_crash_all_ids: BTreeSet<String> = post_crash_all_arr
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        ensure!(
            post_crash_all_ids.len() == 3 && pre_crash_consumer_ids.is_subset(&post_crash_all_ids),
            "Expected 2 surviving + 1 new unique pending deliveries after reboot, got: {post_crash_all_consumer}"
        );

        // 7. Ack all 3 pending consumer deliveries via real `subscription.ack` and verify
        //    clean convergence with zero duplicate deliveries!
        for did in &post_crash_all_ids {
            let acked = world.client_call(
                "subscription.ack",
                json!({ "host_id": "remote-a", "delivery_id": did }),
                Some(&pid_ev),
            )?;
            ensure!(
                acked["id"] == *did && acked["state"] == "completed",
                "subscription.ack({did}) expected state=completed, got: {acked}"
            );
        }
        let final_consumer_deliv = world.client_call(
            "subscription.deliveries",
            json!({ "host_id": "remote-a", "subscription_id": sub_consumer_id }),
            Some(&pid_ev),
        )?;
        let final_consumer_arr = final_consumer_deliv
            .as_array()
            .context("final consumer deliveries array")?;
        assert_unique_deliveries(final_consumer_arr, "final consumer")?;
        ensure!(
            final_consumer_arr.len() == 3
                && final_consumer_arr.iter().all(|d| d["state"] == "completed"),
            "Expected 3 completed consumer deliveries after ack, got: {final_consumer_deliv}"
        );
        world.hit("events.subscription_delivery_and_ack");

        // Final global quiesce, oracle verification, and zero-OS-spawn check.
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        for (host, handle) in &world.servers {
            ensure!(
                handle.exec.blocked_commands().is_empty(),
                "[{host}] unexpected host OS commands blocked during scenario: {:?}",
                handle.exec.blocked_commands()
            );
        }

        Ok(finalize_digest(world))
    })
}
