//! Shared derived `Snapshot` indexes so Board tiles, Card timelines, and Markdown readers
//! borrow pre-indexed `O(log N)` maps instead of cloning or scanning the full `Snapshot`.
use canban_types::{
    Card, Event, GitHubIssueSnapshot, Project, ReviewDraft, Snapshot, TerminalEntry, WorkflowStep,
    normalize_tag,
};
use canban_ui_kit::Tone;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_INDEX_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub struct CardStatusDigest {
    pub is_merging: bool,
    pub specs_normal: Vec<(String, Tone, bool)>,
    pub specs_ui_merging: Vec<(String, Tone, bool)>,
    pub running: Vec<(TerminalEntry, Option<u64>)>,
}

#[derive(Debug, Default)]
pub struct SnapshotIndex {
    pub generation: u64,
    pub projects: BTreeMap<String, Project>,
    pub project_order: Vec<Project>,
    pub project_has_deliver: BTreeSet<String>,
    pub cards: BTreeMap<String, Card>,
    /// Positions into the matching Snapshot; no duplicated event bodies.
    pub events_by_card: BTreeMap<String, Vec<usize>>,
    pub last_event_by_card: BTreeMap<String, u64>,
    pub reply_counts: BTreeMap<String, usize>,
    pub artifact_counts: BTreeMap<String, usize>,
    pub github_issues: BTreeMap<(String, u64, u64), GitHubIssueSnapshot>,
    pub project_has_github_cards: BTreeSet<String>,
    pub explicit_body_tags: Vec<String>,
    pub card_status: BTreeMap<String, CardStatusDigest>,
    pub review_drafts: BTreeMap<String, ReviewDraft>,
}

impl SnapshotIndex {
    pub fn build(snapshot: &Snapshot) -> Self {
        let mut span = canban_profiler::Span::new("index.build");
        span.counts(snapshot.events.len(), 0);
        let mut projects = BTreeMap::new();
        for p in &snapshot.projects {
            projects.insert(p.id.clone(), p.clone());
        }
        let project_order = snapshot.projects.clone();

        let mut project_has_deliver = BTreeSet::new();
        for rev in &snapshot.workflow_revisions {
            if rev
                .definition
                .steps
                .values()
                .any(|st| matches!(st, WorkflowStep::Deliver { .. }))
            {
                project_has_deliver.insert(rev.project_id.clone());
            }
        }

        let mut cards = BTreeMap::new();
        let mut project_has_github_cards = BTreeSet::new();
        let mut explicit_body_tags = Vec::new();
        for c in &snapshot.cards {
            cards.insert(c.id.clone(), c.clone());
            if c.github_issue().is_some() {
                project_has_github_cards.insert(c.project_id.clone());
            }
            if c.removed_at.is_none() {
                explicit_body_tags.extend(crate::tags::explicit_link_tags(&c.body));
            }
        }

        let mut events_by_card: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut last_event_by_card: BTreeMap<String, u64> = BTreeMap::new();
        for (position, ev) in snapshot.events.iter().enumerate() {
            events_by_card
                .entry(ev.card_id.clone())
                .or_default()
                .push(position);
            let last = last_event_by_card.entry(ev.card_id.clone()).or_default();
            *last = (*last).max(ev.sequence);
        }
        let mut reply_counts = BTreeMap::new();
        for (card_id, positions) in &events_by_card {
            let count = crate::card::timeline::reply_count_iter(
                positions.iter().map(|&p| &snapshot.events[p]),
                card_id,
            );
            if count > 0 {
                reply_counts.insert(card_id.clone(), count);
            }
        }

        let mut artifact_counts: BTreeMap<String, usize> = BTreeMap::new();
        for art in &snapshot.artifacts {
            *artifact_counts.entry(art.card_id.clone()).or_default() += 1;
        }

        let mut github_issues = BTreeMap::new();
        for issue in &snapshot.github_issues {
            github_issues.insert(
                (
                    issue.project_id.clone(),
                    issue.source.repository_id,
                    issue.source.issue_id,
                ),
                issue.clone(),
            );
        }

        let mut terminals_by_card: BTreeMap<String, Vec<&TerminalEntry>> = BTreeMap::new();
        for t in &snapshot.terminals {
            if let Some(agent) = &t.agent {
                terminals_by_card
                    .entry(agent.card_id.clone())
                    .or_default()
                    .push(t);
            }
        }

        // Snapshot::merge_queue_status constructs a full Card map. Compute queues
        // once per Project, never once (or three times) per Card in this index.
        let mut queue_status = BTreeMap::new();
        for pid in snapshot
            .cards
            .iter()
            .map(|c| c.project_id.as_str())
            .collect::<BTreeSet<_>>()
        {
            let queue = snapshot.merge_queue(pid);
            if let Some(&head) = queue.first() {
                for (i, run) in queue.iter().enumerate() {
                    queue_status
                        .entry(run.card_id.as_str())
                        .or_insert((i + 1, queue.len(), head));
                }
            }
        }
        let mut card_status = BTreeMap::new();
        for c in snapshot.cards.iter().filter(|c| c.active()) {
            let queue = queue_status.get(c.id.as_str()).copied();
            let is_merging = crate::components::activity::is_merging_indexed(snapshot, c, queue);
            let specs_normal =
                crate::components::activity::card_status_specs_indexed(snapshot, c, false, queue);
            let specs_ui_merging = if c.resolved() || is_merging {
                specs_normal.clone()
            } else {
                crate::components::activity::card_status_specs_indexed(snapshot, c, true, queue)
            };
            let running = running_agents_indexed(
                terminals_by_card
                    .get(&c.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &events_by_card
                    .get(&c.id)
                    .into_iter()
                    .flatten()
                    .map(|&p| &snapshot.events[p])
                    .collect::<Vec<_>>(),
            );
            card_status.insert(
                c.id.clone(),
                CardStatusDigest {
                    is_merging,
                    specs_normal,
                    specs_ui_merging,
                    running,
                },
            );
        }

        let mut review_drafts = BTreeMap::new();
        for draft in &snapshot.review_drafts {
            review_drafts.insert(draft.card_id.clone(), draft.clone());
        }

        Self {
            generation: NEXT_INDEX_GENERATION.fetch_add(1, Ordering::Relaxed),
            projects,
            project_order,
            project_has_deliver,
            cards,
            events_by_card,
            last_event_by_card,
            reply_counts,
            artifact_counts,
            github_issues,
            project_has_github_cards,
            explicit_body_tags,
            card_status,
            review_drafts,
        }
    }

    pub fn review_draft(&self, card_id: &str) -> Option<&ReviewDraft> {
        self.review_drafts.get(card_id)
    }

    pub fn reply_count(&self, card_id: &str) -> usize {
        self.reply_counts.get(card_id).copied().unwrap_or(0)
    }

    /// Materialize only the selected Card's history, paired with this index's source.
    pub fn card_events(&self, snapshot: &Snapshot, card_id: &str) -> Rc<Vec<Event>> {
        Rc::new(
            self.events_by_card
                .get(card_id)
                .into_iter()
                .flatten()
                .map(|&p| snapshot.events[p].clone())
                .collect(),
        )
    }

    pub fn github_issue(
        &self,
        project_id: &str,
        repository_id: u64,
        issue_id: u64,
    ) -> Option<&GitHubIssueSnapshot> {
        self.github_issues
            .get(&(project_id.to_owned(), repository_id, issue_id))
    }
}

impl PartialEq for SnapshotIndex {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}
impl Eq for SnapshotIndex {}

fn agent_started_at_from_events(entry: &TerminalEntry, card_events: &[&Event]) -> Option<u64> {
    let agent = entry.agent.as_ref()?;
    if let Some(ts) = agent.started_at.filter(|&t| t > 1_000_000_000_000) {
        return Some(ts);
    }
    if let Some(ev) = card_events.iter().rev().find(|e| {
        matches!(e.kind.as_str(), "execution.started" | "execution.continued")
            && e.data["run_number"].as_u64() == Some(u64::from(agent.number))
    }) && ev.created_at > 1_000_000_000_000
    {
        return Some(ev.created_at);
    }
    if agent.updated_at > 1_000_000_000_000 {
        Some(agent.updated_at)
    } else if entry.created_at > 1_000_000_000_000 {
        Some(entry.created_at)
    } else {
        None
    }
}

fn running_agents_indexed(
    card_terminals: &[&TerminalEntry],
    card_events: &[&Event],
) -> Vec<(TerminalEntry, Option<u64>)> {
    let mut profiles = BTreeSet::new();
    let mut running: Vec<&TerminalEntry> = card_terminals
        .iter()
        .copied()
        .filter(|t| {
            matches!(
                t.herdr_status(),
                canban_types::HerdrStatus::Working
                    | canban_types::HerdrStatus::Starting
                    | canban_types::HerdrStatus::Blocked
            )
        })
        .collect();
    running.sort_by_key(|t| std::cmp::Reverse(t.agent.as_ref().map(|a| a.updated_at).unwrap_or(0)));
    running
        .into_iter()
        .filter(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| profiles.insert(a.profile.id.clone()))
        })
        .map(|t| (t.clone(), agent_started_at_from_events(t, card_events)))
        .collect()
}

/// Compact reference-resolution lookup for Markdown inline decoration (`#number`, GitHub ambiguity).
/// Implements `PartialEq` so unrelated runtime/event updates never re-decorate open Markdown views.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkLookup {
    pub project_has_github: BTreeSet<String>,
    pub card_id_by_number: BTreeMap<(String, u64), String>,
}

impl LinkLookup {
    pub fn build(index: &SnapshotIndex) -> Self {
        let mut project_has_github = BTreeSet::new();
        for p in &index.project_order {
            if p.github.is_some() {
                project_has_github.insert(p.id.clone());
            }
        }
        let mut card_id_by_number = BTreeMap::new();
        for c in index.cards.values().filter(|c| c.removed_at.is_none()) {
            card_id_by_number
                .entry((c.project_id.clone(), c.number))
                .or_insert_with(|| c.id.clone());
        }
        Self {
            project_has_github,
            card_id_by_number,
        }
    }

    pub fn resolve_card_number(&self, project: &str, number: u64) -> Option<&str> {
        if self.project_has_github.contains(project) {
            return None;
        }
        self.card_id_by_number
            .get(&(project.to_owned(), number))
            .map(String::as_str)
    }
}

/// Precomputed `known_map` per Project and derived tags per non-removed Card.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct KnownTagsIndex {
    pub global: Rc<BTreeMap<String, String>>,
    pub by_project: BTreeMap<String, Rc<BTreeMap<String, String>>>,
    pub card_tags: BTreeMap<String, Vec<String>>,
}

impl KnownTagsIndex {
    pub fn build(index: &SnapshotIndex, local_tags: &BTreeSet<String>) -> Self {
        let build_for = |target_project: Option<&str>| -> Rc<BTreeMap<String, String>> {
            let mut out = BTreeMap::new();
            let mut insert = |raw: &str| {
                if let Some(tag) = normalize_tag(raw) {
                    out.entry(tag.to_lowercase()).or_insert(tag);
                }
            };
            if let Some(pid) = target_project
                && let Some(project) = index.projects.get(pid)
            {
                for tag in &project.tags {
                    insert(tag);
                }
            }
            for project in &index.project_order {
                if target_project != Some(project.id.as_str()) {
                    for tag in &project.tags {
                        insert(tag);
                    }
                }
            }
            for tag in local_tags {
                insert(tag);
            }
            for tag in &index.explicit_body_tags {
                insert(tag);
            }
            Rc::new(out)
        };

        let global = build_for(None);
        let mut by_project = BTreeMap::new();
        for p in &index.project_order {
            by_project.insert(p.id.clone(), build_for(Some(&p.id)));
        }

        let mut card_tags = BTreeMap::new();
        for c in index.cards.values().filter(|c| c.removed_at.is_none()) {
            let known = by_project.get(&c.project_id).unwrap_or(&global);
            let tags = crate::tags::card_tags(c, known);
            if !tags.is_empty() {
                card_tags.insert(c.id.clone(), tags);
            }
        }

        Self {
            global,
            by_project,
            card_tags,
        }
    }

    pub fn for_project(&self, target_project: Option<&str>) -> Rc<BTreeMap<String, String>> {
        target_project
            .and_then(|pid| self.by_project.get(pid))
            .cloned()
            .unwrap_or_else(|| self.global.clone())
    }

    pub fn tags_for_card(&self, card_id: &str) -> Vec<String> {
        self.card_tags.get(card_id).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::Workflow;
    use serde_json::json;

    fn make_project(id: &str, name: &str, tags: Vec<&str>) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            directory: format!("/tmp/{id}"),
            location: None,
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::new(),
            tags: tags.into_iter().map(str::to_owned).collect(),
            workflow: Workflow::default(),
            target_branch: Some("main".into()),
            created_at: 1,
        }
    }

    fn make_card(id: &str, project_id: &str, number: u64, title: &str, body: &str) -> Card {
        serde_json::from_value(json!({
            "id": id,
            "project_id": project_id,
            "number": number,
            "title": title,
            "body": body,
            "stage": "develop",
            "revision": 1,
            "created_at": number
        }))
        .unwrap()
    }

    fn make_event(seq: u64, card_id: &str, kind: &str, message: &str) -> Event {
        Event {
            id: format!("ev-{seq}"),
            sequence: seq,
            project_id: "p1".into(),
            card_id: card_id.into(),
            kind: kind.into(),
            actor: "agent:agy".into(),
            message: message.into(),
            data: json!({}),
            created_at: seq,
        }
    }

    #[test]
    fn queue_index_preserves_fifo_hold_and_pause_status_across_projects() {
        let mut s = Snapshot::default();
        for pid in ["p1", "p2"] {
            s.projects.push(make_project(pid, pid, vec![]));
            for n in 1..=3 {
                let id = format!("{pid}-{n}");
                let mut c = make_card(&id, pid, n, "Task", "");
                c.stage = "review".into();
                s.cards.push(c);
                s.workflow_runs.push(serde_json::from_value(json!({
                    "id":format!("run-{id}"),"project_id":pid,"card_id":id,"workflow_id":"delivery",
                    "definition_revision":1,"definition":{"id":"delivery","name":"Delivery","entries":{"review":"deliver"},"steps":{"deliver":{"type":"deliver","sync_upstream":true,"max_conflict_attempts":3,"next":"done"},"done":{"type":"complete"}}},
                    "state":"waiting_delivery","step":"deliver","visit":1,"trigger_event_id":null,
                    "input_digest":"d","inputs":{"queued_at":4-n},"outputs":{},"history":[],"error":null,"created_at":0,"updated_at":0
                })).unwrap());
            }
        }
        let mut duplicate = s.workflow_runs[2].clone();
        duplicate.id = "duplicate-run".into();
        duplicate.inputs["queued_at"] = json!(99);
        s.workflow_runs.push(duplicate);
        for paused in [false, true] {
            s.workflow_runs[2].inputs["queue_pause_reason"] = if paused {
                json!("Checkout busy")
            } else {
                serde_json::Value::Null
            };
            s.cards[1].hold_auto_merge = paused;
            let idx = SnapshotIndex::build(&s);
            for c in &s.cards {
                let digest = &idx.card_status[&c.id];
                assert_eq!(
                    digest.is_merging,
                    crate::components::activity::is_merging(&s, c)
                );
                assert_eq!(
                    digest.specs_normal,
                    crate::components::activity::card_status_specs(&s, c, false)
                );
                assert_eq!(
                    digest.specs_ui_merging,
                    crate::components::activity::card_status_specs(&s, c, true)
                );
            }
        }
    }
    #[test]
    fn snapshot_index_groups_card_events_reply_counts_tags_and_preserves_link_lookup_equality() {
        let mut s1 = Snapshot::default();
        s1.projects
            .push(make_project("p1", "Canban", vec!["perf", "ui"]));
        s1.cards.push(make_card(
            "c1",
            "p1",
            88,
            "Smooth Card Transitions",
            "Investigate #perf and [#lru-cache](tag:lru-cache) bottlenecks",
        ));
        s1.cards.push(make_card("c2", "p1", 89, "Other Card", ""));
        s1.events
            .push(make_event(1, "c1", "comment.created", "First reply"));
        s1.events
            .push(make_event(2, "c1", "stage.changed", "Moved to develop"));
        s1.events
            .push(make_event(3, "c1", "execution.completed", "Finished"));
        s1.events
            .push(make_event(4, "c2", "comment.created", "Unrelated reply"));

        let idx1 = SnapshotIndex::build(&s1);
        assert_eq!(idx1.card_events(&s1, "c1").len(), 3);
        assert_eq!(idx1.card_events(&s1, "c2").len(), 1);
        assert_eq!(idx1.card_events(&s1, "missing").len(), 0);
        assert_eq!(
            idx1.reply_count("c1"),
            crate::card::timeline::reply_count(&s1.events, "c1")
        );

        let known = KnownTagsIndex::build(&idx1, &BTreeSet::from(["local-tag".to_owned()]));
        let c1_tags = known.tags_for_card("c1");
        assert!(c1_tags.contains(&"perf".to_owned()));
        assert!(c1_tags.contains(&"lru-cache".to_owned()));

        let lookup1 = LinkLookup::build(&idx1);
        assert_eq!(lookup1.resolve_card_number("p1", 88), Some("c1"));
        assert_eq!(lookup1.resolve_card_number("p1", 89), Some("c2"));

        // Appending timeline events changes `SnapshotIndex` generation, while `LinkLookup`
        // remains equal so open Markdown readers skip re-decoration.
        let mut s2 = s1.clone();
        s2.events
            .push(make_event(5, "c2", "comment.created", "Another reply"));
        let idx2 = SnapshotIndex::build(&s2);
        assert_ne!(idx1, idx2);
        let lookup2 = LinkLookup::build(&idx2);
        assert_eq!(lookup1, lookup2);
    }
}
