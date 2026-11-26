use super::*;
pub(super) fn core(app: &AppServer, dir: &Path) {
    let script = dir.join("test-hc");
    std::fs::write(&script,r#"#!/usr/bin/python3
import sys,json,os
base=os.path.dirname(__file__)
for line in sys.stdin:
 p=json.loads(line);m=p['method'];args=p.get('params',{});r={}
 if m=='hello':r={'generation':1}
 elif m=='server.status':r={'core':{'id':'replacement' if os.path.exists(base+'/replace') else 'original'}}
 elif m=='terminal.run':
  with open(base+'/launches','a') as f:f.write(json.dumps(args)+'\n')
  r={'address':'t1'}
 elif m in ['terminal.get','terminal.list']:
  launches=[json.loads(l) for l in open(base+'/launches')] if os.path.exists(base+'/launches') else []
  cwd=open(base+'/cwd').read() if os.path.exists(base+'/cwd') else (launches[-1]['cwd'] if launches else base)
  t={'address':'t1','cwd':launches[-1]['cwd'] if launches else base,'foreground':{'cwd':cwd},'state':'finished' if os.path.exists(base+'/finished') else 'live'}
  r={'terminals':[t]} if m=='terminal.list' else t
 elif m in ['terminal.kill','terminal.interrupt']:
  with open(base+'/controls','a') as f:f.write(m+'\n')
  if m=='terminal.kill':open(base+'/finished','w').close()
  r={'sent':True}
 print(json.dumps({'jsonrpc':'2.0','id':p['id'],'result':r}),flush=True)
"#).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.config.write().unwrap().hc = script.to_string_lossy().into();
}
#[test]
fn terminal_creation_context_and_default_rename_do_not_read_unrelated_history() {
    let (dir, app, project, _) = setup();
    core(&app, dir.path());
    // An unreadable unrelated record makes full-snapshot reads fail. Terminal
    // creation, inherited directory resolution and naming need none of it.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.conn.execute(
                "INSERT INTO notes(id,state,number,data) VALUES('unrelated-history','',0,'{}')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(app.snapshot().is_err());
    let terminal = app
        .call("terminal.create", json!({"project_id":project.id}))
        .unwrap();
    assert_eq!(terminal["name"], "Terminal 1");
    let context = app
        .call("terminal.context", json!({"terminal_id":terminal["id"]}))
        .unwrap();
    assert_eq!(context["project_id"], project.id);
    let second = app
        .call(
            "terminal.create",
            json!({"inherit_terminal_id":terminal["id"]}),
        )
        .unwrap();
    assert_eq!(second["name"], "Terminal 2");
    let renamed = app
        .call(
            "terminal.rename",
            json!({"terminal_id":second["id"],"name":""}),
        )
        .unwrap();
    assert_eq!(renamed["name"], "Terminal 2");
}

#[test]
fn manual_observation_and_controls_fence_core_identity_without_advancing_cards() {
    let (dir, app, p, c) = setup();
    core(&app, dir.path());
    let item = app
        .call("terminal.create", json!({"project_id":p.id}))
        .unwrap();
    let tid = item["id"].as_str().unwrap();
    let args = json!({"terminal_id":tid,"request_key":"interrupt-once"});
    app.call("terminal.interrupt", args.clone()).unwrap();
    app.call("terminal.interrupt", args).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("controls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    std::fs::write(dir.path().join("replace"), "").unwrap();
    app.observe_terminals().unwrap();
    assert!(
        matches!(&app.snapshot().unwrap().terminals[0].runtime,Some(TerminalRuntime {connection,..}) if connection=="replaced")
    );
    assert!(
        app.call("terminal.end", json!({"terminal_id":tid}))
            .is_err()
    );
    std::fs::remove_file(dir.path().join("replace")).unwrap();
    std::fs::write(dir.path().join("finished"), "").unwrap();
    app.observe_terminals().unwrap();
    assert!(
        matches!(&app.snapshot().unwrap().terminals[0].runtime,Some(TerminalRuntime {state,..}) if state=="finished")
    );
    assert_eq!(app.snapshot().unwrap().cards[0].revision, c.revision);
}
#[test]
fn upgrading_conversations_allocates_stable_numbers_without_runtime_changes() {
    fn reopen(home: PathBuf) -> Arc<AppServer> {
        // Restart follows ownership release, rather than assuming drop makes
        // the OS lock immediately available amid concurrent child launches.
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match AppServer::open(home.clone(), PathBuf::from("/bin/echo")) {
                Ok(app) => return app,
                Err(err) if std::time::Instant::now() < until => {
                    let _ = err;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("Old service still owns home: {err:#}"),
            }
        }
    }
    let (dir, app, _p, c) = setup();
    app.call("agent.open", json!({"card_id":c.id,"profile_id":"codex"}))
        .unwrap();
    let before = app.snapshot().unwrap();
    app.store.lock().unwrap().db().conn.execute_batch("DROP TABLE terminals; DELETE FROM counters WHERE id='terminal-number'; DROP TABLE card_reads; DROP TABLE github_issues; DROP INDEX github_card_source; DROP TABLE editor_drafts; PRAGMA user_version=3;").unwrap();
    drop(app);
    let upgraded = reopen(dir.path().join("home"));
    let s = upgraded.snapshot().unwrap();
    assert_eq!(s.conversations, before.conversations);
    assert_eq!(s.terminals[0].id, s.conversations[0].id);
    assert_eq!(s.terminals[0].title(), "1");
    drop(upgraded);
    let reopened = reopen(dir.path().join("home"));
    assert_eq!(reopened.snapshot().unwrap().terminals, s.terminals);
}

#[test]
fn independent_terminals_inherit_observed_cwd_and_only_derive_project_context() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    core(&app, dir.path());
    let first = app.call("terminal.create", json!({})).unwrap();
    assert!(app.snapshot().unwrap().projects.is_empty());
    assert!(first.get("source").is_none());
    assert!(first.get("project_id").is_none());
    assert!(first["conversation_id"].is_null());
    assert!(first["runtime"].get("cwd").is_none());
    let p = app
        .call(
            "project.create",
            json!({"name":"Observed","directory":dir.path().join("project with spaces")}),
        )
        .unwrap();
    std::fs::write(dir.path().join("cwd"), p["directory"].as_str().unwrap()).unwrap();
    app.observe_terminals().unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(
        s.terminal_contexts[0].project_id.as_deref(),
        p["id"].as_str()
    );
    let second = app
        .call(
            "terminal.create",
            json!({"inherit_terminal_id":first["id"]}),
        )
        .unwrap();
    assert_ne!(first["id"], second["id"]);
    let lines = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let last: Value = serde_json::from_str(lines.lines().last().unwrap()).unwrap();
    assert_eq!(last["cwd"], p["directory"]);
    // Reading a current foreground cwd must take precedence over launch cwd.
    std::fs::write(dir.path().join("cwd"), dir.path().to_str().unwrap()).unwrap();
    app.observe_terminals().unwrap();
    assert!(
        app.snapshot()
            .unwrap()
            .terminal_contexts
            .iter()
            .all(|c| c.project_id.is_none())
    );
    app.config.write().unwrap().hc = "/usr/bin/false".into();
    app.observe_terminals().unwrap();
    // A disconnected observer retains last-known same-binding placement, while
    // an explicit current-context request still reports unavailability.
    assert!(
        app.snapshot()
            .unwrap()
            .terminal_contexts
            .iter()
            .all(|c| c.cwd.as_deref() == dir.path().to_str())
    );
    let context = app
        .call("terminal.context", json!({"terminal_id":first["id"]}))
        .unwrap();
    assert!(context["cwd"].is_null());
    assert!(context["project_id"].is_null());
    assert!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .all(|t| t.agent.is_none())
    );
}

#[test]
fn agent_directory_context_cannot_reassign_its_card_or_follow_a_stale_binding() {
    let (dir, app, p, c) = setup();
    core(&app, dir.path());
    let other = app
        .call(
            "project.create",
            json!({"name":"Other","directory":dir.path().join("other")}),
        )
        .unwrap();
    std::fs::write(dir.path().join("cwd"), other["directory"].as_str().unwrap()).unwrap();
    app.call("agent.open", json!({"card_id":c.id,"profile_id":"codex"}))
        .unwrap();
    let mut entry = admit(&app, &app.snapshot().unwrap().terminals[0]);
    entry.agent.as_mut().unwrap().core_id = Some("original".into());
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&entry))
        .unwrap();
    app.observe_terminals().unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(
        s.terminal_contexts[0].project_id.as_deref(),
        other["id"].as_str()
    );
    assert_eq!(s.conversations[0].project_id, p.id);
    assert_eq!(s.conversations[0].card_id, c.id);
    entry.agent.as_mut().unwrap().terminal = Some("new-pty".into());
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&entry))
        .unwrap();
    assert!(app.snapshot().unwrap().terminal_contexts.is_empty());
}

#[test]
fn v4_terminal_migration_keeps_identity_and_runtime_but_drops_shell_ownership() {
    let (dir, app, p, c) = setup();
    app.call("agent.open", json!({"card_id":c.id,"profile_id":"codex"}))
        .unwrap();
    let agent = app.snapshot().unwrap().terminals[0].clone();
    let old_agent = json!({"id":agent.id,"project_id":p.id,"card_id":c.id,"number":agent.number,"name":"Agent name","source":{"kind":"agent"},"created_at":1});
    let old_shell = json!({"id":"legacy-shell","project_id":p.id,"card_id":c.id,"number":9,"name":"Server","source":{"kind":"shell","cwd":"/old/path","core_id":"original","terminal":"t9","state":"running","connection":"connected","error":null},"created_at":2});
    let receipt_args = json!({"request_key":"legacy-receipt"});
    let fingerprint = canban_context::digest(
        &serde_json::to_vec(&json!(["terminal.create", receipt_args])).unwrap(),
    );
    let store = app.store.lock().unwrap();
    store
        .db()
        .conn
        .execute(
            "INSERT INTO operations VALUES(?1,?2,?3)",
            params!["legacy-receipt", fingerprint, old_shell.to_string()],
        )
        .unwrap();
    store
        .db()
        .conn
        .execute(
            "UPDATE terminals SET data=?2 WHERE id=?1",
            params![agent.id, old_agent.to_string()],
        )
        .unwrap();
    store
        .db()
        .conn
        .execute(
            "INSERT INTO terminals VALUES(?1,?2,?3,'',9,?4)",
            params!["legacy-shell", p.id, c.id, old_shell.to_string()],
        )
        .unwrap();
    store
        .db()
        .conn
        .execute_batch(
            "DROP TABLE card_reads; DROP TABLE github_issues; DROP INDEX github_card_source; DROP TABLE editor_drafts; PRAGMA user_version=4;",
        )
        .unwrap();
    drop(store);
    drop(app);
    let upgraded = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let s = upgraded.snapshot().unwrap();
    let receipt = upgraded.call("terminal.create", receipt_args).unwrap();
    assert_eq!(receipt["runtime"]["terminal"], "t9");
    assert!(receipt.get("source").is_none());
    let unchanged: String = upgraded
        .store
        .lock()
        .unwrap()
        .db()
        .conn
        .query_row(
            "SELECT result FROM operations WHERE id='legacy-receipt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, old_shell.to_string());
    let shell = s.terminals.iter().find(|t| t.id == "legacy-shell").unwrap();
    assert_eq!(shell.title(), "Server");
    assert_eq!(shell.number, 9);
    assert!(shell.conversation_id.is_none());
    assert_eq!(
        shell.runtime.as_ref().unwrap().terminal.as_deref(),
        Some("t9")
    );
    let agent = s.terminals.iter().find(|t| t.id == agent.id).unwrap();
    assert_eq!(agent.conversation_id.as_deref(), Some(agent.id.as_str()));
    assert!(agent.runtime.is_none());
    let data: String = upgraded
        .store
        .lock()
        .unwrap()
        .db()
        .conn
        .query_row(
            "SELECT data FROM terminals WHERE id='legacy-shell'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!data.contains("/old/path"));
    assert!(!data.contains("project_id"));
    drop(upgraded);
    let reopened = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(reopened.snapshot().unwrap().terminals, s.terminals);
}

#[test]
fn archive_protects_a_worktree_entered_from_an_unrelated_independent_terminal() {
    let (dir, app, _p, mut c) = setup();
    core(&app, dir.path());
    let worktree = dir.path().join("worktree");
    std::fs::create_dir(&worktree).unwrap();
    c.worktree = Some(worktree.to_str().unwrap().into());
    app.call("terminal.create", json!({"cwd":dir.path()}))
        .unwrap();
    assert!(!app.terminal_retains_worktree(&c).unwrap());
    std::fs::write(dir.path().join("cwd"), worktree.to_str().unwrap()).unwrap();
    assert!(app.terminal_retains_worktree(&c).unwrap());
    app.config.write().unwrap().hc = "/usr/bin/false".into();
    assert!(app.terminal_retains_worktree(&c).unwrap());
}

#[test]
fn manual_labels_derive_max_plus_one_from_live_project_terminals_without_durable_counter() {
    let (dir, app, p, c) = setup();
    core(&app, dir.path());
    let source_p2 = dir.path().join("source-p2");
    std::fs::create_dir_all(&source_p2).unwrap();
    let p2: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Second Project", "directory": source_p2}),
        )
        .unwrap(),
    )
    .unwrap();

    // Card-role Agents do not advance independent Terminal labels.
    app.call(
        "agent.open",
        json!({"card_id": c.id, "profile_id": "codex"}),
    )
    .unwrap();
    let first = app
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    app.call(
        "agent.open",
        json!({"card_id": c.id, "profile_id": "reviewer"}),
    )
    .unwrap();
    let second = app
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(first["name"], "Terminal 1");
    assert_eq!(second["name"], "Terminal 2");
    assert_eq!(second["number"], 4);

    // Independent Project scope: p2 starts fresh at Terminal 1!
    let p2_first = app
        .call("terminal.create", json!({"project_id": p2.id}))
        .unwrap();
    assert_eq!(p2_first["name"], "Terminal 1");

    // Closing Terminal 2 in p leaves Terminal 1 live -> next in p is Terminal 2 again.
    app.call("terminal.close", json!({"terminal_id": second["id"]}))
        .unwrap();
    // Reset test-hc finished marker so new launches stay live.
    let _ = std::fs::remove_file(dir.path().join("finished"));
    let second_reborn = app
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(second_reborn["name"], "Terminal 2");

    // Closing Terminal 1 while Terminal 2 stays live -> max is 2, so next is Terminal 3;
    // then closing both resets p back to Terminal 1!
    app.call("terminal.close", json!({"terminal_id": first["id"]}))
        .unwrap();
    let _ = std::fs::remove_file(dir.path().join("finished"));
    let third = app
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(third["name"], "Terminal 3");

    app.call(
        "terminal.close",
        json!({"terminal_id": second_reborn["id"]}),
    )
    .unwrap();
    app.call("terminal.close", json!({"terminal_id": third["id"]}))
        .unwrap();
    let _ = std::fs::remove_file(dir.path().join("finished"));
    let reset = app
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(reset["name"], "Terminal 1");

    // No durable manual-number counter exists in SQLite.
    let count: i64 = app
        .store
        .lock()
        .unwrap()
        .db()
        .conn
        .query_row(
            "SELECT count(*) FROM counters WHERE id='terminal-manual-number'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn first_terminal_accepts_empty_inheritance_without_a_project_or_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    core(&app, dir.path());
    assert!(app.snapshot().unwrap().terminals.is_empty());
    let first = app
        .call("terminal.create", json!({"inherit_terminal_id":""}))
        .unwrap();
    assert_eq!(first["runtime"]["state"], "running");
    assert_eq!(first["name"], "Terminal 1");
    assert!(first["runtime"]["terminal"].is_string());
    let s = app.snapshot().unwrap();
    assert!(s.projects.is_empty() && s.conversations.is_empty());
    assert!(s.terminals.iter().all(|t| t.agent.is_none()));
    assert_eq!(s.terminals.len(), 1);
}
#[test]
fn ephemeral_agent_terminal_launches_without_prompt_or_resume_and_records_origin_card() {
    let (dir, app, p, c) = setup();
    core(&app, dir.path());
    {
        let mut cfg = app.config();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            model: Some("gemini-4-carbon-high".into()),
            ..Default::default()
        });
        let resolved = canban_profiles::resolve(cfg, &app.home).unwrap();
        *app.config.write().unwrap() = resolved;
    }
    app.call(
        "project.agents",
        json!({"project_id": p.id, "agent_overrides": {"developer": "agy"}}),
    )
    .unwrap();

    let created = app
        .call(
            "terminal.create",
            json!({"project_id": p.id, "card_id": c.id, "agent": true}),
        )
        .unwrap();
    let entry: TerminalEntry = serde_json::from_value(created).unwrap();
    assert!(entry.id.starts_with("agent-"));
    assert_eq!(entry.origin_card_id.as_deref(), Some(c.id.as_str()));
    assert_eq!(entry.agent_engine.as_deref(), Some("agy"));
    assert_eq!(entry.title(), "Antigravity 1");
    assert!(entry.conversation_id.is_none());

    let launches = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let launch: Value = serde_json::from_str(launches.lines().last().unwrap()).unwrap();
    assert_eq!(launch["cwd"], p.directory);
    assert_eq!(launch["env"]["CANBAN_CARD"], c.number.to_string());
    assert_eq!(launch["env"]["CARD_ID"], c.id);
    assert_eq!(launch["env"]["CANBAN_PROJECT"], p.id);
    let argv: Vec<String> = serde_json::from_value(launch["argv"].clone()).unwrap();
    assert!(argv[0].ends_with("agy"));
    assert!(!argv.iter().any(|a| a == "-i"
        || a == "--conversation"
        || a.contains("implement")
        || a.contains("Card")));
    assert!(
        argv.windows(2)
            .any(|w| w == ["--model", "gemini-4-carbon-high"])
    );

    let s = app.snapshot().unwrap();
    assert!(s.terminals.iter().all(|t| t.agent.is_none()));
    assert!(s.conversations.is_empty());
}

#[test]
fn opening_card_shell_and_agent_terminals_immediately_creates_card_worktree_and_injects_card_env() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("README.md"), "# Seed\n").unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&src, &["init", "-b", "main"]);
    git(&src, &["config", "user.name", "Tester"]);
    git(&src, &["config", "user.email", "tester@example.invalid"]);
    git(&src, &["config", "commit.gpgsign", "false"]);
    git(&src, &["add", "."]);
    git(&src, &["commit", "-m", "seed"]);

    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    core(&app, dir.path());
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Card Worktree Terminals", "directory": src}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Open terminal in Card worktree", "body": "Notes"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(c.worktree.is_none());

    // 1. Opening a Shell terminal from the Card immediately prepares the Card's dedicated git worktree.
    let shell_entry: TerminalEntry = serde_json::from_value(
        app.call(
            "terminal.create",
            json!({"project_id": p.id, "card_id": c.id, "request_key": "card-shell-wt"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let updated_card = snap.cards.iter().find(|x| x.id == c.id).unwrap();
    let wt_path = updated_card
        .worktree
        .clone()
        .expect("Card worktree must be created when opening a Card terminal");
    assert_ne!(Path::new(&wt_path), src.as_path());
    assert!(Path::new(&wt_path).is_dir());
    assert!(updated_card.branch.is_some());
    assert_eq!(
        app.events()
            .unwrap()
            .iter()
            .filter(|e| e.card_id == c.id && e.kind == "workspace.prepared")
            .count(),
        1
    );

    let launches = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let shell_launch: Value = serde_json::from_str(launches.lines().next().unwrap()).unwrap();
    assert_eq!(shell_launch["cwd"], wt_path);
    assert_eq!(shell_launch["labels"]["canban.terminal"], shell_entry.id);
    assert_eq!(shell_launch["env"]["CANBAN_CARD"], c.number.to_string());
    assert_eq!(shell_launch["env"]["CARD_ID"], c.id);
    assert_eq!(shell_launch["env"]["CANBAN_PROJECT"], p.id);
    assert!(shell_launch["env"]["CANBAN_CLI"].as_str().is_some());
    assert!(shell_launch["env"]["CANBAN_ENDPOINT"].as_str().is_some());
    assert!(shell_launch["env"]["CANBAN_RUN_DIR"].as_str().is_some());
    assert!(shell_launch["env"].get("CANBAN_REQUEST_ID").is_none());
    assert!(shell_launch["env"].get("CANBAN_ATTEMPT").is_none());

    // 2. Opening an interactive Agent terminal from the same Card reuses that Card worktree and injects CANBAN_CARD.
    let agent_entry: TerminalEntry = serde_json::from_value(
        app.call(
            "terminal.create",
            json!({"project_id": p.id, "card": c.number, "agent": true, "request_key": "card-agent-wt"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(agent_entry.id.starts_with("agent-"));
    assert_eq!(agent_entry.origin_card_id.as_deref(), Some(c.id.as_str()));

    let launches = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let agent_launch: Value = serde_json::from_str(launches.lines().last().unwrap()).unwrap();
    assert_eq!(agent_launch["cwd"], wt_path);
    assert_eq!(agent_launch["env"]["CANBAN_CARD"], c.number.to_string());
    assert_eq!(agent_launch["env"]["CARD_ID"], c.id);
    assert_eq!(agent_launch["env"]["CANBAN_PROJECT"], p.id);
    // Shared-daemon tools must receive the same Card identity as the client.
    let argv = agent_launch["argv"].as_array().unwrap();
    for key in [
        "CANBAN_CARD",
        "CARD_ID",
        "CANBAN_PROJECT",
        "CANBAN_CLI",
        "CANBAN_ENDPOINT",
        "CANBAN_RUN_DIR",
    ] {
        let expected = format!(
            "shell_environment_policy.set.{key}={}",
            serde_json::to_string(&agent_launch["env"][key]).unwrap()
        );
        assert!(
            argv.iter().any(|arg| arg.as_str() == Some(&expected)),
            "{key}"
        );
    }
    assert!(!shell_launch["argv"].as_array().unwrap().iter().any(|arg| {
        arg.as_str()
            .is_some_and(|s| s.contains("shell_environment_policy"))
    }));

    assert!(agent_launch["env"].get("CANBAN_REQUEST_ID").is_none());
    assert!(agent_launch["env"].get("CANBAN_ATTEMPT").is_none());
    assert_eq!(
        app.events()
            .unwrap()
            .iter()
            .filter(|e| e.card_id == c.id && e.kind == "workspace.prepared")
            .count(),
        1,
        "Reusing the prepared Card worktree must not log duplicate workspace.prepared events"
    );
    assert!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .all(|t| t.agent.is_none())
    );
}

#[test]
fn terminal_launches_inject_default_ghostty_identity_and_respect_hc_env_overrides() {
    let (dir, app, p, c) = setup();
    core(&app, dir.path());

    // 1. Default config injects TERM=xterm-256color, TERM_PROGRAM=ghostty, COLORTERM=truecolor
    //    for both manual Shell terminals and interactive Card Agent terminals.
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "request_key": "ghostty-default-shell"}),
    )
    .unwrap();
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "card": c.number, "agent": true, "request_key": "ghostty-default-agent"}),
    )
    .unwrap();

    let launches_raw = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let launches: Vec<Value> = launches_raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(launches.len(), 2);
    for l in &launches {
        assert_eq!(l["env"]["TERM"], "xterm-256color");
        assert_eq!(l["env"]["TERM_PROGRAM"], "ghostty");
        assert_eq!(l["env"]["COLORTERM"], "truecolor");
    }

    // 2. Explicit hc_env overrides take precedence over defaults without clobbering other defaults.
    app.config
        .write()
        .unwrap()
        .hc_env
        .insert("TERM_PROGRAM".into(), "custom-term".into());
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "request_key": "ghostty-override-shell"}),
    )
    .unwrap();
    let updated_raw = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let overridden: Value = serde_json::from_str(updated_raw.lines().last().unwrap()).unwrap();
    assert_eq!(overridden["env"]["TERM"], "xterm-256color");
    assert_eq!(overridden["env"]["TERM_PROGRAM"], "custom-term");
    assert_eq!(overridden["env"]["COLORTERM"], "truecolor");

    // 3. Local terminal launches inject merged `PATH` and respect `hc_env["PATH"]` overrides.
    if std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_none() {
        app.call(
            "terminal.create",
            json!({"project_id": p.id, "request_key": "local-path-default"}),
        )
        .unwrap();
        let path_raw = std::fs::read_to_string(dir.path().join("launches")).unwrap();
        let default_launch: Value = serde_json::from_str(path_raw.lines().last().unwrap()).unwrap();
        let injected_path = default_launch["env"]["PATH"]
            .as_str()
            .expect("local terminal launch must include PATH");
        assert!(
            injected_path.contains("/usr/bin") && injected_path.contains("/bin"),
            "injected PATH should retain system binaries: {injected_path}"
        );
    }

    app.config.write().unwrap().hc_env.insert(
        "PATH".into(),
        "/custom/npm/bin:/custom/node/bin:/usr/bin:/bin".into(),
    );
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "request_key": "local-path-override"}),
    )
    .unwrap();
    let after_override = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let path_overridden: Value =
        serde_json::from_str(after_override.lines().last().unwrap()).unwrap();
    assert_eq!(
        path_overridden["env"]["PATH"],
        "/custom/npm/bin:/custom/node/bin:/usr/bin:/bin"
    );
}

#[test]
fn terminal_run_uses_default_explicit_and_persisted_launch_sizes() {
    let (dir, app, p, _) = setup();
    core(&app, dir.path());

    // 1. Unsized launch uses fallback 120x36 (never Herdr's 200x120 headless default).
    app.call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    let launches: Vec<Value> = std::fs::read_to_string(dir.path().join("launches"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(launches[0]["size"], json!({"cols": 120, "rows": 36}));

    // 2. Explicit size from UI is forwarded to terminal.run and remembered across restart.
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "size": {"cols": 134, "rows": 44}}),
    )
    .unwrap();
    let launches: Vec<Value> = std::fs::read_to_string(dir.path().join("launches"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(launches[1]["size"], json!({"cols": 134, "rows": 44}));

    let home = app.home.clone();
    drop(app);
    let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    core(&reopened, dir.path());

    // 3. Subsequent unsized launch after restart inherits persisted 134x44.
    reopened
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    let launches: Vec<Value> = std::fs::read_to_string(dir.path().join("launches"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(launches[2]["size"], json!({"cols": 134, "rows": 44}));
}

#[test]
fn worktree_create_and_promote_support_fork_upstream_default_branch_and_dirty_main() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };

    // 1. Verify `resolve_sync_remote` matches the project's GitHub repo remote (`upstream`).
    let gh_repo = dir.path().join("gh-fork");
    std::fs::create_dir_all(&gh_repo).unwrap();
    git(&gh_repo, &["init", "-b", "develop"]);
    git(
        &gh_repo,
        &["remote", "add", "origin", "git@github.com:jjyr/fiber.git"],
    );
    git(
        &gh_repo,
        &[
            "remote",
            "add",
            "upstream",
            "git@github.com:nervosnetwork/fiber.git",
        ],
    );
    let gh_proj: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "FiberGH", "directory": gh_repo, "github_auto_link": false}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut gh_proj_linked = gh_proj.clone();
    gh_proj_linked.github = Some(GitHubSource {
        pr_intake: GitHubPrIntake::Off,
        authors: Vec::new(),
        labels: Vec::new(),
        include_drafts: false,
        paused: false,
        repository: "nervosnetwork/fiber".into(),
        repository_id: 1,
        generation: "g1".into(),
        intake: GitHubIntake::Off,
        synced_at: None,
        error: None,
    });
    assert_eq!(
        app.resolve_sync_remote(&app.ops(), &gh_proj_linked),
        "upstream"
    );

    // 2. End-to-end bare upstream fork layout: stale `target_branch = lsp-support`,
    // `upstream/HEAD -> develop`, dirty main checkout, `sync_base = true`, and `worktree.promote`.
    let upstream_bare = dir.path().join("upstream.git");
    let writer = dir.path().join("writer");
    let src = dir.path().join("fiber-local");
    std::fs::create_dir_all(&writer).unwrap();
    git(
        &writer,
        &[
            "init",
            "--bare",
            "-b",
            "develop",
            upstream_bare.to_str().unwrap(),
        ],
    );
    git(&writer, &["init", "-b", "develop"]);
    for (k, v) in [
        ("user.name", "Tester"),
        ("user.email", "tester@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&writer, &["config", k, v]);
    }
    std::fs::write(writer.join("fiber.txt"), "v1\n").unwrap();
    git(&writer, &["add", "."]);
    git(&writer, &["commit", "-m", "v1"]);
    git(
        &writer,
        &["remote", "add", "upstream", upstream_bare.to_str().unwrap()],
    );
    git(&writer, &["push", "upstream", "develop"]);

    git(
        dir.path(),
        &[
            "clone",
            "-b",
            "develop",
            upstream_bare.to_str().unwrap(),
            src.to_str().unwrap(),
        ],
    );
    for (k, v) in [
        ("user.name", "Tester"),
        ("user.email", "tester@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&src, &["config", k, v]);
    }
    // Remove `origin` and use `upstream` so `resolve_sync_remote` selects `upstream`.
    git(&src, &["remote", "rename", "origin", "upstream"]);
    git(&src, &["remote", "set-head", "upstream", "develop"]);
    git(&src, &["branch", "lsp-support"]);

    let proj: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Fiber",
                "directory": src,
                "target_branch": "lsp-support",
                "github_auto_link": false,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(proj.target_branch.as_deref(), Some("lsp-support"));

    // `git.refs` still reports repo default branch `develop` and `sync_remote: "upstream"`.
    let refs = app
        .call("git.refs", json!({"project_id": proj.id}))
        .unwrap();
    assert_eq!(refs["default_branch"], "develop");
    assert_eq!(refs["sync_remote"], "upstream");

    // Advance upstream `develop` and dirty the main checkout on `develop`.
    std::fs::write(writer.join("fiber.txt"), "v2\n").unwrap();
    git(&writer, &["add", "."]);
    git(&writer, &["commit", "-m", "v2"]);
    let v2_sha = git(&writer, &["rev-parse", "HEAD"]);
    git(&writer, &["push", "upstream", "develop"]);
    std::fs::write(src.join("local-wip.txt"), "keep\n").unwrap();

    // Creating a worktree with no base_branch and sync_base=true branches from latest `upstream/develop`.
    let created = app
        .call(
            "worktree.create",
            json!({
                "project_id": proj.id,
                "branch": "feat/upstream-sync",
                "sync_base": true,
                "launch": "none",
                "request_key": "wt-fork-sync-1"
            }),
        )
        .unwrap();
    let wt: ManagedWorktree = serde_json::from_value(created["worktree"].clone()).unwrap();
    assert_eq!(wt.base_branch, "upstream/develop");
    assert_eq!(wt.base_commit.as_deref(), Some(v2_sha.as_str()));
    assert_eq!(
        std::fs::read_to_string(Path::new(&wt.path).join("fiber.txt")).unwrap(),
        "v2\n"
    );
    assert_eq!(
        std::fs::read_to_string(src.join("local-wip.txt")).unwrap(),
        "keep\n"
    );

    // Promoting strips the `upstream/` remote prefix so `workspace.target_branch` is `"develop"`.
    let promoted: Card = serde_json::from_value(
        app.call(
            "worktree.promote",
            json!({
                "worktree_id": wt.id,
                "title": "Upstream spike",
                "request_key": "wt-fork-promote-1"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        promoted
            .workspace
            .as_ref()
            .and_then(|w| w.target_branch.as_deref()),
        Some("develop")
    );

    // Replaying `worktree.promote` with the same `request_key` remains idempotent after row removal.
    let replayed: Card = serde_json::from_value(
        app.call(
            "worktree.promote",
            json!({
                "worktree_id": wt.id,
                "title": "Upstream spike",
                "request_key": "wt-fork-promote-1"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(replayed.id, promoted.id);
    assert_eq!(
        replayed
            .workspace
            .as_ref()
            .and_then(|w| w.target_branch.as_deref()),
        Some("develop")
    );
}

#[test]
fn inherit_terminal_prefers_worktree_and_subdirectory_cwd_over_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let home = root.join("home");
    let src = root.join("source");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("README.md"), "# Seed\n").unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&src, &["init", "-b", "main"]);
    git(&src, &["config", "user.name", "Tester"]);
    git(&src, &["config", "user.email", "tester@example.invalid"]);
    git(&src, &["config", "commit.gpgsign", "false"]);
    git(&src, &["add", "."]);
    git(&src, &["commit", "-m", "seed"]);

    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    core(&app, dir.path());
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Worktree Inherit", "directory": src}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "Card worktree", "body": "Notes"}),
        )
        .unwrap(),
    )
    .unwrap();

    let last_launch = || -> Value {
        let raw = std::fs::read_to_string(dir.path().join("launches")).unwrap();
        serde_json::from_str(raw.lines().last().unwrap()).unwrap()
    };

    // 1. Managed Branch Worktree root and subdirectory inherit that worktree (not `p.directory`).
    let managed = app
        .call(
            "worktree.create",
            json!({
                "project_id": p.id,
                "branch": "feat/managed-wt",
                "launch": "shell",
                "request_key": "inherit-wt-create"
            }),
        )
        .unwrap();
    let wt: ManagedWorktree = serde_json::from_value(managed["worktree"].clone()).unwrap();
    let wt_tid = managed["terminal"]["id"].as_str().unwrap().to_owned();
    let wt_sub = PathBuf::from(&wt.path).join("sub");
    std::fs::create_dir_all(&wt_sub).unwrap();

    app.call("terminal.create", json!({"inherit_terminal_id": wt_tid}))
        .unwrap();
    let launch_wt_root = last_launch();
    assert_eq!(launch_wt_root["cwd"], wt.path);
    assert_eq!(launch_wt_root["env"]["CANBAN_PROJECT"], p.id);
    assert!(launch_wt_root["env"].get("CANBAN_CARD").is_none());

    std::fs::write(dir.path().join("cwd"), wt_sub.to_string_lossy().as_ref()).unwrap();
    let wt_sub_entry: TerminalEntry = serde_json::from_value(
        app.call("terminal.create", json!({"inherit_terminal_id": wt_tid}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        wt_sub_entry.origin_project_id.as_deref(),
        Some(p.id.as_str())
    );
    assert_eq!(last_launch()["cwd"], wt_sub.to_string_lossy().as_ref());

    // 2. External `git worktree` (matched via common `.git`) root and subdirectory inherit that worktree.
    let ext_wt = root.join("external-wt");
    git(
        &src,
        &[
            "worktree",
            "add",
            "-b",
            "feat/external-wt",
            ext_wt.to_str().unwrap(),
        ],
    );
    let ext_sub = ext_wt.join("nested");
    std::fs::create_dir_all(&ext_sub).unwrap();
    let ext_seed = app.call("terminal.create", json!({"cwd": ext_wt})).unwrap();
    let ext_tid = ext_seed["id"].as_str().unwrap().to_owned();
    std::fs::remove_file(dir.path().join("cwd")).unwrap();

    app.call("terminal.create", json!({"inherit_terminal_id": ext_tid}))
        .unwrap();
    let launch_ext_root = last_launch();
    assert_eq!(launch_ext_root["cwd"], ext_wt.to_string_lossy().as_ref());
    assert_eq!(launch_ext_root["env"]["CANBAN_PROJECT"], p.id);

    std::fs::write(dir.path().join("cwd"), ext_sub.to_string_lossy().as_ref()).unwrap();
    app.call("terminal.create", json!({"inherit_terminal_id": ext_tid}))
        .unwrap();
    assert_eq!(last_launch()["cwd"], ext_sub.to_string_lossy().as_ref());

    // 3. Active Card worktree subdirectory retains the subdirectory while keeping Card env.
    std::fs::remove_file(dir.path().join("cwd")).unwrap();
    let card_shell = app
        .call(
            "terminal.create",
            json!({"project_id": p.id, "card_id": c.id}),
        )
        .unwrap();
    let card_tid = card_shell["id"].as_str().unwrap().to_owned();
    let card_wt = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|x| x.id == c.id)
        .unwrap()
        .worktree
        .unwrap();
    let card_sub = PathBuf::from(&card_wt).join("pkg");
    std::fs::create_dir_all(&card_sub).unwrap();
    std::fs::write(dir.path().join("cwd"), card_sub.to_string_lossy().as_ref()).unwrap();
    app.call("terminal.create", json!({"inherit_terminal_id": card_tid}))
        .unwrap();
    let launch_card_sub = last_launch();
    assert_eq!(launch_card_sub["cwd"], card_sub.to_string_lossy().as_ref());
    assert_eq!(launch_card_sub["env"]["CANBAN_CARD"], c.number.to_string());
    assert_eq!(launch_card_sub["env"]["CARD_ID"], c.id);
    assert_eq!(launch_card_sub["env"]["CANBAN_PROJECT"], p.id);

    // 4. Project root subdirectory retains the subdirectory.
    let proj_seed = app
        .call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    let proj_tid = proj_seed["id"].as_str().unwrap().to_owned();
    let proj_sub = src.join("docs");
    std::fs::write(dir.path().join("cwd"), proj_sub.to_string_lossy().as_ref()).unwrap();
    app.call("terminal.create", json!({"inherit_terminal_id": proj_tid}))
        .unwrap();
    assert_eq!(last_launch()["cwd"], proj_sub.to_string_lossy().as_ref());

    // 5. Explicit `project_id` / `card_id` launches still target Project root and Card worktree root.
    app.call("terminal.create", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(last_launch()["cwd"], p.directory);
    app.call(
        "terminal.create",
        json!({"project_id": p.id, "card_id": c.id}),
    )
    .unwrap();
    assert_eq!(last_launch()["cwd"], card_wt);

    // 6. Deleted inherited subdirectory falls back to Card worktree root or cached `directory_root`.
    std::fs::remove_dir_all(&card_sub).unwrap();
    std::fs::remove_dir_all(&wt_sub).unwrap();
    app.observer_running.store(true, Ordering::SeqCst);
    {
        let mut inv = app.inventory.lock().unwrap();
        inv.core = "original".into();
        inv.last_at = now();
        inv.records.insert(
            "t1".into(),
            json!({
                "address": "t1",
                "state": "live",
                "foreground": {"cwd": card_sub}
            }),
        );
    }
    *app.terminal_contexts.lock().unwrap() = vec![TerminalContext {
        terminal_id: card_tid.clone(),
        core_id: "original".into(),
        terminal: "t1".into(),
        cwd: Some(card_sub.to_string_lossy().into_owned()),
        directory_root: None,
        project_id: Some(p.id.clone()),
        card_id: Some(c.id.clone()),
        ..Default::default()
    }];
    app.call("terminal.create", json!({"inherit_terminal_id": card_tid}))
        .unwrap();
    assert_eq!(last_launch()["cwd"], card_wt);

    {
        let mut inv = app.inventory.lock().unwrap();
        inv.last_at = now();
        inv.records.insert(
            "t1".into(),
            json!({
                "address": "t1",
                "state": "live",
                "foreground": {"cwd": wt_sub}
            }),
        );
    }
    *app.terminal_contexts.lock().unwrap() = vec![TerminalContext {
        terminal_id: wt_tid.clone(),
        core_id: "original".into(),
        terminal: "t1".into(),
        cwd: Some(wt_sub.to_string_lossy().into_owned()),
        directory_root: Some(wt.path.clone()),
        project_id: Some(p.id.clone()),
        worktree_id: Some(wt.id.clone()),
        branch: Some(wt.branch),
        ..Default::default()
    }];
    app.call("terminal.create", json!({"inherit_terminal_id": wt_tid}))
        .unwrap();
    assert_eq!(last_launch()["cwd"], wt.path);
}

#[test]
fn inherit_terminal_uses_live_inventory_cwd_while_directory_context_is_updating() {
    let (dir, app, p, _) = setup();
    core(&app, dir.path());
    let src = PathBuf::from(&p.directory);
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("README.md"), "# Seed\n").unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&src, &["init", "-b", "main"]);
    git(&src, &["config", "user.name", "Tester"]);
    git(&src, &["config", "user.email", "tester@example.invalid"]);
    git(&src, &["config", "commit.gpgsign", "false"]);
    git(&src, &["add", "."]);
    git(&src, &["commit", "-m", "seed"]);

    let created = app
        .call(
            "worktree.create",
            json!({
                "project_id": p.id,
                "branch": "feat/live-inventory",
                "launch": "none",
                "request_key": "live-inv-wt"
            }),
        )
        .unwrap();
    let wt: ManagedWorktree = serde_json::from_value(created["worktree"].clone()).unwrap();
    let wt_sub = PathBuf::from(&wt.path).join("sub");
    std::fs::create_dir_all(&wt_sub).unwrap();

    app.observer_running.store(true, Ordering::SeqCst);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let entry = TerminalEntry {
                id: "shell-wt".into(),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: Some("Terminal 1".into()),
                origin_project_id: Some(p.id.clone()),
                origin_card_id: None,
                agent_engine: None,
                created_at: now(),
                runtime: Some(TerminalRuntime {
                    core_id: Some("original".into()),
                    terminal: Some("t1".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
            };
            db.save(&entry)
        })
        .unwrap();
    assert!(app.snapshot().unwrap().terminal_contexts.is_empty());

    {
        let mut inv = app.inventory.lock().unwrap();
        inv.core = "original".into();
        inv.last_at = now();
        inv.records.insert(
            "t1".into(),
            json!({
                "address": "t1",
                "state": "live",
                "foreground": {"cwd": wt_sub}
            }),
        );
    }

    let inherited_entry: TerminalEntry = serde_json::from_value(
        app.call(
            "terminal.create",
            json!({"inherit_terminal_id": "shell-wt"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        inherited_entry.origin_project_id.as_deref(),
        Some(p.id.as_str())
    );
    let launches = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let first_launch: Value = serde_json::from_str(launches.lines().last().unwrap()).unwrap();
    assert_eq!(first_launch["cwd"], wt_sub.to_string_lossy().as_ref());

    // Core identity fence: a mismatched Core inventory must never supply an inherited cwd.
    {
        let mut inv = app.inventory.lock().unwrap();
        inv.core = "replacement".into();
    }
    app.terminal_contexts.lock().unwrap().clear();
    app.call(
        "terminal.create",
        json!({"inherit_terminal_id": "shell-wt"}),
    )
    .unwrap();
    let launches = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let second_launch: Value = serde_json::from_str(launches.lines().last().unwrap()).unwrap();
    assert_ne!(second_launch["cwd"], wt_sub.to_string_lossy().as_ref());
    assert_ne!(second_launch["cwd"], p.directory);
}

#[test]
fn close_unknown_configuration_session_cancels_and_finishes() {
    let (_dir, app, p, _) = setup();
    let opened = app
        .call(
            "configuration.open",
            json!({"project_id": p.id, "profile_id": "codex"}),
        )
        .unwrap();
    let tid = opened["terminal_id"].as_str().unwrap().to_owned();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut session: ConfigurationSession = db.get(&tid)?;
            session.state = "unknown".into();
            session.error = Some("No space left on device".into());
            db.save(&session)?;
            let mut item: TerminalEntry = db.get(&tid)?;
            item.runtime = Some(TerminalRuntime {
                core_id: None,
                terminal: None,
                state: "unknown".into(),
                connection: "unavailable".into(),
                error: Some("No space left on device".into()),
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            });
            db.save(&item)
        })
        .unwrap();

    let closed = app
        .call(
            "terminal.close",
            json!({"terminal_id": tid, "request_key": "close-unknown-cfg"}),
        )
        .unwrap();
    assert_eq!(closed, json!({"state": "closed", "terminal_id": tid}));
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.configuration_sessions[0].state, "cancelled");
    let entry = snap.terminals.iter().find(|t| t.id == tid).unwrap();
    let rt = entry.runtime.as_ref().unwrap();
    assert_eq!(rt.state, "finished");
    assert_eq!(rt.error.as_deref(), Some("No space left on device"));
}
