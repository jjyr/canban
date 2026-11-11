use super::*;
use canban_types::{Card, Snapshot};

/// A read-only index of saved content. Drafts never mutate Card identity or GitHub.
pub fn related(card: &Card, snapshot: &Snapshot) -> Vec<GitHubReference> {
    let mut refs = card.github_links.clone(); // Keep legacy explicit references visible.
    let mut bodies = vec![card.body.as_str()];
    if let Some(source) = card.github_issue()
        && let Some(issue) = snapshot
            .github_issues
            .iter()
            .find(|i| i.project_id == card.project_id && i.source == *source)
    {
        bodies.push(&issue.body);
    }
    for event in snapshot.events.iter().filter(|e| {
        e.card_id == card.id && matches!(e.kind.as_str(), "comment.created" | "comment.replied")
    }) {
        bodies.push(event.data["body"].as_str().unwrap_or(&event.message));
    }
    for body in bodies {
        refs.extend(
            crate::links::content_links(body)
                .into_iter()
                .filter_map(|url| canban_github::parse_reference(&url).ok()),
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    refs.retain(|r| {
        let own = card.github_issue().is_some_and(|s| {
            s.repository.eq_ignore_ascii_case(&r.repository) && r.number == Some(s.number)
        });
        let delivery = card.pull_request.as_ref().is_some_and(|p| {
            p.repository.eq_ignore_ascii_case(&r.repository) && r.number == Some(p.number)
        });
        !own && !delivery
            && seen.insert((
                r.repository.to_lowercase(),
                r.kind.clone(),
                r.number,
                r.sha.clone(),
            ))
    });
    refs
}
pub fn panel(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(c) = ui.current_card() else {
            return view().into_element();
        };
        let refs = ui.snapshot.with(|s| related(&c, s));
        let mut area = view().gap_1();
        if !refs.is_empty() {
            let mut row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(muted("References"));
            for r in refs {
                let kind = match r.kind.as_str() {
                    "pr" => "PR",
                    "issue" => "Issue",
                    _ => "Commit",
                };
                let title = format!(
                    "{kind} {}{}",
                    r.repository,
                    r.number
                        .map(|n| format!("#{n}"))
                        .unwrap_or_else(|| format!("@{}", r.sha.clone().unwrap_or_default()))
                );
                row = row.child(compact_action(title, Variant::Quiet, move |_| {
                    crate::links::open(ui, &r.url)
                }));
            }
            area = area.child(row);
        }
        if let Some(pr) = c.pull_request.clone().filter(|p| {
            p.number > 0
                && !c.github_issue().is_some_and(|s| {
                    s.repository.eq_ignore_ascii_case(&p.repository) && s.number == p.number
                })
        }) {
            area = area.child(compact_action(
                format!(
                    "Delivery PR {}#{} · {}",
                    pr.repository,
                    pr.number,
                    if pr.merged { "merged" } else { &pr.state }
                ),
                Variant::Quiet,
                move |_| crate::links::open(ui, &pr.url),
            ));
        }
        area.into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_content_index_deduplicates_excludes_source_delivery_and_code() {
        let mut card: Card = serde_json::from_value(json!({"id":"c", "project_id":"p", "number":1, "title":"Task", "body":"[Issue](https://github.com/a/b/issues/42) https://github.com/a/b/issues/42 `https://github.com/a/b/issues/99`\n\n```\nhttps://github.com/a/b/pull/98\n```", "stage":"backlog", "revision":1, "created_at":1})).unwrap();
        let mut snapshot = Snapshot::default();
        assert_eq!(related(&card, &snapshot).len(), 1);
        snapshot.events.push(serde_json::from_value(json!({"id":"e", "sequence":1, "project_id":"p", "card_id":"c", "kind":"comment.created", "actor":"user", "message":"https://github.com/a/b/pull/43", "data":{}, "created_at":1})).unwrap());
        assert_eq!(related(&card, &snapshot).len(), 2);
        card.body.clear();
        assert_eq!(related(&card, &snapshot)[0].number, Some(43));
        card.content = canban_types::CardContent::GitHubPr {
            source: canban_types::GitHubIssueRef {
                repository: "a/b".into(),
                repository_id: 1,
                issue_id: 43,
                number: 43,
                url: "https://github.com/a/b/pull/43".into(),
            },
        };
        assert!(related(&card, &snapshot).is_empty());
        snapshot.events[0].card_id = "another-card".into();
        card.content = canban_types::CardContent::Local;
        assert!(related(&card, &snapshot).is_empty());
    }
}
