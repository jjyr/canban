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
        let pid = card
            .map(|c| &c.project_id)
            .or_else(|| {
                s.configuration_sessions
                    .iter()
                    .find(|c| c.id == t.id)
                    .map(|c| &c.project_id)
            })
            .or_else(|| {
                s.merge_batches
                    .iter()
                    .find(|b| b.id == t.id)
                    .map(|b| &b.project_id)
            })
            .or_else(|| {
                s.scheduled_runs
                    .iter()
                    .find(|r| r.id == t.id)
                    .map(|r| &r.project_id)
            });
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
            command: None,
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
            command: None,
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
            command: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-backlog".into(),
        conversation_id: None,
        number: 4,
        name: Some("Terminal 2".into()),
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
            command: None,
        }),
        agent: None,
        created_at: 3,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 5,
        name: Some("Terminal 3".into()),
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
            command: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 4,
        name: Some("Terminal 2".into()),
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
            command: None,
        }),
        agent: None,
        created_at: 2,
    });
    s.terminals.push(TerminalEntry {
        id: "ephemeral-cli".into(),
        conversation_id: None,
        number: 4,
        name: Some("Antigravity 2".into()),
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
            command: None,
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
                command: None,
            }),
            agent: None,
            created_at: 2,
        });
        let att_id = s.terminals[0].id.clone();
        let conv_id = Conversation::key("c1", &prof_id);
        ui.snapshot.set(s);
        ui.page.set("board".into());

        // 0. Clicking the Card body (`open_card`) keeps the Terminal split collapsed (`false`),
        // even when active terminals/agents exist, while pre-selecting the actively running Agent tab.
        ui.open_card("c1".into());
        assert_eq!(ui.page.get_untracked(), "card");
        assert_eq!(ui.card.get_untracked(), "c1");
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
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
        assert_eq!(ui.card_tab.get_untracked(), conv_id);
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
                command: None,
            }),
            agent: None,
            created_at: 10,
        });
        s.terminals.push(TerminalEntry {
            id: "ephemeral-agent".into(),
            conversation_id: None,
            number: 11,
            name: Some("Antigravity 1".into()),
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
                command: None,
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
                command: None,
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
                    command: None,
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
                    command: None,
                }),
                agent: None,
                created_at: 21,
            },
            TerminalEntry {
                id: "term-2".into(),
                conversation_id: None,
                number: 22,
                name: Some("Terminal 2".into()),
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
                    command: None,
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

        // 2b. Missing `message` still surfaces the state or bounded response summary instead of opaque fallback text.
        let ctx_closing = crate::terminals::begin_optimistic_close(ui, "term-1").unwrap();
        crate::terminals::finish_optimistic_close(ui, &ctx_closing, Ok(json!({"state":"closing"})));
        let closing_err = ui.operation_error.get_untracked();
        assert!(closing_err.contains("state=closing"), "got: {closing_err}");

        let ctx_weird = crate::terminals::begin_optimistic_close(ui, "term-1").unwrap();
        crate::terminals::finish_optimistic_close(ui, &ctx_weird, Ok(json!({"ok":true})));
        let weird_err = ui.operation_error.get_untracked();
        assert!(weird_err.contains("{\"ok\":true}"), "got: {weird_err}");

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
                command: None,
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
            WorktreeScope::ProjectRoot("p1".into()),
            "newly created project terminal must land immediately in ProjectRoot instead of Other"
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
fn in_page_new_terminal_inherits_managed_and_external_worktree_scopes_even_when_project_root_is_open()
 {
    use crate::terminals::worktrees::{WorktreeScope, entry_scope};

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, ()) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.projects[0].directory = "/repo/p1".into();
        s.projects[0].target_branch = Some("main".into());
        s.worktrees.push(canban_types::ManagedWorktree {
            id: "wt1".into(),
            project_id: "p1".into(),
            branch: "feat/a".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/wt/feat-a".into(),
            directory: "/repo/p1".into(),
            created_at: 1,
        });

        let add_shell = |s: &mut Snapshot, id: &str, number: u64, ctx: TerminalContext| {
            s.terminals.push(TerminalEntry {
                id: id.into(),
                conversation_id: None,
                number,
                name: Some(format!("Terminal {number}")),
                origin_card_id: None,
                agent_engine: None,
                created_at: 1,
                runtime: Some(TerminalRuntime {
                    core_id: Some("core".into()),
                    terminal: Some(format!("t-{id}")),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                    command: None,
                }),
                agent: None,
            });
            s.terminal_contexts.push(ctx);
        };

        add_shell(
            &mut s,
            "shell-root",
            10,
            TerminalContext {
                terminal_id: "shell-root".into(),
                core_id: "core".into(),
                terminal: "t-shell-root".into(),
                cwd: Some("/repo/p1".into()),
                directory_root: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
        );
        add_shell(
            &mut s,
            "shell-wt",
            11,
            TerminalContext {
                terminal_id: "shell-wt".into(),
                core_id: "core".into(),
                terminal: "t-shell-wt".into(),
                cwd: Some("/wt/feat-a/sub".into()),
                directory_root: Some("/wt/feat-a".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt1".into()),
                branch: Some("feat/a".into()),
                ..Default::default()
            },
        );
        add_shell(
            &mut s,
            "shell-ext",
            12,
            TerminalContext {
                terminal_id: "shell-ext".into(),
                core_id: "core".into(),
                terminal: "t-shell-ext".into(),
                cwd: Some("/ext/wt-b/src".into()),
                directory_root: Some("/ext/wt-b".into()),
                project_id: Some("p1".into()),
                branch: Some("feat/ext".into()),
                ..Default::default()
            },
        );

        let ui = crate::presenter::Ui::new();
        observe_fixture_locations(&mut s);
        ui.snapshot.set(s);
        ui.snapshot_ready.set(true);
        ui.project.set("p1".into());

        // 1. Selecting a Managed Branch Worktree terminal and running in-page `new-terminal`
        //    must stay inside `ManagedWorktree("wt1")` with its subdirectory `cwd`.
        ui.open_terminal("shell-wt".into());
        crate::commands::execute(ui, "new-terminal");
        let wt_new_id = ui.terminal_selection.get_untracked();
        assert_ne!(wt_new_id, "shell-wt");
        let snap1 = ui.snapshot.get_untracked();
        let wt_new = crate::terminals::entries(&snap1)
            .into_iter()
            .find(|e| e.saved.id == wt_new_id)
            .unwrap();
        assert_eq!(
            entry_scope(&snap1, &wt_new),
            WorktreeScope::ManagedWorktree("wt1".into())
        );
        assert_eq!(
            wt_new.location.as_ref().and_then(|l| l.cwd.as_deref()),
            Some("/wt/feat-a/sub")
        );

        // 2. Selecting an external git worktree terminal and running in-page `new-terminal`
        //    must stay inside that external worktree's `Directory` scope and subdirectory `cwd`.
        ui.open_terminal("shell-ext".into());
        crate::commands::execute(ui, "new-terminal");
        let ext_new_id = ui.terminal_selection.get_untracked();
        assert_ne!(ext_new_id, "shell-ext");
        let snap2 = ui.snapshot.get_untracked();
        let ext_new = crate::terminals::entries(&snap2)
            .into_iter()
            .find(|e| e.saved.id == ext_new_id)
            .unwrap();
        assert_eq!(
            entry_scope(&snap2, &ext_new),
            WorktreeScope::Directory {
                host: "local".into(),
                path: "/ext/wt-b".into(),
                project: Some("p1".into()),
            }
        );
        assert_eq!(
            ext_new.location.as_ref().and_then(|l| l.cwd.as_deref()),
            Some("/ext/wt-b/src")
        );

        // 3. Global ⌘T from Board still opens in `ProjectRoot("p1")`.
        ui.navigate("board");
        crate::commands::execute(ui, "new-terminal");
        assert_eq!(ui.page.get_untracked(), "terminals");
        let global_id = ui.terminal_selection.get_untracked();
        let snap3 = ui.snapshot.get_untracked();
        let global_entry = crate::terminals::entries(&snap3)
            .into_iter()
            .find(|e| e.saved.id == global_id)
            .unwrap();
        assert_eq!(
            entry_scope(&snap3, &global_entry),
            WorktreeScope::ProjectRoot("p1".into())
        );
        assert_eq!(
            global_entry
                .location
                .as_ref()
                .and_then(|l| l.cwd.as_deref()),
            Some("/repo/p1")
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
            command: None,
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
            command: None,
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
    assert_eq!(
        before[..3],
        [
            TreeNode::Project("p1".into()),
            TreeNode::Project("p2".into()),
            TreeNode::OtherHeader,
        ]
    );
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
    assert_eq!(
        project_tree(&s, &[]),
        vec![
            TreeNode::Project("p1".into()),
            TreeNode::Project("p2".into()),
        ]
    );
    assert_eq!(
        project_tree(&Snapshot::default(), &[]),
        vec![TreeNode::Empty]
    );
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
fn estimate_terminal_size_for_layout_accounts_for_sidebars_and_card_splits() {
    use crate::terminals::estimate_terminal_size_for_layout;

    // Unmeasured window returns None.
    assert_eq!(
        estimate_terminal_size_for_layout((0.0, 0.0), false, true, 260.0, 520.0, 13, None, false),
        None
    );

    // Wide Terminals with open 260pt sidebar and 8.0x18.0pt cells:
    // pane_w = 1280 - 56 - 260 = 964pt; pane_h = 800 - 42 - 32 = 726pt.
    let wide_open = estimate_terminal_size_for_layout(
        (1280.0, 800.0),
        false,
        true,
        260.0,
        520.0,
        13,
        Some((8000, 18000)),
        false,
    )
    .unwrap();
    // Wide Terminals with collapsed sidebar adds ~32 columns.
    let wide_collapsed = estimate_terminal_size_for_layout(
        (1280.0, 800.0),
        false,
        false,
        260.0,
        520.0,
        13,
        Some((8000, 18000)),
        false,
    )
    .unwrap();
    assert!(wide_collapsed.0 > wide_open.0);
    assert_eq!(wide_collapsed.1, wide_open.1);

    // Narrow Terminals hides sidebar when viewing terminal, matching collapsed width.
    let narrow_term = estimate_terminal_size_for_layout(
        (860.0, 800.0),
        true,
        true,
        260.0,
        520.0,
        13,
        Some((8000, 18000)),
        false,
    )
    .unwrap();
    let narrow_collapsed = estimate_terminal_size_for_layout(
        (860.0, 800.0),
        true,
        false,
        260.0,
        520.0,
        13,
        Some((8000, 18000)),
        false,
    )
    .unwrap();
    assert_eq!(narrow_term, narrow_collapsed);

    // Card side-by-side split subtracts left Card width (520pt) and Card terminal tab bar (38pt).
    let card_split = estimate_terminal_size_for_layout(
        (1280.0, 800.0),
        false,
        true,
        260.0,
        520.0,
        13,
        Some((8000, 18000)),
        true,
    )
    .unwrap();
    assert!(card_split.0 < wide_open.0);
    assert!(card_split.1 < wide_open.1);
}

#[test]
fn visible_project_tree_groups_by_host_and_shows_empty_remote_hosts_and_projects() {
    let mut s = Snapshot::default();
    s.projects.push(Project {
        id: "fiber".into(),
        name: "fiber".into(),
        directory: "/Users/jjy/fiber".into(),
        github: None,
        detected_github: None,
        github_disconnected: false,
        delivery_mode: Default::default(),
        agent_overrides: BTreeMap::new(),
        tags: Vec::new(),
        workflow: Workflow::default(),
        target_branch: None,
        created_at: 1,
    });
    s.register_host_entities("local");
    s.host_summaries.insert(
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
            project_count: 1,
            reconnect_count: 0,
            last_synced_at: None,
            pending_mutations: 0,
            sync_conflicts: 0,
        },
    );

    let mut devpc = Snapshot::default();
    devpc.projects.push(Project {
        id: "fiber".into(),
        name: "fiber".into(),
        directory: "/home/devpc/fiber".into(),
        github: None,
        detected_github: None,
        github_disconnected: false,
        delivery_mode: Default::default(),
        agent_overrides: BTreeMap::new(),
        tags: Vec::new(),
        workflow: Workflow::default(),
        target_branch: None,
        created_at: 2,
    });
    devpc.terminals.push(TerminalEntry {
        id: "devpc-misc".into(),
        conversation_id: None,
        number: 1,
        name: Some("devpc scratch".into()),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("c1".into()),
            terminal: Some("t1".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
            command: None,
        }),
        agent: None,
        created_at: 2,
    });
    devpc.terminal_contexts.push(TerminalContext {
        terminal_id: "devpc-misc".into(),
        cwd: Some("/tmp/devpc-other".into()),
        directory_root: Some("/tmp/devpc-other".into()),
        ..Default::default()
    });
    s.merge_remote_snapshot(
        devpc,
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
                rtt_ms: 10,
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
    // Third host with zero projects and zero terminals must still render a HostHeader.
    s.merge_remote_snapshot(
        Snapshot::default(),
        HostSummary {
            id: "gpu-box".into(),
            name: "GPU Box".into(),
            is_local: false,
            transport_label: "SSH".into(),
            endpoint_label: "gpu-box".into(),
            ssh_target: Some("gpu-box".into()),
            server_id: None,
            server_pubkey_fingerprint: None,
            connection: HostConnectionState::Connected {
                rtt_ms: 12,
                api_version: 1,
                version: "1".into(),
                platform: "linux".into(),
            },
            active_agents: 0,
            max_agents: 4,
            project_count: 0,
            reconnect_count: 0,
            last_synced_at: None,
            pending_mutations: 0,
            sync_conflicts: 0,
        },
    );

    let entries = crate::terminals::entries(&s);
    let nodes = crate::terminals::tree::visible_project_tree(
        &s,
        &entries,
        &std::collections::BTreeSet::new(),
    );
    let rem_fiber_id = canban_types::qualify_host_id("devpc", "fiber");

    assert!(
        nodes.contains(&crate::terminals::tree::TreeNode::HostHeader(
            "local".into()
        ))
    );
    assert!(
        nodes.contains(&crate::terminals::tree::TreeNode::HostHeader(
            "devpc".into()
        ))
    );
    assert!(
        nodes.contains(&crate::terminals::tree::TreeNode::HostHeader(
            "gpu-box".into()
        ))
    );
    assert!(nodes.contains(&crate::terminals::tree::TreeNode::Project("fiber".into())));
    assert!(nodes.contains(&crate::terminals::tree::TreeNode::Project(rem_fiber_id)));
    assert!(
        nodes.contains(&crate::terminals::tree::TreeNode::HostDirectoriesHeader(
            "devpc".into()
        ))
    );
    assert!(!nodes.contains(&crate::terminals::tree::TreeNode::OtherHeader));
}

#[test]
fn natural_terminal_exit_preserves_last_worktree_for_fallback_selection() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (_owner, (ui, after)) = nsh_core::element::create_root(|| {
        let mut s = fixture();
        s.terminals.clear();
        s.terminal_contexts.clear();
        s.projects[0].directory = "/repo/p1".into();
        s.worktrees.push(ManagedWorktree {
            id: "wt1".into(),
            project_id: "p1".into(),
            branch: "feat/a".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/repo/p1-wt1".into(),
            directory: "/repo/p1".into(),
            created_at: 1,
        });
        let mk_shell = |id: &str, num: u64, state: &str| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: num,
            name: Some(id.into()),
            origin_card_id: None,
            agent_engine: None,
            created_at: num,
            runtime: Some(TerminalRuntime {
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{id}")),
                state: state.into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            }),
            agent: None,
        };
        s.terminals.push(mk_shell("root-1", 1, "running"));
        s.terminals.push(mk_shell("wt-a", 2, "running"));
        s.terminals.push(mk_shell("wt-b", 3, "running"));
        s.terminal_contexts.extend([
            TerminalContext {
                terminal_id: "root-1".into(),
                core_id: "core".into(),
                terminal: "pty-root-1".into(),
                cwd: Some("/repo/p1".into()),
                directory_root: Some("/repo/p1".into()),
                project_id: Some("p1".into()),
                ..Default::default()
            },
            TerminalContext {
                terminal_id: "wt-a".into(),
                core_id: "core".into(),
                terminal: "pty-wt-a".into(),
                cwd: Some("/repo/p1-wt1".into()),
                directory_root: Some("/repo/p1-wt1".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt1".into()),
                ..Default::default()
            },
            TerminalContext {
                terminal_id: "wt-b".into(),
                core_id: "core".into(),
                terminal: "pty-wt-b".into(),
                cwd: Some("/repo/p1-wt1".into()),
                directory_root: Some("/repo/p1-wt1".into()),
                project_id: Some("p1".into()),
                worktree_id: Some("wt1".into()),
                ..Default::default()
            },
        ]);

        let ui = crate::presenter::Ui::new();
        ui.snapshot.set(s.clone());
        ui.snapshot_ready.set(true);
        ui.page.set("terminals".into());
        ui.open_terminal("wt-b".into());
        crate::terminals::watch(ui);

        // Simulate `wt-b` exiting naturally: AppServer marks it finished AND drops its TerminalContext.
        let mut after = s;
        after
            .terminals
            .iter_mut()
            .find(|t| t.id == "wt-b")
            .unwrap()
            .runtime
            .as_mut()
            .unwrap()
            .state = "finished".into();
        after.terminal_contexts.retain(|c| c.terminal_id != "wt-b");
        (ui, after)
    });

    ui.snapshot.set(after);
    assert_eq!(
        ui.terminal_selection.get_untracked(),
        "wt-a",
        "exiting wt-b must switch to sibling wt-a in the same worktree, not jump to root-1"
    );
}

#[test]
fn terminal_actions_menu_omits_long_hint_header_and_keeps_actions() {
    use crate::terminals::tabs::{TerminalMenuEntry, terminal_menu_entries};

    let mut s = fixture();
    let long_title = "非常长的任务标题 / a very long task title with important details ".repeat(6);
    s.cards[0].title = long_title.clone();
    let agent_id = s.terminals[0].id.clone();
    s.terminal_contexts.push(TerminalContext {
        terminal_id: agent_id.clone(),
        core_id: "core".into(),
        terminal: "t-c1".into(),
        cwd: Some("/repo/p1-wt".into()),
        directory_root: Some("/repo/p1-wt".into()),
        project_id: Some("p1".into()),
        card_id: Some("c1".into()),
        branch: Some("canban/1".into()),
        dirty: true,
        ahead: 2,
        behind: 0,
        ..Default::default()
    });
    s.terminals.push(TerminalEntry {
        id: "shell-live".into(),
        conversation_id: None,
        number: 10,
        name: None,
        origin_card_id: None,
        agent_engine: None,
        created_at: 2,
        runtime: Some(TerminalRuntime {
            core_id: Some("core".into()),
            terminal: Some("pty-shell-live".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
            command: None,
        }),
        agent: None,
    });
    s.terminal_contexts.push(TerminalContext {
        terminal_id: "shell-live".into(),
        core_id: "core".into(),
        terminal: "pty-shell-live".into(),
        cwd: Some("/tmp/shell-live".into()),
        directory_root: Some("/tmp/shell-live".into()),
        project_id: Some("p1".into()),
        ..Default::default()
    });
    s.terminals.push(TerminalEntry {
        id: "shell-ended".into(),
        conversation_id: None,
        number: 11,
        name: None,
        origin_card_id: None,
        agent_engine: None,
        created_at: 3,
        runtime: Some(TerminalRuntime {
            core_id: Some("core".into()),
            terminal: None,
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
    });

    let syncing = std::collections::BTreeSet::new();
    let all = crate::terminals::entries(&s);
    let agent_entry = all.iter().find(|e| e.saved.id == agent_id).unwrap();
    let agent_hint = agent_entry.hint();
    let agent_menu = terminal_menu_entries(&s, agent_entry, &syncing);
    assert_eq!(agent_menu.first(), Some(&TerminalMenuEntry::RenameTab));
    for item in &agent_menu {
        if let TerminalMenuEntry::DisabledInfo(text) = item {
            assert_ne!(text, &agent_hint);
            assert!(!text.contains(&long_title));
            assert!(!text.contains("Managed by Canban"));
        }
    }
    assert_eq!(
        agent_menu,
        vec![
            TerminalMenuEntry::RenameTab,
            TerminalMenuEntry::CloseTerminal,
            TerminalMenuEntry::OpenCard {
                card_id: "c1".into(),
            },
            TerminalMenuEntry::DisabledInfo("/repo/p1-wt".into()),
            TerminalMenuEntry::DisabledInfo("Branch: canban/1 (changed · ↑2)".into()),
            TerminalMenuEntry::SyncGitBranch {
                target: crate::terminals::worktrees::GitSyncTarget {
                    host_id: "local".into(),
                    project_id: "p1".into(),
                    card_id: Some("c1".into()),
                    cwd: "/repo/p1-wt".into(),
                    branch: "canban/1".into(),
                    scope_key: "scope:card:c1".into(),
                },
                disabled: false,
            },
            TerminalMenuEntry::InterruptAgent,
            TerminalMenuEntry::ContextAndHistory,
        ]
    );

    let live_shell = all.iter().find(|e| e.saved.id == "shell-live").unwrap();
    let live_menu = terminal_menu_entries(&s, live_shell, &syncing);
    assert_eq!(live_menu.first(), Some(&TerminalMenuEntry::RenameTab));
    assert_eq!(
        live_menu,
        vec![
            TerminalMenuEntry::RenameTab,
            TerminalMenuEntry::CloseTerminal,
            TerminalMenuEntry::DisabledInfo("/tmp/shell-live".into()),
            TerminalMenuEntry::InterruptTerminal,
        ]
    );

    let ended_shell = all.iter().find(|e| e.saved.id == "shell-ended").unwrap();
    let ended_menu = terminal_menu_entries(&s, ended_shell, &syncing);
    assert_eq!(ended_menu.first(), Some(&TerminalMenuEntry::RenameTab));
    assert_eq!(
        ended_menu,
        vec![
            TerminalMenuEntry::RenameTab,
            TerminalMenuEntry::CloseTerminal,
            TerminalMenuEntry::DisabledInfo("Directory unavailable".into()),
            TerminalMenuEntry::ReopenTerminal,
        ]
    );
}

#[test]
fn git_sync_target_hints_and_summary_cover_registered_checkouts_and_states() {
    use crate::terminals::worktrees;
    let mut s = fixture();
    s.projects[0].directory = "/repo/p1".into();
    s.cards[0].worktree = Some("/repo/p1-wt/c1".into());
    s.worktrees.push(ManagedWorktree {
        id: "wt1".into(),
        project_id: "p1".into(),
        branch: "feat/wt".into(),
        base_branch: "main".into(),
        base_commit: None,
        path: "/repo/p1-wt/wt1".into(),
        directory: "/repo/p1".into(),
        created_at: 1,
    });

    let mk_shell = |id: &str, num: u64, _proj: Option<&str>| TerminalEntry {
        id: id.into(),
        conversation_id: None,
        number: num,
        name: Some(id.into()),
        origin_card_id: None,
        agent_engine: None,
        created_at: num,
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
            command: None,
        }),
        agent: None,
    };
    s.terminals.push(mk_shell("root-shell", 10, Some("p1")));
    s.terminals.push(mk_shell("wt-shell", 11, Some("p1")));
    s.terminals.push(mk_shell("ext-shell", 12, Some("p1")));
    s.terminals.push(mk_shell("detached-shell", 13, Some("p1")));
    s.terminals.push(mk_shell("outside-shell", 14, None));

    let agent_id = s.terminals[0].id.clone();
    s.terminal_contexts.extend([
        TerminalContext {
            terminal_id: agent_id.clone(),
            core_id: "core".into(),
            terminal: "t-c1".into(),
            cwd: Some("/repo/p1-wt/c1/sub".into()),
            directory_root: Some("/repo/p1-wt/c1".into()),
            project_id: Some("p1".into()),
            card_id: Some("c1".into()),
            branch: Some("canban/1".into()),
            ..Default::default()
        },
        TerminalContext {
            terminal_id: "root-shell".into(),
            core_id: "core".into(),
            terminal: "pty-root-shell".into(),
            cwd: Some("/repo/p1".into()),
            directory_root: Some("/repo/p1".into()),
            project_id: Some("p1".into()),
            branch: Some("main".into()),
            ..Default::default()
        },
        TerminalContext {
            terminal_id: "wt-shell".into(),
            core_id: "core".into(),
            terminal: "pty-wt-shell".into(),
            cwd: Some("/repo/p1-wt/wt1".into()),
            directory_root: Some("/repo/p1-wt/wt1".into()),
            project_id: Some("p1".into()),
            worktree_id: Some("wt1".into()),
            branch: Some("feat/wt".into()),
            ..Default::default()
        },
        TerminalContext {
            terminal_id: "ext-shell".into(),
            core_id: "core".into(),
            terminal: "pty-ext-shell".into(),
            cwd: Some("/repo/p1/.worktrees/ext".into()),
            directory_root: Some("/repo/p1/.worktrees/ext".into()),
            project_id: Some("p1".into()),
            branch: Some("fix/ext".into()),
            ..Default::default()
        },
        TerminalContext {
            terminal_id: "detached-shell".into(),
            core_id: "core".into(),
            terminal: "pty-detached-shell".into(),
            cwd: Some("/repo/p1".into()),
            directory_root: Some("/repo/p1".into()),
            project_id: Some("p1".into()),
            branch: None,
            ..Default::default()
        },
        TerminalContext {
            terminal_id: "outside-shell".into(),
            core_id: "core".into(),
            terminal: "pty-outside-shell".into(),
            cwd: Some("/tmp/unrelated".into()),
            directory_root: Some("/tmp/unrelated".into()),
            project_id: None,
            branch: Some("other".into()),
            ..Default::default()
        },
    ]);

    let all_entries = crate::terminals::entries(&s);
    let find = |id: &str| {
        all_entries
            .iter()
            .find(|e| e.saved.id == id)
            .cloned()
            .unwrap()
    };

    let card_tgt = worktrees::git_sync_target(&s, &find(&agent_id)).unwrap();
    assert_eq!(card_tgt.project_id, "p1");
    assert_eq!(card_tgt.card_id.as_deref(), Some("c1"));
    assert_eq!(card_tgt.cwd, "/repo/p1-wt/c1");
    assert_eq!(card_tgt.branch, "canban/1");

    let root_tgt = worktrees::git_sync_target(&s, &find("root-shell")).unwrap();
    assert_eq!(root_tgt.cwd, "/repo/p1");
    assert_eq!(root_tgt.card_id, None);

    let wt_tgt = worktrees::git_sync_target(&s, &find("wt-shell")).unwrap();
    assert_eq!(wt_tgt.cwd, "/repo/p1-wt/wt1");
    assert_eq!(wt_tgt.branch, "feat/wt");

    let ext_tgt = worktrees::git_sync_target(&s, &find("ext-shell")).unwrap();
    assert_eq!(ext_tgt.cwd, "/repo/p1/.worktrees/ext");
    assert_eq!(ext_tgt.branch, "fix/ext");

    assert!(worktrees::git_sync_target(&s, &find("detached-shell")).is_none());
    assert!(worktrees::git_sync_target(&s, &find("outside-shell")).is_none());

    assert_eq!(
        worktrees::sync_git_button_hint("main", 2, 1, false, false),
        "Sync Git branch main (pull ↓1 & push ↑2)"
    );
    assert_eq!(
        worktrees::sync_git_button_hint("main", 0, 3, false, false),
        "Sync Git branch main (pull ↓3 & push)"
    );
    assert_eq!(
        worktrees::sync_git_button_hint("main", 4, 0, false, false),
        "Sync Git branch main (pull & push ↑4)"
    );
    assert_eq!(
        worktrees::sync_git_button_hint("main", 0, 0, true, false),
        "Syncing Git branch main (pull & push)…"
    );
    assert_eq!(
        worktrees::sync_git_button_hint("main", 0, 0, false, true),
        "Sync Git branch main unavailable: host is offline"
    );

    assert_eq!(
        worktrees::format_git_sync_summary(
            &json!({"branch":"feat/x","remote":"origin","remote_branch":"feat/x","pulled":false,"fast_forwarded":false,"pushed":true,"pushed_commits":1,"set_upstream":true}),
            "feat/x"
        ),
        "Published feat/x to origin/feat/x"
    );
    assert_eq!(
        worktrees::format_git_sync_summary(
            &json!({"branch":"main","remote":"origin","remote_branch":"main","pulled":true,"fast_forwarded":true,"pulled_commits":2,"pushed":true,"pushed_commits":1,"set_upstream":false}),
            "main"
        ),
        "Synced main: pulled 2 commits, pushed 1 commit"
    );
    assert_eq!(
        worktrees::format_git_sync_summary(
            &json!({"branch":"main","remote":"origin","remote_branch":"main","pulled":true,"fast_forwarded":false,"pulled_commits":0,"pushed":false,"pushed_commits":0,"set_upstream":false}),
            "main"
        ),
        "main is already up to date"
    );
}

#[test]
fn plain_shell_command_status_uses_unified_marks_and_agent_layer_overrides() {
    use canban_types::{HerdrStatus, ShellCommand};
    use canban_ui_kit::tokens::{DANGER, MUTED, SUCCESS};

    let mut s = fixture();
    let push_shell = |s: &mut Snapshot,
                      id: &str,
                      num: u64,
                      cmd: Option<ShellCommand>,
                      observed_agent: Option<&str>,
                      agent_status: Option<&str>| {
        s.terminals.push(TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: num,
            name: Some(format!("Terminal {num}")),
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                core_id: Some("core".into()),
                terminal: Some(format!("pty-{id}")),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: agent_status.map(str::to_owned),
                agent_turn: None,
                observed_agent: observed_agent.map(str::to_owned),
                waiting: None,
                command: cmd,
            }),
            agent: None,
            created_at: num,
        });
    };

    push_shell(&mut s, "sh-idle", 10, None, None, None);
    push_shell(
        &mut s,
        "sh-run",
        11,
        Some(ShellCommand {
            label: "cargo".into(),
            exit_code: None,
        }),
        None,
        None,
    );
    push_shell(
        &mut s,
        "sh-ok",
        12,
        Some(ShellCommand {
            label: "git".into(),
            exit_code: Some(0),
        }),
        None,
        None,
    );
    push_shell(
        &mut s,
        "sh-fail",
        13,
        Some(ShellCommand {
            label: "cargo".into(),
            exit_code: Some(1),
        }),
        None,
        None,
    );
    push_shell(
        &mut s,
        "sh-int",
        14,
        Some(ShellCommand {
            label: "sleep".into(),
            exit_code: Some(130),
        }),
        None,
        None,
    );
    push_shell(
        &mut s,
        "sh-agent",
        15,
        Some(ShellCommand {
            label: "agy".into(),
            exit_code: None,
        }),
        Some("agy"),
        Some("working"),
    );

    let entries = crate::terminals::entries(&s);
    let find = |id: &str| entries.iter().find(|e| e.saved.id == id).unwrap();

    let e_idle = find("sh-idle");
    assert_eq!(e_idle.herdr_status(), HerdrStatus::PlainShell);
    assert_eq!(crate::terminals::labels::status_mark_kind(e_idle), None);
    assert_eq!(crate::terminals::labels::sidebar_status_spec(e_idle), None);
    assert!(!crate::terminals::labels::has_status_mark(e_idle));
    assert_eq!(e_idle.display_status(), "Open");

    let e_run = find("sh-run");
    assert_eq!(
        crate::terminals::labels::status_mark_kind(e_run),
        Some(crate::terminals::labels::StatusMarkKind::CommandRunning)
    );
    assert_eq!(
        crate::terminals::labels::sidebar_status_spec(e_run),
        Some(("cargo".into(), MUTED))
    );
    assert!(crate::terminals::labels::has_status_mark(e_run));
    assert_eq!(e_run.close_confirm_reason(&s), None);
    assert_eq!(e_run.display_status(), "Running cargo");

    let e_ok = find("sh-ok");
    assert_eq!(
        crate::terminals::labels::status_mark_kind(e_ok),
        Some(crate::terminals::labels::StatusMarkKind::DoneCheck)
    );
    assert_eq!(
        crate::terminals::labels::sidebar_status_spec(e_ok),
        Some(("".into(), SUCCESS))
    );
    assert!(crate::terminals::labels::has_status_mark(e_ok));
    assert_eq!(
        e_ok.display_status(),
        "Last command succeeded (exit 0) — git"
    );

    let e_fail = find("sh-fail");
    assert_eq!(
        crate::terminals::labels::status_mark_kind(e_fail),
        Some(crate::terminals::labels::StatusMarkKind::ErrorTriangle)
    );
    assert_eq!(
        crate::terminals::labels::sidebar_status_spec(e_fail),
        Some(("exit 1".into(), DANGER))
    );
    assert_eq!(
        e_fail.display_status(),
        "Last command failed (exit 1) — cargo"
    );

    let e_int = find("sh-int");
    assert_eq!(
        crate::terminals::labels::status_mark_kind(e_int),
        Some(crate::terminals::labels::StatusMarkKind::ErrorTriangle)
    );
    assert_eq!(
        crate::terminals::labels::sidebar_status_spec(e_int),
        Some(("exit 130".into(), DANGER))
    );
    assert_eq!(
        e_int.display_status(),
        "Last command failed (exit 130) — sleep"
    );

    let e_agent = find("sh-agent");
    assert_eq!(
        crate::terminals::labels::status_mark_kind(e_agent),
        Some(crate::terminals::labels::StatusMarkKind::LiveWorking)
    );
}
