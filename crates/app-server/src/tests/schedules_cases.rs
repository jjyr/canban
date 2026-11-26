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
                core_id: Some("offline-core".into()),
                terminal: Some("pty-sched-1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("idle".into()),
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
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
