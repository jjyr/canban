use super::*;

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
fn tcp_first_frame_accepts_payloads_over_64_kib() {
    let (_dir, app, _project, card) = setup();
    let (_, token) = app.issue_device_token("repr-test").unwrap();
    let _running = app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
            trust_tailscale_self: true,
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
