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
        12
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
        "# Preserve this comment\nschema: 1\nid: {id}\nname: Test YAML\non:\n  start:\n    event: {event}\n    start_at: gate\nsteps:\n  - id: gate\n    uses: approval\n    with:\n      title: Inspect YAML\n"
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
    assert_eq!(c.prompt, prompt.as_str().unwrap());
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
            run.terminal = None;
            db.save(&run)
        })
        .unwrap();
    app.call("execution.report",json!({"execution":{"request_id":req.id,"attempt_number":a.number},"request_key":key,"state":"completed","message":"Plan ready","body":"Exact plan artifact"})).unwrap();
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
