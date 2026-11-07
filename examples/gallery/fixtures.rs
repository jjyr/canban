use canban_types::{Event, ReviewPreview};
use serde_json::json;

pub fn timeline() -> Event {
    Event {id:"gallery-event".into(),project_id:"gallery-project".into(),card_id:"gallery-card".into(),sequence:1,kind:"comment.created".into(),actor:"user".into(),message:"A **shared component**, rendered from deterministic fixture data.\n\n中文输入、选择、复制与长文本。".into(),request_id:None,data:json!({}),created_at:canban_types::now()}
}

pub fn review() -> ReviewPreview {
    ReviewPreview {card_id:"gallery".into(),revision:1,target_branch:"main".into(),target_head:"0123456789".into(),card_head:"abcdef0123".into(),source_branch:"main".into(),worktree:"/example/Card".into(),files:vec!["src/example.rs".into()],patch:"diff --git a/src/example.rs b/src/example.rs\n--- a/src/example.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n-// old\n+// Reviewed · 中文".into(),truncated:false,dirty:true,conflicts:vec![],token:String::new(),merge_allowed:true,reason:None}
}

pub const TERMINAL: &str = "\x1b[1;36mCanban native terminal\x1b[0m\x1b[?25l\r\nUnicode: 世界 · café\r\nRead-only preview: selection and scroll use the production component.\r\n";

pub fn workspace() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = Snapshot::default();
    s.projects.push(Project {
        id: "gallery-project".into(),
        name: "Canban workspace".into(),
        directory: "/example/project".into(),
        workflow: Workflow::default(),
        target_branch: Some("main".into()),
        created_at: now(),
    });
    for (i, title, stage, attention) in [
        (
            1,
            "Improve search and preserve the user's context",
            "review",
            None,
        ),
        (
            2,
            "Handle a missing credential",
            "develop",
            Some("Which environment should the Agent use?"),
        ),
        (3, "Explore a smaller navigation layout", "backlog", None),
    ] {
        s.cards.push(Card{id:format!("card-{i}"),project_id:"gallery-project".into(),number:i,title:title.into(),body:"## Goal\nMake progress easy to understand.\n\nKeep long descriptions readable, preserve drafts and provide clear next actions.\n\n中文与 English content uses the same document component.".into(),tags:vec!["developer".into()],context_notes:Some(vec!["note-1".into()]),stage:stage.into(),revision:1,attention:attention.map(str::to_owned),archived_at:None,archive:None,archive_error:None,worktree:None,delivery:None,created_at:now()});
    }
    s.notes.push(Note{id:"note-1".into(),project_id:"gallery-project".into(),title:"Search ideas.md".into(),body:"# Search experience\n\nFind a Card without remembering its number.\n\nSee [Architecture](note:note-2).".into(),revision:1,created_at:now(),updated_at:now(),links:vec!["note-2".into()],card_ids:vec!["card-1".into()]});
    s.notes.push(Note {
        id: "note-2".into(),
        project_id: "gallery-project".into(),
        title: "Architecture.md".into(),
        body: "Shared decisions live here. Incoming links appear as backlinks.".into(),
        revision: 1,
        created_at: now(),
        updated_at: now(),
        links: vec![],
        card_ids: vec![],
    });
    let mut event = timeline();
    event.card_id = "card-1".into();
    event.created_at = now();
    s.events.push(event);
    s.note_events.push(NoteEvent {
        id: "note-event".into(),
        project_id: "gallery-project".into(),
        note_id: "note-1".into(),
        sequence: 1,
        kind: "note.updated".into(),
        actor: "@developer".into(),
        message: "Updated search constraints while handling Card #1".into(),
        card_id: Some("card-1".into()),
        request_id: None,
        attempt: None,
        revision: 1,
        data: json!({}),
        created_at: now(),
    });
    s
}
