use super::*;
use serde_json::json;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for (id, name) in [("p1", "Canban"), ("p2", "Tools")] {
        s.projects.push(Project {
            github: None,
            delivery_mode: Default::default(),
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
        s.cards.push(serde_json::from_value(json!({"id":id,"project_id":project,"number":42,"title":title,"body":"","tags":[],"stage":"backlog","revision":1,"attention":null,"archived_at":null,"archive":null,"archive_error":null,"worktree":null,"created_at":1})).unwrap());
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
    assert_eq!(
        ranked(&rows, "Note arcdcs", &[], false)[0].target,
        Target::Note("n1".into())
    );
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
            title: "Note".into(),
            detail: String::new(),
            kind: "Note",
            target: Target::Note("exact".into()),
            fields: vec!["Note".into()],
        },
        Candidate {
            id: "prefix".into(),
            title: "Notebook".into(),
            detail: String::new(),
            kind: "Note",
            target: Target::Note("prefix".into()),
            fields: vec!["Notebook".into()],
        },
    ];
    assert_eq!(
        ranked(&rows, "note", &["prefix".into()], false)[0].id,
        "exact"
    );
}

#[test]
fn duplicate_titles_sort_by_visible_owner_before_internal_identity() {
    let a = Candidate {
        id: "z".into(),
        title: "Architecture.md".into(),
        detail: "Note · Canban".into(),
        kind: "Note",
        target: Target::Note("z".into()),
        fields: vec!["Architecture.md".into()],
    };
    let mut b = a.clone();
    b.id = "a".into();
    b.detail = "Note · Tools".into();
    assert_eq!(ranked(&[b, a], "Architecture", &[], false)[0].id, "z");
}
