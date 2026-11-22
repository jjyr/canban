fn observe_fixture_locations(s: &mut Snapshot) {
    for t in &s.terminals {
        if s.terminal_contexts.iter().any(|c| c.terminal_id == t.id) {
            continue;
        }
        let cid = t
            .agent
            .as_ref()
            .map(|a| &a.card_id)
            .or(t.origin_card_id.as_ref());
        let card = cid.and_then(|id| s.cards.iter().find(|c| &c.id == id));
        let pid = card.map(|c| &c.project_id).or(t.origin_project_id.as_ref());
        let project = pid.and_then(|id| s.projects.iter().find(|p| &p.id == id));
        let path = card.and_then(|c| c.worktree.clone()).unwrap_or_else(|| {
            if let Some(c) = card {
                format!("/fixture/worktrees/{}", c.id)
            } else {
                project
                    .map(|p| p.directory.clone())
                    .unwrap_or_else(|| "/fixture/outside".into())
            }
        });
        s.terminal_contexts.push(TerminalContext {
            terminal_id: t.id.clone(),
            cwd: Some(path.clone()),
            directory_root: Some(path),
            project_id: pid.cloned(),
            card_id: cid.cloned(),
            ..Default::default()
        });
    }
    for c in &mut s.terminal_contexts {
        if c.directory_root.is_none() {
            c.directory_root = c.cwd.as_ref().map(|p| {
                if p == "/repo/p1/crates" {
                    "/repo/p1".into()
                } else {
                    p.clone()
                }
            });
        }
    }
}
use super::*;

#[test]
fn adoption_keeps_the_selected_pty_and_tab_instead_of_falling_back_to_a_neighbor() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, (ui, after, managed_id)) = nsh_core::element::create_root(|| {
        let ui = crate::presenter::Ui::new();
        let mut before = fixture();
        let managed = before.terminals.remove(0);
        let run = managed.agent.as_ref().unwrap();
        let mut manual: TerminalEntry = serde_json::from_value(json!({
            "id":"manual-to-adopt","conversation_id":null,"number":managed.number,
            "name":null,"created_at":1,"origin_card_id":"c1",
            "runtime":{"core_id":run.core_id,"terminal":run.terminal,
                "state":"running","connection":"connected","error":null}
        }))
        .unwrap();
        before.terminals.push(manual.clone());
        observe_fixture_locations(&mut before);
        ui.snapshot.set(before.clone());
        ui.terminal_tabs.set(vec!["manual-to-adopt".into()]);
        ui.terminal_selection.set("manual-to-adopt".into());
        ui.card.set("c1".into());
        ui.card_tab.set("manual-to-adopt".into());
        crate::terminals::watch(ui);
        let mut after = before;
        manual.runtime.as_mut().unwrap().state = "finished".into();
        manual.runtime.as_mut().unwrap().terminal = None;
        *after
            .terminals
            .iter_mut()
            .find(|t| t.id == "manual-to-adopt")
            .unwrap() = manual;
        let managed_id = managed.id.clone();
        after.terminals.push(managed);
        observe_fixture_locations(&mut after);
        (ui, after, managed_id)
    });
    ui.snapshot.set(after);
    assert_eq!(ui.terminal_selection.get_untracked(), managed_id);
    assert_eq!(ui.terminal_tabs.get_untracked(), vec![managed_id.clone()]);
    assert_eq!(ui.card_tab.get_untracked(), managed_id);
}

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
fn board_and_git_clicks_resolve_visible_context_without_tracking_terminal_selection() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let ui = crate::presenter::Ui::new();
        let s = fixture();
        let terminal = s.terminals[0].id.clone();
        ui.snapshot.set(s);
        ui.select_project("p2".into());
        ui.open_terminal(terminal.clone());
        assert_eq!(ui.project.get_untracked(), "p2");
        crate::commands::execute(ui, "go-board");
        assert_eq!(ui.page.get_untracked(), "board");
        assert_eq!(ui.project.get_untracked(), "p1");

        ui.select_project("p2".into());
        ui.open_terminal(terminal.clone());
        crate::commands::execute(ui, "go-review");
        assert_eq!(ui.project.get_untracked(), "p1");
        assert_eq!(ui.page.get_untracked(), "review");

        ui.open_card("c2".into());
        crate::commands::execute(ui, "go-review");
        assert_eq!(ui.project.get_untracked(), "p2");

        // A remembered Terminal must not influence entry from a global page.
        ui.open_terminal(terminal.clone());
        ui.navigate("settings");
        crate::commands::execute(ui, "go-board");
        assert_eq!(ui.project.get_untracked(), "p2");

        // Back restores the recorded Project, even though this Terminal belongs to p1.
        ui.open_terminal(terminal);
        ui.history.update(|h| h.visit(ui.location()));
        crate::commands::execute(ui, "go-board");
        ui.history.update(|h| h.visit(ui.location()));
        crate::navigation::go(ui, false);
        assert_eq!(ui.page.get_untracked(), "terminals");
        assert_eq!(ui.project.get_untracked(), "p2");

        // An unavailable/unassigned selection leaves the existing Project alone.
        ui.terminal_selection.set("missing".into());
        crate::commands::execute(ui, "go-board");
        assert_eq!(ui.project.get_untracked(), "p2");
    });
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
    assert_eq!(task.project_id.as_deref(), Some("p2"));
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

        // Inspecting the Card diff and returning preserves the existing split/tab without launching anything.
        ui.card_terminal_open.set(true);
        ui.card_tab.set(conv_id.clone());
        ui.history.update(|h| {
            h.entries.clear();
            h.index = 0;
            h.visit(ui.location());
        });
        crate::review::open_card(ui, "c1");
        crate::navigation::go(ui, false);
        assert_eq!(ui.page.get_untracked(), "card");
        assert!(ui.card_terminal_open.get_untracked());
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
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

        // 6. Clicking the top Card link (`#N Title`) or `Open Card` menu item inside `Terminals` (`terminal_card`)
        // navigates to the Card in standalone mode AND expands/focuses that exact terminal tab (`conv_id`),
        // forming a symmetric round-trip with `card::terminal::pop_out`.
        ui.open_terminal(conv_id.clone());
        assert_eq!(ui.page.get_untracked(), "terminals");
        assert_eq!(ui.terminal_selection.get_untracked(), conv_id);
        ui.terminal_card("c1".into(), Some(conv_id.clone()));
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert!(ui.card_standalone.get_untracked());
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());

        // Popping back out returns to `Terminals` with `conv_id` selected, and Card worktree `Open Card` (`None`)
        // also preserves the currently selected terminal when it belongs to that Card.
        crate::card::terminal::pop_out(ui);
        assert_eq!(ui.page.get_untracked(), "terminals");
        assert_eq!(ui.terminal_selection.get_untracked(), conv_id);
        ui.terminal_card("c1".into(), None);
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert!(ui.card_standalone.get_untracked());
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
        assert!(ui.card_terminal_open.get_untracked());
        assert!(ui.card_terminal_focused.get_untracked());
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
        observe_fixture_locations(&mut s);
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
            directory: "/repo/p1-worktrees/feat-active".into(),
            created_at: 3,
        });
        s.worktrees.push(ManagedWorktree {
            id: "wt-idle".into(),
            project_id: "p1".into(),
            branch: "chore/idle".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/repo/p1-worktrees/chore-idle".into(),
            directory: "/repo/p1-worktrees/chore-idle".into(),
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
                terminal_id: "root-2".into(),
                core_id: "core".into(),
                terminal: "pty-root-2".into(),
                cwd: Some("/repo/p1/crates".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
            TerminalContext {
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
        observe_fixture_locations(&mut s);
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
        let before_cd = crate::terminals::current(ui).unwrap();
        assert_eq!(
            crate::terminals::worktrees::compare_target(&ui.snapshot.get_untracked(), &before_cd),
            Some(crate::terminals::worktrees::CompareTarget::Card(
                "c1".into()
            ))
        );
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
            ctx.directory_root = ctx.cwd.clone();
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
        assert_eq!(
            crate::terminals::worktrees::compare_target(&snap_after, &moved),
            Some(crate::terminals::worktrees::CompareTarget::Checkout {
                project: "p1".into(),
                cwd: "/repo/p1".into(),
                base: "HEAD".into(),
                branch: "main".into(),
                three_dot: false,
            })
        );
        let managed_entry = crate::terminals::entries(&snap_after)
            .into_iter()
            .find(|e| {
                e.location.as_ref().and_then(|l| l.worktree_id.as_deref()) == Some("wt-active")
            })
            .unwrap();
        assert_eq!(
            crate::terminals::worktrees::compare_target(&snap_after, &managed_entry),
            Some(crate::terminals::worktrees::CompareTarget::Checkout {
                project: "p1".into(),
                cwd: "/repo/p1-worktrees/feat-active".into(),
                base: "main".into(),
                branch: "feat/active".into(),
                three_dot: true,
            })
        );
        let mut non_git = moved.clone();
        non_git.location.as_mut().unwrap().branch = None;
        assert!(crate::terminals::worktrees::compare_target(&snap_after, &non_git).is_none());
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
            directory: "/repo/p1-wt/feat-x".into(),
            created_at: 1,
        });
        s.terminal_contexts.extend([
            TerminalContext {
                terminal_id: "r1".into(),
                core_id: "core".into(),
                terminal: "pty-r1".into(),
                cwd: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                ..Default::default()
            },
            TerminalContext {
                terminal_id: "c1-a".into(),
                core_id: "core".into(),
                terminal: "pty-c1-a".into(),
                cwd: Some("/repo/p1-wt/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                ..Default::default()
            },
            TerminalContext {
                terminal_id: "c1-b".into(),
                core_id: "core".into(),
                terminal: "pty-c1-b".into(),
                cwd: Some("/repo/p1-wt/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                ..Default::default()
            },
            TerminalContext {
                terminal_id: "c1-c".into(),
                core_id: "core".into(),
                terminal: "pty-c1-c".into(),
                cwd: Some("/repo/p1-wt/c1".into()),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                ..Default::default()
            },
            TerminalContext {
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
        observe_fixture_locations(&mut s);
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
        observe_fixture_locations(&mut s);
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
            WorktreeScope::Unassigned("local".into()),
            "a root not yet present in the tree waits for background discovery"
        );
        // Simulate the ordinary observation that confirms the newly occupied root.
        let mut snap2 = snap2;
        observe_fixture_locations(&mut snap2);
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
fn merge_batch_terminal_uses_its_actual_checkout_and_git_summary() {
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
        queued_at: None,
        started_at: None,
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
    assert!(
        matches!(scope, WorktreeScope::Directory { ref path, .. } if path == "/home/worktrees/p1/_merge_queue")
    );

    let summary = scope_git_summary(&s, &scope, std::slice::from_ref(merger_entry));
    assert_eq!(summary.branch, None);
    assert!(summary.dirty);
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
            directory: "/repo/p1-worktrees/feat-isolated".into(),
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
                terminal_id: "root-shell".into(),
                core_id: "core".into(),
                terminal: "pty-root-shell".into(),
                cwd: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
            TerminalContext {
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

        observe_fixture_locations(&mut s);
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
    let default_profile_tag = format!("@{}", Config::default().profiles[0].id);
    assert!(
        texts_card
            .iter()
            .any(|(t, w)| t == &default_profile_tag && *w == FontWeight::NORMAL),
        "sibling CardWorktree Agent tab must appear as compact `@profile` in NORMAL weight: {texts_card:?}"
    );
    assert!(
        texts_card.iter().any(|(t, _)| t == "#1 Worktree Card Task"),
        "top-right context_bar must still render full Card title `#1 Worktree Card Task`: {texts_card:?}"
    );
    assert!(
        !texts_card
            .iter()
            .any(|(t, _)| t == "Worktree Card Task" || t == "develop"),
        "managed Agent tab must not duplicate standalone Card title or column name: {texts_card:?}"
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
            // This shortcut scenario needs observed locations, including the first
            // root creation which has no pre-existing tree group to match.
            ui.snapshot.update(observe_fixture_locations);
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
fn sidebar_rows_render_single_line_compact_layout_with_dedup_and_remote_host_only() {
    use crate::terminals::labels::{sidebar_host_badge, sidebar_meta_icon, sidebar_row_title};
    use canban_ui_kit::Icon;
    use nsh_core::{
        ApproxTextEngine, Bounds, Core, EventKind, EventPayload, MountItem, NodeId, Points,
        element::JsEvent, geometry::size, prelude::*,
    };
    use std::collections::{HashMap, HashSet};

    // 1. Unit checks for single-line title whitespace collapsing and dedup.
    let mut named_entry = crate::terminals::entries(&fixture())[0].clone();
    named_entry.saved.name = Some("line1\n  line2\tline3".into());
    assert_eq!(sidebar_row_title(&named_entry, true), "line1 line2 line3");

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
            let dev_id = Conversation::key("c1", "developer");
            s.terminals[0].id = dev_id.clone();
            s.terminals[0].conversation_id = Some(dev_id);
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

        // Card Role Agent that cd'd into ProjectRoot (`@reviewer` -> custom name on gpu-box; also
        // verify non-renamed `cd` falls back to `#86 @role`).
        let mut renamed_run = s.terminals[0].agent.clone().unwrap();
        renamed_run.profile.id = "reviewer".into();
        renamed_run.profile.preset = Some("claude".into());
        renamed_run.stage = "review".into();
        renamed_run.terminal = Some("t-renamed".into());
        renamed_run.directory = Some("/remote/c1".into());
        renamed_run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
        renamed_run.lifecycle.waiting = None;
        let renamed_id = Conversation::key("c1", "reviewer");
        s.entity_hosts.insert(renamed_id.clone(), "gpu-box".into());
        s.host_summaries.insert(
            "gpu-box".into(),
            HostSummary {
                id: "gpu-box".into(),
                name: "gpu-box".into(),
                is_local: false,
                transport_label: "SSH".into(),
                endpoint_label: "gpu-box".into(),
                ssh_target: Some("gpu-box".into()),
                server_id: None,
                connection: HostConnectionState::Connected {
                    rtt_ms: 4,
                    api_version: canban_updater::DESKTOP_API,
                    version: "test".into(),
                    platform: "linux-x86_64".into(),
                },
                active_agents: 1,
                max_agents: 4,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: None,
                pending_mutations: 0,
                sync_conflicts: 0,
            },
        );
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

        // Observe locations so Card #86 entries sit in `CardWorktree("c1")`, while `renamed_id`
        // moves to ProjectRoot (`/repo/p1`) to exercise both in-worktree dedup and cd fallback.
        observe_fixture_locations(&mut s);
        if let Some(ctx) = s
            .terminal_contexts
            .iter_mut()
            .find(|c| c.terminal_id == renamed_id)
        {
            ctx.cwd = Some("/repo/p1".into());
            ctx.directory_root = Some("/repo/p1".into());
            ctx.card_id = None;
            ctx.branch = Some("main".into());
        }

        let projected = crate::terminals::entries(&s);
        let card_dev = projected
            .iter()
            .find(|e| e.profile() == Some("developer"))
            .unwrap();
        assert_eq!(sidebar_meta_icon(card_dev), Icon::Agy);
        assert_eq!(sidebar_row_title(card_dev, true), "@developer");
        assert_eq!(sidebar_row_title(card_dev, false), "#86 @developer");
        assert_eq!(sidebar_host_badge(card_dev, "Local"), None);

        let card_rev = projected
            .iter()
            .find(|e| e.profile() == Some("reviewer"))
            .unwrap();
        assert_eq!(sidebar_meta_icon(card_rev), Icon::Claude);
        assert_eq!(sidebar_row_title(card_rev, false), "Custom Review Session");
        assert_eq!(
            sidebar_host_badge(card_rev, "gpu-box").as_deref(),
            Some("gpu-box")
        );

        let standalone = projected
            .iter()
            .find(|e| e.saved.id == "standalone-agy")
            .unwrap();
        assert_eq!(sidebar_meta_icon(standalone), Icon::Agy);
        assert_eq!(sidebar_row_title(standalone, true), "Antigravity 1");
        assert_eq!(sidebar_host_badge(standalone, "Local"), None);

        let shell_codex = projected
            .iter()
            .find(|e| e.saved.id == "shell-with-codex")
            .unwrap();
        assert_eq!(sidebar_meta_icon(shell_codex), Icon::Codex);
        assert_eq!(sidebar_row_title(shell_codex, true), "Terminal 4");
        assert_eq!(sidebar_host_badge(shell_codex, "Local"), None);

        let plain = projected
            .iter()
            .find(|e| e.saved.id == "plain-shell")
            .unwrap();
        assert_eq!(sidebar_meta_icon(plain), Icon::Terminal);
        assert_eq!(sidebar_row_title(plain, true), "Terminal 1");
        assert_eq!(sidebar_host_badge(plain, "Local"), None);

        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());
        ui.open_terminal("plain-shell".into());

        let select_blocked_id = Conversation::key("c1", "developer");
        let dev_cd_id = select_blocked_id.clone();
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
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("CD Card Agent Out".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            if let Some(ctx) = s
                                .terminal_contexts
                                .iter_mut()
                                .find(|c| c.terminal_id == dev_cd_id)
                            {
                                ctx.cwd = Some("/repo/p1".into());
                                ctx.directory_root = Some("/repo/p1".into());
                                ctx.card_id = None;
                            }
                        });
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Narrow Sidebar".into()))
                    .on_press(move |_: &()| ui.terminal_width.set(210.)),
            )
            .child(crate::terminals::sidebar_for_test(ui))
    }

    struct MountedSidebar {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, String>,
        labels: HashMap<String, NodeId>,
        test_ids: HashMap<String, NodeId>,
        frames: HashMap<NodeId, Bounds<Points>>,
    }
    impl MountedSidebar {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                labels: HashMap::new(),
                test_ids: HashMap::new(),
                frames: HashMap::new(),
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
                        self.frames.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        self.texts.insert(*id, text.text.to_string());
                    }
                    MountItem::Frame { id, frame } => {
                        self.frames.insert(*id, *frame);
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            self.labels.insert(label.to_string(), *id);
                        }
                        if let Some(tid) = &props.test_id {
                            self.test_ids.insert(tid.to_string(), *id);
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

    let check_invariants = |texts: &[String], phase: &str, expect_cd: bool| {
        assert!(
            !texts.iter().any(|t| t.contains("永远不折行")),
            "[{phase}] full Card #86 title must never render inline even when selected: {texts:?}"
        );
        // Redundant Local label, engine names, and Card Stage labels must NOT appear in sidebar rows.
        for forbidden in [
            "Antigravity ·",
            "Claude ·",
            "Codex",
            "· Develop",
            "· Review",
            "Open",
            "Local",
            "@developer · Local",
        ] {
            assert!(
                !texts.iter().any(|t| t == forbidden
                    || t.contains("Antigravity ·")
                    || t.contains("Claude ·")),
                "[{phase}] redundant metadata `{forbidden}` rendered in sidebar: {texts:?}"
            );
        }
        // Inline wait/error reasons stay in tooltip rather than sidebar row text.
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("Permission required") || t.contains("cargo test")),
            "[{phase}] Blocked wait detail must live in tooltip rather than row text: {texts:?}"
        );
        let expected_dev = if expect_cd {
            "#86 @developer"
        } else {
            "@developer"
        };
        for required in [
            expected_dev,
            "Custom Review Session",
            "· gpu-box",
            "Antigravity 1",
            "Terminal 4",
            "Terminal 1",
            "Blocked",
            "Working",
            "Done",
        ] {
            assert!(
                texts.iter().any(|t| t == required),
                "[{phase}] missing expected single-line sidebar label `{required}` in {texts:?}"
            );
        }
    };

    // Initial state: `@developer` sits under `#86` CardWorktree (deduplicated!), `Local` omitted,
    // and each terminal row height is 34pt (32pt pill + 2pt vertical rhythm).
    check_invariants(&tree.mounted_texts(), "initial", false);
    let dev_tid = format!("terminal:{}", Conversation::key("c1", "developer"));
    let dev_row_node = *tree.test_ids.get(&dev_tid).expect("terminal row test_id");
    let dev_frame = tree.frames.get(&dev_row_node).expect("terminal row frame");
    assert!(
        (dev_frame.size.height.0 - 34.0).abs() < 0.5,
        "expected 34pt terminal row wrapper (32pt pill), got {dev_frame:?}"
    );
    // Tooltip / accessibility label still retains full Card title & Permission required wait reason!
    assert!(
        tree.labels.keys().any(|lbl| lbl.contains("永远不折行")
            && lbl.contains("Permission required: Allow command? · cargo test --workspace")),
        "hover tooltip / accessibility label must preserve full Card title and wait reason: {:?}",
        tree.labels.keys().collect::<Vec<_>>()
    );

    // Select Blocked Card Agent -> stays compact without reflow or full Card title inline.
    let btn_blocked = *tree.labels.get("Select Blocked Card Agent").unwrap();
    assert!(core.component_event(btn_blocked, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    check_invariants(&tree.mounted_texts(), "selected-blocked", false);

    // Select Renamed Card Agent -> no duplicate `#86 ...` line.
    let btn_renamed = *tree.labels.get("Select Renamed Card Agent").unwrap();
    assert!(core.component_event(btn_renamed, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    check_invariants(&tree.mounted_texts(), "selected-renamed", false);

    // When @developer `cd`s out of CardWorktree(#86) into ProjectRoot, its label automatically
    // includes `#86 @developer` so task ownership stays clear!
    let btn_cd = *tree.labels.get("CD Card Agent Out").unwrap();
    assert!(core.component_event(btn_cd, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    check_invariants(&tree.mounted_texts(), "cd-out", true);

    // Narrow sidebar (< 230pt): hides status words ("Blocked"/"Working"/"Done") and remote host text,
    // leaving only icons and titles.
    let btn_narrow = *tree.labels.get("Narrow Sidebar").unwrap();
    assert!(core.component_event(btn_narrow, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    let narrow_texts = tree.mounted_texts();
    for hidden_in_narrow in ["Blocked", "Working", "Done", "· gpu-box"] {
        assert!(
            !narrow_texts.iter().any(|t| t == hidden_in_narrow),
            "narrow sidebar (<230pt) must hide `{hidden_in_narrow}` to keep titles legible: {narrow_texts:?}"
        );
    }
    assert!(narrow_texts.iter().any(|t| t == "#86 @developer"));
}

#[test]
fn two_level_folding_hides_children_preserves_summary_badges_and_supports_option_a_activation() {
    use crate::terminals::{
        labels::{GroupStatusSummary, group_status_summary},
        tree::{TreeNode, project_fold_key, scope_fold_key, visible_project_tree},
        worktrees::location_key,
    };
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountItem, NodeId, element::JsEvent,
        geometry::size, prelude::*,
    };
    use std::collections::{BTreeSet, HashMap, HashSet};

    // 1. Pure unit tests for `visible_project_tree` and `group_status_summary`.
    fn fold_probe_snapshot() -> Snapshot {
        let mut s = fixture();
        s.projects[0].name = "Alpha".into();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[1].name = "Beta".into();
        s.projects[1].directory = "/repo/p2".into();
        s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
        s.cards[0].branch = Some("canban/1".into());
        s.cards[1].worktree = Some("/repo/p2-wt/c2".into());
        s.cards[1].branch = Some("canban/2".into());

        // Add a second shell in Card #1's worktree and a shell in Alpha's ProjectRoot.
        let mut root_shell = s.terminals[0].clone();
        root_shell.id = "p1-root-shell".into();
        root_shell.conversation_id = None;
        root_shell.agent = None;
        root_shell.name = Some("Terminal 1".into());
        root_shell.origin_project_id = Some("p1".into());
        root_shell.origin_card_id = None;
        root_shell.runtime = Some(
            serde_json::from_value(json!({
                "state":"running","connection":"connected","core_id":"core","terminal":"pty-root"
            }))
            .unwrap(),
        );
        let mut c1_shell = root_shell.clone();
        c1_shell.id = "c1-shell-2".into();
        c1_shell.name = Some("Terminal 2".into());
        c1_shell.origin_card_id = Some("c1".into());
        c1_shell.runtime.as_mut().unwrap().terminal = Some("pty-c1-2".into());
        s.terminals.extend([root_shell, c1_shell]);
        observe_fixture_locations(&mut s);

        // Make c1's agent Blocked and p2's agent Error for summary priority assertions.
        s.terminals[0].agent.as_mut().unwrap().observed = Some(json!({
            "state": "live",
            "status": { "state": "blocked", "kind": "permission", "title": "Approve", "msg": "ls" }
        }));
        s.terminals[1].agent.as_mut().unwrap().observed = Some(json!({
            "state": "live",
            "status": { "state": "error" }
        }));
        s
    }

    let s = fold_probe_snapshot();

    let rows = crate::terminals::entries(&s);
    let c1_agent_id = s.terminals[0].id.clone();
    let c1_agent_entry = rows.iter().find(|e| e.saved.id == c1_agent_id).unwrap();
    let c1_loc = location_key(c1_agent_entry);
    let p1_root_entry = rows.iter().find(|e| e.saved.id == "p1-root-shell").unwrap();
    let p1_root_loc = location_key(p1_root_entry);

    let summary = group_status_summary(rows.iter().filter(|e| location_key(e) == c1_loc));
    assert_eq!(
        summary,
        GroupStatusSummary {
            total: 2,
            blocked: 1,
            ..Default::default()
        }
    );

    // Folding c1's WorktreeScope hides its 2 Terminal nodes while keeping Scope and other scopes visible.
    let mut folds = BTreeSet::new();
    folds.insert(scope_fold_key(&c1_loc));
    let folded_scope_tree = visible_project_tree(&s, rows.iter().filter(|e| e.listed()), &folds);
    assert!(folded_scope_tree.contains(&TreeNode::Scope(
        c1_loc.clone(),
        crate::terminals::worktrees::WorktreeScope::CardWorktree("c1".into()),
    )));
    assert!(!folded_scope_tree.contains(&TreeNode::Terminal(c1_agent_id.clone())));
    assert!(!folded_scope_tree.contains(&TreeNode::Terminal("c1-shell-2".into())));
    assert!(folded_scope_tree.contains(&TreeNode::Terminal("p1-root-shell".into())));

    // Folding Project p1 hides both its Scopes and all their Terminals, while Beta stays visible.
    folds.insert(project_fold_key("p1"));
    let folded_proj_tree = visible_project_tree(&s, rows.iter().filter(|e| e.listed()), &folds);
    assert!(folded_proj_tree.contains(&TreeNode::Project("p1".into())));
    assert!(!folded_proj_tree.contains(&TreeNode::Scope(
        c1_loc.clone(),
        crate::terminals::worktrees::WorktreeScope::CardWorktree("c1".into()),
    )));
    assert!(!folded_proj_tree.contains(&TreeNode::Scope(
        p1_root_loc.clone(),
        crate::terminals::worktrees::WorktreeScope::ProjectRoot("p1".into()),
    )));
    assert!(folded_proj_tree.contains(&TreeNode::Project("p2".into())));
    assert!(folded_proj_tree.contains(&TreeNode::Terminal(s.terminals[1].id.clone())));

    // 2. Interactive mounted check for Option A, chevron fold toggle, top tab switch, and external reveal.
    #[component]
    fn FoldProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        provide_context(canban_ui_kit::motion::ReducedMotion(signal(true)));
        let ui = crate::presenter::Ui::new();
        let s = fold_probe_snapshot();
        let c1_agent_id = s.terminals[0].id.clone();
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());
        ui.open_terminal("p1-root-shell".into());
        crate::terminals::tabs::context_updates(ui);

        let ext_open_id = c1_agent_id;
        view()
            .w(px(800.))
            .h(px(640.))
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("External Open C1 Agent".into()))
                    .on_press(move |_: &()| ui.open_terminal(ext_open_id.clone())),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Cycle Tab Forward".into()))
                    .on_press(move |_: &()| crate::terminals::cycle(ui, true)),
            )
            .child(crate::terminals::sidebar_for_test(ui))
    }

    struct Mounted {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, String>,
        labels: HashMap<String, NodeId>,
        test_ids: HashMap<String, NodeId>,
    }
    impl Mounted {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                labels: HashMap::new(),
                test_ids: HashMap::new(),
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
                        if let Some(tid) = &props.test_id {
                            self.test_ids.insert(tid.to_string(), *id);
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

    fn pump(core: &mut Core<ApproxTextEngine>, m: &mut Mounted) {
        for _ in 0..8 {
            let had_batch = if let Some(batch) = core.commit() {
                m.apply(&batch);
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

    // Use reduced motion in Core tests so fade-out removes collapsed rows immediately without timer waits.
    let mut core = Core::new(ApproxTextEngine, size(px(800.), px(640.)));
    core.set_components(nsh_base::registry());
    core.mount_root(&nsh_core::element::Root::of::<FoldProbe>());
    let mut tree = Mounted::new();
    pump(&mut core, &mut tree);

    // Collapse `canban/1` worktree via its chevron button -> Terminal rows leave mounted tree,
    // and folded summary count `2` appears on the collapsed header.
    let collapse_c1 = *tree
        .labels
        .get("Collapse worktree canban/1")
        .expect("Collapse worktree canban/1 button");
    assert!(core.component_event(collapse_c1, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels.contains_key("Expand worktree canban/1"),
        "chevron label must flip to Expand after folding"
    );
    let collapsed_texts = tree.mounted_texts();
    assert!(
        collapsed_texts.iter().any(|t| t == "2"),
        "collapsed worktree header must display terminal count `2`: {collapsed_texts:?}"
    );
    assert!(
        !collapsed_texts.iter().any(|t| t == "Terminal 2"),
        "collapsed worktree children must be hidden from mounted texts: {collapsed_texts:?}"
    );

    // Option A: clicking the collapsed worktree body (`Worktree canban/1 · Collapsed · ...`)
    // activates its terminal while KEEPING `canban/1` collapsed!
    let scope_body_label = tree
        .labels
        .iter()
        .find(|(k, id)| k.starts_with("Worktree canban/1 · Collapsed") && tree.is_mounted(**id))
        .map(|(_, id)| *id)
        .expect("collapsed worktree body button");
    assert!(core.component_event(scope_body_label, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels
            .get("Expand worktree canban/1")
            .is_some_and(|id| tree.is_mounted(*id)),
        "Option A: activating a collapsed worktree via its body must keep the worktree collapsed"
    );

    // Cycling tabs within the collapsed worktree also keeps it collapsed!
    let cycle_btn = *tree.labels.get("Cycle Tab Forward").unwrap();
    assert!(core.component_event(cycle_btn, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels
            .get("Expand worktree canban/1")
            .is_some_and(|id| tree.is_mounted(*id)),
        "cycling tabs inside a collapsed worktree must keep it collapsed"
    );

    // Collapse Project `Alpha` -> hides `canban/1` and `main`, shows summary count `3` on Project Alpha.
    let collapse_alpha = *tree
        .labels
        .get("Project Alpha")
        .expect("Project Alpha fold button");
    assert!(core.component_event(collapse_alpha, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels
            .keys()
            .any(|k| k.starts_with("Project Alpha · Collapsed · 3 terminals")),
        "collapsed Project Alpha must report 3 terminals: {:?}",
        tree.labels.keys().collect::<Vec<_>>()
    );

    // External jump (`ui.open_terminal`) automatically unfolds BOTH Project Alpha and `canban/1`!
    let ext_open = *tree.labels.get("External Open C1 Agent").unwrap();
    assert!(core.component_event(ext_open, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels
            .get("Collapse worktree canban/1")
            .is_some_and(|id| tree.is_mounted(*id)),
        "external open_terminal must auto-reveal both Project and WorktreeScope"
    );
    assert!(
        tree.mounted_texts().iter().any(|t| t == "Terminal 2"),
        "expanded worktree must render its terminal rows again"
    );

    // Collapse other worktrees button collapses Beta and p1-root while keeping active `canban/1` open.
    let collapse_others = *tree
        .labels
        .get("Collapse other worktrees")
        .expect("Collapse other worktrees header button");
    assert!(core.component_event(collapse_others, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels
            .get("Expand all worktrees")
            .is_some_and(|id| tree.is_mounted(*id)),
        "header button must switch to Expand all worktrees when others are collapsed"
    );
    assert!(
        tree.labels
            .get("Collapse worktree canban/1")
            .is_some_and(|id| tree.is_mounted(*id)),
        "active worktree canban/1 must remain expanded after Collapse other worktrees"
    );
    assert!(
        tree.labels
            .iter()
            .any(|(k, id)| k.starts_with("Project Beta · Collapsed") && tree.is_mounted(*id)),
        "other project Beta must be collapsed"
    );

    // Expand all worktrees restores everything.
    let expand_all = *tree.labels.get("Expand all worktrees").unwrap();
    assert!(core.component_event(expand_all, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);
    assert!(
        tree.labels
            .get("Collapse other worktrees")
            .is_some_and(|id| tree.is_mounted(*id)),
        "header button must revert to Collapse other worktrees after Expand all"
    );
}

#[test]
fn top_right_project_card_and_cwd_context_labels_are_vertically_centered_in_terminal_header() {
    use nsh_core::{
        ApproxTextEngine, Bounds, Core, EventKind, EventPayload, MountItem, NodeId, Points,
        element::JsEvent, geometry::size, prelude::*,
    };
    use std::collections::{HashMap, HashSet};

    #[component]
    fn ContextCenterProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        s.projects[0].name = "Alpha Project".into();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[0].target_branch = Some("main".into());
        s.cards[0].title = "Card Context Title".into();
        s.cards[0].worktree = Some("/repo/p1-worktrees/c1".into());
        s.cards[0].branch = Some("canban/1".into());

        let mk_shell = |id: &str, num: u64, proj: Option<&str>, card: Option<&str>| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: num,
            name: Some(format!("Terminal {num}")),
            origin_project_id: proj.map(str::to_owned),
            origin_card_id: card.map(str::to_owned),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
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
            mk_shell("project-shell", 10, Some("p1"), None),
            mk_shell("card-shell", 11, Some("p1"), Some("c1")),
            mk_shell("external-shell", 12, None, None),
        ]);
        s.terminal_contexts.extend([
            TerminalContext {
                terminal_id: "project-shell".into(),
                core_id: "core".into(),
                terminal: "pty-project-shell".into(),
                cwd: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
            TerminalContext {
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
                terminal_id: "external-shell".into(),
                core_id: "core".into(),
                terminal: "pty-external-shell".into(),
                cwd: Some("/opt/external-dir".into()),
                ..Default::default()
            },
        ]);

        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());
        ui.page.set("terminals".into());
        ui.open_terminal("project-shell".into());

        view()
            .w(px(1200.))
            .h(px(42.))
            .child(
                canban_ui_kit::press_target()
                    .w(px(0.))
                    .h(px(0.))
                    .accessibility_label(Some("Switch to Card Shell".into()))
                    .on_press(move |_: &()| ui.open_terminal("card-shell".into())),
            )
            .child(
                canban_ui_kit::press_target()
                    .w(px(0.))
                    .h(px(0.))
                    .accessibility_label(Some("Switch to External Shell".into()))
                    .on_press(move |_: &()| ui.open_terminal("external-shell".into())),
            )
            .child(crate::terminals::tabs::header(ui))
    }

    struct LayoutTree {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        frames: HashMap<NodeId, Bounds<Points>>,
        texts: HashMap<NodeId, String>,
        labels: HashMap<String, NodeId>,
    }

    impl LayoutTree {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                frames: HashMap::new(),
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
                        self.frames.remove(id);
                        self.texts.remove(id);
                    }
                    MountItem::Frame { id, frame } => {
                        self.frames.insert(*id, *frame);
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
        fn page_center_y(&self, mut id: NodeId) -> Option<(f32, Bounds<Points>, Bounds<Points>)> {
            let text_frame = *self.frames.get(&id)?;
            let parent_id = *self.parents.get(&id)?;
            let parent_frame = *self.frames.get(&parent_id)?;
            let mut y = 0.0f32;
            let mut guard = 0;
            while id != nsh_core::ROOT_ID {
                let f = self.frames.get(&id)?;
                y += f.origin.y.0;
                id = *self.parents.get(&id)?;
                guard += 1;
                if guard > 256 {
                    return None;
                }
            }
            Some((y + text_frame.size.height.0 * 0.5, text_frame, parent_frame))
        }
        fn find_mounted_text(&self, expected: &str) -> NodeId {
            *self
                .texts
                .iter()
                .find(|(id, t)| self.is_mounted(**id) && t.as_str() == expected)
                .unwrap_or_else(|| {
                    panic!(
                        "mounted text `{expected}` not found in {:?}",
                        self.texts
                            .iter()
                            .filter(|(id, _)| self.is_mounted(**id))
                            .map(|(_, t)| t.clone())
                            .collect::<Vec<_>>()
                    )
                })
                .0
        }
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut LayoutTree) {
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
    core.mount_root(&nsh_core::element::Root::of::<ContextCenterProbe>());
    let mut tree = LayoutTree::new();
    pump(&mut core, &mut tree);

    // 1. Project context button (`Alpha Project`): must be vertically centered inside its 30pt button
    //    and inside the 42pt header bar (`center_y` == 21.0pt, `text_frame.origin.y > 0`).
    let proj_text_id = tree.find_mounted_text("Alpha Project");
    let (proj_center_y, proj_frame, proj_parent_frame) = tree
        .page_center_y(proj_text_id)
        .expect("project text frame");
    assert!(
        (proj_parent_frame.size.height.0 - 30.0).abs() < 0.5,
        "expected 30pt project button height, got {proj_parent_frame:?}"
    );
    assert!(
        proj_frame.origin.y.0 > 2.0,
        "project title must not stick to top of its 30pt button: text={proj_frame:?}, parent={proj_parent_frame:?}"
    );
    let inner_center_diff = ((proj_frame.origin.y.0 + proj_frame.size.height.0 * 0.5)
        - proj_parent_frame.size.height.0 * 0.5)
        .abs();
    assert!(
        inner_center_diff <= 1.0,
        "project title must be vertically centered within its 30pt button (diff={inner_center_diff}): text={proj_frame:?}, parent={proj_parent_frame:?}"
    );
    assert!(
        (proj_center_y - 21.0).abs() <= 1.5,
        "project title must be vertically centered in 42pt header (center_y={proj_center_y}): text={proj_frame:?}"
    );

    // Also compare against the neighboring worktree switcher pill branch label (`main`).
    let branch_text_id = tree.find_mounted_text("main");
    let (branch_center_y, _, _) = tree
        .page_center_y(branch_text_id)
        .expect("branch pill text frame");
    assert!(
        (proj_center_y - branch_center_y).abs() <= 1.0,
        "project title center_y ({proj_center_y}) must align with worktree switcher pill ({branch_center_y})"
    );

    // 2. Card context button (`#1 Card Context Title`): shares the same vertical center.
    let switch_card = *tree
        .labels
        .get("Switch to Card Shell")
        .expect("Switch to Card Shell");
    assert!(core.component_event(switch_card, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);

    let card_text_id = tree.find_mounted_text("#1 Card Context Title");
    let (card_center_y, card_frame, card_parent_frame) =
        tree.page_center_y(card_text_id).expect("card text frame");
    assert!(
        ((card_frame.origin.y.0 + card_frame.size.height.0 * 0.5)
            - card_parent_frame.size.height.0 * 0.5)
            .abs()
            <= 1.0,
        "card title must be vertically centered in its 30pt button: {card_frame:?} in {card_parent_frame:?}"
    );
    assert!(
        (card_center_y - proj_center_y).abs() <= 0.5,
        "project title ({proj_center_y}) and card title ({card_center_y}) must share identical vertical alignment"
    );

    // 3. Unassigned external cwd context (`/opt/external-dir`): shares the same vertical center.
    let switch_ext = *tree
        .labels
        .get("Switch to External Shell")
        .expect("Switch to External Shell");
    assert!(core.component_event(switch_ext, EventKind::Press, &EventPayload::None));
    pump(&mut core, &mut tree);

    let ext_text_id = tree.find_mounted_text("/opt/external-dir");
    let (ext_center_y, ext_frame, ext_parent_frame) = tree
        .page_center_y(ext_text_id)
        .expect("external cwd text frame");
    assert!(
        ((ext_frame.origin.y.0 + ext_frame.size.height.0 * 0.5)
            - ext_parent_frame.size.height.0 * 0.5)
            .abs()
            <= 1.0,
        "external cwd title must be vertically centered in its 30pt container: {ext_frame:?} in {ext_parent_frame:?}"
    );
    assert!(
        (ext_center_y - proj_center_y).abs() <= 1.0,
        "external cwd ({ext_center_y}) must align vertically with project title ({proj_center_y})"
    );
}

#[test]
fn live_directory_projection_moves_agents_without_reassigning_tasks() {
    use crate::terminals::{
        tree::{TreeNode, project_tree},
        worktrees::{WorktreeScope, entry_scope, location_key},
    };
    let mut s = fixture();
    let terminal = s.terminals[0].id.clone();
    let task_card = s.terminals[0].agent.as_ref().unwrap().card_id.clone();
    s.terminal_contexts = vec![TerminalContext {
        terminal_id: terminal.clone(),
        cwd: Some("/external/src".into()),
        directory_root: Some("/external".into()),
        ..Default::default()
    }];
    let rows = crate::terminals::entries(&s);
    let e = rows.iter().find(|e| e.saved.id == terminal).unwrap();
    assert_eq!(e.agent.as_ref().unwrap().card.id, task_card);
    assert!(
        matches!(entry_scope(&s,e), WorktreeScope::Directory { project: None, ref path, .. } if path == "/external")
    );
    let before = project_tree(&s, std::slice::from_ref(e));
    assert!(!before.iter().any(|n| matches!(n, TreeNode::Project(_))));
    assert_eq!(before.first(), Some(&TreeNode::OtherHeader));
    let key = location_key(e);
    s.terminal_contexts[0].cwd = Some("/external/tests".into());
    let rows = crate::terminals::entries(&s);
    let e = rows.iter().find(|e| e.saved.id == terminal).unwrap();
    assert_eq!(location_key(e), key);
    assert_eq!(project_tree(&s, std::slice::from_ref(e)), before);
    s.terminal_contexts[0].directory_root = Some("/elsewhere".into());
    s.terminal_contexts[0].cwd = Some("/elsewhere".into());
    let rows = crate::terminals::entries(&s);
    let e = rows.iter().find(|e| e.saved.id == terminal).unwrap();
    assert_ne!(location_key(e), key);
    assert!(project_tree(&s, std::slice::from_ref(e)).contains(&TreeNode::Terminal(terminal)));
    assert_eq!(e.agent.as_ref().unwrap().card.id, task_card);
    assert_eq!(project_tree(&s, &[]), vec![TreeNode::Empty]);
}

#[test]
fn shell_origin_does_not_override_observed_external_directory_and_status_does_not_change_tree() {
    use crate::terminals::{tree::project_tree, worktrees::entry_scope};
    let mut s = fixture();
    let mut shell = s.terminals[0].clone();
    shell.id = "external-shell".into();
    shell.agent = None;
    shell.conversation_id = None;
    shell.origin_card_id = None;
    shell.origin_project_id = Some("p1".into());
    shell.runtime = Some(serde_json::from_value(json!({"state":"running", "connection":"connected", "core_id":"core", "terminal":"shell"})).unwrap());
    s.terminals.push(shell);
    s.terminal_contexts.push(TerminalContext {
        terminal_id: "external-shell".into(),
        cwd: Some("/outside".into()),
        directory_root: Some("/outside".into()),
        ..Default::default()
    });
    let rows = crate::terminals::entries(&s);
    let e = rows
        .iter()
        .find(|e| e.saved.id == "external-shell")
        .unwrap()
        .clone();
    assert!(e.project_id.is_none());
    let before = project_tree(&s, std::slice::from_ref(&e));
    let mut updated = e.clone();
    updated.saved.runtime.as_mut().unwrap().agent_status = Some("working".into());
    updated.location.as_mut().unwrap().dirty = true;
    assert_eq!(project_tree(&s, std::slice::from_ref(&updated)), before);
    updated.host_id = "another-host".into();
    assert_ne!(entry_scope(&s, &updated), entry_scope(&s, &e));
}

#[test]
fn worktree_merge_badge_follows_observed_card_and_delivery_target() {
    use crate::terminals::worktrees::{WorktreeScope, entry_scope, scope_resolution_label};
    let mut s = fixture();
    observe_fixture_locations(&mut s);
    s.cards[0].delivery = Some(Delivery {
        target_branch: "release".into(),
        commit: "abc123".into(),
        request_key: "merge".into(),
        merged_at: 1,
    });
    // The second Card's Agent has cd'd into the first Card's dirty worktree.
    let tid = s.terminals[1].id.clone();
    let context = s
        .terminal_contexts
        .iter_mut()
        .find(|c| c.terminal_id == tid)
        .unwrap();
    context.card_id = Some("c1".into());
    context.project_id = Some("p1".into());
    context.cwd = Some("/fixture/worktrees/c1/src".into());
    context.directory_root = Some("/fixture/worktrees/c1".into());
    context.dirty = true;
    let rows = crate::terminals::entries(&s);
    let e = rows.iter().find(|e| e.saved.id == tid).unwrap();
    assert_eq!(e.agent.as_ref().unwrap().card.id, "c2");
    let scope = entry_scope(&s, e);
    assert_eq!(scope, WorktreeScope::CardWorktree("c1".into()));
    assert_eq!(
        scope_resolution_label(&s, &scope).as_deref(),
        Some("Resolved (merged into release)")
    );
    assert_eq!(
        scope_resolution_label(&s, &WorktreeScope::ProjectRoot("p1".into())),
        None
    );
    assert_eq!(
        scope_resolution_label(&s, &WorktreeScope::ManagedWorktree("wt".into())),
        None
    );
    assert_eq!(
        scope_resolution_label(&s, &WorktreeScope::CardWorktree("c2".into())),
        None
    );

    s.cards[0].delivery = None;
    s.cards[0].pull_request = Some(
        serde_json::from_value(json!({
            "repository":"owner/repo", "repository_id":1, "number":42,
            "url":"https://github.com/owner/repo/pull/42", "state":"closed", "head":"abc",
            "head_branch":"feature", "base_branch":"stable", "merged":true, "checks":"",
            "workspace":{"directory":"/repo","path":"/wt"}
        }))
        .unwrap(),
    );
    assert_eq!(
        scope_resolution_label(&s, &scope).as_deref(),
        Some("Resolved (merged into stable)")
    );
    s.cards[0]
        .pull_request
        .as_mut()
        .unwrap()
        .base_branch
        .clear();
    assert_eq!(
        scope_resolution_label(&s, &scope).as_deref(),
        Some("Resolved")
    );
    s.cards[0].pull_request.as_mut().unwrap().merged = false;
    assert_eq!(
        scope_resolution_label(&s, &scope),
        None,
        "closed PR is not merged"
    );
    s.cards[0].resolution = Some(canban_types::CardResolution {
        resolved_at: 2,
        kind: "manual".into(),
        reason: None,
        actor: "user".into(),
    });
    assert_eq!(
        scope_resolution_label(&s, &scope).as_deref(),
        Some("Resolved")
    );
    s.cards[0].resolution = None;
    s.cards[0].pull_request.as_mut().unwrap().merged = true;
    s.cards[0].archived_at = Some(2);
    assert_eq!(scope_resolution_label(&s, &scope), None);
    assert!(
        !matches!(entry_scope(&s, e), WorktreeScope::CardWorktree(_)),
        "stale context must not match an archived Card"
    );
}

#[test]
fn retained_directory_rows_survive_cd_and_unrelated_status_updates() {
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountItem, NodeId, element::JsEvent,
        geometry::size, prelude::*,
    };
    use std::collections::{HashMap, HashSet};
    #[component]
    fn RetainedProbe() -> impl IntoElement {
        let ui = crate::presenter::Ui::new();
        let mut s = fixture();
        observe_fixture_locations(&mut s);
        ui.snapshot.set(s);
        let tid = ui.snapshot.get_untracked().terminals[0].id.clone();
        let move_id = tid.clone();
        view()
            .w(px(300.))
            .h(px(640.))
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Move directory".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            let c = s
                                .terminal_contexts
                                .iter_mut()
                                .find(|c| c.terminal_id == move_id)
                                .unwrap();
                            c.cwd = Some("/external".into());
                            c.directory_root = c.cwd.clone();
                            c.project_id = None;
                            c.card_id = None;
                        })
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Update status".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            let c = s
                                .terminal_contexts
                                .iter_mut()
                                .find(|c| c.terminal_id == tid)
                                .unwrap();
                            c.dirty = !c.dirty;
                        })
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Merge Card".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            s.cards[0].delivery = Some(Delivery {
                                target_branch: "release".into(),
                                commit: "abc123".into(),
                                request_key: "merge".into(),
                                merged_at: 1,
                            });
                        })
                    }),
            )
            .child(crate::terminals::sidebar_for_test(ui))
    }
    #[derive(Default)]
    struct Mounted {
        keys: HashMap<String, NodeId>,
        labels: HashMap<String, NodeId>,
        deleted: HashSet<NodeId>,
    }
    fn pump(core: &mut Core<ApproxTextEngine>, m: &mut Mounted) {
        for _ in 0..8 {
            if let Some(batch) = core.commit() {
                for item in batch.items {
                    match item {
                        MountItem::ViewProps { id, props } => {
                            if let Some(key) = props.test_id {
                                m.keys.insert(key.to_string(), id);
                            }
                            if let Some(label) = props.accessibility.label {
                                m.labels.insert(label.to_string(), id);
                            }
                        }
                        MountItem::Delete { id } => {
                            m.deleted.insert(id);
                        }
                        _ => {}
                    }
                }
            }
            let mut events = Vec::new();
            core.take_events(&mut events);
            for ev in events {
                match ev {
                    JsEvent::Event(id, kind, payload) => {
                        core.component_event(id, kind, &payload);
                    }
                    JsEvent::LayoutListener {
                        host,
                        id,
                        listener,
                        frame,
                    } => core.component_layout(host, id, &listener, frame),
                    _ => {}
                }
            }
        }
    }
    let mut core = Core::new(ApproxTextEngine, size(px(300.), px(640.)));
    core.set_components(nsh_base::registry());
    core.mount_root(&nsh_core::element::Root::of::<RetainedProbe>());
    let mut m = Mounted::default();
    pump(&mut core, &mut m);
    let keys: Vec<_> = m
        .keys
        .iter()
        .filter(|(k, _)| k.starts_with("terminal:"))
        .map(|(k, id)| (k.clone(), *id))
        .collect();
    assert_eq!(keys.len(), 2);
    for action in [
        "Update status",
        "Merge Card",
        "Move directory",
        "Update status",
    ] {
        assert!(core.component_event(m.labels[action], EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut m);
        if action == "Merge Card" {
            assert!(
                m.labels
                    .keys()
                    .any(|l| l.ends_with(" · Resolved (merged into release)")),
                "delivery-only snapshot change must update the mounted worktree header"
            );
        }
        for (key, id) in &keys {
            assert_eq!(m.keys[key], *id, "row remounted during {action}");
            assert!(
                !m.deleted.contains(id),
                "retained row deleted during {action}"
            );
        }
    }
    assert!(
        m.keys.contains_key("section:other"),
        "expected section:other divider header after moving directory outside projects"
    );
}

#[test]
fn external_directories_render_under_other_section_header_and_compact_local_home_with_tilde() {
    use crate::terminals::{
        tree::{TreeNode, project_tree},
        worktrees::{WorktreeScope, entry_scope},
    };
    let mut s = fixture();
    observe_fixture_locations(&mut s);
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/test".into());
    let home_trimmed = home.trim_end_matches('/');
    let scratch_path = format!("{home_trimmed}/Workspace/scratch");

    let mut local_shell = s.terminals[0].clone();
    local_shell.id = "local-home-shell".into();
    local_shell.agent = None;
    local_shell.conversation_id = None;
    local_shell.origin_card_id = None;
    local_shell.origin_project_id = None;
    local_shell.runtime = Some(
        serde_json::from_value(json!({
            "state": "running",
            "connection": "connected",
            "core_id": "core",
            "terminal": "pty-local-home"
        }))
        .unwrap(),
    );
    s.terminals.push(local_shell);
    s.terminal_contexts.push(TerminalContext {
        terminal_id: "local-home-shell".into(),
        cwd: Some(scratch_path.clone()),
        directory_root: Some(scratch_path.clone()),
        branch: Some("spike".into()),
        ..Default::default()
    });

    let mut remote_shell = s.terminals[0].clone();
    remote_shell.id = "remote-shell".into();
    remote_shell.agent = None;
    remote_shell.conversation_id = None;
    remote_shell.origin_card_id = None;
    remote_shell.origin_project_id = None;
    remote_shell.runtime = Some(
        serde_json::from_value(json!({
            "state": "running",
            "connection": "connected",
            "core_id": "core",
            "terminal": "pty-remote"
        }))
        .unwrap(),
    );
    s.terminals.push(remote_shell);
    s.entity_hosts
        .insert("remote-shell".into(), "devbox".into());
    s.terminal_contexts.push(TerminalContext {
        terminal_id: "remote-shell".into(),
        cwd: Some(scratch_path.clone()),
        directory_root: Some(scratch_path.clone()),
        ..Default::default()
    });

    let rows = crate::terminals::entries(&s);
    let tree = project_tree(&s, rows.iter().filter(|e| e.listed()));

    // Registered projects come first, followed by OtherHeader before external scopes.
    let other_idx = tree
        .iter()
        .position(|n| matches!(n, TreeNode::OtherHeader))
        .expect("OtherHeader must separate external directories from the last project");
    let last_proj_idx = tree
        .iter()
        .rposition(|n| matches!(n, TreeNode::Project(_)))
        .expect("at least one project must precede OtherHeader");
    assert!(other_idx > last_proj_idx);

    let local_entry = rows
        .iter()
        .find(|e| e.saved.id == "local-home-shell")
        .unwrap();
    let local_scope = entry_scope(&s, local_entry);
    assert!(matches!(
        local_scope,
        WorktreeScope::Directory { project: None, .. }
    ));
    if !home_trimmed.is_empty() {
        assert_eq!(
            local_scope.display_name(&s, std::slice::from_ref(local_entry), None),
            "~/Workspace/scratch · spike"
        );
    }

    // Remote host paths must NOT rewrite using the local Mac's $HOME.
    let remote_entry = rows.iter().find(|e| e.saved.id == "remote-shell").unwrap();
    let remote_scope = entry_scope(&s, remote_entry);
    assert_eq!(
        remote_scope.display_name(&s, std::slice::from_ref(remote_entry), None),
        scratch_path
    );
}

#[test]
fn compact_terminal_tabs_never_push_notifications_or_hosts_out_of_header_bounds() {
    use nsh_core::{
        ApproxTextEngine, Bounds, Core, EventKind, EventPayload, MountItem, NodeId, Points,
        element::JsEvent, geometry::size, prelude::*,
    };
    use std::collections::{HashMap, HashSet};

    // 1. Unit checks for compact tab labels and widths across scopes.
    let mut s = fixture();
    s.projects[0].directory = "/repo/p1".into();
    s.cards[0].number = 124;
    s.cards[0].stage = "develop".into();
    s.cards[0].title =
        "terminal tab 上如果显示了 card title + column + agent 会非常长，现在还会把通知中心挤到窗口外"
            .into();
    s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
    observe_fixture_locations(&mut s);

    let entries = crate::terminals::entries(&s);
    let card_agent = entries
        .iter()
        .find(|e| e.card.as_ref().is_some_and(|c| c.id == "c1") && e.agent.is_some())
        .expect("card agent entry");
    let expected_profile = format!("@{}", card_agent.profile().unwrap());
    assert_eq!(
        crate::terminals::labels::tab_label_text(card_agent, false),
        expected_profile
    );
    assert_eq!(
        crate::terminals::labels::tab_label_text(card_agent, true),
        format!("#124 {expected_profile}")
    );
    assert!(
        crate::terminals::labels::tab_width(card_agent, false) <= 210.0,
        "compact agent tab width must stay <= 210pt"
    );
    assert!(
        crate::terminals::labels::tab_width(card_agent, true) <= 210.0,
        "shared-scope agent tab width with #num must stay <= 210pt"
    );

    // 2. Layout checks across widths (884pt normal split, 640pt narrow, 420pt extreme narrow)
    //    with 5 tabs in the same CardWorktree, remote hosts pill, and notification alerts.
    fn run_layout_probe(header_width: f32, sidebar_open: bool) {
        #[component]
        fn BoundsProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = crate::presenter::Ui::new();
            let mut s = fixture();
            s.projects[0].directory = "/repo/p1".into();
            s.projects[0].target_branch = Some("main".into());
            s.cards[0].number = 124;
            s.cards[0].stage = "develop".into();
            s.cards[0].title =
                "terminal tab 上如果显示了 card title + column + agent 会非常长，现在还会把通知中心挤到窗口外"
                    .into();
            s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
            s.cards[0].branch = Some("canban-0afd/124-very-long-branch-name".into());
            s.cards[0].attention = Some("Needs review".into());
            s.host_summaries.insert(
                "gpu-box".into(),
                HostSummary {
                    id: "gpu-box".into(),
                    name: "gpu-box".into(),
                    is_local: false,
                    transport_label: "SSH".into(),
                    endpoint_label: "gpu-box".into(),
                    ssh_target: Some("gpu-box".into()),
                    server_id: None,
                    connection: HostConnectionState::Connected {
                        rtt_ms: 4,
                        api_version: canban_updater::DESKTOP_API,
                        version: "test".into(),
                        platform: "linux-x86_64".into(),
                    },
                    active_agents: 2,
                    max_agents: 4,
                    project_count: 1,
                    reconnect_count: 0,
                    last_synced_at: None,
                    pending_mutations: 0,
                    sync_conflicts: 0,
                },
            );

            // Add a second managed agent + 3 shells in the same CardWorktree (5 tabs total).
            let mut rev_run = s.terminals[0].agent.clone().unwrap();
            rev_run.profile.id = "reviewer".into();
            rev_run.profile.preset = Some("claude".into());
            rev_run.stage = "review".into();
            rev_run.terminal = Some("pty-rev".into());
            let rev_id = Conversation::key("c1", "reviewer");
            s.terminals.push(TerminalEntry {
                id: rev_id.clone(),
                conversation_id: Some(rev_id),
                number: 2,
                name: None,
                origin_project_id: Some("p1".into()),
                origin_card_id: Some("c1".into()),
                agent_engine: None,
                runtime: None,
                agent: Some(rev_run),
                created_at: 2,
            });
            for idx in 1..=3u64 {
                let id = format!("c1-shell-{idx}");
                s.terminals.push(TerminalEntry {
                    id: id.clone(),
                    conversation_id: None,
                    number: idx + 10,
                    name: Some(format!("Terminal {idx}")),
                    origin_project_id: Some("p1".into()),
                    origin_card_id: Some("c1".into()),
                    agent_engine: None,
                    runtime: Some(TerminalRuntime {
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
                    created_at: idx + 10,
                });
                s.terminal_contexts.push(TerminalContext {
                    terminal_id: id.clone(),
                    core_id: "core".into(),
                    terminal: format!("pty-{id}"),
                    cwd: Some("/repo/p1-wt/c1".into()),
                    project_id: Some("p1".into()),
                    card_id: Some("c1".into()),
                    branch: Some("canban-0afd/124-very-long-branch-name".into()),
                    dirty: true,
                    additions: 120,
                    deletions: 45,
                    ..Default::default()
                });
            }
            observe_fixture_locations(&mut s);

            let first_id = s.terminals[0].id.clone();
            ui.snapshot.set(s);
            ui.snapshot_ready.set(true);
            ui.project.set("p1".into());
            ui.page.set("terminals".into());
            ui.terminal_sidebar.set(true);
            ui.open_terminal(first_id);

            view()
                .w_full()
                .h(px(42.))
                .child(
                    canban_ui_kit::press_target()
                        .w(px(0.))
                        .h(px(0.))
                        .accessibility_label(Some("Hide Sidebar".into()))
                        .on_press(move |_: &()| ui.terminal_sidebar.set(false)),
                )
                .child(crate::terminals::tabs::header(ui))
        }

        struct FrameTree {
            alive: HashSet<NodeId>,
            parents: HashMap<NodeId, NodeId>,
            frames: HashMap<NodeId, Bounds<Points>>,
            labels: HashMap<String, NodeId>,
        }

        impl FrameTree {
            fn new() -> Self {
                Self {
                    alive: HashSet::new(),
                    parents: HashMap::new(),
                    frames: HashMap::new(),
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
                            self.frames.remove(id);
                        }
                        MountItem::Frame { id, frame } => {
                            self.frames.insert(*id, *frame);
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
            fn page_bounds(&self, mut id: NodeId) -> Option<(f32, f32)> {
                let frame = *self.frames.get(&id)?;
                let mut x = 0.0f32;
                let mut guard = 0;
                while id != nsh_core::ROOT_ID {
                    let f = self.frames.get(&id)?;
                    x += f.origin.x.0;
                    id = *self.parents.get(&id)?;
                    guard += 1;
                    if guard > 256 {
                        return None;
                    }
                }
                Some((x, x + frame.size.width.0))
            }
            fn find_label_prefix(&self, prefix: &str) -> NodeId {
                *self
                    .labels
                    .iter()
                    .find(|(lbl, id)| lbl.starts_with(prefix) && self.is_mounted(**id))
                    .unwrap_or_else(|| panic!("mounted label starting with `{prefix}` not found"))
                    .1
            }
        }

        let mut core = Core::new(ApproxTextEngine, size(px(header_width), px(42.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<BoundsProbe>());
        let mut tree = FrameTree::new();

        let pump = |core: &mut Core<ApproxTextEngine>, tree: &mut FrameTree| {
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
        };

        pump(&mut core, &mut tree);
        if !sidebar_open {
            let btn = *tree.labels.get("Hide Sidebar").unwrap();
            assert!(core.component_event(btn, EventKind::Press, &EventPayload::None));
            pump(&mut core, &mut tree);
        }

        let notif_id = tree.find_label_prefix("Activity & Notifications");
        let (notif_left, notif_right) = tree
            .page_bounds(notif_id)
            .expect("Activity & Notifications bounds");
        assert!(
            notif_right - notif_left > 20.0,
            "Notification Center pill collapsed at width {header_width}: ({notif_left}, {notif_right})"
        );
        assert!(
            notif_left >= 0.0 && notif_right <= header_width + 0.5,
            "Notification Center pushed out of {header_width}pt header (sidebar_open={sidebar_open}): left={notif_left}, right={notif_right}"
        );

        let hosts_id = tree.find_label_prefix("Connected Hosts");
        let (hosts_left, hosts_right) = tree.page_bounds(hosts_id).expect("Connected Hosts bounds");
        assert!(
            hosts_right - hosts_left > 20.0,
            "Hosts pill collapsed at width {header_width}: ({hosts_left}, {hosts_right})"
        );
        assert!(
            hosts_left >= 0.0 && hosts_right <= notif_left + 0.5,
            "Hosts pill overlaps or overflows at {header_width}pt (sidebar_open={sidebar_open}): ({hosts_left}, {hosts_right}) vs notif_left={notif_left}"
        );
    }

    for (w, sidebar_open) in [
        (1200.0, true),
        (884.0, true),
        (640.0, true),
        (420.0, true),
        (884.0, false),
        (560.0, false),
    ] {
        run_layout_probe(w, sidebar_open);
    }
}

#[test]
fn managed_classification_policy_tooltips_and_reclaim_states_are_distinct() {
    use nsh_core::{ApproxTextEngine, Core, geometry::size, prelude::*};

    let mut s = fixture();
    let cfg = Config::default();
    let dev_profile = cfg.profiles[0].id.clone();

    // Turn fixture terminal[0] into a completed un-followed-up managed Agent run.
    let agent_id = s.terminals[0].id.clone();
    {
        let a = s.terminals[0].agent.as_mut().unwrap();
        a.state = "completed".into();
        a.observed = Some(json!({"state":"live","turn":2,"status":{"state":"done"}}));
        a.connection_state = "connected".into();
        a.lifecycle.observed_at = Some(now());
    }

    // Add configuration, merge-queue, cardless scheduled, manual CLI agent, and manual plain shell.
    let make_entry =
        |id: &str, engine: Option<&str>, status: Option<&str>, state: &str| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: 9,
            name: Some(id.into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: None,
            agent_engine: engine.map(str::to_owned),
            runtime: Some(TerminalRuntime {
                core_id: Some("core-1".into()),
                terminal: Some(format!("{id}-pty")),
                state: state.into(),
                connection: "connected".into(),
                error: None,
                agent_status: status.map(str::to_owned),
                agent_turn: Some(1),
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: now(),
        };

    let cfg_id = "configuration:p1:wf1:dev";
    let mq_id = "merge-queue:b1";
    let sched_id = "scheduled:p1:r1";
    let manual_cli_id = "manual-agy";
    let manual_shell_id = "manual-shell";

    s.terminals
        .push(make_entry(cfg_id, Some("codex"), Some("idle"), "running"));
    s.terminals
        .push(make_entry(mq_id, Some("codex"), Some("working"), "running"));
    s.terminals
        .push(make_entry(sched_id, Some("codex"), Some("idle"), "running"));
    s.terminals.push(make_entry(
        manual_cli_id,
        Some("agy"),
        Some("idle"),
        "running",
    ));
    s.terminals
        .push(make_entry(manual_shell_id, None, None, "running"));

    let find = |s: &Snapshot, id: &str| {
        super::super::entries(s)
            .into_iter()
            .find(|e| e.saved.id == id)
            .unwrap()
    };

    // Classification
    let e_agent = find(&s, &agent_id);
    let e_cfg = find(&s, cfg_id);
    let e_mq = find(&s, mq_id);
    let e_sched = find(&s, sched_id);
    let e_manual_cli = find(&s, manual_cli_id);
    let e_manual_shell = find(&s, manual_shell_id);

    assert_eq!(e_agent.managed_kind(), Some(ManagedTerminalKind::Agent));
    assert_eq!(
        e_cfg.managed_kind(),
        Some(ManagedTerminalKind::Configuration)
    );
    assert_eq!(e_mq.managed_kind(), Some(ManagedTerminalKind::MergeQueue));
    assert_eq!(e_sched.managed_kind(), Some(ManagedTerminalKind::Scheduled));
    assert_eq!(e_manual_cli.managed_kind(), None);
    assert_eq!(e_manual_shell.managed_kind(), None);
    assert!(e_agent.is_managed());
    assert!(e_cfg.is_managed());
    assert!(e_mq.is_managed());
    assert!(e_sched.is_managed());
    assert!(!e_manual_cli.is_managed());
    assert!(!e_manual_shell.is_managed());

    // Hover lifecycle policy summaries
    assert_eq!(
        e_manual_cli.lifecycle_policy_summary(15),
        "Manual terminal · Never auto-reclaimed"
    );
    assert_eq!(
        e_manual_shell.lifecycle_policy_summary(15),
        "Manual terminal · Never auto-reclaimed"
    );
    assert_eq!(
        e_cfg.lifecycle_policy_summary(15),
        "Managed by Canban · Stays open until closed"
    );
    assert_eq!(
        e_mq.lifecycle_policy_summary(15),
        "Managed by Canban · Auto-reclaims after merge batch finishes"
    );
    assert_eq!(
        e_sched.lifecycle_policy_summary(15),
        "Managed by Canban · Auto-reclaims after completion"
    );
    assert_eq!(
        e_agent.lifecycle_policy_summary(15),
        "Managed by Canban · Auto-reclaims 30s after completion (history & Resume saved on Card)"
    );
    assert_eq!(
        e_agent.lifecycle_policy_summary(0),
        "Managed by Canban · Auto-reclaim disabled"
    );
    assert!(
        crate::terminals::labels::sidebar_hint(&e_agent, "Local", 15)
            .contains("Auto-reclaims 30s after completion")
    );

    // Interactive follow-up uses <N>m idle tooltip
    s.terminals[0]
        .agent
        .as_mut()
        .unwrap()
        .lifecycle
        .interactive_followup = true;
    let e_followup = find(&s, &agent_id);
    assert_eq!(
        e_followup.lifecycle_policy_summary(15),
        "Managed by Canban · Auto-reclaims after 15m idle"
    );

    // Paused / protected state
    s.reclaim_status.push(TerminalReclaimStatus {
        terminal_id: agent_id.clone(),
        paused: true,
        reclaiming_soon: false,
        auto_reclaiming: false,
        interactive_followup: false,
        deadline_at: None,
    });
    let e_paused = find(&s, &agent_id);
    assert_eq!(
        e_paused.lifecycle_policy_summary(15),
        "Managed by Canban · Auto-reclaim paused while open"
    );
    assert_eq!(e_paused.canvas_notice_status(), "");
    assert_eq!(e_paused.display_status(), "Done");

    // Pre-reclaim warning state keeps terminal writable and shows canvas + status indicator
    s.reclaim_status[0] = TerminalReclaimStatus {
        terminal_id: agent_id.clone(),
        paused: false,
        reclaiming_soon: true,
        auto_reclaiming: false,
        interactive_followup: false,
        deadline_at: Some(now() + 10_000),
    };
    let e_soon = find(&s, &agent_id);
    assert_eq!(
        e_soon.canvas_notice_status(),
        "Reclaiming soon · Focus or type to keep open"
    );
    assert_eq!(e_soon.display_status(), "Reclaiming soon");
    assert_eq!(
        e_soon.lifecycle_policy_summary(15),
        "Managed by Canban · Reclaiming soon — focus or type to keep open"
    );
    assert!(crate::terminals::labels::has_status_mark(&e_soon));
    assert_eq!(
        e_soon.binding().unwrap()["read_only"],
        json!(false),
        "Reclaiming soon must stay writable so user input cancels reclaim"
    );

    // Active idle close shows Auto-reclaiming… instead of Closing (for both Card Agent and scheduled)
    s.terminals[0].agent.as_mut().unwrap().close_state = "closing".into();
    s.terminals[0]
        .agent
        .as_mut()
        .unwrap()
        .lifecycle
        .close_reason = "idle".into();
    s.reclaim_status[0].reclaiming_soon = false;
    s.reclaim_status[0].auto_reclaiming = true;
    let e_reclaiming = find(&s, &agent_id);
    assert_eq!(e_reclaiming.display_status(), "Auto-reclaiming…");
    let key = Conversation::key("c1", &dev_profile);
    let row = conversations(&s).into_iter().find(|r| r.id == key).unwrap();
    let b = binding(&row).unwrap();
    assert_eq!(b["input_status"], json!("Auto-reclaiming…"));
    assert_eq!(b["read_only"], json!(true));
    assert!(crate::terminals::labels::has_status_mark(&e_reclaiming));

    // Scheduled entry entering closing state via reclaim_status also binds Auto-reclaiming…
    let sched_idx = s.terminals.iter().position(|t| t.id == sched_id).unwrap();
    s.terminals[sched_idx].runtime.as_mut().unwrap().state = "closing".into();
    s.reclaim_status.push(TerminalReclaimStatus {
        terminal_id: sched_id.into(),
        paused: false,
        reclaiming_soon: false,
        auto_reclaiming: true,
        interactive_followup: false,
        deadline_at: None,
    });
    let e_sched_reclaiming = find(&s, sched_id);
    assert_eq!(
        e_sched_reclaiming.binding().unwrap()["input_status"],
        json!("Auto-reclaiming…")
    );
    assert_eq!(e_sched_reclaiming.display_status(), "Auto-reclaiming…");

    // Verify managed/reclaim views construct and mount inside an NSH Core without panics.
    #[component]
    fn ReclaimBadgeProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        let mut s = fixture();
        let agent_id = s.terminals[0].id.clone();
        s.terminals[0].agent.as_mut().unwrap().state = "completed".into();
        s.reclaim_status.push(TerminalReclaimStatus {
            terminal_id: agent_id.clone(),
            paused: false,
            reclaiming_soon: true,
            auto_reclaiming: false,
            interactive_followup: false,
            deadline_at: Some(now() + 10_000),
        });
        let soon_entry = super::super::entries(&s)
            .into_iter()
            .find(|e| e.saved.id == agent_id)
            .unwrap();
        let soon_badge = crate::terminals::labels::tab_reclaim_badge(&soon_entry).unwrap();

        s.terminals[0].agent.as_mut().unwrap().close_state = "closing".into();
        s.terminals[0]
            .agent
            .as_mut()
            .unwrap()
            .lifecycle
            .close_reason = "idle".into();
        s.reclaim_status[0].reclaiming_soon = false;
        s.reclaim_status[0].auto_reclaiming = true;
        let reclaiming_entry = super::super::entries(&s)
            .into_iter()
            .find(|e| e.saved.id == agent_id)
            .unwrap();
        let reclaiming_badge =
            crate::terminals::labels::tab_reclaim_badge(&reclaiming_entry).unwrap();

        view()
            .flex_row()
            .child(crate::terminals::labels::tab(&soon_entry, false, true))
            .child(soon_badge)
            .child(crate::terminals::labels::managed_badge())
            .child(reclaiming_badge)
    }

    let mut core = Core::new(ApproxTextEngine, size(px(640.), px(48.)));
    core.set_components(nsh_base::registry());
    core.mount_root(&nsh_core::element::Root::of::<ReclaimBadgeProbe>());
    assert!(core.commit().is_some());
}
