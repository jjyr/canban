//! Local archive metadata projection. Search never opens compressed resources,
//! reads Terminals or changes the selected Project.
use canban_types::{Card, Snapshot};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub card: Card,
    pub agents: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Archived,
    Deleted,
}

pub fn groups(snapshot: &Snapshot, removed: &[Card], query: &str, kind: Kind) -> Vec<Group> {
    let mut source = snapshot
        .cards
        .iter()
        .map(|c| (c.id.clone(), c.clone()))
        .collect::<BTreeMap<_, _>>();
    for card in removed {
        source
            .entry(card.id.clone())
            .or_insert_with(|| card.clone());
    }
    let mut groups = BTreeMap::<String, Vec<(usize, Entry)>>::new();
    for card in source.into_values().filter(|c| match kind {
        Kind::Archived => c.archived_at.is_some() && c.removed_at.is_none(),
        Kind::Deleted => c.removed_at.is_some(),
    }) {
        let name = snapshot
            .projects
            .iter()
            .find(|p| p.id == card.project_id)
            .map(|p| p.name.as_str())
            .unwrap_or("Unknown Project");
        let mut agents = BTreeSet::new();
        let mut providers = BTreeSet::new();
        for c in snapshot
            .conversations
            .iter()
            .filter(|c| c.card_id == card.id)
        {
            agents.insert(format!("@{}", c.profile_id));
            providers.insert(c.provider.clone());
        }
        for r in snapshot.requests.iter().filter(|r| r.card_id == card.id) {
            agents.insert(format!("@{}", r.profile.id));
            providers.insert(r.profile.command_name());
        }
        let agents = agents.into_iter().collect::<Vec<_>>();
        let fields = vec![
            card.title.clone(),
            format!("#{}", card.number),
            name.into(),
            agents.join(" "),
            providers.into_iter().collect::<Vec<_>>().join(" "),
        ];
        let Some(score) = canban_ui_kit::search::score(&fields, query) else {
            continue;
        };
        groups
            .entry(card.project_id.clone())
            .or_default()
            .push((score, Entry { card, agents }));
    }
    let mut result = groups
        .into_iter()
        .map(|(id, mut entries)| {
            entries.sort_by(|(a, x), (b, y)| {
                a.cmp(b)
                    .then_with(|| {
                        let time = |e: &Entry| {
                            if kind == Kind::Deleted {
                                e.card.removed_at
                            } else {
                                e.card.archived_at
                            }
                        };
                        time(y).cmp(&time(x))
                    })
                    .then(x.card.number.cmp(&y.card.number))
                    .then(x.card.id.cmp(&y.card.id))
            });
            let score = entries.first().map(|e| e.0).unwrap_or(0);
            let name = snapshot
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Unknown Project".into());
            (
                score,
                Group {
                    id,
                    name,
                    entries: entries.into_iter().map(|e| e.1).collect(),
                },
            )
        })
        .collect::<Vec<_>>();
    result.sort_by(|(a, x), (b, y)| {
        a.cmp(b)
            .then(x.name.to_lowercase().cmp(&y.name.to_lowercase()))
            .then(x.id.cmp(&y.id))
    });
    result.into_iter().map(|(_, g)| g).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Config, Project, Workflow};
    use serde_json::json;
    fn fixture() -> Snapshot {
        let mut s = Snapshot::default();
        for (id, name) in [("a", "Canban"), ("b", "Tools"), ("empty", "Empty Project")] {
            s.projects.push(Project {
                id: id.into(),
                name: name.into(),
                directory: "/example".into(),
                workflow: Workflow::default(),
                github: None,
                delivery_mode: Default::default(),
                location: None,
                target_branch: None,
                created_at: 1,
            });
        }
        for (id, project, title, archived, removed) in [
            ("one", "a", "Improve search", Some(10), None),
            ("two", "a", "原生终端恢复", Some(20), None),
            ("three", "b", "Fix resume", Some(30), None),
            ("active", "empty", "Active", None, None),
            ("deleted", "empty", "Deleted", None, Some(40)),
        ] {
            s.cards.push(serde_json::from_value(json!({"id":id,"project_id":project,"number":if id=="two"{42}else{1},"title":title,"body":"","stage":"review","revision":1,"archived_at":archived,"removed_at":removed,"created_at":1})).unwrap());
        }
        s
    }
    #[test]
    fn archive_is_cross_project_nested_and_hides_active_deleted_and_empty_projects() {
        let s = fixture();
        let result = groups(&s, &[], "", Kind::Archived);
        assert_eq!(
            result.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            result[0]
                .entries
                .iter()
                .map(|e| e.card.id.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "one"]
        );
        assert_eq!(
            groups(&s, &[], "Tools", Kind::Archived)[0].entries[0]
                .card
                .id,
            "three"
        );
    }
    #[test]
    fn fuzzy_terms_numbers_unicode_and_agent_metadata_filter_only_matching_children() {
        let mut s = fixture();
        let profile = Config::default().profiles[0].clone();
        let mut r = canban_scheduler::request(
            "b",
            "three",
            "review",
            "stage_entry",
            profile,
            "Review".into(),
            None,
        );
        r.id = "request".into();
        s.requests.push(r);
        assert_eq!(
            groups(&s, &[], "cnBn impr srch", Kind::Archived)[0].entries[0]
                .card
                .id,
            "one"
        );
        assert_eq!(
            groups(&s, &[], "原终恢复 #42", Kind::Archived)[0].entries[0]
                .card
                .id,
            "two"
        );
        assert_eq!(
            groups(&s, &[], "codex resume", Kind::Archived)[0].entries[0]
                .card
                .id,
            "three"
        );
        assert!(groups(&s, &[], "not-found", Kind::Archived).is_empty());
    }
    #[test]
    fn stale_deleted_results_cannot_resurrect_a_restored_card() {
        let mut s = fixture();
        let removed = s.cards.pop().unwrap();
        assert_eq!(
            groups(&s, std::slice::from_ref(&removed), "", Kind::Deleted).len(),
            1
        );
        let mut restored = removed.clone();
        restored.removed_at = None;
        s.cards.push(restored);
        assert!(groups(&s, &[removed], "", Kind::Deleted).is_empty());
    }
}
