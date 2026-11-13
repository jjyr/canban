use super::*;
use canban_types::*;
use serde_json::json;

fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    for id in ["attention", "review", "working", "updated", "quiet"] {
        s.projects.push(serde_json::from_value(json!({
            "id":id,"name":id,"directory":"/example","workflow":Workflow::default(),"created_at":1
        })).unwrap());
        s.cards.push(serde_json::from_value(json!({
            "id":id,"project_id":id,"number":1,"title":id,"body":"","stage":"develop","revision":1,"created_at":1
        })).unwrap());
    }
    s
}
fn attempt(s: &mut Snapshot, card: &str, profile: usize, state: &str) -> String {
    let mut a = canban_scheduler::attempt(
        card,
        card,
        1,
        "develop",
        "stage_entry",
        Config::default().profiles[profile].clone(),
        "Task".into(),
        None,
    );
    a.id = format!("{card}-{profile}");
    a.created_at = 10;
    a.state = state.into();
    let id = a.id.clone();
    s.attempts.push(a);
    id
}
fn event(s: &mut Snapshot, project: &str, kind: &str, time: u64) {
    s.events.push(Event {
        id: format!("{project}-{time}"),
        project_id: project.into(),
        card_id: project.into(),
        sequence: time,
        kind: kind.into(),
        actor: "user".into(),
        message: String::new(),
        data: json!({}),
        created_at: time,
    });
}

#[test]
fn attention_unifies_review_and_input_by_card_and_ignores_working_and_archived() {
    let mut s = fixture();
    let blocked = attempt(&mut s, "attention", 0, "blocked");
    attempt(&mut s, "attention", 1, "blocked");
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Answer needed".into(),
        attempt_id: Some(blocked),
        next_stage: None,
    });
    let review = attempt(&mut s, "review", 0, "completed");
    // Same Card has a Review gate and another Profile requiring input: still one.
    attempt(&mut s, "review", 1, "blocked");
    s.cards[1].human_gate = Some(HumanGate {
        reason: "Review needed".into(),
        attempt_id: Some(review),
        next_stage: None,
    });
    attempt(&mut s, "working", 0, "starting");
    attempt(&mut s, "working", 1, "queued");
    let rows = projects(&s, &[]);
    assert_eq!(
        rows.iter().find(|r| r.id == "attention").unwrap().attention,
        1
    );
    assert_eq!(rows.iter().find(|r| r.id == "review").unwrap().attention, 1);
    assert_eq!(
        rows.iter().find(|r| r.id == "working").unwrap().attention,
        0
    );
    // A Review column alone is not an outstanding human decision.
    s.cards[3].stage = "review".into();
    assert_eq!(
        projects(&s, &[])
            .iter()
            .find(|r| r.id == "updated")
            .unwrap()
            .attention,
        0
    );
    s.cards[0].archived_at = Some(20);
    s.cards[1].removed_at = Some(20);
    assert!(projects(&s, &[]).iter().all(|r| r.attention == 0));
}

#[test]
fn every_project_is_switchable_and_visits_win_over_events_or_attention() {
    let mut s = fixture();
    let recent = vec![
        "card:elsewhere".into(),
        "project:quiet".into(),
        "project:working".into(),
    ];
    let original = projects(&s, &recent);
    assert_eq!(original.len(), s.projects.len());
    assert_eq!(original[0].id, "quiet");
    assert_eq!(original[1].id, "working");
    s.cards[0].attention = Some("New input".into());
    event(&mut s, "attention", "comment.created", 99999);
    event(&mut s, "updated", "note.updated", 100000);
    assert_eq!(
        projects(&s, &recent)
            .iter()
            .map(|r| &r.id)
            .collect::<Vec<_>>(),
        original.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
    assert_eq!(
        stable_ids(&stable_ids(&[], &original), &projects(&s, &recent)),
        stable_ids(&[], &original)
    );
}

#[test]
fn project_badges_count_cards_once_across_unread_replies_and_pending_actions() {
    let mut s = fixture();
    for sequence in [100, 101] {
        event(&mut s, "quiet", "comment.replied", sequence);
        s.events.last_mut().unwrap().actor = "agent:developer".into();
    }
    s.cards
        .iter_mut()
        .find(|c| c.id == "quiet")
        .unwrap()
        .attention = Some("Review this outcome".into());
    let quiet = |s: &Snapshot| {
        projects(s, &[])
            .into_iter()
            .find(|p| p.id == "quiet")
            .unwrap()
    };
    assert_eq!(quiet(&s).attention, 1);
    assert!(quiet(&s).pending);
    s.card_reads.push(CardRead {
        id: "quiet".into(),
        card_id: "quiet".into(),
        project_id: "quiet".into(),
        sequence: 101,
    });
    assert_eq!(quiet(&s).attention, 1); // Reading cannot settle the action.
    s.cards
        .iter_mut()
        .find(|c| c.id == "quiet")
        .unwrap()
        .attention = None;
    assert_eq!(quiet(&s).attention, 0);
}

#[test]
fn responsive_capacity_has_no_three_project_limit_and_badges_do_not_move_targets() {
    let mut rows = (0..12)
        .map(|i| Summary {
            id: i.to_string(),
            name: format!("App {i}"),
            attention: 0,
            pending: false,
        })
        .collect::<Vec<_>>();
    let before = visible_count(&rows, 1100.);
    assert!(before > 3 && before < rows.len());
    assert!(visible_count(&rows, 600.) < before);
    assert_eq!(visible_count(&rows, 200.), 0);
    assert_eq!(visible_count(&rows, 2000.), 12);
    let width = chip_width(&rows[0]);
    rows[0].attention = 200;
    assert_eq!(chip_width(&rows[0]), width);
    assert_eq!(visible_count(&rows, 1100.), before);
    let all_width = rows[..3].iter().map(chip_width).sum::<f32>() + GAP * 2.;
    // If all Projects fit, no space is wasted on a nonexistent overflow button.
    assert_eq!(visible_count(&rows[..3], all_width + 76.), 3);
    assert_eq!(stable_ids(&["deleted".into()], &rows).len(), rows.len());
}
