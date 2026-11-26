use super::*;

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
        .write(|db| db.prune_cardless_events(60_000, 100, hw_before, app.now_ms()))
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

#[test]
fn activity_record_and_insights_activity_validate_clip_and_include_open_agent_work() {
    let (_dir, app, project, card) = setup();
    let now_ts = canban_types::now();
    let base = now_ts - 3_600_000;

    // Validation rejections for activity.record
    let too_many: Vec<Value> = (0..201)
        .map(|i| json!({"start_ms": base + i, "end_ms": base + i + 100, "lane": "other", "page": "board"}))
        .collect();
    assert!(
        app.call("activity.record", json!({"segments": too_many}))
            .is_err()
    );
    assert!(
        app.call(
            "activity.record",
            json!({"segments":[{"start_ms": base + 500, "end_ms": base + 500, "lane": "card", "page": "card"}]})
        )
        .is_err()
    );
    assert!(
        app.call(
            "activity.record",
            json!({"segments":[{"start_ms": base, "end_ms": base + 12 * 3_600_000 + 1, "lane": "card", "page": "card"}]})
        )
        .is_err()
    );
    assert!(
        app.call(
            "activity.record",
            json!({"segments":[{"start_ms": now_ts, "end_ms": now_ts + 120_000, "lane": "card", "page": "card"}]})
        )
        .is_err()
    );
    assert!(
        app.call(
            "activity.record",
            json!({"segments":[{"start_ms": base, "end_ms": base + 10_000, "lane": "bogus", "page": "card"}]})
        )
        .is_err()
    );

    // Valid dwell segments
    let rec = app
        .call(
            "activity.record",
            json!({
                "segments": [
                    {
                        "start_ms": base,
                        "end_ms": base + 60_000,
                        "lane": "card",
                        "page": "card",
                        "project_id": project.id,
                        "card_id": card.id,
                    },
                    {
                        "start_ms": base + 70_000,
                        "end_ms": base + 130_000,
                        "lane": "terminal",
                        "page": "terminals",
                        "project_id": project.id,
                        "terminal_id": "term-1",
                    }
                ]
            }),
        )
        .unwrap();
    assert_eq!(rec["recorded"], 2);

    // Attach an AgentRun and an open working entry in Inventory
    let mut profile = app.config().profiles[0].clone();
    profile.id = "developer".into();
    profile.preset = Some("claude".into());
    profile.model = Some("claude-sonnet-4".into());
    let run = AgentRun {
        workflow_run_id: None,
        workflow_step: None,
        resume_session: false,
        trigger: ExecutionTrigger::Workflow,
        stage: "develop".into(),
        profile,
        instruction: "go".into(),
        on_completed: None,
        require_human_review: false,
        workspace: None,
        directory: None,
        lifecycle: Default::default(),
        project_id: project.id.clone(),
        card_id: card.id.clone(),
        card_number: card.number,
        number: 1,
        state: "running".into(),
        core_id: Some("core-1".into()),
        terminal: Some("t-agent".into()),
        cwd: "/tmp".into(),
        input_path: String::new(),
        output_path: None,
        observed: None,
        error: None,
        started_at: Some(base + 10_000),
        updated_at: base + 10_000,
        provider_session_id: None,
        close_state: String::new(),
        turn_ended_at: None,
        response_path: None,
        completed_at: None,
        completion_turn: None,
        close_requested_at: None,
        connection_state: "connected".into(),
        pending: None,
        applied_env: Default::default(),
        applied_env_fingerprint: String::new(),
    };
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&TerminalEntry {
                id: "term-agent-1".into(),
                conversation_id: Some("conv-1".into()),
                number: 1,
                name: None,
                origin_project_id: Some(project.id.clone()),
                origin_card_id: Some(card.id.clone()),
                agent_engine: None,
                runtime: None,
                agent: Some(run),
                created_at: base + 10_000,
            })?;
            db.save(&TerminalEntry {
                id: "term-manual-1".into(),
                conversation_id: None,
                number: 2,
                name: Some("Manual".into()),
                origin_project_id: Some(project.id.clone()),
                origin_card_id: None,
                agent_engine: Some("codex".into()),
                runtime: Some(TerminalRuntime {
                    core_id: Some("core-1".into()),
                    terminal: Some("t-manual".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: Some("working".into()),
                    agent_turn: None,
                    observed_agent: Some("codex".into()),
                    waiting: None,
                }),
                agent: None,
                created_at: base + 20_000,
            })
        })
        .unwrap();

    {
        let mut inv = app.inventory.lock().unwrap();
        inv.core = "core-1".into();
        inv.snapshot_at(
            &json!({
                "seq": 1,
                "terminals": [
                    {"address": "t-agent", "state": "live", "status": {"state": "working"}},
                    {"address": "t-manual", "state": "live", "status": {"state": "working"}}
                ]
            }),
            base + 200_000,
        );
        inv.apply_at(
            &json!({"method":"event","params":{"seq":2,"type":"status","terminal":"t-manual","data":{"status":{"state":"idle"}}}}),
            base + 210_000,
        );
    }
    app.flush_agent_work().unwrap();

    // Reject > 8 days range
    assert!(
        app.call(
            "insights.activity",
            json!({"from_ms": base, "to_ms": base + 9 * 24 * 3_600_000})
        )
        .is_err()
    );

    // Clipping test: [base + 30_000, base + 250_000]
    let res = app
        .call(
            "insights.activity",
            json!({"from_ms": base + 30_000, "to_ms": base + 250_000, "project_id": project.id}),
        )
        .unwrap();
    let segs: Vec<ActivitySegment> = serde_json::from_value(res["segments"].clone()).unwrap();
    assert_eq!(segs.len(), 4);
    assert_eq!(segs[0].kind, ActivityKind::Dwell);
    assert_eq!(segs[0].start_ms, base + 30_000);
    assert_eq!(segs[0].end_ms, base + 60_000);
    assert_eq!(segs[1].kind, ActivityKind::Dwell);
    assert_eq!(segs[1].start_ms, base + 70_000);
    assert_eq!(segs[1].end_ms, base + 130_000);

    let open_agent = segs
        .iter()
        .find(|s| s.open)
        .expect("open working agent must appear");
    let open_info = open_agent.agent_data().unwrap();
    assert_eq!(open_info.profile_id, "developer");
    assert_eq!(open_info.provider, "claude");
    assert_eq!(open_info.model, "claude-sonnet-4");
    assert!(!open_info.manual);

    let closed_manual = segs
        .iter()
        .find(|s| s.kind == ActivityKind::Agent && !s.open)
        .expect("closed manual agent must be persisted");
    assert_eq!(closed_manual.duration_ms(), 10_000);
    assert_eq!(closed_manual.agent_data().unwrap().profile_id, "manual");
    assert_eq!(closed_manual.agent_data().unwrap().provider, "codex");
    assert!(closed_manual.agent_data().unwrap().manual);

    // Bucket validation and math across segment split boundary
    assert!(
        app.call("insights.activity_buckets", json!({"boundaries": [base]}))
            .is_err()
    );
    assert!(
        app.call(
            "insights.activity_buckets",
            json!({"boundaries": [base + 100, base + 100]})
        )
        .is_err()
    );
    let b_res = app
        .call(
            "insights.activity_buckets",
            json!({
                "boundaries": [base, base + 30_000, base + 220_000],
                "project_id": project.id,
            }),
        )
        .unwrap();
    let buckets: Vec<ActivityBucket> = serde_json::from_value(b_res["buckets"].clone()).unwrap();
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0].dwell.card, 30_000);
    assert_eq!(buckets[0].dwell.terminal, 0);
    assert_eq!(buckets[1].dwell.card, 30_000);
    assert_eq!(buckets[1].dwell.terminal, 60_000);
    assert_eq!(buckets[1].agent_ms, 30_000); // 10_000 closed manual + 20_000 clipped open developer
    assert_eq!(
        buckets[1].by_project.get(&project.id).unwrap().dwell_ms,
        90_000
    );
    assert_eq!(
        buckets[1].by_project.get(&project.id).unwrap().agent_ms,
        30_000
    );
}

#[test]
fn insights_activity_and_activity_buckets_have_exact_parity_for_same_window() {
    use canban_types::{
        ActivityBucket, ActivityDetail, ActivityKind, ActivitySegment, AgentBucketStat,
        DwellBucketTotals, DwellLane, ProjectActivityTotals,
    };
    use std::collections::BTreeMap;

    let (_dir, app, project, card) = setup();
    let project2: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Second", "directory": _dir.path().join("second")}),
        )
        .unwrap(),
    )
    .unwrap();

    let now_ts = canban_types::now();
    let day_end = now_ts - 60_000;
    let day_start = day_end - 8 * 3_600_000;

    // Record dwell segments:
    // - straddling day_start (clipped to 90_000 in project)
    // - fully inside (180_000 in project2, terminal)
    // - straddling day_end (clipped to 45_000 unattributed other)
    // - fully outside (after day_end, excluded)
    app.call(
        "activity.record",
        json!({
            "segments": [
                {
                    "start_ms": day_start - 30_000,
                    "end_ms": day_start + 90_000,
                    "lane": "card",
                    "page": "card",
                    "project_id": project.id,
                    "card_id": card.id,
                },
                {
                    "start_ms": day_start + 300_000,
                    "end_ms": day_start + 480_000,
                    "lane": "terminal",
                    "page": "terminals",
                    "project_id": project2.id,
                    "terminal_id": "term-p2",
                },
                {
                    "start_ms": day_end - 45_000,
                    "end_ms": day_end + 30_000,
                    "lane": "other",
                    "page": "board",
                },
                {
                    "start_ms": day_end + 10_000,
                    "end_ms": day_end + 40_000,
                    "lane": "card",
                    "page": "card",
                    "project_id": project.id,
                    "card_id": card.id,
                },
            ]
        }),
    )
    .unwrap();

    // Seed one open AgentRun terminal (project) and one closed manual terminal (project2).
    let mut profile = app.config().profiles[0].clone();
    profile.id = "developer".into();
    profile.preset = Some("claude".into());
    profile.model = Some("claude-sonnet-4".into());
    let run = AgentRun {
        workflow_run_id: None,
        workflow_step: None,
        resume_session: false,
        trigger: ExecutionTrigger::Workflow,
        stage: "develop".into(),
        profile,
        instruction: "go".into(),
        on_completed: None,
        require_human_review: false,
        workspace: None,
        directory: None,
        lifecycle: Default::default(),
        project_id: project.id.clone(),
        card_id: card.id.clone(),
        card_number: card.number,
        number: 1,
        state: "running".into(),
        core_id: Some("core-1".into()),
        terminal: Some("t-open".into()),
        cwd: "/tmp".into(),
        input_path: String::new(),
        output_path: None,
        observed: None,
        error: None,
        started_at: Some(day_end - 120_000),
        updated_at: day_end - 120_000,
        provider_session_id: None,
        close_state: String::new(),
        turn_ended_at: None,
        response_path: None,
        completed_at: None,
        completion_turn: None,
        close_requested_at: None,
        connection_state: "connected".into(),
        pending: None,
        applied_env: Default::default(),
        applied_env_fingerprint: String::new(),
    };
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&TerminalEntry {
                id: "term-open-1".into(),
                conversation_id: Some("conv-open".into()),
                number: 1,
                name: None,
                origin_project_id: Some(project.id.clone()),
                origin_card_id: Some(card.id.clone()),
                agent_engine: None,
                runtime: None,
                agent: Some(run),
                created_at: day_end - 120_000,
            })?;
            db.save(&TerminalEntry {
                id: "term-closed-2".into(),
                conversation_id: None,
                number: 2,
                name: Some("Manual".into()),
                origin_project_id: Some(project2.id.clone()),
                origin_card_id: None,
                agent_engine: Some("codex".into()),
                runtime: Some(TerminalRuntime {
                    core_id: Some("core-1".into()),
                    terminal: Some("t-closed".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: Some("working".into()),
                    agent_turn: None,
                    observed_agent: Some("codex".into()),
                    waiting: None,
                }),
                agent: None,
                created_at: day_start + 50_000,
            })
        })
        .unwrap();

    {
        let mut inv = app.inventory.lock().unwrap();
        inv.core = "core-1".into();
        inv.snapshot_at(
            &json!({
                "seq": 1,
                "terminals": [
                    {"address": "t-open", "state": "live", "status": {"state": "working"}},
                    {"address": "t-closed", "state": "live", "status": {"state": "working"}},
                ]
            }),
            day_start + 60_000,
        );
        inv.apply_at(
            &json!({"method": "event", "params": {"seq": 2, "type": "status", "terminal": "t-closed", "data": {"status": {"state": "idle"}}}}),
            day_start + 85_000,
        );
        // Reset open agent start time cleanly to day_end - 120_000 so it clips at day_end.
        inv.working.get_mut("t-open").unwrap().since = day_end - 120_000;
    }
    app.flush_agent_work().unwrap();

    let aggregate = |segments: &[ActivitySegment]| -> ActivityBucket {
        let mut dwell = DwellBucketTotals::default();
        let mut agent_ms = 0u64;
        let mut by_project: BTreeMap<String, ProjectActivityTotals> = BTreeMap::new();
        let mut by_agent_map: BTreeMap<(String, String, String), u64> = BTreeMap::new();
        for seg in segments {
            let dur = seg
                .end_ms
                .min(day_end)
                .saturating_sub(seg.start_ms.max(day_start));
            if dur == 0 {
                continue;
            }
            let pid = seg.project_id.as_deref().filter(|s| !s.is_empty());
            match (&seg.kind, &seg.data) {
                (ActivityKind::Dwell, ActivityDetail::Dwell(d)) => {
                    match d.lane {
                        DwellLane::Card => dwell.card += dur,
                        DwellLane::Terminal => dwell.terminal += dur,
                        DwellLane::Other => dwell.other += dur,
                    }
                    if let Some(pid) = pid {
                        by_project.entry(pid.to_owned()).or_default().dwell_ms += dur;
                    }
                }
                (ActivityKind::Agent, ActivityDetail::Agent(a)) => {
                    agent_ms += dur;
                    if let Some(pid) = pid {
                        by_project.entry(pid.to_owned()).or_default().agent_ms += dur;
                    }
                    *by_agent_map
                        .entry((a.profile_id.clone(), a.provider.clone(), a.model.clone()))
                        .or_default() += dur;
                }
                _ => {}
            }
        }
        let mut by_agent: Vec<AgentBucketStat> = by_agent_map
            .into_iter()
            .map(|((profile_id, provider, model), ms)| AgentBucketStat {
                profile_id,
                provider,
                model,
                ms,
            })
            .collect();
        by_agent.sort_by(|a, b| {
            b.ms.cmp(&a.ms)
                .then_with(|| a.profile_id.cmp(&b.profile_id))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.model.cmp(&b.model))
        });
        ActivityBucket {
            start_ms: day_start,
            end_ms: day_end,
            dwell,
            agent_ms,
            by_project,
            by_agent,
        }
    };

    // Unscoped parity check
    let act_res = app
        .call(
            "insights.activity",
            json!({"from_ms": day_start, "to_ms": day_end}),
        )
        .unwrap();
    let segs: Vec<ActivitySegment> = serde_json::from_value(act_res["segments"].clone()).unwrap();
    assert!(segs.iter().any(|s| s.open));
    let bucket_res = app
        .call(
            "insights.activity_buckets",
            json!({"boundaries": [day_start, day_end]}),
        )
        .unwrap();
    let buckets: Vec<ActivityBucket> =
        serde_json::from_value(bucket_res["buckets"].clone()).unwrap();
    assert_eq!(buckets.len(), 1);
    let expected = aggregate(&segs);
    assert_eq!(expected.dwell, buckets[0].dwell);
    assert_eq!(expected.agent_ms, buckets[0].agent_ms);
    assert_eq!(expected.by_project, buckets[0].by_project);
    assert_eq!(expected.by_agent, buckets[0].by_agent);
    assert_eq!(buckets[0].dwell.card, 90_000);
    assert_eq!(buckets[0].dwell.terminal, 180_000);
    assert_eq!(buckets[0].dwell.other, 45_000);
    assert_eq!(buckets[0].agent_ms, 145_000); // 120_000 open + 25_000 closed

    // Project-scoped parity check
    let scoped_act = app
        .call(
            "insights.activity",
            json!({"from_ms": day_start, "to_ms": day_end, "project_id": project.id}),
        )
        .unwrap();
    let scoped_segs: Vec<ActivitySegment> =
        serde_json::from_value(scoped_act["segments"].clone()).unwrap();
    let scoped_buckets_res = app
        .call(
            "insights.activity_buckets",
            json!({"boundaries": [day_start, day_end], "project_id": project.id}),
        )
        .unwrap();
    let scoped_buckets: Vec<ActivityBucket> =
        serde_json::from_value(scoped_buckets_res["buckets"].clone()).unwrap();
    let expected_scoped = aggregate(&scoped_segs);
    assert_eq!(expected_scoped.dwell, scoped_buckets[0].dwell);
    assert_eq!(expected_scoped.agent_ms, scoped_buckets[0].agent_ms);
    assert_eq!(expected_scoped.by_project, scoped_buckets[0].by_project);
    assert_eq!(expected_scoped.by_agent, scoped_buckets[0].by_agent);
    assert_eq!(scoped_buckets[0].dwell.card, 90_000);
    assert_eq!(scoped_buckets[0].agent_ms, 120_000);
}
