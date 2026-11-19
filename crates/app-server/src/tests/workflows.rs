use super::*;

#[test]
fn manual_column_moves_dispatch_only_destination_and_record_resulting_card() {
    let (_dir, app, p, c) = setup();
    std::fs::create_dir_all(&p.directory).unwrap();
    // Seed a finished development stage without launching a process.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c.id)?;
            card.stage = "develop".into();
            // Keep delivery at its human gate; this test exercises dispatch,
            // while the isolated acceptance harness exercises actual merging.
            card.hold_auto_merge = true;
            db.save(&card)
        })
        .unwrap();
    let mut observers = Vec::new();
    for view in ["card.column_entered", "card.column_left"] {
        for column in ["develop", "review"] {
            let sub = app.call("subscription.save", json!({"revision":0,"enabled":true,"selector":{"card_id":c.id,"types":[view],"column":column},"target":{"type":"consumer"}})).unwrap();
            observers.push((view, column, sub["id"].clone()));
        }
    }
    for (from, to) in [("develop", "review"), ("review", "develop")] {
        let before = app.snapshot().unwrap();
        let card = before.cards.iter().find(|card| card.id == c.id).unwrap();
        let moved = app
            .call(
                "card.move",
                json!({"view":"full","card_id":c.id,"revision":card.revision,"stage":to}),
            )
            .unwrap();
        let after = app.snapshot().unwrap();
        let event = after
            .events
            .iter()
            .rev()
            .find(|e| e.card_id == c.id && e.kind == "card.stage_changed")
            .unwrap();
        assert_eq!(event.data["from"], from);
        assert_eq!(event.data["to"], to);
        assert_eq!(event.data["stage"], to);
        assert_eq!(event.data["card_revision"], moved["revision"]);
        for (view, column, id) in &observers {
            let deliveries = app
                .call("subscription.deliveries", json!({"subscription_id":id}))
                .unwrap();
            let count = deliveries
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["event_id"] == event.id)
                .count();
            assert_eq!(
                count,
                usize::from(
                    *column
                        == if *view == "card.column_entered" {
                            to
                        } else {
                            from
                        }
                ),
                "{view} {column}"
            );
        }
        let runs: Vec<_> = after
            .workflow_runs
            .iter()
            .filter(|r| r.card_id == c.id && r.active())
            .collect();
        assert_eq!(runs.len(), 1, "runs: {:?}", after.workflow_runs);
        if to == "review" {
            assert_eq!(runs[0].step.as_deref(), Some("deliver:review"));
            assert_eq!(runs[0].state, "waiting_approval");
            assert!(after.terminals.iter().all(|t| t.agent.is_none()));
        } else {
            assert_eq!(runs[0].step.as_deref(), Some("agent:develop"));
            assert_eq!(
                after
                    .terminals
                    .iter()
                    .filter_map(|t| t.agent.as_ref())
                    .filter(|a| a.card_id == c.id && a.profile.id == "developer")
                    .count(),
                1
            );
        }
    }
}

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
    assert!(!full.events.is_empty());
    assert!(!full.questions.is_empty());
    let narrow = app.store.lock().unwrap().db().admission_snapshot().unwrap();
    assert!(narrow.events.is_empty());
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
    let req = app.snapshot().unwrap().terminals[0].clone();
    assert!(req.agent.as_ref().unwrap().instruction.contains("FIRST"));
    finish(&app, &req, "plan-result");
    let s = app.snapshot().unwrap();
    assert_eq!(s.cards[0].stage, "backlog");
    assert_eq!(s.workflow_runs[0].state, "waiting_approval");
    assert_eq!(s.workflow_runs[0].definition_revision, 1);
    assert_eq!(s.terminals.len(), 1);
    assert!(app.call("workflow.decide",json!({"run_id":run["id"],"visit":2,"revision":s.cards[0].revision,"decision":"approve","execution":{}})).is_err());
    app.call("workflow.decide",json!({"run_id":run["id"],"visit":2,"revision":s.cards[0].revision,"decision":"approve","request_key":"approve"})).unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.cards[0].stage, "develop");
    assert_eq!(s.terminals.len(), 2);
    assert_eq!(s.workflow_runs.len(), 1);
    assert!(
        s.terminals[1]
            .agent
            .as_ref()
            .unwrap()
            .instruction
            .contains("report_id")
    );
    let other: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id":p.id,"title":"Second"}),
        )
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
            .terminals
            .last()
            .unwrap()
            .agent
            .as_ref()
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
        json!({"view":"full","card_id":c.id,"body":"one","mentions":[],"request_key":"comment"}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c.id,"body":"one","mentions":[],"request_key":"comment"}),
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
            json!({"view":"full","project_id":p.id,"title":"Automatic"}),
        )
        .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.workflow_runs.len(), 1);
    assert_eq!(s.workflow_runs[0].card_id, c["id"].as_str().unwrap());
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
        app.snapshot()
            .unwrap()
            .events
            .iter()
            .any(|e| e.message == "Still committed")
    );
    assert_eq!(app.snapshot().unwrap().terminals.len(), 1);
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
    let initial_step = app.snapshot().unwrap().terminals[0]
        .agent
        .as_ref()
        .unwrap()
        .workflow_step
        .clone();
    finish(&app, &app.snapshot().unwrap().terminals[0], "done");
    let s = app.snapshot().unwrap();
    let run = s.workflow_runs[0].clone();
    let card = s.cards[0].clone();
    let updated = app
        .call(
            "card.update",
            json!({"view":"full","card_id":card.id,"revision":card.revision,"body":"New requirement"}),
        )
        .unwrap();
    assert!(app.call("workflow.decide",json!({"run_id":run.id,"visit":run.visit,"revision":updated["revision"],"decision":"approve"})).is_err());
    app.call("workflow.decide",json!({"run_id":run.id,"visit":run.visit,"revision":updated["revision"],"decision":"rework"})).unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.workflow_runs[0].visit, 3);
    let rework_step = s.terminals[0]
        .agent
        .as_ref()
        .unwrap()
        .pending
        .as_ref()
        .map(|q| q.workflow_step.clone())
        .unwrap_or_else(|| s.terminals[0].agent.as_ref().unwrap().workflow_step.clone());
    assert_ne!(initial_step, rework_step);
}
#[test]
fn invalid_graph_cannot_publish_and_active_run_survives_service_restart() {
    let (dir, app, p, c) = setup();
    let bad_yaml = plan_yaml("bad", "Plan").replace("next: finish", "next: plan");
    assert!(upsert_yaml(&app, &p, "bad.yaml", &bad_yaml).is_err());
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().terminals[0].clone();
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
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.terminals[0].id, request.id);
    assert!(
        s.terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .workflow_run_id
            .is_some()
    );
    finish(&app, &s.terminals[0], "adopted-done");
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
    app.call(
        "card.restore",
        json!({"view":"full","card_id":c.id,"revision":revision}),
    )
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
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    finish(&app, &app.snapshot().unwrap().terminals[0], "develop-done");
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
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("isolated".into()),
                terminal: Some("local/closed".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
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
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
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
                machine: "local".into(),
                owner_core_id: Some("isolated".into()),
                core_id: Some("isolated".into()),
                terminal: Some("local/gen1".into()),
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
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
            json!({"view":"full","card_id": c.id, "revision": c.revision, "hold": true}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(held.hold_auto_merge);

    // Move to develop -> agent:develop runs -> moves to review -> pauses at deliver:review because hold_auto_merge is true
    app.call(
        "card.move",
        json!({"view":"full","card_id": c.id, "revision": held.revision, "stage": "develop"}),
    )
    .unwrap();
    let req1 = app.snapshot().unwrap().terminals[0].clone();
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
    let req2 = snap.terminals.last().unwrap().clone();
    let rework_instr = req2
        .agent
        .as_ref()
        .unwrap()
        .pending
        .as_ref()
        .map(|q| q.instruction.as_str())
        .unwrap_or_else(|| req2.agent.as_ref().unwrap().instruction.as_str());
    assert!(rework_instr.contains("Please add edge-case unit tests"));

    // Unhold auto-merge while agent works, then finish -> auto-merges / completes!
    let card_rev = snap.cards[0].revision;
    app.call(
        "card.hold_auto_merge",
        json!({"view":"full","card_id": c.id, "revision": card_rev, "hold": false}),
    )
    .unwrap();
    finish(&app, &req2, "dev-2");
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.workflow_runs[0].state, "completed");

    // Verify external PR card pauses at approval:review even when auto-merge is not held
    let ext_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "External PR card"}),
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
        json!({"view":"full","card_id": ext_card.id, "revision": ext_card.revision, "stage": "review"}),
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

#[test]
fn default_workflow_queue_merge_syncs_upstream_and_respects_local_only_option() {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&source).unwrap();

    let git = |cwd: &std::path::Path, args: &[&str]| -> String {
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
    };

    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["init", "-b", "main"]);
    git_identity(&source);
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
            json!({"view":"full","project_id": p.id, "title": "Feature 1", "body": "Sync upstream by default"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "backlog");
    assert!(snap.terminals.iter().all(|t| t.agent.is_none()));
    assert!(snap.workflow_runs.is_empty());

    // 2. Entering Develop runs @developer automatically, moves to Review (no @reviewer),
    // and Queue Merge merges locally + pushes to upstream remote!
    app.call(
        "card.move",
        json!({"view":"full","card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.terminals.len(), 1);
    assert_eq!(
        snap.terminals[0].agent.as_ref().unwrap().profile.id,
        "developer"
    );

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
    git_identity(&wt1);
    std::fs::write(wt1.join("feature1.txt"), "hello upstream\n").unwrap();

    finish_closed(&app, &snap.terminals[0], "dev-c1-done");
    app.pump_deliveries().unwrap();
    // Ensure only 1 agent request ran (no @reviewer step in default Review).
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.terminals.len(), 1);
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
            json!({"view":"full","project_id": p.id, "title": "Feature 2", "body": "Local merge without upstream push"}),
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
    let req2 = snap
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c2.id))
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
    git_identity(&wt2);
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
            json!({"view":"full","project_id": p.id, "title": "Analyze then Develop", "body": "Wait for analysis"}),
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
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c_plan_dev.id))
        .expect("agent:plan request should queue on card.create")
        .clone();
    assert!(
        plan_req
            .agent
            .as_ref()
            .unwrap()
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
        .terminals
        .iter()
        .rfind(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c_plan_dev.id))
        .unwrap()
        .clone();
    let dev_agent = dev_req.agent.as_ref().unwrap();
    let dev_step = dev_agent
        .pending
        .as_ref()
        .and_then(|p| p.workflow_step.as_deref())
        .or(dev_agent.workflow_step.as_deref());
    assert!(dev_step.is_some_and(|s| s.starts_with("agent:develop:")));

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
            json!({"view":"full","project_id": p.id, "title": "Immediate Develop", "body": "Skip analysis"}),
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
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
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
            json!({"view":"full","project_id": p.id, "title": "Edit README", "body": "Conflicting edit"}),
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
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c.id) && r.id != dev.id)
        .expect("conflict rework request")
        .clone();
    let resolve_agent = resolve.agent.as_ref().unwrap();
    assert_eq!(
        resolve_agent.profile.id, "merger",
        "Queue Merge conflicts must dispatch the dedicated @merger role"
    );
    assert!(
        resolve_agent
            .instruction
            .contains("Merge conflict to resolve before delivery")
    );
    assert!(resolve_agent.instruction.contains("README.md"));
    assert!(
        resolve_agent
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
            json!({"view":"full","project_id": p.id, "title": "Edit", "body": "x"}),
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
        snap.terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .filter(|r| r.card_id == c.id)
            .count(),
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
            .find(|r| r.card_id == id && r.active())
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
    let rel_prompt = canban_context::capture(&app.home, &p, &moved, req, &snap).unwrap();
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
    let rel_prompt2 = canban_context::capture(&app.home, &p, card2, &next_att, &snap2).unwrap();
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
            .any(|r| r.card_id == card.id && r.active())
    );
    assert!(
        !after
            .terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .any(|r| r.card_id == card.id && (r.state == "queued" || r.pending.is_some()))
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
            && e.data["stage"] == "backlog"
            && e.data["card_revision"] == updated.revision
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
            json!({"view":"full","project_id": p.id, "title": "Wrap-up Wait", "body": "Wait for turn end"}),
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
    let req1 = snap
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
    std::fs::write(wt1.join("part1.txt"), "committed before report\n").unwrap();
    git_in(&wt1, &["add", "."]);
    git_in(&wt1, &["commit", "-m", "feat: part 1"]);

    let a1 = admit(&app, &req1);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&a1.id)?;
            let run = entry.agent.as_mut().unwrap();
            run.terminal = Some("t-wrapup-1".into());
            run.connection_state = "connected".into();
            run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            db.save(&entry)
        })
        .unwrap();
    app.call(
        "execution.report",
        json!({
            "terminal_id": a1.id,
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
            let mut entry: TerminalEntry = db.get(&a1.id)?;
            entry.agent.as_mut().unwrap().observed =
                Some(json!({"state": "live", "status": {"state": "done"}}));
            db.save(&entry)
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
            json!({"view":"full","project_id": p.id, "title": "Settle Timeout Fallback", "body": "Stuck status"}),
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
    let req2 = snap
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
    std::fs::write(wt2.join("part2.txt"), "timeout fallback\n").unwrap();

    let a2 = admit(&app, &req2);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&a2.id)?;
            let run = entry.agent.as_mut().unwrap();
            run.terminal = Some("t-wrapup-2".into());
            run.connection_state = "connected".into();
            run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            db.save(&entry)
        })
        .unwrap();
    app.call(
        "execution.report",
        json!({
            "terminal_id": a2.id,
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
            let mut entry: TerminalEntry = db.get(&a2.id)?;
            entry.agent.as_mut().unwrap().completed_at = Some(expired);
            db.save(&entry)?;
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
fn backlog_interactive_session_updates_card_adopts_develop_without_retrigger_and_continues_pipeline()
 {
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
            json!({"name": "Discuss Then Adopt", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();

    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full",
                "project_id": p.id,
                "title": "Rough Backlog Idea",
                "body": "Initial notes"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let _other_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full",
                "project_id": p.id,
                "title": "Unrelated Card",
                "body": "Do not touch"
            }),
        )
        .unwrap(),
    )
    .unwrap();

    // 1. Open interactive discussion in Backlog.
    app.call(
        "agent.open",
        json!({"card_id": card.id, "profile_id": "developer"}),
    )
    .unwrap();
    let req = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == card.id))
        .unwrap();
    let req_agent = req.agent.as_ref().unwrap();
    assert_eq!(req_agent.trigger, ExecutionTrigger::Interactive);
    assert!(req_agent.workflow_run_id.is_none());

    let ops = canban_workspaces::Ops::local();
    let wt = ops.prepare(&app.home, &p, &card).unwrap();
    let branch = ops.current_branch(wt.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current: Card = db.get(&card.id)?;
            current.worktree = Some(wt.to_str().unwrap().into());
            current.branch = Some(branch);
            db.save(&current)
        })
        .unwrap();
    git_identity(&wt);
    let term_entry = admit(&app, &req);

    // Prompt teaches both user-confirmed Card updates and permission-gated `card adopt`.
    let prompt_rel =
        canban_context::capture(&app.home, &p, &card, &term_entry, &app.snapshot().unwrap())
            .unwrap();
    let prompt = std::fs::read_to_string(app.home.join(prompt_rel)).unwrap();
    assert!(prompt.contains("card update"));
    assert!(prompt.contains("card adopt"));
    assert!(prompt.contains("Ask the user for permission before starting implementation"));

    // Validation check: `card.adopt` requires `--card` (or `CANBAN_CARD`) / `card_id`.
    assert!(
        app.call("card.adopt", json!({"view":"full","stage": "develop"}))
            .is_err(),
        "card.adopt requires --card / CANBAN_CARD or card_id"
    );

    // 2. Agent updates Card with user-confirmed design key points via CANBAN_PROJECT + CANBAN_CARD (no revision required).
    let updated_card: Card = serde_json::from_value(
        app.call(
            "card.update",
            json!({"view":"full",
                "project_id": p.id,
                "card": card.number,
                "agent": true,
                "title": "Confirmed Design Spec",
                "body": "## Confirmed Design\n- Use `card adopt` without re-triggering scheduling"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(updated_card.title, "Confirmed Design Spec");
    assert!(updated_card.body.contains("Confirmed Design"));
    assert_eq!(updated_card.stage, "backlog");
    let update_event = app
        .snapshot()
        .unwrap()
        .events
        .into_iter()
        .rev()
        .find(|e| e.card_id == card.id && e.kind == "card.updated")
        .unwrap();
    assert_eq!(update_event.actor, "agent:developer");

    // 3. User authorizes implementation: Agent adopts `develop` in the same session.
    let adopt_res = app
        .call(
            "card.adopt",
            json!({"view":"full",
                "project_id": p.id,
                "card": card.number,
                "stage": "develop",
                "request_key": "adopt-1"
            }),
        )
        .unwrap();
    assert_eq!(adopt_res["adopted"], json!(true));
    assert_eq!(adopt_res["card"]["stage"], json!("develop"));
    let run_id = adopt_res["workflow_run"]["id"].as_str().unwrap().to_owned();

    // Calling `card.adopt` again in `develop` is idempotent and returns the same active WorkflowRun.
    let adopt_again = app
        .call(
            "card.adopt",
            json!({"view":"full",
                "project_id": p.id,
                "card": card.number,
                "stage": "develop",
                "request_key": "adopt-2"
            }),
        )
        .unwrap();
    assert_eq!(adopt_again["workflow_run"]["id"], json!(run_id));

    let after_adopt = app.snapshot().unwrap();
    // Crucial: NO second AgentRun was queued by `on.develop` (`card.column_entered`)!
    assert_eq!(
        after_adopt
            .terminals
            .iter()
            .filter(|r| r.agent.as_ref().is_some_and(|a| a.card_id == card.id))
            .count(),
        1,
        "card.adopt must suppress duplicate column_entered scheduling"
    );
    let bound_req = after_adopt
        .terminals
        .iter()
        .find(|r| r.id == term_entry.id)
        .unwrap();
    let bound_agent = bound_req.agent.as_ref().unwrap();
    assert!(bound_agent.pending.is_none());
    assert_eq!(
        bound_agent.workflow_run_id.as_deref(),
        Some(run_id.as_str())
    );
    assert_eq!(bound_agent.stage, "develop");
    let wf_run = after_adopt
        .workflow_runs
        .iter()
        .find(|r| r.id == run_id)
        .unwrap();
    assert_eq!(wf_run.state, "waiting_agent");
    assert_eq!(wf_run.terminal_id.as_deref(), Some(term_entry.id.as_str()));
    assert!(after_adopt.events.iter().any(|e| {
        e.card_id == card.id
            && e.kind == "card.stage_changed"
            && e.data["from"] == "backlog"
            && e.data["to"] == "develop"
            && e.data["reason"] == "agent_adopted_stage"
    }));

    // Updating the Card again from the same bound Agent refreshes `input_digest` so completion does not go `stale`.
    app.call(
        "card.update",
        json!({"view":"full",
            "project_id": p.id,
            "card": card.number,
            "agent": true,
            "body": "## Confirmed Design\n- Use `card adopt`\n- Refresh workflow digest on bound update"
        }),
    )
    .unwrap();

    // 4. Agent implements in the worktree and reports `completed` in the same session -> pipeline moves to Review & merges!
    std::fs::write(wt.join("feature.rs"), "pub fn adopted() -> bool { true }\n").unwrap();
    git_in(&wt, &["add", "."]);
    git_in(&wt, &["commit", "-m", "Implement adopted feature"]);

    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": card.number,
            "request_key": "adopt-complete",
            "state": "completed",
            "message": "Implemented directly from interactive session"
        }),
    )
    .unwrap();

    let final_snap = app.snapshot().unwrap();
    let final_card = final_snap.cards.iter().find(|c| c.id == card.id).unwrap();
    assert_eq!(final_card.stage, "review");
    assert!(
        final_card.delivery.is_some(),
        "completing the adopted session must advance Move to Review -> Queue Merge"
    );
    let final_run = final_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run_id)
        .unwrap();
    assert_eq!(final_run.state, "completed");
}

#[test]
fn comment_session_completed_then_adopted_in_same_live_terminal_advances_to_merge_queue() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    std::fs::write(source.join("lib.rs"), "pub fn base() -> usize { 1 }\n").unwrap();
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
            json!({"name": "Comment Adopt Merge", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full",
                "project_id": p.id,
                "title": "Jump to Card and expand terminal",
                "body": "Initial notes"
            }),
        )
        .unwrap(),
    )
    .unwrap();

    // 1. User posts a `@developer` comment in Backlog -> spawns a comment session.
    app.call(
        "comment.add",
        json!({"view":"full",
            "card_id": card.id,
            "body": "@developer Please analyze the approach first"
        }),
    )
    .unwrap();
    let req = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card.id))
        .expect("comment session queued");
    assert!(req.agent.as_ref().unwrap().trigger.is_comment());

    let ops = canban_workspaces::Ops::local();
    let wt = ops.prepare(&app.home, &p, &card).unwrap();
    let branch = ops.current_branch(wt.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current: Card = db.get(&card.id)?;
            current.worktree = Some(wt.to_str().unwrap().into());
            current.branch = Some(branch);
            db.save(&current)
        })
        .unwrap();
    git_identity(&wt);
    let term_entry = admit(&app, &req);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&term_entry.id)?;
            let run = entry.agent.as_mut().unwrap();
            run.connection_state = "connected".into();
            run.observed = Some(json!({
                "state": "live",
                "turn": 1,
                "status": { "state": "idle" }
            }));
            run.lifecycle.observed_at = Some(now());
            db.save(&entry)
        })
        .unwrap();

    // 2. Agent answers the comment task via `report --state completed` while the PTY stays live.
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": card.number,
            "profile_id": "developer",
            "request_key": "comment-report-1",
            "state": "completed",
            "message": "Analyzed design; ready to implement upon confirmation",
            "body": "## Plan\n1. Update jump handler"
        }),
    )
    .unwrap();
    let after_comment = app.snapshot().unwrap();
    let comment_term = after_comment
        .terminals
        .iter()
        .find(|t| t.id == term_entry.id)
        .unwrap();
    assert_eq!(comment_term.agent.as_ref().unwrap().state, "completed");
    assert!(!comment_term.agent.as_ref().unwrap().finished());
    assert_eq!(
        after_comment
            .cards
            .iter()
            .find(|c| c.id == card.id)
            .unwrap()
            .stage,
        "backlog"
    );

    // 3. User authorizes implementation in the same live terminal -> Agent runs `card adopt`.
    let adopt_res = app
        .call(
            "card.adopt",
            json!({"view":"full",
                "project_id": p.id,
                "card": card.number,
                "profile_id": "developer",
                "stage": "develop",
                "request_key": "comment-adopt-1"
            }),
        )
        .unwrap();
    assert_eq!(adopt_res["adopted"], json!(true));
    assert_eq!(adopt_res["card"]["stage"], json!("develop"));
    let run_id = adopt_res["workflow_run"]["id"].as_str().unwrap().to_owned();

    let after_adopt = app.snapshot().unwrap();
    let adopted_term = after_adopt
        .terminals
        .iter()
        .find(|t| t.id == term_entry.id)
        .unwrap();
    let adopted_agent = adopted_term.agent.as_ref().unwrap();
    assert_eq!(
        adopted_agent.state, "running",
        "adopting a stage in an already-completed live session must reset AgentRun.state to running"
    );
    assert!(adopted_agent.completed_at.is_none());
    assert!(adopted_agent.completion_turn.is_none());
    assert_eq!(adopted_agent.trigger, ExecutionTrigger::Interactive);
    assert_eq!(
        adopted_agent.workflow_run_id.as_deref(),
        Some(run_id.as_str())
    );
    assert_eq!(adopted_agent.number, 2);

    // 4. Agent commits code and runs second `report --state completed` -> advances to Review & merges!
    std::fs::write(
        wt.join("lib.rs"),
        "pub fn base() -> usize { 1 }\npub fn jump_expanded() -> bool { true }\n",
    )
    .unwrap();
    git_in(&wt, &["add", "."]);
    git_in(
        &wt,
        &["commit", "-m", "Implement jump with expanded terminal"],
    );

    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": card.number,
            "profile_id": "developer",
            "request_key": "comment-adopt-report-2",
            "state": "completed",
            "message": "Implemented jump with expanded terminal"
        }),
    )
    .unwrap();

    let final_snap = app.snapshot().unwrap();
    let final_card = final_snap.cards.iter().find(|c| c.id == card.id).unwrap();
    assert_eq!(final_card.stage, "review");
    assert!(
        final_card.delivery.is_some(),
        "second completed report after card.adopt in a live comment terminal must enter Merge Queue and deliver"
    );
    let final_run = final_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run_id)
        .unwrap();
    assert_eq!(final_run.state, "completed");
    let second_reported = final_snap
        .events
        .iter()
        .rev()
        .find(|e| e.card_id == card.id && e.kind == "execution.reported")
        .unwrap();
    assert_eq!(
        second_reported.data["comment_replied"],
        json!(false),
        "adopted workflow completion report must remain visible as a normal task completion report"
    );

    // 5. Defensive fallback: even if legacy/in-flight state had `AgentRun.state == "completed"` while
    // `WorkflowRun` remained `waiting_agent`, a fresh report still advances the workflow.
    let card2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full",
                "project_id": p.id,
                "title": "Legacy stuck adopted terminal",
                "body": "Fallback check"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "agent.open",
        json!({"card_id": card2.id, "profile_id": "developer"}),
    )
    .unwrap();
    let req2 = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card2.id))
        .unwrap();
    let wt2 = ops.prepare(&app.home, &p, &card2).unwrap();
    let branch2 = ops.current_branch(wt2.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current: Card = db.get(&card2.id)?;
            current.worktree = Some(wt2.to_str().unwrap().into());
            current.branch = Some(branch2);
            db.save(&current)
        })
        .unwrap();
    git_identity(&wt2);
    let term2 = admit(&app, &req2);
    let adopt2 = app
        .call(
            "card.adopt",
            json!({"view":"full",
                "project_id": p.id,
                "card": card2.number,
                "stage": "develop",
                "request_key": "legacy-adopt"
            }),
        )
        .unwrap();
    let run2_id = adopt2["workflow_run"]["id"].as_str().unwrap().to_owned();
    // Simulate legacy pre-fix state where `AgentRun.state` stayed `"completed"` while `WorkflowRun` waited.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&term2.id)?;
            entry.agent.as_mut().unwrap().state = "completed".into();
            db.save(&entry)
        })
        .unwrap();
    std::fs::write(wt2.join("fallback.rs"), "pub fn ok() {}\n").unwrap();
    git_in(&wt2, &["add", "."]);
    git_in(&wt2, &["commit", "-m", "Fallback commit"]);
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": card2.number,
            "request_key": "legacy-report",
            "state": "completed",
            "message": "Completed against legacy adopted state"
        }),
    )
    .unwrap();
    let snap2 = app.snapshot().unwrap();
    let final_card2 = snap2.cards.iter().find(|c| c.id == card2.id).unwrap();
    assert_eq!(final_card2.stage, "review");
    assert!(final_card2.delivery.is_some());
    assert_eq!(
        snap2
            .workflow_runs
            .iter()
            .find(|r| r.id == run2_id)
            .unwrap()
            .state,
        "completed"
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
        canban_workflow_engine::stages::set_step_with(
            d,
            "deliver:review",
            "batch_size",
            Some(json!(1)),
        )
        .unwrap();
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
                json!({"view":"full","project_id": p.id, "title": title, "body": title}),
            )
            .unwrap(),
        )
        .unwrap();
        created.push(c);
    }
    let (c1, c2, c3) = (created[0].clone(), created[1].clone(), created[2].clone());

    let prepare_in_develop = |card: &Card| -> (TerminalEntry, PathBuf) {
        app.call(
            "card.move",
            json!({"view":"full","card_id": card.id, "revision": card.revision, "stage": "develop"}),
        )
        .unwrap();
        let snap = app.snapshot().unwrap();
        let req = snap
            .terminals
            .iter()
            .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == card.id))
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
        !snap
            .terminals
            .iter()
            .any(|r| r.agent.as_ref().is_some_and(|a| a.profile.id == "merger")),
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
        !snap.terminals.iter().any(|r| r
            .agent
            .as_ref()
            .is_some_and(|a| a.card_id == c3.id && a.profile.id == "merger")),
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
    let merger_c1: Vec<&TerminalEntry> = snap
        .terminals
        .iter()
        .filter(|r| {
            r.agent
                .as_ref()
                .is_some_and(|a| a.card_id == c1.id && a.profile.id == "merger")
        })
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
        !snap.terminals.iter().any(|r| r
            .agent
            .as_ref()
            .is_some_and(|a| a.card_id == c2.id && a.profile.id == "merger")),
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

#[test]
fn live_card_move_from_backlog_to_review_returns_immediately_while_delivery_merges_in_background() {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&source).unwrap();

    let git = |cwd: &std::path::Path, args: &[&str]| -> String {
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
    };

    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["init", "-b", "main"]);
    git_identity(&source);
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(source.join("README.md"), "# Initial\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "init"]);
    git(&source, &["push", "-u", "origin", "main"]);
    let initial_sha = git(&source, &["rev-parse", "refs/heads/main"]);

    // Gate `git push origin main` on the bare remote so Queue Merge blocks on network/push I/O
    // until released, simulating a slow upstream remote during Backlog -> Review drag.
    let push_started = dir.path().join("push-started");
    let push_release = dir.path().join("push-release");
    let hook = remote.join("hooks/pre-receive");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.02; done\nexit 0\n",
            push_started.display(),
            push_release.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    super::terminal_cases::core(&app, dir.path());
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Live Async Delivery", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Dragged from Backlog to Review", "body": "Ready branch"}),
        )
        .unwrap(),
    )
    .unwrap();
    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Peer Card remains interactive", "body": "Peer"}),
        )
        .unwrap(),
    )
    .unwrap();

    // Prepare a committed worktree on c1 while c1 is still in `backlog`.
    let ops = canban_workspaces::Ops::local();
    let wt1 = ops.prepare(&app.home, &p, &c1).unwrap();
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
    git_identity(&wt1);
    std::fs::write(wt1.join("dragged.txt"), "backlog-to-review\n").unwrap();
    git(&wt1, &["add", "dragged.txt"]);
    git(&wt1, &["commit", "-m", "feat: ready in backlog"]);

    let running = app.start().unwrap();
    let endpoint = app.endpoint();

    // Drag c1 directly from `backlog` to `review` over the live Unix socket RPC.
    let started = std::time::Instant::now();
    let moved: Card = serde_json::from_value(
        rpc(
            &endpoint,
            "card.move",
            json!({"view":"full","card_id": c1.id, "revision": c1.revision, "stage": "review", "before_card_id": null}),
        )
        .unwrap(),
    )
    .unwrap();
    let move_elapsed = started.elapsed();
    assert_eq!(moved.stage, "review");
    assert!(
        move_elapsed < Duration::from_millis(500),
        "live `card.move` to `review` must not block on Git fetch/merge/push (took {move_elapsed:?})"
    );

    // Wait until the background delivery worker enters the gated `git push`.
    let wait_push = std::time::Instant::now();
    while !push_started.exists() {
        assert!(
            wait_push.elapsed() < Duration::from_secs(5),
            "background delivery worker did not start pushing within 5s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // While c1 is actively pushing in the background:
    // 1. Snapshot exposes c1 in `merging_cards` so the Board shows live `Merging`.
    // 2. Peer Card RPCs (reordering/opening/updating) remain fast and unblocked.
    let mid_snap = app.snapshot().unwrap();
    assert!(
        mid_snap.merging_cards.contains(&c1.id),
        "c1 should be tracked in `merging_cards` while background delivery runs"
    );
    let peer_started = std::time::Instant::now();
    let updated_peer: Card = serde_json::from_value(
        rpc(
            &endpoint,
            "card.update",
            json!({"view":"full","card_id": c2.id, "revision": c2.revision, "title": "Peer updated immediately"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(updated_peer.revision, c2.revision + 1);
    assert!(
        peer_started.elapsed() < Duration::from_millis(500),
        "peer RPC must remain fast while another Card merges in background"
    );

    // Release the remote push gate and verify background delivery finishes cleanly.
    std::fs::write(&push_release, "ok").unwrap();
    let wait_done = std::time::Instant::now();
    loop {
        let snap = app.snapshot().unwrap();
        let card = snap.cards.iter().find(|c| c.id == c1.id).unwrap();
        if let Some(delivery) = &card.delivery
            && snap.merging_cards.is_empty()
        {
            let merged_sha = git(&source, &["rev-parse", "refs/heads/main"]);
            assert_ne!(merged_sha, initial_sha);
            assert_eq!(git(&remote, &["rev-parse", "refs/heads/main"]), merged_sha);
            assert_eq!(delivery.commit, merged_sha);
            break;
        }
        assert!(
            wait_done.elapsed() < Duration::from_secs(5),
            "background delivery did not complete after releasing push gate"
        );
        std::thread::sleep(Duration::from_millis(15));
    }

    rpc(&endpoint, "app.shutdown", json!({})).unwrap();
    drop(running);
}

#[test]
fn manual_card_terminal_updates_adopts_and_reports_via_short_card_number_with_zero_agent_runs() {
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
    super::terminal_cases::core(&app, dir.path());
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Manual Terminal Flow", "directory": source}),
        )
        .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Manual Terminal Card", "body": "Draft idea"}),
        )
        .unwrap(),
    )
    .unwrap();

    // 1. User opens a standalone Agent Terminal from the Card in Backlog -> creates worktree, 0 AgentRuns!
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "card_id": card.id, "agent": true, "request_key": "open-manual-agent"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert!(snap.terminals.iter().all(|t| t.agent.is_none()));
    let wt = PathBuf::from(
        snap.cards
            .iter()
            .find(|c| c.id == card.id)
            .unwrap()
            .worktree
            .clone()
            .expect("worktree created on terminal.create"),
    );
    git_identity(&wt);

    // 2. Agent inside that terminal inspects and updates the Card using only `card` number + `cwd`.
    let shown: Card = serde_json::from_value(
        app.call(
            "card.show",
            json!({"view":"full","card": card.number, "cwd": wt}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(shown.id, card.id);

    let updated: Card = serde_json::from_value(
        app.call(
            "card.update",
            json!({"view":"full",
                "card": card.number,
                "cwd": wt,
                "agent": true,
                "title": "Confirmed Manual Terminal Spec",
                "body": "## Design\n- Short card number + cwd"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(updated.title, "Confirmed Manual Terminal Spec");
    assert_eq!(updated.stage, "backlog");

    // 3. User approves starting implementation -> Agent calls `card.adopt` with `card` + `cwd`.
    // Card moves to `develop`, WorkflowRun enters `waiting_agent` with `terminal_id: None`, and ZERO background AgentRuns spawn!
    let adopted = app
        .call(
            "card.adopt",
            json!({"view":"full",
                "card": card.number,
                "cwd": wt,
                "stage": "develop",
                "request_key": "manual-adopt-1"
            }),
        )
        .unwrap();
    assert_eq!(adopted["adopted"], true);
    assert_eq!(adopted["card"]["stage"], "develop");
    let run_id = adopted["workflow_run"]["id"].as_str().unwrap().to_owned();

    let mid_snap = app.snapshot().unwrap();
    assert!(
        mid_snap.terminals.iter().all(|t| t.agent.is_none()),
        "Adopting from a manual Card terminal must not spawn a background @developer AgentRun"
    );
    let wf_run = mid_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run_id)
        .unwrap();
    assert_eq!(wf_run.state, "waiting_agent");
    assert!(wf_run.terminal_id.is_none());

    // Updating the Card again while adopted keeps `WorkflowRun.input_digest` in sync even with `terminal_id: None`.
    app.call(
        "card.update",
        json!({"view":"full",
            "card": card.number,
            "cwd": wt,
            "agent": true,
            "body": "## Design\n- Short card number + cwd\n- Digest synced"
        }),
    )
    .unwrap();

    // 4. Agent commits changes in the Card worktree and runs `canban report --state completed` (`card` + `cwd`).
    std::fs::write(wt.join("manual.txt"), "implemented from manual terminal\n").unwrap();
    git_in(&wt, &["add", "."]);
    git_in(
        &wt,
        &["commit", "-m", "feat: manual terminal implementation"],
    );

    app.call(
        "execution.report",
        json!({
            "card": card.number,
            "cwd": wt,
            "state": "completed",
            "message": "Completed in manual terminal",
            "body": "# Summary\nImplemented and tested.",
            "request_key": "manual-report-done"
        }),
    )
    .unwrap();
    app.pump_deliveries().unwrap();

    let final_snap = app.snapshot().unwrap();
    assert!(
        final_snap.terminals.iter().all(|t| t.agent.is_none()),
        "Entire manual terminal lifecycle completed and merged with zero scheduled AgentRuns"
    );
    assert_eq!(final_snap.reports.len(), 1);
    assert!(final_snap.reports[0].terminal_id.is_none());
    assert_eq!(
        final_snap.reports[0].workflow_run_id.as_deref(),
        Some(run_id.as_str())
    );
    let final_card = final_snap.cards.iter().find(|c| c.id == card.id).unwrap();
    assert_eq!(final_card.stage, "review");
    assert!(
        final_card.delivery.is_some(),
        "Manual terminal report must advance adopted workflow through Review Queue Merge"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("manual.txt")).unwrap(),
        "implemented from manual terminal\n"
    );
}

#[test]
fn queue_merge_supports_rebase_squash_and_merge_strategies_and_routes_conflicts_to_merger() {
    for strategy in ["rebase", "squash", "merge"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        git_in(&source, &["init", "-b", "main"]);
        git_identity(&source);
        std::fs::write(source.join("shared.txt"), "base\n").unwrap();
        git_in(&source, &["add", "."]);
        git_in(&source, &["commit", "-m", "initial"]);

        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let p: Project = serde_json::from_value(
            app.call(
                "project.create",
                json!({
                    "name": format!("Strategy {strategy}"),
                    "directory": source,
                    "workspace": {"create_worktree": true, "sync_base": false}
                }),
            )
            .unwrap(),
        )
        .unwrap();

        let wf = collection(&app, &p);
        let fname = wf["sources"][0]["filename"].as_str().unwrap();
        let mut doc =
            canban_workflow_engine::source::parse(wf["sources"][0]["source"].as_str().unwrap())
                .unwrap();
        canban_workflow_engine::source::set_sync_upstream(&mut doc, false);
        canban_workflow_engine::source::set_merge_strategy(
            &mut doc,
            canban_types::MergeStrategy::parse(strategy).unwrap(),
        );
        let raw = canban_workflow_engine::source::encode(&doc).unwrap();
        app.call(
            "workflow.apply",
            json!({
                "project_id": p.id,
                "revision": wf["revision"],
                "sources": [{"filename": fname, "source": raw}]
            }),
        )
        .unwrap();

        // 1. Clean divergent target delivery: zero raw SHA merge commits!
        let c1: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view":"full","project_id": p.id, "title": "Clean feature"}),
            )
            .unwrap(),
        )
        .unwrap();
        app.call(
            "card.move",
            json!({"view":"full","card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
        )
        .unwrap();
        let dev1 = snap_latest_card_terminal(&app, &c1.id);
        let dev1 = admit(&app, &dev1);
        let ops = canban_workspaces::Ops::local();
        let wt1 = ops.prepare(&app.home, &p, &c1).unwrap();
        let br1 = ops.current_branch(wt1.to_str().unwrap()).unwrap();
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut card: Card = db.get(&c1.id)?;
                card.worktree = Some(wt1.to_str().unwrap().into());
                card.branch = Some(br1);
                db.save(&card)
            })
            .unwrap();
        git_identity(&wt1);
        std::fs::write(wt1.join("c1.txt"), "c1 step 1\n").unwrap();
        git_in(&wt1, &["add", "."]);
        git_in(&wt1, &["commit", "-m", "feat: add c1 step 1"]);
        std::fs::write(wt1.join("c1.txt"), "c1 step 2\n").unwrap();
        git_in(&wt1, &["add", "."]);
        git_in(&wt1, &["commit", "-m", "feat: add c1 step 2"]);

        // Advance `main` on an unrelated file while c1 is in flight.
        std::fs::write(source.join("main_extra.txt"), "main moved\n").unwrap();
        git_in(&source, &["add", "."]);
        git_in(&source, &["commit", "-m", "docs: advance main"]);
        let main_before_c1 = git_in(&source, &["rev-parse", "HEAD"]);

        finish_closed(&app, &dev1, &format!("c1-done-{strategy}"));
        app.pump_deliveries().unwrap();

        let snap = app.snapshot().unwrap();
        assert!(
            snap.cards
                .iter()
                .find(|c| c.id == c1.id)
                .unwrap()
                .delivery
                .is_some(),
            "{strategy}: clean divergent delivery should succeed without @merger"
        );
        let head_subject = git_in(&source, &["log", "-n", "1", "--format=%s"]);
        assert!(
            !head_subject.starts_with("Merge commit '"),
            "{strategy}: must never produce raw SHA merge commit, got `{head_subject}`"
        );
        match strategy {
            "rebase" => {
                assert_eq!(head_subject, "feat: add c1 step 2");
                assert!(git_in(&source, &["rev-list", "--merges", "HEAD"]).is_empty());
            }
            "squash" => {
                assert_eq!(
                    git_in(&source, &["rev-parse", "HEAD^"]),
                    main_before_c1,
                    "squash must create a single commit on top of main"
                );
                assert!(git_in(&source, &["rev-list", "--merges", "HEAD"]).is_empty());
            }
            "merge" => {
                assert_eq!(
                    git_in(&source, &["rev-parse", "HEAD^1"]),
                    main_before_c1,
                    "merge strategy must keep main as first parent"
                );
                assert_eq!(head_subject, "Merge card #1: Clean feature");
            }
            _ => unreachable!(),
        }

        // 2. Conflicting delivery: ALL 3 strategies must route through `@merger` and then land cleanly!
        let c2: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view":"full","project_id": p.id, "title": "Conflicting feature"}),
            )
            .unwrap(),
        )
        .unwrap();
        app.call(
            "card.move",
            json!({"view":"full","card_id": c2.id, "revision": c2.revision, "stage": "develop"}),
        )
        .unwrap();
        let dev2 = snap_latest_card_terminal(&app, &c2.id);
        let dev2 = admit(&app, &dev2);
        let wt2 = ops.prepare(&app.home, &p, &c2).unwrap();
        let br2 = ops.current_branch(wt2.to_str().unwrap()).unwrap();
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut card: Card = db.get(&c2.id)?;
                card.worktree = Some(wt2.to_str().unwrap().into());
                card.branch = Some(br2.clone());
                db.save(&card)
            })
            .unwrap();
        git_identity(&wt2);
        std::fs::write(wt2.join("shared.txt"), "card2 conflict edit\n").unwrap();
        git_in(&wt2, &["add", "."]);
        git_in(&wt2, &["commit", "-m", "feat: update shared from card2"]);

        std::fs::write(source.join("shared.txt"), "main conflict edit\n").unwrap();
        git_in(&source, &["add", "."]);
        git_in(&source, &["commit", "-m", "fix: update shared on main"]);
        let main_before_c2 = git_in(&source, &["rev-parse", "HEAD"]);

        finish_closed(&app, &dev2, &format!("c2-done-{strategy}"));
        app.pump_deliveries().unwrap();

        let snap = app.snapshot().unwrap();
        let run2 = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id == c2.id && r.active())
            .expect("active run waiting on @merger");
        assert_eq!(
            run2.state, "waiting_agent",
            "{strategy}: conflict must route to @merger"
        );
        let merger = snap
            .terminals
            .iter()
            .rfind(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == c2.id && a.profile.id == "merger")
            })
            .expect("@merger terminal queued")
            .clone();
        let merger = admit(&app, &merger);
        let card_for_merger = snap.cards.iter().find(|c| c.id == c2.id).unwrap().clone();
        assert_eq!(ops.prepare(&app.home, &p, &card_for_merger).unwrap(), wt2);
        assert_eq!(
            ops.info(&p, &card_for_merger).unwrap().branch.as_deref(),
            Some(br2.as_str())
        );

        // Simulate `@merger` resolving conflict according to strategy instructions.
        std::fs::write(wt2.join("shared.txt"), "main + card2 resolved\n").unwrap();
        git_in(&wt2, &["add", "shared.txt"]);
        if strategy == "rebase" {
            git_in(&wt2, &["commit", "--no-edit"]);
            git_in(&wt2, &["rebase", "--skip"]);
        } else {
            git_in(
                &wt2,
                &["commit", "-m", "chore(merge): integrate main into card #2"],
            );
        }
        finish_closed(&app, &merger, &format!("c2-merged-{strategy}"));
        app.pump_deliveries().unwrap();

        let snap = app.snapshot().unwrap();
        let card2 = snap.cards.iter().find(|c| c.id == c2.id).unwrap();
        assert!(
            card2.delivery.is_some(),
            "{strategy}: c2 must deliver after @merger resolution"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("shared.txt")).unwrap(),
            "main + card2 resolved\n"
        );
        let final_subject = git_in(&source, &["log", "-n", "1", "--format=%s"]);
        assert!(
            !final_subject.starts_with("Merge commit '"),
            "{strategy}: resolved delivery must not leave raw SHA merge commit, got `{final_subject}`"
        );
        match strategy {
            "rebase" => {
                assert_eq!(final_subject, "feat: update shared from card2");
                assert_eq!(git_in(&source, &["rev-parse", "HEAD^"]), main_before_c2);
            }
            "squash" => {
                assert_eq!(final_subject, "feat: update shared from card2");
                assert_eq!(git_in(&source, &["rev-parse", "HEAD^"]), main_before_c2);
            }
            "merge" => {
                assert_eq!(final_subject, "Merge card #2: Conflicting feature");
                assert_eq!(git_in(&source, &["rev-parse", "HEAD^1"]), main_before_c2);
            }
            _ => unreachable!(),
        }
    }
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
fn failed_rebase_conflict_delivery_retries_via_workflow_deliver_without_burning_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    std::fs::write(source.join("shared.txt"), "base\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "init"]);

    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Rebase Retry",
                "directory": source,
                "workspace": {"create_worktree": true, "sync_base": false}
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let wf = collection(&app, &p);
    let fname = wf["sources"][0]["filename"].as_str().unwrap();
    let mut doc =
        canban_workflow_engine::source::parse(wf["sources"][0]["source"].as_str().unwrap())
            .unwrap();
    canban_workflow_engine::source::set_sync_upstream(&mut doc, false);
    canban_workflow_engine::source::set_merge_strategy(
        &mut doc,
        canban_types::MergeStrategy::Rebase,
    );
    let raw = canban_workflow_engine::source::encode(&doc).unwrap();
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": wf["revision"],
            "sources": [{"filename": fname, "source": raw}]
        }),
    )
    .unwrap();

    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Conflicting rebase"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c.id, "revision": c.revision, "stage": "develop"}),
    )
    .unwrap();
    let dev = admit(&app, &snap_latest_card_terminal(&app, &c.id));
    let ops = canban_workspaces::Ops::local();
    let wt = ops.prepare(&app.home, &p, &c).unwrap();
    let br = ops.current_branch(wt.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c.id)?;
            card.worktree = Some(wt.to_str().unwrap().into());
            card.branch = Some(br.clone());
            db.save(&card)
        })
        .unwrap();
    git_identity(&wt);
    std::fs::write(wt.join("shared.txt"), "card change\n").unwrap();
    git_in(&wt, &["add", "."]);
    git_in(&wt, &["commit", "-m", "feat: card change"]);

    std::fs::write(source.join("shared.txt"), "main change\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "fix: main change"]);
    let main_before = git_in(&source, &["rev-parse", "HEAD"]);

    finish_closed(&app, &dev, "dev-done");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c.id && r.active())
        .unwrap();
    assert_eq!(run.state, "waiting_agent");
    assert_eq!(run.inputs["conflict_attempts"], 1);
    assert_eq!(ops.current_branch(wt.to_str().unwrap()), None);
    let merger = snap
        .terminals
        .iter()
        .rfind(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == c.id && a.profile.id == "merger")
        })
        .unwrap()
        .clone();

    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&merger.id)?;
            let agent = entry.agent.as_mut().unwrap();
            agent.state = "failed".into();
            agent.pending = None;
            agent.terminal = None;
            agent.error = Some("git switch: simulated branch exists".into());
            db.save(&entry)?;
            crate::workflow::reconcile(db)?;
            Ok(())
        })
        .unwrap();

    let snap = app.snapshot().unwrap();
    let failed_run = snap.workflow_runs.iter().find(|r| r.id == run.id).unwrap();
    assert_eq!(failed_run.state, "failed");
    assert!(failed_run.error.is_some());
    let card_rev = snap.cards.iter().find(|x| x.id == c.id).unwrap().revision;

    let retried = app
        .call(
            "workflow.deliver",
            json!({"card_id": c.id, "revision": card_rev}),
        )
        .unwrap();
    assert_eq!(retried["state"], "waiting_agent");
    assert_eq!(retried["inputs"]["conflict_attempts"], 1);
    assert!(retried["error"].is_null());

    let snap = app.snapshot().unwrap();
    let card_for_merger = snap.cards.iter().find(|x| x.id == c.id).unwrap().clone();
    assert_eq!(ops.prepare(&app.home, &p, &card_for_merger).unwrap(), wt);
    let info = ops.info(&p, &card_for_merger).unwrap();
    assert_eq!(info.branch.as_deref(), Some(br.as_str()));
    assert_eq!(info.state, "conflict");

    let merger2 = snap
        .terminals
        .iter()
        .rfind(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == c.id && a.profile.id == "merger")
        })
        .unwrap()
        .clone();
    let merger2 = admit(&app, &merger2);
    std::fs::write(wt.join("shared.txt"), "resolved\n").unwrap();
    git_in(&wt, &["add", "shared.txt"]);
    git_in(&wt, &["commit", "--no-edit"]);
    git_in(&wt, &["rebase", "--skip"]);
    finish_closed(&app, &merger2, "merger-done");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let delivered = snap.cards.iter().find(|x| x.id == c.id).unwrap();
    assert!(delivered.delivery.is_some());
    assert_eq!(
        std::fs::read_to_string(source.join("shared.txt")).unwrap(),
        "resolved\n"
    );
    assert_eq!(git_in(&source, &["rev-parse", "HEAD^"]), main_before);
}

fn script_exec_hc(app: &AppServer, dir: &Path) {
    let hc = dir.join("script-exec-hc");
    std::fs::write(
        &hc,
        r#"#!/usr/bin/python3
import json, os, signal, subprocess, sys
base = os.path.dirname(__file__)
state_file = os.path.join(base, "script_counter")
for line in sys.stdin:
    p = json.loads(line)
    m = p["method"]
    args = p.get("params", {})
    r = {}
    if m == "hello":
        r = {"generation": 1}
    elif m == "server.status":
        r = {"core": {"id": "script-test-core"}}
    elif m == "terminal.run":
        with open(os.path.join(base, "script_launches.jsonl"), "a") as f:
            f.write(json.dumps(args) + "\n")
        n = 1
        if os.path.exists(state_file):
            n = int(open(state_file).read().strip()) + 1
        with open(state_file, "w") as f:
            f.write(str(n))
        addr = f"s{n}"
        env = {**os.environ, **{str(k): str(v) for k, v in args.get("env", {}).items()}}
        runner = (
            "import os, subprocess, sys; "
            "p = subprocess.Popen(sys.argv[3:], cwd=sys.argv[2], start_new_session=True); "
            "open(sys.argv[1] + '.pid', 'w').write(str(p.pid)); "
            "rc = p.wait(); "
            "open(sys.argv[1] + '.done', 'w').write(str(rc))"
        )
        prefix = os.path.join(base, addr)
        subprocess.Popen(
            ["/usr/bin/python3", "-c", runner, prefix, args["cwd"], *args["argv"]],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        r = {"address": addr}
    elif m == "terminal.get":
        addr = args["terminal"]
        prefix = os.path.join(base, addr)
        done = prefix + ".done"
        if os.path.exists(done):
            rc = int(open(done).read().strip())
            r = {"address": addr, "state": "finished", "exit_code": rc}
        else:
            r = {"address": addr, "state": "live"}
    elif m == "terminal.read":
        r = {"text": ""}
    elif m == "terminal.kill":
        addr = args["terminal"]
        pid_file = os.path.join(base, addr + ".pid")
        if os.path.exists(pid_file):
            pid = int(open(pid_file).read().strip())
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        r = {"sent": True}
    elif m == "terminal.list":
        r = {"terminals": []}
    print(json.dumps({"jsonrpc": "2.0", "id": p["id"], "result": r}), flush=True)
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hc, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    app.config.write().unwrap().hc = hc.to_string_lossy().into();
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
 elif m in ['terminal.kill','terminal.interrupt']:
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
        .find(|r| r.card_id == c.id)
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
fn script_step_bypasses_max_agents_supports_continue_on_error_timeout_and_single_on_failure_recovery()
 {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    app.config.write().unwrap().scheduler.max_agents = 1;

    let wf_yaml = r#"schema: 6
id: delivery:script-recovery
name: Script Recovery
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
    start_at: agent:blocker
    match:
      column: develop
  manual_recovery:
    event: manual
    start_at: script:soft_fail
steps:
  - id: agent:blocker
    uses: agent
    with:
      profile: developer
    next: __end
  - id: script:soft_fail
    name: Soft check
    continue_on_error: true
    run: |
      echo "soft failure output"
      exit 7
    next: script:guarded
  - id: script:guarded
    name: Guarded build
    uses: script
    on_failure: agent:fix
    with:
      run: |
        if [ -f fixed.ok ]; then
          echo "build-fixed"
          exit 0
        fi
        echo "UNIT_TEST_BROKEN_BUILD" >&2
        exit 42
    next: script:timeout_check
  - id: agent:fix
    name: Fix failure
    uses: agent
    with:
      profile: developer
      instruction: Repair the broken script.
    next: script:guarded
  - id: script:timeout_check
    name: Slow script
    timeout_seconds: 1
    run: |
      sleep 10
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
            json!({"name": "Script Recovery", "directory": source, "workflow_yaml": wf_yaml}),
        )
        .unwrap(),
    )
    .unwrap();

    // Occupy the single agent slot (`max_agents = 1`) with Card 1.
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Slot holder"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let holder = snap_latest_card_terminal(&app, &c1.id);
    let _holder = admit(&app, &holder);

    // Start Card 2 at `script:soft_fail`: scripts must bypass `max_agents = 1`, ignore non-zero under
    // `continue_on_error: true`, and trigger `on_failure: agent:fix` ONCE on `script:guarded`.
    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Script recovery"}),
        )
        .unwrap(),
    )
    .unwrap();
    let run_val = app
        .call(
            "workflow.start",
            json!({
                "card_id": c2.id,
                "workflow_id": "delivery:script-recovery",
                "entry": "manual_recovery"
            }),
        )
        .unwrap();
    let run_id = run_val["id"].as_str().unwrap().to_owned();

    // First failure auto-routes to `agent:fix` carrying exit code 42 and log tail.
    let snap1 = app.snapshot().unwrap();
    let run1 = snap1.workflow_runs.iter().find(|r| r.id == run_id).unwrap();
    assert_eq!(run1.step.as_deref(), Some("agent:fix"));
    let fix_req = snap_latest_card_terminal(&app, &c2.id);
    let fix_instr = &fix_req.agent.as_ref().unwrap().instruction;
    assert!(fix_instr.contains("42") && fix_instr.contains("UNIT_TEST_BROKEN_BUILD"));

    // Finish `agent:fix` without creating `fixed.ok`: second failure must pause for human approval
    // instead of looping back to `agent:fix`.
    finish(&app, &fix_req, "fix-attempt-1");
    let snap2 = app.snapshot().unwrap();
    let run2 = snap2.workflow_runs.iter().find(|r| r.id == run_id).unwrap();
    let card2 = snap2.cards.iter().find(|c| c.id == c2.id).unwrap();
    assert_eq!(run2.state, "waiting_approval");
    assert_eq!(run2.step.as_deref(), Some("script:guarded"));
    assert!(card2.human_gate.is_some());

    // Create `fixed.ok` and click Retry (`workflow.decide` approve): `script:guarded` exits 0 and
    // advances into `script:timeout_check` (`timeout_seconds: 1`), which terminates and pauses.
    std::fs::write(source.join("fixed.ok"), "fixed\n").unwrap();
    app.call(
        "workflow.decide",
        json!({
            "run_id": run_id,
            "visit": run2.visit,
            "revision": card2.revision,
            "decision": "approve",
            "request_key": "retry-guarded"
        }),
    )
    .unwrap();

    let snap3 = app.snapshot().unwrap();
    let run3 = snap3.workflow_runs.iter().find(|r| r.id == run_id).unwrap();
    assert_eq!(run3.state, "waiting_approval");
    assert_eq!(run3.step.as_deref(), Some("script:timeout_check"));
    assert!(
        snap3
            .events
            .iter()
            .any(|e| e.kind == "workflow.script_failed" && e.message.contains("timed out"))
    );
}

#[test]
fn script_step_executes_via_herdr_runtime_with_labels_and_logs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    script_exec_hc(&app, dir.path());

    let wf_yaml = r#"schema: 6
id: delivery:herdr-script
name: Herdr Script
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  check_entry:
    event: manual
    start_at: check
steps:
  - id: check
    run: |
      echo "herdr-script-ok"
      echo "done=1" >> "$CANBAN_OUTPUT"
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
            json!({"name": "Herdr Script", "directory": source, "workflow_yaml": wf_yaml}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Herdr script card"}),
        )
        .unwrap(),
    )
    .unwrap();

    let _running = app.start().unwrap();
    app.call(
        "workflow.start",
        json!({"card_id": c.id, "workflow_id": "delivery:herdr-script", "entry": "check_entry"}),
    )
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let final_run = loop {
        app.request_scripts();
        let snap = app.snapshot().unwrap();
        if let Some(run) = snap
            .workflow_runs
            .into_iter()
            .find(|r| r.card_id == c.id && r.state == "completed")
        {
            break run;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for Herdr-backed script run completion"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(final_run.outputs["check"]["done"], "1");
    let launches = std::fs::read_to_string(dir.path().join("script_launches.jsonl")).unwrap();
    let launch: Value = serde_json::from_str(launches.lines().next().unwrap()).unwrap();
    assert_eq!(launch["labels"]["canban.script_step"], "check");
    assert_eq!(launch["labels"]["canban.card_id"], c.id);
}

#[test]
fn failed_merger_retry_joins_tail_of_merge_queue_behind_active_card() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    std::fs::write(source.join("a.txt"), "base\n").unwrap();
    std::fs::write(source.join("b.txt"), "base\n").unwrap();
    std::fs::write(source.join("c.txt"), "base\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "init"]);

    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Merger Retry FIFO",
                "directory": source,
                "workspace": {"create_worktree": true, "sync_base": false}
            }),
        )
        .unwrap(),
    )
    .unwrap();
    edit_development(&app, &p, |doc| {
        canban_workflow_engine::source::set_review_manual(doc, false);
        canban_workflow_engine::source::set_sync_upstream(doc, false);
        canban_workflow_engine::stages::set_step_with(
            doc,
            "deliver:review",
            "batch_size",
            Some(json!(1)),
        )
        .unwrap();
    });
    let ops = canban_workspaces::Ops::local();

    // Helper: create a Card whose worktree conflicts with `main` on `conflict_file`, finish `@developer`,
    // and move the Card into the Deliver (`review`) step.
    let enter_conflicting_deliver = |title: &str, conflict_file: &str| -> (Card, PathBuf) {
        let card: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view":"full","project_id": p.id, "title": title}),
            )
            .unwrap(),
        )
        .unwrap();
        app.call(
            "card.move",
            json!({"view":"full","card_id": card.id, "revision": card.revision, "stage": "develop"}),
        )
        .unwrap();
        let dev = admit(&app, &snap_latest_card_terminal(&app, &card.id));
        let wt = ops.prepare(&app.home, &p, &card).unwrap();
        let br = ops.current_branch(wt.to_str().unwrap()).unwrap();
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut stored: Card = db.get(&card.id)?;
                stored.worktree = Some(wt.to_str().unwrap().into());
                stored.branch = Some(br);
                db.save(&stored)
            })
            .unwrap();
        git_identity(&wt);
        std::fs::write(wt.join(conflict_file), format!("{title} change\n")).unwrap();
        git_in(&wt, &["add", "."]);
        git_in(&wt, &["commit", "-m", &format!("feat: {title}")]);

        std::fs::write(source.join(conflict_file), format!("main vs {title}\n")).unwrap();
        git_in(&source, &["add", "."]);
        git_in(
            &source,
            &["commit", "-m", &format!("main update for {title}")],
        );

        finish_closed(&app, &dev, &format!("dev-done-{title}"));
        let updated = app
            .snapshot()
            .unwrap()
            .cards
            .into_iter()
            .find(|c| c.id == card.id)
            .unwrap();
        (updated, wt)
    };

    // Card A enters Queue Merge first and hits conflict -> dispatches `@merger`.
    let (card_a, wt_a) = enter_conflicting_deliver("Card A", "a.txt");
    app.pump_deliveries().unwrap();
    // Card B enters Queue Merge second and waits behind Card A.
    let (card_b, wt_b) = enter_conflicting_deliver("Card B", "b.txt");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    assert_eq!(snap.merge_queue_status(&card_a.id).unwrap().0, 1);
    assert_eq!(snap.merge_queue_status(&card_b.id).unwrap().0, 2);

    // Simulate Card A's `@merger` failing (`agent.state = "failed"`).
    let merger_a1 = snap_latest_card_terminal(&app, &card_a.id);
    assert_eq!(merger_a1.agent.as_ref().unwrap().profile.id, "merger");
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&merger_a1.id)?;
            let agent = entry.agent.as_mut().unwrap();
            agent.state = "failed".into();
            agent.pending = None;
            agent.terminal = None;
            agent.error = Some("simulated merger failure".into());
            db.save(&entry)?;
            crate::workflow::reconcile(db)?;
            Ok(())
        })
        .unwrap();

    // Card A left the active queue on failure; pumping advances Card B to #1 and launches B's `@merger`.
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    let run_a = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == card_a.id && r.active())
        .unwrap();
    let run_b = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == card_b.id && r.active())
        .unwrap();
    assert_eq!(run_a.state, "failed");
    assert!(run_a.inputs["queued_at"].is_null());
    assert_eq!(run_b.state, "waiting_agent");
    assert_eq!(snap.merge_queue_status(&card_b.id).unwrap().0, 1);

    // 1. Retry Card A via `workflow.deliver` WHILE Card B's `@merger` is actively running!
    // Card A must enter `waiting_delivery` behind Card B (#2 of 2) instead of jumping ahead or spawning `@merger`.
    let card_a_rev = snap
        .cards
        .iter()
        .find(|c| c.id == card_a.id)
        .unwrap()
        .revision;
    let retried_a = app
        .call(
            "workflow.deliver",
            json!({"card_id": card_a.id, "revision": card_a_rev}),
        )
        .unwrap();
    assert_eq!(retried_a["state"], "waiting_delivery");
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let (pos_b, total_b, _) = snap.merge_queue_status(&card_b.id).unwrap();
    let (pos_a, total_a, _) = snap.merge_queue_status(&card_a.id).unwrap();
    assert_eq!((pos_b, total_b), (1, 2));
    assert_eq!(
        (pos_a, total_a),
        (2, 2),
        "retried failed @merger must queue at #2 behind active @merger on Card B"
    );
    let merger_a_after_retry = snap_latest_card_terminal(&app, &card_a.id);
    assert!(
        merger_a_after_retry.agent.as_ref().unwrap().state == "failed",
        "no new @merger session may start for Card A while Card B holds head-of-line"
    );

    // Complete Card B's `@merger`: Card B delivers and advances `main`.
    let merger_b = admit(&app, &snap_latest_card_terminal(&app, &card_b.id));
    std::fs::write(wt_b.join("b.txt"), "resolved b\n").unwrap();
    git_in(&wt_b, &["add", "b.txt"]);
    git_in(&wt_b, &["commit", "--no-edit"]);
    git_in(&wt_b, &["rebase", "--skip"]);
    finish_closed(&app, &merger_b, "merger-b-done");
    app.pump_deliveries().unwrap();

    // Card A now automatically advances to #1 and dispatches a fresh `@merger` run AFTER Card B delivered.
    let snap = app.snapshot().unwrap();
    assert!(
        snap.cards
            .iter()
            .find(|c| c.id == card_b.id)
            .unwrap()
            .delivery
            .is_some()
    );
    let run_a = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == card_a.id && r.active())
        .unwrap();
    assert_eq!(run_a.state, "waiting_agent");
    let merger_a2 = admit(&app, &snap_latest_card_terminal(&app, &card_a.id));
    assert!(
        merger_a2.agent.as_ref().unwrap().number > merger_a1.agent.as_ref().unwrap().number,
        "Card A must get a fresh @merger run after advancing back to #1"
    );
    assert!(
        merger_a2
            .agent
            .as_ref()
            .unwrap()
            .instruction
            .contains("Merge conflict to resolve before delivery")
    );

    // 2. Simulate `merger_a2` aborting the stale rebase and reporting `--state blocked`
    // (sets Card gate & `run.state == "blocked"`).
    git_in(&wt_a, &["rebase", "--abort"]);
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": card_a.number,
            "terminal_id": merger_a2.id,
            "state": "blocked",
            "message": "Need product decision on a.txt",
            "request_key": "merger-a2-blocked"
        }),
    )
    .unwrap();
    app.call("terminal.close", json!({"terminal_id": merger_a2.id}))
        .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&merger_a2.id)?;
            entry.agent.as_mut().unwrap().observed =
                Some(json!({"state":"finished","exit_code":0}));
            db.save(&entry)
        })
        .unwrap();

    // Card C enters and holds `#1` with an active `@merger`.
    let (card_c, wt_c) = enter_conflicting_deliver("Card C", "c.txt");
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.merge_queue_status(&card_c.id).unwrap().0, 1);

    // Retry Card A's blocked `@merger` via `execution.retry` WHILE Card C's `@merger` is active!
    // Must queue Card A in `waiting_delivery` behind Card C, NOT call `retry_agent` directly!
    app.call("execution.retry", json!({"terminal_id": merger_a2.id}))
        .unwrap();
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let run_a = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id == card_a.id && r.active())
        .unwrap();
    assert_eq!(run_a.state, "waiting_delivery");
    assert_eq!(snap.merge_queue_status(&card_c.id).unwrap().0, 1);
    assert_eq!(snap.merge_queue_status(&card_a.id).unwrap().0, 2);

    // Finish Card C -> `main` advances to `main_after_c`, and Card A advances to #1 -> hits conflict -> dispatches `merger_a3`.
    let merger_c = admit(&app, &snap_latest_card_terminal(&app, &card_c.id));
    std::fs::write(wt_c.join("c.txt"), "resolved c\n").unwrap();
    git_in(&wt_c, &["add", "c.txt"]);
    git_in(&wt_c, &["commit", "--no-edit"]);
    git_in(&wt_c, &["rebase", "--skip"]);
    finish_closed(&app, &merger_c, "merger-c-done");
    app.pump_deliveries().unwrap();
    let main_after_c = git_in(&source, &["rev-parse", "HEAD"]);

    // 3. Also verify `merger_a3` saw the updated `main_after_c` head and `card.continue` on blocked `@merger` goes through the Merge Queue.
    let merger_a3 = admit(&app, &snap_latest_card_terminal(&app, &card_a.id));
    assert!(
        merger_a3
            .agent
            .as_ref()
            .unwrap()
            .instruction
            .contains(&main_after_c),
        "Card A's queued retry must observe the updated target HEAD after Card C delivered"
    );
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": card_a.number,
            "terminal_id": merger_a3.id,
            "state": "failed",
            "message": "Conflict resolution failed",
            "request_key": "merger-a3-failed"
        }),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&merger_a3.id)?;
            entry.agent.as_mut().unwrap().observed =
                Some(json!({"state":"finished","exit_code":1}));
            db.save(&entry)
        })
        .unwrap();

    // Resolve Card A's conflict in its worktree so the eventual queued delivery finishes cleanly.
    std::fs::write(wt_a.join("a.txt"), "resolved a\n").unwrap();
    git_in(&wt_a, &["add", "a.txt"]);
    git_in(&wt_a, &["commit", "--no-edit"]);
    git_in(&wt_a, &["rebase", "--skip"]);

    let snap = app.snapshot().unwrap();
    let card_a_rev = snap
        .cards
        .iter()
        .find(|c| c.id == card_a.id)
        .unwrap()
        .revision;
    app.call(
        "card.continue",
        json!({"card_id": card_a.id, "revision": card_a_rev}),
    )
    .unwrap();
    app.pump_deliveries().unwrap();

    let snap = app.snapshot().unwrap();
    let delivered_a = snap.cards.iter().find(|c| c.id == card_a.id).unwrap();
    assert!(
        delivered_a.delivery.is_some(),
        "card.continue on blocked @merger must complete via the Merge Queue"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("a.txt")).unwrap(),
        "resolved a\n"
    );
}

struct BatchQueueTestHarness {
    dir: tempfile::TempDir,
    source: PathBuf,
    remote: PathBuf,
    pushes_log: PathBuf,
    app: std::sync::Arc<AppServer>,
    project: Project,
    ops: canban_workspaces::Ops,
}

impl BatchQueueTestHarness {
    fn new(test_command: Option<&str>, agy_merger: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let remote = dir.path().join("remote.git");
        let pushes_log = dir.path().join("pushes.log");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&remote).unwrap();

        git_in(&source, &["init", "-b", "main"]);
        git_identity(&source);
        std::fs::write(source.join("shared.txt"), "base\n").unwrap();
        git_in(&source, &["add", "shared.txt"]);
        git_in(&source, &["commit", "-m", "initial commit"]);

        git_in(&remote, &["init", "--bare", "-b", "main"]);
        let hook_path = remote.join("hooks").join("post-receive");
        std::fs::write(
            &hook_path,
            format!("#!/bin/sh\ncat >> '{}'\n", pushes_log.to_string_lossy()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git_in(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_in(&source, &["push", "-u", "origin", "main"]);
        let _ = std::fs::remove_file(&pushes_log);
        // Pause auto-merge on the main checkout while test Cards finish and enter the queue,
        // preventing the background delivery worker from draining Card 1 before Card 2..N arrive.
        std::fs::write(
            source.join("wip.local"),
            "hold queue until batch is populated\n",
        )
        .unwrap();

        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        if agy_merger {
            let mut cfg = app.config();
            if let Some(m) = cfg.profiles.iter_mut().find(|p| p.id == "merger") {
                m.preset = Some("agy".into());
                m.argv = None;
                m.resume_argv = None;
            } else {
                cfg.profiles.push(Profile {
                    id: "merger".into(),
                    preset: Some("agy".into()),
                    ..Default::default()
                });
            }
            let resolved = canban_profiles::resolve(cfg, &app.home).unwrap();
            *app.config.write().unwrap() = resolved;
        }
        let project: Project = serde_json::from_value(
            app.call(
                "project.create",
                json!({
                    "name": "Batch Merge Queue",
                    "directory": source,
                    "workspace": {"create_worktree": true, "sync_base": false}
                }),
            )
            .unwrap(),
        )
        .unwrap();
        edit_development(&app, &project, |doc| {
            canban_workflow_engine::source::set_review_manual(doc, false);
            canban_workflow_engine::source::set_sync_upstream(doc, true);
            if let Some(cmd) = test_command {
                canban_workflow_engine::stages::set_step_with(
                    doc,
                    "deliver:review",
                    "test_command",
                    Some(json!(cmd)),
                )
                .unwrap();
            }
        });
        Self {
            dir,
            source,
            remote,
            pushes_log,
            app,
            project,
            ops: canban_workspaces::Ops::local(),
        }
    }

    fn finish_card_in_develop(&self, title: &str, file: &str, content: &str) -> (Card, PathBuf) {
        let card: Card = serde_json::from_value(
            self.app
                .call(
                    "card.create",
                    json!({"view":"full","project_id": self.project.id, "title": title, "body": title}),
                )
                .unwrap(),
        )
        .unwrap();
        self.app
            .call(
                "card.move",
                json!({"view":"full","card_id": card.id, "revision": card.revision, "stage": "develop"}),
            )
            .unwrap();
        let dev = admit(&self.app, &snap_latest_card_terminal(&self.app, &card.id));
        let wt = self
            .ops
            .prepare(&self.app.home, &self.project, &card)
            .unwrap();
        let br = self.ops.current_branch(wt.to_str().unwrap()).unwrap();
        self.app
            .store
            .lock()
            .unwrap()
            .write(|db| {
                let mut stored: Card = db.get(&card.id)?;
                stored.worktree = Some(wt.to_str().unwrap().into());
                stored.branch = Some(br);
                db.save(&stored)
            })
            .unwrap();
        git_identity(&wt);
        std::fs::write(wt.join(file), content).unwrap();
        git_in(&wt, &["add", file]);
        git_in(&wt, &["commit", "-m", title]);
        finish_closed(&self.app, &dev, &format!("dev-{}", card.number));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let updated_card = self
            .app
            .snapshot()
            .unwrap()
            .cards
            .into_iter()
            .find(|c| c.id == card.id)
            .unwrap();
        (updated_card, wt)
    }

    fn push_count(&self) -> usize {
        std::fs::read_to_string(&self.pushes_log)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    fn release_and_pump(&self, expected_queued: usize) {
        let queue_len = self
            .app
            .snapshot()
            .unwrap()
            .merge_queue(&self.project.id)
            .len();
        assert_eq!(
            queue_len, expected_queued,
            "All {expected_queued} batch Cards must be queued together before pumping"
        );
        let _ = std::fs::remove_file(self.source.join("wip.local"));
        // Serialize behind any in-flight background worker pass before pumping synchronously.
        for _ in 0..40 {
            self.app.pump_deliveries().unwrap();
            let snap = self.app.snapshot().unwrap();
            let still_waiting_unbatched = snap
                .merge_queue(&self.project.id)
                .into_iter()
                .any(|r| r.state == "waiting_delivery")
                && snap
                    .active_merge_batch_for_project(&self.project.id)
                    .is_none();
            if !still_waiting_unbatched {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

#[test]
fn batch_merge_queue_integrates_five_clean_cards_without_tests_in_single_push() {
    let env = BatchQueueTestHarness::new(None, false);
    let mut cards = Vec::new();
    for i in 1..=5 {
        let (c, _) = env.finish_card_in_develop(
            &format!("Clean batch card {i}"),
            &format!("feature_{i}.txt"),
            &format!("feature {i}\n"),
        );
        cards.push(c);
    }

    env.release_and_pump(5);

    let snap = env.app.snapshot().unwrap();
    for (idx, c) in cards.iter().enumerate() {
        let live = snap.cards.iter().find(|x| x.id == c.id).unwrap();
        assert!(
            live.delivery.is_some(),
            "Card #{} must be delivered cleanly in the batch",
            c.number
        );
        assert_eq!(
            std::fs::read_to_string(env.source.join(format!("feature_{}.txt", idx + 1))).unwrap(),
            format!("feature {}\n", idx + 1)
        );
    }
    assert_eq!(
        env.push_count(),
        1,
        "5 clean Cards in one batch must push upstream exactly once"
    );
    assert!(
        snap.merge_batches.is_empty(),
        "Clean batch with no test failure or conflict must not leave persistent MergeBatch records"
    );
    assert!(
        !snap
            .terminals
            .iter()
            .any(|t| t.id.starts_with("merge-queue:")
                || t.agent.as_ref().is_some_and(|a| a.profile.id == "merger")),
        "Clean batch without test failures must never launch @merger"
    );
    let remote_head = git_in(&env.remote, &["rev-parse", "refs/heads/main"]);
    let local_head = git_in(&env.source, &["rev-parse", "HEAD"]);
    assert_eq!(remote_head, local_head);
}

#[test]
fn batch_merge_queue_runs_test_command_once_for_clean_batch() {
    let dir_tmp = tempfile::tempdir().unwrap();
    let test_log = dir_tmp.path().join("batch_test_runs.log");
    let test_cmd = format!("echo tested >> '{}'", test_log.to_string_lossy());
    let env = BatchQueueTestHarness::new(Some(&test_cmd), false);
    let mut cards = Vec::new();
    for i in 1..=3 {
        let (c, _) = env.finish_card_in_develop(
            &format!("Tested card {i}"),
            &format!("tested_{i}.txt"),
            &format!("tested {i}\n"),
        );
        cards.push(c);
    }

    env.release_and_pump(3);

    let snap = env.app.snapshot().unwrap();
    for c in &cards {
        assert!(
            snap.cards
                .iter()
                .find(|x| x.id == c.id)
                .unwrap()
                .delivery
                .is_some()
        );
    }
    let test_runs = std::fs::read_to_string(&test_log)
        .unwrap_or_default()
        .lines()
        .filter(|l| *l == "tested")
        .count();
    assert_eq!(
        test_runs, 1,
        "Configured test_command must run exactly once for the whole clean batch"
    );
    assert_eq!(
        env.push_count(),
        1,
        "Tested clean batch must push upstream once after tests pass"
    );
    assert!(snap.merge_batches.is_empty());
}

#[test]
fn batch_merge_queue_conflict_dispatches_single_merger_with_agy_trusted_paths_and_completes_on_batch_report()
 {
    let env = BatchQueueTestHarness::new(None, true);
    let (c1, wt1) = env.finish_card_in_develop("Batch card 1", "shared.txt", "from card 1\n");
    let (c2, wt2) =
        env.finish_card_in_develop("Batch card 2 conflicts", "shared.txt", "from card 2\n");
    let (c3, wt3) = env.finish_card_in_develop("Batch card 3", "card3.txt", "from card 3\n");

    super::terminal_cases::core(&env.app, env.dir.path());
    env.release_and_pump(3);
    env.app.tick().unwrap();

    let snap = env.app.snapshot().unwrap();
    assert_eq!(
        snap.merge_batches.len(),
        1,
        "Batch conflict must create exactly one MergeBatch record"
    );
    let batch = &snap.merge_batches[0];
    assert!(
        batch
            .id
            .starts_with(&format!("merge-queue:{}:", env.project.id))
    );
    assert_eq!(batch.state, "running");
    assert!(batch.queued_at.is_some_and(|q| q >= batch.created_at));
    assert!(
        batch
            .started_at
            .is_some_and(|s| s >= batch.queued_at.unwrap())
    );
    assert_eq!(batch.failure_kind.as_deref(), Some("conflict"));
    assert_eq!(batch.conflict_card_number, Some(c2.number));
    assert_eq!(batch.items.len(), 3);
    for c in [&c1, &c2, &c3] {
        let run = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id == c.id && r.active())
            .unwrap();
        assert_eq!(run.state, "waiting_batch");
    }

    let entry = snap.terminals.iter().find(|t| t.id == batch.id).unwrap();
    assert_eq!(entry.agent_engine.as_deref(), Some("agy"));
    let launches = std::fs::read_to_string(env.dir.path().join("launches")).unwrap();
    let launch: Value = serde_json::from_str(launches.lines().last().unwrap()).unwrap();
    let merge_wt = PathBuf::from(&batch.worktree);
    assert_eq!(launch["cwd"], batch.worktree);
    assert_eq!(launch["env"]["CANBAN_BATCH_ID"], batch.id);
    let argv: Vec<String> = serde_json::from_value(launch["argv"].clone()).unwrap();
    let settings_idx = argv.iter().position(|a| a == "--settings").unwrap();
    let settings: Value = serde_json::from_str(&argv[settings_idx + 1]).unwrap();
    let trusted: Vec<&str> = settings["trustedWorkspaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let batch_run_dir = env.app.home.join("terminals").join(&batch.id);
    let project_dir = PathBuf::from(&env.project.directory);
    for expected_dir in [&merge_wt, &project_dir, &batch_run_dir, &wt1, &wt2, &wt3] {
        let canon = std::fs::canonicalize(expected_dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            trusted.contains(&canon.as_str()),
            "agy trustedWorkspaces missing {canon}; got {trusted:?}"
        );
    }
    let add_dirs: Vec<&str> = argv
        .windows(2)
        .filter_map(|w| (w[0] == "--add-dir").then_some(w[1].as_str()))
        .collect();
    for expected_dir in [&merge_wt, &project_dir, &batch_run_dir, &wt1, &wt2, &wt3] {
        let dir_str = expected_dir.to_str().unwrap();
        assert!(
            add_dirs.contains(&dir_str),
            "agy --add-dir missing {dir_str}; got {add_dirs:?}"
        );
    }

    // Simulate `@merger` resolving Card 2's conflict (in cherry-pick state) and integrating Card 3 in `_merge_queue`.
    git_identity(&merge_wt);
    std::fs::write(merge_wt.join("shared.txt"), "from card 1 + from card 2\n").unwrap();
    git_in(&merge_wt, &["add", "shared.txt"]);
    git_in(&merge_wt, &["commit", "--no-edit"]);
    let card3_ref = format!("refs/canban/batch/{}/card-{}", batch.short_id(), c3.number);
    git_in(&merge_wt, &["cherry-pick", &card3_ref]);

    let report_res = env
        .app
        .call(
            "batch.report",
            json!({
                "batch_id": batch.id,
                "state": "completed",
                "message": "Resolved conflict and integrated Card #3",
                "request_key": "batch-conflict-resolved"
            }),
        )
        .unwrap();
    assert_eq!(report_res["delivered"], 3);

    let snap_after = env.app.snapshot().unwrap();
    for c in [&c1, &c2, &c3] {
        assert!(
            snap_after
                .cards
                .iter()
                .find(|x| x.id == c.id)
                .unwrap()
                .delivery
                .is_some(),
            "Card #{} must be delivered after batch completion",
            c.number
        );
    }
    assert_eq!(
        std::fs::read_to_string(env.source.join("shared.txt")).unwrap(),
        "from card 1 + from card 2\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.source.join("card3.txt")).unwrap(),
        "from card 3\n"
    );
    assert_eq!(
        env.push_count(),
        1,
        "Resolved batch must push upstream once"
    );
    assert_eq!(
        snap_after
            .merge_batches
            .iter()
            .find(|b| b.id == batch.id)
            .unwrap()
            .state,
        "completed"
    );
}

#[test]
fn batch_merge_queue_test_failure_supports_evicting_bad_card_and_delivering_remaining_batch() {
    let env = BatchQueueTestHarness::new(Some("test ! -f bad.flag"), false);
    let (c1, _) = env.finish_card_in_develop("Batch ok 1", "ok1.txt", "ok 1\n");
    let (c2, _) = env.finish_card_in_develop("Batch bad 2", "bad.flag", "broken\n");
    let (c3, _) = env.finish_card_in_develop("Batch ok 3", "ok3.txt", "ok 3\n");

    super::terminal_cases::core(&env.app, env.dir.path());
    env.release_and_pump(3);
    env.app.tick().unwrap();

    let snap = env.app.snapshot().unwrap();
    assert_eq!(snap.merge_batches.len(), 1);
    let batch = &snap.merge_batches[0];
    assert_eq!(batch.state, "running");
    assert_eq!(batch.failure_kind.as_deref(), Some("test_failure"));
    assert_eq!(
        env.push_count(),
        0,
        "Failing batch test must block delivery and push"
    );

    // Simulate `@merger` resetting `_merge_queue` to target, cherry-picking Cards 1 & 3, and evicting Card 2.
    let merge_wt = PathBuf::from(&batch.worktree);
    let short_id = batch.short_id();
    let target_ref = format!("refs/canban/batch/{short_id}/target");
    let c1_ref = format!("refs/canban/batch/{short_id}/card-{}", c1.number);
    let c3_ref = format!("refs/canban/batch/{short_id}/card-{}", c3.number);
    git_identity(&merge_wt);
    git_in(&merge_wt, &["reset", "--hard", &target_ref]);
    git_in(&merge_wt, &["cherry-pick", &c1_ref]);
    git_in(&merge_wt, &["cherry-pick", &c3_ref]);

    let res = env
        .app
        .call(
            "batch.report",
            json!({
                "batch_id": batch.id,
                "state": "completed",
                "evict": [c2.number],
                "message": "Evicted Card #2 due to test failure",
                "request_key": "batch-evict-c2"
            }),
        )
        .unwrap();
    assert_eq!(res["delivered"], 2);
    assert_eq!(res["evicted"], 1);

    let snap_after = env.app.snapshot().unwrap();
    for good in [&c1, &c3] {
        assert!(
            snap_after
                .cards
                .iter()
                .find(|x| x.id == good.id)
                .unwrap()
                .delivery
                .is_some()
        );
    }
    let bad_card = snap_after.cards.iter().find(|x| x.id == c2.id).unwrap();
    assert!(bad_card.delivery.is_none());
    assert!(bad_card.human_gate.is_some());
    assert!(bad_card.attention.is_some());
    let bad_run = snap_after
        .workflow_runs
        .iter()
        .find(|r| r.card_id == c2.id && r.active())
        .unwrap();
    assert_eq!(bad_run.state, "waiting_approval");
    assert!(
        snap_after
            .events
            .iter()
            .any(|e| e.card_id == c2.id && e.kind == "review.batch_evicted"),
        "Evicted Card must record review.batch_evicted event"
    );
    assert!(env.source.join("ok1.txt").exists());
    assert!(env.source.join("ok3.txt").exists());
    assert!(!env.source.join("bad.flag").exists());
    assert_eq!(env.push_count(), 1);
    let finished_batch = snap_after
        .merge_batches
        .iter()
        .find(|b| b.id == batch.id)
        .unwrap();
    assert_eq!(finished_batch.state, "completed");
    assert!(
        finished_batch
            .items
            .iter()
            .find(|i| i.card_id == c2.id)
            .unwrap()
            .evicted
    );
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
        git_in(&remote, &["init", "--bare"]);
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

    // 2. Repo A's own main checkout `git.pull` and `git.commit` fail with clear contextual error.
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

    // 5. Releasing Repo A's gate allows `git.pull` on Repo A to proceed immediately.
    drop(guard_a);
    app.call(
        "git.pull",
        json!({"project_id": proj_a.id, "request_key": "pull-a-after-release"}),
    )
    .expect("git.pull on Repo A should succeed once delivery gate is released");
    assert_eq!(
        std::fs::read_to_string(repo_a.join("upstream-a.txt")).unwrap(),
        "from upstream\n"
    );
}

#[test]
fn batch_delivery_releases_main_checkout_gate_during_test_command_and_retries_target_advanced() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-b", "main"]);
    git_identity(&source);
    git_in(&source, &["config", "core.hooksPath", "/dev/null"]);
    std::fs::write(source.join("README.md"), "base\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "initial"]);

    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let proj: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Batch Concurrent Checkout",
                "directory": source,
                "target_branch": "main",
                "workspace": {"create_worktree": true, "sync_base": false}
            }),
        )
        .unwrap(),
    )
    .unwrap();

    let started_flag = dir.path().join("batch-test-started.flag");
    let continue_flag = dir.path().join("main-commit-done.flag");
    let test_cmd = format!(
        "touch '{}' && i=0; while [ ! -f '{}' ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done; test -f '{}'",
        started_flag.display(),
        continue_flag.display(),
        continue_flag.display(),
    );
    edit_development(&app, &proj, |doc| {
        canban_workflow_engine::source::set_review_manual(doc, false);
        canban_workflow_engine::source::set_sync_upstream(doc, false);
        canban_workflow_engine::stages::set_step_with(
            doc,
            "deliver:review",
            "batch_size",
            Some(json!(2)),
        )
        .unwrap();
        canban_workflow_engine::stages::set_step_with(
            doc,
            "deliver:review",
            "test_command",
            Some(json!(test_cmd)),
        )
        .unwrap();
    });

    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": proj.id, "title": "Feature under test"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": card.id, "revision": card.revision, "stage": "develop"}),
    )
    .unwrap();
    let dev = admit(&app, &snap_latest_card_terminal(&app, &card.id));
    let ops = canban_workspaces::Ops::local();
    let wt = ops.prepare(&app.home, &proj, &card).unwrap();
    let br = ops.current_branch(wt.to_str().unwrap()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut stored: Card = db.get(&card.id)?;
            stored.worktree = Some(wt.to_str().unwrap().into());
            stored.branch = Some(br);
            db.save(&stored)
        })
        .unwrap();
    git_identity(&wt);
    git_in(&wt, &["config", "core.hooksPath", "/dev/null"]);
    std::fs::write(wt.join("feature.txt"), "from card\n").unwrap();
    git_in(&wt, &["add", "."]);
    git_in(&wt, &["commit", "-m", "feat: card work"]);
    finish_closed(&app, &dev, "dev-done-batch-concurrency");

    let app_bg = Arc::clone(&app);
    let pump_thread = std::thread::spawn(move || {
        app_bg.pump_deliveries().unwrap();
    });

    // Wait until `run_batch_tests` has actually started inside `_merge_queue`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !started_flag.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for batch test_command to start"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // While `run_batch_tests` is executing in `_merge_queue`, the main checkout gate MUST be
    // released so user `git.commit` on the SAME project's main checkout succeeds immediately.
    std::fs::write(
        source.join("main-concurrent.txt"),
        "committed during batch tests\n",
    )
    .unwrap();
    app.call(
        "git.stage",
        json!({"project_id": proj.id, "paths": ["main-concurrent.txt"]}),
    )
    .unwrap();
    app.call(
        "git.commit",
        json!({
            "project_id": proj.id,
            "message": "concurrent main commit",
            "request_key": "commit-during-batch-tests"
        }),
    )
    .expect("main checkout git.commit must succeed while batch test_command runs in _merge_queue");

    // Allow `run_batch_tests` to finish; `execute_batch_delivery` will see `TargetAdvanced`
    // and cleanly retry without overwriting the main checkout commit.
    std::fs::write(&continue_flag, "ok").unwrap();
    pump_thread.join().unwrap();

    let mid_snap = app.snapshot().unwrap();
    assert!(
        mid_snap
            .events
            .iter()
            .any(|e| e.card_id == card.id && e.kind == "review.merge_retry"),
        "expected TargetAdvanced retry log after concurrent main commit"
    );

    // Second pump re-integrates on top of the new main HEAD and delivers cleanly.
    app.pump_deliveries().unwrap();
    let final_snap = app.snapshot().unwrap();
    let final_card = final_snap.cards.iter().find(|c| c.id == card.id).unwrap();
    assert!(
        final_card.delivery.is_some(),
        "Card should deliver cleanly after TargetAdvanced retry"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("main-concurrent.txt")).unwrap(),
        "committed during batch tests\n"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("feature.txt")).unwrap(),
        "from card\n"
    );
}
