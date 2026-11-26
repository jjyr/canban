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
