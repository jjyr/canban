//! Local archive metadata projection. Search never opens compressed resources,
//! reads Terminals or changes the selected Project.
use canban_types::{Card, Snapshot};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub card: Card,
    pub agents: Vec<String>,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub entries: Vec<Entry>,
}
pub fn project_entries(snapshot: &Snapshot, project_id: &str, query: &str) -> Vec<Entry> {
    project_entries_with_tags(snapshot, &BTreeSet::new(), project_id, query)
}

pub fn project_entries_with_tags(
    snapshot: &Snapshot,
    local_tags: &BTreeSet<String>,
    project_id: &str,
    query: &str,
) -> Vec<Entry> {
    groups_for_project(
        snapshot,
        local_tags,
        (!project_id.is_empty()).then_some(project_id),
        query,
    )
    .into_iter()
    .flat_map(|g| g.entries)
    .collect()
}

pub fn groups(snapshot: &Snapshot, query: &str) -> Vec<Group> {
    groups_with_tags(snapshot, &BTreeSet::new(), query)
}

pub fn groups_with_tags(
    snapshot: &Snapshot,
    local_tags: &BTreeSet<String>,
    query: &str,
) -> Vec<Group> {
    groups_for_project(snapshot, local_tags, None, query)
}

fn groups_for_project(
    snapshot: &Snapshot,
    local_tags: &BTreeSet<String>,
    project_filter: Option<&str>,
    query: &str,
) -> Vec<Group> {
    let known_tags = crate::tags::known_map(snapshot, local_tags, project_filter);
    let (tag_terms, other_query) = crate::tags::split_tag_query(query);
    let project_names: std::collections::HashMap<&str, &str> = snapshot
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    let mut conv_by_card: std::collections::HashMap<&str, Vec<(&str, &str)>> =
        std::collections::HashMap::new();
    for c in &snapshot.conversations {
        conv_by_card
            .entry(c.card_id.as_str())
            .or_default()
            .push((c.profile_id.as_str(), c.provider.as_str()));
    }
    let mut term_by_card: std::collections::HashMap<&str, Vec<(&str, String)>> =
        std::collections::HashMap::new();
    for a in snapshot.terminals.iter().filter_map(|t| t.agent.as_ref()) {
        term_by_card
            .entry(a.card_id.as_str())
            .or_default()
            .push((a.profile.id.as_str(), a.profile.command_name()));
    }
    let mut groups = BTreeMap::<String, Vec<(usize, Entry)>>::new();
    for card in snapshot
        .cards
        .iter()
        .filter(|c| {
            c.archived_at.is_some()
                && c.removed_at.is_none()
                && project_filter.is_none_or(|pid| c.project_id == pid)
        })
        .cloned()
    {
        let name = project_names
            .get(card.project_id.as_str())
            .copied()
            .unwrap_or("Unknown Project");
        let mut agents = BTreeSet::new();
        let mut providers = BTreeSet::new();
        if let Some(convs) = conv_by_card.get(card.id.as_str()) {
            for (profile_id, provider) in convs {
                agents.insert(format!("@{profile_id}"));
                providers.insert((*provider).to_owned());
            }
        }
        if let Some(terms) = term_by_card.get(card.id.as_str()) {
            for (profile_id, cmd) in terms {
                agents.insert(format!("@{profile_id}"));
                providers.insert(cmd.clone());
            }
        }
        let agents = agents.into_iter().collect::<Vec<_>>();
        let tags = crate::tags::card_tags(&card, &known_tags);
        let mut fields = vec![
            card.title.clone(),
            format!("#{}", card.number),
            name.into(),
            agents.join(" "),
            providers.into_iter().collect::<Vec<_>>().join(" "),
        ];
        fields.extend(crate::quick_open::github_search_fields(&card));
        for tag in &tags {
            fields.push(format!("#{tag}"));
            fields.push(tag.clone());
        }
        let score = if tag_terms.is_empty() {
            let Some(score) = canban_ui_kit::search::score(&fields, query) else {
                continue;
            };
            score
        } else {
            let Some(tag_score) = crate::tags::score_tags(&tags, &tag_terms) else {
                continue;
            };
            let other_score = if other_query.is_empty() {
                0
            } else {
                let Some(s) = canban_ui_kit::search::score(&fields, &other_query) else {
                    continue;
                };
                s
            };
            tag_score + other_score
        };
        groups
            .entry(card.project_id.clone())
            .or_default()
            .push((score, Entry { card, agents, tags }));
    }
    let mut result = groups
        .into_iter()
        .map(|(id, mut entries)| {
            entries.sort_by(|(a, x), (b, y)| {
                a.cmp(b)
                    .then(y.card.archived_at.cmp(&x.card.archived_at))
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
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Default::default(),
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
        let result = groups(&s, "");
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
        assert_eq!(groups(&s, "Tools")[0].entries[0].card.id, "three");
    }
    #[test]
    fn fuzzy_terms_numbers_unicode_and_agent_metadata_filter_only_matching_children() {
        let mut s = fixture();
        let profile = Config::default().profiles[0].clone();
        let a = canban_scheduler::agent_run(
            "b",
            "three",
            1,
            "review",
            "stage_entry",
            profile,
            "Review".into(),
            None,
        );
        s.terminals.push(canban_types::TerminalEntry {
            id: "three:developer".into(),
            conversation_id: Some("three:developer".into()),
            number: 1,
            name: None,
            origin_project_id: None,
            origin_card_id: Some("three".into()),
            agent_engine: None,
            runtime: None,
            agent: Some(a),
            created_at: 1,
        });
        assert_eq!(groups(&s, "cnBn impr srch")[0].entries[0].card.id, "one");
        assert_eq!(groups(&s, "原终恢复 #42")[0].entries[0].card.id, "two");
        assert_eq!(groups(&s, "codex resume")[0].entries[0].card.id, "three");
        assert!(groups(&s, "not-found").is_empty());
    }
    #[test]
    fn removed_cards_stay_out_of_archive_even_with_an_archive_timestamp() {
        let mut s = fixture();
        let removed = s.cards.last_mut().unwrap();
        removed.archived_at = Some(50);
        assert_eq!(groups(&s, "Deleted").len(), 0);
        assert!(!groups(&s, "").iter().any(|g| g.id == "empty"));
        assert_eq!(
            project_entries(&s, "a", "")
                .iter()
                .map(|e| e.card.id.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "one"]
        );
        assert_eq!(project_entries(&s, "b", "").len(), 1);
        assert!(project_entries(&s, "empty", "").is_empty());
    }

    #[test]
    fn archive_search_matches_github_pr_and_issue_numbers() {
        let mut s = fixture();
        s.cards[0].content = canban_types::CardContent::GitHubPr {
            source: canban_types::GitHubIssueRef {
                repository: "jjyr/canban".into(),
                repository_id: 1,
                issue_id: 777,
                number: 777,
                url: "https://github.com/jjyr/canban/pull/777".into(),
            },
        };
        assert_eq!(groups(&s, "pr 777")[0].entries[0].card.id, "one");
        assert_eq!(groups(&s, "pr#777")[0].entries[0].card.id, "one");
    }
}
