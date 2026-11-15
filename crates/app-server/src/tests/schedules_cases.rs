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
        .filter(|w| w.card_id == card.id)
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
