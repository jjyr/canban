//! Content-derived Card tags for indexing, Search modal queries, and Board filtering.
use crate::presenter::Ui;
use canban_types::{Card, Snapshot, normalize_tag};
use nsh_base::text_view::{Block, Inline, Parser};
use nsh_core::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum number of tag chips displayed on a Board Card tile by default.
pub const MAX_BOARD_TILE_TAGS: usize = 3;

/// Canonical lookup map (`lowercase -> display name`) of all registered tags.
/// Prioritizes `target_project` first when supplied, then other Projects and `local_tags`,
/// plus any explicit `[#tag](tag:name)` links present in saved Card bodies.
pub fn known_map(
    snapshot: &Snapshot,
    local_tags: &BTreeSet<String>,
    target_project: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut insert = |raw: &str| {
        if let Some(tag) = normalize_tag(raw) {
            out.entry(tag.to_lowercase()).or_insert(tag);
        }
    };
    if let Some(pid) = target_project
        && let Some(project) = snapshot.projects.iter().find(|p| p.id == pid)
    {
        for tag in &project.tags {
            insert(tag);
        }
    }
    for project in &snapshot.projects {
        if target_project != Some(project.id.as_str()) {
            for tag in &project.tags {
                insert(tag);
            }
        }
    }
    for tag in local_tags {
        insert(tag);
    }
    for card in snapshot.cards.iter().filter(|c| c.removed_at.is_none()) {
        for tag in explicit_link_tags(&card.body) {
            insert(&tag);
        }
    }
    out
}

/// Ordered list of known tags for completion/menus: current Project tags first, then workspace tags.
pub fn ordered_known(
    snapshot: &Snapshot,
    local_tags: &BTreeSet<String>,
    target_project: Option<&str>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |raw: &str| {
        if let Some(tag) = normalize_tag(raw)
            && seen.insert(tag.to_lowercase())
        {
            out.push(tag);
        }
    };
    if let Some(pid) = target_project
        && let Some(project) = snapshot.projects.iter().find(|p| p.id == pid)
    {
        for tag in &project.tags {
            push(tag);
        }
    }
    for project in &snapshot.projects {
        if target_project != Some(project.id.as_str()) {
            for tag in &project.tags {
                push(tag);
            }
        }
    }
    for tag in local_tags {
        push(tag);
    }
    for card in snapshot.cards.iter().filter(|c| c.removed_at.is_none()) {
        for tag in explicit_link_tags(&card.body) {
            push(&tag);
        }
    }
    out
}

fn explicit_link_tags(source: &str) -> Vec<String> {
    if !source.contains("tag:") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for link in crate::links::content_links(source) {
        if let Some(raw) = link.strip_prefix("tag:")
            && let Some(tag) = normalize_tag(raw)
        {
            out.push(tag);
        }
    }
    out
}

/// Extract deduplicated tags from Markdown prose (excluding code blocks, inline code,
/// blockquotes, URLs, headings, and unknown `#words` such as CSS hex colors).
pub fn body_tags(body: &str, known: &BTreeMap<String, String>) -> Vec<String> {
    if body.is_empty() || (!body.contains('#') && !body.contains("tag:")) {
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut record = |canonical: String| {
        if seen.insert(canonical.to_lowercase()) {
            out.push(canonical);
        }
    };

    fn scan_text(text: &str, known: &BTreeMap<String, String>, record: &mut impl FnMut(String)) {
        for (start, _) in text.char_indices().filter(|(_, c)| *c == '#') {
            if text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | '[' | '{'))
            {
                continue;
            }
            let tail = &text[start + 1..];
            let len = tail
                .char_indices()
                .find(|(_, c)| !c.is_alphanumeric() && *c != '_' && *c != '-')
                .map(|(i, _)| i)
                .unwrap_or(tail.len());
            if len == 0 {
                continue;
            }
            let end = start + 1 + len;
            if text[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
            {
                continue;
            }
            let token = &tail[..len];
            if let Some(normalized) = normalize_tag(token)
                && let Some(canonical) = known.get(&normalized.to_lowercase())
            {
                record(canonical.clone());
            }
        }
    }

    fn walk_inlines(
        nodes: &[Inline],
        known: &BTreeMap<String, String>,
        record: &mut impl FnMut(String),
    ) {
        for node in nodes {
            match node {
                Inline::Text { text } => scan_text(text, known, record),
                Inline::Link { url, .. } => {
                    if let Some(raw) = url.strip_prefix("tag:")
                        && let Some(tag) = normalize_tag(raw)
                    {
                        let canonical = known.get(&tag.to_lowercase()).cloned().unwrap_or(tag);
                        record(canonical);
                    }
                }
                Inline::Strong { content }
                | Inline::Emphasis { content }
                | Inline::Strikethrough { content }
                | Inline::Underline { content }
                | Inline::Highlight { content, .. }
                | Inline::Superscript { content }
                | Inline::Subscript { content }
                | Inline::Small { content } => walk_inlines(content, known, record),
                _ => {}
            }
        }
    }

    fn walk_blocks(
        nodes: &[Block],
        known: &BTreeMap<String, String>,
        record: &mut impl FnMut(String),
    ) {
        for node in nodes {
            match node {
                Block::Paragraph { content } | Block::Heading { content, .. } => {
                    walk_inlines(content, known, record);
                }
                Block::List { items, .. } => {
                    for item in items {
                        walk_blocks(&item.children, known, record);
                    }
                }
                Block::Details {
                    summary, children, ..
                } => {
                    walk_inlines(summary, known, record);
                    walk_blocks(children, known, record);
                }
                Block::Table(table) => {
                    for cell in table.head.iter().chain(table.rows.iter().flatten()) {
                        walk_inlines(&cell.content, known, record);
                    }
                }
                // Blockquotes and CodeBlocks are intentionally excluded.
                _ => {}
            }
        }
    }

    let document = Parser::new().markdown(body);
    walk_blocks(&document.blocks, known, &mut record);
    for note in &document.footnotes {
        walk_blocks(&note.children, known, &mut record);
    }
    out
}

/// Extract the tags of a Card from its saved body (and title if tagged inline).
pub fn card_tags(card: &Card, known: &BTreeMap<String, String>) -> Vec<String> {
    let mut tags = body_tags(&card.body, known);
    if card.title.contains('#') || card.title.contains("tag:") {
        for tag in body_tags(&card.title, known) {
            if !tags
                .iter()
                .any(|t| t.eq_ignore_ascii_case(&tag) || t.to_lowercase() == tag.to_lowercase())
            {
                tags.push(tag);
            }
        }
    }
    tags
}

/// Format up to `max` tags as `" · #a #b"` (with `" +N"` overflow), or `""` when empty.
pub fn format_summary(tags: &[String], max: usize) -> String {
    if tags.is_empty() || max == 0 {
        return String::new();
    }
    let shown = tags
        .iter()
        .take(max)
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(" ");
    let extra = tags.len().saturating_sub(max);
    if extra > 0 {
        format!(" · {shown} +{extra}")
    } else {
        format!(" · {shown}")
    }
}

/// Split a search query into explicit `#tag` filter terms (excluding numeric `#42` Card numbers)
/// and the remaining free-text query string.
pub fn split_tag_query(query: &str) -> (Vec<String>, String) {
    let mut tag_terms = Vec::new();
    let mut other_terms = Vec::new();
    for term in query.split_whitespace() {
        if term.starts_with('#')
            && let Some(tag) = normalize_tag(term)
        {
            tag_terms.push(tag.to_lowercase());
        } else {
            other_terms.push(term);
        }
    }
    (tag_terms, other_terms.join(" "))
}

/// Score a Card's tags against explicit `#tag` query terms (lower is better; `0` is exact).
pub fn score_tags(card_tags: &[String], tag_terms: &[String]) -> Option<usize> {
    if tag_terms.is_empty() {
        return Some(0);
    }
    if card_tags.is_empty() {
        return None;
    }
    let lowered = card_tags
        .iter()
        .map(|t| t.to_lowercase())
        .collect::<Vec<_>>();
    let mut total = 0usize;
    for needle in tag_terms {
        let best = lowered
            .iter()
            .filter_map(|tag| {
                if tag == needle {
                    Some(0)
                } else if tag.starts_with(needle) {
                    Some(4 + tag.chars().count().saturating_sub(needle.chars().count()))
                } else {
                    tag.find(needle).map(|pos| 24 + pos)
                }
            })
            .min()?;
        total += best;
    }
    Some(total)
}

/// Count non-removed Cards matching `tag` (case-insensitive) across the workspace.
pub fn matching_card_count(
    snapshot: &Snapshot,
    known: &BTreeMap<String, String>,
    tag: &str,
) -> usize {
    let needle = tag
        .trim()
        .strip_prefix('#')
        .unwrap_or(tag.trim())
        .to_lowercase();
    if needle.is_empty() {
        return 0;
    }
    snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none())
        .filter(|c| {
            card_tags(c, known)
                .iter()
                .any(|t| t.to_lowercase() == needle)
        })
        .count()
}

/// Count non-removed Cards per lowercase tag across the workspace.
pub fn tag_card_counts(
    snapshot: &Snapshot,
    known: &BTreeMap<String, String>,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for card in snapshot.cards.iter().filter(|c| c.removed_at.is_none()) {
        for tag in card_tags(card, known) {
            *counts.entry(tag.to_lowercase()).or_insert(0) += 1;
        }
    }
    counts
}

/// Register a newly created tag locally (optimistic + Gallery) and persist on `project_id`
/// in the background without blocking ongoing Card/comment draft submission.
pub fn register(ui: Ui, project_id: &str, raw_tag: &str) -> Option<String> {
    let tag = normalize_tag(raw_tag)?;
    let lower = tag.to_lowercase();
    ui.local_tags.update(|tags| {
        if !tags.iter().any(|existing| existing.to_lowercase() == lower) {
            tags.insert(tag.clone());
        }
    });
    let pid = if project_id.is_empty() {
        ui.project.get_untracked()
    } else {
        project_id.to_owned()
    };
    if !pid.is_empty() {
        let already_in_project = ui.snapshot.with_untracked(|s| {
            s.projects
                .iter()
                .find(|p| p.id == pid)
                .is_some_and(|p| p.tags.iter().any(|t| t.to_lowercase() == lower))
        });
        if !already_in_project {
            if crate::APP.get().is_some() {
                let send_tag = tag.clone();
                ui.spawn(async move {
                    let result = blocking(move || {
                        crate::routed_call(
                            "tag.create",
                            json!({"project_id": pid, "tag": send_tag}),
                            None,
                        )?;
                        crate::merged_snapshot()
                    })
                    .await;
                    if let Ok(Ok(snapshot)) = result {
                        ui.snapshot.try_set(snapshot);
                    }
                });
            } else {
                // Gallery / standalone fixture mode: persist directly into the in-memory Project.
                ui.snapshot.update(|s| {
                    if let Some(p) = s.projects.iter_mut().find(|p| p.id == pid)
                        && !p.tags.iter().any(|t| t.to_lowercase() == lower)
                    {
                        p.tags.push(tag.clone());
                    }
                });
            }
        }
    }
    Some(tag)
}

/// Toggle a tag filter rule on the Board for `project_id`.
pub fn toggle_board_filter(ui: Ui, project_id: &str, raw_tag: &str) {
    let Some(tag) = normalize_tag(raw_tag) else {
        return;
    };
    let lower = tag.to_lowercase();
    let pid = if project_id.is_empty() {
        ui.project.get_untracked()
    } else {
        project_id.to_owned()
    };
    if pid.is_empty() {
        return;
    }
    ui.github.filters.update(|all| {
        let entry = all.entry(pid).or_default();
        if let Some(pos) = entry.tags.iter().position(|t| t.to_lowercase() == lower) {
            entry.tags.remove(pos);
        } else {
            entry.tags.push(tag);
        }
    });
}

/// Clear all active tag filters on the Board for `project_id`.
pub fn clear_board_tags(ui: Ui, project_id: &str) {
    let pid = if project_id.is_empty() {
        ui.project.get_untracked()
    } else {
        project_id.to_owned()
    };
    if pid.is_empty() {
        return;
    }
    ui.github.filters.update(|all| {
        if let Some(entry) = all.get_mut(&pid) {
            entry.tags.clear();
        }
    });
}

/// Active Board tag filters for `project_id`.
pub fn active_board_tags(ui: Ui, project_id: &str) -> Vec<String> {
    ui.github.filters.with(|all| {
        all.get(project_id)
            .map(|f| f.tags.clone())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Project, Workflow};

    fn fixture_snapshot() -> Snapshot {
        let mut s = Snapshot::default();
        s.projects.push(Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: vec!["bug".into(), "ui-kit".into(), "前端".into()],
            id: "p1".into(),
            name: "Canban".into(),
            directory: "/example".into(),
            location: None,
            workflow: Workflow::default(),
            target_branch: None,
            created_at: 1,
        });
        s
    }

    #[test]
    fn normalize_tag_accepts_letters_cjk_hyphens_and_rejects_pure_numbers_or_punctuation() {
        assert_eq!(normalize_tag("#bug"), Some("bug".into()));
        assert_eq!(normalize_tag("ui-kit_2"), Some("ui-kit_2".into()));
        assert_eq!(normalize_tag("#前端"), Some("前端".into()));
        assert_eq!(normalize_tag("18"), None);
        assert_eq!(normalize_tag("#42"), None);
        assert_eq!(normalize_tag(""), None);
        assert_eq!(normalize_tag("#"), None);
        assert_eq!(normalize_tag("bad space"), None);
        assert_eq!(normalize_tag("bad/slash"), None);
    }

    #[test]
    fn body_tags_extracts_known_prose_tags_and_explicit_links_while_ignoring_code_quotes_and_hex_colors()
     {
        let s = fixture_snapshot();
        let known = known_map(&s, &BTreeSet::new(), Some("p1"));
        let body = "\
## Heading #bug

Track #UI-KIT and #前端 alongside [custom](tag:search) and duplicate #bug.
Ignore CSS color #ff0000, Card #18, URL https://example.com/#bug, and email@#bug.

Inline `#前端` and fenced:
```rust
let t = \"#ui-kit\";
```

> Quoted #bug should not tag the Card
";
        let extracted = body_tags(body, &known);
        assert_eq!(extracted, vec!["bug", "ui-kit", "前端", "search"]);
    }
}
