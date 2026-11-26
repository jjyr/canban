use super::*;

#[test]
fn admission_projection_matches_full_state_without_loading_history() {
    let (_dir, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"stage":"develop","revision":c.revision}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c.id,"body":"Question","mentions":["codex"]}),
    )
    .unwrap();
    let other = app
        .call(
            "card.create",
            json!({"view":"full","project_id":p.id,"title":"Second"}),
        )
        .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id":other["id"],"stage":"develop","revision":other["revision"]}),
    )
    .unwrap();
    let full = app.snapshot().unwrap();
    assert!(!full.card_summaries.is_empty());
    assert!(!full.questions.is_empty());
    let narrow = app.store.lock().unwrap().db().admission_snapshot().unwrap();
    assert!(narrow.card_summaries.is_empty());
    assert!(narrow.workflow_revisions.is_empty());
    assert_eq!(narrow.questions, full.questions);
    assert_eq!(
        canban_scheduler::active_count(&narrow),
        canban_scheduler::active_count(&full)
    );
    for slots in 0..=4 {
        assert_eq!(
            canban_scheduler::admissions(&narrow, slots),
            canban_scheduler::admissions(&full, slots)
        );
    }
}
fn plan_yaml(id: &str, instruction: &str) -> String {
    format!(
        r#"schema: 1
id: {id}
name: Plan and deliver
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: plan
  develop:
    event: card.column_entered
    start_at: build
    match:
      column: develop
steps:
  - id: plan
    uses: agent
    with:
      profile: codex
      instruction: {instruction}
    next: approve
  - id: approve
    uses: approval
    with:
      title: Approve this exact plan
    next: move
    on_changes_requested: plan
  - id: move
    uses: card.move
    with:
      column: develop
    next: build
  - id: build
    uses: agent
    with:
      profile: developer
      instruction: Implement the approved plan
    next: finish
  - id: finish
    uses: complete
"#
    )
}
fn upsert_yaml(app: &AppServer, p: &Project, filename: &str, source: &str) -> Result<Value> {
    let c = app.call("workflow.sources", json!({"project_id":p.id}))?;
    let mut files = c["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["filename"] != filename)
        .map(|f| json!({"filename":f["filename"],"source":f["source"]}))
        .collect::<Vec<_>>();
    files.push(json!({"filename":filename,"source":source}));
    app.call(
        "workflow.apply",
        json!({"project_id":p.id,"revision":c["revision"],"sources":files}),
    )
}
fn publish(app: &AppServer, p: &Project, id: &str, instruction: &str) {
    upsert_yaml(app, p, &format!("{id}.yaml"), &plan_yaml(id, instruction)).unwrap();
}
fn finish(app: &AppServer, req: &TerminalEntry, key: &str) {
    let a = admit(app, req);
    app.call("execution.report",json!({"terminal_id":a.id,"request_key":key,"state":"completed","message":"Plan ready","body":"Exact plan artifact"})).unwrap();
}
#[test]
fn event_workflows_use_final_card_binding_and_failures_do_not_rollback_source_commands() {
    let (_dir, app, p, _) = setup();
    publish(&app, &p, "plan", "Plan");
    let sub=app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"project_id":p.id,"types":["card.created"]},"target":{"type":"workflow","workflow_id":"plan","entry":"start"}})).unwrap();
    let c = app
        .call(
            "card.create",
            json!({"view":"full","project_id":p.id,"title":"Automatic"}),
        )
        .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.workflow_runs.len(), 1);
    assert_eq!(s.workflow_runs[0].card_id.as_deref(), c["id"].as_str());
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(
        app.call(
            "subscription.deliveries",
            json!({"subscription_id":sub["id"]})
        )
        .unwrap()[0]["state"],
        "completed"
    );
    // Another matching event on an already-running Card leaves a visible failure, not lost content.
    app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"types":["comment.created"]},"target":{"type":"workflow","workflow_id":"plan","entry":"start"}})).unwrap();
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c["id"],"body":"Still committed","mentions":[]}),
    )
    .unwrap();
    assert!(
        app.events()
            .unwrap()
            .iter()
            .any(|e| e.message == "Still committed")
    );
    assert_eq!(app.snapshot().unwrap().terminals.len(), 1);
}
#[test]
fn remote_metadata_deduplication_survives_cache_clear_and_has_source_scope() {
    let (_dir, app, _p, _c) = setup();
    let path = "/repos/example/repo/issues/42";
    let mut issue = json!({"id":42,"title":"DO_NOT_PERSIST_TITLE","body":"DO_NOT_PERSIST_BODY","state":"open","updated_at":"2026-11-11T01:00:00Z","user":{"id":1}});
    app.observe_github_response(path, &issue).unwrap();
    app.observe_github_response(path, &issue).unwrap();
    issue["body"] = json!("CHANGED_SECRET");
    issue["updated_at"] = json!("2026-11-11T02:00:00Z");
    app.observe_github_response(path, &issue).unwrap();
    let events = app
        .call(
            "event.list",
            json!({"selector":{"repository":"example/repo","types":["github.*"]}}),
        )
        .unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 2);
    assert_eq!(events["events"][1]["kind"], "github.issue.edited");
    let persisted = serde_json::to_string(&app.events().unwrap()).unwrap();
    assert!(!persisted.contains("DO_NOT_PERSIST"));
    assert!(!persisted.contains("CHANGED_SECRET"));
}

#[test]
fn conditions_notifications_and_column_views_use_one_canonical_event() {
    let (_dir, app, p, c) = setup();
    let conditional_yaml = format!(
        r#"schema: 1
id: conditional
name: Conditional observer
on:
  start:
    event: manual
    start_at: choose
steps:
  - id: choose
    uses: condition
    with:
      pointer: /card/title
      equals: {:?}
      then: notify
      otherwise: done
  - id: notify
    uses: notify
    with:
      message: Condition selected
    next: done
  - id: done
    uses: complete
"#,
        c.title
    );
    upsert_yaml(&app, &p, "conditional.yaml", &conditional_yaml).unwrap();
    let run = app
        .call(
            "workflow.start",
            json!({"workflow_id":"conditional","card_id":c.id,"entry":"start"}),
        )
        .unwrap();
    assert_eq!(run["state"], "completed");
    assert!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .all(|t| t.agent.is_none())
    );
    assert_eq!(
        app.snapshot().unwrap().notices[0].message,
        "Condition selected"
    );
    let sub=app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"card_id":c.id,"types":["card.column_entered","card.column_left"]},"target":{"type":"consumer"}})).unwrap();
    let card = app.snapshot().unwrap().cards[0].clone();
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":card.revision,"stage":"develop"}),
    )
    .unwrap();
    let deliveries = app
        .call(
            "subscription.deliveries",
            json!({"subscription_id":sub["id"]}),
        )
        .unwrap();
    assert_eq!(deliveries.as_array().unwrap().len(), 1);
}

#[test]
fn complete_remote_collection_detects_removal_without_persisting_bodies() {
    let (_dir, app, _p, _c) = setup();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            events::github::complete_collection(
                db,
                "example/repo",
                42,
                "issue",
                "discussion",
                &[json!({"id":5,"body":"PRIVATE BODY","user":{"id":1}})],
            )
        })
        .unwrap();
    app.github.cache.clear().unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            events::github::complete_collection(db, "example/repo", 42, "issue", "discussion", &[])
        })
        .unwrap();
    let events = app
        .call(
            "event.list",
            json!({"selector":{"types":["github.issue.comment_deleted"]}}),
        )
        .unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 1);
    assert!(
        !serde_json::to_string(&app.events().unwrap())
            .unwrap()
            .contains("PRIVATE BODY")
    );
}

#[test]
fn downgrade_cannot_discard_user_workflow_definitions() {
    let (dir, app, p, _) = setup();
    publish(&app, &p, "retained", "Keep this definition");
    drop(app);
    let home = dir.path().join("home");
    let error = canban_storage::migrations::migrate(&home, 10).unwrap_err();
    assert!(error.to_string().contains("YAML workflow sources"));
    assert_eq!(
        canban_storage::migrations::database_version(&home).unwrap(),
        canban_storage::migrations::CURRENT
    );
    let store = Store::open(&home).unwrap();
    assert!(
        store
            .db()
            .list::<WorkflowRevision>()
            .unwrap()
            .iter()
            .any(|w| w.workflow_id == "retained")
    );
    assert_eq!(store.db().list::<Card>().unwrap().len(), 1);
}

#[test]
fn service_lock_ends_with_last_owner_even_if_a_child_inherited_its_descriptor() {
    let (dir, app, _p, _c) = setup();
    let inherited = app._service_lock.try_clone().unwrap();
    let owner = app.clone();
    drop(app);
    assert!(AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).is_err());
    drop(owner);
    let next = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    drop(inherited);
    assert!(AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).is_err());
    drop(next);
}

fn yaml(id: &str, event: &str) -> String {
    format!(
        "# Preserve this comment\nschema: {}\nid: {id}\nname: Test YAML\non:\n  start:\n    event: {event}\n    start_at: gate\nsteps:\n  - id: gate\n    uses: approval\n    with:\n      title: Inspect YAML\n",
        canban_workflow_engine::migrations::CURRENT
    )
}
fn collection(app: &AppServer, p: &Project) -> Value {
    app.call("workflow.sources", json!({"project_id":p.id}))
        .unwrap()
}
fn apply_yaml(app: &AppServer, p: &Project, source: &str) -> Value {
    let c = collection(app, p);
    let mut files = c["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| json!({"filename":f["filename"],"source":f["source"]}))
        .collect::<Vec<_>>();
    files.push(json!({"filename":"custom.yaml","source":source}));
    app.call(
        "workflow.apply",
        json!({"project_id":p.id,"revision":c["revision"],"sources":files}),
    )
    .unwrap()
}
#[test]
fn yaml_bytes_publication_conflict_and_validation_preserve_active_revision() {
    let (dir, app, p, _) = setup();
    let raw = yaml("raw-source", "manual");
    let c = apply_yaml(&app, &p, &raw);
    let custom = c["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["filename"] == "custom.yaml")
        .unwrap();
    assert_eq!(custom["source"], raw);
    let active = custom["active"].clone();
    assert_eq!(
        app.call(
            "workflow.get",
            json!({"workflow_id":"raw-source","revision":1})
        )
        .unwrap()["source_yaml"],
        raw
    );
    assert!(
        !dir.path()
            .join("home/projects")
            .join(&p.id)
            .join("workflows")
            .exists()
    );
    assert!(!dir.path().join("home/config/workflows").exists());
    let mut inputs = c["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| json!({"filename":f["filename"],"source":f["source"]}))
        .collect::<Vec<_>>();
    inputs
        .iter_mut()
        .find(|f| f["filename"] == "custom.yaml")
        .unwrap()["source"] = json!(raw.replace("uses: approval", "uses: unsupported"));
    assert!(
        app.call(
            "workflow.apply",
            json!({"project_id":p.id,"revision":c["revision"],"sources":inputs})
        )
        .is_err()
    );
    assert_eq!(collection(&app, &p)["revision"], c["revision"]);
    let mut updated = c["sources"].as_array().unwrap().to_vec();
    updated
        .iter_mut()
        .find(|s| s["filename"] == "custom.yaml")
        .unwrap()["source"] = json!(format!("{raw}# Concurrent SQL publication\n"));
    app.call(
        "workflow.apply",
        json!({"project_id":p.id,"revision":c["revision"],"sources":updated}),
    )
    .unwrap();
    assert!(
        app.call(
            "workflow.apply",
            json!({"project_id":p.id,"revision":c["revision"],"sources":c["sources"]})
        )
        .unwrap_err()
        .to_string()
        .contains("revision_conflict")
    );
    let current = collection(&app, &p);
    assert_ne!(current["revision"], c["revision"]);
    assert_eq!(
        app.call(
            "workflow.get",
            json!({"workflow_id":"raw-source","revision":1})
        )
        .unwrap()["source_yaml"],
        raw
    );
    assert_eq!(active["revision"], 1);
}

#[test]
fn multiple_yaml_event_rules_queue_without_expiring_and_capture_definition() {
    let (_dir, app, p, c) = setup();
    let source = yaml("multiple", "comment.created").replace(
        "steps:\n",
        "  second:\n    event: comment.created\n    start_at: gate\nsteps:\n",
    );
    apply_yaml(&app, &p, &source);
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c.id,"body":"Start both","mentions":[]}),
    )
    .unwrap();
    let deliveries = app.call("subscription.deliveries", json!({})).unwrap();
    let pending = deliveries
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["state"] == "pending")
        .unwrap();
    assert_eq!(pending["definition_revision"], 1);
    assert_eq!(pending["attempts"], 0);
    let run = app.snapshot().unwrap().workflow_runs[0].clone();
    let col = collection(&app, &p);
    let mut inputs = col["sources"].as_array().unwrap().to_vec();
    inputs
        .iter_mut()
        .find(|f| f["filename"] == "custom.yaml")
        .unwrap()["source"] = json!(source.replace("Inspect YAML", "NEW TITLE"));
    app.call(
        "workflow.apply",
        json!({"project_id":p.id,"revision":col["revision"],"sources":inputs}),
    )
    .unwrap();
    app.call("workflow.stop", json!({"run_id":run.id})).unwrap();
    // Make the persisted queue eligible without sleeping; this is a scheduling check.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut d: EventDelivery = db.get(pending["id"].as_str().unwrap())?;
            d.next_attempt_at = 0;
            db.save(&d)?;
            crate::events::pump(db, &app.config())
        })
        .unwrap();
    let s = app.snapshot().unwrap();
    let next = s.workflow_runs.iter().find(|r| r.id != run.id).unwrap();
    assert_eq!(next.definition_revision, 1);
    assert!(
        matches!(&next.definition.steps["gate"],WorkflowStep::Approval{title,..} if title=="Inspect YAML")
    );
}
#[test]
fn configuration_sessions_have_project_ownership_reuse_and_exact_history_resume() {
    let (_dir, app, p, _) = setup();
    let prompt = app
        .call("workflow.prompt", json!({"project_id":p.id}))
        .unwrap()["prompt"]
        .clone();
    let prompt_str = prompt.as_str().unwrap();
    assert!(
        prompt_str.contains("Workflow Configuration Assistant"),
        "workflow.prompt must use English instructions"
    );
    assert!(
        !prompt_str
            .chars()
            .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
        "workflow.prompt must not contain Chinese characters"
    );
    let params = json!({"project_id":p.id,"profile_id":"codex","prompt":prompt});
    let first = app.call("configuration.open", params.clone()).unwrap();
    let again = app.call("configuration.open", params.clone()).unwrap();
    assert_eq!(first["terminal_id"], again["terminal_id"]);
    assert_eq!(again["reused"], true);
    let s = app.snapshot().unwrap();
    assert!(s.terminals.iter().all(|t| t.agent.is_none()));
    assert_eq!(s.cards.len(), 1);
    assert_eq!(s.configuration_sessions.len(), 1);
    let mut c = s.configuration_sessions[0].clone();
    assert_eq!(c.prompt, prompt_str);
    c.provider_session_id = Some("exact-history".into());
    c.state = "finished".into();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&c)?;
            let mut t: TerminalEntry = db.get(&c.id)?;
            t.runtime = Some(TerminalRuntime {
                core_id: Some("isolated".into()),
                terminal: Some("local/closed".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            });
            db.save(&t)
        })
        .unwrap();
    app.call("configuration.open", params).unwrap();
    let resumed = app.snapshot().unwrap().configuration_sessions[0].clone();
    assert_eq!(
        resumed.provider_session_id.as_deref(),
        Some("exact-history")
    );
    assert_eq!(resumed.generation, 2);
    assert!(
        app.call(
            "configuration.bind",
            json!({"configuration_id":c.id,"generation":1,"session_id":"wrong"})
        )
        .is_err()
    );
}

#[test]
fn configuration_open_uses_configured_default_agent_and_auto_fresh_fallback() {
    let (_dir, app, p, _) = setup();
    assert_eq!(app.config().default_agent, "developer");
    let opened = app
        .call(
            "configuration.open",
            json!({"project_id": p.id, "auto_fresh": true}),
        )
        .unwrap();
    let expected_key = format!("configuration:{}:workflows:developer", p.id);
    assert_eq!(opened["terminal_id"], expected_key);
    assert_eq!(opened["reused"], false);

    // Simulate an ended session where exact provider history was never captured:
    // strict open still guards history, whereas direct `auto_fresh` starts cleanly.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut s: ConfigurationSession = db.get(&expected_key)?;
            s.provider_session_id = None;
            s.state = "finished".into();
            db.save(&s)?;
            let mut t: TerminalEntry = db.get(&expected_key)?;
            t.runtime = Some(TerminalRuntime {
                core_id: Some("isolated".into()),
                terminal: Some("local/closed".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            });
            db.save(&t)
        })
        .unwrap();

    assert!(
        app.call("configuration.open", json!({"project_id": p.id}))
            .is_err(),
        "Without auto_fresh or fresh, missing provider history must still error"
    );
    let restarted = app
        .call(
            "configuration.open",
            json!({"project_id": p.id, "auto_fresh": true}),
        )
        .unwrap();
    assert_eq!(restarted["terminal_id"], expected_key);
    assert_eq!(restarted["reused"], false);
    let session = app
        .snapshot()
        .unwrap()
        .configuration_sessions
        .into_iter()
        .find(|c| c.id == expected_key)
        .unwrap();
    assert_eq!(session.profile_id, "developer");
    assert_eq!(session.generation, 2);
    assert!(session.provider_session_id.is_none());
}

#[test]
fn sql_publication_rolls_back_original_yaml_revisions_and_trigger_changes_together() {
    let (_dir, app, p, _) = setup();
    let before = collection(&app, &p);
    let revisions = app
        .store
        .lock()
        .unwrap()
        .db()
        .list::<WorkflowRevision>()
        .unwrap()
        .len();
    app.store.lock().unwrap().write(|db| {db.conn.execute_batch("CREATE TRIGGER reject_revision BEFORE INSERT ON workflow_revisions BEGIN SELECT RAISE(ABORT,'injected publication failure'); END;")?;Ok(())}).unwrap();
    let mut rows = before["sources"].as_array().unwrap().to_vec();
    rows.push(json!({"filename":"fail.yaml","source":yaml("rollback-source","card.created")}));
    assert!(
        app.call(
            "workflow.apply",
            json!({"project_id":p.id,"revision":before["revision"],"sources":rows})
        )
        .is_err()
    );
    assert_eq!(collection(&app, &p), before);
    assert_eq!(
        app.store
            .lock()
            .unwrap()
            .db()
            .list::<WorkflowRevision>()
            .unwrap()
            .len(),
        revisions
    );
    assert!(
        !app.snapshot()
            .unwrap()
            .event_subscriptions
            .iter()
            .any(|s| s.id.contains("rollback-source"))
    );
}
#[test]
fn closing_a_queued_configuration_agent_cancels_launch_without_creating_a_card() {
    let (_dir, app, p, _) = setup();
    let opened = app
        .call(
            "configuration.open",
            json!({"project_id":p.id,"profile_id":"codex"}),
        )
        .unwrap();
    app.call(
        "terminal.close",
        json!({"terminal_id":opened["terminal_id"]}),
    )
    .unwrap();
    let snapshot = app.snapshot().unwrap();
    assert_eq!(snapshot.configuration_sessions[0].state, "cancelled");
    assert!(snapshot.terminals.iter().all(|t| t.agent.is_none()));
    assert_eq!(snapshot.cards.len(), 1);
}

#[test]
fn stale_configuration_observation_never_overwrites_newly_queued_generation() {
    let (_dir, app, p, _) = setup();
    let opened = app
        .call(
            "configuration.open",
            json!({"project_id": p.id, "auto_fresh": true}),
        )
        .unwrap();
    let key = opened["terminal_id"].as_str().unwrap().to_owned();

    // Simulate Generation 1 finished terminal whose ConfigurationSession is still `running`.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut s: ConfigurationSession = db.get(&key)?;
            s.provider_session_id = Some("exact-gen1-history".into());
            s.state = "running".into();
            db.save(&s)?;
            let mut t: TerminalEntry = db.get(&key)?;
            t.created_at = 1_000;
            t.runtime = Some(TerminalRuntime {
                core_id: Some("isolated".into()),
                terminal: Some("local/gen1".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            });
            db.save(&t)
        })
        .unwrap();

    // Snapshot captured by `pump_configuration` BEFORE `configuration.open` runs.
    let stale_snapshot_session = app
        .snapshot()
        .unwrap()
        .configuration_sessions
        .into_iter()
        .find(|c| c.id == key)
        .unwrap();
    assert_eq!(stale_snapshot_session.generation, 1);
    assert_eq!(stale_snapshot_session.state, "running");

    // Client concurrently calls `configuration.open` -> advances to Generation 2 (`queued`).
    let reopened = app
        .call(
            "configuration.open",
            json!({"project_id": p.id, "auto_fresh": true}),
        )
        .unwrap();
    assert_eq!(reopened["reused"], false);
    assert_eq!(reopened["state"], "queued");

    let after_open = app.snapshot().unwrap();
    let gen2 = after_open
        .configuration_sessions
        .iter()
        .find(|c| c.id == key)
        .unwrap();
    assert_eq!(gen2.generation, 2);
    assert_eq!(gen2.state, "queued");
    assert_eq!(
        gen2.provider_session_id.as_deref(),
        Some("exact-gen1-history")
    );
    let term = after_open.terminals.iter().find(|t| t.id == key).unwrap();
    assert!(
        term.created_at > 1_000,
        "requeued configuration terminal must refresh created_at for observation lag guard"
    );

    // Verify generation guard: attempting a stale gen-1 state write leaves gen-2 `queued`.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current: ConfigurationSession = db.get(&key)?;
            if current.generation == stale_snapshot_session.generation
                && current.state == stale_snapshot_session.state
            {
                current.state = "finished".into();
                db.save(&current)?;
            }
            Ok(())
        })
        .unwrap();

    let preserved = app
        .snapshot()
        .unwrap()
        .configuration_sessions
        .into_iter()
        .find(|c| c.id == key)
        .unwrap();
    assert_eq!(preserved.generation, 2);
    assert_eq!(preserved.state, "queued");
}

#[test]
fn edited_default_yaml_owns_callback_snapshots_instead_of_historical_stage_fields() {
    let (_dir, app, p, c) = setup();
    let review_yaml = format!(
        r#"schema: 1
id: delivery:{}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Develop
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: agent:review
  - id: agent:review
    uses: agent
    with:
      profile: reviewer
      instruction: Review
    next: __end
  - id: __end
    uses: complete
"#,
        p.id
    );
    upsert_yaml(&app, &p, "development.yaml", &review_yaml).unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"review"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().terminals[0].clone();
    assert!(!request.agent.as_ref().unwrap().require_human_review);
    finish(&app, &request, "edited-default-complete");
    assert!(app.snapshot().unwrap().cards[0].human_gate.is_none());
}

fn finish_closed(app: &AppServer, req: &TerminalEntry, key: &str) {
    let a = admit(app, req);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&a.id)?;
            let run = entry.agent.as_mut().unwrap();
            // Simulate a real retained interactive Herdr terminal whose Agent is still mid-turn
            // when invoking `canban report`, then finishes its turn (`done`) right after.
            run.terminal = Some("t-retained".into());
            run.connection_state = "connected".into();
            run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            db.save(&entry)
        })
        .unwrap();
    app.call("execution.report",json!({"terminal_id":a.id,"request_key":key,"state":"completed","message":"Plan ready","body":"Exact plan artifact"})).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&a.id)?;
            entry.agent.as_mut().unwrap().observed =
                Some(json!({"state": "live", "status": {"state": "done"}}));
            db.save(&entry)
        })
        .unwrap();
}

fn git_in(cwd: &std::path::Path, args: &[&str]) -> String {
    let out = canban_exec::raw_command("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}
fn git_config_path(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let dot_git = cwd.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("config"));
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let raw = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = if std::path::Path::new(raw).is_absolute() {
        std::path::PathBuf::from(raw)
    } else {
        cwd.join(raw)
    };
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .ok()
        .map(|s| gitdir.join(s.trim()))
        .unwrap_or(gitdir);
    Some(common.join("config"))
}
fn git_identity(cwd: &std::path::Path) {
    if let Some(config_path) = git_config_path(cwd)
        && let Ok(existing) = std::fs::read_to_string(&config_path)
    {
        if existing.contains("email = tester@example.invalid") {
            return;
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&config_path)
            && f.write_all(
                b"\n[user]\n\tname = Tester\n\temail = tester@example.invalid\n[commit]\n\tgpgsign = false\n",
            )
            .is_ok()
        {
            return;
        }
    }
    git_in(cwd, &["config", "user.name", "Tester"]);
    git_in(cwd, &["config", "user.email", "tester@example.invalid"]);
    git_in(cwd, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn queue_merge_conflict_respects_custom_deliver_profile_and_rejects_unknown_profile() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    std::fs::write(source.join("README.md"), "base\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "init"]);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Custom Deliver Profile", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();

    // Unknown profile on `deliver` must be rejected by `workflow.apply`.
    let col = collection(&app, &p);
    let raw = col["sources"][0]["source"].as_str().unwrap();
    let mut bad_doc = canban_workflow_engine::source::parse(raw).unwrap();
    bad_doc
        .steps
        .iter_mut()
        .find(|s| s.uses == "deliver")
        .unwrap()
        .with
        .insert("profile".into(), json!("nonexistent-merger"));
    assert!(
        app.call(
            "workflow.apply",
            json!({
                "project_id": p.id,
                "revision": col["revision"],
                "sources": [{"filename": "development.yaml", "source": canban_workflow_engine::source::encode(&bad_doc).unwrap()}]
            }),
        )
        .is_err()
    );

    // Custom `with.profile: reviewer` on `deliver` overrides `@merger` for conflict resolution.
    let mut custom_doc = canban_workflow_engine::source::parse(raw).unwrap();
    custom_doc.workspace.sync_base = false;
    let deliver = custom_doc
        .steps
        .iter_mut()
        .find(|s| s.uses == "deliver")
        .unwrap();
    deliver.with.insert("profile".into(), json!("reviewer"));
    deliver.with.insert("sync_upstream".into(), json!(false));
    deliver.on_changes_requested = None;
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": col["revision"],
            "sources": [{"filename": "development.yaml", "source": canban_workflow_engine::source::encode(&custom_doc).unwrap()}]
        }),
    )
    .unwrap();

    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Conflict with custom profile", "body": "Test"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev = snap.terminals[0].clone();
    let updated_p = snap.projects.iter().find(|x| x.id == p.id).unwrap();
    let ops = canban_workspaces::Ops::local();
    let wt = ops.prepare(&app.home, updated_p, &snap.cards[0]).unwrap();
    let branch = ops.current_branch(wt.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c.id)?;
            card.worktree = Some(wt.to_str().unwrap().into());
            card.branch = Some(branch);
            db.save(&card)
        })
        .unwrap();
    git_identity(&wt);
    std::fs::write(wt.join("README.md"), "card edit\n").unwrap();
    std::fs::write(source.join("README.md"), "main edit\n").unwrap();
    git_in(&source, &["commit", "-am", "main edit"]);

    finish_closed(&app, &dev, "dev-done");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let resolve = snap
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c.id) && r.id != dev.id)
        .expect("custom conflict profile request");
    assert_eq!(resolve.agent.as_ref().unwrap().profile.id, "reviewer");
}

#[test]
fn backlog_admission_plans_ahead_k_and_starts_in_board_order_when_agent_free() {
    let (_dir, app, p, _) = setup();
    let col = collection(&app, &p);
    let raw = col["sources"][0]["source"].as_str().unwrap();
    let mut doc = canban_workflow_engine::source::parse(raw).unwrap();
    canban_workflow_engine::source::set_backlog_options(&mut doc, true, true);
    canban_workflow_engine::source::set_backlog_admission(&mut doc, true, Some(1));
    let opts = canban_workflow_engine::source::derive_options(&doc);
    assert!(opts.backlog_analyze && opts.backlog_auto_develop && opts.backlog_wait_for_agent);
    assert_eq!(opts.backlog_max_ready, Some(1));
    app.call(
        "workflow.apply",
        json!({"project_id": p.id, "revision": col["revision"], "sources": [{"filename": "development.yaml", "source": canban_workflow_engine::source::encode(&doc).unwrap()}]}),
    )
    .unwrap();
    app.call("scheduler.configure", json!({"max_agents": 1}))
        .unwrap();

    let create = |title: &str| -> Card {
        serde_json::from_value(
            app.call(
                "card.create",
                json!({"view":"full","project_id": p.id, "title": title, "body": "task"}),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let c1 = create("First");
    let c2 = create("Second");
    let c3 = create("Third");
    // Kanban priority: drag Third above Second.
    let c3_now = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == c3.id)
        .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c3.id, "revision": c3_now.revision, "stage": "backlog", "before_card_id": c2.id}),
    )
    .unwrap();

    let run_of = |snap: &Snapshot, id: &str| {
        snap.workflow_runs
            .iter()
            .find(|r| r.card_id.as_deref() == Some(id) && r.active())
            .cloned()
            .unwrap()
    };
    let snap = app.snapshot().unwrap();
    // K = 1: only the first Card is planning; the rest wait before planning.
    let plan1 = snap
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c1.id))
        .expect("first Card plans")
        .clone();
    assert!(
        plan1
            .agent
            .as_ref()
            .unwrap()
            .workflow_step
            .as_deref()
            .unwrap()
            .starts_with("agent:plan:")
    );
    for c in [&c2, &c3] {
        let r = run_of(&snap, &c.id);
        assert_eq!(r.state, "waiting_capacity");
        assert_eq!(r.step.as_deref(), Some("admit:plan"));
        assert!(
            !snap
                .terminals
                .iter()
                .filter_map(|t| t.agent.as_ref())
                .any(|q| q.card_id == c.id)
        );
    }

    // Planning finishes; the Agent that planned is still occupying the only slot.
    finish(&app, &plan1, "plan1-done");
    app.pump_admissions().unwrap();
    let snap = app.snapshot().unwrap();
    let r1 = run_of(&snap, &c1.id);
    assert_eq!(r1.state, "waiting_capacity");
    assert_eq!(r1.step.as_deref(), Some("admit:develop"));
    assert_eq!(
        snap.cards.iter().find(|c| c.id == c1.id).unwrap().stage,
        "backlog"
    );
    // The planned Card fills the plan-ahead window, so nobody else plans yet.
    assert_eq!(run_of(&snap, &c3.id).step.as_deref(), Some("admit:plan"));

    // A slot frees up: First starts Develop, then Third (Board order) begins planning.
    app.call("scheduler.configure", json!({"max_agents": 2}))
        .unwrap();
    app.pump_admissions().unwrap();
    app.pump_admissions().unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(
        snap.cards.iter().find(|c| c.id == c1.id).unwrap().stage,
        "develop"
    );
    assert!(
        snap.terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .any(|r| {
                let step = r
                    .pending
                    .as_ref()
                    .and_then(|p| p.workflow_step.as_deref())
                    .or(r.workflow_step.as_deref());
                r.card_id == c1.id && step.is_some_and(|s| s.starts_with("agent:develop:"))
            })
    );
    assert!(
        snap.terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .any(|r| r.card_id == c3.id
                && r.workflow_step
                    .as_deref()
                    .is_some_and(|s| s.starts_with("agent:plan:"))),
        "Third was dragged above Second, so it plans first"
    );
    let r2 = run_of(&snap, &c2.id);
    assert_eq!(r2.step.as_deref(), Some("admit:plan"));
    assert_eq!(r2.state, "waiting_capacity");
}

fn edit_development(
    app: &AppServer,
    p: &Project,
    edit: impl FnOnce(&mut canban_workflow_engine::source::Document),
) {
    let col = collection(app, p);
    let raw = col["sources"][0]["source"].as_str().unwrap();
    let mut doc = canban_workflow_engine::source::parse(raw).unwrap();
    canban_workflow_engine::stages::normalize(&mut doc, &[]);
    edit(&mut doc);
    app.call(
        "workflow.apply",
        json!({"project_id": p.id, "revision": col["revision"], "sources": [{"filename": "development.yaml", "source": canban_workflow_engine::source::encode(&doc).unwrap()}]}),
    )
    .unwrap();
}

#[test]
fn removing_a_column_returns_its_cards_to_the_first_column() {
    let (_dir, app, p, c) = setup();
    // Disable Review automation first so the moved Card does not start delivery.
    let moved: Card = serde_json::from_value(
        app.call(
            "card.move",
            json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "review"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(moved.stage, "review");
    edit_development(&app, &p, |d| {
        canban_workflow_engine::stages::remove_column(d, "review").unwrap();
    });
    let snap = app.snapshot().unwrap();
    let card = snap.cards.iter().find(|x| x.id == c.id).unwrap();
    assert_eq!(card.stage, "backlog");
    assert!(app.events().unwrap().iter().any(|e| e.card_id == c.id
        && e.kind == "card.stage_changed"
        && e.data["reason"] == "column_removed"));
    let project = snap.projects.iter().find(|x| x.id == p.id).unwrap();
    assert!(!project.workflow.stages.iter().any(|s| s.id == "review"));
}

#[test]
fn removing_the_running_step_finishes_it_then_stops_the_run() {
    let (_dir, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().terminals[0].clone();
    edit_development(&app, &p, |d| {
        canban_workflow_engine::stages::remove_step(d, "agent:develop").unwrap();
    });
    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(c.id.as_str()))
        .unwrap();
    assert_eq!(run.state, "waiting_agent", "the current step keeps running");
    assert_eq!(run.inputs["stop_after_step"], "agent:develop");
    finish(&app, &req, "dev-done");
    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(c.id.as_str()))
        .unwrap();
    assert_eq!(run.state, "cancelled");
    assert_eq!(
        snap.cards.iter().find(|x| x.id == c.id).unwrap().stage,
        "develop",
        "the removed step's successors (move to Review) do not run"
    );
}

#[test]
fn report_auto_commits_card_worktree_and_workflow_deliver_retries_paused_or_stopped_merge() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    std::fs::write(source.join("README.md"), "base\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "init"]);
    let remote = dir.path().join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git_in(&remote, &["init", "--bare", "-b", "main"]);
    git_in(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git_in(&source, &["push", "-u", "origin", "main"]);

    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "AutoCommit & Retry", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    // Disable human review on Review column so Develop -> Review enters Queue merge directly.
    edit_development(&app, &p, |d| {
        canban_workflow_engine::source::set_review_manual(d, false);
    });

    // Part 1: Uncommitted edits in Card worktree are auto-committed on execution.report,
    // and when the main checkout temporarily has local edits, workflow.deliver retries immediately once clean.
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Feature A", "body": "auto commit test"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev1 = snap
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c1.id))
        .unwrap()
        .clone();
    let ops = canban_workspaces::Ops::local();
    let wt1 = ops
        .prepare(
            &app.home,
            &p,
            snap.cards.iter().find(|c| c.id == c1.id).unwrap(),
        )
        .unwrap();
    let branch1 = ops.current_branch(wt1.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c1.id)?;
            card.worktree = Some(wt1.to_str().unwrap().into());
            card.branch = Some(branch1);
            db.save(&card)
        })
        .unwrap();
    git_identity(&wt1);
    let head_before = git_in(&wt1, &["rev-parse", "HEAD"]);
    // Agent writes a file WITHOUT running git commit!
    std::fs::write(wt1.join("feature_a.txt"), "hello auto commit\n").unwrap();
    // Make main checkout dirty so Queue merge pauses cleanly.
    std::fs::write(source.join("untracked_local.tmp"), "dirty\n").unwrap();

    finish_closed(&app, &dev1, "dev1-done");
    // Verify execution.report auto-committed the Card worktree!
    let head_after = git_in(&wt1, &["rev-parse", "HEAD"]);
    assert_ne!(
        head_before, head_after,
        "Card worktree should be auto-committed on completion"
    );
    assert!(
        git_in(&wt1, &["status", "--porcelain"]).is_empty(),
        "Card worktree must be clean after auto-commit"
    );

    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let run1 = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(c1.id.as_str()) && r.active())
        .unwrap();
    assert_eq!(
        run1.state, "waiting_delivery",
        "dirty main checkout keeps the Card queued non-destructively at the head of the merge queue"
    );
    let pause_reason = run1.queue_pause_reason().unwrap();
    assert!(
        pause_reason.contains("Main repository checkout")
            && pause_reason.contains("untracked_local.tmp"),
        "expected path and dirty filename in queue_pause_reason, got: {pause_reason}"
    );
    let card1 = snap.cards.iter().find(|c| c.id == c1.id).unwrap();
    assert!(
        card1.human_gate.is_none(),
        "transient dirty main checkout must not emit noisy human-gate blocks"
    );

    // Clean main checkout and manually trigger retry via workflow.deliver (automatic pump also drains it).
    std::fs::remove_file(source.join("untracked_local.tmp")).unwrap();
    let retried = app
        .call(
            "workflow.deliver",
            json!({"card_id": c1.id, "revision": card1.revision}),
        )
        .unwrap();
    assert_eq!(
        retried["state"], "completed",
        "workflow.deliver synchronously pumps delivery"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("feature_a.txt")).unwrap(),
        "hello auto commit\n"
    );

    // Part 2: If a workflow run was stopped after hitting a merge conflict (leaving no active run
    // and an interrupted MERGE_HEAD in the Card worktree), workflow.deliver starts a fresh run
    // at deliver:review and routes the pending conflict back to @developer!
    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Feature B", "body": "stopped conflict retry"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c2.id, "revision": c2.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev2 = snap
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c2.id))
        .unwrap()
        .clone();
    let wt2 = ops
        .prepare(
            &app.home,
            &p,
            snap.cards.iter().find(|c| c.id == c2.id).unwrap(),
        )
        .unwrap();
    let branch2 = ops.current_branch(wt2.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c2.id)?;
            card.worktree = Some(wt2.to_str().unwrap().into());
            card.branch = Some(branch2);
            db.save(&card)
        })
        .unwrap();
    git_identity(&wt2);
    std::fs::write(wt2.join("README.md"), "card2 conflict\n").unwrap();
    std::fs::write(source.join("README.md"), "main conflict\n").unwrap();
    git_in(&source, &["commit", "-am", "main conflict"]);
    git_in(&source, &["push", "origin", "main"]);

    finish_closed(&app, &dev2, "dev2-done");
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let run2 = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(c2.id.as_str()) && r.active())
        .unwrap();
    assert_eq!(run2.state, "waiting_agent");
    // User stops the workflow, leaving the Card in `review` with no active run and an unmerged MERGE_HEAD!
    app.call("workflow.stop", json!({"run_id": run2.id}))
        .unwrap();
    let snap = app.snapshot().unwrap();
    assert!(
        !snap
            .workflow_runs
            .iter()
            .any(|r| r.card_id.as_deref() == Some(c2.id.as_str()) && r.active())
    );

    let card2 = snap.cards.iter().find(|c| c.id == c2.id).unwrap();
    let restarted = app
        .call(
            "workflow.deliver",
            json!({"card_id": c2.id, "revision": card2.revision}),
        )
        .unwrap();
    assert_eq!(
        restarted["state"], "waiting_agent",
        "retrying delivery on an interrupted merge conflict routes back to the Agent"
    );
    let snap = app.snapshot().unwrap();
    let new_req = snap
        .terminals
        .iter()
        .rfind(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c2.id))
        .unwrap()
        .clone();
    let new_instr = new_req
        .agent
        .as_ref()
        .unwrap()
        .pending
        .as_ref()
        .map(|q| q.instruction.as_str())
        .unwrap_or_else(|| new_req.agent.as_ref().unwrap().instruction.as_str());
    assert!(new_instr.contains("Merge conflict to resolve before delivery"));
}

#[test]
fn startup_and_apply_migrate_schema_1_workflows_to_current_revision_preserving_in_flight_runs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let home = dir.path().join("home");
    let project_id;
    let card_id;
    let first_run_rev;
    {
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let p: Project = serde_json::from_value(
            app.call(
                "project.create",
                json!({
                    "name": "Legacy Schema 1",
                    "directory": source,
                    "workflow_yaml": r#"schema: 1
id: delivery:legacy
name: Legacy v1
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory. Explain your changes and validation in your report.
    next: agent:custom
  - id: agent:custom
    uses: agent
    with:
      profile: reviewer
      instruction: Verify project-specific invariants.
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#
                }),
            )
            .unwrap(),
        )
        .unwrap();
        project_id = p.id.clone();
        // `project.create` with a `schema: 1` payload immediately upgrades it to CURRENT!
        let col = collection(&app, &p);
        let stored = col["sources"][0]["source"].as_str().unwrap();
        assert!(stored.contains(&format!(
            "schema: {}",
            canban_workflow_engine::migrations::CURRENT
        )));
        assert!(!stored.contains("Explain your changes and validation in your report."));
        assert!(stored.contains("Verify project-specific invariants."));

        // Simulate an existing SQLite database created by an older Canban build with `schema: 1` in `workflow_revisions`.
        let v1_raw = r#"schema: 1
id: delivery:legacy
name: Legacy v1
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory. Explain your changes and validation in your report.
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#;
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut src: WorkflowSource = db
                    .list::<WorkflowSource>()?
                    .into_iter()
                    .find(|s| s.project_id == p.id)
                    .unwrap();
                let mut rev: WorkflowRevision = db.get(&src.revision_id)?;
                rev.source_yaml = Some(v1_raw.into());
                src.hash = canban_context::digest(v1_raw.as_bytes());
                db.save(&rev)?;
                db.save(&src)
            })
            .unwrap();

        let c: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view":"full","project_id": p.id, "title": "In flight", "body": "keep rev 1"}),
            )
            .unwrap(),
        )
        .unwrap();
        card_id = c.id.clone();
        app.call(
            "card.move",
            json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "develop"}),
        )
        .unwrap();
        let snap = app.snapshot().unwrap();
        first_run_rev = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id.as_deref() == Some(c.id.as_str()))
            .unwrap()
            .definition_revision;
        assert_eq!(first_run_rev, 1);
    }

    // Reopening AppServer runs startup workflow migrations, publishing revision 2 (`schema: 2`)
    // while preserving revision 1 in history and keeping the in-flight WorkflowRun on revision 1!
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let snap = app.snapshot().unwrap();
    let p = snap.projects.iter().find(|p| p.id == project_id).unwrap();
    let col = collection(&app, p);
    assert_eq!(col["active"][0]["revision"], 2);
    let active_yaml = col["sources"][0]["source"].as_str().unwrap();
    assert!(active_yaml.starts_with(&format!(
        "schema: {}",
        canban_workflow_engine::migrations::CURRENT
    )));
    assert!(!active_yaml.contains("instruction:"));

    // Historical revision 1 remains intact for audit/rollback.
    let old_rev = app
        .call(
            "workflow.get",
            json!({"workflow_id": "delivery:legacy", "revision": 1}),
        )
        .unwrap();
    assert!(
        old_rev["source_yaml"]
            .as_str()
            .unwrap()
            .starts_with("schema: 1")
    );

    // Active run keeps its captured revision 1.
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(card_id.as_str()) && r.active())
        .unwrap();
    assert_eq!(run.definition_revision, first_run_rev);

    // Compiled revision 2 fills in the latest decision-aware built-in default prompt (`--state backlog`).
    let latest_rev = app
        .call("workflow.get", json!({"workflow_id": "delivery:legacy"}))
        .unwrap();
    let compiled_instr = latest_rev["definition"]["steps"]["agent:develop"]["instruction"]
        .as_str()
        .unwrap();
    assert!(compiled_instr.contains("--state backlog"));
    assert!(compiled_instr.contains("commit deliverables"));
}

#[test]
fn streamlined_prompt_omits_manifest_instruction_and_inlines_context_conditionally() {
    let (_dir, app, p, c) = setup();
    let moved: Card = serde_json::from_value(
        app.call(
            "card.move",
            json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "develop"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req = snap
        .terminals
        .iter()
        .find(|r| {
            r.agent
                .as_ref()
                .is_some_and(|a| a.card_id == moved.id && a.state == "queued")
        })
        .unwrap();
    let rel_prompt =
        canban_context::capture(&app.home, &p, &moved, req, &snap, &app.events().unwrap()).unwrap();
    // Internal context.json remains persisted for audit/archival, never referenced in prompt.md.
    assert!(
        app.home
            .join(&rel_prompt)
            .with_file_name("context.json")
            .exists()
    );
    let fresh_prompt = std::fs::read_to_string(app.home.join(&rel_prompt)).unwrap();
    assert!(!fresh_prompt.contains("context.json"));
    assert!(!fresh_prompt.contains("Earlier step results"));
    assert!(!fresh_prompt.contains("Earlier workflow step outputs"));
    assert!(!fresh_prompt.contains("## Card discussion"));
    assert!(!fresh_prompt.contains("## Earlier reports"));
    assert!(!fresh_prompt.contains("## Attachments"));
    assert!(fresh_prompt.contains("--state <completed|backlog|blocked|failed|working>"));
    assert!(fresh_prompt.contains("--state backlog"));

    // Finish initial request, add a human comment + attachment, and verify conditional sections appear.
    let attempt = admit(&app, req);
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": moved.number,
            "state": "blocked",
            "message": "Need API spec",
            "body": "# Plan notes\nCheck auth handler.",
            "request_key": "blocked-with-file"
        }),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"view":"full","card_id": c.id, "body": "Use OAuth2 PKCE flow."}),
    )
    .unwrap();
    let src_file = app.home.join("spec.txt");
    std::fs::write(&src_file, b"pkce-spec").unwrap();
    app.call(
        "attachment.add",
        json!({"card_id": c.id, "name": "spec.txt", "source": src_file, "media_type": "text/plain"}),
    )
    .unwrap();
    let snap2 = app.snapshot().unwrap();
    let card2 = snap2.cards.iter().find(|x| x.id == c.id).unwrap();
    let mut next_att = attempt.clone();
    next_att.agent.as_mut().unwrap().number = 2;
    let rel_prompt2 = canban_context::capture(
        &app.home,
        &p,
        card2,
        &next_att,
        &snap2,
        &app.events().unwrap(),
    )
    .unwrap();
    let rich_prompt = std::fs::read_to_string(app.home.join(&rel_prompt2)).unwrap();
    assert!(!rich_prompt.contains("context.json"));
    assert!(rich_prompt.contains("## Card discussion"));
    assert!(rich_prompt.contains("Use OAuth2 PKCE flow."));
    assert!(rich_prompt.contains("## Earlier reports"));
    assert!(rich_prompt.contains("Need API spec — file:"));
    assert!(rich_prompt.contains("## Attachments"));
    assert!(rich_prompt.contains("spec.txt"));
}

#[test]
fn report_state_backlog_stops_workflow_returns_card_with_explanation_without_retriggering_backlog()
{
    let (_dir, app, p, _initial) = setup();
    // Enable an automatic rule on `backlog` entry as well so we verify `--state backlog`
    // never causes an infinite Backlog re-entry loop.
    let yaml = format!(
        r#"schema: {}
id: delivery:{}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  backlog_analyze:
    event: card.column_entered
    start_at: agent:plan
    match:
      column: backlog
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:plan
    uses: agent
    with:
      profile: planner
    next: __end
  - id: agent:develop
    uses: agent
    with:
      profile: developer
    next: move:review
  - id: move:review
    uses: card.move
    with:
      column: review
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#,
        canban_workflow_engine::migrations::CURRENT,
        p.id
    );
    let col = collection(&app, &p);
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": col["revision"],
            "sources": [{"filename": "development.yaml", "source": yaml}]
        }),
    )
    .unwrap();

    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Legacy cleanup", "body": "Remove v0 shim"}),
        )
        .unwrap(),
    )
    .unwrap();
    // Creating in Backlog triggered `backlog_analyze`; move directly to `develop` to start `agent:develop`.
    let card: Card = serde_json::from_value(
        app.call(
            "card.move",
            json!({"view":"full","card_id": card.id, "revision": card.revision, "stage": "develop"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req = snap
        .terminals
        .iter()
        .find(|r| {
            r.agent.as_ref().is_some_and(|a| {
                a.card_id == card.id && a.stage == "develop" && a.state == "queued"
            })
        })
        .unwrap()
        .clone();
    let attempt = admit(&app, &req);

    // Agent inspects codebase and decides the Card is already implemented -> reports `--state backlog`.
    let res = app
        .call(
            "execution.report",
            json!({
                "project_id": p.id,
                "card": card.number,
                "state": "backlog",
                "message": "Already removed in PR #42",
                "body": "Verified `src/v0_shim.rs` no longer exists on `main`.",
                "request_key": format!("backlog:{}", attempt.id)
            }),
        )
        .unwrap();
    assert_eq!(res["recorded_state"], "backlog");

    let after = app.snapshot().unwrap();
    let updated = after.cards.iter().find(|c| c.id == card.id).unwrap();
    assert_eq!(updated.stage, "backlog");
    assert!(updated.human_gate.is_none());
    assert!(updated.attention.is_none());

    // Active workflow stopped, and NO new workflow or queued request re-triggered in `backlog`!
    assert!(
        !after
            .workflow_runs
            .iter()
            .any(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active())
    );
    assert!(
        !after
            .terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .any(|r| r.card_id == card.id && (r.state == "queued" || r.pending.is_some()))
    );

    // Timeline records both the Agent's narrative explanation and the stage return reason.
    let all_events = app.events().unwrap();
    let card_events: Vec<_> = all_events.iter().filter(|e| e.card_id == card.id).collect();
    assert!(card_events.iter().any(|e| {
        e.kind == "execution.reported"
            && e.data["state"] == "backlog"
            && e.message == "Returned to Backlog: Already removed in PR #42"
            && e.data["path"].as_str().is_some()
    }));
    assert!(card_events.iter().any(|e| {
        e.kind == "card.stage_changed"
            && e.data["from"] == "develop"
            && e.data["to"] == "backlog"
            && e.data["reason"] == "agent_returned_backlog"
            && e.data["stage"] == "backlog"
            && e.data["card_revision"] == updated.revision
    }));

    // Unread notification reminder alerts the user to read the Agent's explanation on the Card.
    let reminders = canban_notifications::reminders(&after);
    assert!(reminders.iter().any(|r| r.card_id == card.id && r.unread));
}

fn snap_latest_card_terminal(app: &AppServer, card_id: &str) -> TerminalEntry {
    app.snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .rfind(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card_id))
        .unwrap()
}

#[test]
fn workflow_schema_6_migration_dual_syntax_env_outputs_and_agent_injection() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(source.join("sub")).unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();

    // 1. Discoverable `workflow.schema` reports current schema version and `script` / `env`.
    let schema = app.call("workflow.schema", json!({})).unwrap();
    assert_eq!(
        schema["schema_version"],
        canban_workflow_engine::migrations::CURRENT
    );
    assert!(schema["step_types"]["script"].is_object());
    assert!(schema["yaml"]["env"].is_string());

    // 2. Submitting a schema 5 workflow on `project.create` upgrades it to schema 6.
    let v5_yaml = r#"schema: 5
id: delivery:migrated
name: Migrated v5
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:dev
    match:
      column: develop
steps:
  - id: agent:dev
    uses: agent
    with:
      profile: developer
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#;
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Schema 6 Workflow",
                "directory": source,
                "workflow_yaml": v5_yaml
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let wf = collection(&app, &p);
    let stored = wf["sources"][0]["source"].as_str().unwrap();
    assert!(stored.contains(&format!(
        "schema: {}",
        canban_workflow_engine::migrations::CURRENT
    )));

    // 3. Apply a schema 6 workflow covering 4-layer env precedence, dual syntax,
    // `$CANBAN_ENV`, `$CANBAN_OUTPUT`, `condition` routing, and Agent PTY invalidation.
    let v6_yaml = r#"schema: 6
id: delivery:migrated
name: Migrated v6
env:
  WF_SHARED: from_wf
  OVERRIDE_ME: from_wf
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:first
    match:
      column: develop
steps:
  - id: agent:first
    name: Initial agent
    uses: agent
    env:
      OVERRIDE_ME: from_first
    with:
      profile: developer
      instruction: Initial implementation step.
    next: script:prep
  - id: script:prep
    name: Shorthand prep script
    working_directory: sub
    env:
      STEP_ONLY: from_prep
      OVERRIDE_ME: from_prep
    run: |
      pwd
      [ "$(basename "$(pwd)")" = "sub" ]
      [ -n "$CANBAN_PROJECT" ]
      [ "$CANBAN_CARD" = "1" ]
      [ -n "$CARD_ID" ]
      [ -n "$CANBAN_WORKFLOW" ]
      [ -n "$CANBAN_RUN_ID" ]
      [ "$CANBAN_STEP_ID" = "script:prep" ]
      [ "$CANBAN_STAGE" = "develop" ]
      [ -n "$CANBAN_RUN_DIR" ]
      [ -n "$CANBAN_CLI" ]
      [ "$WF_SHARED" = "from_wf" ]
      [ "$OVERRIDE_ME" = "from_prep" ]
      echo "prep-ran-$CARD_ID"
      {
        echo "DYNAMIC_VAR=hello_env"
        echo "OVERRIDE_ME=from_dotenv"
        echo "MULTI_VAR<<EOF"
        echo "line1"
        echo "line2"
        echo "EOF"
      } >> "$CANBAN_ENV"
      echo "gate=pass" >> "$CANBAN_OUTPUT"
    next: script:explicit
  - id: script:explicit
    name: Explicit python script
    uses: script
    with:
      shell: python3
      env:
        OVERRIDE_ME: from_step
      run: |
        import os
        assert os.environ["WF_SHARED"] == "from_wf"
        assert os.environ["DYNAMIC_VAR"] == "hello_env"
        assert os.environ["MULTI_VAR"] == "line1\nline2"
        assert os.environ["OVERRIDE_ME"] == "from_step"
        with open(os.environ["CANBAN_OUTPUT"], "a") as f:
            f.write("checked=yes\n")
        print("python-checked")
    next: condition:gate
  - id: condition:gate
    uses: condition
    with:
      pointer: /outputs/script:prep/gate
      equals: pass
      then: agent:dev
      otherwise: __end
  - id: agent:dev
    name: Final agent
    uses: agent
    env:
      OVERRIDE_ME: from_agent_step
      AGENT_FLAG: "1"
    with:
      profile: developer
      instruction: Finish after script checks.
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#;
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": wf["revision"],
            "sources": [{"filename": wf["sources"][0]["filename"], "source": v6_yaml}]
        }),
    )
    .unwrap();

    super::terminal_cases::core(&app, dir.path());
    // Extend `test-hc` so `terminal.list`/`terminal.get` report `status: {"state": "done"}` once
    // `done` exists, matching how live Herdr Core reports a completed turn.
    std::fs::write(
        dir.path().join("test-hc"),
        r#"#!/usr/bin/python3
import sys,json,os
base=os.path.dirname(__file__)
for line in sys.stdin:
 p=json.loads(line);m=p['method'];args=p.get('params',{});r={}
 if m=='hello':r={'generation':1}
 elif m=='server.status':r={'core':{'id':'replacement' if os.path.exists(base+'/replace') else 'original'}}
 elif m=='terminal.run':
  with open(base+'/launches','a') as f:f.write(json.dumps(args)+'\n')
  r={'address':'t1'}
 elif m in ['terminal.get','terminal.list']:
  launches=[json.loads(l) for l in open(base+'/launches')] if os.path.exists(base+'/launches') else []
  cwd=open(base+'/cwd').read() if os.path.exists(base+'/cwd') else (launches[-1]['cwd'] if launches else base)
  t={'address':'t1','cwd':launches[-1]['cwd'] if launches else base,'foreground':{'cwd':cwd},'state':'finished' if os.path.exists(base+'/finished') else 'live'}
  if os.path.exists(base+'/done') and not os.path.exists(base+'/finished'):
   t['status']={'state':'done'}
  r={'terminals':[t]} if m=='terminal.list' else t
 elif m in ['terminal.kill','terminal.interrupt','terminal.keys']:
  with open(base+'/controls','a') as f:f.write(m+'\n')
  if m=='terminal.kill':open(base+'/finished','w').close()
  r={'sent':True}
 print(json.dumps({'jsonrpc':'2.0','id':p['id'],'result':r}),flush=True)
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("test-hc"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Env flow card"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();

    // Launch `agent:first` through the real scheduler tick so `terminal` + `applied_env` are set.
    app.tick().unwrap();
    let first_entry = snap_latest_card_terminal(&app, &c.id);
    assert_eq!(
        first_entry
            .agent
            .as_ref()
            .unwrap()
            .applied_env
            .get("OVERRIDE_ME")
            .map(String::as_str),
        Some("from_first")
    );
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut e: TerminalEntry = db.get(&first_entry.id)?;
            let a = e.agent.as_mut().unwrap();
            a.connection_state = "connected".into();
            a.observed = Some(json!({"state": "live", "status": {"state": "done"}}));
            db.save(&e)
        })
        .unwrap();
    std::fs::write(dir.path().join("done"), "").unwrap();

    // Complete `agent:first`: runs `script:prep` -> `script:explicit` -> `condition:gate` -> queues `agent:dev`!
    app.call(
        "execution.report",
        json!({
            "terminal_id": first_entry.id,
            "request_key": "first-done",
            "state": "completed",
            "message": "First done",
            "body": "ok"
        }),
    )
    .unwrap();

    let snap_mid = app.snapshot().unwrap();
    let run_mid = snap_mid
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(c.id.as_str()))
        .unwrap();
    assert_eq!(run_mid.step.as_deref(), Some("agent:dev"));
    assert_eq!(run_mid.outputs["script:prep"]["gate"], "pass");
    assert_eq!(run_mid.outputs["script:explicit"]["checked"], "yes");

    // Because `$CANBAN_ENV` added `DYNAMIC_VAR`/`MULTI_VAR` and `agent:dev` changed `OVERRIDE_ME`/`AGENT_FLAG`,
    // the warm PTY cannot be continued in place and must be replaced cleanly.
    let pending_entry = snap_latest_card_terminal(&app, &c.id);
    assert!(canban_scheduler::eligible(&snap_mid, &pending_entry));
    assert!(!canban_scheduler::can_continue_in_place(
        &snap_mid,
        &pending_entry
    ));
    assert!(
        pending_entry
            .agent
            .as_ref()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .instruction
            .contains("pass")
    );

    // Next tick kills stale `t1` and launches `agent:dev` with all 4 env layers merged.
    app.tick().unwrap();
    assert!(
        std::fs::read_to_string(dir.path().join("controls"))
            .unwrap()
            .contains("terminal.kill")
    );
    let dev_entry = snap_latest_card_terminal(&app, &c.id);
    let applied = &dev_entry.agent.as_ref().unwrap().applied_env;
    assert_eq!(
        applied.get("WF_SHARED").map(String::as_str),
        Some("from_wf")
    );
    assert_eq!(
        applied.get("DYNAMIC_VAR").map(String::as_str),
        Some("hello_env")
    );
    assert_eq!(
        applied.get("MULTI_VAR").map(String::as_str),
        Some("line1\nline2")
    );
    assert_eq!(
        applied.get("OVERRIDE_ME").map(String::as_str),
        Some("from_agent_step")
    );
    assert_eq!(applied.get("AGENT_FLAG").map(String::as_str), Some("1"));
    assert!(
        dev_entry
            .agent
            .as_ref()
            .unwrap()
            .instruction
            .contains("gate")
    );

    let launches_raw = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let last_launch: Value = serde_json::from_str(launches_raw.lines().last().unwrap()).unwrap();
    assert_eq!(last_launch["env"]["WF_SHARED"], "from_wf");
    assert_eq!(last_launch["env"]["DYNAMIC_VAR"], "hello_env");
    assert_eq!(last_launch["env"]["MULTI_VAR"], "line1\nline2");
    assert_eq!(last_launch["env"]["OVERRIDE_ME"], "from_agent_step");
    assert_eq!(last_launch["env"]["AGENT_FLAG"], "1");

    // Verify persisted script log files under `cards/<number>/scripts/`.
    let script_dir = app
        .home
        .join(format!("projects/{}/cards/{}/scripts", p.id, c.number));
    let log_texts: Vec<String> = std::fs::read_dir(&script_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(
        log_texts
            .iter()
            .any(|t| t.contains(&format!("prep-ran-{}", c.id)))
    );
    assert!(log_texts.iter().any(|t| t.contains("python-checked")));
}

#[test]
fn global_default_workflow_and_project_copy_instantiate_isolated_revisions_for_new_projects() {
    let (dir, app, p1, _c1) = setup();
    let custom_dev = format!(
        r#"schema: {}
id: delivery:{}
name: Tuned Delivery
columns:
  - id: backlog
    name: Backlog
  - id: design
    name: Design
  - id: develop
    name: Develop
  - id: review
    name: Review
workspace:
  create_worktree: false
  sync_base: false
  remote: upstream
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: deliver:review
    match:
      column: review
steps:
  - id: agent:develop
    name: Implement
    uses: agent
    with:
      profile: developer
      instruction: Custom tuned developer prompt
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: deliver:review
  - id: deliver:review
    uses: deliver
    with:
      strategy: squash
      sync_upstream: false
    next: __end
  - id: __end
    uses: complete
"#,
        canban_workflow_engine::migrations::CURRENT,
        p1.id
    );
    let secondary = format!(
        r#"schema: {}
id: helper
name: Manual Helper
on:
  triage:
    event: manual
    start_at: explain
steps:
  - id: explain
    uses: agent
    with:
      profile: planner
      instruction: Explain tradeoffs
    next: __end
  - id: __end
    uses: complete
"#,
        canban_workflow_engine::migrations::CURRENT
    );
    let cur = app
        .call("workflow.sources", json!({"project_id": p1.id}))
        .unwrap();
    app.call(
        "workflow.apply",
        json!({
            "project_id": p1.id,
            "revision": cur["revision"],
            "sources": [
                {"filename": "development.yaml", "source": custom_dev},
                {"filename": "helper.yaml", "source": secondary},
            ]
        }),
    )
    .unwrap();

    // 1. Copy workflow collection directly from p1 into p2 (`workflow_source_project_id`).
    let dir_p2 = dir.path().join("repo-p2");
    std::fs::create_dir_all(&dir_p2).unwrap();
    let p2: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Copied Project",
                "directory": dir_p2,
                "workflow_source_project_id": p1.id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        p2.workflow
            .stages
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        vec!["backlog", "design", "develop", "review"]
    );
    assert!(!p2.workflow.workspace.create_worktree);
    assert!(!p2.workflow.workspace.sync_base);
    assert_eq!(p2.workflow.workspace.remote, "upstream");

    let p2_sources = app
        .call("workflow.sources", json!({"project_id": p2.id}))
        .unwrap();
    let p2_rows = p2_sources["sources"].as_array().unwrap();
    assert_eq!(p2_rows.len(), 2);
    assert_eq!(
        p2_rows[0]["active"]["workflow_id"],
        format!("delivery:{}", p2.id)
    );
    assert_eq!(
        p2_rows[1]["active"]["workflow_id"],
        format!("helper:{}", p2.id)
    );
    assert!(
        p2_rows[0]["source"]
            .as_str()
            .unwrap()
            .contains("Custom tuned developer prompt")
    );

    // Chained copy p2 -> p3 does not accumulate `:pid:pid` suffixes.
    let dir_p3 = dir.path().join("repo-p3");
    std::fs::create_dir_all(&dir_p3).unwrap();
    let p3: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Chained Copy",
                "directory": dir_p3,
                "workflow_mode": "project",
                "workflow_source_project_id": p2.id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let p3_sources = app
        .call("workflow.sources", json!({"project_id": p3.id}))
        .unwrap();
    assert_eq!(
        p3_sources["sources"][1]["active"]["workflow_id"],
        format!("helper:{}", p3.id)
    );

    // 2. Save p1 as global default (`workflow.default.set`), verify Snapshot & `project.create` default.
    assert!(
        app.call(
            "workflow.default.set",
            json!({"project_id": p1.id, "execution": {"token": "agent"}}),
        )
        .is_err(),
        "Card Agent execution tokens must not overwrite global default workflow"
    );
    let saved_tpl: WorkflowTemplate = serde_json::from_value(
        app.call("workflow.default.set", json!({"project_id": p1.id}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved_tpl.id, DEFAULT_WORKFLOW_TEMPLATE_ID);
    assert_eq!(
        saved_tpl.source_project_name.as_deref(),
        Some(p1.name.as_str())
    );
    assert_eq!(saved_tpl.sources.len(), 2);
    assert_eq!(app.snapshot().unwrap().workflow_templates.len(), 1);

    let dir_p4 = dir.path().join("repo-p4");
    std::fs::create_dir_all(&dir_p4).unwrap();
    let p4: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Uses Global Default",
                "directory": dir_p4,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(p4.workflow.stages.len(), 4);
    assert_eq!(p4.workflow.workspace.remote, "upstream");
    let p4_sources = app
        .call("workflow.sources", json!({"project_id": p4.id}))
        .unwrap();
    assert_eq!(p4_sources["sources"].as_array().unwrap().len(), 2);

    // 3. Explicit `workflow_mode: "builtin"` bypasses the custom global default.
    let dir_p5 = dir.path().join("repo-p5");
    std::fs::create_dir_all(&dir_p5).unwrap();
    let p5: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Uses Builtin Explicitly",
                "directory": dir_p5,
                "workflow_mode": "builtin",
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        p5.workflow
            .stages
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        vec!["backlog", "develop", "review"]
    );
    let p5_sources = app
        .call("workflow.sources", json!({"project_id": p5.id}))
        .unwrap();
    assert_eq!(p5_sources["sources"].as_array().unwrap().len(), 1);

    // 4. Resetting global default restores built-in default for subsequent projects.
    let reset = app.call("workflow.default.reset", json!({})).unwrap();
    assert_eq!(reset["reset"], true);
    assert!(app.snapshot().unwrap().workflow_templates.is_empty());
    assert!(app.call("workflow.default.get", json!({})).unwrap()["template"].is_null());
}

#[test]
fn delivery_gates_isolate_unrelated_repos_and_serialize_same_canonical_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();

    let init_repo_with_remote = |name: &str| -> (PathBuf, PathBuf) {
        let remote = dir.path().join(format!("{name}-remote.git"));
        let local = dir.path().join(name);
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(&local).unwrap();
        git_in(&remote, &["init", "--bare", "-b", "main"]);
        git_in(&local, &["init", "-b", "main"]);
        git_identity(&local);
        git_in(&local, &["config", "core.hooksPath", "/dev/null"]);
        git_in(
            &local,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(local.join("README.md"), format!("{name} base\n")).unwrap();
        git_in(&local, &["add", "."]);
        git_in(&local, &["commit", "-m", "initial"]);
        git_in(&local, &["push", "-u", "origin", "main"]);
        (local, remote)
    };

    let (repo_a, remote_a) = init_repo_with_remote("repo-a");
    let (repo_b, remote_b) = init_repo_with_remote("repo-b");

    // Advance both remotes from separate helper clones so `git.pull` performs a real pull.
    let push_upstream_commit = |remote: &Path, name: &str, filename: &str| {
        let pusher = dir.path().join(format!("{name}-pusher"));
        git_in(
            dir.path(),
            &["clone", remote.to_str().unwrap(), pusher.to_str().unwrap()],
        );
        git_identity(&pusher);
        git_in(&pusher, &["config", "core.hooksPath", "/dev/null"]);
        std::fs::write(pusher.join(filename), "from upstream\n").unwrap();
        git_in(&pusher, &["add", "."]);
        git_in(&pusher, &["commit", "-m", "upstream update"]);
        git_in(&pusher, &["push", "origin", "main"]);
    };
    push_upstream_commit(&remote_a, "repo-a", "upstream-a.txt");
    push_upstream_commit(&remote_b, "repo-b", "upstream-b.txt");

    // Create a symlink pointing to `repo_a` and register a second Project on that symlink.
    let repo_a_link = dir.path().join("repo-a-symlink");
    std::os::unix::fs::symlink(&repo_a, &repo_a_link).unwrap();

    let proj_a: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Repo A", "directory": repo_a, "target_branch": "main"}),
        )
        .unwrap(),
    )
    .unwrap();
    let proj_b: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Repo B", "directory": repo_b, "target_branch": "main"}),
        )
        .unwrap(),
    )
    .unwrap();
    let proj_a_alias: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Repo A Alias", "directory": repo_a_link, "target_branch": "main"}),
        )
        .unwrap(),
    )
    .unwrap();

    // Create a standalone worktree inside Repo A to verify Card/worktree `git.commit` stays unlocked.
    let wt_res = app
        .call(
            "worktree.create",
            json!({"project_id": proj_a.id, "branch": "feat/wt", "launch": "none"}),
        )
        .unwrap();
    let wt_path = wt_res["worktree"]["path"].as_str().unwrap().to_owned();
    git_identity(Path::new(&wt_path));
    git_in(
        Path::new(&wt_path),
        &["config", "core.hooksPath", "/dev/null"],
    );

    // Hold Repo A's main checkout gate (simulating active delivery on Repo A).
    let guard_a = app
        .try_lock_checkout(&proj_a, "Code delivery for Repo A (main)")
        .expect("Repo A checkout gate should be free");

    // 1. Completely different repo (`proj_b`) can `git.pull` and `git.commit` freely.
    app.call(
        "git.pull",
        json!({"project_id": proj_b.id, "request_key": "pull-b-while-a-locked"}),
    )
    .expect("git.pull on unrelated Repo B must not be blocked by Repo A delivery");
    assert_eq!(
        std::fs::read_to_string(repo_b.join("upstream-b.txt")).unwrap(),
        "from upstream\n"
    );

    std::fs::write(repo_b.join("local-b.txt"), "local commit on b\n").unwrap();
    app.call(
        "git.stage",
        json!({"project_id": proj_b.id, "paths": ["local-b.txt"]}),
    )
    .unwrap();
    app.call(
        "git.commit",
        json!({"project_id": proj_b.id, "message": "commit on B", "request_key": "commit-b-1"}),
    )
    .expect("git.commit on unrelated Repo B must not be blocked by Repo A delivery");

    // 2. Repo A's own main checkout `git.pull`, `git.sync`, and `git.commit` fail with clear contextual error.
    let err_a = app
        .call(
            "git.pull",
            json!({"project_id": proj_a.id, "request_key": "pull-a-while-locked"}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err_a.contains("Code delivery for Repo A (main) is updating this checkout"),
        "unexpected error: {err_a}"
    );
    let err_sync_a = app
        .call(
            "git.sync",
            json!({"project_id": proj_a.id, "request_key": "sync-a-while-locked"}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err_sync_a.contains("Code delivery for Repo A (main) is updating this checkout"),
        "unexpected git.sync error: {err_sync_a}"
    );

    // 3. Another Project pointing to the same physical repo via symlink is ALSO serialized.
    let err_alias = app
        .call(
            "git.pull",
            json!({"project_id": proj_a_alias.id, "request_key": "pull-alias-while-locked"}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err_alias.contains("Code delivery for Repo A (main) is updating this checkout"),
        "symlinked checkout must share the same canonical gate, got: {err_alias}"
    );

    // 4. Non-root worktree under Repo A is NOT blocked by Repo A's main checkout gate.
    std::fs::write(Path::new(&wt_path).join("wt.txt"), "worktree change\n").unwrap();
    app.call(
        "git.stage",
        json!({"project_id": proj_a.id, "cwd": wt_path, "paths": ["wt.txt"]}),
    )
    .unwrap();
    app.call(
        "git.commit",
        json!({
            "project_id": proj_a.id,
            "cwd": wt_path,
            "message": "commit in worktree",
            "request_key": "commit-wt-while-main-locked"
        }),
    )
    .expect("git.commit in a Card/managed worktree must not wait on main checkout gate");
    let wt_sync = app
        .call(
            "git.sync",
            json!({
                "project_id": proj_a.id,
                "cwd": wt_path,
                "request_key": "sync-wt-while-main-locked"
            }),
        )
        .expect("git.sync in a Card/managed worktree must not wait on main checkout gate");
    assert_eq!(wt_sync["pushed"], true);

    // 5. Releasing Repo A's gate allows `git.pull` and `git.sync` on Repo A to proceed immediately.
    drop(guard_a);
    app.call(
        "git.pull",
        json!({"project_id": proj_a.id, "request_key": "pull-a-after-release"}),
    )
    .expect("git.pull on Repo A should succeed once delivery gate is released");
    app.call(
        "git.sync",
        json!({"project_id": proj_a.id, "request_key": "sync-a-after-release"}),
    )
    .expect("git.sync on Repo A should succeed once delivery gate is released");
    assert_eq!(
        std::fs::read_to_string(repo_a.join("upstream-a.txt")).unwrap(),
        "from upstream\n"
    );
}
