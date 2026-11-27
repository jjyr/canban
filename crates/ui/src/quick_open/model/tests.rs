use super::*;
use serde_json::json;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (id, name) in [("p1", "Canban"), ("p2", "Tools")] {
        s.projects.push(Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
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
    let profile = Config::default().profiles[0].clone();
    let tid = Conversation::key("c1", &profile.id);
    let att = canban_scheduler::agent_run(
        "p1",
        "c1",
        1,
        "develop",
        "interactive",
        profile,
        "Discuss".into(),
        None,
    );
    s.terminals.push(TerminalEntry {
        id: tid.clone(),
        conversation_id: Some(tid),
        number: 1,
        name: None,
        origin_project_id: None,
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: None,
        agent: Some(att),
        created_at: 1,
    });
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
    let run = s.terminals[0].agent.as_mut().unwrap();
    run.number = 2;
    run.updated_at += 1;
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
    assert_eq!(
        agents[0].target,
        Target::Agent(Conversation::key("c1", &Config::default().profiles[0].id))
    );
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
                "directory": "/example/p1",
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
        let tid = Conversation::key(&card.id, &profile.id);
        let att = canban_scheduler::agent_run(
            "p1",
            &card.id,
            1,
            "review",
            "interactive",
            profile.clone(),
            "Inspect".into(),
            None,
        );
        s.terminals.push(TerminalEntry {
            id: tid.clone(),
            conversation_id: Some(tid),
            number: 40 + idx as u64,
            name: None,
            origin_project_id: None,
            origin_card_id: Some(card.id.clone()),
            agent_engine: None,
            runtime: None,
            agent: Some(att),
            created_at: 1,
        });
        s.terminals.push(TerminalEntry {
            id: format!("term-hidden-{idx}"),
            conversation_id: None,
            number: 50 + idx as u64,
            name: Some(format!("Shell needle {idx}")),
            origin_project_id: None,
            origin_card_id: Some(card.id.clone()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{idx}")),
                state: "live".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            }),
            agent: None,
            created_at: 1,
        });
    }
    // Also check a closed standalone terminal on an active unmerged card is omitted.
    s.terminals.push(TerminalEntry {
        id: "term-ended".into(),
        conversation_id: None,
        number: 60,
        name: Some("Ended shell needle".into()),
        origin_project_id: None,
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("core".into()),
            terminal: Some("pty-ended".into()),
            state: "finished".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
            command: None,
        }),
        agent: None,
        created_at: 1,
    });
    // Whereas a live standalone terminal on an active unmerged card remains searchable.
    s.terminals.push(TerminalEntry {
        id: "term-live".into(),
        conversation_id: None,
        number: 61,
        name: Some("Active shell".into()),
        origin_project_id: None,
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("core".into()),
            terminal: Some("pty-live".into()),
            state: "live".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
            command: None,
        }),
        agent: None,
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
    assert!(gt_agent.len() >= 3);
    assert_eq!(gt_agent[0].id, "go-agent-setup");
    assert_eq!(gt_agent[1].id, "go-project-agents");
    assert!(gt_agent.iter().any(|c| c.id == "go-settings-agents"));

    let gt_argv = crate::quick_open::matching_commands(">argv");
    assert!(gt_argv.iter().any(|c| c.id == "go-settings-agents"));

    let gt_setup = crate::quick_open::matching_commands(">setup agent");
    assert!(!gt_setup.is_empty());
    assert_eq!(gt_setup[0].id, "go-agent-setup");

    let notif = crate::quick_open::matching_commands(">notifications");
    assert!(notif.iter().any(|c| c.id == "toggle-notifications"));
    assert!(notif.iter().any(|c| c.id == "mark-all-read"));

    let split = crate::quick_open::matching_commands(">card live terminal");
    assert!(split.iter().any(|c| c.id == "toggle-card-terminal"));

    let wt = crate::quick_open::matching_commands(">worktree");
    assert!(wt.iter().any(|c| c.id == "new-worktree"));

    let merge = crate::quick_open::matching_commands(">quick merge");
    assert!(merge.iter().any(|c| c.id == "quick-merge-card"));

    let advance = crate::quick_open::matching_commands(">advance");
    assert!(advance.iter().any(|c| c.id == "advance-card-stage"));
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
        directory: "/example/p1-wt".into(),
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

#[test]
fn quick_open_indexes_global_and_project_schedules() {
    let mut s = fixture();
    s.schedule_states.push(ScheduleState {
        host_id: String::new(),
        id: "sched-global-gc".into(),
        project_id: GLOBAL_PROJECT_ID.into(),
        workflow_id: "global:worktree-gc".into(),
        workflow_name: "Worktree GC (7d)".into(),
        filename: "worktree-gc.yaml".into(),
        rule_id: "schedule".into(),
        cron: "0 3 * * *".into(),
        timezone: None,
        overlap: ScheduleOverlap::Skip,
        creates_card: false,
        starts_with_script: true,
        target_column: None,
        profile_id: Some("developer".into()),
        enabled: true,
        last_scheduled_at: None,
        next_run_at: Some(1_000_000),
        last_run_id: Some("run-blocked".into()),
        run_count: 1,
        queued_triggers: 0,
        updated_at: 100,
    });
    s.scheduled_runs.push(ScheduledRun {
        host_id: String::new(),
        id: "run-blocked".into(),
        schedule_id: "sched-global-gc".into(),
        project_id: GLOBAL_PROJECT_ID.into(),
        workflow_id: "global:worktree-gc".into(),
        workflow_name: "Worktree GC (7d)".into(),
        filename: "worktree-gc.yaml".into(),
        rule_id: "schedule".into(),
        cron: "0 3 * * *".into(),
        number: 1,
        trigger_kind: "cron".into(),
        creates_card: false,
        card_id: None,
        card_number: None,
        workflow_run_id: None,
        start_step: Some("gc".into()),
        current_step: Some("ask_agent".into()),
        profile_id: Some("developer".into()),
        provider_session_id: None,
        terminal_id: Some("run-blocked".into()),
        cwd: "/tmp".into(),
        state: "blocked".into(),
        summary: Some("Dirty worktree needs decision".into()),
        report_path: None,
        output_path: None,
        error: None,
        steps: Vec::new(),
        scheduled_at: 100,
        started_at: Some(100),
        completed_at: None,
        close_requested_at: None,
        updated_at: 100,
    });

    let rows = candidates(&s);
    let gc_hits = ranked(&rows, "worktree gc", &[], false);
    assert_eq!(gc_hits[0].kind, "Schedule");
    assert_eq!(
        gc_hits[0].target,
        Target::Schedule("sched-global-gc".into())
    );
    assert!(gc_hits[0].detail.contains("Global · Host"));
    assert!(gc_hits[0].detail.contains("Script → Agent"));
    assert!(gc_hits[0].detail.contains("Needs you"));

    let global_hits = ranked(&rows, "global cron", &[], false);
    assert_eq!(global_hits[0].id, "schedule:sched-global-gc");
}

#[test]
fn quick_open_searches_cards_by_github_issue_and_pr_numbers() {
    let mut s = fixture();
    // Card #42 (c1) is local #42; add Card #118 linked to PR #402 and Card #119 linked to Issue #399,
    // plus Card #402 with local card number 402 to verify local number ranks above PR #402.
    s.cards.push(
        serde_json::from_value(json!({
            "id": "c-pr",
            "project_id": "p1",
            "number": 118,
            "title": "Fix scheduler lock",
            "body": "",
            "stage": "review",
            "revision": 1,
            "content": {
                "kind": "github_pr",
                "source": {
                    "repository": "jjyr/canban",
                    "repository_id": 1,
                    "issue_id": 4020,
                    "number": 402,
                    "url": "https://github.com/jjyr/canban/pull/402"
                }
            },
            "created_at": 1
        }))
        .unwrap(),
    );
    s.cards.push(
        serde_json::from_value(json!({
            "id": "c-issue",
            "project_id": "p1",
            "number": 119,
            "title": "Improve cache invalidation",
            "body": "",
            "stage": "develop",
            "revision": 1,
            "content": {
                "kind": "github_issue",
                "source": {
                    "repository": "jjyr/canban",
                    "repository_id": 1,
                    "issue_id": 3990,
                    "number": 399,
                    "url": "https://github.com/jjyr/canban/issues/399"
                }
            },
            "created_at": 1
        }))
        .unwrap(),
    );
    s.cards.push(
        serde_json::from_value(json!({
            "id": "c-local-402",
            "project_id": "p1",
            "number": 402,
            "title": "Local card 402",
            "body": "",
            "stage": "backlog",
            "revision": 1,
            "created_at": 1
        }))
        .unwrap(),
    );

    let rows = candidates(&s);
    // Exact local card number ranks first when searching `#402`, while PR #402 also matches.
    let by_hash = ranked(&rows, "#402", &[], false);
    assert_eq!(by_hash[0].target, Target::Card("c-local-402".into()));
    assert!(
        by_hash
            .iter()
            .any(|r| r.target == Target::Card("c-pr".into()))
    );

    // Prefix queries `pr 402`, `pr#402`, `pr:402`, `jjyr/canban#402` narrow straight to the PR Card.
    for q in ["pr 402", "pr#402", "pr:402", "jjyr/canban#402", "gh#402"] {
        let hits = ranked(&rows, q, &[], false);
        assert_eq!(hits.len(), 1, "expected 1 hit for query '{q}'");
        assert_eq!(hits[0].target, Target::Card("c-pr".into()));
        assert!(hits[0].detail.contains("PR #402"));
    }

    // Issue queries `issue:399` and `399` match the Issue Card.
    assert_eq!(
        ranked(&rows, "issue:399", &[], false)[0].target,
        Target::Card("c-issue".into())
    );
    assert_eq!(
        ranked(&rows, "399", &[], false)[0].target,
        Target::Card("c-issue".into())
    );
}

#[test]
fn quick_open_shows_host_names_on_duplicate_and_unique_multi_host_entries() {
    let mut local = Snapshot::default();
    for (id, name) in [("fiber", "fiber"), ("canban", "Canban")] {
        local.projects.push(Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            id: id.into(),
            name: name.into(),
            directory: format!("/local/{id}"),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: 1,
        });
    }
    local.terminals.push(TerminalEntry {
        id: "t-local".into(),
        conversation_id: None,
        number: 1,
        name: Some("Local Shell".into()),
        origin_project_id: Some("fiber".into()),
        origin_card_id: None,
        agent_engine: None,
        runtime: None,
        agent: None,
        created_at: 1,
    });
    local.register_host_entities("local");
    local.host_summaries.insert(
        "local".into(),
        HostSummary {
            id: "local".into(),
            name: crate::platform::LOCAL_HOST_NAME.into(),
            is_local: true,
            transport_label: "Local socket".into(),
            endpoint_label: "~/.canban/app.sock".into(),
            ssh_target: None,
            server_id: None,
            server_pubkey_fingerprint: None,
            connection: HostConnectionState::Connected {
                rtt_ms: 0,
                api_version: 1,
                version: "1".into(),
                platform: "macos".into(),
            },
            active_agents: 0,
            max_agents: 4,
            project_count: 2,
            reconnect_count: 0,
            last_synced_at: None,
            pending_mutations: 0,
            sync_conflicts: 0,
        },
    );

    let mut remote = Snapshot::default();
    remote.projects.push(Project {
        github: None,
        detected_github: None,
        github_disconnected: false,
        delivery_mode: Default::default(),
        agent_overrides: Default::default(),
        tags: Default::default(),
        id: "fiber".into(),
        name: "fiber".into(),
        directory: "/home/devpc/fiber".into(),
        workflow: Workflow::default(),
        target_branch: None,
        created_at: 2,
    });
    remote.terminals.push(TerminalEntry {
        id: "t-remote".into(),
        conversation_id: None,
        number: 2,
        name: Some("Remote Shell".into()),
        origin_project_id: Some("fiber".into()),
        origin_card_id: None,
        agent_engine: None,
        runtime: None,
        agent: None,
        created_at: 2,
    });
    remote.schedule_states.push(ScheduleState {
        host_id: String::new(),
        id: "schedule:__global:global:worktree-gc:schedule".into(),
        project_id: "__global".into(),
        workflow_id: "global:worktree-gc".into(),
        workflow_name: "Worktree GC".into(),
        filename: "worktree-gc.yaml".into(),
        rule_id: "schedule".into(),
        cron: "0 3 * * *".into(),
        timezone: None,
        overlap: ScheduleOverlap::Skip,
        creates_card: false,
        starts_with_script: true,
        target_column: None,
        profile_id: None,
        enabled: true,
        last_scheduled_at: None,
        next_run_at: None,
        last_run_id: None,
        run_count: 0,
        queued_triggers: 0,
        updated_at: 2,
    });

    local.merge_remote_snapshot(
        remote,
        HostSummary {
            id: "devpc".into(),
            name: "devpc".into(),
            is_local: false,
            transport_label: "SSH".into(),
            endpoint_label: "devpc".into(),
            ssh_target: Some("devpc".into()),
            server_id: None,
            server_pubkey_fingerprint: None,
            connection: HostConnectionState::Connected {
                rtt_ms: 8,
                api_version: 1,
                version: "1".into(),
                platform: "linux".into(),
            },
            active_agents: 0,
            max_agents: 4,
            project_count: 1,
            reconnect_count: 0,
            last_synced_at: None,
            pending_mutations: 0,
            sync_conflicts: 0,
        },
    );

    let rows = candidates(&local);
    let fiber_projects: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "Project" && r.title == "fiber")
        .collect();
    assert_eq!(fiber_projects.len(), 2);
    assert!(
        fiber_projects
            .iter()
            .any(|r| r.detail.contains(crate::platform::LOCAL_HOST_NAME))
    );
    assert!(fiber_projects.iter().any(|r| r.detail.contains("devpc")));

    let canban_project = rows
        .iter()
        .find(|r| r.kind == "Project" && r.title == "Canban")
        .unwrap();
    assert!(
        canban_project
            .detail
            .contains(crate::platform::LOCAL_HOST_NAME)
    );

    let remote_term = rows
        .iter()
        .find(|r| r.kind == "Terminal" && r.title.contains("Remote Shell"))
        .unwrap();
    assert!(remote_term.detail.contains("devpc"));

    let remote_sched = rows
        .iter()
        .find(|r| r.kind == "Schedule" && r.title.contains("Worktree GC"))
        .unwrap();
    assert!(remote_sched.detail.contains("devpc"));
}
