use super::*;
use canban_types::*;
use serde_json::json;

fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (idx, id) in ["attention", "review", "working", "updated", "quiet"]
        .into_iter()
        .enumerate()
    {
        s.projects.push(
            serde_json::from_value(json!({
                "id": id,
                "name": id,
                "directory": format!("/example/{id}"),
                "workflow": Workflow::default(),
                "created_at": idx as u64 + 1
            }))
            .unwrap(),
        );
        s.cards.push(
            serde_json::from_value(json!({
                "id": id,
                "project_id": id,
                "number": 1,
                "title": id,
                "body": "",
                "stage": "develop",
                "revision": 1,
                "created_at": 1
            }))
            .unwrap(),
        );
    }
    s
}

fn agent_terminal(s: &mut Snapshot, card: &str, profile: usize, state: &str) -> String {
    let prof = Config::default().profiles[profile].clone();
    let tid = Conversation::key(card, &prof.id);
    let mut a = canban_scheduler::agent_run(
        card,
        card,
        1,
        "develop",
        "stage_entry",
        prof,
        "Task".into(),
        None,
    );
    a.updated_at = 10;
    a.state = state.into();
    let number = s.terminals.len() as u64 + 1;
    s.terminals.push(TerminalEntry {
        id: tid.clone(),
        conversation_id: Some(tid.clone()),
        number,
        name: None,
        origin_card_id: Some(card.into()),
        agent_engine: None,
        runtime: None,
        agent: Some(a),
        created_at: 10,
    });
    tid
}

fn event(events: &mut Vec<Event>, project: &str, kind: &str, time: u64) {
    events.push(Event {
        id: format!("{project}-{time}"),
        project_id: project.into(),
        card_id: project.into(),
        sequence: time,
        kind: kind.into(),
        actor: "user".into(),
        message: String::new(),
        data: json!({}),
        created_at: time,
    });
}

#[test]
fn attention_unifies_review_and_input_by_card_and_ignores_working_and_archived() {
    let mut s = fixture();
    let blocked = agent_terminal(&mut s, "attention", 0, "blocked");
    agent_terminal(&mut s, "attention", 1, "blocked");
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Answer needed".into(),
        terminal_id: Some(blocked),
        next_stage: None,
    });
    let review = agent_terminal(&mut s, "review", 0, "completed");
    // Same Card has a Review gate and another Profile requiring input: still one.
    agent_terminal(&mut s, "review", 1, "blocked");
    s.cards[1].human_gate = Some(HumanGate {
        reason: "Review needed".into(),
        terminal_id: Some(review),
        next_stage: None,
    });
    agent_terminal(&mut s, "working", 0, "starting");
    agent_terminal(&mut s, "working", 1, "queued");
    s.reminders = canban_notifications::reminders_from_events(&s, &[]);
    let rows = projects(&s);
    assert_eq!(
        rows.iter().find(|r| r.id == "attention").unwrap().attention,
        1
    );
    assert_eq!(rows.iter().find(|r| r.id == "review").unwrap().attention, 1);
    assert_eq!(
        rows.iter().find(|r| r.id == "working").unwrap().attention,
        0
    );
    // A Review column alone is not an outstanding human decision.
    s.cards[3].stage = "review".into();
    s.reminders = canban_notifications::reminders_from_events(&s, &[]);
    assert_eq!(
        projects(&s)
            .iter()
            .find(|r| r.id == "updated")
            .unwrap()
            .attention,
        0
    );
    s.cards[0].archived_at = Some(20);
    s.cards[1].removed_at = Some(20);
    s.reminders = canban_notifications::reminders_from_events(&s, &[]);
    assert!(projects(&s).iter().all(|r| r.attention == 0));
}

#[test]
fn stable_order_uses_creation_time_then_name_and_ignores_events_or_attention() {
    let mut s = Snapshot::default();
    for (id, name, created_at) in [
        ("z-tie", "zeta", 10),
        ("a-tie", "Alpha", 10),
        ("early", "Early", 5),
        ("mid", "Mid", 20),
    ] {
        s.projects.push(
            serde_json::from_value(json!({
                "id": id,
                "name": name,
                "directory": format!("/example/{id}"),
                "workflow": Workflow::default(),
                "created_at": created_at
            }))
            .unwrap(),
        );
        s.cards.push(
            serde_json::from_value(json!({
                "id": id,
                "project_id": id,
                "number": 1,
                "title": id,
                "body": "",
                "stage": "develop",
                "revision": 1,
                "created_at": 1
            }))
            .unwrap(),
        );
    }

    let original = projects(&s);
    assert_eq!(
        original.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["early", "a-tie", "z-tie", "mid"]
    );

    // Reminders and activity events must never reorder existing tabs.
    s.cards[0].attention = Some("New input".into());
    let mut events = Vec::new();
    event(&mut events, "z-tie", "comment.replied", 99999);
    events.last_mut().unwrap().actor = "agent:developer".into();
    event(&mut events, "mid", "comment.created", 100000);
    s.reminders = canban_notifications::reminders_from_events(&s, &events);

    let after_events = projects(&s);
    assert_eq!(
        after_events
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["early", "a-tie", "z-tie", "mid"]
    );

    // Newly created project appends at the end.
    s.projects.push(
        serde_json::from_value(json!({
            "id": "newest",
            "name": "Aardvark",
            "directory": "/example/newest",
            "workflow": Workflow::default(),
            "created_at": 30
        }))
        .unwrap(),
    );
    let after_create = projects(&s);
    assert_eq!(
        after_create
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["early", "a-tie", "z-tie", "mid", "newest"]
    );
}

#[test]
fn project_badges_count_cards_once_across_unread_replies_and_pending_actions() {
    let mut s = fixture();
    let mut events = Vec::new();
    for sequence in [100, 101] {
        event(&mut events, "quiet", "comment.replied", sequence);
        events.last_mut().unwrap().actor = "agent:developer".into();
    }
    s.cards
        .iter_mut()
        .find(|c| c.id == "quiet")
        .unwrap()
        .attention = Some("Review this outcome".into());
    s.reminders = canban_notifications::reminders_from_events(&s, &events);
    let quiet = |s: &Snapshot| projects(s).into_iter().find(|p| p.id == "quiet").unwrap();
    assert_eq!(quiet(&s).attention, 1);
    assert!(quiet(&s).pending);
    s.card_reads.push(CardRead {
        id: "quiet".into(),
        card_id: "quiet".into(),
        project_id: "quiet".into(),
        sequence: 101,
    });
    assert_eq!(quiet(&s).attention, 1); // Reading cannot settle the action.
    s.cards
        .iter_mut()
        .find(|c| c.id == "quiet")
        .unwrap()
        .attention = None;
    s.reminders = canban_notifications::reminders_from_events(&s, &events);
    assert_eq!(quiet(&s).attention, 0);
}

#[test]
fn tab_width_is_bounded_and_stable_across_badge_changes() {
    let mut row = Summary {
        id: "p1".into(),
        name: "App".into(),
        directory: "/tmp/app".into(),
        host: None,
        offline: false,
        created_at: 1,
        attention: 0,
        pending: false,
    };
    let short_w = tab_width(&row);
    assert_eq!(short_w, TAB_MIN_WIDTH);
    row.attention = 200;
    row.pending = true;
    assert_eq!(tab_width(&row), short_w);

    let unicode_row = Summary {
        id: "p2".into(),
        name: "中文项目架构与超长上下文验证名称非常非常长".into(),
        directory: "/tmp/unicode".into(),
        host: Some("home-linux".into()),
        offline: false,
        created_at: 2,
        attention: 3,
        pending: false,
    };
    assert_eq!(tab_width(&unicode_row), TAB_MAX_WIDTH);
    assert_eq!(max_tab_width(0.), TAB_MAX_WIDTH);
    assert_eq!(max_tab_width(240.), 130.);
    assert_eq!(max_tab_width(150.), TAB_MIN_WIDTH);
}

#[test]
fn reveal_scroll_x_keeps_visible_tabs_stationary_and_scrolls_clipped_tabs_into_view() {
    let rows = (0..6)
        .map(|i| Summary {
            id: format!("p{i}"),
            name: format!("ProjectName{i}"),
            directory: format!("/tmp/p{i}"),
            host: None,
            offline: false,
            created_at: i as u64 + 1,
            attention: 0,
            pending: false,
        })
        .collect::<Vec<_>>();
    let w = tab_width(&rows[0]);
    assert!(w > TAB_MIN_WIDTH);

    // Zero viewport or missing selection never requests a scroll.
    assert_eq!(reveal_scroll_x(&rows, "p2", 0., 0., TAB_MAX_WIDTH), None);
    assert_eq!(
        reveal_scroll_x(&rows, "missing", 300., 0., TAB_MAX_WIDTH),
        None
    );

    // When everything fits, stay at 0 unless scrolled past 0.
    assert_eq!(
        reveal_scroll_x(&rows, "p5", w * 6. + 10., 0., TAB_MAX_WIDTH),
        None
    );
    assert_eq!(
        reveal_scroll_x(&rows, "p5", w * 6. + 10., 20., TAB_MAX_WIDTH),
        Some(0.)
    );

    // Viewport fits ~2.5 tabs; selecting an already-visible tab returns None.
    let viewport = w * 2.5;
    assert_eq!(
        reveal_scroll_x(&rows, "p1", viewport, 0., TAB_MAX_WIDTH),
        None
    );

    // Selecting a tab clipped on the right scrolls its right edge into view.
    assert_eq!(
        reveal_scroll_x(&rows, "p4", viewport, 0., TAB_MAX_WIDTH),
        Some(w * 5. - viewport)
    );

    // Selecting a tab clipped on the left scrolls its left edge into view.
    assert_eq!(
        reveal_scroll_x(&rows, "p1", viewport, w * 2., TAB_MAX_WIDTH),
        Some(w)
    );

    // When a single tab is wider than the narrow viewport, align its start edge.
    assert_eq!(
        reveal_scroll_x(&rows, "p3", w * 0.5, 0., TAB_MAX_WIDTH),
        Some(w * 3.)
    );
}

#[test]
fn remote_host_display_name_and_offline_tooltip() {
    let mut s = Snapshot::default();
    s.projects.push(
        serde_json::from_value(json!({
            "id": "local-p",
            "name": "LocalApp",
            "directory": "/Users/demo/local",
            "workflow": Workflow::default(),
            "created_at": 1
        }))
        .unwrap(),
    );
    s.projects.push(
        serde_json::from_value(json!({
            "id": "remote-p",
            "name": "RemoteApp",
            "directory": "/home/builder/remote",
            "workflow": Workflow::default(),
            "created_at": 2
        }))
        .unwrap(),
    );
    s.entity_hosts.insert("local-p".into(), "local".into());
    s.entity_hosts
        .insert("remote-p".into(), "home-linux".into());
    s.host_summaries.insert(
        "home-linux".into(),
        HostSummary {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            is_local: false,
            transport_label: "SSH tunnel".into(),
            endpoint_label: "builder@home-linux".into(),
            ssh_target: Some("builder@home-linux".into()),
            server_id: None,
            server_pubkey_fingerprint: None,
            connection: HostConnectionState::Offline {
                error: "network unreachable".into(),
                cached_at: Some(100),
            },
            active_agents: 0,
            max_agents: 4,
            project_count: 1,
            reconnect_count: 0,
            last_synced_at: Some(100),
            pending_mutations: 0,
            sync_conflicts: 0,
        },
    );

    let rows = projects(&s);
    assert_eq!(rows[0].display_name(), "LocalApp");
    assert_eq!(rows[0].tooltip(), "LocalApp\n/Users/demo/local");
    assert_eq!(rows[1].display_name(), "RemoteApp · Home Linux");
    assert_eq!(
        rows[1].tooltip(),
        "RemoteApp · Home Linux (offline)\n/home/builder/remote"
    );
}
