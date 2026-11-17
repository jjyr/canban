use super::*;

#[test]
fn agent_defaults_and_full_views_share_one_idempotent_write() {
    let (_dir, app, project, card) = setup();
    let summary = app.call("card.get", json!({"card_id":card.id})).unwrap();
    assert_eq!(summary.as_object().unwrap().len(), 5);
    assert!(summary.get("body").is_none());
    let chosen = app
        .call(
            "card.get",
            json!({"card_id":card.id,"fields":["body","number","archive"]}),
        )
        .unwrap();
    assert_eq!(
        chosen,
        json!({"body":card.body,"number":card.number,"archive":null})
    );
    let p = json!({"project_id":project.id,"title":"A new card","body":"large unique body","request_key":"one-card"});
    let receipt = app.call("card.create", p.clone()).unwrap();
    assert_eq!(receipt["accepted"], true);
    assert!(receipt.get("body").is_none());
    let mut full = p.clone();
    full["view"] = json!("full");
    let complete = app.call("card.create", full).unwrap();
    assert_eq!(complete["id"], receipt["id"]);
    assert_eq!(complete["body"], "large unique body");
    assert_eq!(app.snapshot().unwrap().cards.len(), 2);
    assert_eq!(receipt["request_key"], "one-card");
    let mut invalid = p;
    invalid["request_key"] = json!("invalid-must-not-write");
    invalid["fields"] = json!(["boddy"]);
    assert!(
        app.call("card.create", invalid)
            .unwrap_err()
            .to_string()
            .contains("unknown_field")
    );
    assert_eq!(app.snapshot().unwrap().cards.len(), 2);
}

#[test]
fn card_pages_bind_filters_and_detect_concurrent_changes() {
    let (_dir, app, project, card) = setup();
    for n in 0..4 {
        app.call(
            "card.create",
            json!({"project_id":project.id,"title":format!("Card {n}"),"body":"not in summary"}),
        )
        .unwrap();
    }
    let mut p = json!({"project_id":project.id,"stage":"backlog","limit":2});
    let first = app.call("card.list", p.clone()).unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert!(first["items"][0].get("body").is_none());
    let mut seen = Vec::new();
    let mut page = first.clone();
    loop {
        seen.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap().to_owned()),
        );
        if page["next_cursor"].is_null() {
            break;
        }
        p["cursor"] = page["next_cursor"].clone();
        page = app.call("card.list", p.clone()).unwrap();
    }
    assert_eq!(seen.len(), 5);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 5);
    p["cursor"] = first["next_cursor"].clone();
    p["stage"] = json!("develop");
    assert!(
        app.call("card.list", p.clone())
            .unwrap_err()
            .to_string()
            .contains("cursor_scope_mismatch")
    );
    p["stage"] = json!("backlog");
    app.call("card.update", json!({"card_id":card.id,"revision":card.revision,"title":"Changed","request_key":"edit-during-page"})).unwrap();
    assert!(
        app.call("card.list", p)
            .unwrap_err()
            .to_string()
            .contains("cursor_stale")
    );
    let empty = app
        .call(
            "card.list",
            json!({"project_id":project.id,"stage":"develop","limit":1}),
        )
        .unwrap();
    assert_eq!(empty["items"], json!([]));
    assert!(empty["next_cursor"].is_null());
    assert!(app.call("card.list", json!({"limit":0})).is_err());
    let full = app
        .call(
            "card.list",
            json!({"project_id":project.id,"view":"full","limit":1}),
        )
        .unwrap();
    assert_eq!(full["items"][0]["body"], card.body);
}

#[test]
fn comment_views_filter_before_paging_and_do_not_rewrite_stored_events() {
    let (_dir, app, _project, card) = setup();
    for n in 0..5 {
        let p = json!({"card_id":card.id,"body":format!("unique comment {n}"),"mentions":[],"request_key":format!("comment-{n}")});
        let receipt = app.call("comment.add", p.clone()).unwrap();
        assert!(receipt.get("body").is_none());
        assert!(receipt.get("message").is_none());
        assert_eq!(receipt["card_revision"], n + 2);
        let mut full = p;
        full["view"] = json!("full");
        let response = app.call("comment.add", full).unwrap();
        assert_eq!(response["id"], receipt["id"]);
        assert_eq!(response["body"], format!("unique comment {n}"));
        assert!(response.get("message").is_none());
        assert!(response["data"].get("body").is_none());
        assert_eq!(
            serde_json::to_string(&response)
                .unwrap()
                .matches(&format!("unique comment {n}"))
                .count(),
            1
        );
        let typed: Event = serde_json::from_value(response).unwrap();
        assert_eq!(typed.message, format!("unique comment {n}"));
    }
    let mut after = 0;
    let mut seen = Vec::new();
    loop {
        let page = app.call("timeline.list", json!({"card_id":card.id,"kinds":["comment.created"],"after_sequence":after,"limit":2})).unwrap();
        for event in page["events"].as_array().unwrap() {
            assert_eq!(event["kind"], "comment.created");
            assert!(event.get("message").is_none());
            seen.push(event["id"].clone());
        }
        after = page["last_sequence"].as_u64().unwrap();
        if page["next_cursor"].is_null() {
            break;
        }
        assert_eq!(page["next_cursor"], after);
    }
    assert_eq!(seen.len(), 5);
    let empty = app
        .call(
            "timeline.list",
            json!({"card_id":card.id,"kinds":["never.matches"]}),
        )
        .unwrap();
    assert_eq!(empty["events"], json!([]));
    assert_eq!(empty["last_sequence"], after);
    let stored = app.snapshot().unwrap();
    let comments = stored
        .events
        .iter()
        .filter(|e| e.kind == "comment.created")
        .collect::<Vec<_>>();
    assert_eq!(comments.len(), 5);
    assert!(
        comments
            .iter()
            .all(|e| e.message == e.data["body"].as_str().unwrap())
    );
    let selected = app
        .call(
            "timeline.list",
            json!({"card_id":card.id,"fields":["id"],"limit":1}),
        )
        .unwrap();
    assert!(selected["last_sequence"].as_u64().is_some());
    assert_eq!(selected["events"][0].as_object().unwrap().len(), 1);
}

#[test]
fn revision_conflicts_and_legacy_operation_replays_survive_projection() {
    let (_dir, app, _project, card) = setup();
    let p = json!({"card_id":card.id,"revision":card.revision,"title":"Updated","request_key":"legacy"});
    // Simulate an operations record written before response options existed.
    let old = app
        .mutate("card.update", &p, |db| {
            let mut c: Card = db.get(&card.id)?;
            c.title = "Updated".into();
            c.revision += 1;
            db.save(&c)?;
            Ok(serde_json::to_value(c)?)
        })
        .unwrap();
    let mut retry = p.clone();
    retry["fields"] = json!(["body"]);
    let receipt = app.call("card.update", retry).unwrap();
    assert_eq!(receipt["id"], old["id"]);
    assert_eq!(receipt["revision"], 2);
    assert_eq!(receipt["body"], card.body);
    let before = app.snapshot().unwrap().events.len();
    assert!(
        app.call(
            "card.move",
            json!({"card_id":card.id,"revision":1,"stage":"develop"})
        )
        .is_err()
    );
    assert_eq!(app.snapshot().unwrap().events.len(), before);
    let receipt = app
        .call(
            "card.move",
            json!({"card_id":card.id,"revision":2,"stage":"backlog","request_key":"same-column"}),
        )
        .unwrap();
    assert_eq!(receipt["stage"], "backlog");
    assert!(receipt.get("body").is_none());
}
