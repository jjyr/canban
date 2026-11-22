//! Field-level three-way merge classification for offline `card.update` mutations.
use crate::outbox::{Mutation, UpdateBase};
use canban_types::Card;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldMergeOutcome {
    Apply {
        title: Option<String>,
        body: Option<String>,
    },
    NoOp,
    Conflict {
        fields: Vec<String>,
        mine: BTreeMap<String, String>,
        server: BTreeMap<String, String>,
    },
}

/// Classify a queued `card.update` against the latest authoritative server `Card`.
///
/// For each edited field:
/// - If `server == mine`: already satisfied on server -> skip.
/// - Else if `server == base`: no concurrent edit on that field -> auto-apply `mine`.
/// - Else: concurrent edit on the same field -> conflict.
pub fn classify_card_update(
    base: &UpdateBase,
    mine_title: Option<&str>,
    mine_body: Option<&str>,
    server: &Card,
) -> FieldMergeOutcome {
    let mut apply_title = None;
    let mut apply_body = None;
    let mut conflict_fields = Vec::new();
    let mut mine_map = BTreeMap::new();
    let mut server_map = BTreeMap::new();

    if let Some(m_title) = mine_title {
        let trimmed_mine = m_title.trim();
        let unchanged_from_base = base
            .title
            .as_deref()
            .is_some_and(|b| b.trim() == trimmed_mine);
        if !unchanged_from_base && server.title != trimmed_mine {
            if base.title.as_deref().map(str::trim) == Some(server.title.as_str()) {
                apply_title = Some(m_title.to_owned());
            } else {
                conflict_fields.push("title".into());
                mine_map.insert("title".into(), m_title.to_owned());
                server_map.insert("title".into(), server.title.clone());
            }
        }
    }

    if let Some(m_body) = mine_body {
        let unchanged_from_base = base.body.as_deref().is_some_and(|b| b == m_body);
        if !unchanged_from_base && server.body != m_body {
            if base.body.as_deref() == Some(server.body.as_str()) {
                apply_body = Some(m_body.to_owned());
            } else {
                conflict_fields.push("body".into());
                mine_map.insert("body".into(), m_body.to_owned());
                server_map.insert("body".into(), server.body.clone());
            }
        }
    }

    if !conflict_fields.is_empty() {
        return FieldMergeOutcome::Conflict {
            fields: conflict_fields,
            mine: mine_map,
            server: server_map,
        };
    }
    if apply_title.is_none() && apply_body.is_none() {
        FieldMergeOutcome::NoOp
    } else {
        FieldMergeOutcome::Apply {
            title: apply_title,
            body: apply_body,
        }
    }
}

/// Build user-facing `mine` and `server` maps for a conflicted update operation.
pub fn conflict_details(
    op: &Mutation,
    server: Option<&Card>,
) -> (
    Vec<String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
) {
    let mut fields = Vec::new();
    let mut mine = BTreeMap::new();
    let mut srv = BTreeMap::new();
    if let crate::outbox::MutationState::Conflict { fields: f } = &op.state {
        fields = f.clone();
    }
    for field in &fields {
        if let Some(val) = op.params.get(field).and_then(|v| v.as_str()) {
            mine.insert(field.clone(), val.to_owned());
        }
        if let Some(card) = server {
            let s_val = match field.as_str() {
                "title" => Some(card.title.clone()),
                "body" => Some(card.body.clone()),
                _ => None,
            };
            if let Some(s) = s_val {
                srv.insert(field.clone(), s);
            }
        }
    }
    (fields, mine, srv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::CardContent;

    fn sample_card(title: &str, body: &str, rev: u64) -> Card {
        Card {
            schedule_source: None,
            source_trust: Some("local".into()),
            hold_auto_merge: false,
            github_links: Vec::new(),
            created_by: String::new(),
            content: CardContent::Local,
            removed_at: None,
            pull_request: None,
            pull_request_history: Vec::new(),
            delivery_mode: None,
            workspace: None,
            workspace_history: Vec::new(),
            id: "c1".into(),
            project_id: "p1".into(),
            number: 1,
            board_order: Some(0),
            title: title.into(),
            body: body.into(),
            context_notes: None,
            stage: "backlog".into(),
            revision: rev,
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
            created_at: 1,
        }
    }

    #[test]
    fn disjoint_fields_auto_apply_and_same_field_conflicts() {
        let base = UpdateBase {
            title: Some("Old title".into()),
            body: Some("Old body".into()),
        };
        // Server changed title; offline changed body -> auto-apply body only.
        let server_title_changed = sample_card("New server title", "Old body", 2);
        assert_eq!(
            classify_card_update(
                &base,
                Some("Old title"),
                Some("My body"),
                &server_title_changed
            ),
            FieldMergeOutcome::Apply {
                title: None,
                body: Some("My body".into()),
            }
        );

        // Server already matches mine -> NoOp.
        let server_same = sample_card("My title", "My body", 2);
        assert_eq!(
            classify_card_update(&base, Some("My title"), Some("My body"), &server_same),
            FieldMergeOutcome::NoOp
        );

        // Server changed body differently from mine -> conflict on body only.
        let server_body_conflict = sample_card("Old title", "Server body", 2);
        let outcome = classify_card_update(
            &base,
            Some("Old title"),
            Some("My body"),
            &server_body_conflict,
        );
        match outcome {
            FieldMergeOutcome::Conflict {
                fields,
                mine,
                server,
            } => {
                assert_eq!(fields, vec!["body"]);
                assert_eq!(mine.get("body").map(String::as_str), Some("My body"));
                assert_eq!(server.get("body").map(String::as_str), Some("Server body"));
            }
            other => panic!("Expected Conflict, got {other:?}"),
        }
    }
}
