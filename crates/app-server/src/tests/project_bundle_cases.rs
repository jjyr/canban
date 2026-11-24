use super::*;

fn apply_yaml(app: &AppServer, project_id: &str, filename: &str, yaml: &str) {
    let collection = app
        .call("workflow.sources", json!({"project_id": project_id}))
        .unwrap();
    let revision = collection["revision"].as_str().unwrap().to_owned();
    app.call(
        "workflow.apply",
        json!({
            "project_id": project_id,
            "revision": revision,
            "sources": [{"filename": filename, "source": yaml}],
            "request_key": canban_types::id(),
        }),
    )
    .unwrap();
}

#[test]
fn active_only_export_and_import_preserves_business_state_without_terminals() {
    let dir = tempfile::tempdir().unwrap();
    let src_home = dir.path().join("src-home");
    let dst_home = dir.path().join("dst-home");
    let src_repo = dir.path().join("src-repo");
    let dst_repo = dir.path().join("dst-repo");
    std::fs::create_dir_all(&src_repo).unwrap();
    std::fs::create_dir_all(&dst_repo).unwrap();

    let src = AppServer::open(src_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let dst = AppServer::open(dst_home.clone(), PathBuf::from("/bin/echo")).unwrap();

    // Pre-populate target host with another project and an independent terminal (number=1)
    // to verify cross-project isolation and clean terminal numbering when sessions start on target.
    let existing_dst_dir = dir.path().join("dst-existing");
    std::fs::create_dir_all(&existing_dst_dir).unwrap();
    let dst_existing: Project = serde_json::from_value(
        dst.call(
            "project.create",
            json!({"name": "DstExisting", "directory": existing_dst_dir}),
        )
        .unwrap(),
    )
    .unwrap();
    dst.store
        .lock()
        .unwrap()
        .write(|db| {
            let num = db.counter("terminal-number")?;
            db.save(&TerminalEntry {
                id: "dst-shell-1".into(),
                conversation_id: None,
                number: num,
                name: Some("Existing Shell".into()),
                origin_project_id: Some(dst_existing.id.clone()),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(TerminalRuntime {
                    core_id: Some("dst-core".into()),
                    terminal: Some("dst-term-1".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
                created_at: now(),
            })
        })
        .unwrap();

    // Remove `reviewer` profile from target host to verify `missing_profiles` reporting.
    {
        let agents_path = dst_home.join("config/agents.json");
        let mut cfg_json: Value =
            serde_json::from_slice(&std::fs::read(&agents_path).unwrap()).unwrap();
        let profiles = cfg_json["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["id"] != "reviewer")
            .cloned()
            .collect::<Vec<_>>();
        cfg_json["profiles"] = json!(profiles);
        std::fs::write(&agents_path, serde_json::to_vec_pretty(&cfg_json).unwrap()).unwrap();
        dst.call("config.reload", json!({})).unwrap();
    }

    // Create source project with workflow using `@reviewer` and a cron schedule.
    let p: Project = serde_json::from_value(
        src.call(
            "project.create",
            json!({"name": "FiberMigrate", "directory": src_repo, "target_branch": "lsp-support"}),
        )
        .unwrap(),
    )
    .unwrap();
    let wf_yaml = format!(
        r#"schema: 9
id: delivery:{pid}
name: Custom Flow
on:
  develop:
    event: card.column_entered
    match:
      column: develop
    start_at: dev
  nightly:
    cron: "0 3 * * *"
    timezone: UTC
    overlap: skip
    start_at: dev
    card:
      column: backlog
      title: "Nightly ({{{{date}}}})"
steps:
  - id: dev
    uses: agent
    with:
      profile: reviewer
      instruction: Implement and test
    next: rev
  - id: rev
    uses: card.move
    with:
      column: review
    next: finish
  - id: finish
    uses: complete
"#,
        pid = p.id
    );
    apply_yaml(&src, &p.id, "development.yaml", &wf_yaml);

    // Set project plain & secret env vars on source.
    src.call(
        "env.save",
        json!({
            "scope": "project",
            "target_id": p.id,
            "entries": {
                "PLAIN_VAR": {"kind": "plain", "value": "hello-world"},
                "PROJECT_SECRET": {"kind": "secret", "value": "super-secret-value"}
            }
        }),
    )
    .unwrap();

    // Card 1: Active card with comment, attachment, live terminal/report, worktree metadata.
    let c1: Card = serde_json::from_value(
        src.call(
            "card.create",
            json!({
                "view": "full",
                "project_id": p.id,
                "title": "Active task",
                "body": "Active body"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    src.call(
        "comment.add",
        json!({"card_id": c1.id, "body": "Initial context comment", "mentions": []}),
    )
    .unwrap();
    let attachment = src
        .call(
            "attachment.add",
            json!({
                "card_id": c1.id,
                "name": "design.txt",
                "media_type": "text/plain",
                "bytes": b"attachment-payload".to_vec()
            }),
        )
        .unwrap();
    let art_path = attachment["path"].as_str().unwrap().to_owned();

    // Move c1 to develop, admit a live runtime + provider_session_id, submit a report,
    // and add worktree metadata + a standalone shell + an archived card + tombstones + activity.
    src.call(
        "card.move",
        json!({"card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
    )
    .unwrap();
    let term_entry = src.snapshot().unwrap().terminals[0].clone();
    let _admitted = admit(&src, &term_entry);
    let _rep = src
        .call(
            "execution.report",
            json!({
                "project_id": p.id,
                "card": c1.number,
                "state": "completed",
                "message": "Done",
                "body": "# Report Body\nAll checks passed",
                "request_key": "src-rep-1"
            }),
        )
        .unwrap();
    let report_path = src
        .snapshot()
        .unwrap()
        .reports
        .into_iter()
        .find(|r| r.card_id == c1.id)
        .and_then(|r| r.path)
        .unwrap();

    // Card 2: Archived local card.
    let c2: Card = serde_json::from_value(
        src.call(
            "card.create",
            json!({
                "view": "full",
                "project_id": p.id,
                "title": "Archived local",
                "body": "Old work"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    src.call(
        "card.archive",
        json!({"card_id": c2.id, "revision": c2.revision}),
    )
    .unwrap();

    // Card 3: Removed GitHub Issue tombstone.
    // Card 4: Archived GitHub Issue card.
    src.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card1: Card = db.get(&c1.id)?;
            card1.branch = Some("fiber/active-task".into());
            card1.base_commit = Some("deadbeef1234".into());
            card1.worktree = Some("/old/host/worktrees/c1".into());
            card1.workspace = Some(WorkspaceBinding {
                directory: src_repo.to_string_lossy().into_owned(),
                path: "/old/host/worktrees/c1".into(),
                branch: Some("fiber/active-task".into()),
                base_commit: Some("deadbeef1234".into()),
                target_branch: Some("lsp-support".into()),
            });
            card1
                .workspace_history
                .push(card1.workspace.clone().unwrap());
            db.save(&card1)?;

            let mut conv: Conversation = db
                .list::<Conversation>()?
                .into_iter()
                .find(|c| c.card_id == c1.id)
                .unwrap();
            conv.provider_session_id = Some("codex-thread-old-host".into());
            db.save(&conv)?;

            let mut term: TerminalEntry = db.get(&term_entry.id)?;
            if let Some(a) = &mut term.agent {
                a.state = "running".into();
                a.core_id = Some("old-core".into());
                a.terminal = Some("old-term".into());
                a.connection_state = "connected".into();
                a.provider_session_id = Some("codex-thread-old-host".into());
            }
            db.save(&term)?;

            // Standalone shell terminal on source (must NOT be exported).
            let shell_num = db.counter("terminal-number")?;
            db.save(&TerminalEntry {
                id: "src-shell-only".into(),
                conversation_id: None,
                number: shell_num,
                name: Some("Src Shell".into()),
                origin_project_id: Some(p.id.clone()),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(TerminalRuntime {
                    core_id: Some("old-core".into()),
                    terminal: Some("old-shell".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
                created_at: now(),
            })?;

            db.save(&ManagedWorktree {
                id: "mw-1".into(),
                project_id: p.id.clone(),
                branch: "dst-branch".into(),
                base_branch: "lsp-support".into(),
                base_commit: Some("cafebabe".into()),
                path: "/old/host/worktrees/dst-branch".into(),
                directory: src_repo.to_string_lossy().into_owned(),
                created_at: now(),
            })?;

            // Past-due ScheduleState to ensure target advances it into the future.
            for mut st in db
                .list::<ScheduleState>()?
                .into_iter()
                .filter(|s| s.project_id == p.id)
            {
                st.next_run_at = Some(1_000);
                st.queued_triggers = 2;
                db.save(&st)?;
            }

            let num3 = db.counter(&format!("cards:{}", p.id))?;
            db.save(&Card {
                id: "gh-removed-3".into(),
                project_id: p.id.clone(),
                number: num3,
                title: "owner/repo#900".into(),
                body: String::new(),
                stage: "backlog".into(),
                revision: 1,
                created_at: now(),
                created_by: "github".into(),
                content: CardContent::GitHubIssue {
                    source: GitHubIssueRef {
                        repository: "owner/repo".into(),
                        repository_id: 42,
                        issue_id: 900,
                        number: 900,
                        url: "https://github.com/owner/repo/issues/900".into(),
                    },
                },
                removed_at: Some(now()),
                schedule_source: None,
                source_trust: None,
                hold_auto_merge: false,
                github_links: Vec::new(),
                pull_request: None,
                pull_request_history: Vec::new(),
                delivery_mode: None,
                workspace: None,
                workspace_history: Vec::new(),
                board_order: None,
                context_notes: None,
                attention: None,
                archived_at: None,
                archive: None,
                archive_error: None,
                worktree: None,
                branch: None,
                base_commit: None,
                human_gate: None,
                delivery: None,
                resolution: None,
            })?;

            let num4 = db.counter(&format!("cards:{}", p.id))?;
            db.save(&Card {
                id: "gh-archived-4".into(),
                project_id: p.id.clone(),
                number: num4,
                title: "owner/repo#901".into(),
                body: String::new(),
                stage: "review".into(),
                revision: 1,
                created_at: now(),
                created_by: "github".into(),
                content: CardContent::GitHubIssue {
                    source: GitHubIssueRef {
                        repository: "owner/repo".into(),
                        repository_id: 42,
                        issue_id: 901,
                        number: 901,
                        url: "https://github.com/owner/repo/issues/901".into(),
                    },
                },
                removed_at: None,
                schedule_source: None,
                source_trust: None,
                hold_auto_merge: false,
                github_links: Vec::new(),
                pull_request: None,
                pull_request_history: Vec::new(),
                delivery_mode: None,
                workspace: None,
                workspace_history: Vec::new(),
                board_order: None,
                context_notes: None,
                attention: None,
                archived_at: Some(now()),
                archive: None,
                archive_error: None,
                worktree: None,
                branch: None,
                base_commit: None,
                human_gate: None,
                delivery: None,
                resolution: None,
            })?;

            db.insert_activity_segments(&[ActivitySegment {
                id: None,
                kind: ActivityKind::Dwell,
                start_ms: 1000,
                end_ms: 2000,
                project_id: Some(p.id.clone()),
                card_id: Some(c1.id.clone()),
                terminal_id: None,
                open: false,
                data: ActivityDetail::Dwell(DwellData {
                    lane: DwellLane::Card,
                    page: "card".into(),
                }),
            }])?;
            Ok(())
        })
        .unwrap();

    // 1. Export active-only bundle.
    let bundle_path = dir.path().join("fiber.canban.tar.zst");
    let exp = src
        .call(
            "project.export",
            json!({
                "project_id": "FiberMigrate",
                "output": bundle_path
            }),
        )
        .unwrap();
    assert_eq!(exp["exported"], true);
    assert_eq!(exp["project_id"], p.id);
    assert_eq!(exp["include_archived"], false);
    assert_eq!(exp["exported_cards"], 1);
    assert_eq!(exp["exported_archived_cards"], 0);
    assert_eq!(exp["exported_tombstones"], 1);
    assert_eq!(exp["skipped_archived_cards"], 2);
    assert_eq!(exp["skipped_archived_github_cards"], 1);
    assert_eq!(exp["secret_keys"], json!(["PROJECT_SECRET"]));
    assert_eq!(exp["required_profiles"], json!(["developer", "reviewer"]));
    assert_eq!(
        exp["git_hints"]["card_branches"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        exp["git_hints"]["managed_worktrees"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        exp["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("live runtime activity"))
    );

    // 2. Import onto target host.
    let before_import = now();
    let imp = dst
        .call(
            "project.import",
            json!({
                "file": bundle_path,
                "directory": dst_repo,
                "request_key": "import-fiber-1"
            }),
        )
        .unwrap();
    assert_eq!(imp["imported"], true);
    assert_eq!(imp["project_id"], p.id);
    assert_eq!(imp["imported_cards"], 1);
    assert_eq!(imp["imported_archived_cards"], 0);
    assert_eq!(imp["imported_tombstones"], 1);
    assert_eq!(imp["skipped_archived_cards"], 2);
    assert!(imp.get("cleared_live_terminals").is_none());
    assert_eq!(imp["missing_profiles"], json!(["reviewer"]));
    assert_eq!(imp["missing_secrets"], json!(["PROJECT_SECRET"]));

    let dst_repo_canon = dst_repo
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let snap = dst.snapshot().unwrap();
    let imported_proj = snap.projects.iter().find(|pr| pr.id == p.id).unwrap();
    assert_eq!(imported_proj.directory, dst_repo_canon);
    assert_eq!(
        snap.scoped_env
            .get(&Snapshot::project_env_key(&p.id))
            .and_then(|m| m.get("PLAIN_VAR")),
        Some(&EnvEntryMeta::Plain {
            value: "hello-world".into()
        })
    );
    assert!(
        snap.scoped_env
            .get(&Snapshot::project_env_key(&p.id))
            .and_then(|m| m.get("PROJECT_SECRET"))
            .is_none(),
        "secrets must not be imported without explicit configuration"
    );

    // Card 1 active; Card 2 & 4 omitted; Card 3 tombstone in DB.
    let imported_cards: Vec<_> = snap.cards.iter().filter(|c| c.project_id == p.id).collect();
    assert_eq!(imported_cards.len(), 1);
    let ic1 = imported_cards[0];
    assert_eq!(ic1.id, c1.id);
    assert!(ic1.worktree.is_none());
    assert!(ic1.workspace.is_none());
    assert!(ic1.workspace_history.is_empty());
    assert_eq!(ic1.branch.as_deref(), Some("fiber/active-task"));
    assert_eq!(ic1.base_commit.as_deref(), Some("deadbeef1234"));

    // Tombstone preserved in DB so GitHub sync won't resurrect #900.
    let all_dst_cards = dst.store.lock().unwrap().db().list::<Card>().unwrap();
    assert!(
        all_dst_cards
            .iter()
            .any(|c| c.id == "gh-removed-3" && c.removed_at.is_some())
    );
    assert!(!all_dst_cards.iter().any(|c| c.id == c2.id));

    // Terminals and conversations are NOT bundled; dangling terminal_id refs are cleared.
    assert!(
        !snap
            .terminals
            .iter()
            .any(|t| t.agent.as_ref().is_some_and(|a| a.project_id == p.id))
    );
    assert_eq!(snap.terminals.len(), 1);
    assert_eq!(snap.terminals[0].id, "dst-shell-1");
    assert!(!snap.conversations.iter().any(|c| c.project_id == p.id));
    assert!(
        snap.reports
            .iter()
            .filter(|r| r.project_id == p.id)
            .all(|r| r.terminal_id.is_none())
    );
    assert!(
        snap.workflow_runs
            .iter()
            .filter(|r| r.project_id == p.id)
            .all(|r| !r.active() && r.terminal_id.is_none())
    );

    // ScheduleState advanced into future.
    let sched = snap
        .schedule_states
        .iter()
        .find(|s| s.project_id == p.id)
        .unwrap();
    assert_eq!(sched.queued_triggers, 0);
    assert!(sched.next_run_at.unwrap() > before_import);

    // Managed files (attachment + report) are readable via resource.read.
    let att_read = dst
        .call("resource.read", json!({"card_id": c1.id, "path": art_path}))
        .unwrap();
    assert_eq!(att_read["body"], "attachment-payload");
    let rep_read = dst
        .call(
            "resource.read",
            json!({"card_id": c1.id, "path": report_path}),
        )
        .unwrap();
    assert!(
        rep_read["body"]
            .as_str()
            .unwrap()
            .contains("All checks passed")
    );

    // Card counter preserved (new card gets number 5 > max card number 4).
    let c_new: Card = serde_json::from_value(
        dst.call(
            "card.create",
            json!({"view": "full", "project_id": p.id, "title": "Post-migration card", "body": ""}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(c_new.number, 5);

    // Timeline counter preserved (new comment succeeds without sequence collision),
    // and mentioning @developer cleanly creates a fresh Conversation + TerminalEntry on target.
    dst.call(
        "comment.add",
        json!({"card_id": c1.id, "body": "Continue @developer on devpc", "mentions": ["developer"]}),
    )
    .unwrap();
    let after_mention = dst.snapshot().unwrap();
    assert_eq!(
        after_mention
            .conversations
            .iter()
            .filter(|c| c.project_id == p.id)
            .count(),
        1
    );
    let fresh_term = after_mention
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == c1.id))
        .expect("fresh TerminalEntry created for @developer after import");
    assert!(fresh_term.number > 1);

    // Activity segments excluded in active-only mode.
    let (segs, _) = dst
        .store
        .lock()
        .unwrap()
        .db()
        .query_activity_segments(0, i64::MAX as u64, Some(&p.id), None, 50)
        .unwrap();
    assert!(segs.is_empty());
}

#[test]
fn full_export_and_import_includes_archived_cards_archives_notes_and_activity() {
    let dir = tempfile::tempdir().unwrap();
    let src_home = dir.path().join("src-home");
    let dst_home = dir.path().join("dst-home");
    let src_repo = dir.path().join("src-repo");
    let dst_repo = dir.path().join("dst-repo");
    std::fs::create_dir_all(&src_repo).unwrap();
    std::fs::create_dir_all(&dst_repo).unwrap();

    let src = AppServer::open(src_home, PathBuf::from("/bin/echo")).unwrap();
    let dst = AppServer::open(dst_home, PathBuf::from("/bin/echo")).unwrap();

    let p: Project = serde_json::from_value(
        src.call(
            "project.create",
            json!({"name": "FullProj", "directory": src_repo}),
        )
        .unwrap(),
    )
    .unwrap();
    let archived_card: Card = serde_json::from_value(
        src.call(
            "card.create",
            json!({"view": "full", "project_id": p.id, "title": "Archived with attachment", "body": "Body"}),
        )
        .unwrap(),
    )
    .unwrap();
    let att = src
        .call(
            "attachment.add",
            json!({
                "card_id": archived_card.id,
                "name": "evidence.txt",
                "media_type": "text/plain",
                "bytes": b"archived-file-bytes".to_vec()
            }),
        )
        .unwrap();
    let att_path = att["path"].as_str().unwrap().to_owned();
    src.call(
        "card.archive",
        json!({"card_id": archived_card.id, "revision": archived_card.revision}),
    )
    .unwrap();

    // Seed legacy Note, source_observations, and activity_segments.
    src.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&Note {
                id: "note-1".into(),
                project_id: p.id.clone(),
                title: "Legacy Note".into(),
                body: "Preserved note".into(),
                revision: 1,
                created_at: now(),
                updated_at: now(),
                links: Vec::new(),
                card_ids: Vec::new(),
            })?;
            db.conn.execute(
                "INSERT INTO source_observations(id, data) VALUES(?1, ?2)",
                rusqlite::params![format!("github:{}", p.id), r#"{"baseline":123}"#],
            )?;
            db.insert_activity_segments(&[ActivitySegment {
                id: None,
                kind: ActivityKind::Dwell,
                start_ms: 5000,
                end_ms: 9000,
                project_id: Some(p.id.clone()),
                card_id: Some(archived_card.id.clone()),
                terminal_id: None,
                open: false,
                data: ActivityDetail::Dwell(DwellData {
                    lane: DwellLane::Other,
                    page: "board".into(),
                }),
            }])?;
            Ok(())
        })
        .unwrap();

    let bundle_path = dir.path().join("full.canban.tar.zst");
    let exp = src
        .call(
            "project.export",
            json!({
                "project_id": p.id,
                "output": bundle_path,
                "include_archived": true
            }),
        )
        .unwrap();
    assert_eq!(exp["include_archived"], true);
    assert_eq!(exp["exported_archived_cards"], 1);
    assert_eq!(exp["skipped_archived_cards"], 0);

    let imp = dst
        .call(
            "project.import",
            json!({
                "file": bundle_path,
                "directory": dst_repo,
                "name": "FullProj-DevPC"
            }),
        )
        .unwrap();
    assert_eq!(imp["imported_archived_cards"], 1);
    assert_eq!(imp["name"], "FullProj-DevPC");

    // Restoring the archived Card on target succeeds and expands its archived attachment!
    let imported_archived = dst
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == archived_card.id)
        .unwrap();
    assert!(imported_archived.archived_at.is_some());
    assert!(imported_archived.archive.is_some());

    dst.call(
        "card.restore",
        json!({"card_id": imported_archived.id, "revision": imported_archived.revision}),
    )
    .unwrap();
    let restored_att = dst
        .call(
            "resource.read",
            json!({"card_id": imported_archived.id, "path": att_path}),
        )
        .unwrap();
    assert_eq!(restored_att["body"], "archived-file-bytes");

    let dst_store = dst.store.lock().unwrap();
    let db = dst_store.db();
    assert_eq!(db.get::<Note>("note-1").unwrap().title, "Legacy Note");
    let obs: String = db
        .conn
        .query_row(
            "SELECT data FROM source_observations WHERE id=?1",
            [format!("github:{}", p.id)],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(obs, r#"{"baseline":123}"#);
    let (segs, _) = db
        .query_activity_segments(0, i64::MAX as u64, Some(&p.id), None, 50)
        .unwrap();
    assert_eq!(segs.len(), 1);
}

#[test]
fn export_import_error_and_safety_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let src_home = dir.path().join("src-home");
    let dst_home = dir.path().join("dst-home");
    let src_repo = dir.path().join("src-repo");
    let dst_repo = dir.path().join("dst-repo");
    std::fs::create_dir_all(&src_repo).unwrap();
    std::fs::create_dir_all(&dst_repo).unwrap();

    let src = AppServer::open(src_home, PathBuf::from("/bin/echo")).unwrap();
    let dst = AppServer::open(dst_home.clone(), PathBuf::from("/bin/echo")).unwrap();

    let p: Project = serde_json::from_value(
        src.call(
            "project.create",
            json!({"name": "Safety", "directory": src_repo}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        src.call(
            "card.create",
            json!({"view": "full", "project_id": p.id, "title": "Card", "body": ""}),
        )
        .unwrap(),
    )
    .unwrap();
    src.call(
        "attachment.add",
        json!({"card_id": c.id, "name": "f.txt", "media_type": "text/plain", "bytes": b"hi".to_vec()}),
    )
    .unwrap();
    src.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut proj: Project = db.get(&p.id)?;
            proj.github = Some(GitHubSource {
                repository: "owner/repo".into(),
                repository_id: 99,
                generation: "gen1".into(),
                intake: GitHubIntake::All,
                pr_intake: GitHubPrIntake::Off,
                authors: Vec::new(),
                labels: Vec::new(),
                include_drafts: false,
                paused: false,
                synced_at: None,
                error: Some("transient".into()),
            });
            db.save(&proj)
        })
        .unwrap();

    let bundle_path = dir.path().join("safety.canban.tar.zst");
    src.call(
        "project.export",
        json!({"project_id": p.id, "output": bundle_path}),
    )
    .unwrap();

    // Refuses overwrite when overwrite=false.
    assert!(
        src.call(
            "project.export",
            json!({"project_id": p.id, "output": bundle_path}),
        )
        .is_err()
    );
    src.call(
        "project.export",
        json!({"project_id": p.id, "output": bundle_path, "overwrite": true}),
    )
    .unwrap();

    // Corrupted archive fails cleanly without touching DB.
    let bad_path = dir.path().join("corrupt.canban.tar.zst");
    std::fs::write(&bad_path, b"corrupt-bytes").unwrap();
    assert!(
        dst.call(
            "project.import",
            json!({"file": bad_path, "directory": dst_repo}),
        )
        .is_err()
    );
    assert!(dst.snapshot().unwrap().projects.is_empty());

    // First import with pause_github=true succeeds and clears stale github.error.
    let imp = dst
        .call(
            "project.import",
            json!({"file": bundle_path, "directory": dst_repo, "pause_github": true}),
        )
        .unwrap();
    assert_eq!(imp["github_paused"], true);
    let imported_p = dst.snapshot().unwrap().projects.into_iter().next().unwrap();
    let gh = imported_p.github.unwrap();
    assert!(gh.paused);
    assert!(gh.error.is_none());

    // Duplicate project_id import fails cleanly without touching existing project.
    assert!(
        dst.call(
            "project.import",
            json!({"file": bundle_path, "directory": dst_repo}),
        )
        .is_err()
    );
    assert_eq!(dst.snapshot().unwrap().projects.len(), 1);
}

#[test]
fn cold_archive_list_restore_dedups_activity_and_enforces_dir_safety() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    let alt_repo = dir.path().join("alt-repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# Keep me safe\n").unwrap();
    std::fs::create_dir_all(&alt_repo).unwrap();

    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();

    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Fiber", "directory": repo}),
        )
        .unwrap(),
    )
    .unwrap();

    let active_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": p.id, "title": "Active task", "body": "Active body"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "attachment.add",
        json!({"card_id": active_card.id, "name": "spec.txt", "media_type": "text/plain", "bytes": b"spec-bytes".to_vec()}),
    )
    .unwrap();

    let archived_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": p.id, "title": "Archived task", "body": "Old body"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.archive",
        json!({"card_id": archived_card.id, "revision": archived_card.revision}),
    )
    .unwrap();

    // Add GitHub link (initially unpaused), project secret, note, activity segment, and a live terminal.
    app.call(
        "env.save",
        json!({
            "scope": "project",
            "target_id": p.id,
            "entries": {
                "APP_ENV": { "kind": "plain", "value": "prod" },
                "GITHUB_PAT": { "kind": "secret", "value": "secret-val" }
            }
        }),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut proj: Project = db.get(&p.id)?;
            proj.github = Some(GitHubSource {
                repository: "nervosnetwork/fiber".into(),
                repository_id: 77,
                generation: "gen1".into(),
                intake: GitHubIntake::All,
                pr_intake: GitHubPrIntake::Off,
                authors: Vec::new(),
                labels: Vec::new(),
                include_drafts: false,
                paused: false,
                synced_at: None,
                error: None,
            });
            db.save(&proj)?;
            db.save(&Note {
                id: "note-arch".into(),
                project_id: p.id.clone(),
                title: "Design Note".into(),
                body: "Preserved in cold archive".into(),
                card_ids: Vec::new(),
                links: Vec::new(),
                revision: 1,
                created_at: 100,
                updated_at: 200,
            })?;
            db.insert_activity_segments(&[ActivitySegment {
                id: None,
                kind: ActivityKind::Dwell,
                start_ms: 1000,
                end_ms: 5000,
                project_id: Some(p.id.clone()),
                card_id: Some(active_card.id.clone()),
                terminal_id: None,
                open: false,
                data: ActivityDetail::Dwell(DwellData {
                    lane: DwellLane::Other,
                    page: "board".into(),
                }),
            }])?;
            db.save(&TerminalEntry {
                id: "live-term-blocker".into(),
                conversation_id: None,
                number: 1,
                name: Some("Live Shell".into()),
                origin_project_id: Some(p.id.clone()),
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
                }),
                agent: None,
                created_at: 1,
            })?;
            Ok(())
        })
        .unwrap();

    // 1. Live terminal blocks archiving and leaves no leftover bundle.
    let blocked_err = app
        .call("project.archive", json!({"project_id": p.id}))
        .unwrap_err();
    assert!(blocked_err.to_string().contains("1 live terminal"));
    assert_eq!(app.snapshot().unwrap().projects.len(), 1);
    let expected_archive_path = home
        .join("project-archives")
        .join(format!("{}.canban.tar.zst", p.id));
    assert!(!expected_archive_path.exists());

    // Finish the terminal -> archive succeeds (even with agent execution context absent or CLI).
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut t: TerminalEntry = db.get("live-term-blocker")?;
            t.runtime.as_mut().unwrap().state = "finished".into();
            db.save(&t)
        })
        .unwrap();

    let arch_res = app
        .call("project.archive", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(arch_res["archived"], true);
    assert_eq!(arch_res["project_id"], p.id);
    assert_eq!(arch_res["active_cards"], 1);
    assert_eq!(arch_res["archived_cards"], 1);
    assert_eq!(arch_res["secret_keys"], json!(["GITHUB_PAT"]));
    assert!(expected_archive_path.is_file());

    // Active project + metadata dirs removed, DB active rows removed, activity_segments + repo kept.
    assert!(app.snapshot().unwrap().projects.is_empty());
    assert!(app.snapshot().unwrap().cards.is_empty());
    assert!(!home.join("projects").join(&p.id).exists());
    assert!(!home.join("archives").join(&p.id).exists());
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "# Keep me safe\n"
    );
    {
        let store = app.store.lock().unwrap();
        let db = store.db();
        let (segs, _) = db
            .query_activity_segments(0, i64::MAX as u64, Some(&p.id), None, 50)
            .unwrap();
        assert_eq!(segs.len(), 1);
    }

    // 2. Corrupt file in project-archives/ does not prevent project.archived.list from listing Fiber.
    let corrupt_path = home.join("project-archives").join("corrupt.canban.tar.zst");
    std::fs::write(&corrupt_path, b"bad-archive").unwrap();

    let listed = app.call("project.archived.list", json!({})).unwrap();
    let projects_arr = listed["projects"].as_array().unwrap();
    assert_eq!(projects_arr.len(), 1);
    assert_eq!(projects_arr[0]["project_id"], p.id);
    assert_eq!(projects_arr[0]["name"], "Fiber");
    assert_eq!(projects_arr[0]["active_cards"], 1);
    assert_eq!(projects_arr[0]["archived_cards"], 1);
    assert_eq!(projects_arr[0]["total_cards"], 2);
    assert_eq!(projects_arr[0]["secret_keys"], json!(["GITHUB_PAT"]));
    assert_eq!(listed["errors"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_file(&corrupt_path);

    // 3. Removing original repo directory without supplying `directory` fails restore safely.
    let moved_repo = dir.path().join("moved-away");
    std::fs::rename(&repo, &moved_repo).unwrap();
    let missing_dir_err = app
        .call("project.restore", json!({"project_id": p.id}))
        .unwrap_err();
    assert!(
        missing_dir_err
            .to_string()
            .contains("Original directory no longer exists")
    );
    assert!(
        !repo.exists(),
        "must not silently mkdir missing source_directory"
    );
    assert!(
        expected_archive_path.is_file(),
        "archive must stay intact on error"
    );

    // 4. Restore onto explicit alt_repo succeeds, defaults github.paused=true, deduplicates activity_segments,
    //    and deletes the archive file after commit.
    let restored = app
        .call(
            "project.restore",
            json!({
                "project_id": "Fiber",
                "directory": alt_repo,
            }),
        )
        .unwrap();
    assert_eq!(restored["restored"], true);
    assert_eq!(restored["project_id"], p.id);
    assert_eq!(restored["github_paused"], true);
    assert_eq!(restored["missing_secrets"], json!(["GITHUB_PAT"]));
    assert!(!expected_archive_path.exists());

    let list_after = app.call("project.archived.list", json!({})).unwrap();
    assert!(list_after["projects"].as_array().unwrap().is_empty());

    let snap = app.snapshot().unwrap();
    assert_eq!(snap.projects.len(), 1);
    assert!(snap.projects[0].github.as_ref().unwrap().paused);
    assert_eq!(snap.cards.len(), 2);
    {
        let store = app.store.lock().unwrap();
        let db = store.db();
        assert_eq!(db.get::<Note>("note-arch").unwrap().title, "Design Note");
        // Activity segments must NOT be duplicated on same-host archive+restore!
        let (segs, _) = db
            .query_activity_segments(0, i64::MAX as u64, Some(&p.id), None, 50)
            .unwrap();
        assert_eq!(segs.len(), 1);
    }

    // 5. Re-archive and restore with pause_github: false leaves GitHub unpaused.
    app.call("project.archive", json!({"project_id": p.id}))
        .unwrap();
    let resumed = app
        .call(
            "project.restore",
            json!({"project_id": p.id, "pause_github": false}),
        )
        .unwrap();
    assert_eq!(resumed["github_paused"], false);
    assert!(
        !app.snapshot().unwrap().projects[0]
            .github
            .as_ref()
            .unwrap()
            .paused
    );
}

#[test]
fn archive_delete_requires_user_action_and_removes_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Disposable", "directory": repo}),
        )
        .unwrap(),
    )
    .unwrap();

    app.call("project.archive", json!({"project_id": p.id}))
        .unwrap();
    let archive_path = home
        .join("project-archives")
        .join(format!("{}.canban.tar.zst", p.id));
    assert!(archive_path.is_file());

    // Agent execution context cannot permanently delete archived bundles.
    let agent_err = app
        .call(
            "project.archived.delete",
            json!({"project_id": p.id, "execution": {"agent": true}}),
        )
        .unwrap_err();
    assert!(
        agent_err
            .to_string()
            .contains("Deleting an archived Project requires a user action")
    );
    assert!(archive_path.is_file());

    // Explicit CLI/user call deletes the bundle cleanly.
    let del = app
        .call("project.archived.delete", json!({"project_id": p.id}))
        .unwrap();
    assert_eq!(del["deleted"], true);
    assert_eq!(del["project_id"], p.id);
    assert!(!archive_path.exists());
    assert!(
        app.call("project.archived.list", json!({})).unwrap()["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
