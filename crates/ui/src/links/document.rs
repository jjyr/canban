//! Decorate parsed inline nodes; code and link labels are never reparsed as prose.
use super::NODE;
use canban_types::{Profile, Snapshot};
use nsh_base::text_view::{Block, CustomNode, Document, Inline};

fn reference(href: &str, text: String, content: Vec<Inline>) -> Inline {
    Inline::Custom(
        CustomNode::new(NODE)
            .with_attribute("href", href)
            .with_text(text)
            .with_content(content),
    )
}
fn plain(content: &[Inline]) -> String {
    content.iter().map(Inline::plain_text).collect()
}
fn token_link(
    token: &str,
    trigger: char,
    s: &Snapshot,
    profiles: &[Profile],
    project: &str,
) -> Option<String> {
    if trigger == '@' {
        return profiles
            .iter()
            .find(|p| p.id == token)
            .map(|p| format!("profile:{}", p.id));
    }
    let number = token.parse::<u64>().ok()?;
    // Bare hashes in a GitHub-connected project are ambiguous. Completion writes
    // stable explicit links; never guess Issue vs PR vs local Card here.
    if s.projects
        .iter()
        .any(|p| p.id == project && p.github.is_some())
    {
        return None;
    }
    s.cards
        .iter()
        .find(|c| c.project_id == project && c.number == number && c.removed_at.is_none())
        .map(|c| format!("card:{}", c.id))
}
fn text_nodes(text: &str, s: &Snapshot, profiles: &[Profile], project: &str) -> Vec<Inline> {
    let mut nodes = Vec::new();
    let mut last = 0;
    for (start, trigger) in text.char_indices().filter(|(_, c)| matches!(c, '@' | '#')) {
        if start < last
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | '[' | '{'))
        {
            continue;
        }
        let tail = &text[start + 1..];
        let len = tail
            .char_indices()
            .find(|(_, c)| {
                if trigger == '@' {
                    !c.is_alphanumeric() && *c != '_' && *c != '-'
                } else {
                    !c.is_ascii_digit()
                }
            })
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
        let Some(href) = token_link(&tail[..len], trigger, s, profiles, project) else {
            continue;
        };
        if start > last {
            nodes.push(Inline::text(&text[last..start]));
        }
        let label = text[start..end].to_owned();
        nodes.push(reference(&href, label.clone(), vec![Inline::text(label)]));
        last = end;
    }
    if last < text.len() {
        nodes.push(Inline::text(&text[last..]));
    }
    nodes
}
fn inlines(
    nodes: &mut Vec<Inline>,
    s: &Snapshot,
    profiles: &[Profile],
    project: &str,
    prose: bool,
) {
    let mut out = Vec::with_capacity(nodes.len());
    for mut node in std::mem::take(nodes) {
        match &mut node {
            Inline::Text { text } if prose => {
                out.extend(text_nodes(text, s, profiles, project));
                continue;
            }
            Inline::Link { url, content, .. }
                if !content.iter().any(|c| matches!(c, Inline::Image(_))) =>
            {
                out.push(reference(url, plain(content), content.clone()));
                continue;
            }
            Inline::Strong { content }
            | Inline::Emphasis { content }
            | Inline::Strikethrough { content }
            | Inline::Underline { content }
            | Inline::Highlight { content, .. }
            | Inline::Superscript { content }
            | Inline::Subscript { content }
            | Inline::Small { content } => inlines(content, s, profiles, project, prose),
            _ => {}
        }
        out.push(node);
    }
    *nodes = out;
}
fn blocks(nodes: &mut [Block], s: &Snapshot, profiles: &[Profile], project: &str, prose: bool) {
    for node in nodes {
        match node {
            Block::Paragraph { content } | Block::Heading { content, .. } => {
                inlines(content, s, profiles, project, prose)
            }
            Block::Blockquote { children, .. } => blocks(children, s, profiles, project, false),
            Block::List { items, .. } => {
                for item in items {
                    blocks(&mut item.children, s, profiles, project, prose);
                }
            }
            Block::Details {
                summary, children, ..
            } => {
                inlines(summary, s, profiles, project, prose);
                blocks(children, s, profiles, project, prose);
            }
            Block::Table(table) => {
                for cell in table.head.iter_mut().chain(table.rows.iter_mut().flatten()) {
                    inlines(&mut cell.content, s, profiles, project, prose);
                }
            }
            _ => {}
        }
    }
}
pub(super) fn decorate(document: &mut Document, s: &Snapshot, profiles: &[Profile], project: &str) {
    blocks(&mut document.blocks, s, profiles, project, true);
    for note in &mut document.footnotes {
        blocks(&mut note.children, s, profiles, project, true);
    }
}

/// Extract explicit Markdown destinations and pasted URLs, excluding code/images.
pub(crate) fn content_links(source: &str) -> Vec<String> {
    fn inline(nodes: &[Inline], out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Inline::Link { url, .. } => out.push(url.clone()),
                Inline::Text { text } => {
                    for word in text.split_whitespace() {
                        let word = word.trim_matches(|c: char| {
                            matches!(c, '(' | ')' | '<' | '>' | ',' | '.' | ';' | '!' | '"')
                        });
                        if word.starts_with("https://github.com/") {
                            out.push(word.into());
                        }
                    }
                }
                Inline::Strong { content }
                | Inline::Emphasis { content }
                | Inline::Strikethrough { content }
                | Inline::Underline { content }
                | Inline::Highlight { content, .. }
                | Inline::Superscript { content }
                | Inline::Subscript { content }
                | Inline::Small { content } => inline(content, out),
                _ => {}
            }
        }
    }
    fn walk(nodes: &[Block], out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Block::Paragraph { content } | Block::Heading { content, .. } => {
                    inline(content, out)
                }
                Block::Blockquote { children, .. } => walk(children, out),
                Block::List { items, .. } => {
                    for item in items {
                        walk(&item.children, out);
                    }
                }
                Block::Details {
                    summary, children, ..
                } => {
                    inline(summary, out);
                    walk(children, out);
                }
                Block::Table(table) => {
                    for cell in table.head.iter().chain(table.rows.iter().flatten()) {
                        inline(&cell.content, out);
                    }
                }
                _ => {}
            }
        }
    }
    let document = nsh_base::text_view::Parser::new().markdown(source);
    let mut out = Vec::new();
    walk(&document.blocks, &mut out);
    for note in &document.footnotes {
        walk(&note.children, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_base::text_view::Parser;
    #[test]
    fn markdown_survives_decoration_without_turning_code_email_or_link_labels_into_mentions() {
        let source = "## Heading\n\n**Bold** and @developer; mail@example.com and `@developer #12`.\n\n[External **label**](https://example.com)\n\n```rust\nlet x = \"@developer #12\";\n```\n\n> @developer is quoted\n\n- [ ] Task";
        let mut doc = Parser::new().markdown(source);
        let before = doc.plain_text();
        decorate(
            &mut doc,
            &Snapshot::default(),
            &canban_types::Config::default().profiles,
            "p",
        );
        assert_eq!(doc.plain_text(), before);
        assert!(matches!(doc.blocks[0], Block::Heading { .. }));
        let serialized = serde_json::to_value(&doc).unwrap();
        let data = serialized.to_string();
        assert_eq!(data.matches("profile:developer").count(), 1);
        assert!(data.contains("https://example.com"));
        assert!(data.contains("codeBlock"));
        assert!(data.contains("mail@example.com"));
    }
}
