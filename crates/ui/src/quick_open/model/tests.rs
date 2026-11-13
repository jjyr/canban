use super::*;
use serde_json::json;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (id, name) in [("p1", "Canban"), ("p2", "Tools")] {
        s.projects.push(Project {
            github: None,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            location: None,
            id: id.into(),
            name: name.into(),
            directory: format!("/example/{id}"),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: 1,
        });
    }
    for (id, project, title) in [
        ("c1", "p1", "Terminal integration"),
        ("c2", "p2", "Terminal integration"),
        ("c3", "p1", "中文搜索与 café"),
    ] {
        s.cards.push(serde_json::from_value(json!({"id":id,"project_id":project,"number":42,"title":title,"body":"","stage":"backlog","revision":1,"attention":null,"archived_at":null,"archive":null,"archive_error":null,"worktree":null,"created_at":1})).unwrap());
    }
    s.notes.push(Note {
        id: "n1".into(),
        project_id: "p2".into(),
        title: "Architecture decision.md".into(),
        body: "Private text is not indexed".into(),
        revision: 1,
        created_at: 1,
        updated_at: 1,
        links: vec![],
        card_ids: vec![],
    });
    let mut req = canban_scheduler::request(
        "p1",
        "c1",
        "develop",
        "interactive",
        Config::default().profiles[0].clone(),
        "Discuss".into(),
        None,
    );
    req.id = "request".into();
    s.requests.push(req);
    s
}
#[test]
fn fuzzy_search_matches_subsequences_multiple_terms_and_unicode() {
    let rows = candidates(&fixture());
    assert!(!ranked(&rows, "tmg", &[], false).is_empty());
    let tools = ranked(&rows, "Tools #42", &[], false);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].target, Target::Card("c2".into()));
    assert_eq!(
        ranked(&rows, "中文 café", &[], false)[0].target,
        Target::Card("c3".into())
    );
    // Legacy records remain in the snapshot, but deferred Notes are not destinations.
    assert!(ranked(&rows, "Architecture decision", &[], false).is_empty());
    assert!(rows.iter().all(|r| r.kind != "Note"));
    assert!(ranked(&rows, "not-present-123", &[], false).is_empty());
    assert!(ranked(&rows, "Private text", &[], false).is_empty());
}
#[test]
fn candidates_distinguish_duplicate_numbers_and_keep_one_agent_conversation() {
    let mut s = fixture();
    let mut next = s.requests[0].clone();
    next.id = "follow-up".into();
    next.created_at += 1;
    s.requests.push(next);
    let rows = candidates(&s);
    let cards = rows
        .iter()
        .filter(|r| r.title.starts_with("#42 Terminal"))
        .collect::<Vec<_>>();
    assert_eq!(cards.len(), 2);
    assert_ne!(cards[0].id, cards[1].id);
    assert_ne!(cards[0].detail, cards[1].detail);
    let agents = rows
        .iter()
        .filter(|r| r.kind == "Agent")
        .collect::<Vec<_>>();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].target, Target::Agent("c1:codex".into()));
    assert!(agents[0].detail.contains("Canban"));
}
#[test]
fn project_picker_filters_types_and_recency_does_not_overrule_a_better_match() {
    let rows = candidates(&fixture());
    let recent = vec!["project:p2".into()];
    let projects = ranked(&rows, "", &recent, true);
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].target, Target::Project("p2".into()));
    assert!(projects.iter().all(|r| r.kind == "Project"));
    assert_eq!(
        ranked(&rows, "Canban", &recent, true)[0].target,
        Target::Project("p1".into())
    );
    let rows = vec![
        Candidate {
            id: "exact".into(),
            title: "Card".into(),
            detail: String::new(),
            kind: "Card",
            target: Target::Card("exact".into()),
            fields: vec!["Card".into()],
        },
        Candidate {
            id: "prefix".into(),
            title: "Cards".into(),
            detail: String::new(),
            kind: "Card",
            target: Target::Card("prefix".into()),
            fields: vec!["Cards".into()],
        },
    ];
    assert_eq!(
        ranked(&rows, "card", &["prefix".into()], false)[0].id,
        "exact"
    );
}

#[test]
fn duplicate_titles_sort_by_visible_owner_before_internal_identity() {
    let a = Candidate {
        id: "z".into(),
        title: "Architecture.md".into(),
        detail: "Card · Canban".into(),
        kind: "Card",
        target: Target::Card("z".into()),
        fields: vec!["Architecture.md".into()],
    };
    let mut b = a.clone();
    b.id = "a".into();
    b.detail = "Card · Tools".into();
    assert_eq!(ranked(&[b, a], "Architecture", &[], false)[0].id, "z");
}

#[test]
fn card_numbers_use_numeric_order_and_unarchived_before_archived() {
    let card = |id: &str, num: u64, archived: bool| Candidate {
        id: id.into(),
        title: format!("#{num} Task"),
        detail: if archived {
            "Card · Canban · done · Archived".into()
        } else {
            "Card · Canban · backlog".into()
        },
        kind: "Card",
        target: Target::Card(id.into()),
        fields: vec![format!("#{num} Task"), "Card".into()],
    };
    let rows = vec![
        card("c1", 1, false),
        card("c10", 10, false),
        card("c2", 2, false),
        card("c11", 11, true),
    ];
    // Empty query surfaces newest unarchived cards first, archived last.
    let initial = ranked(&rows, "", &[], false);
    let ids: Vec<_> = initial.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["c10", "c2", "c1", "c11"]);

    // Text query sorts matching cards in natural ascending card number (#1, #2, #10), never #1, #10, #2.
    let matched = ranked(&rows, "Task", &[], false);
    let matched_ids: Vec<_> = matched.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(matched_ids, vec!["c1", "c2", "c10", "c11"]);
}

#[test]
fn command_queries_require_gt_prefix_and_match_agent_setup_commands() {
    assert!(crate::quick_open::is_command_query(">"));
    assert!(crate::quick_open::is_command_query(" >agent"));
    assert!(!crate::quick_open::is_command_query("/"));
    assert!(!crate::quick_open::is_command_query("/agent"));
    assert!(!crate::quick_open::is_command_query("agent"));

    let gt_agent = crate::quick_open::matching_commands(">agent");
    assert!(gt_agent.len() >= 2);
    assert_eq!(gt_agent[0].id, "go-agent-setup");
    assert_eq!(gt_agent[1].id, "go-project-agents");

    let gt_setup = crate::quick_open::matching_commands(">setup agent");
    assert!(!gt_setup.is_empty());
    assert_eq!(gt_setup[0].id, "go-agent-setup");
}

#[test]
fn command_catalog_ids_are_unique_and_cover_notifications_card_terminal_and_worktrees() {
    let mut ids = std::collections::BTreeSet::new();
    for cmd in crate::commands::CATALOG {
        assert!(
            ids.insert(cmd.id),
            "duplicate command id in CATALOG: {}",
            cmd.id
        );
    }

    let notif = crate::quick_open::matching_commands(">notifications");
    assert!(notif.iter().any(|c| c.id == "toggle-notifications"));
    assert!(notif.iter().any(|c| c.id == "mark-all-read"));

    let split = crate::quick_open::matching_commands(">card live terminal");
    assert!(split.iter().any(|c| c.id == "toggle-card-terminal"));

    let wt = crate::quick_open::matching_commands(">worktree");
    assert!(wt.iter().any(|c| c.id == "new-worktree"));

    let merge = crate::quick_open::matching_commands(">quick merge");
    assert!(merge.iter().any(|c| c.id == "quick-merge-card"));
}

#[test]
fn quick_open_indexes_standalone_managed_worktrees() {
    let mut s = fixture();
    s.worktrees.push(ManagedWorktree {
        id: "wt-1".into(),
        project_id: "p1".into(),
        branch: "feat/standalone-spike".into(),
        base_branch: "main".into(),
        base_commit: None,
        path: "/example/p1-wt".into(),
        location: WorkLocation {
            machine: "local".into(),
            directory: "/example/p1-wt".into(),
        },
        created_at: 2,
    });
    let rows = candidates(&s);
    let matched = ranked(&rows, "standalone-spike", &[], false);
    assert!(!matched.is_empty());
    assert_eq!(matched[0].kind, "Worktree");
    assert_eq!(matched[0].target, Target::Worktree("wt-1".into()));
}
