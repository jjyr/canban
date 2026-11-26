//! Shared derived `Snapshot` indexes so Board tiles, Card timelines, and Markdown readers
//! borrow pre-indexed `O(log N)` maps instead of cloning or scanning the full `Snapshot`.
use canban_types::{
    Artifact, Card, CardEventSummary, GitHubIssueSnapshot, HerdrStatus, Project, ReviewDraft,
    Snapshot, TerminalEntry, WorkflowRevision, WorkflowStep, normalize_tag,
};
use canban_ui_kit::{Icon, Tone};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_INDEX_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningAgentBadge {
    pub terminal_id: String,
    pub card_id: String,
    pub profile_id: String,
    pub provider_icon: Icon,
    pub provider_name: String,
    pub herdr_status: HerdrStatus,
    pub wait_detail: Option<String>,
    pub started_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardStatusDigest {
    pub is_merging: bool,
    pub specs_normal: Vec<(String, Tone, bool)>,
    pub specs_ui_merging: Vec<(String, Tone, bool)>,
    pub running: Vec<RunningAgentBadge>,
}

#[derive(Debug, Default)]
pub struct SnapshotIndex {
    pub generation: u64,
    pub projects: BTreeMap<String, Project>,
    pub project_order: Vec<Project>,
    pub project_has_deliver: BTreeSet<String>,
    pub cards: BTreeMap<String, Card>,
    pub card_summaries: BTreeMap<String, CardEventSummary>,
    pub last_event_by_card: BTreeMap<String, u64>,
    pub reply_counts: BTreeMap<String, usize>,
    pub artifacts: Vec<Artifact>,
    pub artifact_counts: BTreeMap<String, usize>,
    pub github_issues: BTreeMap<(String, u64, u64), GitHubIssueSnapshot>,
    pub project_has_github_cards: BTreeSet<String>,
    pub explicit_body_tags: Vec<String>,
    pub review_drafts: BTreeMap<String, ReviewDraft>,
}

fn deliver_projects(revisions: &[WorkflowRevision]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for rev in revisions {
        if rev
            .definition
            .steps
            .values()
            .any(|st| matches!(st, WorkflowStep::Deliver { .. }))
        {
            out.insert(rev.project_id.clone());
        }
    }
    out
}

impl SnapshotIndex {
    pub fn build(snapshot: &Snapshot) -> Self {
        let mut span = canban_profiler::Span::new("index.build");
        span.counts(snapshot.cards.len(), snapshot.card_summaries.len());
        let mut projects = BTreeMap::new();
        for p in &snapshot.projects {
            projects.insert(p.id.clone(), p.clone());
        }
        let project_order = snapshot.projects.clone();
        let project_has_deliver = deliver_projects(&snapshot.workflow_revisions);

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

        let card_summaries = snapshot.card_summaries.clone();
        let mut last_event_by_card: BTreeMap<String, u64> = BTreeMap::new();
        let mut reply_counts = BTreeMap::new();
        for (card_id, summary) in &snapshot.card_summaries {
            if summary.last_event_seq > 0 {
                last_event_by_card.insert(card_id.clone(), summary.last_event_seq);
            }
            if summary.reply_count > 0 {
                reply_counts.insert(card_id.clone(), summary.reply_count as usize);
            }
        }

        let artifacts = snapshot.artifacts.clone();
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
            card_summaries,
            last_event_by_card,
            reply_counts,
            artifacts,
            artifact_counts,
            github_issues,
            project_has_github_cards,
            explicit_body_tags,
            review_drafts,
        }
    }

    pub fn matches_workspace(&self, snapshot: &Snapshot) -> bool {
        self.project_order == snapshot.projects
            && self.cards.len() == snapshot.cards.len()
            && snapshot
                .cards
                .iter()
                .all(|c| self.cards.get(&c.id) == Some(c))
            && self.card_summaries == snapshot.card_summaries
            && self.artifacts == snapshot.artifacts
            && self.review_drafts.len() == snapshot.review_drafts.len()
            && snapshot
                .review_drafts
                .iter()
                .all(|d| self.review_drafts.get(&d.card_id) == Some(d))
            && self.github_issues.len() == snapshot.github_issues.len()
            && snapshot.github_issues.iter().all(|i| {
                self.github_issue(&i.project_id, i.source.repository_id, i.source.issue_id)
                    == Some(i)
            })
            && self.project_has_deliver == deliver_projects(&snapshot.workflow_revisions)
    }

    pub fn reuse_or_build(prev: Option<&Rc<Self>>, snapshot: &Snapshot) -> Rc<Self> {
        if let Some(existing) = prev
            && existing.matches_workspace(snapshot)
        {
            return Rc::clone(existing);
        }
        Rc::new(Self::build(snapshot))
    }

    pub fn review_draft(&self, card_id: &str) -> Option<&ReviewDraft> {
        self.review_drafts.get(card_id)
    }

    pub fn reply_count(&self, card_id: &str) -> usize {
        self.reply_counts.get(card_id).copied().unwrap_or(0)
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

fn agent_started_at_indexed(entry: &TerminalEntry) -> Option<u64> {
    let agent = entry.agent.as_ref()?;
    if let Some(ts) = agent.started_at.filter(|&t| t > 1_000_000_000_000) {
        return Some(ts);
    }
    if agent.updated_at > 1_000_000_000_000 {
        Some(agent.updated_at)
    } else if entry.created_at > 1_000_000_000_000 {
        Some(entry.created_at)
    } else {
        None
    }
}

fn running_agents_indexed(card_terminals: &[&TerminalEntry]) -> Vec<RunningAgentBadge> {
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
        .filter_map(|t| {
            let agent = t.agent.as_ref()?;
            let (provider_icon, provider_name) = crate::profiles::provider(&agent.profile);
            let wait_detail = t
                .runtime_wait()
                .and_then(crate::terminals::entries::format_runtime_wait);
            Some(RunningAgentBadge {
                terminal_id: t.id.clone(),
                card_id: agent.card_id.clone(),
                profile_id: agent.profile.id.clone(),
                provider_icon,
                provider_name,
                herdr_status: t.herdr_status(),
                wait_detail,
                started_at: agent_started_at_indexed(t),
            })
        })
        .collect()
}

/// Separate per-card runtime/workflow status projection (merging, workflow queue specs,
/// and active agent badges without volatile `observed_at` timestamps).
pub fn build_card_status(snapshot: &Snapshot) -> BTreeMap<String, CardStatusDigest> {
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
    // once per Project, never once (or three times) per Card.
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
                if let Some(cid) = run.card_id.as_deref() {
                    queue_status
                        .entry(cid)
                        .or_insert((i + 1, queue.len(), head));
                }
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
    card_status
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
