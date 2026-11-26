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
use canban_types::{Card, Event, ExecutionTrigger, Note, NoteEvent};
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

        // =====================================================================
        // PART E — Comments (`comment.{update,delete}`), Notes (`note.{write,read,revision,history,rename}`),
        //          Tags (`tag.create`), Card-Create Mentions & Prompt Dedup, Profile Reload
        //          Immutability, & Review Activity Warnings (persisted across `bounce_server`)
        // =====================================================================
        finish_all_live_herdr_terminals(world, "remote-a")?;
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 4 }),
        )?;
        let cnt_dir = world.project_dir(&format!("cnt-proj-{seed}"))?;
        let foreign_dir = world.project_dir(&format!("cnt-foreign-{seed}"))?;
        let proj_cnt = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Comments Notes Tags {seed}"),
                "directory": cnt_dir,
                "workspace": { "create_worktree": false, "sync_base": false },
            }),
            None,
        )?;
        let pid_cnt = proj_cnt["id"].as_str().context("proj_cnt id")?.to_owned();
        let raw_pid_cnt = strip_host(&pid_cnt);

        let proj_foreign = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Foreign Scope {seed}"),
                "directory": foreign_dir,
                "workspace": { "create_worktree": false, "sync_base": false },
            }),
            None,
        )?;
        let pid_foreign = proj_foreign["id"]
            .as_str()
            .context("proj_foreign id")?
            .to_owned();

        // E1. Project Tags (`tag.create`: normalize, case-insensitive dedup, Unicode/CJK, invalid rejection).
        let tag_ui = world.client_call(
            "tag.create",
            json!({ "project_id": pid_cnt, "tag": "#ui-kit" }),
            Some(&pid_cnt),
        )?;
        ensure!(
            tag_ui["tag"] == "ui-kit" && tag_ui["tags"] == json!(["ui-kit"]),
            "tag.create #ui-kit mismatch: {tag_ui}"
        );
        let tag_dup = world.client_call(
            "tag.create",
            json!({ "project_id": pid_cnt, "tag": "UI-KIT" }),
            Some(&pid_cnt),
        )?;
        ensure!(
            tag_dup["tag"] == "ui-kit" && tag_dup["tags"] == json!(["ui-kit"]),
            "tag.create case-insensitive dedup mismatch: {tag_dup}"
        );
        let tag_cjk = world.client_call(
            "tag.create",
            json!({ "project_id": pid_cnt, "tag": "#前端" }),
            Some(&pid_cnt),
        )?;
        ensure!(
            tag_cjk["tags"] == json!(["ui-kit", "前端"]),
            "tag.create Unicode mismatch: {tag_cjk}"
        );
        for bad_tag in ["", "#", "18", "#42", "bad space", "bad/slash"] {
            ensure!(
                world
                    .client_call(
                        "tag.create",
                        json!({ "project_id": pid_cnt, "tag": bad_tag }),
                        Some(&pid_cnt),
                    )
                    .is_err(),
                "Expected invalid tag {bad_tag:?} to be rejected"
            );
        }

        // E2. Explicit comment recipients override prose scanning + `comment.update` & `comment.delete`
        //     + card revision non-conflict!
        let card_e1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_cnt,
                "title": format!("Comment Lifecycle {seed}"),
                "body": "Exercises comment.update, comment.delete, mentions, and non-conflict",
            }),
            Some(&pid_cnt),
        )?;
        let cid_e1 = card_e1["id"].as_str().context("E1 id")?.to_owned();
        let raw_cid_e1 = strip_host(&cid_e1);
        let e1_init_rev = card_e1["revision"].as_u64().unwrap_or(1);
        oracle.record_committed_card("remote-a", &raw_cid_e1, &raw_pid_cnt, e1_init_rev);

        // Hold Herdr offline briefly so queued mention questions & AgentRuns can be inspected cleanly
        // before launch, and deleted while queued!
        let herdr_a = world.herdr("remote-a").unwrap();
        herdr_a.set_offline(true);

        // Explicit `mentions: []` suppresses prose `@codex`.
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_e1,
                "body": "The UI shows Ask @codex",
                "mentions": [],
            }),
            Some(&pid_cnt),
        )?;
        ensure!(
            !world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .questions
                .iter()
                .any(|q| q.card_id == raw_cid_e1),
            "Explicit mentions: [] must suppress prose @codex scanning"
        );

        // Explicit `mentions: ["codex"]` overrides prose `@reviewer and @codex` & does not bump card rev.
        let cmt_1_val = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_e1,
                "body": "Compare @reviewer and @codex",
                "mentions": ["codex"],
            }),
            Some(&pid_cnt),
        )?;
        let cmt_1_id = cmt_1_val["id"].as_str().context("cmt_1 id")?.to_owned();
        ensure!(
            cmt_1_val["card_revision"].as_u64() == Some(e1_init_rev),
            "comment.add must not bump card revision: {cmt_1_val}"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let e1_qs: Vec<_> = snap
                .questions
                .iter()
                .filter(|q| q.card_id == raw_cid_e1)
                .collect();
            ensure!(
                e1_qs.len() == 1 && e1_qs[0].profile_id == "codex",
                "Explicit mentions: [codex] must create exactly 1 @codex Question: {e1_qs:?}"
            );
        }

        // `comment.update`: preserves sequence, bumps revision to 2, idempotent replay, updates
        // existing `@codex` Question in place, adds `@reviewer` Question, rejects empty body.
        let cmt_1_seq = cmt_1_val["sequence"].as_u64().unwrap_or(0);
        let upd_req = json!({
            "card_id": cid_e1,
            "comment_id": cmt_1_id,
            "body": format!("Refined thought for @codex and @reviewer ({seed})"),
            "mentions": ["codex", "reviewer"],
            "request_key": format!("cnt-edit-1-{seed}"),
        });
        let upd_res = world.client_call("comment.update", upd_req.clone(), Some(&pid_cnt))?;
        let upd_replay = world.client_call("comment.update", upd_req, Some(&pid_cnt))?;
        ensure!(
            upd_res["id"] == cmt_1_id
                && upd_res["sequence"] == cmt_1_seq
                && upd_res["card_revision"].as_u64() == Some(e1_init_rev)
                && upd_replay == upd_res,
            "comment.update / replay mismatch: first={upd_res} replay={upd_replay}"
        );
        {
            let tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": raw_cid_e1, "limit": 50, "view": "full" }),
            )?;
            let edited_ev = tl["events"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["id"] == cmt_1_id))
                .context("Edited comment missing from timeline")?;
            ensure!(
                edited_ev["sequence"] == cmt_1_seq
                    && edited_ev["data"]["revision"] == 2
                    && edited_ev["data"]["edited_at"].as_u64().is_some(),
                "Edited comment timeline metadata mismatch: {edited_ev}"
            );
        }
        ensure!(
            world
                .client_call(
                    "comment.update",
                    json!({ "card_id": cid_e1, "comment_id": cmt_1_id, "body": "   " }),
                    Some(&pid_cnt),
                )
                .is_err(),
            "comment.update must reject whitespace-only body"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let codex_qs: Vec<_> = snap
                .questions
                .iter()
                .filter(|q| q.comment_id == cmt_1_id && q.profile_id == "codex")
                .collect();
            let rev_qs: Vec<_> = snap
                .questions
                .iter()
                .filter(|q| q.comment_id == cmt_1_id && q.profile_id == "reviewer")
                .collect();
            ensure!(
                codex_qs.len() == 1
                    && codex_qs[0].body.contains("Refined thought")
                    && rev_qs.len() == 1,
                "comment.update must update @codex in-place and add @reviewer: codex={codex_qs:?} rev={rev_qs:?}"
            );
        }

        // `comment.delete` on queued mention comment: tombstones in place, idempotent replay,
        // cancels pending Question & queued AgentRun, rejects second delete with new key!
        let cmt_del_val = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_e1,
                "body": "Disposable question for @planner https://github.com/a/b/issues/55",
                "mentions": ["planner"],
            }),
            Some(&pid_cnt),
        )?;
        let cmt_del_id = cmt_del_val["id"].as_str().context("cmt_del id")?.to_owned();
        let cmt_del_seq = cmt_del_val["sequence"].as_u64().unwrap_or(0);
        let del_req = json!({
            "card_id": cid_e1,
            "comment_id": cmt_del_id,
            "request_key": format!("cnt-del-1-{seed}"),
        });
        let del_res = world.client_call("comment.delete", del_req.clone(), Some(&pid_cnt))?;
        let del_replay = world.client_call("comment.delete", del_req, Some(&pid_cnt))?;
        ensure!(
            del_res["deleted"] == true
                && del_res["comment_id"] == cmt_del_id
                && del_res["revision"].as_u64() == Some(e1_init_rev)
                && del_replay == del_res,
            "comment.delete / replay mismatch: first={del_res} replay={del_replay}"
        );
        ensure!(
            world
                .client_call(
                    "comment.delete",
                    json!({ "card_id": cid_e1, "comment_id": cmt_del_id }),
                    Some(&pid_cnt),
                )
                .is_err(),
            "Deleting an already-deleted comment with a new key must fail"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            ensure!(
                snap.questions
                    .iter()
                    .find(|q| q.comment_id == cmt_del_id)
                    .is_some_and(|q| q.state == "cancelled"),
                "comment.delete must cancel pending Question"
            );
            ensure!(
                snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_cid_e1
                            && a.profile.id == "planner"
                            && a.state == "cancelled"
                    })
                }),
                "comment.delete must cancel still-queued AgentRun"
            );
            let tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": raw_cid_e1, "limit": 50, "view": "full" }),
            )?;
            let tomb = tl["events"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["id"] == cmt_del_id))
                .context("Tombstoned comment missing from timeline")?;
            ensure!(
                tomb["kind"] == "comment.deleted"
                    && tomb["sequence"] == cmt_del_seq
                    && !tomb["message"].as_str().unwrap_or("").contains("issues/55"),
                "Tombstoned comment mismatch: {tomb}"
            );
        }

        // Non-conflict check: `card.attention.clear` + `card.update` succeed at `e1_init_rev`!
        let _ = world.client_call(
            "card.attention.clear",
            json!({ "card_id": cid_e1 }),
            Some(&pid_cnt),
        )?;
        let e1_upd = world.client_call(
            "card.update",
            json!({
                "card_id": cid_e1,
                "revision": e1_init_rev,
                "title": format!("Comment Lifecycle Updated {seed}"),
                "body": "Updated without revision conflict after comment add/edit/delete",
            }),
            Some(&pid_cnt),
        )?;
        ensure!(
            e1_upd["revision"].as_u64() == Some(e1_init_rev + 1),
            "card.update after comment edit/delete must not conflict: {e1_upd}"
        );

        // Bring Herdr back online so `@codex` & `@reviewer` on E1 launch cleanly; verify:
        // (a) deleting a follow-up `@reviewer` comment while `@reviewer` is running cancels only
        //     the follow-up Question and preserves the running Agent,
        // (b) agent reply (`comment.replied`) cannot be edited or deleted,
        // (c) archived card rejects `comment.update` and `comment.delete`!
        herdr_a.set_offline(false);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let codex_up = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_e1 && a.profile.id == "codex" && a.state == "running"
                })
            });
            let rev_up = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_e1 && a.profile.id == "reviewer" && a.state == "running"
                })
            });
            Ok(codex_up && rev_up)
        })?;
        // Block reviewer's Herdr terminal briefly so the follow-up mention stays `pending` until deleted.
        let (rev_tid_e1, rev_addr_e1, codex_tid_e1) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let rev = snap
                .terminals
                .iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_e1 && a.profile.id == "reviewer")
                })
                .context("reviewer terminal on E1")?;
            let cdx = snap
                .terminals
                .iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_e1 && a.profile.id == "codex")
                })
                .context("codex terminal on E1")?;
            (
                rev.id.clone(),
                rev.agent.as_ref().unwrap().terminal.clone().unwrap(),
                cdx.id.clone(),
            )
        };
        herdr_a.set_status(&rev_addr_e1, "blocked", Some(2));
        world.advance(Duration::from_millis(200))?;
        let followup_cmt = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_e1,
                "body": "Accidental follow-up @reviewer",
                "mentions": ["reviewer"],
            }),
            Some(&pid_cnt),
        )?;
        let followup_id = followup_cmt["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "comment.delete",
            json!({ "card_id": cid_e1, "comment_id": followup_id }),
            Some(&pid_cnt),
        )?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let rev_agent = snap
                .terminals
                .iter()
                .find(|t| t.id == rev_tid_e1)
                .and_then(|t| t.agent.as_ref())
                .context("reviewer agent missing")?;
            ensure!(
                rev_agent.state == "running",
                "Deleting follow-up comment must keep running reviewer agent alive"
            );
            ensure!(
                snap.questions
                    .iter()
                    .find(|q| q.comment_id == followup_id)
                    .is_some_and(|q| q.state == "cancelled"),
                "Follow-up Question must be cancelled by comment.delete"
            );
        }
        // Complete `@codex` with explicit `agent.reply` FIRST + then `execution.report(completed)`
        // to verify fallback completion dedup (`comment_replied == true`, single `comment.replied`,
        // unread count == 1), and verify agent replies reject `comment.update` & `comment.delete`!
        let codex_q_id = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .questions
            .into_iter()
            .find(|q| q.card_id == raw_cid_e1 && q.profile_id == "codex")
            .context("codex question missing")?
            .id;
        let _ = world.server_call(
            "remote-a",
            "agent.reply",
            json!({
                "request_key": format!("cnt-explicit-reply-{seed}"),
                "card_id": raw_cid_e1,
                "profile_id": "codex",
                "question_id": codex_q_id,
                "body": "Explicit reply before report.",
            }),
        )?;
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "request_key": format!("cnt-followup-rep-{seed}"),
                "terminal_id": codex_tid_e1,
                "state": "completed",
                "message": "Answered mention",
            }),
        )?;
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "request_key": format!("cnt-rev-rep-{seed}"),
                "terminal_id": rev_tid_e1,
                "state": "completed",
                "message": "Reviewer done",
            }),
        )?;
        finish_all_live_herdr_terminals(world, "remote-a")?;
        {
            let tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": raw_cid_e1, "limit": 50, "view": "full" }),
            )?;
            let evs: Vec<Event> = serde_json::from_value(tl["events"].clone())?;
            let codex_replies: Vec<_> = evs
                .iter()
                .filter(|e| {
                    e.data["terminal_id"].as_str() == Some(&codex_tid_e1)
                        && e.kind == "comment.replied"
                })
                .collect();
            ensure!(
                codex_replies.len() == 1,
                "Explicit reply + completed report must produce exactly 1 comment.replied: {codex_replies:?}"
            );
            let reported_ev = evs
                .iter()
                .find(|e| {
                    e.data["terminal_id"].as_str() == Some(&codex_tid_e1)
                        && e.kind == "execution.reported"
                })
                .context("codex execution.reported missing")?;
            ensure!(
                reported_ev.data["comment_replied"] == json!(true),
                "execution.reported must mark comment_replied=true after explicit agent.reply: {reported_ev:?}"
            );
            let agent_reply_id = codex_replies[0].id.clone();
            ensure!(
                world
                    .client_call(
                        "comment.update",
                        json!({ "card_id": cid_e1, "comment_id": agent_reply_id, "body": "Tamper" }),
                        Some(&pid_cnt),
                    )
                    .is_err(),
                "comment.update must reject editing agent replies"
            );
            ensure!(
                world
                    .client_call(
                        "comment.delete",
                        json!({ "card_id": cid_e1, "comment_id": agent_reply_id }),
                        Some(&pid_cnt),
                    )
                    .is_err(),
                "comment.delete must reject deleting agent replies"
            );
        }

        // Archived card rejects both `comment.update` and `comment.delete`.
        let arch_card = world.client_call(
            "card.create",
            json!({ "project_id": pid_cnt, "title": "Archived Comment Guard" }),
            Some(&pid_cnt),
        )?;
        let arch_cid = arch_card["id"].as_str().unwrap().to_owned();
        let arch_cmt = world.client_call(
            "comment.add",
            json!({ "card_id": arch_cid, "body": "Before archive" }),
            Some(&pid_cnt),
        )?;
        let arch_cmt_id = arch_cmt["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "card.archive",
            json!({ "card_id": arch_cid, "revision": arch_card["revision"] }),
            Some(&pid_cnt),
        )?;
        ensure!(
            world
                .client_call(
                    "comment.update",
                    json!({ "card_id": arch_cid, "comment_id": arch_cmt_id, "body": "Edit archived" }),
                    Some(&pid_cnt),
                )
                .is_err()
                && world
                    .client_call(
                        "comment.delete",
                        json!({ "card_id": arch_cid, "comment_id": arch_cmt_id }),
                        Some(&pid_cnt),
                    )
                    .is_err(),
            "Archived card must reject comment.update and comment.delete"
        );

        // E3. Card-create prose & explicit mentions, `card.update` non-trigger, & prompt verification!
        let card_mention = world.client_call(
            "card.create",
            json!({
                "project_id": pid_cnt,
                "title": format!("@developer Investigate title mention ({seed})"),
                "body": "Please coordinate with @reviewer before coding.",
            }),
            Some(&pid_cnt),
        )?;
        let cid_m1 = card_mention["id"].as_str().unwrap().to_owned();
        let raw_cid_m1 = strip_host(&cid_m1);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let running = snap
                .terminals
                .iter()
                .filter(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_m1 && a.state == "running")
                })
                .count();
            Ok(running == 2)
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": raw_cid_m1, "limit": 20, "view": "full" }),
            )?;
            let created_ev_id = tl["events"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["kind"] == "card.created"))
                .and_then(|e| e["id"].as_str())
                .context("card.created event missing")?
                .to_owned();
            let dev_term = snap
                .terminals
                .iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_m1 && a.profile.id == "developer")
                })
                .context("developer mention terminal missing")?;
            let dev_run = dev_term.agent.as_ref().unwrap();
            ensure!(
                dev_run.trigger
                    == ExecutionTrigger::Comment {
                        comment_id: created_ev_id.clone()
                    }
                    && !dev_run
                        .instruction
                        .contains("Please coordinate with @reviewer before coding."),
                "card.create mention trigger/instruction mismatch: {dev_run:?}"
            );
            // Verify real captured `prompt.md` uses `reporting_mention.txt` (no duplicate reply--to).
            ensure!(
                !dev_run.input_path.is_empty(),
                "input_path missing on mention run: {dev_run:?}"
            );
            let prompt_path = world.servers["remote-a"].home.join(&dev_run.input_path);
            let prompt_text = std::fs::read_to_string(&prompt_path)?;
            ensure!(
                prompt_text.contains("Finish this `@mention` in one report (no separate `reply`).")
                    && !prompt_text.contains("## Pending questions")
                    && !prompt_text.contains("reply --to")
                    && !prompt_text.contains("CANBAN_ENDPOINT="),
                "Captured mention prompt mismatch: {prompt_text}"
            );
            // Complete `@developer` -> posts `comment.replied` referencing `card.created`!
            let _ = world.server_call(
                "remote-a",
                "execution.report",
                json!({
                    "request_key": format!("cnt-m1-dev-{seed}"),
                    "terminal_id": dev_term.id,
                    "state": "completed",
                    "message": "Title mention analysis complete.",
                }),
            )?;
            let tl_after = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": raw_cid_m1, "limit": 20, "view": "full" }),
            )?;
            ensure!(
                tl_after["events"].as_array().is_some_and(|evs| {
                    evs.iter().any(|e| {
                        e["kind"] == "comment.replied"
                            && e["actor"] == "agent:developer"
                            && e["data"]["reply_to"] == created_ev_id
                    })
                }),
                "Completing card.create mention must post comment.replied with reply_to=card.created: {tl_after}"
            );
        }
        // Explicit `mentions: ["reviewer"]` and `mentions: []` on `card.create`, plus `card.update` non-trigger!
        let m_exp = world.client_call(
            "card.create",
            json!({
                "project_id": pid_cnt,
                "title": "Ask @developer about @reviewer",
                "body": "Body mentions @developer",
                "mentions": ["reviewer"],
            }),
            Some(&pid_cnt),
        )?;
        let raw_cid_m_exp = strip_host(m_exp["id"].as_str().unwrap());
        let m_sup = world.client_call(
            "card.create",
            json!({
                "project_id": pid_cnt,
                "title": "Discuss @developer behavior",
                "body": "Mention @reviewer only in docs",
                "mentions": [],
            }),
            Some(&pid_cnt),
        )?;
        let cid_m_sup = m_sup["id"].as_str().unwrap().to_owned();
        let raw_cid_m_sup = strip_host(&cid_m_sup);
        let _ = world.client_call(
            "card.update",
            json!({
                "card_id": cid_m_sup,
                "revision": m_sup["revision"],
                "title": "@developer @reviewer Updated title",
                "body": "Updated body mentioning @developer",
            }),
            Some(&pid_cnt),
        )?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let exp_terms: Vec<_> = snap
                .terminals
                .iter()
                .filter_map(|t| t.agent.as_ref().filter(|a| a.card_id == raw_cid_m_exp))
                .collect();
            ensure!(
                exp_terms.len() == 1 && exp_terms[0].profile.id == "reviewer",
                "Explicit card.create mentions must override prose: {exp_terms:?}"
            );
            ensure!(
                snap.terminals
                    .iter()
                    .all(|t| t.agent.as_ref().is_none_or(|a| a.card_id != raw_cid_m_sup)),
                "mentions: [] and card.update must never trigger agents"
            );
        }
        // Interactive (`agent.open`) prompt uses `reporting_interactive.txt`; workflow task + concurrent
        // question includes `## Pending questions` + `reply --to`!
        let _ = world.client_call(
            "agent.open",
            json!({ "card_id": cid_m_sup, "profile_id": "developer" }),
            Some(&pid_cnt),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_m_sup && a.state == "running" && !a.input_path.is_empty()
                })
            }))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let int_run = snap
                .terminals
                .iter()
                .find_map(|t| t.agent.as_ref().filter(|a| a.card_id == raw_cid_m_sup))
                .context("interactive run missing")?;
            ensure!(int_run.trigger == ExecutionTrigger::Interactive);
            let p_text =
                std::fs::read_to_string(world.servers["remote-a"].home.join(&int_run.input_path))?;
            ensure!(
                p_text.contains("Interactive discussion: reply in the terminal during discussion")
                    && !p_text.contains("CANBAN_ENDPOINT="),
                "Interactive prompt mismatch: {p_text}"
            );
        }
        finish_all_live_herdr_terminals(world, "remote-a")?;

        // Workflow task + concurrent `@developer` question -> prompt includes `## Pending questions`
        // AND tests profile reload immutability + referenced profile removal rejection!
        let card_prof = world.client_call(
            "card.create",
            json!({
                "project_id": pid_cnt,
                "title": format!("Profile Reload & Task Prompt {seed}"),
                "body": "Build task",
            }),
            Some(&pid_cnt),
        )?;
        let cid_prof = card_prof["id"].as_str().unwrap().to_owned();
        let raw_cid_prof = strip_host(&cid_prof);
        herdr_a.set_offline(true);
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid_prof, "revision": card_prof["revision"], "stage": "develop" }),
            Some(&pid_cnt),
        )?;
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_prof, "body": "@developer Please also check Windows paths" }),
            Some(&pid_cnt),
        )?;
        // Profile reload immutability: change `developer.role_instruction` in `config/agents.json`
        // while the task is queued -> queued run & its launched prompt MUST preserve the original role,
        // and removing a referenced profile (`developer`) must be REJECTED by `config.reload`!
        let agents_json_path = world.servers["remote-a"].home.join("config/agents.json");
        let original_agents_bytes = std::fs::read(&agents_json_path)?;
        let orig_dev_role = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            snap.terminals
                .iter()
                .find_map(|t| t.agent.as_ref().filter(|a| a.card_id == raw_cid_prof))
                .context("queued developer task missing")?
                .profile
                .role_instruction
                .clone()
        };
        {
            let mut bad_cfg: Value = serde_json::from_slice(&original_agents_bytes)?;
            bad_cfg["profiles"]
                .as_array_mut()
                .unwrap()
                .retain(|p| p["id"] != "developer");
            std::fs::write(&agents_json_path, serde_json::to_vec(&bad_cfg)?)?;
            ensure!(
                world
                    .server_call("remote-a", "config.reload", json!({}))
                    .is_err(),
                "Removing referenced profile 'developer' must be rejected by config.reload"
            );
            let mut updated_cfg: Value = serde_json::from_slice(&original_agents_bytes)?;
            for p in updated_cfg["profiles"].as_array_mut().unwrap() {
                if p["id"] == "developer" {
                    p["role_instruction"] = json!("Reloaded role v2");
                }
            }
            std::fs::write(&agents_json_path, serde_json::to_vec(&updated_cfg)?)?;
            let _ = world.server_call("remote-a", "config.reload", json!({}))?;
        }
        herdr_a.set_offline(false);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_prof && a.state == "running" && !a.input_path.is_empty()
                })
            }))
        })?;
        let prof_tid = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let entry = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_prof))
                .context("running developer task missing")?;
            let run = entry.agent.as_ref().unwrap();
            ensure!(
                run.profile.role_instruction == orig_dev_role,
                "Queued role_instruction mutated after config.reload!"
            );
            let p_text =
                std::fs::read_to_string(world.servers["remote-a"].home.join(&run.input_path))?;
            ensure!(
                p_text.contains(&orig_dev_role)
                    && !p_text.contains("Reloaded role v2")
                    && p_text.contains("## Pending questions")
                    && p_text.contains("\"$CANBAN_CLI\" reply --to"),
                "Task prompt must preserve queued role and include Pending questions: {p_text}"
            );
            entry.id.clone()
        };

        // E4. Notes (`note.save`, `note.get`, `note.read`, `note.rename`, `note.write` with
        //     agent execution provenance + idempotency, `note.history`, `note.revision`,
        //     cross-project scope rollback, and `source_note_id` / `card.context.update`).
        let note_b: Note = serde_json::from_value(world.client_call(
            "note.save",
            json!({ "project_id": pid_cnt, "title": "Architecture.md", "body": "Original" }),
            Some(&pid_cnt),
        )?)?;
        let raw_note_b_id = strip_host(&note_b.id);
        let note_a: Note = serde_json::from_value(world.client_call(
            "note.save",
            json!({
                "project_id": pid_cnt,
                "title": "Idea.md",
                "body": format!("[Architecture](note:{raw_note_b_id})\n`[ignored](note:missing)`"),
            }),
            Some(&pid_cnt),
        )?)?;
        let raw_note_a_id = strip_host(&note_a.id);
        ensure!(
            note_a.links == vec![raw_note_b_id.clone()],
            "Expected parsed note link to Architecture.md, got {:?}",
            note_a.links
        );
        let hist_b1: Vec<NoteEvent> = serde_json::from_value(world.client_call(
            "note.history",
            json!({ "note_id": note_b.id }),
            Some(&pid_cnt),
        )?)?;
        ensure!(
            hist_b1
                .iter()
                .any(|e| e.kind == "note.referenced" && e.data["source_note_id"] == raw_note_a_id),
            "note.history missing note.referenced event: {hist_b1:?}"
        );
        let renamed_b: Note = serde_json::from_value(world.client_call(
            "note.rename",
            json!({
                "note_id": note_b.id,
                "title": "Design.md",
                "revision": note_b.revision,
            }),
            Some(&pid_cnt),
        )?)?;
        ensure!(
            strip_host(&renamed_b.id) == raw_note_b_id && renamed_b.title == "Design.md",
            "note.rename mismatch: {renamed_b:?}"
        );
        let note_a_after: Note = serde_json::from_value(world.client_call(
            "note.get",
            json!({ "note_id": note_a.id }),
            Some(&pid_cnt),
        )?)?;
        ensure!(note_a_after.links == vec![raw_note_b_id.clone()]);

        // Agent `note.write` with idempotent `request_key` + `note.revision(1)` + `note.read`!
        let write_payload = json!({
            "note_id": raw_note_b_id,
            "revision": renamed_b.revision,
            "body": format!("Updated by Agent ({seed})"),
            "execution": { "terminal_id": prof_tid },
            "request_key": format!("cnt-note-agent-{seed}"),
        });
        let written_b = world.server_call("remote-a", "note.write", write_payload.clone())?;
        let written_b_replay = world.server_call("remote-a", "note.write", write_payload)?;
        ensure!(
            written_b == written_b_replay,
            "Idempotent note.write replay diverged"
        );
        let read_b =
            world.client_call("note.read", json!({ "note_id": note_b.id }), Some(&pid_cnt))?;
        ensure!(
            read_b["body"].as_str() == Some(&format!("Updated by Agent ({seed})")),
            "note.read mismatch: {read_b}"
        );
        let rev1_b = world.client_call(
            "note.revision",
            json!({ "note_id": note_b.id, "revision": 1 }),
            Some(&pid_cnt),
        )?;
        ensure!(
            rev1_b["body"] == "Original",
            "note.revision(1) mismatch: {rev1_b}"
        );
        let hist_b2: Vec<NoteEvent> = serde_json::from_value(world.client_call(
            "note.history",
            json!({ "note_id": note_b.id }),
            Some(&pid_cnt),
        )?)?;
        let last_note_ev = hist_b2.last().context("note.history empty")?;
        ensure!(
            last_note_ev.actor == "@developer"
                && last_note_ev.card_id.as_deref() == Some(raw_cid_prof.as_str())
                && hist_b2.iter().filter(|e| e.kind == "note.updated").count() == 1,
            "note.write provenance mismatch: {hist_b2:?}"
        );
        // Reject stale revision, unknown terminal_id, and cross-project foreign note links!
        ensure!(
            world
                .server_call(
                    "remote-a",
                    "note.write",
                    json!({ "note_id": raw_note_b_id, "revision": renamed_b.revision, "body": "Stale" }),
                )
                .is_err()
                && world
                    .server_call(
                        "remote-a",
                        "note.write",
                        json!({
                            "note_id": raw_note_b_id,
                            "revision": written_b["revision"],
                            "body": "Bad exec",
                            "execution": { "terminal_id": "missing-terminal" },
                        }),
                    )
                    .is_err(),
            "Stale revision and invalid execution terminal_id must fail note.write"
        );
        let foreign_note: Note = serde_json::from_value(world.client_call(
            "note.save",
            json!({ "project_id": pid_foreign, "title": "Foreign", "body": "Keep" }),
            Some(&pid_foreign),
        )?)?;
        let raw_foreign_note_id = strip_host(&foreign_note.id);
        ensure!(
            world
                .server_call(
                    "remote-a",
                    "note.write",
                    json!({
                        "note_id": raw_note_b_id,
                        "revision": written_b["revision"],
                        "body": format!("[x](note:{raw_foreign_note_id})"),
                    }),
                )
                .is_err()
                && world
                    .client_call(
                        "card.create",
                        json!({
                            "project_id": pid_foreign,
                            "title": "Invalid Scope",
                            "source_note_id": raw_note_b_id,
                        }),
                        Some(&pid_foreign),
                    )
                    .is_err(),
            "Cross-project note link and source_note_id must roll back cleanly"
        );
        let scoped_card = world.client_call(
            "card.create",
            json!({
                "project_id": pid_cnt,
                "title": "Scoped Note Card",
                "body": "Implement scope",
                "source_note_id": raw_note_b_id,
            }),
            Some(&pid_cnt),
        )?;
        let scoped_cid = scoped_card["id"].as_str().unwrap().to_owned();
        let raw_scoped_cid = strip_host(&scoped_cid);
        let note_b_linked: Note = serde_json::from_value(world.client_call(
            "note.get",
            json!({ "note_id": note_b.id }),
            Some(&pid_cnt),
        )?)?;
        ensure!(note_b_linked.card_ids.contains(&raw_scoped_cid));
        let _ = world.client_call(
            "card.context.update",
            json!({
                "card_id": scoped_cid,
                "revision": scoped_card["revision"],
                "note_ids": [],
            }),
            Some(&pid_cnt),
        )?;
        let note_b_unlinked: Note = serde_json::from_value(world.client_call(
            "note.get",
            json!({ "note_id": note_b.id }),
            Some(&pid_cnt),
        )?)?;
        ensure!(
            !note_b_unlinked.card_ids.contains(&raw_scoped_cid),
            "card.context.update must unlink scoped card {raw_scoped_cid}: {:?}",
            note_b_unlinked.card_ids
        );
        let hist_b3: Vec<NoteEvent> = serde_json::from_value(world.client_call(
            "note.history",
            json!({ "note_id": note_b.id }),
            Some(&pid_cnt),
        )?)?;
        ensure!(
            hist_b3
                .last()
                .is_some_and(|e| e.kind == "note.card_unlinked"),
            "card.context.update unlink must record note.card_unlinked: {hist_b3:?}"
        );

        // Complete `prof_tid` -> next mention on `cid_prof` picks up `"Reloaded role v2"`, then restore config!
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "request_key": format!("cnt-prof-done-{seed}"),
                "terminal_id": prof_tid,
                "state": "completed",
                "message": "Done old role",
            }),
        )?;
        finish_all_live_herdr_terminals(world, "remote-a")?;
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_prof, "body": "@developer next assignment after reload" }),
            Some(&pid_cnt),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_prof
                        && (a.profile.role_instruction == "Reloaded role v2"
                            || a.pending
                                .as_ref()
                                .is_some_and(|p| p.profile.role_instruction == "Reloaded role v2"))
                })
            }))
        })?;
        // Restore original agents.json so subsequent checks see clean defaults.
        std::fs::write(&agents_json_path, &original_agents_bytes)?;
        let _ = world.server_call("remote-a", "config.reload", json!({}))?;
        finish_all_live_herdr_terminals(world, "remote-a")?;

        // E5. Review activity warnings (`working` -> warns, `idle`/`done`/`blocked`/`error` -> clears,
        //     `unavailable` -> warns, queued -> warns & `review.merge` still succeeds!).
        let card_warn = world.client_call(
            "card.create",
            json!({
                "project_id": pid_rev,
                "title": format!("Review Warning Check ({seed})"),
                "body": "Advisory warnings never block review.merge",
            }),
            Some(&pid_rev),
        )?;
        let cid_warn = card_warn["id"].as_str().unwrap().to_owned();
        let raw_cid_warn = strip_host(&cid_warn);
        let _ = world.client_call(
            "review.run_agent",
            json!({ "card_id": cid_warn, "profile_id": "codex" }),
            Some(&pid_rev),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .any(|c| c.id == raw_cid_warn && c.worktree.is_some())
                && snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_cid_warn && a.state == "running" && a.terminal.is_some()
                    })
                }))
        })?;
        let (warn_wt, warn_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wt = snap
                .cards
                .iter()
                .find(|c| c.id == raw_cid_warn)
                .unwrap()
                .worktree
                .clone()
                .unwrap();
            let addr = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_warn))
                .unwrap()
                .agent
                .as_ref()
                .unwrap()
                .terminal
                .clone()
                .unwrap();
            (wt, addr)
        };
        let _ = exec.commit_in_worktree(&warn_wt, "feat(review): warn check commit")?;
        herdr_a.set_status(&warn_addr, "working", Some(1));
        world.advance(Duration::from_millis(200))?;
        let p_working = world.client_call(
            "review.preview",
            json!({ "card_id": cid_warn }),
            Some(&pid_rev),
        )?;
        ensure!(
            p_working["merge_allowed"] == true
                && p_working["warning"]
                    .as_str()
                    .is_some_and(|w| w.contains("still running")),
            "Working agent must warn without blocking merge: {p_working}"
        );
        for stopped_state in ["idle", "done", "blocked", "error"] {
            herdr_a.set_status(&warn_addr, stopped_state, Some(2));
            world.advance(Duration::from_millis(200))?;
            let p_clear = world.client_call(
                "review.preview",
                json!({ "card_id": cid_warn }),
                Some(&pid_rev),
            )?;
            ensure!(
                p_clear["merge_allowed"] == true && p_clear["warning"].is_null(),
                "Stopped state '{stopped_state}' must clear review warning: {p_clear}"
            );
        }
        // Offline Herdr (`unavailable`) + queued agent warning still allow `review.merge`!
        herdr_a.set_offline(true);
        world.advance(Duration::from_millis(200))?;
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_warn, "body": "@developer queued while offline", "mentions": ["developer"] }),
            Some(&pid_rev),
        )?;
        let p_queued = world.client_call(
            "review.preview",
            json!({ "card_id": cid_warn }),
            Some(&pid_rev),
        )?;
        ensure!(
            p_queued["merge_allowed"] == true && p_queued["warning"].is_string(),
            "Queued/unavailable agent must warn without blocking merge: {p_queued}"
        );
        let warn_rev = find_card(world, "remote-a", &raw_cid_warn)?.revision;
        let warn_merged = world.client_call(
            "review.merge",
            json!({
                "card_id": cid_warn,
                "revision": warn_rev,
                "token": p_queued["token"],
                "strategy": "rebase",
                "request_key": format!("cnt-warn-merge-{seed}"),
            }),
            Some(&pid_rev),
        )?;
        ensure!(
            warn_merged["state"] == "merged",
            "review.merge must succeed despite advisory warning: {warn_merged}"
        );
        herdr_a.set_offline(false);
        world.advance(Duration::from_millis(1_500))?;
        finish_all_live_herdr_terminals(world, "remote-a")?;

        // Bounce `remote-a` and verify comments, notes, and tags persisted durably!
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let p_cnt_after = snap
                .projects
                .iter()
                .find(|p| p.id == raw_pid_cnt)
                .context("CommentsNotesTags project missing after bounce")?;
            ensure!(
                p_cnt_after.tags == vec!["ui-kit".to_owned(), "前端".to_owned()],
                "Tags did not persist across bounce: {:?}",
                p_cnt_after.tags
            );
            let read_after =
                world.client_call("note.read", json!({ "note_id": note_b.id }), Some(&pid_cnt))?;
            ensure!(
                read_after["title"] == "Design.md"
                    && read_after["body"].as_str() == Some(&format!("Updated by Agent ({seed})")),
                "Note rename/write did not persist across bounce: {read_after}"
            );
            let rev1_after = world.client_call(
                "note.revision",
                json!({ "note_id": note_b.id, "revision": 1 }),
                Some(&pid_cnt),
            )?;
            ensure!(rev1_after["body"] == "Original");
            let tl_after = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": raw_cid_e1, "limit": 50, "view": "full" }),
            )?;
            let evs = tl_after["events"].as_array().context("tl events")?;
            ensure!(
                evs.iter()
                    .any(|e| e["id"] == cmt_1_id && e["data"]["revision"] == 2)
                    && evs
                        .iter()
                        .any(|e| e["id"] == cmt_del_id && e["kind"] == "comment.deleted"),
                "Comment edit/delete did not persist across bounce: {tl_after}"
            );
        }
        world.hit("review_events.comments_notes_tags");
        oracle.check_step_invariants(world)?;

        // =====================================================================
        // PART F — Git Client RPCs (`git.{refs,compare,file_lines,stage,unstage,commit,log,commit_detail,fetch,sync}`)
        //          + `review.send_agent(new_card)` backed 100% by `SimGitRepo` (ZERO host git spawns!)
        // =====================================================================
        let git_dir = world.project_dir(&format!("git-client-repo-{seed}"))?;
        let outside_dir = world.project_dir(&format!("git-outside-{seed}"))?;
        exec.register_git_repo(&git_dir, SimGitRepo::default());
        exec.set_branch_upstream(&git_dir, "main", "origin/main")?;

        let proj_git = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Git Client Repo {seed}"),
                "directory": git_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_git = proj_git["id"].as_str().context("proj_git id")?.to_owned();

        // Checkout containment & user-action execution guards.
        ensure!(
            world
                .client_call(
                    "git.refs",
                    json!({ "project_id": pid_git, "cwd": outside_dir }),
                    Some(&pid_git),
                )
                .is_err()
                && world
                    .server_call(
                        "remote-a",
                        "git.stage",
                        json!({
                            "project_id": strip_host(&pid_git),
                            "all": true,
                            "execution": { "request_id": "agent-denied" },
                        }),
                    )
                    .is_err()
                && world
                    .server_call(
                        "remote-a",
                        "git.sync",
                        json!({
                            "project_id": strip_host(&pid_git),
                            "execution": { "request_id": "agent-denied" },
                        }),
                    )
                    .is_err()
                && world
                    .client_call(
                        "git.sync",
                        json!({ "project_id": pid_git, "cwd": outside_dir }),
                        Some(&pid_git),
                    )
                    .is_err(),
            "git.* checkout containment or agent execution guard failed"
        );

        // Managed worktree + external git worktree discovery via `git.refs`.
        let git_wt_res = world.client_call(
            "worktree.create",
            json!({
                "project_id": pid_git,
                "branch": "feat/git-ui",
                "launch": "none",
            }),
            Some(&pid_git),
        )?;
        let git_wt_path = git_wt_res["worktree"]["path"]
            .as_str()
            .context("git_wt path")?
            .to_owned();
        let refs_wt = world.client_call(
            "git.refs",
            json!({ "project_id": pid_git, "cwd": git_wt_path }),
            Some(&pid_git),
        )?;
        ensure!(
            refs_wt["current_branch"] == "feat/git-ui"
                && refs_wt["checkouts"]
                    .as_array()
                    .is_some_and(|a| a.len() == 2)
                && refs_wt["branches"]
                    .as_array()
                    .is_some_and(|b| b.iter().any(|x| x["name"] == "feat/git-ui")),
            "git.refs(managed worktree) mismatch: {refs_wt}"
        );

        let ext_wt_dir = std::path::Path::new(&git_dir).join(".worktrees/ext-branch");
        exec.add_external_worktree(&git_dir, "fix/external-wt", &ext_wt_dir)?;
        exec.write_worktree_file(&ext_wt_dir, "ext.rs", "pub fn external() {}\n")?;
        let refs_with_ext =
            world.client_call("git.refs", json!({ "project_id": pid_git }), Some(&pid_git))?;
        ensure!(
            refs_with_ext["checkouts"]
                .as_array()
                .is_some_and(|a| a.len() == 3 && a.iter().any(|c| c["branch"] == "fix/external-wt")),
            "git.refs must discover external worktree: {refs_with_ext}"
        );
        let ext_refs = world.client_call(
            "git.refs",
            json!({ "project_id": pid_git, "cwd": ext_wt_dir }),
            Some(&pid_git),
        )?;
        ensure!(
            ext_refs["current_branch"] == "fix/external-wt" && ext_refs["dirty"] == true,
            "git.refs(external worktree) mismatch: {ext_refs}"
        );
        ensure!(
            world
                .client_call(
                    "git.refs",
                    json!({ "project_id": pid_git, "cwd": outside_dir }),
                    Some(&pid_git),
                )
                .is_err(),
            "Outside dir must stay rejected with external worktree fallback"
        );

        // `git.stage`, `git.unstage`, `git.compare`, `git.file_lines`, `git.commit`, `git.log`,
        // `git.commit_detail`, `git.fetch`, `git.sync`, and `review.send_agent(new_card)`!
        exec.write_worktree_file(
            &git_wt_path,
            "client.rs",
            "pub fn sync() -> i32 { 1 }\npub fn helper() {}\n",
        )?;
        let stg_req = json!({
            "project_id": pid_git,
            "cwd": git_wt_path,
            "paths": ["client.rs"],
            "request_key": format!("git-stg-1-{seed}"),
        });
        let stg_res = world.client_call("git.stage", stg_req.clone(), Some(&pid_git))?;
        let stg_replay = world.client_call("git.stage", stg_req, Some(&pid_git))?;
        ensure!(
            stg_res == stg_replay
                && stg_res["status_files"].as_array().is_some_and(|a| a
                    .iter()
                    .any(|f| f["path"] == "client.rs" && f["staged"] == true)),
            "git.stage mismatch: {stg_res}"
        );
        let unstg_res = world.client_call(
            "git.unstage",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "paths": ["client.rs"],
                "request_key": format!("git-unstg-1-{seed}"),
            }),
            Some(&pid_git),
        )?;
        ensure!(
            unstg_res["status_files"].as_array().is_some_and(|a| a
                .iter()
                .any(|f| f["path"] == "client.rs" && f["staged"] == false)),
            "git.unstage must unstage client.rs: {unstg_res}"
        );
        // Re-stage, verify `git.compare(UNCOMMITTED)` & `git.file_lines(index)`, then commit + push!
        let _ = world.client_call(
            "git.stage",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "paths": ["client.rs"],
                "request_key": format!("git-stg-2-{seed}"),
            }),
            Some(&pid_git),
        )?;
        let cmp_uncommitted = world.client_call(
            "git.compare",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "base": "HEAD",
                "compare": "UNCOMMITTED",
            }),
            Some(&pid_git),
        )?;
        ensure!(
            cmp_uncommitted["files"].as_array().is_some_and(|f| f
                .iter()
                .any(|x| x["filename"] == "client.rs" && x["section"] == "staged")),
            "git.compare(UNCOMMITTED) missing staged client.rs: {cmp_uncommitted}"
        );
        let idx_lines = world.client_call(
            "git.file_lines",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "path": "client.rs",
                "source": "index",
            }),
            Some(&pid_git),
        )?;
        ensure!(
            idx_lines["total_lines"].as_u64() == Some(2),
            "git.file_lines(index) mismatch: {idx_lines}"
        );

        let cmt_res = world.client_call(
            "git.commit",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "message": "feat: add git client rpc\n\nverified in worktree",
                "push": true,
                "request_key": format!("git-cmt-1-{seed}"),
            }),
            Some(&pid_git),
        )?;
        ensure!(
            cmt_res["pushed"] == true && cmt_res["subject"] == "feat: add git client rpc",
            "git.commit mismatch: {cmt_res}"
        );
        let cmt_sha = cmt_res["sha"].as_str().context("commit sha")?.to_owned();

        let cmp_branch = world.client_call(
            "git.compare",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "base": "main",
                "compare": "feat/git-ui",
                "three_dot": true,
            }),
            Some(&pid_git),
        )?;
        ensure!(
            cmp_branch["files"]
                .as_array()
                .is_some_and(|f| f.len() == 1 && f[0]["filename"] == "client.rs"),
            "git.compare(main..feat/git-ui) mismatch: {cmp_branch}"
        );
        let rev_lines = world.client_call(
            "git.file_lines",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "path": "client.rs",
                "source": "rev",
                "rev": cmt_sha,
            }),
            Some(&pid_git),
        )?;
        ensure!(
            rev_lines["total_lines"].as_u64().unwrap_or(0) >= 2,
            "git.file_lines(rev) mismatch: {rev_lines}"
        );

        let log_res = world.client_call(
            "git.log",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "rev": "feat/git-ui",
                "limit": 10,
            }),
            Some(&pid_git),
        )?;
        ensure!(
            log_res["commits"].as_array().is_some_and(|c| c.len() == 2),
            "git.log expected 2 commits, got: {log_res}"
        );

        let detail_res = world.client_call(
            "git.commit_detail",
            json!({
                "project_id": pid_git,
                "cwd": git_wt_path,
                "sha": cmt_sha,
            }),
            Some(&pid_git),
        )?;
        ensure!(
            detail_res["commit_detail"]["subject"] == "feat: add git client rpc"
                && detail_res["commit_detail"]["body"] == "verified in worktree"
                && detail_res["files"][0]["filename"] == "client.rs",
            "git.commit_detail mismatch: {detail_res}"
        );

        let fetch_res = world.client_call(
            "git.fetch",
            json!({ "project_id": pid_git, "request_key": format!("git-fetch-1-{seed}") }),
            Some(&pid_git),
        )?;
        ensure!(fetch_res["remote"] == "origin");

        // Ad-hoc `review.send_agent(mode: "new_card")` creates a brand-new Card & assigns `@developer`.
        let adhoc_card = world.client_call(
            "review.send_agent",
            json!({
                "mode": "new_card",
                "project_id": pid_git,
                "profile_id": "developer",
                "title": format!("Address compare review ({seed})"),
                "context_label": "main <- feat/git-ui",
                "summary": "Follow up on branch diff.",
                "comments": [],
                "request_key": format!("git-new-card-{seed}"),
            }),
            Some(&pid_git),
        )?;
        ensure!(
            adhoc_card["state"] == "created"
                && adhoc_card["card_record"]["id"] == adhoc_card["card_id"],
            "review.send_agent(new_card) mismatch: {adhoc_card}"
        );
        finish_all_live_herdr_terminals(world, "remote-a")?;

        // `git.sync`: publish/push worktree branch + idempotent replay + mismatched-key rejection +
        // fast-forward pull on project root `main`!
        let sync_wt_res = world.client_call(
            "worktree.create",
            json!({
                "project_id": pid_git,
                "branch": "feat/rpc-sync",
                "launch": "none",
            }),
            Some(&pid_git),
        )?;
        let sync_wt_path = sync_wt_res["worktree"]["path"]
            .as_str()
            .context("sync_wt path")?
            .to_owned();
        exec.write_worktree_file(&sync_wt_path, "sync_rpc.rs", "pub fn ok() {}\n")?;
        let _ = world.client_call(
            "git.stage",
            json!({ "project_id": pid_git, "cwd": sync_wt_path, "paths": ["sync_rpc.rs"] }),
            Some(&pid_git),
        )?;
        let sync_cmt = world.client_call(
            "git.commit",
            json!({ "project_id": pid_git, "cwd": sync_wt_path, "message": "feat: rpc sync" }),
            Some(&pid_git),
        )?;
        let sync_head = sync_cmt["sha"].as_str().unwrap().to_owned();
        let sync_wt_req = json!({
            "project_id": pid_git,
            "cwd": sync_wt_path,
            "request_key": format!("git-sync-wt-{seed}"),
        });
        let sync_wt = world.client_call("git.sync", sync_wt_req.clone(), Some(&pid_git))?;
        ensure!(
            sync_wt["pushed"] == true
                && sync_wt["set_upstream"] == true
                && exec
                    .remote_ref_sha(&git_dir, "origin", "feat/rpc-sync")
                    .as_deref()
                    == Some(sync_head.as_str()),
            "git.sync(worktree) mismatch: {sync_wt}"
        );
        let sync_wt_replay = world.client_call("git.sync", sync_wt_req, Some(&pid_git))?;
        ensure!(sync_wt_replay == sync_wt, "git.sync replay diverged");
        ensure!(
            world
                .client_call(
                    "git.sync",
                    json!({ "project_id": pid_git, "request_key": format!("git-sync-wt-{seed}") }),
                    Some(&pid_git),
                )
                .is_err(),
            "git.sync with same request_key and different payload must reject"
        );

        let upstream_sha = exec.advance_remote_ref(&git_dir, "origin", "main")?;
        let sync_root = world.client_call(
            "git.sync",
            json!({ "project_id": pid_git, "request_key": format!("git-sync-root-{seed}") }),
            Some(&pid_git),
        )?;
        ensure!(
            sync_root["fast_forwarded"] == true
                && sync_root["pulled_commits"] == 1
                && sync_root["pushed"] == false
                && exec.head_sha(&git_dir).as_deref() == Some(upstream_sha.as_str()),
            "git.sync(root fast-forward) mismatch: {sync_root}"
        );

        // Bounce `remote-a` and verify `git.refs`, `git.log`, and idempotent receipt survive restart!
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        let log_after_bounce = world.client_call(
            "git.log",
            json!({ "project_id": pid_git, "cwd": git_wt_path, "rev": "feat/git-ui", "limit": 10 }),
            Some(&pid_git),
        )?;
        ensure!(
            log_after_bounce["commits"]
                .as_array()
                .is_some_and(|c| c.len() == 2),
            "git.log after bounce mismatch: {log_after_bounce}"
        );
        let sync_replay_after_bounce = world.client_call(
            "git.sync",
            json!({ "project_id": pid_git, "request_key": format!("git-sync-root-{seed}") }),
            Some(&pid_git),
        )?;
        ensure!(sync_replay_after_bounce == sync_root);
        world.hit("review_events.git_client_rpcs");

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
