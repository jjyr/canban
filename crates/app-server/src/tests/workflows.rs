use super::*;
fn plan(id: &str, instruction: &str) -> Value {
    json!({"id":id,"name":"Plan and deliver","workspace":{"create_worktree":false,"sync_base":false},"entries":{"start":"plan","develop":"build"},"steps":{"plan":{"type":"agent","profile":"codex","instruction":instruction,"next":"approve"},"approve":{"type":"approval","title":"Approve this exact plan","next":"move","rework":"plan"},"move":{"type":"move","column":"develop","next":"build"},"build":{"type":"agent","profile":"developer","instruction":"Implement the approved plan","next":"finish"},"finish":{"type":"complete"}}})
}
fn publish(app: &AppServer, p: &Project, id: &str, instruction: &str, revision: u64) {
    app.call(
        "workflow.publish",
        json!({"project_id":p.id,"revision":revision,"definition":plan(id,instruction)}),
    )
    .unwrap();
}
fn finish(app: &AppServer, req: &Request, key: &str) {
    let a = admit(app, req);
    app.call("execution.report",json!({"execution":{"request_id":req.id,"attempt_number":a.number},"request_key":key,"state":"completed","message":"Plan ready","body":"Exact plan artifact"})).unwrap();
}
#[test]
fn published_revision_is_immediate_for_new_runs_and_frozen_for_active_runs() {
    let (_dir, app, p, c) = setup();
    publish(&app, &p, "plan", "FIRST", 0);
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
    publish(&app, &p, "plan", "SECOND", 1);
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
            "workflow.publish",
            json!({"project_id":p.id,"revision":1,"definition":plan("plan","bad")})
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
    publish(&app, &p, "plan", "Plan", 0);
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
    publish(&app, &p, "plan", "Plan", 0);
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
fn invalid_graph_cannot_publish_and_old_assignment_survives_engine_adoption() {
    let (dir, app, p, c) = setup();
    let mut def = plan("bad", "Plan");
    def["steps"]["build"]["next"] = json!("plan");
    assert!(
        app.call(
            "workflow.publish",
            json!({"project_id":p.id,"revision":0,"definition":def})
        )
        .is_err()
    );
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut req = request.clone();
            req.workflow_run_id = None;
            req.workflow_step = None;
            db.save(&req)?;
            db.conn.execute_batch("DELETE FROM workflow_runs; DELETE FROM workflow_sources; DELETE FROM workflow_revisions; DELETE FROM event_subscriptions; DELETE FROM event_deliveries;")?;
            Ok(())
        })
        .unwrap();
    drop(app);
    canban_storage::migrations::migrate(&dir.path().join("home"), 10).unwrap();
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
    let flow = json!({"id":"conditional","name":"Conditional observer","entries":{"start":"choose"},"steps":{"choose":{"type":"condition","pointer":"/card/title","equals":c.title,"then":"notify","otherwise":"done"},"notify":{"type":"notify","message":"Condition selected","next":"done"},"done":{"type":"complete"}}});
    app.call(
        "workflow.publish",
        json!({"project_id":p.id,"revision":0,"definition":flow}),
    )
    .unwrap();
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
    publish(&app, &p, "retained", "Keep this definition", 0);
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
    app.call("workflow.publish",json!({"project_id":p.id,"revision":0,"definition":{"id":"restore-safe","name":"Restore safe","entries":{"start":"done"},"steps":{"done":{"type":"complete"}}}})).unwrap();
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
    let (_dir, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    finish(&app, &app.snapshot().unwrap().requests[0], "develop-done");
    finish(&app, &app.snapshot().unwrap().requests[1], "review-done");
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
    let mut definition = legacy_definition(&p.workflow, format!("delivery:{}", p.id));
    if let Some(WorkflowStep::Agent { next, .. }) = definition.steps.get_mut("agent:review") {
        *next = None;
    }
    definition.steps.remove("approval:review");
    let current = app
        .store
        .lock()
        .unwrap()
        .db()
        .get::<WorkflowSource>(&format!("{}:development.yaml", p.id))
        .unwrap();
    app.call(
        "workflow.publish",
        json!({"project_id":p.id,"revision":current.revision,"definition":definition}),
    )
    .unwrap();
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
