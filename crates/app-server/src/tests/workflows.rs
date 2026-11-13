use super::*;
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
fn finish(app: &AppServer, req: &Request, key: &str) {
    let a = admit(app, req);
    app.call("execution.report",json!({"execution":{"request_id":req.id,"attempt_number":a.number},"request_key":key,"state":"completed","message":"Plan ready","body":"Exact plan artifact"})).unwrap();
}
#[test]
fn published_revision_is_immediate_for_new_runs_and_frozen_for_active_runs() {
    let (_dir, app, p, c) = setup();
    let stale_collection = app
        .call("workflow.sources", json!({"project_id":p.id}))
        .unwrap();
    publish(&app, &p, "plan", "FIRST");
    let run = app
        .call(
            "workflow.start",
            json!({"workflow_id":"plan","card_id":c.id,"entry":"start","request_key":"start"}),
        )
        .unwrap();
    let again = app
        .call(
            "workflow.start",
            json!({"workflow_id":"plan","card_id":c.id,"entry":"start","request_key":"start"}),
        )
        .unwrap();
    assert_eq!(run["id"], again["id"]);
    publish(&app, &p, "plan", "SECOND");
    let req = app.snapshot().unwrap().requests[0].clone();
    assert!(req.instruction.contains("FIRST"));
    finish(&app, &req, "plan-result");
    let s = app.snapshot().unwrap();
    assert_eq!(s.cards[0].stage, "backlog");
    assert_eq!(s.workflow_runs[0].state, "waiting_approval");
    assert_eq!(s.workflow_runs[0].definition_revision, 1);
    assert_eq!(s.requests.len(), 1);
    assert!(app.call("workflow.decide",json!({"run_id":run["id"],"visit":2,"revision":s.cards[0].revision,"decision":"approve","execution":{}})).is_err());
    app.call("workflow.decide",json!({"run_id":run["id"],"visit":2,"revision":s.cards[0].revision,"decision":"approve","request_key":"approve"})).unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.cards[0].stage, "develop");
    assert_eq!(s.requests.len(), 2);
    assert_eq!(s.workflow_runs.len(), 1);
    assert!(s.requests[1].instruction.contains("report_id"));
    let other: Card = serde_json::from_value(
        app.call("card.create", json!({"project_id":p.id,"title":"Second"}))
            .unwrap(),
    )
    .unwrap();
    let r = app
        .call(
            "workflow.start",
            json!({"workflow_id":"plan","card_id":other.id,"entry":"start"}),
        )
        .unwrap();
    assert_eq!(r["definition_revision"], 2);
    assert!(
        app.snapshot()
            .unwrap()
            .requests
            .last()
            .unwrap()
            .instruction
            .contains("SECOND")
    );
    assert!(
        app.call(
            "workflow.apply",
            json!({"project_id":p.id,"revision":stale_collection["revision"],"sources":[{"filename":"plan.yaml","source":plan_yaml("plan","bad")}]})
        )
        .is_err()
    );
}
#[test]
fn subscription_progress_survives_restart_and_consumers_ack_independently() {
    let (dir, app, p, c) = setup();
    let sub=app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"card_id":c.id,"types":["comment.*"]},"target":{"type":"consumer"}})).unwrap();
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"one","mentions":[],"request_key":"comment"}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"one","mentions":[],"request_key":"comment"}),
    )
    .unwrap();
    let deliveries = app
        .call(
            "subscription.deliveries",
            json!({"subscription_id":sub["id"]}),
        )
        .unwrap();
    assert_eq!(deliveries.as_array().unwrap().len(), 1);
    let delivery = deliveries[0]["id"].clone();
    drop(app);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(
        app.call("subscription.deliveries", json!({})).unwrap()[0]["state"],
        "pending"
    );
    app.call("subscription.ack", json!({"delivery_id":delivery}))
        .unwrap();
    assert_eq!(
        app.call("subscription.deliveries", json!({})).unwrap()[0]["state"],
        "completed"
    );
    assert_eq!(app.snapshot().unwrap().projects[0].id, p.id);
}
#[test]
fn event_workflows_use_final_card_binding_and_failures_do_not_rollback_source_commands() {
    let (_dir, app, p, _) = setup();
    publish(&app, &p, "plan", "Plan");
    let sub=app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"project_id":p.id,"types":["card.created"]},"target":{"type":"workflow","workflow_id":"plan","entry":"start"}})).unwrap();
    let c = app
        .call(
            "card.create",
            json!({"project_id":p.id,"title":"Automatic"}),
        )
        .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.workflow_runs.len(), 1);
    assert_eq!(s.workflow_runs[0].card_id, c["id"].as_str().unwrap());
    assert_eq!(s.requests.len(), 1);
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
        json!({"card_id":c["id"],"body":"Still committed","mentions":[]}),
    )
    .unwrap();
    assert!(
        app.snapshot()
            .unwrap()
            .events
            .iter()
            .any(|e| e.message == "Still committed")
    );
    assert_eq!(app.snapshot().unwrap().requests.len(), 1);
}
#[test]
fn stale_plan_cannot_approve_and_rework_gets_new_step_identity() {
    let (_dir, app, p, c) = setup();
    publish(&app, &p, "plan", "Plan");
    app.call(
        "workflow.start",
        json!({"workflow_id":"plan","card_id":c.id,"entry":"start"}),
    )
    .unwrap();
    finish(&app, &app.snapshot().unwrap().requests[0], "done");
    let s = app.snapshot().unwrap();
    let run = s.workflow_runs[0].clone();
    let card = s.cards[0].clone();
    let updated = app
        .call(
            "card.update",
            json!({"card_id":card.id,"revision":card.revision,"body":"New requirement"}),
        )
        .unwrap();
    assert!(app.call("workflow.decide",json!({"run_id":run.id,"visit":run.visit,"revision":updated["revision"],"decision":"approve"})).is_err());
    app.call("workflow.decide",json!({"run_id":run.id,"visit":run.visit,"revision":updated["revision"],"decision":"rework"})).unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.requests.len(), 2);
    assert_eq!(s.workflow_runs[0].visit, 3);
    assert_ne!(s.requests[0].workflow_step, s.requests[1].workflow_step);
}
#[test]
fn invalid_graph_cannot_publish_and_active_run_survives_service_restart() {
    let (dir, app, p, c) = setup();
    let bad_yaml = plan_yaml("bad", "Plan").replace("next: finish", "next: plan");
    assert!(upsert_yaml(&app, &p, "bad.yaml", &bad_yaml).is_err());
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    drop(app);
    let mut opened = None;
    for _ in 0..100 {
        match AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")) {
            Ok(app) => {
                opened = Some(app);
                break;
            }
            Err(error) => {
                assert!(error.to_string().contains("owns this home"), "{error:#}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    let app = opened.expect("Isolated service lock did not release after shutdown");
    let s = app.snapshot().unwrap();
    assert_eq!(s.requests.len(), 1);
    assert_eq!(s.requests[0].id, request.id);
    assert!(s.requests[0].workflow_run_id.is_some());
    finish(&app, &s.requests[0], "adopted-done");
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "review");
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
    let persisted = serde_json::to_string(&app.snapshot().unwrap().events).unwrap();
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
    assert!(app.snapshot().unwrap().requests.is_empty());
    assert_eq!(
        app.snapshot().unwrap().notices[0].message,
        "Condition selected"
    );
    let sub=app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"card_id":c.id,"types":["card.column_entered","card.column_left"]},"target":{"type":"consumer"}})).unwrap();
    let card = app.snapshot().unwrap().cards[0].clone();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":card.revision,"stage":"develop"}),
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
        !serde_json::to_string(&app.snapshot().unwrap().events)
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
fn restoring_a_card_does_not_implicitly_reenter_its_workflow() {
    let (_dir, app, p, c) = setup();
    let restore_yaml = r#"schema: 1
id: restore-safe
name: Restore safe
on:
  start:
    event: manual
    start_at: done
steps:
  - id: done
    uses: complete
"#;
    upsert_yaml(&app, &p, "restore-safe.yaml", restore_yaml).unwrap();
    app.call("subscription.save",json!({"revision":0,"enabled":true,"selector":{"card_id":c.id,"types":["card.column_entered"],"column":"backlog"},"target":{"type":"workflow","workflow_id":"restore-safe","entry":"start"}})).unwrap();
    app.call(
        "card.archive",
        json!({"card_id":c.id,"revision":c.revision}),
    )
    .unwrap();
    let revision = app.snapshot().unwrap().cards[0].revision;
    app.call("card.restore", json!({"card_id":c.id,"revision":revision}))
        .unwrap();
    assert!(app.snapshot().unwrap().workflow_runs.is_empty());
}

#[test]
fn compatible_continue_records_one_human_approval_fact() {
    let (_dir, app, _p, mut c) = setup();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            c.source_trust = Some("external_pr".into());
            db.save(&c)
        })
        .unwrap();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    finish(&app, &app.snapshot().unwrap().requests[0], "develop-done");
    let card = app.snapshot().unwrap().cards[0].clone();
    app.call(
        "card.continue",
        json!({"card_id":c.id,"revision":card.revision}),
    )
    .unwrap();
    let facts = app
        .snapshot()
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "workflow.approved")
        .collect::<Vec<_>>();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].actor, "user");
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
        json!({"card_id":c.id,"body":"Start both","mentions":[]}),
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
    assert!(s.requests.is_empty());
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
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("isolated".into()),
                terminal: Some("local/closed".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
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
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("isolated".into()),
                terminal: Some("local/closed".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
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
    assert!(snapshot.requests.is_empty());
    assert_eq!(snapshot.cards.len(), 1);
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
        json!({"card_id":c.id,"revision":1,"stage":"review"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    assert!(!request.require_human_review);
    finish(&app, &request, "edited-default-complete");
    assert!(app.snapshot().unwrap().cards[0].human_gate.is_none());
}

#[test]
fn custom_columns_auto_merge_hold_external_pr_and_rework_feedback() {
    let (_dir, app, p, c) = setup();
    let before = collection(&app, &p);
    let custom_yaml = format!(
        r#"schema: 1
id: delivery:{}
name: Custom 4-Column Auto-merge
columns:
  - id: backlog
    name: Backlog
  - id: design
    name: Design
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
    start_at: condition:external_pr
    match:
      column: review
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the task
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: condition:external_pr
  - id: condition:external_pr
    uses: condition
    with:
      pointer: /card/source_trust
      equals: external_pr
      then: approval:review
      otherwise: deliver:review
  - id: deliver:review
    uses: deliver
    next: __end
    on_changes_requested: agent:develop
  - id: approval:review
    uses: approval
    with:
      title: External PR requires human review
    next: __end
    on_changes_requested: agent:develop
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#,
        p.id
    );
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": before["revision"],
            "sources": [{"filename": "development.yaml", "source": custom_yaml}]
        }),
    )
    .unwrap();

    // Verify 4 columns synchronized to Project stages
    let snap = app.snapshot().unwrap();
    let stage_ids: Vec<_> = snap.projects[0]
        .workflow
        .stages
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(stage_ids, vec!["backlog", "design", "develop", "review"]);

    // Hold auto-merge on Card #1 before moving to develop
    let held: Card = serde_json::from_value(
        app.call(
            "card.hold_auto_merge",
            json!({"card_id": c.id, "revision": c.revision, "hold": true}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(held.hold_auto_merge);

    // Move to develop -> agent:develop runs -> moves to review -> pauses at deliver:review because hold_auto_merge is true
    app.call(
        "card.move",
        json!({"card_id": c.id, "revision": held.revision, "stage": "develop"}),
    )
    .unwrap();
    let req1 = app.snapshot().unwrap().requests[0].clone();
    finish(&app, &req1, "dev-1");
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "review");
    assert_eq!(snap.workflow_runs[0].state, "waiting_approval");
    assert_eq!(
        snap.workflow_runs[0].step.as_deref(),
        Some("deliver:review")
    );

    // Request changes with rework feedback -> returns to agent:develop with feedback in instruction
    let run = snap.workflow_runs[0].clone();
    let card_rev = snap.cards[0].revision;
    app.call(
        "workflow.decide",
        json!({
            "run_id": run.id,
            "visit": run.visit,
            "revision": card_rev,
            "decision": "rework",
            "feedback": "Please add edge-case unit tests"
        }),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req2 = snap.requests.last().unwrap().clone();
    assert!(req2.instruction.contains("Please add edge-case unit tests"));

    // Unhold auto-merge while agent works, then finish -> auto-merges / completes!
    let card_rev = snap.cards[0].revision;
    app.call(
        "card.hold_auto_merge",
        json!({"card_id": c.id, "revision": card_rev, "hold": false}),
    )
    .unwrap();
    finish(&app, &req2, "dev-2");
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.workflow_runs[0].state, "completed");

    // Verify external PR card pauses at approval:review even when auto-merge is not held
    let ext_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "External PR card"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut ec: Card = db.get(&ext_card.id)?;
            ec.source_trust = Some("external_pr".into());
            db.save(&ec)
        })
        .unwrap();
    let ext_card = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|x| x.id == ext_card.id)
        .unwrap();
    app.call(
        "card.move",
        json!({"card_id": ext_card.id, "revision": ext_card.revision, "stage": "review"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let ext_run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == ext_card.id)
        .unwrap();
    assert_eq!(ext_run.state, "waiting_approval");
    assert_eq!(ext_run.step.as_deref(), Some("approval:review"));
}

fn finish_closed(app: &AppServer, req: &Request, key: &str) {
    let a = admit(app, req);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut run: Attempt = db.get(&a.id)?;
            // Simulate a real retained interactive Herdr terminal whose Agent is still mid-turn
            // when invoking `canban report`, then finishes its turn (`done`) right after.
            run.terminal = Some("t-retained".into());
            run.connection_state = "connected".into();
            run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            db.save(&run)
        })
        .unwrap();
    app.call("execution.report",json!({"execution":{"request_id":req.id,"attempt_number":a.number},"request_key":key,"state":"completed","message":"Plan ready","body":"Exact plan artifact"})).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut run: Attempt = db.get(&a.id)?;
            run.observed = Some(json!({"state": "live", "status": {"state": "done"}}));
            db.save(&run)
        })
        .unwrap();
}

#[test]
fn default_workflow_queue_merge_syncs_upstream_and_respects_local_only_option() {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&source).unwrap();

    let git = |cwd: &std::path::Path, args: &[&str]| -> String {
        let out = std::process::Command::new("git")
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
    };

    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["init", "-b", "main"]);
    git(&source, &["config", "user.name", "Tester"]);
    git(&source, &["config", "user.email", "tester@example.invalid"]);
    git(&source, &["config", "commit.gpgsign", "false"]);
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(source.join("README.md"), "# Initial\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "init"]);
    git(&source, &["push", "-u", "origin", "main"]);
    let initial_sha = git(&source, &["rev-parse", "refs/heads/main"]);
    assert_eq!(git(&remote, &["rev-parse", "refs/heads/main"]), initial_sha);

    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Default Queue Merge", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();

    // 1. Default workflow: joining Backlog does NOT analyze and does NOT move to Develop.
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Feature 1", "body": "Sync upstream by default"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "backlog");
    assert!(snap.requests.is_empty());
    assert!(snap.workflow_runs.is_empty());

    // 2. Entering Develop runs @developer automatically, moves to Review (no @reviewer),
    // and Queue Merge merges locally + pushes to upstream remote!
    app.call(
        "card.move",
        json!({"card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.requests.len(), 1);
    assert_eq!(snap.requests[0].profile.id, "developer");

    let ops = canban_workspaces::Ops::local();
    let wt1 = ops.prepare(&app.home, &p, &snap.cards[0]).unwrap();
    let branch1 = ops
        .current_branch(wt1.to_str().unwrap())
        .expect("worktree branch");
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
    git(&wt1, &["config", "user.name", "Tester"]);
    git(&wt1, &["config", "user.email", "tester@example.invalid"]);
    git(&wt1, &["config", "commit.gpgsign", "false"]);
    std::fs::write(wt1.join("feature1.txt"), "hello upstream\n").unwrap();

    finish_closed(&app, &snap.requests[0], "dev-c1-done");
    app.pump_deliveries().unwrap();
    // Ensure only 1 agent request ran (no @reviewer step in default Review).
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.requests.len(), 1);
    let run1 = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c1.id)
        .unwrap();
    assert_eq!(run1.state, "completed");

    let merged_sha_1 = git(&source, &["rev-parse", "refs/heads/main"]);
    assert_ne!(merged_sha_1, initial_sha);
    assert_eq!(
        git(&remote, &["rev-parse", "refs/heads/main"]),
        merged_sha_1,
        "Default Queue Merge must sync merged target branch to upstream remote"
    );
    let snap = app.snapshot().unwrap();
    let delivered_c1 = snap.cards.iter().find(|x| x.id == c1.id).unwrap();
    assert_eq!(delivered_c1.delivery.as_ref().unwrap().commit, merged_sha_1);
    assert!(
        snap.events.iter().any(|e| {
            e.card_id == c1.id && e.kind == "review.merged" && e.data["synced_upstream"] == true
        }),
        "review.merged event should record synced_upstream: true"
    );

    // 3. Toggle "Merge Only (Local)" via derived source options (sync_upstream: false).
    let col = collection(&app, &p);
    let raw = col["sources"][0]["source"].as_str().unwrap();
    let mut doc = canban_workflow_engine::source::parse(raw).unwrap();
    let derived = canban_workflow_engine::source::derive_options(&doc);
    assert!(!derived.backlog_analyze);
    assert!(!derived.backlog_auto_develop);
    assert!(!derived.review_manual);
    assert!(derived.sync_upstream);

    canban_workflow_engine::source::set_sync_upstream(&mut doc, false);
    assert!(!canban_workflow_engine::source::derive_options(&doc).sync_upstream);
    let updated_yaml = canban_workflow_engine::source::encode(&doc).unwrap();
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": col["revision"],
            "sources": [{"filename": "development.yaml", "source": updated_yaml}]
        }),
    )
    .unwrap();

    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Feature 2", "body": "Local merge without upstream push"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c2.id, "revision": c2.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req2 = snap
        .requests
        .iter()
        .find(|r| r.card_id == c2.id)
        .unwrap()
        .clone();

    let card2 = snap.cards.iter().find(|x| x.id == c2.id).unwrap().clone();
    let wt2 = ops.prepare(&app.home, &p, &card2).unwrap();
    let branch2 = ops
        .current_branch(wt2.to_str().unwrap())
        .expect("worktree branch 2");
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
    git(&wt2, &["config", "user.name", "Tester"]);
    git(&wt2, &["config", "user.email", "tester@example.invalid"]);
    git(&wt2, &["config", "commit.gpgsign", "false"]);
    std::fs::write(wt2.join("feature2.txt"), "local merge only\n").unwrap();

    finish_closed(&app, &req2, "dev-c2-done");
    app.pump_deliveries().unwrap();

    let merged_sha_2 = git(&source, &["rev-parse", "refs/heads/main"]);
    assert_ne!(
        merged_sha_2, merged_sha_1,
        "Local target branch should advance"
    );
    assert_eq!(
        git(&remote, &["rev-parse", "refs/heads/main"]),
        merged_sha_1,
        "Upstream remote must NOT advance when sync_upstream is false"
    );
    let snap = app.snapshot().unwrap();
    assert!(
        snap.events.iter().any(|e| {
            e.card_id == c2.id && e.kind == "review.merged" && e.data["synced_upstream"] == false
        }),
        "review.merged event should record synced_upstream: false"
    );
}

#[test]
fn backlog_and_review_options_drive_card_lifecycle_and_preserve_custom_stages() {
    let (_dir, app, p, _) = setup();

    // Helper to patch development.yaml via Studio's derived option helpers.
    let update_doc = |mutate: &dyn Fn(&mut canban_workflow_engine::source::Document)| {
        let col = collection(&app, &p);
        let raw = col["sources"][0]["source"].as_str().unwrap();
        let mut doc = canban_workflow_engine::source::parse(raw).unwrap();
        mutate(&mut doc);
        let next_yaml = canban_workflow_engine::source::encode(&doc).unwrap();
        app.call(
            "workflow.apply",
            json!({
                "project_id": p.id,
                "revision": col["revision"],
                "sources": [{"filename": "development.yaml", "source": next_yaml}]
            }),
        )
        .unwrap();
    };

    // Case A: Enable Backlog Auto-Analyze + Backlog Auto-Start Develop + Manual Review,
    // AND add a custom QA stage between Develop and Review to verify non-destructive derivation.
    update_doc(&|doc| {
        doc.columns = vec![
            WorkflowColumn {
                id: "backlog".into(),
                name: "Backlog".into(),
            },
            WorkflowColumn {
                id: "develop".into(),
                name: "Develop".into(),
            },
            WorkflowColumn {
                id: "qa".into(),
                name: "QA".into(),
            },
            WorkflowColumn {
                id: "review".into(),
                name: "Review".into(),
            },
        ];
        canban_workflow_engine::source::set_backlog_options(doc, true, true);
        canban_workflow_engine::source::set_review_manual(doc, true);
    });

    // Creating a new Card in Backlog immediately runs agent:plan, waits for it to finish,
    // then automatically moves to Develop and runs agent:develop, then pauses at Manual Review!
    let c_plan_dev: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Analyze then Develop", "body": "Wait for analysis"}),
        )
        .unwrap(),
    )
    .unwrap();

    let snap = app.snapshot().unwrap();
    let card_state = snap.cards.iter().find(|x| x.id == c_plan_dev.id).unwrap();
    assert_eq!(
        card_state.stage, "backlog",
        "Card stays in Backlog until auto-analysis completes"
    );
    let plan_req = snap
        .requests
        .iter()
        .find(|r| r.card_id == c_plan_dev.id)
        .expect("agent:plan request should queue on card.create")
        .clone();
    assert!(
        plan_req
            .workflow_step
            .as_deref()
            .is_some_and(|s| s.starts_with("agent:plan:"))
    );

    finish(&app, &plan_req, "analysis-done");
    let snap = app.snapshot().unwrap();
    let card_state = snap.cards.iter().find(|x| x.id == c_plan_dev.id).unwrap();
    assert_eq!(
        card_state.stage, "develop",
        "After analysis finishes, Card automatically moves to Develop"
    );
    let dev_req = snap
        .requests
        .iter()
        .rfind(|r| r.card_id == c_plan_dev.id)
        .unwrap()
        .clone();
    assert!(
        dev_req
            .workflow_step
            .as_deref()
            .is_some_and(|s| s.starts_with("agent:develop:"))
    );

    finish(&app, &dev_req, "develop-done");
    let snap = app.snapshot().unwrap();
    let card_state = snap.cards.iter().find(|x| x.id == c_plan_dev.id).unwrap();
    assert_eq!(card_state.stage, "review");
    assert!(
        card_state.human_gate.is_some(),
        "Manual Review option must gate Review on human approval"
    );
    let run_state = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c_plan_dev.id)
        .unwrap();
    assert_eq!(run_state.state, "waiting_approval");
    assert_eq!(run_state.step.as_deref(), Some("approval:review"));

    // Verify custom 4-column layout survived option mutations.
    let stage_ids: Vec<_> = snap.projects[0]
        .workflow
        .stages
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(stage_ids, vec!["backlog", "develop", "qa", "review"]);

    // Case B: Disable Backlog Auto-Analyze while keeping Backlog Auto-Start Develop enabled.
    // Newly created Card immediately moves to Develop and starts @developer without planning.
    update_doc(&|doc| {
        canban_workflow_engine::source::set_backlog_options(doc, false, true);
    });

    let c_direct_dev: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Immediate Develop", "body": "Skip analysis"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let card2_state = snap.cards.iter().find(|x| x.id == c_direct_dev.id).unwrap();
    assert_eq!(
        card2_state.stage, "develop",
        "Card immediately moves from Backlog to Develop when analyze=false, auto_develop=true"
    );
    let reqs_for_c2: Vec<_> = snap
        .requests
        .iter()
        .filter(|r| r.card_id == c_direct_dev.id)
        .collect();
    assert_eq!(reqs_for_c2.len(), 1);
    assert!(
        reqs_for_c2[0]
            .workflow_step
            .as_deref()
            .is_some_and(|s| s.starts_with("agent:develop:"))
    );
}

fn git_in(cwd: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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
fn git_identity(cwd: &std::path::Path) {
    git_in(cwd, &["config", "user.name", "Tester"]);
    git_in(cwd, &["config", "user.email", "tester@example.invalid"]);
    git_in(cwd, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn queue_merge_conflict_routes_to_agent_then_fast_forwards_after_resolution() {
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
            json!({"name": "Conflict Queue", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Edit README", "body": "Conflicting edit"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev = snap.requests[0].clone();
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
    std::fs::write(wt.join("README.md"), "card version\n").unwrap();
    // The target branch moves with a conflicting edit while the Card is in flight.
    std::fs::write(source.join("README.md"), "target version\n").unwrap();
    git_in(&source, &["commit", "-am", "target edit"]);
    git_in(&source, &["push", "origin", "main"]);
    let target_before = git_in(&source, &["rev-parse", "HEAD"]);

    finish_closed(&app, &dev, "dev-done");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c.id)
        .unwrap();
    assert_eq!(
        run.state, "waiting_agent",
        "conflict must route to the Agent"
    );
    assert_eq!(run.inputs["conflict_attempts"], 1);
    let card = snap.cards.iter().find(|x| x.id == c.id).unwrap();
    assert!(card.human_gate.is_none() && card.attention.is_none());
    assert!(card.delivery.is_none());
    assert_eq!(git_in(&source, &["rev-parse", "HEAD"]), target_before);
    assert!(snap.events.iter().any(|e| e.card_id == c.id
        && e.kind == "review.merge_conflict"
        && e.data["files"] == json!(["README.md"])));
    let resolve = snap
        .requests
        .iter()
        .find(|r| r.card_id == c.id && r.id != dev.id)
        .expect("conflict rework request")
        .clone();
    assert_eq!(
        resolve.profile.id, "merger",
        "Queue Merge conflicts must dispatch the dedicated @merger role"
    );
    assert!(
        resolve
            .instruction
            .contains("Merge conflict to resolve before delivery")
    );
    assert!(resolve.instruction.contains("README.md"));
    assert!(
        resolve
            .workflow_step
            .as_deref()
            .unwrap()
            .starts_with("deliver:review:")
    );
    assert_eq!(
        run.step.as_deref(),
        Some("deliver:review"),
        "WorkflowRun stays on the Deliver step while @merger resolves conflicts"
    );

    // The Agent resolves the in-progress merge in the Card worktree.
    std::fs::write(wt.join("README.md"), "card + target version\n").unwrap();
    git_in(&wt, &["add", "README.md"]);
    git_in(&wt, &["commit", "--no-edit"]);
    let resolved = git_in(&wt, &["rev-parse", "HEAD"]);
    finish_closed(&app, &resolve, "resolve-done");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c.id)
        .unwrap();
    assert_eq!(run.state, "completed");
    assert_eq!(git_in(&source, &["rev-parse", "HEAD"]), resolved);
    assert_eq!(
        std::fs::read_to_string(source.join("README.md")).unwrap(),
        "card + target version\n"
    );
    let dev_conv = snap
        .conversations
        .iter()
        .find(|conv| conv.card_id == c.id && conv.profile_id == "developer");
    let merger_conv = snap
        .conversations
        .iter()
        .find(|conv| conv.card_id == c.id && conv.profile_id == "merger");
    assert!(
        dev_conv.is_some()
            && merger_conv.is_some()
            && dev_conv.unwrap().id != merger_conv.unwrap().id,
        "@developer and @merger must keep separate role conversations on the Card"
    );
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
            json!({"project_id": p.id, "title": "Conflict with custom profile", "body": "Test"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev = snap.requests[0].clone();
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
        .requests
        .iter()
        .find(|r| r.card_id == c.id && r.id != dev.id)
        .expect("custom conflict profile request");
    assert_eq!(resolve.profile.id, "reviewer");
}

#[test]
fn queue_merge_conflict_pauses_for_human_after_attempt_budget() {
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
            json!({"name": "Budget", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    // Disable automatic conflict rework.
    let col = collection(&app, &p);
    let raw = col["sources"][0]["source"].as_str().unwrap();
    let mut doc = canban_workflow_engine::source::parse(raw).unwrap();
    doc.steps
        .iter_mut()
        .find(|s| s.uses == "deliver")
        .unwrap()
        .with
        .insert("max_conflict_attempts".into(), json!(0));
    app.call(
        "workflow.apply",
        json!({"project_id": p.id, "revision": col["revision"], "sources": [{"filename": "development.yaml", "source": canban_workflow_engine::source::encode(&doc).unwrap()}]}),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Edit", "body": "x"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev = snap.requests[0].clone();
    let updated_p = snap.projects.iter().find(|x| x.id == p.id).unwrap();
    let ops = canban_workspaces::Ops::local();
    let wt = ops.prepare(&app.home, updated_p, &snap.cards[0]).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c.id)?;
            card.worktree = Some(wt.to_str().unwrap().into());
            db.save(&card)
        })
        .unwrap();
    git_identity(&wt);
    std::fs::write(wt.join("README.md"), "card\n").unwrap();
    std::fs::write(source.join("README.md"), "target\n").unwrap();
    git_in(&source, &["commit", "-am", "target edit"]);
    git_in(&source, &["push", "origin", "main"]);
    finish_closed(&app, &dev, "dev-done");
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c.id)
        .unwrap();
    assert_eq!(run.state, "waiting_approval");
    assert_eq!(
        snap.requests.iter().filter(|r| r.card_id == c.id).count(),
        1
    );
    assert!(
        snap.cards
            .iter()
            .find(|x| x.id == c.id)
            .unwrap()
            .human_gate
            .is_some()
    );
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
                json!({"project_id": p.id, "title": title, "body": "task"}),
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
        json!({"card_id": c3.id, "revision": c3_now.revision, "stage": "backlog", "before_card_id": c2.id}),
    )
    .unwrap();

    let run_of = |snap: &Snapshot, id: &str| {
        snap.workflow_runs
            .iter()
            .find(|r| r.card_id == id && r.active())
            .cloned()
            .unwrap()
    };
    let snap = app.snapshot().unwrap();
    // K = 1: only the first Card is planning; the rest wait before planning.
    let plan1 = snap
        .requests
        .iter()
        .find(|r| r.card_id == c1.id)
        .expect("first Card plans")
        .clone();
    assert!(
        plan1
            .workflow_step
            .as_deref()
            .unwrap()
            .starts_with("agent:plan:")
    );
    for c in [&c2, &c3] {
        let r = run_of(&snap, &c.id);
        assert_eq!(r.state, "waiting_capacity");
        assert_eq!(r.step.as_deref(), Some("admit:plan"));
        assert!(!snap.requests.iter().any(|q| q.card_id == c.id));
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
    assert!(snap.requests.iter().any(|r| {
        r.card_id == c1.id
            && r.workflow_step
                .as_deref()
                .is_some_and(|s| s.starts_with("agent:develop:"))
    }));
    assert!(
        snap.requests.iter().any(|r| r.card_id == c3.id
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
            json!({"card_id": c.id, "revision": c.revision, "stage": "review"}),
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
    assert!(snap.events.iter().any(|e| e.card_id == c.id
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
        json!({"card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().requests[0].clone();
    edit_development(&app, &p, |d| {
        canban_workflow_engine::stages::remove_step(d, "agent:develop").unwrap();
    });
    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c.id)
        .unwrap();
    assert_eq!(run.state, "waiting_agent", "the current step keeps running");
    assert_eq!(run.inputs["stop_after_step"], "agent:develop");
    finish(&app, &req, "dev-done");
    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c.id)
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
            json!({"project_id": p.id, "title": "Feature A", "body": "auto commit test"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev1 = snap
        .requests
        .iter()
        .find(|r| r.card_id == c1.id)
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
        .find(|r| r.card_id == c1.id && r.active())
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
            json!({"project_id": p.id, "title": "Feature B", "body": "stopped conflict retry"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c2.id, "revision": c2.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let dev2 = snap
        .requests
        .iter()
        .find(|r| r.card_id == c2.id)
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
        .find(|r| r.card_id == c2.id && r.active())
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
            .any(|r| r.card_id == c2.id && r.active())
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
        .requests
        .iter()
        .rfind(|r| r.card_id == c2.id)
        .unwrap()
        .clone();
    assert!(
        new_req
            .instruction
            .contains("Merge conflict to resolve before delivery")
    );
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
                json!({"project_id": p.id, "title": "In flight", "body": "keep rev 1"}),
            )
            .unwrap(),
        )
        .unwrap();
        card_id = c.id.clone();
        app.call(
            "card.move",
            json!({"card_id": c.id, "revision": c.revision, "stage": "develop"}),
        )
        .unwrap();
        let snap = app.snapshot().unwrap();
        first_run_rev = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id == c.id)
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
        .find(|r| r.card_id == card_id && r.active())
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
            json!({"card_id": c.id, "revision": c.revision, "stage": "develop"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req = snap
        .requests
        .iter()
        .find(|r| r.card_id == moved.id && r.state == "queued")
        .unwrap();
    let rel_prompt = canban_context::capture(
        &app.home,
        &p,
        &moved,
        req,
        1,
        &snap,
        "/tmp/canban-test.sock",
        Path::new("/bin/echo"),
    )
    .unwrap();
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
    assert!(fresh_prompt.contains("--state <completed|backlog|blocked|failed>"));
    assert!(fresh_prompt.contains("--state backlog"));

    // Finish initial request, add a human comment + attachment, and verify conditional sections appear.
    let attempt = admit(&app, req);
    app.call(
        "execution.report",
        json!({
            "execution": {"request_id": attempt.request_id, "attempt_number": 1},
            "state": "blocked",
            "message": "Need API spec",
            "body": "# Plan notes\nCheck auth handler.",
            "request_key": "blocked-with-file"
        }),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"card_id": c.id, "body": "Use OAuth2 PKCE flow."}),
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
    let rel_prompt2 = canban_context::capture(
        &app.home,
        &p,
        card2,
        req,
        2,
        &snap2,
        "/tmp/canban-test.sock",
        Path::new("/bin/echo"),
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
            json!({"project_id": p.id, "title": "Legacy cleanup", "body": "Remove v0 shim"}),
        )
        .unwrap(),
    )
    .unwrap();
    // Creating in Backlog triggered `backlog_analyze`; move directly to `develop` to start `agent:develop`.
    let card: Card = serde_json::from_value(
        app.call(
            "card.move",
            json!({"card_id": card.id, "revision": card.revision, "stage": "develop"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req = snap
        .requests
        .iter()
        .find(|r| r.card_id == card.id && r.stage == "develop" && r.state == "queued")
        .unwrap()
        .clone();
    let attempt = admit(&app, &req);

    // Agent inspects codebase and decides the Card is already implemented -> reports `--state backlog`.
    let res = app
        .call(
            "execution.report",
            json!({
                "execution": {"request_id": attempt.request_id, "attempt_number": 1},
                "state": "backlog",
                "message": "Already removed in PR #42",
                "body": "Verified `src/v0_shim.rs` no longer exists on `main`.",
                "request_key": format!("backlog:{}:1", attempt.request_id)
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
            .any(|r| r.card_id == card.id && r.active())
    );
    assert!(
        !after
            .requests
            .iter()
            .any(|r| r.card_id == card.id && r.state == "queued")
    );

    // Timeline records both the Agent's narrative explanation and the stage return reason.
    let card_events: Vec<_> = after
        .events
        .iter()
        .filter(|e| e.card_id == card.id)
        .collect();
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
    }));

    // Unread notification reminder alerts the user to read the Agent's explanation on the Card.
    let reminders = canban_notifications::reminders(&after);
    assert!(reminders.iter().any(|r| r.card_id == card.id && r.unread));
}

#[test]
fn snapshot_exposes_merging_cards_while_merge_is_in_flight_and_clears_afterward() {
    let (_dir, app, _p, c) = setup();
    let rx = app.subscribe();
    assert!(app.snapshot().unwrap().merging_cards.is_empty());

    let guard = app.track_merging(&c.id);
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.merging_cards, vec![c.id.clone()]);
    assert!(matches!(
        rx.try_recv(),
        Ok(AppEvent::Changed { topic: "review", card_id: Some(ref id) }) if id == &c.id
    ));

    drop(guard);
    assert!(app.snapshot().unwrap().merging_cards.is_empty());

    // Merged or archived Cards are filtered out even if a guard is still dropping.
    let guard = app.track_merging(&c.id);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c.id)?;
            card.delivery = Some(Delivery {
                target_branch: "main".into(),
                commit: "1234567".into(),
                request_key: "k".into(),
                merged_at: now(),
            });
            db.save(&card)
        })
        .unwrap();
    assert!(app.snapshot().unwrap().merging_cards.is_empty());
    drop(guard);
}

#[test]
fn queue_merge_waits_for_connected_agent_turn_wrapup_and_falls_back_after_settle_grace() {
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
            json!({"name": "Settle Grace", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();

    // Case 1: Agent reports `completed` via tool call while Herdr still reports `status.state == "working"`.
    // Queue merge stays in `waiting_delivery` until the Agent finishes its turn (`done` / `idle`),
    // including any post-report worktree changes before merging!
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Wrap-up Wait", "body": "Wait for turn end"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req1 = snap
        .requests
        .iter()
        .find(|r| r.card_id == c1.id)
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
    std::fs::write(wt1.join("part1.txt"), "committed before report\n").unwrap();
    git_in(&wt1, &["add", "."]);
    git_in(&wt1, &["commit", "-m", "feat: part 1"]);

    let a1 = admit(&app, &req1);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut run: Attempt = db.get(&a1.id)?;
            run.terminal = Some("t-wrapup-1".into());
            run.connection_state = "connected".into();
            run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            db.save(&run)
        })
        .unwrap();
    app.call(
        "execution.report",
        json!({
            "execution": {"request_id": req1.id, "attempt_number": a1.number},
            "request_key": "c1-report",
            "state": "completed",
            "message": "Implemented feature"
        }),
    )
    .unwrap();

    // Immediately after `execution.report` (and even on `pump_deliveries`), the Card moved to
    // `review` and entered `waiting_delivery`, but has NOT merged yet while Agent is `working`!
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let card1 = snap.cards.iter().find(|c| c.id == c1.id).unwrap();
    assert_eq!(card1.stage, "review");
    assert!(
        card1.delivery.is_none(),
        "Queue merge must not merge while the connected Agent is still wrapping up its turn"
    );
    let run1 = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c1.id)
        .unwrap();
    assert_eq!(run1.state, "waiting_delivery");

    // Agent writes a cleanup/followup edit before finishing its turn, then transitions to `done`.
    std::fs::write(wt1.join("wrapup.txt"), "added during wrapup\n").unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut run: Attempt = db.get(&a1.id)?;
            run.observed = Some(json!({"state": "live", "status": {"state": "done"}}));
            db.save(&run)
        })
        .unwrap();
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let card1 = snap.cards.iter().find(|c| c.id == c1.id).unwrap();
    assert!(
        card1.delivery.is_some(),
        "Queue merge completes once Agent turn transitions to done/idle"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("wrapup.txt")).unwrap(),
        "added during wrapup\n"
    );

    // Case 2: If a provider's status stays stuck at `"working"` past `DELIVERY_AGENT_SETTLE_MS`,
    // Queue merge falls back and delivers instead of wedging forever.
    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": p.id, "title": "Settle Timeout Fallback", "body": "Stuck status"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c2.id, "revision": c2.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req2 = snap
        .requests
        .iter()
        .find(|r| r.card_id == c2.id)
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
    std::fs::write(wt2.join("part2.txt"), "timeout fallback\n").unwrap();

    let a2 = admit(&app, &req2);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut run: Attempt = db.get(&a2.id)?;
            run.terminal = Some("t-wrapup-2".into());
            run.connection_state = "connected".into();
            run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            db.save(&run)
        })
        .unwrap();
    app.call(
        "execution.report",
        json!({
            "execution": {"request_id": req2.id, "attempt_number": a2.number},
            "request_key": "c2-report",
            "state": "completed",
            "message": "Done"
        }),
    )
    .unwrap();
    assert!(
        app.snapshot()
            .unwrap()
            .cards
            .iter()
            .find(|c| c.id == c2.id)
            .unwrap()
            .delivery
            .is_none()
    );

    // Age `completed_at` and `run.updated_at` past the settle grace window.
    let expired = now().saturating_sub(AppServer::DELIVERY_AGENT_SETTLE_MS + 1_000);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut att: Attempt = db.get(&a2.id)?;
            att.completed_at = Some(expired);
            db.save(&att)?;
            let mut wf: WorkflowRun = db
                .list::<WorkflowRun>()?
                .into_iter()
                .find(|r| r.card_id == c2.id)
                .unwrap();
            wf.updated_at = expired;
            db.save(&wf)
        })
        .unwrap();
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let card2 = snap.cards.iter().find(|c| c.id == c2.id).unwrap();
    assert!(
        card2.delivery.is_some(),
        "Queue merge must proceed once settle grace expires"
    );
}

#[test]
fn merge_queue_drains_fifo_holds_head_during_conflict_and_never_spawns_merger_for_clean_merges() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    std::fs::write(source.join("shared.txt"), "base\n").unwrap();
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
            json!({"name": "FIFO Merge Queue", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    edit_development(&app, &p, |d| {
        canban_workflow_engine::source::set_review_manual(d, false);
    });
    let ops = canban_workspaces::Ops::local();

    // Create 3 Cards BEFORE moving any of them to Develop.
    // Deliberately move them to Develop / finish them in reverse creation order (c3 -> c1 -> c2)
    // to prove the Merge Queue follows actual queue entry (`queued_at`), NOT Card/run creation rowid!
    let mut created = Vec::new();
    for title in [
        "Card 1 (conflicts with 3)",
        "Card 2 (clean)",
        "Card 3 (first in queue, clean)",
    ] {
        let c: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"project_id": p.id, "title": title, "body": title}),
            )
            .unwrap(),
        )
        .unwrap();
        created.push(c);
    }
    let (c1, c2, c3) = (created[0].clone(), created[1].clone(), created[2].clone());

    let prepare_in_develop = |card: &Card| -> (Request, PathBuf) {
        app.call(
            "card.move",
            json!({"card_id": card.id, "revision": card.revision, "stage": "develop"}),
        )
        .unwrap();
        let snap = app.snapshot().unwrap();
        let req = snap
            .requests
            .iter()
            .find(|r| r.card_id == card.id)
            .unwrap()
            .clone();
        let wt = ops
            .prepare(
                &app.home,
                &p,
                snap.cards.iter().find(|c| c.id == card.id).unwrap(),
            )
            .unwrap();
        let branch = ops.current_branch(wt.to_str().unwrap()).unwrap();
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut stored: Card = db.get(&card.id)?;
                stored.worktree = Some(wt.to_str().unwrap().into());
                stored.branch = Some(branch);
                db.save(&stored)
            })
            .unwrap();
        git_identity(&wt);
        (req, wt)
    };

    let (dev1, wt1) = prepare_in_develop(&c1);
    let (dev2, wt2) = prepare_in_develop(&c2);
    let (dev3, wt3) = prepare_in_develop(&c3);

    // c3 modifies shared.txt cleanly vs base; c1 ALSO modifies shared.txt differently (so c1 will
    // conflict once c3 lands first!); c2 touches an independent file (`clean2.txt`).
    std::fs::write(wt3.join("shared.txt"), "from c3\n").unwrap();
    git_in(&wt3, &["commit", "-am", "feat: c3"]);

    std::fs::write(wt1.join("shared.txt"), "from c1\n").unwrap();
    git_in(&wt1, &["commit", "-am", "feat: c1"]);

    std::fs::write(wt2.join("clean2.txt"), "from c2\n").unwrap();
    git_in(&wt2, &[" add", "."].map(str::trim));
    git_in(&wt2, &["commit", "-m", "feat: c2"]);

    // Temporarily dirty the main checkout so all 3 Cards queue up while the user has local edits!
    std::fs::write(source.join("wip.local"), "temporary local edit\n").unwrap();

    // Finish in order: c3 (1st), then c1 (2nd), then c2 (3rd), spacing by >=2ms for distinct `queued_at`.
    finish_closed(&app, &dev3, "c3-done");
    std::thread::sleep(std::time::Duration::from_millis(5));
    finish_closed(&app, &dev1, "c1-done");
    std::thread::sleep(std::time::Duration::from_millis(5));
    finish_closed(&app, &dev2, "c2-done");

    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let queue_ids: Vec<String> = snap
        .merge_queue(&p.id)
        .into_iter()
        .map(|r| r.card_id.clone())
        .collect();
    assert_eq!(
        queue_ids,
        vec![c3.id.clone(), c1.id.clone(), c2.id.clone()],
        "Merge queue must preserve strict FIFO arrival order (c3 -> c1 -> c2)"
    );
    // None of the 3 Cards were kicked out to waiting_approval, and NO @merger session was spawned!
    for cid in [&c3.id, &c1.id, &c2.id] {
        let card = snap.cards.iter().find(|c| &c.id == cid).unwrap();
        assert!(card.delivery.is_none());
        assert!(card.human_gate.is_none());
    }
    assert_eq!(
        snap.merge_queue_status(&c3.id)
            .map(|(pos, total, _)| (pos, total)),
        Some((1, 3))
    );
    assert_eq!(
        snap.merge_queue_status(&c2.id)
            .map(|(pos, total, _)| (pos, total)),
        Some((3, 3))
    );
    assert!(
        !snap.requests.iter().any(|r| r.profile.id == "merger"),
        "No @merger request may be created while waiting or for clean merges"
    );

    // User cleans up the main repository checkout: a single `pump_deliveries()` automatically:
    // 1. Merges c3 natively (0 @merger sessions!)
    // 2. Advances to c1, detects conflict with new `main`, and dispatches @merger ONLY for c1
    // 3. Keeps c2 waiting behind c1 at queue position #2 (c2 MUST NOT jump ahead of c1!)
    std::fs::remove_file(source.join("wip.local")).unwrap();
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let card3 = snap.cards.iter().find(|c| c.id == c3.id).unwrap();
    let card1 = snap.cards.iter().find(|c| c.id == c1.id).unwrap();
    let card2 = snap.cards.iter().find(|c| c.id == c2.id).unwrap();
    assert!(
        card3.delivery.is_some(),
        "c3 (head of queue) must auto-merge immediately once main checkout is clean"
    );
    assert!(
        !snap
            .requests
            .iter()
            .any(|r| r.card_id == c3.id && r.profile.id == "merger"),
        "Clean merge of c3 must NOT spawn @merger"
    );
    assert!(
        !snap
            .conversations
            .iter()
            .any(|conv| conv.card_id == c3.id && conv.profile_id == "merger"),
        "Clean merge of c3 must NOT create a @merger conversation"
    );
    assert!(card1.delivery.is_none());
    assert!(
        card2.delivery.is_none(),
        "c2 must stay queued behind c1 while @merger resolves c1's conflict (strict HoL serialization)"
    );
    let queue_after_c3: Vec<String> = snap
        .merge_queue(&p.id)
        .into_iter()
        .map(|r| r.card_id.clone())
        .collect();
    assert_eq!(
        queue_after_c3,
        vec![c1.id.clone(), c2.id.clone()],
        "c1 holds head #1 during @merger conflict resolution, with c2 queued at #2"
    );
    let merger_c1: Vec<&Request> = snap
        .requests
        .iter()
        .filter(|r| r.card_id == c1.id && r.profile.id == "merger")
        .collect();
    assert_eq!(
        merger_c1.len(),
        1,
        "Exactly one @merger request is dispatched for conflicting c1"
    );

    // Even if another pump_deliveries() fires while @merger is still working on c1, c2 STILL waits!
    app.pump_deliveries().unwrap();
    assert!(
        app.snapshot()
            .unwrap()
            .cards
            .iter()
            .find(|c| c.id == c2.id)
            .unwrap()
            .delivery
            .is_none()
    );

    // @merger resolves the conflict in c1's worktree and reports completed -> pump_deliveries
    // merges c1 AND immediately drains c2 right behind it without spawning @merger for c2!
    let merger_req = merger_c1[0].clone();
    std::fs::write(wt1.join("shared.txt"), "from c3 + from c1\n").unwrap();
    git_in(&wt1, &["add", "shared.txt"]);
    git_in(&wt1, &["commit", "--no-edit"]);
    finish_closed(&app, &merger_req, "c1-conflict-resolved");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    assert!(
        snap.merge_queue(&p.id).is_empty(),
        "Entire FIFO merge queue should be drained"
    );
    assert!(
        snap.cards
            .iter()
            .find(|c| c.id == c1.id)
            .unwrap()
            .delivery
            .is_some(),
        "c1 merged after @merger resolved conflict"
    );
    assert!(
        snap.cards
            .iter()
            .find(|c| c.id == c2.id)
            .unwrap()
            .delivery
            .is_some(),
        "c2 automatically merged right after c1 completed"
    );
    assert!(
        !snap
            .requests
            .iter()
            .any(|r| r.card_id == c2.id && r.profile.id == "merger"),
        "Clean merge of c2 must NOT spawn @merger"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("shared.txt")).unwrap(),
        "from c3 + from c1\n"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("clean2.txt")).unwrap(),
        "from c2\n"
    );
}
