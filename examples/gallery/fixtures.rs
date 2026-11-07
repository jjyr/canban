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
        s.cards.push(Card{id:format!("card-{i}"),project_id:"gallery-project".into(),number:i,title:title.into(),body:"## Goal\nMake progress easy to understand.\n\nKeep long descriptions readable, preserve drafts and provide clear next actions.\n\n中文与 English content uses the same document component.".into(),tags:vec!["developer".into()],context_notes:Some(vec!["note-1".into()]),stage:stage.into(),revision:1,attention:attention.map(str::to_owned),archived_at:None,archive:None,archive_error:None,worktree:Some(format!("/preview/worktrees/card-{i}")),branch:Some(format!("canban/gallery/{i}")),base_commit:Some("abcdef0123".into()),human_gate:attention.map(|reason|HumanGate{reason:reason.into(),request_id:None,next_stage:None}),delivery:None,created_at:now()});
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

/// All Overview groups, independent reasons on one row, multiple Projects and long context.
pub fn overview() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    let mut second = s.projects[0].clone();
    second.id = "second-project".into();
    second.name = "Tools".into();
    s.projects.push(second);
    let base = s.cards[0].clone();
    s.cards.clear();
    for (i, state) in [
        "blocked", "review", "working", "queued", "idle", "closed", "unknown",
    ]
    .iter()
    .enumerate()
    {
        let mut card = base.clone();
        card.id = format!("overview-card-{i}");
        card.number = i as u64 + 1;
        card.title = format!(
            "{} · {}",
            [
                "Approve repository access",
                "Inspect saved delivery",
                "Preserve keyboard focus",
                "Build the next iteration",
                "Discuss the API",
                "Resume previous investigation",
                "Recover disconnected Agent"
            ][i],
            state
        );
        card.project_id = s.projects[i % 2].id.clone();
        card.stage = if *state == "review" {
            "review"
        } else {
            "develop"
        }
        .into();
        card.human_gate = None;
        let mut req=canban_scheduler::request(&card.project_id,&card.id,&card.stage,"stage_entry",Config::default().profiles[0].clone(),"Inspect the Card context and implement a small, reviewable change.\n\nKeep the user's existing work and explain any decision that needs attention.".into(),None);
        req.id = format!("overview-request-{i}");
        req.attempt = 1;
        req.state = if *state == "queued" {
            "queued"
        } else if *state == "review" {
            "completed"
        } else {
            "running"
        }
        .into();
        if *state == "review" {
            card.human_gate = Some(HumanGate {
                reason: "Agent report ready; human review required".into(),
                request_id: Some(req.id.clone()),
                next_stage: None,
            });
        }
        if *state != "queued" {
            let mut run:Attempt=serde_json::from_value(json!({"id":Attempt::key(&req.id,1),"request_id":req.id,"project_id":card.project_id,"card_id":card.id,"number":1,"state":req.state,"core_id":"gallery-core","terminal":format!("t{}",i+1),"cwd":"/example/card-worktree","input_path":"input.md","output_path":null,"observed":{"state":if *state=="closed" {"finished"} else {"live"},"status":{"state":if *state=="review" {"done"} else {state},"kind":"permission","title":"Allow repository command?","msg":"Agent requests permission to run git status in the Card worktree."}},"error":null,"created_at":now()-180_000,"provider_session_id":format!("session-{i}"),"connection_state":if *state=="unknown" {"unavailable"} else {"connected"}})).unwrap();
            // A Gallery fixture has no polling runtime; keep its declared status fresh.
            run.lifecycle.observed_at = Some(u64::MAX);
            if *state == "blocked" {
                run.lifecycle.waiting = Some(RuntimeWait {
                    id: "gallery-wait".into(),
                    kind: "permission".into(),
                    title: "Allow repository command?".into(),
                    message: "git status in this worktree".into(),
                    since: now() - 120_000,
                });
            }
            if *state == "idle" {
                run.lifecycle.idle_since = Some(now() - 240_000);
            }
            s.attempts.push(run);
        }
        s.requests.push(req);
        s.cards.push(card);
    }
    let mut recovery = base;
    recovery.id = "routing-recovery".into();
    recovery.title = "Select an available Agent profile".into();
    recovery.human_gate = None;
    recovery.attention = Some("No Agent matches the Card tags".into());
    s.cards.push(recovery);
    s
}
