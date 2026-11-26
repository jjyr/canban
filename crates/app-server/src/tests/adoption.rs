use super::*;

fn manual() -> (tempfile::TempDir, Arc<AppServer>, Card, TerminalEntry) {
    let (dir, app, project, card) = setup();
    std::fs::create_dir_all(&project.directory).unwrap();
    super::terminal_cases::core(&app, dir.path());
    let terminal = serde_json::from_value(
        app.call(
            "terminal.create",
            json!({
                "card_id":card.id,"agent":true,"profile_id":"codex","name":"My session"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let card = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    (dir, app, card, terminal)
}

fn adopt(card: &Card) -> Value {
    json!({"view":"full","card_id":card.id,"profile_id":"codex","cwd":card.worktree,
        "runtime_terminal":"t1","runtime_session":"default","request_key":"adopt-once"})
}

#[test]
fn adopt_can_bind_after_an_earlier_workflow_only_adoption() {
    let (dir, app, card, _) = manual();
    let first = app.call("card.adopt", json!({"view":"full","card_id":card.id,"profile_id":"codex","request_key":"without-terminal"})).unwrap();
    assert!(first["workflow_run"]["terminal_id"].is_null());
    let second = app.call("card.adopt", adopt(&card)).unwrap();
    assert_eq!(first["workflow_run"]["id"], second["workflow_run"]["id"]);
    assert_eq!(
        second["workflow_run"]["terminal_id"],
        Conversation::key(&card.id, "codex")
    );
    let mut again = adopt(&card);
    again["request_key"] = json!("again");
    app.call("card.adopt", again).unwrap();
    assert_eq!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .filter(|t| t.agent.is_some())
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn adopt_without_a_usable_terminal_keeps_the_workflow_and_manual_runtime() {
    for failure in [
        "missing",
        "unknown",
        "session",
        "cwd",
        "profile",
        "core",
        "ambiguous",
    ] {
        let (dir, app, card, source) = manual();
        let mut params = adopt(&card);
        match failure {
            "missing" => {
                params.as_object_mut().unwrap().remove("runtime_terminal");
            }
            "unknown" => params["runtime_terminal"] = json!("t404"),
            "session" => params["runtime_session"] = json!("another-core"),
            "cwd" => params["cwd"] = json!(dir.path()),
            "profile" => params["profile_id"] = json!("claude"),
            "core" => std::fs::write(dir.path().join("replace"), "").unwrap(),
            "ambiguous" => {
                let mut other = source.clone();
                other.id = "duplicate".into();
                other.number += 100;
                app.store
                    .lock()
                    .unwrap()
                    .write(|db| db.save(&other))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let adopted = app.call("card.adopt", params).unwrap();
        assert_eq!(adopted["card"]["stage"], "develop", "{failure}");
        assert!(
            adopted["workflow_run"]["terminal_id"].is_null(),
            "{failure}"
        );
        let s = app.snapshot().unwrap();
        assert!(s.terminals.iter().all(|t| t.agent.is_none()), "{failure}");
        assert_eq!(
            s.terminals
                .iter()
                .find(|t| t.id == source.id)
                .unwrap()
                .runtime,
            source.runtime,
            "{failure}"
        );
        assert!(s.conversations.is_empty(), "{failure}");
    }
}

#[test]
fn adopt_binding_savepoint_preserves_an_existing_workflow_owner() {
    let (_dir, app, card, source) = manual();
    let result = app
        .call("card.adopt", json!({"view":"full","card_id":card.id}))
        .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut workflow: WorkflowRun =
                db.get(result["workflow_run"]["id"].as_str().unwrap())?;
            workflow.terminal_id = Some("another-owner".into());
            db.save(&workflow)
        })
        .unwrap();
    let result = app.call("card.adopt", adopt(&card)).unwrap();
    assert_eq!(result["workflow_run"]["terminal_id"], "another-owner");
    let s = app.snapshot().unwrap();
    assert!(s.conversations.is_empty());
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.terminals[0].runtime, source.runtime);
}
