use super::*;

fn apply_workflow_file(
    app: &AppServer,
    p: &Project,
    filename: &str,
    source: &str,
) -> Result<Value> {
    let c = app.call("workflow.sources", json!({"project_id": p.id}))?;
    let mut files = c["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["filename"] != filename)
        .map(|f| json!({"filename": f["filename"], "source": f["source"]}))
        .collect::<Vec<_>>();
    files.push(json!({"filename": filename, "source": source}));
    app.call(
        "workflow.apply",
        json!({"project_id": p.id, "revision": c["revision"], "sources": files}),
    )
}

#[test]
fn schedule_publish_github_shorthand_toggle_and_startup_sleep_skip() {
    let (dir, app, p, _c) = setup();
    let yaml = r#"schema: 8
id: nightly_audit
name: Nightly Audit
on:
  schedule:
    - cron: "0 2 * * *"
      timezone: UTC
      overlap: queue
  weekly_card:
    cron: "0 9 * * MON"
    timezone: Asia/Shanghai
    card:
      column: develop
      title: "Weekly QA ({{date}} #{{run_number}})"
      body: "Automated check for {{workflow}}"
    start_at: audit
steps:
  - id: audit
    uses: agent
    with:
      profile: developer
      instruction: Check dependency advisories
    next: done
  - id: done
    uses: complete
"#;
    apply_workflow_file(&app, &p, "nightly_audit.yaml", yaml).unwrap();

    let listed = app
        .call("schedule.list", json!({"project_id": p.id}))
        .unwrap();
    let schedules = listed["schedules"].as_array().unwrap();
    assert_eq!(schedules.len(), 2);

    let cardless = schedules
        .iter()
        .find(|s| s["rule_id"] == "schedule")
        .expect("GitHub Actions on.schedule shorthand expanded to rule `schedule`");
    assert_eq!(cardless["cron"], "0 2 * * *");
    assert_eq!(cardless["timezone"], "UTC");
    assert_eq!(cardless["overlap"], "queue");
    assert_eq!(cardless["creates_card"], false);
    assert_eq!(cardless["enabled"], true);
    assert!(cardless["next_run_at"].as_u64().is_some());

    let card_mode = schedules
        .iter()
        .find(|s| s["rule_id"] == "weekly_card")
        .expect("Named schedule rule present");
    assert_eq!(card_mode["creates_card"], true);
    assert_eq!(card_mode["target_column"], "develop");

    // Toggle off and back on.
    let sid = cardless["id"].as_str().unwrap().to_owned();
    let toggled_off = app
        .call(
            "schedule.toggle",
            json!({"schedule_id": sid, "enabled": false}),
        )
        .unwrap();
    assert_eq!(toggled_off["enabled"], false);
    assert!(toggled_off["next_run_at"].is_null());

    let toggled_on = app
        .call(
            "schedule.toggle",
            json!({"schedule_id": sid, "enabled": true}),
        )
        .unwrap();
    assert_eq!(toggled_on["enabled"], true);
    assert!(toggled_on["next_run_at"].as_u64().is_some());

    // Simulate daemon offline / sleep past `next_run_at`: startup reconciliation must advance
    // `next_run_at` into the future without firing missed runs.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut st: ScheduleState = db.get(&sid)?;
            st.next_run_at = Some(1_000);
            db.save(&st)
        })
        .unwrap();
    drop(app);

    let mut reopened = None;
    for _ in 0..100 {
        if let Ok(srv) = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")) {
            reopened = Some(srv);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let app = reopened.expect("Reopened AppServer");
    let snap = app.snapshot().unwrap();
    let refreshed = snap
        .schedule_states
        .iter()
        .find(|s| s.id == sid)
        .expect("ScheduleState persisted");
    assert!(refreshed.next_run_at.unwrap() > 1_700_000_000_000);
    assert!(
        snap.scheduled_runs.is_empty(),
        "Offline missed ticks must not replay runs on startup"
    );
}

#[test]
fn schedule_card_creating_mode_attaches_provenance_avoids_double_trigger_and_reconciles() {
    let (_dir, app, p, _c) = setup();
    let yaml = r#"schema: 8
id: scheduled_qa
name: Scheduled QA
workspace:
  create_worktree: false
  sync_base: false
on:
  develop_entered:
    event: card.column_entered
    match:
      column: develop
    start_at: qa_step
  nightly:
    cron: "30 3 * * *"
    timezone: UTC
    start_at: qa_step
    card:
      column: develop
      title: "Nightly QA #{{run_number}}"
      body: "Scheduled run for {{workflow}}"
steps:
  - id: qa_step
    uses: agent
    with:
      profile: developer
      instruction: Run nightly QA suite
    next: finish
  - id: finish
    uses: complete
"#;
    apply_workflow_file(&app, &p, "scheduled_qa.yaml", yaml).unwrap();

    let triggered: ScheduledRun = serde_json::from_value(
        app.call(
            "schedule.trigger",
            json!({
                "project_id": p.id,
                "workflow_id": "scheduled_qa",
                "rule_id": "nightly"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(triggered.creates_card);
    assert_eq!(triggered.number, 1);
    assert_eq!(triggered.trigger_kind, "manual");
    let created_card_id = triggered.card_id.clone().expect("Card created");

    let snap = app.snapshot().unwrap();
    let card = snap
        .cards
        .iter()
        .find(|c| c.id == created_card_id)
        .expect("Scheduled Card in snapshot");
    assert_eq!(card.stage, "develop");
    assert_eq!(card.title, "Nightly QA #1");
    let src = card
        .schedule_source
        .as_ref()
        .expect("Card carries schedule_source provenance");
    assert_eq!(src.workflow_id, "scheduled_qa");
    assert_eq!(src.rule_id, "nightly");
    assert_eq!(src.run_number, 1);

    // Must start exactly ONE WorkflowRun (card.column_entered must NOT double-trigger).
    let card_runs: Vec<_> = snap
        .workflow_runs
        .iter()
        .filter(|w| w.card_id.as_deref() == Some(card.id.as_str()))
        .collect();
    assert_eq!(card_runs.len(), 1);

    // Complete the Card's agent step and pump schedules so `ScheduledRun` reconciles to completed.
    let term = snap
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card.id))
        .expect("Card terminal queued")
        .clone();
    let admitted = admit(&app, &term);
    app.call(
        "execution.report",
        json!({
            "terminal_id": admitted.id,
            "request_key": "sched-qa-done",
            "state": "completed",
            "message": "QA green",
            "body": "# Nightly QA\nAll checks passed."
        }),
    )
    .unwrap();
    app.pump_schedules().unwrap();

    let after = app.snapshot().unwrap();
    let updated_run = after
        .scheduled_runs
        .iter()
        .find(|r| r.id == triggered.id)
        .unwrap();
    assert_eq!(updated_run.state, "completed");
    assert!(updated_run.completed_at.is_some());
}

#[test]
fn schedule_cardless_mode_reports_reads_artifacts_respects_overlap_and_reclaims_terminal() {
    let (_dir, app, p, _c) = setup();
    let yaml = r#"schema: 8
id: dep_audit
name: Dependency Audit
on:
  hourly_skip:
    cron: "0 * * * *"
    timezone: UTC
    overlap: skip
    start_at: inspect
  hourly_queue:
    cron: "15 * * * *"
    timezone: UTC
    overlap: queue
    start_at: inspect
steps:
  - id: inspect
    uses: agent
    with:
      profile: developer
      instruction: Audit outdated dependencies
    next: done
  - id: done
    uses: complete
"#;
    apply_workflow_file(&app, &p, "dep_audit.yaml", yaml).unwrap();

    // Trigger Cardless run manually.
    let run: ScheduledRun = serde_json::from_value(
        app.call(
            "schedule.trigger",
            json!({
                "project_id": p.id,
                "workflow_id": "dep_audit",
                "rule_id": "hourly_skip"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!run.creates_card);
    assert!(run.card_id.is_none());
    assert_eq!(run.state, "queued");
    assert!(run.id.starts_with(&format!("scheduled:{}:", p.id)));

    // Overlap `skip`: if due tick arrives while run #1 is still active, record a `skipped` run.
    let now_ms = crate::now();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut st: ScheduleState = db.get(&run.schedule_id)?;
            st.next_run_at = Some(now_ms.saturating_sub(1_000));
            db.save(&st)
        })
        .unwrap();
    app.evaluate_and_fire_due_schedules_for_test().unwrap();
    let snap = app.snapshot().unwrap();
    assert!(
        snap.scheduled_runs
            .iter()
            .any(|r| r.schedule_id == run.schedule_id && r.state == "skipped"),
        "Overlap skip recorded a skipped ScheduledRun"
    );

    // Simulate live PTY execution + `canban report` (`schedule.report`) with Markdown body.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut cur: ScheduledRun = db.get(&run.id)?;
            cur.state = "running".into();
            cur.started_at = Some(now_ms);
            db.save(&cur)?;
            let mut entry: TerminalEntry = db.get(&run.id)?;
            entry.runtime = Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: Some("offline-core".into()),
                core_id: Some("offline-core".into()),
                terminal: Some("pty-sched-1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("idle".into()),
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            });
            db.save(&entry)
        })
        .unwrap();

    // Active count includes running Cardless scheduled terminal.
    assert_eq!(canban_scheduler::active_count(&app.snapshot().unwrap()), 1);

    app.call(
        "execution.report",
        json!({
            "schedule_run_id": run.id,
            "request_key": "cardless-report-1",
            "state": "completed",
            "message": "0 vulnerabilities found",
            "body": "# Audit Report\nClean dependency tree."
        }),
    )
    .unwrap();

    // Auto-reclamation immediately transitions finished Cardless PTY to `state = "finished"`.
    app.pump_schedules().unwrap();
    let after = app.snapshot().unwrap();
    let finished_run = after
        .scheduled_runs
        .iter()
        .find(|r| r.id == run.id)
        .unwrap();
    assert_eq!(finished_run.state, "completed");
    assert_eq!(
        finished_run.summary.as_deref(),
        Some("0 vulnerabilities found")
    );
    let report_path = finished_run.report_path.as_deref().expect("Report saved");
    let artifact = app
        .call(
            "schedule.read_artifact",
            json!({
                "schedule_run_id": run.id,
                "path": report_path
            }),
        )
        .unwrap();
    assert!(
        artifact["body"]
            .as_str()
            .unwrap()
            .contains("Clean dependency tree.")
    );

    let entry = after.terminals.iter().find(|t| t.id == run.id).unwrap();
    assert_eq!(
        entry.runtime.as_ref().unwrap().state,
        "finished",
        "Cardless PTY auto-reclaimed after completion"
    );
    assert!(!after.terminal_is_live(entry));
    assert_eq!(canban_scheduler::active_count(&after), 0);
}

#[test]
fn global_schedule_create_toggle_delete_and_validation() {
    let (_dir, app, _p, _c) = setup();

    // Global schedules cannot create Board Cards.
    let card_err = app
        .call(
            "schedule.create",
            json!({
                "scope": "global",
                "name": "Invalid Global Card",
                "cron": "0 3 * * *",
                "mode": "card",
                "card_column": "develop",
                "request_key": "global-card-err"
            }),
        )
        .unwrap_err();
    assert!(
        card_err
            .to_string()
            .contains("Global schedules cannot create Board Cards"),
        "unexpected error: {card_err:#}"
    );

    // Create a Global script -> agent escalation schedule.
    let created = app
        .call(
            "schedule.create",
            json!({
                "scope": "global",
                "name": "Worktree GC",
                "cron": "0 3 * * *",
                "timezone": "Local",
                "overlap": "skip",
                "mode": "script_agent",
                "script": "echo clean",
                "profile_id": "developer",
                "instruction": "Ask the user when dirty worktrees remain.",
                "request_key": "global-create-1"
            }),
        )
        .unwrap();
    let sched: ScheduleState = serde_json::from_value(created["schedule"].clone()).unwrap();
    assert_eq!(sched.project_id, canban_types::GLOBAL_PROJECT_ID);
    assert!(sched.starts_with_script);
    assert!(sched.enabled);

    // Global list filter returns only global schedules.
    let global_list = app
        .call("schedule.list", json!({"scope": "global"}))
        .unwrap();
    assert_eq!(global_list["schedules"].as_array().unwrap().len(), 1);

    // Toggle off and back on.
    let toggled_off = app
        .call(
            "schedule.toggle",
            json!({
                "schedule_id": sched.id,
                "enabled": false,
                "request_key": "global-toggle-off"
            }),
        )
        .unwrap();
    assert_eq!(toggled_off["enabled"], json!(false));

    // Direct workflow authoring under `__global` rejects columns.
    let src = app
        .call(
            "workflow.sources",
            json!({"project_id": canban_types::GLOBAL_PROJECT_ID}),
        )
        .unwrap();
    let col_err = app
        .call(
            "workflow.apply",
            json!({
                "project_id": canban_types::GLOBAL_PROJECT_ID,
                "revision": src["revision"],
                "sources": [{
                    "filename": "bad.yaml",
                    "source": "schema: 9\nid: bad\nname: Bad Global\nscope: global\ncolumns:\n  - id: backlog\n    name: Backlog\non:\n  schedule:\n    - cron: '0 3 * * *'\nsteps:\n  - id: done\n    uses: complete\n"
                }]
            }),
        )
        .unwrap_err();
    assert!(
        format!("{col_err:#}").contains("Global workflows cannot declare Board `columns`"),
        "unexpected error: {col_err:#}"
    );

    // Delete drops the standalone global schedule file cleanly.
    let deleted = app
        .call(
            "schedule.delete",
            json!({
                "schedule_id": sched.id,
                "request_key": "global-delete-1"
            }),
        )
        .unwrap();
    assert_eq!(deleted["deleted"], json!(true));
    let after = app
        .call("schedule.list", json!({"scope": "global"}))
        .unwrap();
    assert!(after["schedules"].as_array().unwrap().is_empty());
}

#[test]
fn script_lazy_admission_zero_pty_on_success_and_escalates_on_failure() {
    let (_dir, app, _p, _c) = setup();

    // 1. Clean script run finishes without creating any TerminalEntry or consuming agent slots.
    let ok_sched = app
        .call(
            "schedule.create",
            json!({
                "scope": "global",
                "name": "Clean Check",
                "cron": "0 4 * * *",
                "mode": "script",
                "script": "printf 'all clean\\n'",
                "request_key": "lazy-ok-create"
            }),
        )
        .unwrap();
    let ok_id = ok_sched["schedule"]["id"].as_str().unwrap().to_owned();
    let ok_run: ScheduledRun = serde_json::from_value(
        app.call(
            "schedule.trigger",
            json!({"schedule_id": ok_id, "request_key": "lazy-ok-trigger"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(ok_run.terminal_id.is_none());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let finished_ok = loop {
        app.pump_schedules().unwrap();
        let snap = app.snapshot().unwrap();
        let cur = snap
            .scheduled_runs
            .iter()
            .find(|r| r.id == ok_run.id)
            .unwrap()
            .clone();
        if matches!(
            cur.state.as_str(),
            "completed" | "failed" | "skipped" | "canceled"
        ) {
            break (snap, cur);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for script-only schedule run: {cur:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    assert_eq!(finished_ok.1.state, "completed");
    assert!(finished_ok.1.terminal_id.is_none());
    assert!(
        finished_ok.0.terminals.iter().all(|t| t.id != ok_run.id),
        "Script-only run must not allocate a TerminalEntry"
    );
    assert_eq!(finished_ok.1.steps.len(), 1);
    assert_eq!(finished_ok.1.steps[0].kind, "script");
    assert_eq!(finished_ok.1.steps[0].state, "completed");

    // 2. Failing script with `on_failure: ask_agent` lazily allocates TerminalEntry during failure transition.
    let fail_sched = app
        .call(
            "schedule.create",
            json!({
                "scope": "global",
                "name": "Dirty Escalation",
                "cron": "0 5 * * *",
                "mode": "script_agent",
                "script": "echo 'dirty worktree found' >&2; exit 2",
                "profile_id": "developer",
                "instruction": "Inspect dirty paths and decide.",
                "request_key": "lazy-fail-create"
            }),
        )
        .unwrap();
    let fail_id = fail_sched["schedule"]["id"].as_str().unwrap().to_owned();
    let fail_run: ScheduledRun = serde_json::from_value(
        app.call(
            "schedule.trigger",
            json!({"schedule_id": fail_id, "request_key": "lazy-fail-trigger"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(fail_run.terminal_id.is_none());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let escalated = loop {
        app.pump_schedules().unwrap();
        let snap = app.snapshot().unwrap();
        let cur = snap
            .scheduled_runs
            .iter()
            .find(|r| r.id == fail_run.id)
            .unwrap()
            .clone();
        if cur.terminal_id.is_some() {
            break (snap, cur);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for on_failure lazy TerminalEntry allocation: {cur:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    assert!(
        escalated.0.terminals.iter().any(|t| t.id == fail_run.id
            && t.origin_project_id
                .as_deref()
                .is_some_and(canban_types::is_global_project_id)),
        "Escalation to agent step must allocate a Global TerminalEntry"
    );
    assert!(
        escalated
            .1
            .steps
            .iter()
            .any(|s| s.kind == "script" && s.state == "failed" && s.exit_code == Some(2))
    );
}

#[test]
fn cardless_blocked_report_keeps_pty_alive_until_completed() {
    let (_dir, app, p, _c) = setup();
    let created = app
        .call(
            "schedule.create",
            json!({
                "project_id": p.id,
                "name": "Ask User Task",
                "cron": "0 6 * * *",
                "mode": "agent",
                "profile_id": "developer",
                "instruction": "Ask the user when unsure.",
                "request_key": "blocked-pty-create"
            }),
        )
        .unwrap();
    let sid = created["schedule"]["id"].as_str().unwrap().to_owned();
    let run: ScheduledRun = serde_json::from_value(
        app.call(
            "schedule.trigger",
            json!({"schedule_id": sid, "request_key": "blocked-pty-trigger"}),
        )
        .unwrap(),
    )
    .unwrap();
    let now_ms = canban_types::now();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut cur: ScheduledRun = db.get(&run.id)?;
            cur.state = "running".into();
            cur.started_at = Some(now_ms);
            db.save(&cur)?;
            let mut entry: TerminalEntry = db.get(&run.id)?;
            entry.runtime = Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: Some("offline-core".into()),
                core_id: Some("offline-core".into()),
                terminal: Some("pty-blocked-1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("idle".into()),
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            });
            db.save(&entry)
        })
        .unwrap();

    // Reporting `blocked` preserves the live PTY across pump_schedules().
    app.call(
        "execution.report",
        json!({
            "schedule_run_id": run.id,
            "request_key": "blocked-report-1",
            "state": "blocked",
            "message": "Found dirty worktree with untracked patch; keep or reset?"
        }),
    )
    .unwrap();
    app.pump_schedules().unwrap();
    let mid = app.snapshot().unwrap();
    let mid_run = mid.scheduled_runs.iter().find(|r| r.id == run.id).unwrap();
    let mid_term = mid.terminals.iter().find(|t| t.id == run.id).unwrap();
    assert_eq!(mid_run.state, "blocked");
    assert_eq!(mid_term.runtime.as_ref().unwrap().state, "running");
    assert!(mid.terminal_is_live(mid_term));

    // Reporting `completed` after user answers allows PTY reclamation.
    app.call(
        "execution.report",
        json!({
            "schedule_run_id": run.id,
            "request_key": "blocked-report-2",
            "state": "completed",
            "message": "User chose to keep; done."
        }),
    )
    .unwrap();
    app.pump_schedules().unwrap();
    let done = app.snapshot().unwrap();
    let done_run = done.scheduled_runs.iter().find(|r| r.id == run.id).unwrap();
    let done_term = done.terminals.iter().find(|t| t.id == run.id).unwrap();
    assert_eq!(done_run.state, "completed");
    assert_eq!(done_term.runtime.as_ref().unwrap().state, "finished");
    assert!(!done.terminal_is_live(done_term));
}

#[test]
fn worktree_gc_removes_clean_old_merged_and_preserves_dirty() {
    let (dir, app, _p, _c) = setup();
    let repo_dir = dir.path().join("gc_repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    let git = |args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .arg("-C")
            .arg(&repo_dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Canban Test"]);
    git(&["config", "user.email", "test@canban.local"]);
    std::fs::write(repo_dir.join("README.md"), "init\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-m", "init"]);

    let wt_root = dir.path().join("home").join("worktrees").join("gc");
    std::fs::create_dir_all(&wt_root).unwrap();
    let clean_wt = wt_root.join("wt_clean");
    let dirty_wt = wt_root.join("wt_dirty");
    git(&[
        "worktree",
        "add",
        "-b",
        "feat/clean",
        clean_wt.to_str().unwrap(),
        "main",
    ]);
    git(&[
        "worktree",
        "add",
        "-b",
        "feat/dirty",
        dirty_wt.to_str().unwrap(),
        "main",
    ]);
    std::fs::write(dirty_wt.join("UNCOMMITTED.txt"), "wip changes\n").unwrap();

    let proj: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "GC Repo", "directory": repo_dir}),
        )
        .unwrap(),
    )
    .unwrap();
    let old_merged_at = canban_types::now().saturating_sub(10 * 24 * 60 * 60 * 1_000);

    let mut clean_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": proj.id, "title": "Clean merged"}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut dirty_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": proj.id, "title": "Dirty merged"}),
        )
        .unwrap(),
    )
    .unwrap();

    app.store
        .lock()
        .unwrap()
        .write(|db| {
            clean_card.worktree = Some(clean_wt.to_string_lossy().into_owned());
            clean_card.branch = Some("feat/clean".into());
            clean_card.delivery = Some(canban_types::Delivery {
                target_branch: "main".into(),
                commit: "0000000".into(),
                request_key: "gc-clean".into(),
                merged_at: old_merged_at,
            });
            db.save(&clean_card)?;

            dirty_card.worktree = Some(dirty_wt.to_string_lossy().into_owned());
            dirty_card.branch = Some("feat/dirty".into());
            dirty_card.delivery = Some(canban_types::Delivery {
                target_branch: "main".into(),
                commit: "0000000".into(),
                request_key: "gc-dirty".into(),
                merged_at: old_merged_at,
            });
            db.save(&dirty_card)
        })
        .unwrap();

    let gc = app
        .call(
            "worktree.gc",
            json!({
                "merged_before": "7d",
                "project_id": proj.id
            }),
        )
        .unwrap();
    let removed = gc["removed"].as_array().unwrap();
    let dirty = gc["dirty"].as_array().unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["card_id"], json!(clean_card.id));
    assert!(
        !clean_wt.exists(),
        "Clean old merged worktree must be removed"
    );

    assert_eq!(dirty.len(), 1);
    assert_eq!(dirty[0]["card_id"], json!(dirty_card.id));
    assert_eq!(dirty[0]["reason"], json!("uncommitted_changes"));
    assert!(
        dirty_wt.exists(),
        "Dirty worktree must be preserved for agent/user"
    );

    let snap = app.snapshot().unwrap();
    let updated_clean = snap.cards.iter().find(|c| c.id == clean_card.id).unwrap();
    assert!(updated_clean.worktree.is_none());
}
