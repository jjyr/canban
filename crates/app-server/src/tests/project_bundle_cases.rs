use super::*;

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
