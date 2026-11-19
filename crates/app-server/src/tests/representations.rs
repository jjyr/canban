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
        assert_eq!(receipt["card_revision"], card.revision);
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
    let stored = app.events().unwrap();
    let comments = stored
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
    let before = app.events().unwrap().len();
    assert!(
        app.call(
            "card.move",
            json!({"card_id":card.id,"revision":1,"stage":"develop"})
        )
        .is_err()
    );
    assert_eq!(app.events().unwrap().len(), before);
    let receipt = app
        .call(
            "card.move",
            json!({"card_id":card.id,"revision":2,"stage":"backlog","request_key":"same-column"}),
        )
        .unwrap();
    assert_eq!(receipt["stage"], "backlog");
    assert!(receipt.get("body").is_none());
}

#[test]
fn timeline_list_supports_tail_and_before_sequence_paging() {
    let (_dir, app, _project, card) = setup();
    for i in 1..=7 {
        app.call(
            "comment.add",
            json!({"card_id": card.id, "body": format!("msg-{i}")}),
        )
        .unwrap();
    }
    // Tail page returns the latest 3 events in ascending sequence order.
    let tail = app
        .call(
            "timeline.list",
            json!({"card_id": card.id, "tail": true, "limit": 3, "view": "full"}),
        )
        .unwrap();
    let tail_events = tail["events"].as_array().unwrap();
    assert_eq!(tail_events.len(), 3);
    assert_eq!(tail["has_more_before"], true);
    let seqs: Vec<u64> = tail_events
        .iter()
        .map(|e| e["sequence"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(tail["oldest_sequence"], json!(seqs[0]));
    assert_eq!(tail["highwater"], json!(seqs[2]));

    // Page backwards with before_sequence.
    let older = app
        .call(
            "timeline.list",
            json!({"card_id": card.id, "before_sequence": seqs[0], "limit": 3, "view": "full"}),
        )
        .unwrap();
    let older_events = older["events"].as_array().unwrap();
    assert_eq!(older_events.len(), 3);
    let older_seqs: Vec<u64> = older_events
        .iter()
        .map(|e| e["sequence"].as_u64().unwrap())
        .collect();
    assert!(older_seqs.windows(2).all(|w| w[0] < w[1]));
    assert!(older_seqs[2] < seqs[0]);
    assert_eq!(older["oldest_sequence"], json!(older_seqs[0]));
    assert_eq!(older["has_more_before"], true);

    // Final older page reaches the start (card.created + first comment = 2 events left).
    let earliest = app
        .call(
            "timeline.list",
            json!({"card_id": card.id, "before_sequence": older_seqs[0], "limit": 10, "view": "full"}),
        )
        .unwrap();
    assert_eq!(earliest["has_more_before"], false);
    assert_eq!(earliest["oldest_sequence"], json!(1));
}

#[test]
fn insights_events_filters_kinds_strips_message_and_data_and_skips_cardless() {
    let (_dir, app, project, card) = setup();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            // Card-bound insights event with extra data keys and message.
            db.append(canban_timeline::event(
                &project.id,
                &card.id,
                "execution.reported",
                "agent:developer",
                "Long secret report prose",
                json!({
                    "state": "completed",
                    "terminal_id": "t-1",
                    "run_number": 2,
                    "extra_secret": "drop-me",
                    "path": "/tmp/file"
                }),
            ))?;
            // Card-bound non-insights event (should be excluded).
            db.append(canban_timeline::event(
                &project.id,
                &card.id,
                "card.updated",
                "user",
                "Updated title",
                json!({"from": "a", "to": "b"}),
            ))?;
            // Cardless event (should be excluded).
            db.append(canban_timeline::event(
                &project.id,
                "",
                "github.issue.opened",
                "github",
                "Cardless event",
                json!({"origin": "github"}),
            ))?;
            Ok(())
        })
        .unwrap();

    let res = app
        .call("insights.events", json!({"project_id": project.id}))
        .unwrap();
    let events: Vec<Event> = serde_json::from_value(res["events"].clone()).unwrap();
    assert!(events.iter().all(|e| !e.card_id.is_empty()));
    assert!(events.iter().all(|e| e.message.is_empty()));
    assert!(!events.iter().any(|e| e.kind == "card.updated"));
    assert!(!events.iter().any(|e| e.kind == "github.issue.opened"));

    let reported = events
        .iter()
        .find(|e| e.kind == "execution.reported")
        .expect("execution.reported must be returned");
    assert_eq!(reported.data["state"], json!("completed"));
    assert_eq!(reported.data["terminal_id"], json!("t-1"));
    assert_eq!(reported.data["run_number"], json!(2));
    assert!(reported.data.get("extra_secret").is_none());
    assert!(reported.data.get("path").is_none());
}

#[test]
fn prune_cardless_events_preserves_highwater_pending_deliveries_and_card_events() {
    let (_dir, app, project, card) = setup();
    let (ev_old_id, ev_pending_id, ev_recent_id) = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            let mut old = canban_timeline::event(
                &project.id,
                "",
                "github.issue.opened",
                "github",
                "Old cardless",
                json!({}),
            );
            old.created_at = 1; // ancient
            let old = db.append(old)?;

            let mut pending_ev = canban_timeline::event(
                &project.id,
                "",
                "github.issue.opened",
                "github",
                "Pending delivery cardless",
                json!({}),
            );
            pending_ev.created_at = 1; // ancient, but has pending delivery
            let pending_ev = db.append(pending_ev)?;
            db.save(&EventDelivery {
                id: format!("sub1:{}", pending_ev.id),
                subscription_id: "sub1".into(),
                subscription_revision: 1,
                event_id: pending_ev.id.clone(),
                offset: 2,
                selector: EventSelector::default(),
                target: SubscriptionTarget::Workflow {
                    workflow_id: "wf".into(),
                    entry: "main".into(),
                    card_id: None,
                },
                definition_revision: None,
                state: "pending".into(),
                attempts: 0,
                next_attempt_at: 0,
                error: None,
                workflow_run_id: None,
            })?;

            let recent = db.append(canban_timeline::event(
                &project.id,
                "",
                "github.issue.opened",
                "github",
                "Recent cardless",
                json!({}),
            ))?;
            Ok((old.id, pending_ev.id, recent.id))
        })
        .unwrap();

    let hw_before = crate::events::highwater(app.store.lock().unwrap().db()).unwrap();
    let pruned = app
        .store
        .lock()
        .unwrap()
        .write(|db| db.prune_cardless_events(60_000, 100, hw_before))
        .unwrap();
    assert_eq!(
        pruned, 1,
        "only the old consumed event without pending delivery is pruned"
    );

    let hw_after = crate::events::highwater(app.store.lock().unwrap().db()).unwrap();
    assert!(
        hw_after >= hw_before,
        "highwater must never decrease after pruning ({hw_after} < {hw_before})"
    );

    let remaining = app.events().unwrap();
    assert!(!remaining.iter().any(|e| e.id == ev_old_id));
    assert!(remaining.iter().any(|e| e.id == ev_pending_id));
    assert!(remaining.iter().any(|e| e.id == ev_recent_id));
    assert!(remaining.iter().any(|e| e.card_id == card.id));
}

#[test]
fn tcp_first_frame_accepts_payloads_over_64_kib() {
    let (_dir, app, _project, card) = setup();
    let token = crate::network::ensure_server_token(&app.home).unwrap();
    let _running = app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
        }))
        .unwrap();
    let addr = app.bound_tcp_addrs()[0].clone();
    let large_body = "x".repeat(100 * 1024);
    let reply = crate::client::rpc_tcp(
        &addr,
        Some(&token),
        "card.update",
        json!({
            "card_id": card.id,
            "revision": card.revision,
            "body": large_body,
            "view": "full",
        }),
    )
    .expect("first TCP RPC > 64 KiB must succeed without truncation");
    assert_eq!(reply["body"].as_str().unwrap().len(), 100 * 1024);
}
