use super::*;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (project, card) in [("p1", "c1"), ("p2", "c2")] {
        s.projects.push(serde_json::from_value(json!({"id":project,"name":project,"directory":"/tmp/project","workflow":Workflow::default(),"created_at":1})).unwrap());
        s.cards.push(serde_json::from_value(json!({"id":card,"project_id":project,"number":1,"title":"Same title","body":"","stage":"develop","revision":1,"created_at":1})).unwrap());
        let mut a = canban_scheduler::attempt(
            project,
            card,
            1,
            "develop",
            "interactive",
            Config::default().profiles[0].clone(),
            "task".into(),
            None,
        );
        a.id = format!("run-{card}");
        a.created_at = 1;
        a.state = "running".into();
        a.core_id = Some("core".into());
        a.terminal = Some(format!("t-{card}"));
        a.cwd = "/tmp".into();
        a.input_path = "input".into();
        a.observed = Some(json!({"state":"live","status":{"state":"working"}}));
        a.provider_session_id = Some(format!("provider-{card}"));
        s.attempts.push(a);
    }
    s
}
#[test]
fn tree_keeps_projects_and_profiles_distinct_and_excludes_unassigned_recovery() {
    let mut s = fixture();
    let mut att = s.attempts[0].clone();
    att.id = "reviewer".into();
    att.profile.id = "reviewer".into();
    att.state = "queued".into();
    att.terminal = None;
    att.observed = None;
    s.attempts.push(att);
    let mut recovery = s.cards[0].clone();
    recovery.id = "unassigned".into();
    recovery.attention = Some("No profile".into());
    s.cards.push(recovery);
    let groups = groups(&s, false);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].cards.len(), 1);
    assert_eq!(groups[0].cards[0].rows.len(), 2);
    assert_eq!(conversations(&s).len(), 3);
    assert_ne!(groups[0].cards[0].rows[0].id, groups[1].cards[0].rows[0].id);
}
#[test]
fn live_invocation_precedes_queued_followup_and_resume_keeps_identity() {
    let mut s = fixture();
    let key = Conversation::key("c1", "codex");
    let mut next = s.attempts[0].clone();
    next.id = "followup".into();
    next.number = 2;
    next.created_at = 2;
    next.state = "queued".into();
    next.terminal = None;
    next.observed = None;
    s.attempts.push(next);
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    assert_eq!(row.attempt.unwrap().id, "run-c1");
    s.attempts[0].observed = Some(json!({"state":"finished"}));
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    assert_eq!(row.attempt.as_ref().unwrap().id, "followup");
    assert!(!closed(&row));
    assert!(binding(&row).is_none());
    let f = s.attempts.iter_mut().find(|a| a.id == "followup").unwrap();
    f.state = "running".into();
    f.terminal = Some("new-terminal".into());
    f.observed = Some(json!({"state":"live"}));
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    assert_eq!(binding(&row).unwrap()["terminal"], "new-terminal");
    assert_eq!(row.history.len(), 2);
}
#[test]
fn closed_archived_and_missing_bindings_never_create_viewers_or_implicit_resume() {
    let mut s = fixture();
    s.attempts[0].observed = Some(json!({"state":"finished"}));
    let mut profiles = Config::default().profiles;
    profiles[0].resume_argv = Some(vec!["resume".into()]);
    let row = conversations(&s)
        .into_iter()
        .find(|r| r.card.id == "c1")
        .unwrap();
    assert!(closed(&row));
    assert!(binding(&row).is_none());
    assert!(can_resume(&row, &profiles));
    assert_eq!(groups(&s, true).len(), 1);
    s.cards[0].archived_at = Some(3);
    let row = conversations(&s)
        .into_iter()
        .find(|r| r.card.id == "c1")
        .unwrap();
    assert!(!can_resume(&row, &profiles));
    assert!(binding(&row).is_none());
    s.attempts[1].core_id = None;
    let row = conversations(&s)
        .into_iter()
        .find(|r| r.card.id == "c2")
        .unwrap();
    assert!(binding(&row).is_none());
    assert_eq!(s.attempts.len(), 2);
}
#[test]
fn unified_entries_keep_numeric_names_across_agent_resume_and_shell_lifetimes() {
    let mut s = fixture();
    s.terminals = crate::terminals::entries(&s)
        .into_iter()
        .map(|e| e.saved)
        .collect();
    let key = Conversation::key("c1", "codex");
    let label = s.terminals.iter().find(|t| t.id == key).unwrap().number;
    s.terminals.push(TerminalEntry {
        id: "manual".into(),
        conversation_id: None,
        number: 9,
        name: Some("Server".into()),
        manual_number: Some(1),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("shell-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 0,
    });
    let rows = crate::terminals::entries(&s);
    let shell = rows.iter().find(|e| e.saved.id == "manual").unwrap();
    assert_eq!(shell.saved.title(), "Server");
    assert!(shell.card.is_none());
    assert!(shell.agent.is_none());
    assert_eq!(shell.binding().unwrap()["terminal"], "shell-pty");
    s.attempts[0].terminal = Some("resumed-pty".into());
    let row = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(row.saved.number, label);
    assert_eq!(row.binding().unwrap()["terminal"], "resumed-pty");
    if let Some(TerminalRuntime { state, .. }) = &mut s.terminals.last_mut().unwrap().runtime {
        *state = "finished".into();
    }
    let ended = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "manual")
        .unwrap();
    assert!(ended.ended());
    assert!(ended.binding().is_none());
    assert_eq!(s.attempts.len(), 2);
}
#[test]
fn workspace_shortcuts_are_valid_nsh_keystrokes() {
    for (key, _) in crate::terminals::TAB_KEYS {
        nsh_core::desktop::Shortcut::parse(key, false).unwrap();
    }
}

#[test]
fn directory_regrouping_changes_hints_without_changing_terminal_identity_or_agent_assignment() {
    let mut s = fixture();
    s.terminals = crate::terminals::entries(&s)
        .into_iter()
        .map(|e| e.saved)
        .collect();
    let agent = s.terminals[0].id.clone();
    let shell = TerminalEntry {
        id: "shell".into(),
        conversation_id: None,
        number: 9,
        name: None,
        manual_number: None,
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 0,
    };
    s.terminals.push(shell.clone());
    let rows = crate::terminals::entries(&s);
    assert!(
        rows.iter()
            .find(|e| e.saved.id == "shell")
            .unwrap()
            .project_id
            .is_none()
    );
    for id in ["shell", agent.as_str()] {
        s.terminal_contexts.push(TerminalContext {
            machine: "local".into(),
            terminal_id: id.into(),
            core_id: "core".into(),
            terminal: "pty".into(),
            cwd: Some("/project-2".into()),
            project_id: Some("p2".into()),
            card_id: Some("c2".into()),
            branch: Some("feature".into()),
        });
    }
    let rows = crate::terminals::entries(&s);
    let manual = rows.iter().find(|e| e.saved.id == "shell").unwrap();
    assert_eq!(manual.saved, shell);
    assert_eq!(manual.project_id.as_deref(), Some("p2"));
    assert!(manual.inferred_card);
    assert!(manual.agent.is_none());
    let task = rows.iter().find(|e| e.saved.id == agent).unwrap();
    assert_eq!(task.project_id.as_deref(), Some("p1"));
    assert_eq!(task.card.as_ref().unwrap().id, "c1");
    assert!(!task.inferred_card);
    s.terminal_contexts.clear();
    assert!(
        crate::terminals::entries(&s)
            .into_iter()
            .find(|e| e.saved.id == "shell")
            .unwrap()
            .project_id
            .is_none()
    );
}

#[test]
fn automatic_labels_follow_execution_stage_and_profile_without_changing_binding() {
    use canban_ui_kit::Icon;
    let mut s = fixture();
    s.cards[0].stage = "review".into();
    s.cards[0].title = "非常长的任务标题 / a long task title with important details".repeat(8);
    s.attempts[0].profile.id = "developer".into();
    let key = Conversation::key("c1", "developer");
    let entry = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    let binding = entry.binding();
    assert!(entry.title().starts_with("#1 非常长的任务标题"));
    assert_eq!(entry.stage.as_deref(), Some("Develop"));
    assert_eq!(entry.profile(), Some("developer"));
    assert_eq!(entry.provider().unwrap().0, Icon::Codex);
    assert!(entry.hint().contains(&s.cards[0].title));
    s.terminals.push(entry.saved.clone());
    s.terminals[0].name = Some("My review desk".into());
    let named = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(named.title(), "My review desk");
    assert_eq!(named.binding(), binding);
    s.terminals[0].name = None;
    s.attempts[0].profile.argv = Some(vec!["/opt/bin/claude".into()]);
    let entry = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(entry.provider().unwrap().0, Icon::Claude);
    s.attempts[0].profile.preset = None;
    s.attempts[0].profile.argv = Some(vec!["my-agent".into()]);
    let entry = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(entry.provider(), Some((Icon::Agent, "my-agent".into())));
}

#[test]
fn input_gating_has_a_specific_lifecycle_label_and_keeps_binding_identity() {
    let mut s = fixture();
    let key = Conversation::key("c1", "codex");
    for (connection, close, label) in [
        ("connecting", "", "Connecting"),
        ("unavailable", "", "Disconnected"),
        ("connected", "closing", "Closing"),
        ("connected", "pending", "Closing"),
        ("connected", "", ""),
    ] {
        s.attempts[0].connection_state = connection.into();
        s.attempts[0].close_state = close.into();
        let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
        let b = binding(&row).unwrap();
        assert_eq!(b["input_status"].as_str().unwrap(), label);
        assert_eq!(b["read_only"], !label.is_empty());
        assert_eq!(b["terminal"], s.attempts[0].terminal.as_deref().unwrap());
    }
}

#[test]
fn inactive_is_a_live_unattended_conversation_and_closed_entries_leave_inventory() {
    let mut s = fixture();
    s.attempts[0].state = "completed".into();
    s.attempts[0].observed = Some(json!({"state":"live","status":{"state":"done"}}));
    s.attempts[0].connection_state = "connected".into();
    s.attempts[0].lifecycle.observed_at = Some(now());
    let entry = |s: &Snapshot| {
        super::super::entries(s)
            .into_iter()
            .find(|e| e.card.as_ref().is_some_and(|c| c.id == "c1"))
            .unwrap()
    };
    assert!(entry(&s).listed());
    assert!(entry(&s).inactive());
    // Immediate state update: recent interaction does not pin a finished idle Agent as active.
    s.attempts[0].lifecycle.last_interaction_at = Some(now());
    assert!(entry(&s).inactive());
    assert!(!entry(&s).needs_attention());
    // Unfinished assignment sitting idle requires user inspection instead of silent inactive state.
    s.attempts[0].state = "running".into();
    assert!(entry(&s).needs_attention());
    assert!(!entry(&s).inactive());
    s.attempts[0].observed = Some(json!({"state":"finished"}));
    assert!(!entry(&s).listed());
    s.attempts[0].observed = Some(json!({"state":"live"}));
    s.cards[0].archived_at = Some(now());
    assert!(!entry(&s).listed());
}

#[test]
fn card_destination_requires_choice_and_remembers_only_live_selection() {
    use crate::terminals::{entries, navigation::direct};
    let mut s = fixture();
    let mut a = s.attempts[0].clone();
    a.id = "second-attempt".into();
    a.profile.id = "reviewer".into();
    a.terminal = Some("second-pty".into());
    s.attempts.push(a);
    let card_entries = |s: &Snapshot| {
        entries(s)
            .into_iter()
            .filter(|e| e.card.as_ref().is_some_and(|c| c.id == "c1"))
            .collect::<Vec<_>>()
    };
    let rows = card_entries(&s);
    assert_eq!(direct(&rows, None), None);
    let preferred = Conversation::key("c1", "reviewer");
    assert_eq!(direct(&rows, Some(&preferred)), Some(preferred.clone()));
    // Disconnection retains the original target; exit invalidates the preference.
    s.attempts.last_mut().unwrap().connection_state = "unavailable".into();
    assert_eq!(
        direct(&card_entries(&s), Some(&preferred)),
        Some(preferred.clone())
    );
    s.attempts.last_mut().unwrap().observed = Some(json!({"state":"finished"}));
    assert_eq!(
        direct(&card_entries(&s), Some(&preferred)),
        Some(Conversation::key("c1", "codex"))
    );
    s.attempts[0].terminal = None;
    assert_eq!(direct(&card_entries(&s), Some(&preferred)), None);
}

#[test]
fn historical_request_never_attaches_to_a_newer_profile_invocation() {
    use crate::terminals::navigation::live_request;
    let mut s = fixture();
    let original = s.attempts[0].id.clone();
    assert!(live_request(&s, &original).is_some());
    s.attempts[0].observed = Some(json!({"state":"finished"}));
    let mut attempt = s.attempts[0].clone();
    attempt.id = "new-attempt".into();
    attempt.number = 2;
    attempt.created_at = 2;
    attempt.observed = Some(json!({"state":"live","status":{"state":"working"}}));
    s.attempts.push(attempt);
    assert_eq!(live_request(&s, &original), None);
    assert!(live_request(&s, "new-attempt").is_some());
    assert_eq!(live_request(&s, "removed-attempt"), None);
}

#[test]
fn card_live_entries_unify_workflow_agents_ephemeral_cli_agents_and_card_shells() {
    use canban_ui_kit::Icon;
    let mut s = fixture();
    s.terminals = crate::terminals::entries(&s)
        .into_iter()
        .map(|e| e.saved)
        .collect();
    s.terminals.push(TerminalEntry {
        id: "agent-ephemeral".into(),
        conversation_id: None,
        number: 2,
        name: None,
        manual_number: Some(1),
        origin_card_id: Some("c1".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-agy".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-backlog".into(),
        conversation_id: None,
        number: 3,
        name: None,
        manual_number: Some(2),
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 3,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 4,
        name: None,
        manual_number: Some(3),
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
        created_at: 4,
    });

    let live = crate::terminals::card_live_entries(&s, "c1");
    let ids: Vec<_> = live.iter().map(|e| e.saved.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            Conversation::key("c1", "codex").as_str(),
            "agent-ephemeral",
            "shell-backlog",
        ]
    );
    let agy = live
        .iter()
        .find(|e| e.saved.id == "agent-ephemeral")
        .unwrap();
    assert_eq!(agy.saved.title(), "Antigravity 1");
    assert_eq!(agy.provider(), Some((Icon::Agy, "Antigravity".into())));
    assert!(!agy.inferred_card);

    let shell = live.iter().find(|e| e.saved.id == "shell-backlog").unwrap();
    assert_eq!(shell.saved.title(), "Terminal 2");
    assert!(shell.provider().is_none());

    // Moving the shell into another project detaches it from the origin card tab bar.
    s.terminal_contexts.push(TerminalContext {
        machine: "local".into(),
        terminal_id: "shell-backlog".into(),
        core_id: "core".into(),
        terminal: "pty-shell".into(),
        cwd: Some("/project-2".into()),
        project_id: Some("p2".into()),
        card_id: None,
        branch: None,
    });
    let live_after_move = crate::terminals::card_live_entries(&s, "c1");
    assert!(
        live_after_move
            .iter()
            .all(|e| e.saved.id != "shell-backlog")
    );
}

#[test]
fn default_card_terminal_prefers_recent_live_otherwise_first_active_entry() {
    let mut s = fixture();
    s.terminals = crate::terminals::entries(&s)
        .into_iter()
        .map(|e| e.saved)
        .collect();
    s.terminals.push(TerminalEntry {
        id: "agent-second".into(),
        conversation_id: None,
        number: 2,
        name: None,
        manual_number: Some(1),
        origin_card_id: Some("c1".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-agy".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 3,
        name: None,
        manual_number: Some(2),
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
        created_at: 3,
    });

    let first_id = Conversation::key("c1", "codex");
    // Without a recent override, auto-expansion selects the FIRST active entry, not the last.
    assert_eq!(
        crate::terminals::default_card_terminal(&s, "c1", None),
        Some(first_id.clone())
    );
    // Ended recent entries are ignored in favor of the first active entry.
    assert_eq!(
        crate::terminals::default_card_terminal(&s, "c1", Some("shell-ended")),
        Some(first_id)
    );
    // An active recent entry on the same Card is preserved.
    assert_eq!(
        crate::terminals::default_card_terminal(&s, "c1", Some("agent-second")),
        Some("agent-second".into())
    );
    // A Card with no active terminals does not auto-expand.
    s.attempts[1].observed = Some(json!({"state":"finished"}));
    assert_eq!(
        crate::terminals::default_card_terminal(&s, "c2", None),
        None
    );
}

#[test]
fn manual_terminals_never_participate_in_inactive_and_ephemeral_cli_agents_surface_attention() {
    let mut s = fixture();
    s.terminals = crate::terminals::entries(&s)
        .into_iter()
        .map(|e| e.saved)
        .collect();
    s.terminals.push(TerminalEntry {
        id: "manual-shell".into(),
        conversation_id: None,
        number: 2,
        name: None,
        manual_number: Some(1),
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(3),
        }),
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "ephemeral-cli".into(),
        conversation_id: None,
        number: 3,
        name: None,
        manual_number: Some(2),
        origin_card_id: Some("c1".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-agy".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(0),
        }),
        created_at: 3,
    });

    let rows = crate::terminals::entries(&s);
    let shell = rows.iter().find(|e| e.saved.id == "manual-shell").unwrap();
    assert!(!shell.inactive());
    assert!(!shell.needs_attention());

    let cli = rows.iter().find(|e| e.saved.id == "ephemeral-cli").unwrap();
    assert!(!cli.inactive());
    // Fresh launch at turn 0 does not show false attention.
    assert!(!cli.needs_attention());

    // After finishing turn 1, ephemeral CLI Agent shows attention icon / reason.
    s.terminals
        .last_mut()
        .unwrap()
        .runtime
        .as_mut()
        .unwrap()
        .agent_turn = Some(1);
    let cli = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "ephemeral-cli")
        .unwrap();
    assert!(cli.needs_attention());
    assert!(
        cli.attention_reason()
            .unwrap()
            .contains("waiting for your input")
    );

    // Permission block at any turn shows attention icon / reason.
    let rt = s.terminals.last_mut().unwrap().runtime.as_mut().unwrap();
    rt.agent_turn = Some(0);
    rt.agent_status = Some("blocked".into());
    let cli = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "ephemeral-cli")
        .unwrap();
    assert!(cli.needs_attention());
    assert_eq!(
        cli.attention_reason().as_deref(),
        Some("Agent needs terminal input")
    );
}

#[test]
fn card_agent_actions_open_card_with_expanded_agent_terminal_instead_of_terminals_page() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        s.terminals.push(TerminalEntry {
            id: "c1-shell".into(),
            conversation_id: None,
            number: 2,
            name: None,
            manual_number: Some(1),
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-shell".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
            }),
            created_at: 2,
        });
        let att_id = s.attempts[0].id.clone();
        let conv_id = Conversation::key("c1", "codex");
        ui.snapshot.set(s);
        ui.page.set("board".into());

        // 1. Clicking a running Agent pill (`open_request`) from Board jumps to Card + expanded Agent tab.
        crate::terminals::open_request(ui, &att_id);
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());

        // 2. Clicking the Card Agent action from Diff & Review (`page == "review"`) navigates to Card + Agent tab, never `"terminals"`.
        ui.page.set("review".into());
        ui.card_terminal_open.set(false);
        ui.card_terminal_focused.set(false);
        ui.terminal_recent.with(|m| {
            m.insert("c1".into(), "c1-shell".into());
        });
        crate::terminals::open_card_agent(ui, "c1");
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());

        // 3. Clicking the Card Agent action on a Card with no existing Agent spawns a clean Agent inside the Card split (never jumping to `"terminals"`).
        ui.snapshot
            .update(|s| s.cards.push(serde_json::from_value(json!({"id":"c3","project_id":"p1","number":3,"title":"Fresh card","body":"","stage":"backlog","revision":1,"created_at":3})).unwrap()));
        ui.page.set("board".into());
        crate::terminals::open_card_agent(ui, "c3");
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c3");
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());
        let spawned_tab = ui.card_tab.get_untracked();
        assert!(spawned_tab.starts_with("gallery-agent-"));

        // 4. Stale/ended conversation IDs passed to `open_card_with_terminal` do not overwrite a valid fallback or leave a dead tab open.
        ui.open_card_with_terminal("c1".into(), Some("ended-nonexistent-tab".into()));
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), "c1-shell");
        assert!(ui.card_terminal_open.get_untracked());
    });
}

#[test]
fn closing_terminal_with_non_done_agent_prompts_for_confirmation_while_done_and_shells_close_immediately()
 {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        s.terminals = crate::terminals::entries(&s)
            .into_iter()
            .map(|e| e.saved)
            .collect();
        let conv_c1 = Conversation::key("c1", "codex");
        let conv_c2 = Conversation::key("c2", "codex");

        // Add a plain shell, an ephemeral CLI Agent, and a Workflow Configuration Agent.
        s.terminals.push(TerminalEntry {
            id: "plain-shell".into(),
            conversation_id: None,
            number: 10,
            name: None,
            manual_number: Some(1),
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-plain".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
            }),
            created_at: 10,
        });
        s.terminals.push(TerminalEntry {
            id: "ephemeral-agent".into(),
            conversation_id: None,
            number: 11,
            name: None,
            manual_number: Some(2),
            origin_card_id: Some("c1".into()),
            agent_engine: Some("agy".into()),
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-ephemeral".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
            }),
            created_at: 11,
        });
        s.configuration_sessions.push(ConfigurationSession {
            id: "cfg-session".into(),
            project_id: "p1".into(),
            profile_id: "developer".into(),
            provider_session_id: None,
            generation: 1,
            state: "running".into(),
            prompt: String::new(),
            error: None,
        });
        s.terminals.push(TerminalEntry {
            id: "cfg-session".into(),
            conversation_id: None,
            number: 12,
            name: Some("Workflows · developer".into()),
            manual_number: None,
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-cfg".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("working".into()),
                agent_turn: Some(1),
            }),
            created_at: 12,
        });

        ui.snapshot.set(s);
        ui.terminal_tabs.set(vec![
            conv_c1.clone(),
            conv_c2.clone(),
            "plain-shell".into(),
            "ephemeral-agent".into(),
            "cfg-session".into(),
        ]);
        ui.terminal_selection.set(conv_c1.clone());

        // 1. Plain shell without an Agent closes immediately with no confirmation dialog.
        crate::terminals::close_tab(ui, "plain-shell");
        assert!(ui.terminal_close_confirm.get_untracked().is_none());
        assert!(
            !ui.terminal_tabs
                .get_untracked()
                .iter()
                .any(|t| t == "plain-shell")
        );

        // 2. Managed Card Agent in `"working"`, `"blocked"`, or `"error"` prompts for confirmation.
        for non_done in ["working", "blocked", "error"] {
            ui.snapshot.update(|s| {
                s.attempts[0].connection_state = "connected".into();
                s.attempts[0].observed = Some(json!({"state":"live","status":{"state":non_done}}));
            });
            crate::terminals::close_tab(ui, &conv_c1);
            assert_eq!(
                ui.terminal_close_confirm.get_untracked().as_deref(),
                Some(conv_c1.as_str()),
                "expected confirmation for managed agent status `{non_done}`"
            );
            assert!(ui.has_blocking_overlay());
            assert!(
                ui.terminal_tabs
                    .get_untracked()
                    .iter()
                    .any(|t| t == &conv_c1)
            );
            ui.terminal_close_confirm.set(None);
        }

        // Confirming `close_entry_confirmed` dismisses the modal and closes the running Agent tab.
        crate::terminals::close_tab(ui, &conv_c1);
        assert_eq!(
            ui.terminal_close_confirm.get_untracked().as_deref(),
            Some(conv_c1.as_str())
        );
        crate::terminals::close_entry_confirmed(ui, &conv_c1);
        assert!(ui.terminal_close_confirm.get_untracked().is_none());
        assert!(
            !ui.terminal_tabs
                .get_untracked()
                .iter()
                .any(|t| t == &conv_c1)
        );

        // 3. Managed Card Agent in `"done"` or `"idle"` (even with older `observed_at`) closes immediately!
        for finished_turn in ["done", "idle"] {
            ui.snapshot.update(|s| {
                s.attempts[1].connection_state = "connected".into();
                s.attempts[1].lifecycle.observed_at = Some(0);
                s.attempts[1].observed =
                    Some(json!({"state":"live","status":{"state":finished_turn}}));
            });
            let entry = crate::terminals::entries(&ui.snapshot.get_untracked())
                .into_iter()
                .find(|e| e.saved.id == conv_c2)
                .unwrap();
            assert_eq!(
                entry.close_confirm_reason(&ui.snapshot.get_untracked()),
                None,
                "status `{finished_turn}` must not require close confirmation"
            );
        }
        crate::terminals::close_tab(ui, &conv_c2);
        assert!(ui.terminal_close_confirm.get_untracked().is_none());
        assert!(
            !ui.terminal_tabs
                .get_untracked()
                .iter()
                .any(|t| t == &conv_c2)
        );

        // 4. Ephemeral CLI Agent (`agent_engine = Some("agy")`) requires confirmation before `"done"`/`"idle"`,
        //    and closes immediately once it reports `"done"`.
        crate::terminals::close_entry(ui, "ephemeral-agent");
        assert_eq!(
            ui.terminal_close_confirm.get_untracked().as_deref(),
            Some("ephemeral-agent")
        );
        ui.terminal_close_confirm.set(None);

        ui.snapshot.update(|s| {
            let t = s
                .terminals
                .iter_mut()
                .find(|t| t.id == "ephemeral-agent")
                .unwrap();
            t.runtime.as_mut().unwrap().agent_status = Some("done".into());
        });
        crate::terminals::close_tab(ui, "ephemeral-agent");
        assert!(ui.terminal_close_confirm.get_untracked().is_none());
        assert!(
            !ui.terminal_tabs
                .get_untracked()
                .iter()
                .any(|t| t == "ephemeral-agent")
        );

        // 5. Manual Agent launched inside a plain shell (`agent_engine = None`, `agent_status = Some("working")`)
        //    also prompts before terminating, while `"done"` closes immediately.
        ui.snapshot.update(|s| {
            s.terminals.push(TerminalEntry {
                id: "shell-with-manual-agent".into(),
                conversation_id: None,
                number: 13,
                name: None,
                manual_number: Some(3),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: None,
                    core_id: Some("core".into()),
                    terminal: Some("pty-manual-agent".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: Some("working".into()),
                    agent_turn: Some(2),
                }),
                created_at: 13,
            });
        });
        crate::terminals::close_entry(ui, "shell-with-manual-agent");
        assert_eq!(
            ui.terminal_close_confirm.get_untracked().as_deref(),
            Some("shell-with-manual-agent")
        );
        ui.terminal_close_confirm.set(None);

        // 6. Project Workflow Configuration Agent prompts when working and closes immediately when `"done"`.
        crate::terminals::close_tab(ui, "cfg-session");
        assert_eq!(
            ui.terminal_close_confirm.get_untracked().as_deref(),
            Some("cfg-session")
        );
        ui.terminal_close_confirm.set(None);
        ui.snapshot.update(|s| {
            let t = s
                .terminals
                .iter_mut()
                .find(|t| t.id == "cfg-session")
                .unwrap();
            t.runtime.as_mut().unwrap().agent_status = Some("done".into());
        });
        crate::terminals::close_tab(ui, "cfg-session");
        assert!(ui.terminal_close_confirm.get_untracked().is_none());
        assert!(
            !ui.terminal_tabs
                .get_untracked()
                .iter()
                .any(|t| t == "cfg-session")
        );
    });
}

#[test]
fn optimistic_close_removes_tab_immediately_and_rolls_back_on_unknown_or_error() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.terminals.extend([
            TerminalEntry {
                id: "term-1".into(),
                conversation_id: None,
                number: 21,
                name: None,
                manual_number: Some(1),
                origin_card_id: Some("c1".into()),
                agent_engine: None,
                runtime: Some(TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: Some("core-1".into()),
                    core_id: Some("core-1".into()),
                    terminal: Some("pty-1".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                }),
                created_at: 21,
            },
            TerminalEntry {
                id: "term-2".into(),
                conversation_id: None,
                number: 22,
                name: None,
                manual_number: Some(2),
                origin_card_id: Some("c1".into()),
                agent_engine: None,
                runtime: Some(TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: Some("core-1".into()),
                    core_id: Some("core-1".into()),
                    terminal: Some("pty-2".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                }),
                created_at: 22,
            },
        ]);
        s.terminals = crate::terminals::entries(&s)
            .into_iter()
            .map(|e| e.saved)
            .collect();

        let ui = crate::presenter::Ui::new();
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.terminal_tabs.set(vec!["term-1".into(), "term-2".into()]);
        ui.terminal_selection.set("term-1".into());
        ui.page.set("terminals".into());

        // 1. Optimistic close removes the tab and switches selection in 0ms without blocking `ui.pending`.
        let ctx1 = crate::terminals::begin_optimistic_close(ui, "term-1").unwrap();
        assert!(!ui.pending.get_untracked());
        assert!(
            ui.closing_terminals
                .with_untracked(|c| c.contains("term-1"))
        );
        assert_eq!(ui.terminal_tabs.get_untracked(), vec!["term-2"]);
        assert_eq!(ui.terminal_selection.get_untracked(), "term-2");
        assert!(
            !crate::terminals::card_live_entries_for_ui(ui, "c1")
                .iter()
                .any(|e| e.saved.id == "term-1")
        );
        assert!(
            crate::terminals::card_live_entries_for_ui(ui, "c1")
                .iter()
                .any(|e| e.saved.id == "term-2")
        );

        // 2. If backend reports `"unknown"` (e.g. Core unavailable/replaced), tab rolls back & shows error!
        crate::terminals::finish_optimistic_close(
            ui,
            &ctx1,
            Ok(json!({"state":"unknown","message":"Core replaced"})),
        );
        assert!(
            !ui.closing_terminals
                .with_untracked(|c| c.contains("term-1"))
        );
        assert_eq!(
            ui.terminal_tabs.get_untracked(),
            vec!["term-1", "term-2"],
            "rolled-back tab must return to its original tab position"
        );
        assert!(ui.operation_error.get_untracked().contains("Core replaced"));

        // 3. Successful background close marks entry finished and clears `closing_terminals`.
        ui.operation_error.set(String::new());
        let ctx2 = crate::terminals::begin_optimistic_close(ui, "term-1").unwrap();
        assert_eq!(ui.terminal_tabs.get_untracked(), vec!["term-2"]);
        crate::terminals::finish_optimistic_close(ui, &ctx2, Ok(json!({"state":"closed"})));
        assert!(
            !ui.closing_terminals
                .with_untracked(|c| c.contains("term-1"))
        );
        assert_eq!(ui.terminal_tabs.get_untracked(), vec!["term-2"]);
        assert!(crate::terminals::current(ui).is_some_and(|e| e.saved.id == "term-2"));
        assert!(ui.terminal_entries.with_untracked(|rows| {
            rows.iter()
                .find(|e| e.saved.id == "term-1")
                .unwrap()
                .ended()
        }));
    });
}
