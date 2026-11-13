use super::*;
use serde_json::json;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (id, name) in [("p1", "Canban"), ("p2", "Tools")] {
        s.projects.push(Project {
            github: None,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
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
    let mut att = canban_scheduler::attempt(
        "p1",
        "c1",
        1,
        "develop",
        "interactive",
        Config::default().profiles[0].clone(),
        "Discuss".into(),
        None,
    );
    att.id = "attempt-1".into();
    s.attempts.push(att);
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
    let mut next = s.attempts[0].clone();
    next.id = "follow-up".into();
    next.number = 2;
    next.created_at += 1;
    s.attempts.push(next);
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
            tags: Vec::new(),
        },
        Candidate {
            id: "prefix".into(),
            title: "Cards".into(),
            detail: String::new(),
            kind: "Card",
            target: Target::Card("prefix".into()),
            fields: vec!["Cards".into()],
            tags: Vec::new(),
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
        tags: Vec::new(),
    };
    let mut b = a.clone();
    b.id = "a".into();
    b.detail = "Card · Tools".into();
    assert_eq!(ranked(&[b, a], "Architecture", &[], false)[0].id, "z");
}

#[test]
fn card_numbers_use_numeric_order() {
    let card = |id: &str, num: u64| Candidate {
        id: id.into(),
        title: format!("#{num} Task"),
        detail: "Card · Canban · backlog".into(),
        kind: "Card",
        target: Target::Card(id.into()),
        fields: vec![format!("#{num} Task"), "Card".into()],
        tags: Vec::new(),
    };
    let rows = vec![card("c1", 1), card("c10", 10), card("c2", 2)];
    // Empty query surfaces newest active cards first.
    let initial = ranked(&rows, "", &[], false);
    let ids: Vec<_> = initial.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["c10", "c2", "c1"]);

    // Text query sorts matching cards in natural ascending card number (#1, #2, #10), never #1, #10, #2.
    let matched = ranked(&rows, "Task", &[], false);
    let matched_ids: Vec<_> = matched.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(matched_ids, vec!["c1", "c2", "c10"]);
}

#[test]
fn quick_open_excludes_archived_removed_and_merged_cards_and_their_sessions() {
    let mut s = fixture();
    let profile = Config::default().profiles[0].clone();

    let mut archived: Card = serde_json::from_value(json!({
        "id": "c-archived",
        "project_id": "p1",
        "number": 90,
        "title": "Archived secret needle",
        "body": "",
        "stage": "review",
        "revision": 1,
        "archived_at": 100,
        "created_at": 1
    }))
    .unwrap();
    let mut removed = archived.clone();
    removed.id = "c-removed".into();
    removed.number = 91;
    removed.title = "Removed secret needle".into();
    removed.archived_at = None;
    removed.removed_at = Some(100);

    let mut local_merged = archived.clone();
    local_merged.id = "c-local-merged".into();
    local_merged.number = 92;
    local_merged.title = "Local merged secret needle".into();
    local_merged.archived_at = None;
    local_merged.delivery = Some(Delivery {
        target_branch: "main".into(),
        commit: "abc1234".into(),
        request_key: "merge-1".into(),
        merged_at: 200,
    });

    let mut pr_merged: Card = serde_json::from_value(json!({
        "id": "c-pr-merged",
        "project_id": "p1",
        "number": 93,
        "title": "PR merged secret needle",
        "body": "",
        "stage": "review",
        "revision": 1,
        "created_at": 1,
        "pull_request": {
            "repository": "jjyr/canban",
            "repository_id": 1,
            "number": 55,
            "url": "https://github.com/jjyr/canban/pull/55",
            "state": "closed",
            "head": "def5678",
            "head_branch": "canban/93",
            "base_branch": "main",
            "merged": true,
            "checks": "success",
            "workspace": {
                "location": {"machine": "local", "directory": "/example/p1"},
                "path": "/example/p1"
            }
        }
    }))
    .unwrap();

    // Ensure archived/merged flags are as expected on the Card type.
    assert!(!archived.active());
    assert!(!removed.active());
    assert!(local_merged.active() && local_merged.merged());
    assert!(pr_merged.active() && pr_merged.merged());

    for (idx, card) in [&archived, &removed, &local_merged, &pr_merged]
        .into_iter()
        .enumerate()
    {
        let mut att = canban_scheduler::attempt(
            "p1",
            &card.id,
            1,
            "review",
            "interactive",
            profile.clone(),
            "Inspect".into(),
            None,
        );
        att.id = format!("req-hidden-{idx}");
        s.attempts.push(att);
        s.terminals.push(TerminalEntry {
            id: format!("term-hidden-{idx}"),
            conversation_id: None,
            number: 50 + idx as u64,
            name: Some(format!("Shell needle {idx}")),
            manual_number: Some(50 + idx as u64),
            origin_card_id: Some(card.id.clone()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{idx}")),
                state: "live".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
            }),
            created_at: 1,
        });
    }
    // Also check a closed standalone terminal on an active unmerged card is omitted.
    s.terminals.push(TerminalEntry {
        id: "term-ended".into(),
        conversation_id: None,
        number: 60,
        name: Some("Ended shell needle".into()),
        manual_number: Some(60),
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-ended".into()),
            state: "finished".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 1,
    });
    // Whereas a live standalone terminal on an active unmerged card remains searchable.
    s.terminals.push(TerminalEntry {
        id: "term-live".into(),
        conversation_id: None,
        number: 61,
        name: Some("Active shell".into()),
        manual_number: Some(61),
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-live".into()),
            state: "live".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 1,
    });

    archived.branch = Some("feat/archived-branch".into());
    pr_merged.branch = Some("feat/merged-branch".into());
    s.cards.extend([archived, removed, local_merged, pr_merged]);

    let rows = candidates(&s);
    for hidden_id in ["c-archived", "c-removed", "c-local-merged", "c-pr-merged"] {
        assert!(
            !rows
                .iter()
                .any(|r| r.target == Target::Card(hidden_id.into())),
            "unexpected Card candidate {hidden_id}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.target == Target::Agent(format!("{hidden_id}:codex"))),
            "unexpected Agent candidate for {hidden_id}"
        );
    }
    assert!(ranked(&rows, "secret needle", &[], false).is_empty());
    assert!(ranked(&rows, "Shell needle", &[], false).is_empty());
    assert!(ranked(&rows, "Ended shell needle", &[], false).is_empty());
    assert!(ranked(&rows, "#90", &[], false).is_empty());
    assert!(ranked(&rows, "#92", &[], false).is_empty());
    assert!(ranked(&rows, "#93", &[], false).is_empty());
    assert!(ranked(&rows, "archived-branch", &[], false).is_empty());
    assert!(ranked(&rows, "merged-branch", &[], false).is_empty());

    let live_term = ranked(&rows, "Active shell", &[], false);
    assert_eq!(live_term.len(), 1);
    assert_eq!(live_term[0].target, Target::Terminal("term-live".into()));
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

#[test]
fn hash_tag_search_matches_tagged_cards_without_false_matching_title_subsequences() {
    let mut s = fixture();
    s.projects[0].tags = vec!["bug".into(), "bugfix".into(), "前端".into()];
    s.cards[0].title = "Fix bug in login".into(); // NO tags on c1!
    s.cards[0].body = "No tags in this card".into();
    s.cards[1].title = "Crash on startup".into();
    s.cards[1].body = "Investigate #bug in startup".into();
    s.cards[2].body = "Polish #bugfix and #前端".into();

    let rows = candidates(&s);
    // `#bug` matches `#bug` (exact first) and `#bugfix` (prefix second), NOT untagged `#42 Fix bug in login`!
    let matched = ranked(&rows, "#bug", &[], false);
    let matched_ids: Vec<_> = matched.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(matched_ids, vec!["card:c2", "card:c3"]);
    assert!(matched[0].detail.contains("#bug"));

    // Multi-term `#前端 Canban` narrows to c3.
    let cjk = ranked(&rows, "#前端 Canban", &[], false);
    assert_eq!(cjk.len(), 1);
    assert_eq!(cjk[0].target, Target::Card("c3".into()));

    // Numeric `#42` still works as a Card number query rather than a tag filter.
    assert_eq!(ranked(&rows, "Tools #42", &[], false).len(), 1);

    // Archived and merged tagged Cards are excluded from `#tag` Quick Open results.
    let mut archived_tagged: Card = serde_json::from_value(json!({
        "id": "c-archived-tag",
        "project_id": "p1",
        "number": 94,
        "title": "Archived tagged card",
        "body": "Old #bug report",
        "stage": "review",
        "revision": 1,
        "archived_at": 100,
        "created_at": 1
    }))
    .unwrap();
    let mut merged_tagged = archived_tagged.clone();
    merged_tagged.id = "c-merged-tag".into();
    merged_tagged.number = 95;
    merged_tagged.title = "Merged tagged card".into();
    merged_tagged.archived_at = None;
    merged_tagged.delivery = Some(Delivery {
        target_branch: "main".into(),
        commit: "abc1234".into(),
        request_key: "merge-tag".into(),
        merged_at: 200,
    });
    archived_tagged.body = "Old #bug report".into();
    s.cards.extend([archived_tagged, merged_tagged]);
    let rows_with_historical = candidates(&s);
    let matched_after = ranked(&rows_with_historical, "#bug", &[], false);
    let after_ids: Vec<_> = matched_after.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(after_ids, vec!["card:c2", "card:c3"]);
}
