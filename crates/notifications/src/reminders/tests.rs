use super::*;
use serde_json::json;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    s.cards.push(serde_json::from_value(json!({"id":"c","project_id":"p","number":1,"title":"Task","body":"","stage":"review","revision":1,"created_at":1})).unwrap());
    s
}
fn reply(s: &mut Snapshot, id: &str, sequence: u64) {
    s.events.push(Event {
        id: id.into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence,
        kind: "comment.replied".into(),
        actor: "agent:developer".into(),
        message: id.into(),
        request_id: None,
        data: json!({"question_id":id}),
        created_at: sequence,
    });
}
#[test]
fn card_counts_items_and_project_counts_cards_without_progress_or_own_comments() {
    let mut s = fixture();
    reply(&mut s, "one", 1);
    reply(&mut s, "two", 2);
    s.cards[0].attention = Some("Check configuration".into());
    let mut event = s.events[0].clone();
    event.id = "own".into();
    event.actor = "user".into();
    event.sequence = 3;
    s.events.push(event);
    let mut event = s.events[0].clone();
    event.id = "working".into();
    event.kind = "execution.reported".into();
    event.data = json!({"state":"working"});
    event.sequence = 4;
    s.events.push(event);
    let counts = counts(&reminders(&s));
    assert_eq!(counts.len(), 1);
    assert_eq!(
        counts["c"],
        ReminderCount {
            total: 3,
            unread: 2,
            pending: 1
        }
    );
    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 4,
    });
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 0,
            pending: 1
        }
    );
    s.cards[0].archived_at = Some(5);
    assert!(reminders(&s).is_empty());
}
fn counts_count(s: &Snapshot) -> ReminderCount {
    counts(&reminders(s))["c"]
}
#[test]
fn report_reply_and_human_gate_share_one_item_even_without_a_resource_file() {
    let mut s = fixture();
    let r:Request=serde_json::from_value(json!({"id":"r","project_id":"p","card_id":"c","stage":"review","trigger":"stage_entry","profile":Config::default().profiles[0],"instruction":"Review","state":"completed","attempt":1,"created_at":1})).unwrap();
    s.requests.push(r);
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Review needed".into(),
        request_id: Some("r".into()),
        next_stage: None,
    });
    reply(&mut s, "report-event", 1);
    s.events[0].kind = "execution.reported".into();
    s.events[0].request_id = Some("r".into());
    s.events[0].data = json!({"report_id":"report","state":"completed","path":null});
    reply(&mut s, "echo", 2);
    s.events[1].data = json!({"report_id":"report","question_id":"q","path":null});
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 1,
            pending: 1
        }
    );
    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 2,
    });
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 0,
            pending: 1
        }
    );
    s.cards[0].human_gate = None;
    assert!(reminders(&s).is_empty());
}
#[test]
fn delivery_baselines_and_duplicate_snapshots_are_silent() {
    let mut s = fixture();
    reply(&mut s, "old", 1);
    let mut delivery = DeliveryTracker::default();
    assert!(delivery.observe(&reminders(&s)).is_empty());
    reply(&mut s, "new", 2);
    assert_eq!(delivery.observe(&reminders(&s)), vec!["reply:new"]);
    assert!(delivery.observe(&reminders(&s)).is_empty());
    reply(&mut s, "offline", 3);
    delivery.baseline(&reminders(&s));
    assert!(delivery.observe(&reminders(&s)).is_empty());
    reply(&mut s, "live", 4);
    assert_eq!(delivery.observe(&reminders(&s)), vec!["reply:live"]);
}
