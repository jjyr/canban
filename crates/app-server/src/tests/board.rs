use super::*;
fn peer(app: &AppServer, project: &Project, title: &str) -> Card {
    serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id":project.id,"title":title}),
        )
        .unwrap(),
    )
    .unwrap()
}
fn order(app: &AppServer, stage: &str) -> Vec<String> {
    let mut cards = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .filter(|c| c.stage == stage && c.active())
        .collect::<Vec<_>>();
    cards.sort_by(Card::board_cmp);
    cards.into_iter().map(|c| c.id).collect()
}
#[test]
fn board_reorder_is_durable_idempotent_and_preserves_human_gates() {
    let (dir, app, p, a) = setup();
    let b = peer(&app, &p, "B");
    let mut c = peer(&app, &p, "C");
    c.human_gate = Some(HumanGate {
        reason: "Review first".into(),
        attempt_id: None,
        next_stage: None,
    });
    app.store.lock().unwrap().write(|db| db.save(&c)).unwrap();
    let args = json!({"card_id":c.id,"revision":1,"stage":"backlog","before_card_id":b.id,"request_key":"reorder-c"});
    let moved = app.call("card.move", args.clone()).unwrap();
    assert_eq!(
        order(&app, "backlog"),
        vec![a.id.clone(), c.id.clone(), b.id.clone()]
    );
    assert_eq!(app.call("card.move", args).unwrap(), moved);
    assert!(app.snapshot().unwrap().attempts.is_empty());
    assert_eq!(
        app.snapshot()
            .unwrap()
            .cards
            .iter()
            .find(|x| x.id == c.id)
            .unwrap()
            .human_gate,
        c.human_gate
    );
    assert_eq!(
        app.snapshot()
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind == "card.order_changed")
            .count(),
        1
    );
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":2,"stage":"backlog","before_card_id":b.id}),
    )
    .unwrap();
    assert_eq!(
        app.snapshot()
            .unwrap()
            .cards
            .iter()
            .find(|x| x.id == c.id)
            .unwrap()
            .revision,
        2
    );
    drop(app);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(order(&app, "backlog"), vec![a.id, c.id, b.id]);
}
#[test]
fn cross_column_drop_persists_slot_and_dispatches_exactly_once() {
    let (_dir, app, p, a) = setup();
    let b = peer(&app, &p, "B");
    app.call(
        "card.move",
        json!({"card_id":b.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let args = json!({"card_id":a.id,"revision":1,"stage":"develop","before_card_id":b.id,"request_key":"drop-a"});
    app.call("card.move", args.clone()).unwrap();
    app.call("card.move", args).unwrap();
    assert_eq!(order(&app, "develop"), vec![a.id.clone(), b.id]);
    assert_eq!(
        app.snapshot()
            .unwrap()
            .attempts
            .iter()
            .filter(|r| r.card_id == a.id)
            .count(),
        1
    );
    let saved = app.snapshot().unwrap();
    assert!(
        app.call(
            "card.move",
            json!({"card_id":a.id,"revision":1,"stage":"develop","before_card_id":null})
        )
        .is_err()
    );
    assert!(
        app.call(
            "card.move",
            json!({"card_id":a.id,"revision":2,"stage":"backlog","before_card_id":"missing"})
        )
        .is_err()
    );
    assert_eq!(
        serde_json::to_value(app.snapshot().unwrap()).unwrap(),
        serde_json::to_value(saved).unwrap()
    );
}
#[test]
fn exhausted_ranks_compact_without_changing_peer_revisions_or_relative_order() {
    let (_dir, app, p, mut a) = setup();
    let mut b = peer(&app, &p, "B");
    let c = peer(&app, &p, "C");
    a.board_order = Some(0);
    b.board_order = Some(1);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&a)?;
            db.save(&b)
        })
        .unwrap();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"backlog","before_card_id":b.id}),
    )
    .unwrap();
    assert_eq!(
        order(&app, "backlog"),
        vec![a.id.clone(), c.id, b.id.clone()]
    );
    assert!(
        app.snapshot()
            .unwrap()
            .cards
            .iter()
            .filter(|c| c.id == a.id || c.id == b.id)
            .all(|c| c.revision == 1)
    );
}

#[test]
fn a_new_card_appends_after_existing_custom_order() {
    let (_dir, app, p, mut a) = setup();
    a.board_order = Some(1_000_000);
    app.store.lock().unwrap().write(|db| db.save(&a)).unwrap();
    let b = peer(&app, &p, "New Card");
    assert_eq!(order(&app, "backlog"), vec![a.id, b.id]);
}
