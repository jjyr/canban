use super::*;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (idx, (project, card)) in [("p1", "c1"), ("p2", "c2")].into_iter().enumerate() {
        s.projects.push(serde_json::from_value(json!({"id":project,"name":project,"directory":"/tmp/project","workflow":Workflow::default(),"created_at":1})).unwrap());
        s.cards.push(serde_json::from_value(json!({"id":card,"project_id":project,"number":1,"title":"Same title","body":"","stage":"develop","revision":1,"created_at":1})).unwrap());
        let profile = Config::default().profiles[0].clone();
        let tid = Conversation::key(card, &profile.id);
        let mut a = canban_scheduler::agent_run(
            project,
            card,
            1,
            "develop",
            "interactive",
            profile,
            "task".into(),
            None,
        );
        a.updated_at = 1;
        a.state = "running".into();
        a.core_id = Some("core".into());
        a.terminal = Some(format!("t-{card}"));
        a.cwd = "/tmp".into();
        a.input_path = "input".into();
        a.observed = Some(json!({"state":"live","status":{"state":"working"}}));
        a.provider_session_id = Some(format!("provider-{card}"));
        s.terminals.push(TerminalEntry {
            id: tid.clone(),
            conversation_id: Some(tid),
            number: idx as u64 + 1,
            name: None,
            origin_project_id: None,
            origin_card_id: Some(card.into()),
            agent_engine: None,
            runtime: None,
            agent: Some(a),
            created_at: 1,
        });
    }
    s
}
#[test]
fn tree_keeps_projects_and_profiles_distinct_and_excludes_unassigned_recovery() {
    let mut s = fixture();
    let mut att = s.terminals[0].agent.clone().unwrap();
    att.profile.id = "reviewer".into();
    att.state = "queued".into();
    att.terminal = None;
    att.observed = None;
    let rev_id = Conversation::key("c1", "reviewer");
    s.terminals.push(TerminalEntry {
        id: rev_id.clone(),
        conversation_id: Some(rev_id),
        number: 3,
        name: None,
        origin_project_id: None,
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: None,
        agent: Some(att),
        created_at: 1,
    });
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
    let key = Conversation::key("c1", &Config::default().profiles[0].id);
    s.terminals[0].agent.as_mut().unwrap().pending = Some(QueuedAssignment {
        stage: "develop".into(),
        trigger: "comment".into(),
        profile: s.terminals[0].agent.as_ref().unwrap().profile.clone(),
        instruction: "followup".into(),
        on_completed: None,
        require_human_review: false,
        workflow_run_id: None,
        workflow_step: None,
        queued_at: 2,
    });
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    assert_eq!(row.agent().unwrap().number, 1);
    assert_eq!(binding(&row).unwrap()["terminal"], "t-c1");
    let run = s.terminals[0].agent.as_mut().unwrap();
    run.observed = Some(json!({"state":"finished"}));
    run.promote_pending();
    run.number = 2;
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    assert_eq!(row.agent().unwrap().number, 2);
    assert!(!closed(&row));
    assert!(binding(&row).is_none());
    let f = s.terminals[0].agent.as_mut().unwrap();
    f.state = "running".into();
    f.core_id = Some("core".into());
    f.connection_state = "connected".into();
    f.terminal = Some("new-terminal".into());
    f.observed = Some(json!({"state":"live"}));
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    assert_eq!(binding(&row).unwrap()["terminal"], "new-terminal");
    assert_eq!(row.id, key);
}
#[test]
fn closed_archived_and_missing_bindings_never_create_viewers_or_implicit_resume() {
    let mut s = fixture();
    s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state":"finished"}));
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
    s.terminals[1].agent.as_mut().unwrap().core_id = None;
    let row = conversations(&s)
        .into_iter()
        .find(|r| r.card.id == "c2")
        .unwrap();
    assert!(binding(&row).is_none());
    assert_eq!(s.terminals.len(), 2);
}
#[test]
fn unified_entries_keep_numeric_names_across_agent_resume_and_shell_lifetimes() {
    let mut s = fixture();
    let key = Conversation::key("c1", &Config::default().profiles[0].id);
    let label = s.terminals.iter().find(|t| t.id == key).unwrap().number;
    s.terminals.push(TerminalEntry {
        id: "manual".into(),
        conversation_id: None,
        number: 9,
        name: Some("Server".into()),
        origin_project_id: None,
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 0,
    });
    let rows = crate::terminals::entries(&s);
    let shell = rows.iter().find(|e| e.saved.id == "manual").unwrap();
    assert_eq!(shell.saved.title(), "Server");
    assert!(shell.card.is_none());
    assert!(shell.agent.is_none());
    assert_eq!(shell.binding().unwrap()["terminal"], "shell-pty");
    s.terminals[0].agent.as_mut().unwrap().terminal = Some("resumed-pty".into());
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
    assert_eq!(s.terminals.len(), 3);
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
    let agent = s.terminals[0].id.clone();
    let shell = TerminalEntry {
        id: "shell".into(),
        conversation_id: None,
        number: 9,
        name: None,
        origin_project_id: None,
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
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
            ..Default::default()
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
    let key = Conversation::key("c1", "developer");
    s.terminals[0].id = key.clone();
    s.terminals[0].conversation_id = Some(key.clone());
    s.terminals[0].agent.as_mut().unwrap().profile.id = "developer".into();
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
    s.terminals[0].name = Some("My review desk".into());
    let named = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(named.title(), "My review desk");
    assert_eq!(named.binding(), binding);
    s.terminals[0].name = None;
    s.terminals[0].agent.as_mut().unwrap().profile.argv = Some(vec!["/opt/bin/claude".into()]);
    let entry = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(entry.provider().unwrap().0, Icon::Claude);
    let prof = &mut s.terminals[0].agent.as_mut().unwrap().profile;
    prof.preset = None;
    prof.argv = Some(vec!["my-agent".into()]);
    let entry = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == key)
        .unwrap();
    assert_eq!(entry.provider(), Some((Icon::Agent, "my-agent".into())));
}

#[test]
fn input_gating_has_a_specific_lifecycle_label_and_keeps_binding_identity() {
    let mut s = fixture();
    let key = Conversation::key("c1", &Config::default().profiles[0].id);
    for (connection, close, label) in [
        ("connecting", "", "Connecting"),
        ("unavailable", "", "Disconnected"),
        ("connected", "closing", "Closing"),
        ("connected", "pending", "Closing"),
        ("connected", "", ""),
    ] {
        let a = s.terminals[0].agent.as_mut().unwrap();
        a.connection_state = connection.into();
        a.close_state = close.into();
        let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
        let b = binding(&row).unwrap();
        assert_eq!(b["input_status"].as_str().unwrap(), label);
        assert_eq!(b["read_only"], !label.is_empty());
        assert_eq!(
            b["terminal"],
            s.terminals[0]
                .agent
                .as_ref()
                .unwrap()
                .terminal
                .as_deref()
                .unwrap()
        );
    }
}

#[test]
fn inactive_is_a_live_unattended_conversation_and_closed_entries_leave_inventory() {
    let mut s = fixture();
    {
        let a = s.terminals[0].agent.as_mut().unwrap();
        a.state = "completed".into();
        a.observed = Some(json!({"state":"live","status":{"state":"done"}}));
        a.connection_state = "connected".into();
        a.lifecycle.observed_at = Some(now());
    }
    let entry = |s: &Snapshot| {
        super::super::entries(s)
            .into_iter()
            .find(|e| e.card.as_ref().is_some_and(|c| c.id == "c1"))
            .unwrap()
    };
    assert!(entry(&s).listed());
    assert!(entry(&s).inactive());
    // Immediate state update: recent interaction does not pin a finished idle Agent as active.
    s.terminals[0]
        .agent
        .as_mut()
        .unwrap()
        .lifecycle
        .last_interaction_at = Some(now());
    assert!(entry(&s).inactive());
    assert!(!entry(&s).needs_attention());
    // Unfinished assignment sitting idle requires user inspection instead of silent inactive state.
    s.terminals[0].agent.as_mut().unwrap().state = "running".into();
    assert!(entry(&s).needs_attention());
    assert!(!entry(&s).inactive());
    s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state":"finished"}));
    assert!(!entry(&s).listed());
    s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state":"live"}));
    s.cards[0].archived_at = Some(now());
    assert!(!entry(&s).listed());
}

#[test]
fn card_destination_requires_choice_and_remembers_only_live_selection() {
    use crate::terminals::{entries, navigation::direct};
    let mut s = fixture();
    let mut a = s.terminals[0].agent.clone().unwrap();
    a.profile.id = "reviewer".into();
    a.terminal = Some("second-pty".into());
    let preferred = Conversation::key("c1", "reviewer");
    s.terminals.push(TerminalEntry {
        id: preferred.clone(),
        conversation_id: Some(preferred.clone()),
        number: 3,
        name: None,
        origin_project_id: None,
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: None,
        agent: Some(a),
        created_at: 2,
    });
    let card_entries = |s: &Snapshot| {
        entries(s)
            .into_iter()
            .filter(|e| e.card.as_ref().is_some_and(|c| c.id == "c1"))
            .collect::<Vec<_>>()
    };
    let rows = card_entries(&s);
    assert_eq!(direct(&rows, None), None);
    assert_eq!(direct(&rows, Some(&preferred)), Some(preferred.clone()));
    // Disconnection retains the original target; exit invalidates the preference.
    s.terminals
        .last_mut()
        .unwrap()
        .agent
        .as_mut()
        .unwrap()
        .connection_state = "unavailable".into();
    assert_eq!(
        direct(&card_entries(&s), Some(&preferred)),
        Some(preferred.clone())
    );
    s.terminals
        .last_mut()
        .unwrap()
        .agent
        .as_mut()
        .unwrap()
        .observed = Some(json!({"state":"finished"}));
    assert_eq!(
        direct(&card_entries(&s), Some(&preferred)),
        Some(Conversation::key("c1", &Config::default().profiles[0].id))
    );
    s.terminals[0].agent.as_mut().unwrap().terminal = None;
    assert_eq!(direct(&card_entries(&s), Some(&preferred)), None);
}

#[test]
fn closed_terminal_never_attaches_until_reopened_in_place() {
    use crate::terminals::navigation::live_request;
    let mut s = fixture();
    let original = s.terminals[0].id.clone();
    assert!(live_request(&s, &original).is_some());
    s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state":"finished"}));
    assert_eq!(live_request(&s, &original), None);
    {
        let a = s.terminals[0].agent.as_mut().unwrap();
        a.number = 2;
        a.updated_at = 2;
        a.observed = Some(json!({"state":"live","status":{"state":"working"}}));
    }
    assert!(live_request(&s, &original).is_some());
    assert_eq!(live_request(&s, "removed-terminal"), None);
}

#[test]
fn card_live_entries_unify_workflow_agents_ephemeral_cli_agents_and_card_shells() {
    use canban_ui_kit::Icon;
    let mut s = fixture();
    let prof_id = Config::default().profiles[0].id.clone();
    s.terminals.push(TerminalEntry {
        id: "agent-ephemeral".into(),
        conversation_id: None,
        number: 3,
        name: Some("Antigravity 1".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-backlog".into(),
        conversation_id: None,
        number: 4,
        name: Some("Terminal 2".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 3,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 5,
        name: Some("Terminal 3".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 4,
    });

    let live = crate::terminals::card_live_entries(&s, "c1");
    let ids: Vec<_> = live.iter().map(|e| e.saved.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            Conversation::key("c1", &prof_id).as_str(),
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
        ..Default::default()
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
    let prof_id = Config::default().profiles[0].id.clone();
    s.terminals.push(TerminalEntry {
        id: "agent-second".into(),
        conversation_id: None,
        number: 3,
        name: Some("Antigravity 1".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 4,
        name: Some("Terminal 2".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 3,
    });

    let first_id = Conversation::key("c1", &prof_id);
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
    s.terminals[1].agent.as_mut().unwrap().observed = Some(json!({"state":"finished"}));
    assert_eq!(
        crate::terminals::default_card_terminal(&s, "c2", None),
        None
    );
}

#[test]
fn manual_terminals_never_participate_in_inactive_and_ephemeral_cli_agents_surface_attention() {
    let mut s = fixture();
    s.terminals.push(TerminalEntry {
        id: "manual-shell".into(),
        conversation_id: None,
        number: 3,
        name: Some("Terminal 1".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "ephemeral-cli".into(),
        conversation_id: None,
        number: 4,
        name: Some("Antigravity 2".into()),
        origin_project_id: Some("p1".into()),
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
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 3,
    });

    let rows = crate::terminals::entries(&s);
    let shell = rows.iter().find(|e| e.saved.id == "manual-shell").unwrap();
    assert!(!shell.inactive());
    assert!(!shell.needs_attention());
    assert_eq!(shell.herdr_status(), HerdrStatus::PlainShell);

    let cli = rows.iter().find(|e| e.saved.id == "ephemeral-cli").unwrap();
    assert!(!cli.inactive());
    // Fresh launch at turn 0 is `Idle` and does not show false attention.
    assert!(!cli.needs_attention());
    assert_eq!(cli.herdr_status(), HerdrStatus::Idle);
    assert_eq!(cli.display_status(), "Idle");

    // After finishing turn 1, ephemeral CLI Agent shows positive `Done` status (not amber Needs attention).
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
    assert!(!cli.needs_attention());
    assert_eq!(cli.herdr_status(), HerdrStatus::Done);
    assert_eq!(cli.display_status(), "Done");

    // Permission block at any turn shows attention icon / reason (including RuntimeWait details when present).
    let rt = s.terminals.last_mut().unwrap().runtime.as_mut().unwrap();
    rt.agent_turn = Some(0);
    rt.agent_status = Some("blocked".into());
    let cli = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "ephemeral-cli")
        .unwrap();
    assert!(cli.needs_attention());
    assert_eq!(cli.herdr_status(), HerdrStatus::Blocked);
    assert_eq!(
        cli.attention_reason().as_deref(),
        Some("Agent needs terminal input")
    );

    // Manual shell (`agent_engine: None`) where user ran `claude` (`observed_agent: Some("claude")`)
    // also surfaces `Working`, `Blocked` (with RuntimeWait), and `Done` uniformly!
    let shell_rt = s
        .terminals
        .iter_mut()
        .find(|t| t.id == "manual-shell")
        .unwrap()
        .runtime
        .as_mut()
        .unwrap();
    shell_rt.observed_agent = Some("claude".into());
    shell_rt.agent_status = Some("blocked".into());
    shell_rt.waiting = Some(RuntimeWait {
        id: "w-shell".into(),
        kind: "permission".into(),
        title: "Allow bash?".into(),
        message: "cargo test".into(),
        since: 1,
    });
    let shell = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "manual-shell")
        .unwrap();
    assert_eq!(shell.herdr_status(), HerdrStatus::Blocked);
    assert!(shell.needs_attention());
    assert_eq!(
        shell.attention_reason().as_deref(),
        Some("Permission required: Allow bash? · cargo test")
    );
}

#[test]
fn card_open_collapses_terminal_by_default_while_terminal_and_pill_entries_expand_split() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        let prof_id = Config::default().profiles[0].id.clone();
        // Place the manual shell ahead of the Agent in terminal creation order so
        // first-live fallback (`c1-shell`) is distinguishable from explicit Agent selection (`c1:developer`).
        s.terminals[0].number = 2;
        s.terminals.push(TerminalEntry {
            id: "c1-shell".into(),
            conversation_id: None,
            number: 1,
            name: Some("Terminal 1".into()),
            origin_project_id: Some("p1".into()),
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
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: 2,
        });
        let att_id = s.terminals[0].id.clone();
        let conv_id = Conversation::key("c1", &prof_id);
        ui.snapshot.set(s);
        ui.page.set("board".into());

        // 0. Clicking the Card body (`open_card`) keeps the Terminal split collapsed (`false`),
        // even when active terminals/agents exist, while pre-selecting the default live tab.
        ui.open_card("c1".into());
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), "c1-shell");
        assert!(!ui.card_terminal_open.get_untracked());
        assert!(!ui.card_terminal_focused.get_untracked());

        // Toggling (`[>_ Terminal]` / ⌘J) expands and collapses; reopening the Card resets to collapsed (`false`).
        crate::card::terminal::toggle(ui);
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());
        ui.close_card();
        assert!(!ui.card_terminal_open.get_untracked());
        ui.open_card("c1".into());
        assert!(!ui.card_terminal_open.get_untracked());

        // 1. Clicking a running Agent pill (`open_request`) from Board jumps to Card + expanded Agent tab.
        ui.page.set("board".into());
        crate::terminals::open_request(ui, &att_id);
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());

        // 2. Back/Forward history navigation (`navigation::go`) also keeps the Card Terminal split collapsed.
        ui.history.update(|h| {
            h.entries.clear();
            h.index = 0;
            h.visit(ui.location());
        });
        ui.navigate("board");
        ui.history.update(|h| h.visit(ui.location()));
        crate::navigation::go(ui, false);
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert!(!ui.card_terminal_open.get_untracked());
        assert!(!ui.card_terminal_focused.get_untracked());

        // 3. Clicking the Card `[>_ Terminal]` action from Board or Diff & Review (`open_card_quick`) navigates to Card + expanded Terminal split, never `"terminals"`.
        ui.page.set("review".into());
        ui.card_terminal_open.set(false);
        ui.card_terminal_focused.set(false);
        ui.terminal_recent.with(|m| {
            m.insert("c1".into(), "c1-shell".into());
        });
        crate::terminals::open_card_quick(ui, "c1");
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), "c1-shell");
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());

        // 4. Clicking `[>_ Terminal]` on a Card with no existing Terminal spawns a Card Shell Terminal inside the Card split (never jumping to `"terminals"`).
        ui.snapshot
            .update(|s| s.cards.push(serde_json::from_value(json!({"id":"c3","project_id":"p1","number":3,"title":"Fresh card","body":"","stage":"backlog","revision":1,"created_at":3})).unwrap()));
        ui.page.set("board".into());
        crate::terminals::open_card_quick(ui, "c3");
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c3");
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());
        let spawned_tab = ui.card_tab.get_untracked();
        assert!(spawned_tab.starts_with("gallery-shell-"));

        // 5. Stale/ended conversation IDs passed to `open_card_with_terminal` do not overwrite a valid fallback or leave a dead tab open.
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
        let prof_id = Config::default().profiles[0].id.clone();
        let conv_c1 = Conversation::key("c1", &prof_id);
        let conv_c2 = Conversation::key("c2", &prof_id);

        // Add a plain shell, an ephemeral CLI Agent, and a Workflow Configuration Agent.
        s.terminals.push(TerminalEntry {
            id: "plain-shell".into(),
            conversation_id: None,
            number: 10,
            name: Some("Terminal 1".into()),
            origin_project_id: Some("p1".into()),
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
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: 10,
        });
        s.terminals.push(TerminalEntry {
            id: "ephemeral-agent".into(),
            conversation_id: None,
            number: 11,
            name: Some("Antigravity 1".into()),
            origin_project_id: Some("p1".into()),
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
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
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
            origin_project_id: Some("p1".into()),
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
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
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
                let a = s.terminals[0].agent.as_mut().unwrap();
                a.connection_state = "connected".into();
                a.observed = Some(json!({"state":"live","status":{"state":non_done}}));
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
                let a = s.terminals[1].agent.as_mut().unwrap();
                a.connection_state = "connected".into();
                a.lifecycle.observed_at = Some(0);
                a.observed = Some(json!({"state":"live","status":{"state":finished_turn}}));
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
                name: Some("Terminal 3".into()),
                origin_project_id: None,
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
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
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
        s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state":"finished"}));
        s.terminals.extend([
            TerminalEntry {
                id: "term-1".into(),
                conversation_id: None,
                number: 21,
                name: Some("Terminal 1".into()),
                origin_project_id: Some("p1".into()),
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
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
                created_at: 21,
            },
            TerminalEntry {
                id: "term-2".into(),
                conversation_id: None,
                number: 22,
                name: Some("Terminal 2".into()),
                origin_project_id: Some("p1".into()),
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
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
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

#[test]
fn worktree_scopes_isolate_tabs_and_cycle_hide_empty_branches_and_regroup_on_cd() {
    use crate::terminals::worktrees::{
        WorktreeScope, entry_scope, format_git_indicators, project_scopes_in_tree_order,
    };

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[0].target_branch = Some("main".into());
        s.cards[0].worktree = Some("/repo/p1-worktrees/c1".into());
        s.cards[0].branch = Some("canban/1".into());
        s.projects.push(
            serde_json::from_value(json!({
                "id": "p-empty",
                "name": "Empty Project",
                "directory": "/repo/empty",
                "workflow": Workflow::default(),
                "created_at": 2
            }))
            .unwrap(),
        );
        s.worktrees.push(ManagedWorktree {
            id: "wt-active".into(),
            project_id: "p1".into(),
            branch: "feat/active".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/repo/p1-worktrees/feat-active".into(),
            location: WorkLocation {
                machine: "local".into(),
                directory: "/repo/p1-worktrees/feat-active".into(),
            },
            created_at: 3,
        });
        s.worktrees.push(ManagedWorktree {
            id: "wt-idle".into(),
            project_id: "p1".into(),
            branch: "chore/idle".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/repo/p1-worktrees/chore-idle".into(),
            location: WorkLocation {
                machine: "local".into(),
                directory: "/repo/p1-worktrees/chore-idle".into(),
            },
            created_at: 4,
        });

        let mk_shell = |id: &str, num: u64, origin: Option<&str>| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: num,
            name: Some(format!("Terminal {num}")),
            origin_project_id: Some("p1".into()),
            origin_card_id: origin.map(str::to_owned),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{id}")),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: num,
        };

        s.terminals.extend([
            mk_shell("root-1", 10, None),
            mk_shell("root-2", 11, None),
            mk_shell("card-shell", 12, Some("c1")),
            mk_shell("wt-shell", 13, None),
        ]);
        s.terminal_contexts.extend([
            TerminalContext {
                machine: "local".into(),
                terminal_id: "root-1".into(),
                core_id: "core".into(),
                terminal: "pty-root-1".into(),
                cwd: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                dirty: true,
                ahead: 2,
                behind: 1,
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "root-2".into(),
                core_id: "core".into(),
                terminal: "pty-root-2".into(),
                cwd: Some("/repo/p1/crates".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "card-shell".into(),
                core_id: "core".into(),
                terminal: "pty-card-shell".into(),
                cwd: Some("/repo/p1-worktrees/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                branch: Some("canban/1".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "wt-shell".into(),
                core_id: "core".into(),
                terminal: "pty-wt-shell".into(),
                cwd: Some("/repo/p1-worktrees/feat-active".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt-active".into()),
                branch: Some("feat/active".into()),
                ..Default::default()
            },
        ]);

        assert_eq!(
            format_git_indicators(true, 2, 1),
            vec!["changed", "↑2", "↓1"]
        );
        assert!(format_git_indicators(false, 0, 0).is_empty());

        let ui = crate::presenter::Ui::new();
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());

        // 1. Left tree hides `wt-idle` and `p-empty` children completely.
        let snap = ui.snapshot.get_untracked();
        let live: Vec<_> = crate::terminals::entries(&snap)
            .into_iter()
            .filter(|e| e.listed())
            .collect();
        let p1_scopes = project_scopes_in_tree_order(&snap, &live, Some("p1"));
        let p1_scope_keys: Vec<_> = p1_scopes.iter().map(|(sc, _)| sc.clone()).collect();
        assert_eq!(
            p1_scope_keys,
            vec![
                WorktreeScope::ProjectRoot("p1".into()),
                WorktreeScope::CardWorktree("c1".into()),
                WorktreeScope::ManagedWorktree("wt-active".into()),
            ]
        );
        assert!(project_scopes_in_tree_order(&snap, &live, Some("p-empty")).is_empty());
        assert!(project_scopes_in_tree_order(&snap, &live, None).is_empty());

        // 2. Selecting `root-1` shows ONLY `root-1` & `root-2` in `scope_tabs` and `cycle`.
        ui.open_terminal("root-1".into());
        let tab_ids: Vec<_> = crate::terminals::scope_tabs(ui)
            .into_iter()
            .map(|e| e.saved.id)
            .collect();
        assert_eq!(tab_ids, vec!["root-1", "root-2"]);
        crate::terminals::cycle(ui, true);
        assert_eq!(ui.terminal_selection.get_untracked(), "root-2");
        crate::terminals::cycle(ui, true);
        assert_eq!(ui.terminal_selection.get_untracked(), "root-1");

        // 3. When `card-shell` (opened with `origin_card_id = Some("c1")`) `cd`s into ProjectRoot `/repo/p1`,
        //    it dynamically regroups from `CardWorktree("c1")` into `ProjectRoot("p1")`!
        ui.open_terminal("card-shell".into());
        let conv_c1 = Conversation::key("c1", &Config::default().profiles[0].id);
        let card_tabs: Vec<_> = crate::terminals::scope_tabs(ui)
            .into_iter()
            .map(|e| e.saved.id)
            .collect();
        assert_eq!(card_tabs, vec![conv_c1.clone(), "card-shell".into()]);

        ui.snapshot.update(|s| {
            let ctx = s
                .terminal_contexts
                .iter_mut()
                .find(|c| c.terminal_id == "card-shell")
                .unwrap();
            ctx.cwd = Some("/repo/p1".into());
            ctx.card_id = None;
            ctx.branch = Some("main".into());
        });
        let snap_after = ui.snapshot.get_untracked();
        let moved = crate::terminals::entries(&snap_after)
            .into_iter()
            .find(|e| e.saved.id == "card-shell")
            .unwrap();
        assert_eq!(
            entry_scope(&snap_after, &moved),
            WorktreeScope::ProjectRoot("p1".into())
        );
        let root_tabs_after: Vec<_> = crate::terminals::scope_tabs(ui)
            .into_iter()
            .map(|e| e.saved.id)
            .collect();
        assert_eq!(root_tabs_after, vec!["root-1", "root-2", "card-shell"]);
    });
}

#[test]
fn close_terminal_fallback_prefers_same_worktree_prev_then_same_project_prev_then_global_prev() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
        // Finish the default workflow agent on c1 so we control exact shells in p1; keep p2's agent alive!
        s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state": "finished"}));
        let p2_agent_id = s.terminals[1].id.clone();

        let mk = |id: &str, num: u64| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: num,
            name: Some(format!("Terminal {num}")),
            origin_project_id: Some("p1".into()),
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{id}")),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: num,
        };
        s.terminals.extend([
            mk("r1", 10),
            mk("c1-a", 11),
            mk("c1-b", 12),
            mk("c1-c", 13),
            mk("wt1", 14),
        ]);
        s.worktrees.push(ManagedWorktree {
            id: "wt-1".into(),
            project_id: "p1".into(),
            branch: "feat/x".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/repo/p1-wt/feat-x".into(),
            location: WorkLocation {
                machine: "local".into(),
                directory: "/repo/p1-wt/feat-x".into(),
            },
            created_at: 1,
        });
        s.terminal_contexts.extend([
            TerminalContext {
                machine: "local".into(),
                terminal_id: "r1".into(),
                core_id: "core".into(),
                terminal: "pty-r1".into(),
                cwd: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "c1-a".into(),
                core_id: "core".into(),
                terminal: "pty-c1-a".into(),
                cwd: Some("/repo/p1-wt/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "c1-b".into(),
                core_id: "core".into(),
                terminal: "pty-c1-b".into(),
                cwd: Some("/repo/p1-wt/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "c1-c".into(),
                core_id: "core".into(),
                terminal: "pty-c1-c".into(),
                cwd: Some("/repo/p1-wt/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "wt1".into(),
                core_id: "core".into(),
                terminal: "pty-wt1".into(),
                cwd: Some("/repo/p1-wt/feat-x".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt-1".into()),
                ..Default::default()
            },
        ]);

        let ui = crate::presenter::Ui::new();
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.page.set("terminals".into());

        // 1. Closing middle terminal `c1-b` -> jumps to PREVIOUS in same worktree (`c1-a`).
        ui.open_terminal("c1-b".into());
        crate::terminals::close_tab(ui, "c1-b");
        assert_eq!(ui.terminal_selection.get_untracked(), "c1-a");

        // 2. Closing first terminal `c1-a` -> no previous in worktree, jumps to NEXT in same worktree (`c1-c`).
        crate::terminals::close_tab(ui, "c1-a");
        assert_eq!(ui.terminal_selection.get_untracked(), "c1-c");

        // 3. Closing last terminal in `CardWorktree("c1")` (`c1-c`) -> jumps to PREVIOUS in same Project (`r1`, before `c1` in tree order, rather than `wt1` after it).
        crate::terminals::close_tab(ui, "c1-c");
        assert_eq!(ui.terminal_selection.get_untracked(), "r1");

        // 4. Closing first terminal in Project (`r1`) -> no previous in Project, jumps to NEXT in same Project (`wt1`).
        crate::terminals::close_tab(ui, "r1");
        assert_eq!(ui.terminal_selection.get_untracked(), "wt1");

        // 5. Closing the final terminal in `p1` (`wt1`) -> jumps globally to `p2`'s live terminal (`p2_agent_id`)!
        crate::terminals::close_tab(ui, "wt1");
        assert_eq!(ui.terminal_selection.get_untracked(), p2_agent_id);
        assert_eq!(ui.project.get_untracked(), "p2");

        // 6. Add an Unassigned ("Other") terminal after p2; closing it jumps back to PREVIOUS global terminal (`p2_agent_id`),
        //    and closing the last global terminal enters Empty State (`""`).
        let mut ext = mk("ext-1", 30);
        ext.origin_project_id = None;
        ui.snapshot.update(|s| s.terminals.push(ext));
        ui.open_terminal("ext-1".into());
        crate::terminals::close_tab(ui, "ext-1");
        assert_eq!(ui.terminal_selection.get_untracked(), p2_agent_id);

        crate::terminals::close_entry_confirmed(ui, &p2_agent_id);
        assert_eq!(ui.terminal_selection.get_untracked(), "");
    });
}

#[test]
fn global_new_terminal_opens_in_project_root_while_in_page_new_tab_inherits_current_worktree() {
    use crate::terminals::worktrees::{WorktreeScope, entry_scope};

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[0].target_branch = Some("main".into());
        s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
        s.cards[0].branch = Some("canban/1".into());

        let ui = crate::presenter::Ui::new();
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());

        // Select Card c1's agent terminal first on Terminals page, then run in-page `new_tab`:
        let conv_c1 = Conversation::key("c1", &Config::default().profiles[0].id);
        ui.open_terminal(conv_c1);
        crate::commands::execute(ui, "new-terminal");
        let in_page_id = ui.terminal_selection.get_untracked();
        let snap1 = ui.snapshot.get_untracked();
        let in_page_entry = crate::terminals::entries(&snap1)
            .into_iter()
            .find(|e| e.saved.id == in_page_id)
            .unwrap();
        assert_eq!(
            entry_scope(&snap1, &in_page_entry),
            WorktreeScope::CardWorktree("c1".into()),
            "in-page + / ⌘T on Terminals must inherit current CardWorktree"
        );

        // Now navigate to Board while `in_page_id` (a CardWorktree shell) is still remembered in `terminal_selection`.
        // Pressing global `⌘T` (`new-terminal` from Board) must create in `ProjectRoot("p1")`, NOT `CardWorktree("c1")`!
        ui.navigate("board");
        crate::commands::execute(ui, "new-terminal");
        assert_eq!(ui.page.get_untracked(), "terminals");
        let global_id = ui.terminal_selection.get_untracked();
        assert_ne!(global_id, in_page_id);
        let snap2 = ui.snapshot.get_untracked();
        let global_entry = crate::terminals::entries(&snap2)
            .into_iter()
            .find(|e| e.saved.id == global_id)
            .unwrap();
        assert_eq!(
            entry_scope(&snap2, &global_entry),
            WorktreeScope::ProjectRoot("p1".into()),
            "global ⌘T from outside Terminals must land in active ProjectRoot"
        );
    });
}

#[test]
fn merge_batch_terminal_lives_directly_in_project_root_without_polluting_root_git_summary() {
    use crate::terminals::worktrees::{WorktreeScope, entry_scope, scope_git_summary};

    let mut s = fixture();
    s.projects[0].directory = "/repo/p1".into();
    s.projects[0].target_branch = Some("main".into());

    let batch_id = "merge-queue:p1:abcd1234".to_owned();
    s.merge_batches.push(canban_types::MergeBatch {
        id: batch_id.clone(),
        project_id: "p1".into(),
        profile_id: "merger".into(),
        provider_session_id: None,
        target_branch: "main".into(),
        target_head: "deadbeef".into(),
        strategy: canban_types::MergeStrategy::Rebase,
        sync_upstream: true,
        test_command: None,
        worktree: "/home/worktrees/p1/_merge_queue".into(),
        state: "running".into(),
        failure_kind: Some("conflict".into()),
        conflict_card_number: Some(1),
        conflict_files: vec!["shared.txt".into()],
        test_output: None,
        items: vec![],
        prompt: String::new(),
        error: None,
        created_at: 1,
        updated_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: batch_id.clone(),
        conversation_id: None,
        number: 10,
        name: Some("@merger".into()),
        origin_project_id: Some("p1".into()),
        origin_card_id: None,
        agent_engine: Some("agy".into()),
        runtime: Some(canban_types::TerminalRuntime {
            machine: "local".into(),
            owner_core_id: Some("core".into()),
            core_id: Some("core".into()),
            terminal: Some("pty-merger".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("running".into()),
            agent_turn: Some(1),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminal_contexts.push(TerminalContext {
        machine: "local".into(),
        terminal_id: batch_id.clone(),
        core_id: "core".into(),
        terminal: "pty-merger".into(),
        cwd: Some("/home/worktrees/p1/_merge_queue".into()),
        project_id: Some("p1".into()),
        dirty: true,
        branch: None,
        ..Default::default()
    });

    let rows = crate::terminals::entries(&s);
    let merger_entry = rows
        .iter()
        .find(|e| e.saved.id == batch_id)
        .expect("merger entry must appear in terminal entries");
    assert_eq!(merger_entry.title(), "@merger");
    let scope = entry_scope(&s, merger_entry);
    assert_eq!(scope, WorktreeScope::ProjectRoot("p1".into()));

    let summary = scope_git_summary(&s, &scope, std::slice::from_ref(merger_entry));
    assert_eq!(summary.branch.as_deref(), Some("main"));
    assert!(!summary.dirty);
    assert_eq!(summary.ahead, 0);
    assert_eq!(summary.behind, 0);
}

#[test]
fn manual_shell_observed_agent_exposes_icon_only_without_changing_title_or_close_confirmation() {
    use canban_ui_kit::Icon;
    let mut s = fixture();
    s.terminals.push(TerminalEntry {
        id: "manual-shell".into(),
        conversation_id: None,
        number: 10,
        name: Some("Terminal 2".into()),
        origin_project_id: Some("p1".into()),
        origin_card_id: Some("c1".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: Some("core".into()),
            core_id: Some("core".into()),
            terminal: Some("pty-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: Some("agy".into()),
            waiting: None,
        }),
        agent: None,
        created_at: 10,
    });

    let entry = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "manual-shell")
        .unwrap();
    // Title remains "Terminal 2" (icon-only, no rename to "Antigravity 2").
    assert_eq!(entry.title(), "Terminal 2");
    assert_eq!(entry.provider(), Some((Icon::Agy, "Antigravity".into())));
    assert!(!entry.needs_attention());
    // Without OSC 7501 working state, plain shells do not gain unexpected close modals.
    assert_eq!(entry.close_confirm_reason(&s), None);

    // If Herdr connection is unavailable or finished, provider icon is hidden.
    s.terminals
        .last_mut()
        .unwrap()
        .runtime
        .as_mut()
        .unwrap()
        .connection = "unavailable".into();
    let disconnected = crate::terminals::entries(&s)
        .into_iter()
        .find(|e| e.saved.id == "manual-shell")
        .unwrap();
    assert_eq!(disconnected.provider(), None);
}

#[test]
fn mounted_tabs_header_updates_across_project_root_and_worktree_selection_and_marks_selected_tab() {
    use nsh_core::{
        AccessibilityRole, ApproxTextEngine, Core, EventKind, EventPayload, MountItem, NodeId,
        element::JsEvent, geometry::size, prelude::*,
    };
    use std::collections::{HashMap, HashSet};

    #[component]
    fn HeaderProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[0].target_branch = Some("main".into());
        s.cards[0].title = "Worktree Card Task".into();
        s.cards[0].worktree = Some("/repo/p1-worktrees/c1".into());
        s.cards[0].branch = Some("canban/1".into());
        s.worktrees.push(ManagedWorktree {
            id: "wt-1".into(),
            project_id: "p1".into(),
            branch: "feat/isolated".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/repo/p1-worktrees/feat-isolated".into(),
            location: WorkLocation {
                machine: "local".into(),
                directory: "/repo/p1-worktrees/feat-isolated".into(),
            },
            created_at: 3,
        });

        let mk_shell = |id: &str, num: u64, origin: Option<&str>| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: num,
            name: Some(format!("Terminal {num}")),
            origin_project_id: Some("p1".into()),
            origin_card_id: origin.map(str::to_owned),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{id}")),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: num,
        };

        s.terminals.extend([
            mk_shell("root-shell", 17, None),
            mk_shell("card-shell", 18, Some("c1")),
            mk_shell("wt-shell-1", 19, None),
            mk_shell("wt-shell-2", 20, None),
        ]);
        s.terminal_contexts.extend([
            TerminalContext {
                machine: "local".into(),
                terminal_id: "root-shell".into(),
                core_id: "core".into(),
                terminal: "pty-root-shell".into(),
                cwd: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "card-shell".into(),
                core_id: "core".into(),
                terminal: "pty-card-shell".into(),
                cwd: Some("/repo/p1-worktrees/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                branch: Some("canban/1".into()),
                dirty: true,
                additions: 27,
                deletions: 5,
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "wt-shell-1".into(),
                core_id: "core".into(),
                terminal: "pty-wt-shell-1".into(),
                cwd: Some("/repo/p1-worktrees/feat-isolated".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt-1".into()),
                branch: Some("feat/isolated".into()),
                dirty: true,
                additions: 0,
                deletions: 0,
                ..Default::default()
            },
            TerminalContext {
                machine: "local".into(),
                terminal_id: "wt-shell-2".into(),
                core_id: "core".into(),
                terminal: "pty-wt-shell-2".into(),
                cwd: Some("/repo/p1-worktrees/feat-isolated".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt-1".into()),
                branch: Some("feat/isolated".into()),
                ..Default::default()
            },
        ]);

        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());
        // Initially select the ProjectRoot shell (`Terminal 17`).
        ui.open_terminal("root-shell".into());

        view()
            .w(px(1200.))
            .h(px(42.))
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Select CardWorktree Shell".into()))
                    .on_press(move |_: &()| ui.open_terminal("card-shell".into())),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Select ManagedWorktree Shell".into()))
                    .on_press(move |_: &()| ui.open_terminal("wt-shell-1".into())),
            )
            .child(crate::terminals::tabs::header(ui))
    }

    struct MountedTree {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, (String, FontWeight)>,
        tabs: HashMap<NodeId, (String, bool)>,
        labels: HashMap<String, NodeId>,
    }

    impl MountedTree {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                tabs: HashMap::new(),
                labels: HashMap::new(),
            }
        }
        fn apply(&mut self, batch: &nsh_core::MountBatch) {
            for item in &batch.items {
                match item {
                    MountItem::Create { id, .. } => {
                        self.alive.insert(*id);
                    }
                    MountItem::Insert { parent, child, .. } => {
                        self.parents.insert(*child, *parent);
                    }
                    MountItem::Remove { id } => {
                        self.parents.remove(id);
                    }
                    MountItem::Delete { id } => {
                        self.alive.remove(id);
                        self.parents.remove(id);
                        self.texts.remove(id);
                        self.tabs.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        let weight = text
                            .spans
                            .first()
                            .map(|s| FontWeight(s.style.font_weight))
                            .unwrap_or(FontWeight::NORMAL);
                        self.texts.insert(*id, (text.text.to_string(), weight));
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            self.labels.insert(label.to_string(), *id);
                        }
                        if props.accessibility.role == AccessibilityRole::Tab {
                            let lbl = props
                                .accessibility
                                .label
                                .as_ref()
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            self.tabs
                                .insert(*id, (lbl, props.accessibility.state.selected));
                        } else {
                            self.tabs.remove(id);
                        }
                    }
                    _ => {}
                }
            }
        }
        fn is_mounted(&self, mut id: NodeId) -> bool {
            if !self.alive.contains(&id) {
                return false;
            }
            let mut guard = 0;
            while id != nsh_core::ROOT_ID {
                let Some(&parent) = self.parents.get(&id) else {
                    return false;
                };
                id = parent;
                guard += 1;
                if guard > 256 {
                    return false;
                }
            }
            true
        }
        fn mounted_texts(&self) -> Vec<(String, FontWeight)> {
            self.texts
                .iter()
                .filter(|(id, _)| self.is_mounted(**id))
                .map(|(_, v)| v.clone())
                .collect()
        }
        fn mounted_tabs(&self) -> Vec<(String, bool)> {
            self.tabs
                .iter()
                .filter(|(id, _)| self.is_mounted(**id))
                .map(|(_, v)| v.clone())
                .collect()
        }
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut MountedTree) {
        for _ in 0..8 {
            let had_batch = if let Some(batch) = core.commit() {
                tree.apply(&batch);
                true
            } else {
                false
            };
            let mut events = Vec::new();
            core.take_events(&mut events);
            let mut dispatched = false;
            for ev in events {
                match ev {
                    JsEvent::Event(id, kind, payload) => {
                        core.component_event(id, kind, &payload);
                        dispatched = true;
                    }
                    JsEvent::LayoutListener {
                        host,
                        id,
                        listener,
                        frame,
                    } => {
                        core.component_layout(host, id, &listener, frame);
                        dispatched = true;
                    }
                    _ => {}
                }
            }
            if !had_batch && !dispatched {
                break;
            }
        }
    }

    let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(42.)));
    core.set_components(nsh_base::registry());
    core.mount_root(&nsh_core::element::Root::of::<HeaderProbe>());
    let mut tree = MountedTree::new();
    pump(&mut core, &mut tree);

    // 1. Initial mount on `root-shell` (`ProjectRoot("p1")`): only `Terminal 17` is rendered & selected.
    let texts_initial = tree.mounted_texts();
    assert!(
        texts_initial
            .iter()
            .any(|(t, w)| t == "Terminal 17" && *w == FontWeight::SEMIBOLD),
        "expected selected ProjectRoot tab `Terminal 17` in SEMIBOLD, got {texts_initial:?}"
    );
    assert!(
        !texts_initial
            .iter()
            .any(|(t, _)| t == "Terminal 18" || t == "Terminal 19"),
        "worktree terminals must not appear while ProjectRoot is selected: {texts_initial:?}"
    );
    assert_eq!(tree.mounted_tabs().len(), 1);
    assert!(tree.mounted_tabs()[0].1);

    // 2. Switch `terminal_selection` to `card-shell` (`CardWorktree("c1")`) WITHOUT mutating `snapshot`:
    //    `Terminal 17` (ProjectRoot) MUST disappear from the tab strip immediately, and BOTH `c1`'s Agent
    //    and `Terminal 18` MUST appear, with `Terminal 18` marked selected (`SEMIBOLD` + `selected: true`).
    let select_card_btn = *tree
        .labels
        .get("Select CardWorktree Shell")
        .expect("Select CardWorktree Shell button");
    assert!(core.component_event(select_card_btn, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);

    let texts_card = tree.mounted_texts();
    assert!(
        !texts_card.iter().any(|(t, _)| t == "Terminal 17"),
        "ProjectRoot `Terminal 17` leaked into CardWorktree tab bar after switching selection: {texts_card:?}"
    );
    assert!(
        texts_card
            .iter()
            .any(|(t, w)| t == "Terminal 18" && *w == FontWeight::SEMIBOLD),
        "current CardWorktree shell `Terminal 18` must appear in top tabs as SEMIBOLD: {texts_card:?}"
    );
    assert!(
        texts_card
            .iter()
            .any(|(t, w)| t == "Worktree Card Task" && *w == FontWeight::NORMAL),
        "sibling CardWorktree Agent tab must appear alongside current shell in NORMAL weight: {texts_card:?}"
    );
    assert!(
        texts_card.iter().any(|(t, _)| t == "+27") && texts_card.iter().any(|(t, _)| t == "−5"),
        "dirty CardWorktree switcher pill must render `+27` and `−5` instead of `changed`: {texts_card:?}"
    );
    assert!(
        !texts_card.iter().any(|(t, _)| t == "changed"),
        "top header pill must not render `changed`: {texts_card:?}"
    );
    let card_tabs = tree.mounted_tabs();
    assert_eq!(
        card_tabs.len(),
        2,
        "expected 2 tabs in CardWorktree, got {card_tabs:?}"
    );
    assert_eq!(
        card_tabs.iter().filter(|(_, sel)| *sel).count(),
        1,
        "exactly one tab must be selected: {card_tabs:?}"
    );
    assert!(
        card_tabs
            .iter()
            .any(|(lbl, sel)| lbl.contains("Terminal 18") && *sel),
        "expected `Terminal 18` tab to carry `selected: true`, got {card_tabs:?}"
    );

    // 3. Switch `terminal_selection` to `wt-shell-1` (`ManagedWorktree("wt-1")`):
    //    Both `Terminal 17` and CardWorktree tabs disappear; `Terminal 19` (selected, SEMIBOLD)
    //    and `Terminal 20` (unselected, NORMAL) render in the top strip.
    let select_wt_btn = *tree
        .labels
        .get("Select ManagedWorktree Shell")
        .expect("Select ManagedWorktree Shell button");
    assert!(core.component_event(select_wt_btn, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);

    let texts_wt = tree.mounted_texts();
    assert!(
        !texts_wt
            .iter()
            .any(|(t, _)| t == "Terminal 17" || t == "Terminal 18"),
        "neither ProjectRoot nor CardWorktree tabs may leak into ManagedWorktree: {texts_wt:?}"
    );
    assert!(
        texts_wt
            .iter()
            .any(|(t, w)| t == "Terminal 19" && *w == FontWeight::SEMIBOLD),
        "selected ManagedWorktree tab `Terminal 19` must be SEMIBOLD: {texts_wt:?}"
    );
    assert!(
        texts_wt
            .iter()
            .any(|(t, w)| t == "Terminal 20" && *w == FontWeight::NORMAL),
        "sibling ManagedWorktree tab `Terminal 20` must be NORMAL: {texts_wt:?}"
    );
    assert!(
        texts_wt.iter().any(|(t, _)| t == "+0") && texts_wt.iter().any(|(t, _)| t == "−0"),
        "dirty zero-line ManagedWorktree switcher pill must render `+0` and `−0` (Scheme B): {texts_wt:?}"
    );
    let wt_tabs = tree.mounted_tabs();
    assert_eq!(wt_tabs.len(), 2);
    assert!(
        wt_tabs
            .iter()
            .any(|(lbl, sel)| lbl.contains("Terminal 19") && *sel)
    );
    assert!(
        wt_tabs
            .iter()
            .any(|(lbl, sel)| lbl.contains("Terminal 20") && !*sel)
    );
}

#[test]
fn mod_digit_shortcuts_select_numbered_tab_in_current_workspace_and_reset_labels_on_close() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state": "finished"}));

        let ui = crate::presenter::Ui::new();
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());

        // Spawn 9 terminals in ProjectRoot("p1") -> Terminal 1 .. Terminal 9
        let mut ids = Vec::new();
        for expected_num in 1..=9 {
            crate::terminals::create(ui, "p1".into(), None);
            let tid = ui.terminal_selection.get_untracked();
            let title = ui.snapshot.with_untracked(|snap| {
                snap.terminals.iter().find(|t| t.id == tid).unwrap().title()
            });
            assert_eq!(title, format!("Terminal {expected_num}"));
            ids.push(tid);
        }

        // ⌘1 .. ⌘9 switch directly to the 1st .. 9th tab in the current workspace!
        for idx in 1..=9 {
            crate::commands::execute(ui, &format!("terminal-tab-{idx}"));
            assert_eq!(ui.terminal_selection.get_untracked(), ids[idx - 1]);
        }

        // Close all 9 terminals -> next created terminal in p1 resets to Terminal 1!
        for tid in &ids {
            crate::terminals::close_tab(ui, tid);
        }
        crate::terminals::create(ui, "p1".into(), None);
        let reset_tid = ui.terminal_selection.get_untracked();
        let reset_title = ui.snapshot.with_untracked(|snap| {
            snap.terminals
                .iter()
                .find(|t| t.id == reset_tid)
                .unwrap()
                .title()
        });
        assert_eq!(reset_title, "Terminal 1");
    });
}

#[test]
fn sidebar_rows_render_strict_two_line_compact_layout_without_engine_names_stages_or_selection_reflow()
 {
    use crate::terminals::labels::{
        SIDEBAR_TITLE_MAX_CHARS, sidebar_meta_icon, sidebar_meta_text, truncate_sidebar_title,
    };
    use canban_ui_kit::Icon;
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountItem, NodeId, element::JsEvent,
        geometry::size, prelude::*,
    };
    use std::collections::{HashMap, HashSet};

    // 1. Unit checks for title truncation and second-line metadata rules.
    assert_eq!(truncate_sidebar_title("Terminal 1"), "Terminal 1");
    let exact_32 = "a".repeat(SIDEBAR_TITLE_MAX_CHARS);
    assert_eq!(truncate_sidebar_title(&exact_32), exact_32);
    let long_cjk = "#86 Terminals 现在中间栏最多 3 行，能否紧凑些最多两行并且永远不折行";
    let truncated = truncate_sidebar_title(long_cjk);
    assert!(truncated.ends_with('…'), "expected ellipsis: {truncated}");
    assert_eq!(
        truncated.chars().count(),
        SIDEBAR_TITLE_MAX_CHARS + 1,
        "32 chars + 1 ellipsis: {truncated}"
    );
    assert_eq!(
        truncate_sidebar_title("line1\n  line2\tline3"),
        "line1 line2 line3"
    );

    #[component]
    fn SidebarProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[0].target_branch = Some("main".into());
        s.cards[0].number = 86;
        s.cards[0].stage = "develop".into();
        s.cards[0].title = "Terminals 现在中间栏最多 3 行，能否紧凑些最多两行并且永远不折行".into();
        s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
        s.cards[0].branch = Some("canban-0afd/86".into());

        // Card Role Agent (Antigravity engine, @developer role, Blocked with RuntimeWait reason)
        {
            let run = s.terminals[0].agent.as_mut().unwrap();
            run.profile.id = "developer".into();
            run.profile.preset = Some("agy".into());
            run.stage = "develop".into();
            run.observed = Some(json!({
                "state": "live",
                "status": {
                    "state": "blocked",
                    "kind": "permission",
                    "title": "Allow command?",
                    "msg": "cargo test --workspace"
                }
            }));
            run.lifecycle.waiting = Some(RuntimeWait {
                id: "wait-86".into(),
                kind: "permission".into(),
                title: "Allow command?".into(),
                message: "cargo test --workspace".into(),
                since: 1,
            });
        }
        // Finish p2's default agent so we focus on p1's four canonical session kinds + renamed Card Agent.
        s.terminals[1].agent.as_mut().unwrap().observed = Some(json!({"state": "finished"}));

        // Renamed Card Role Agent on the same Card (#86) to verify selecting a renamed Agent
        // never injects an extra original Card title line.
        let mut renamed_run = s.terminals[0].agent.clone().unwrap();
        renamed_run.profile.id = "reviewer".into();
        renamed_run.profile.preset = Some("claude".into());
        renamed_run.stage = "review".into();
        renamed_run.terminal = Some("t-renamed".into());
        renamed_run.location = Some(WorkLocation {
            machine: "gpu-box".into(),
            directory: "/remote/c1".into(),
        });
        renamed_run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
        renamed_run.lifecycle.waiting = None;
        let renamed_id = Conversation::key("c1", "reviewer");
        s.terminals.push(TerminalEntry {
            id: renamed_id.clone(),
            conversation_id: Some(renamed_id.clone()),
            number: 3,
            name: Some("Custom Review Session".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: None,
            agent: Some(renamed_run),
            created_at: 2,
        });

        // Standalone ephemeral Agent (`Antigravity 1`, Done)
        s.terminals.push(TerminalEntry {
            id: "standalone-agy".into(),
            conversation_id: None,
            number: 4,
            name: Some("Antigravity 1".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: Some("c1".into()),
            agent_engine: Some("agy".into()),
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-standalone".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("done".into()),
                agent_turn: Some(1),
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: 3,
        });

        // Shell with observed Agent (`Terminal 4` running `codex`, Working)
        s.terminals.push(TerminalEntry {
            id: "shell-with-codex".into(),
            conversation_id: None,
            number: 5,
            name: Some("Terminal 4".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-codex-shell".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("working".into()),
                agent_turn: Some(1),
                observed_agent: Some("codex".into()),
                waiting: None,
            }),
            agent: None,
            created_at: 4,
        });

        // Plain idle Shell (`Terminal 1`)
        s.terminals.push(TerminalEntry {
            id: "plain-shell".into(),
            conversation_id: None,
            number: 6,
            name: Some("Terminal 1".into()),
            origin_project_id: Some("p1".into()),
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
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: 5,
        });

        let projected = crate::terminals::entries(&s);
        let card_dev = projected
            .iter()
            .find(|e| e.profile() == Some("developer"))
            .unwrap();
        assert_eq!(sidebar_meta_icon(card_dev), Icon::Agy);
        assert_eq!(sidebar_meta_text(card_dev, "Local"), "@developer · Local");

        let card_rev = projected
            .iter()
            .find(|e| e.profile() == Some("reviewer"))
            .unwrap();
        assert_eq!(sidebar_meta_icon(card_rev), Icon::Claude);
        assert_eq!(
            sidebar_meta_text(card_rev, "gpu-box"),
            "@reviewer · gpu-box"
        );

        let standalone = projected
            .iter()
            .find(|e| e.saved.id == "standalone-agy")
            .unwrap();
        assert_eq!(sidebar_meta_icon(standalone), Icon::Agy);
        assert_eq!(sidebar_meta_text(standalone, "Local"), "Local");

        let shell_codex = projected
            .iter()
            .find(|e| e.saved.id == "shell-with-codex")
            .unwrap();
        assert_eq!(sidebar_meta_icon(shell_codex), Icon::Codex);
        assert_eq!(sidebar_meta_text(shell_codex, "Local"), "Local");

        let plain = projected
            .iter()
            .find(|e| e.saved.id == "plain-shell")
            .unwrap();
        assert_eq!(sidebar_meta_icon(plain), Icon::Terminal);
        assert_eq!(sidebar_meta_text(plain, "Local"), "Local");

        ui.machines
            .set(json!({"machines": [{"id": "gpu-box", "name": "gpu-box", "state": "connected"}]}));
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());
        ui.open_terminal("plain-shell".into());

        let select_blocked_id = Conversation::key("c1", "developer");
        view()
            .w(px(300.))
            .h(px(640.))
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Select Blocked Card Agent".into()))
                    .on_press(move |_: &()| ui.open_terminal(select_blocked_id.clone())),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Select Renamed Card Agent".into()))
                    .on_press(move |_: &()| ui.open_terminal(renamed_id.clone())),
            )
            .child(crate::terminals::sidebar_for_test(ui))
    }

    struct MountedSidebar {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, String>,
        labels: HashMap<String, NodeId>,
    }
    impl MountedSidebar {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                labels: HashMap::new(),
            }
        }
        fn apply(&mut self, batch: &nsh_core::MountBatch) {
            for item in &batch.items {
                match item {
                    MountItem::Create { id, .. } => {
                        self.alive.insert(*id);
                    }
                    MountItem::Insert { parent, child, .. } => {
                        self.parents.insert(*child, *parent);
                    }
                    MountItem::Remove { id } => {
                        self.parents.remove(id);
                    }
                    MountItem::Delete { id } => {
                        self.alive.remove(id);
                        self.parents.remove(id);
                        self.texts.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        self.texts.insert(*id, text.text.to_string());
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            self.labels.insert(label.to_string(), *id);
                        }
                    }
                    _ => {}
                }
            }
        }
        fn is_mounted(&self, mut id: NodeId) -> bool {
            if !self.alive.contains(&id) {
                return false;
            }
            let mut guard = 0;
            while id != nsh_core::ROOT_ID {
                let Some(&parent) = self.parents.get(&id) else {
                    return false;
                };
                id = parent;
                guard += 1;
                if guard > 256 {
                    return false;
                }
            }
            true
        }
        fn mounted_texts(&self) -> Vec<String> {
            self.texts
                .iter()
                .filter(|(id, _)| self.is_mounted(**id))
                .map(|(_, v)| v.clone())
                .collect()
        }
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut MountedSidebar) {
        for _ in 0..8 {
            let had_batch = if let Some(batch) = core.commit() {
                tree.apply(&batch);
                true
            } else {
                false
            };
            let mut events = Vec::new();
            core.take_events(&mut events);
            let mut dispatched = false;
            for ev in events {
                match ev {
                    JsEvent::Event(id, kind, payload) => {
                        core.component_event(id, kind, &payload);
                        dispatched = true;
                    }
                    JsEvent::LayoutListener {
                        host,
                        id,
                        listener,
                        frame,
                    } => {
                        core.component_layout(host, id, &listener, frame);
                        dispatched = true;
                    }
                    _ => {}
                }
            }
            if !had_batch && !dispatched {
                break;
            }
        }
    }

    let mut core = Core::new(ApproxTextEngine, size(px(300.), px(640.)));
    core.set_components(nsh_base::registry());
    core.mount_root(&nsh_core::element::Root::of::<SidebarProbe>());
    let mut tree = MountedSidebar::new();
    pump(&mut core, &mut tree);

    let check_invariants = |texts: &[String], phase: &str| {
        let expected_truncated = truncate_sidebar_title(
            "#86 Terminals 现在中间栏最多 3 行，能否紧凑些最多两行并且永远不折行",
        );
        assert!(
            texts.iter().any(|t| t == &expected_truncated),
            "[{phase}] Card #86 title must stay truncated to 32 chars + ellipsis: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("永远不折行")),
            "[{phase}] full un-truncated Card #86 title must never render inline even when selected: {texts:?}"
        );
        // Engine names and Card Stage labels must NOT appear in sidebar rows.
        for forbidden in [
            "Antigravity ·",
            "Claude ·",
            "Codex",
            "· Develop",
            "· Review",
            "Open",
        ] {
            assert!(
                !texts.iter().any(|t| t.contains(forbidden)),
                "[{phase}] forbidden legacy metadata `{forbidden}` rendered in sidebar: {texts:?}"
            );
        }
        // Inline 3rd-line wait/error reasons must NOT render in the sidebar (kept in hover tooltip).
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("Permission required") || t.contains("cargo test")),
            "[{phase}] Blocked wait detail must live in tooltip rather than a 3rd row line: {texts:?}"
        );
        // Expected 2nd-line left captions and right Herdr status words:
        for required in [
            "@developer · Local",
            "@reviewer · gpu-box",
            "Local",
            "Antigravity 1",
            "Terminal 4",
            "Terminal 1",
            "Custom Review Session",
            "Blocked",
            "Working",
            "Done",
        ] {
            assert!(
                texts.iter().any(|t| t == required),
                "[{phase}] missing expected sidebar label `{required}` in {texts:?}"
            );
        }
    };

    // Initial state (`plain-shell` selected)
    let initial_texts = tree.mounted_texts();
    check_invariants(&initial_texts, "initial");
    // Tooltip / accessibility label still retains full Card title & Permission required wait reason!
    assert!(
        tree.labels.keys().any(|lbl| lbl.contains("永远不折行")
            && lbl.contains("Permission required: Allow command? · cargo test --workspace")),
        "hover tooltip / accessibility label must preserve full Card title and wait reason: {:?}",
        tree.labels.keys().collect::<Vec<_>>()
    );

    // Select Blocked Card Agent (`#86 @developer`) -> title stays truncated, no 3rd line appears!
    let btn_blocked = *tree
        .labels
        .get("Select Blocked Card Agent")
        .expect("Select Blocked Card Agent button");
    assert!(core.component_event(btn_blocked, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    check_invariants(&tree.mounted_texts(), "selected-blocked");

    // Select Renamed Card Agent (`Custom Review Session` / `@reviewer`) -> no extra original Card title line!
    let btn_renamed = *tree
        .labels
        .get("Select Renamed Card Agent")
        .expect("Select Renamed Card Agent button");
    assert!(core.component_event(btn_renamed, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    let renamed_texts = tree.mounted_texts();
    check_invariants(&renamed_texts, "selected-renamed");
    assert_eq!(
        renamed_texts
            .iter()
            .filter(|t| t.starts_with("#86 "))
            .count(),
        1,
        "selecting a renamed Agent must NOT add a second `#86 ...` original Card title line: {renamed_texts:?}"
    );
}
