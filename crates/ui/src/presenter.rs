use crate::drafts::Draft;
pub mod submission;
use canban_types::Event;
use canban_types::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
pub use submission::CommittedWrites;

pub const REPORT_BODY_CACHE_CAP: usize = 128;
pub const LINK_PREVIEW_CACHE_CAP: usize = 128;
pub const PARSED_MARKDOWN_CACHE_CAP: usize = 192;
pub const MAX_CACHED_MARKDOWN_BYTES: usize = 256 * 1024;
const PREFETCH_REPORT_BATCH_LIMIT: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedReportBody {
    pub version: u64,
    pub body: String,
}

pub(crate) fn format_incomplete_outcome(method: &str, value: &Value) -> String {
    if let Some(msg) = value["message"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return format!("{method}: {msg}");
    }
    if let Some(state) = value["state"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if state == "closing" {
            return format!(
                "{method}: close still in progress (state=closing); inspect before retrying"
            );
        }
        return format!("{method}: unexpected state '{state}'; inspect before retrying");
    }
    let raw = value.to_string();
    let summary: String = raw.chars().take(120).collect();
    if raw.chars().count() > 120 {
        format!("{method}: unexpected response ({summary}…); inspect before retrying")
    } else {
        format!("{method}: unexpected response ({summary}); inspect before retrying")
    }
}

pub fn event_body_version(event: &canban_types::Event) -> u64 {
    event.data["edited_at"]
        .as_u64()
        .or_else(|| event.data["revision"].as_u64())
        .unwrap_or(event.created_at)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardTimelineState {
    pub events: Vec<Event>,
    pub oldest_seq: Option<u64>,
    pub highwater: u64,
    pub has_more_before: bool,
    pub loading: bool,
    pub loading_older: bool,
    pub initialized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Location {
    #[serde(default)]
    pub new_draft: String,
    pub page: String,
    pub project: String,
    pub card: String,
    pub request: String,
    pub terminal: String,
    pub standalone: bool,
    pub terminal_card: String,
    #[serde(default)]
    pub card_terminal_open: bool,
    #[serde(default)]
    pub card_tab: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RefreshPending {
    #[default]
    None,
    Runtime,
    Full,
}

#[derive(Clone, Copy)]
pub struct Ui {
    /// Commands outlive transient buttons/lists, but end with the window.
    pub owner: Owner,
    pub github: crate::github::Browser,
    pub inbox: crate::inbox::State,
    pub insights: crate::insights::State,
    pub schedules: crate::schedules::State,
    pub workflows: crate::workflows::State,
    pub review_state: crate::review::State,
    /// Optional host adapter for the reusable Board drop intent (Gallery uses local fixtures).
    pub board_move: Stored<Option<crate::board::DropHandler>>,
    pub new_draft: Signal<String>,
    pub draft_epoch: Signal<u64>,
    pub draft_status: Signal<u64>,
    pub card_target: Signal<String>,
    pub card_submit: Signal<u64>,
    pub comment_submit: Signal<u64>,
    pub update: Signal<Value>,
    pub update_loading: Signal<bool>,
    pub update_busy: Signal<bool>,
    pub task_submit: Signal<u64>,
    pub editor_focus: Signal<u64>,
    pub utility: Signal<Value>,
    pub utility_popup: Signal<String>,
    pub agent_popover: crate::components::agent_popover::AgentPopoverState,
    pub native_modal: Signal<bool>,
    pub open_popovers: Signal<usize>,
    pub utility_query: Signal<String>,
    pub theme_preview: Signal<Option<String>>,
    pub utility_loading: Signal<bool>,
    pub utility_epoch: Signal<u64>,
    pub usage: Signal<Value>,
    pub usage_host: Signal<String>,
    pub usage_loading: Signal<bool>,
    pub usage_epoch: Signal<u64>,
    pub terminal_selection: Signal<String>,
    pub terminal_tabs: Signal<Vec<String>>,
    pub terminal_details: Signal<bool>,
    pub terminal_focus: Signal<u64>,
    pub terminal_rename: Signal<Option<String>>,
    pub terminal_close_confirm: Signal<Option<String>>,
    pub closing_terminals: Signal<BTreeSet<String>>,
    pub terminal_git_syncing: Signal<BTreeSet<String>>,
    pub terminal_entries: Memo<Vec<crate::terminals::Entry>>,
    pub operation_error: Signal<String>,
    pub backend_notice: Signal<String>,
    pub history: Signal<crate::navigation::History>,
    pub card_standalone: Signal<bool>,
    pub card_tab: Signal<String>,
    pub terminal_card_context: Signal<String>,
    pub terminal_sidebar: Signal<bool>,
    pub terminal_folds: Signal<BTreeSet<String>>,
    pub terminal_width: Signal<f32>,
    pub quick_open: Signal<bool>,
    pub project_picker: Signal<bool>,
    pub search_query: Signal<String>,
    pub local_tags: Signal<BTreeSet<String>>,
    pub recent_destinations: Stored<Vec<String>>,
    pub link_previews: Stored<crate::cache::LruCache<String, crate::links::CachedPreview>>,
    pub report_bodies: Stored<crate::cache::LruCache<(String, String), CachedReportBody>>,
    pub report_inflight: Stored<BTreeSet<(String, String)>>,
    pub parsed_markdown: Stored<crate::cache::LruCache<String, nsh_base::text_view::Document>>,
    pub snapshot: Signal<Snapshot>,
    pub snapshot_ready: Signal<bool>,
    pub index: Memo<Rc<crate::index::SnapshotIndex>>,
    pub card_status: Memo<Rc<BTreeMap<String, crate::index::CardStatusDigest>>>,
    pub link_lookup: Memo<Rc<crate::index::LinkLookup>>,
    pub known_tags: Memo<Rc<crate::index::KnownTagsIndex>>,
    pub overview_rows: Memo<Rc<Vec<canban_notifications::OverviewRow>>>,
    pub daily_inbox: Memo<Rc<canban_notifications::DailyInbox>>,
    pub reminders: Memo<Vec<canban_notifications::Reminder>>,
    pub sound_items: Memo<Vec<String>>,
    pub reminder_counts: Memo<BTreeMap<String, canban_notifications::ReminderCount>>,
    pub active_card: Memo<Option<Card>>,
    pub card_view_card: Option<Memo<Option<Card>>>,
    pub active_project: Memo<Option<Project>>,
    pub github_account: Memo<GitHubAccount>,
    pub notification_delivery: Stored<canban_notifications::DeliveryTracker>,
    pub notification_resync: Stored<bool>,
    pub profiles: Signal<Vec<Profile>>,
    pub all_profiles: Signal<Vec<Profile>>,
    pub default_agent: Signal<String>,
    pub onboarded: Signal<bool>,
    pub agent_catalog: Signal<Value>,
    pub agent_catalog_loading: Signal<bool>,
    pub agent_config_epoch: Signal<u64>,
    pub agent_catalog_epoch: Signal<u64>,
    pub(crate) agent_catalog_loading_host: Stored<Option<String>>,
    pub(crate) agent_catalog_requested: Stored<Option<String>>,
    pub(crate) pending_catalog_ops: Stored<Vec<(u64, Value)>>,
    pub max_agents: Signal<usize>,
    pub idle_minutes: Signal<u64>,
    pub terminal_env_mode: Signal<HostTerminalEnvMode>,
    pub overview_selection: Signal<String>,
    pub overview_closed: Signal<bool>,
    pub overview_idle: Signal<bool>,
    pub terminal_recent: Stored<BTreeMap<String, String>>,
    pub terminal_pending_open: Signal<Option<crate::terminals::navigation::PendingOpen>>,
    pub card_event: Signal<Option<u64>>,
    pub editing_comment: Signal<Option<String>>,
    pub deleting_comment: Signal<Option<String>>,
    pub project: Signal<String>,
    pub(crate) pending_remote_project:
        Stored<Option<crate::projects::selection::PendingRemoteRestore>>,
    pub archive_query: Signal<String>,
    pub archive_folds: Signal<BTreeSet<String>>,
    pub card: Signal<String>,
    /// Read-only identity of a mounted Card, retained during its exit animation.
    pub card_view: Option<Signal<String>>,
    pub request: Signal<String>,
    pub page: Signal<String>,
    pub settings_section: Signal<String>,
    pub status: Signal<String>,
    pub toast: Signal<String>,
    pub toast_epoch: Signal<u64>,
    pub pending: Signal<bool>,
    pub pending_methods: Signal<BTreeSet<&'static str>>,
    /// Cards with a UI-initiated merge operation currently in flight.
    pub merging_cards: Signal<BTreeSet<String>>,
    /// Periodic wall-clock timestamp (ms) updated on a slow timer for non-realtime elapsed badges.
    pub runtime_clock: Signal<u64>,
    /// Shared minute-granularity clock (s) for utility bar usage countdowns.
    pub utility_clock: Signal<u64>,
    pub(crate) dwell: Stored<crate::activity::DwellMachine>,
    pub activity_flush_seq: Signal<u64>,
    pub refreshing: Signal<bool>,
    pub(crate) refresh_pending: Signal<RefreshPending>,
    pub snapshot_epoch: Signal<u64>,
    pub env_sync_epoch: Signal<u64>,
    pub card_timelines: Signal<BTreeMap<String, CardTimelineState>>,
    pub(crate) committed_writes: Stored<submission::CommittedWrites>,
    pub output: Signal<String>,
    pub output_observed_at: Signal<Option<u64>>,
    pub output_request: Signal<u64>,
    pub review: Signal<Option<ReviewPreview>>,
    pub review_request: Signal<u64>,
    /// Card header quick merge awaiting confirmation of a preview warning.
    pub quick_merge: Signal<Option<ReviewPreview>>,
    /// Card resolve action awaiting confirmation when active runs or unmerged branch changes exist.
    pub resolve_confirm: Signal<Option<crate::card::ResolveConfirm>>,
    pub drafts: Stored<BTreeMap<String, Draft>>,
    pub scrolls: Stored<BTreeMap<String, Point<Points>>>,
    pub window_size: Signal<(f32, f32)>,
    pub narrow: Signal<bool>,
    pub expanded: Signal<bool>,
    pub focused_card: Signal<bool>,
    pub board_width: Signal<f32>,
    pub focus_width: Signal<f32>,
    pub card_terminal_open: Signal<bool>,
    pub card_terminal_width: Signal<f32>,
    pub card_terminal_focused: Signal<bool>,
    pub worktree_modal: Signal<Option<String>>,
    pub worktree_promote: Signal<Option<String>>,
    pub worktree_remove_confirm: Signal<Option<String>>,
    pub project_archive_confirm: Signal<Option<String>>,
    pub project_restore_modal: Signal<Option<(String, crate::projects::ArchivedProjectSummary)>>,
    pub archived_projects_reload: Signal<u64>,
    pub archived_projects_preview: Signal<Option<Vec<crate::projects::ArchivedProjectSummary>>>,
    pub hosts: Signal<Vec<HostSummary>>,
    /// Per-remote-host reconnect epoch bumped whenever a remote host transitions back
    /// to `Connected` or reports a new `reconnect_count`.
    pub host_connect_epochs: Signal<BTreeMap<String, u64>>,
    pub paired_devices_preview: Signal<Option<PairedDevicesPreview>>,
    pub sync_preview: Signal<Vec<SyncItemView>>,
    pub settings_host: Signal<String>,
    pub installed_fonts: Signal<Option<canban_terminal::InstalledFonts>>,
    pub installed_fonts_loading: Signal<bool>,
    pub diagnostics: Signal<Value>,
    pub diagnostics_logs: Signal<Value>,
    pub diagnostics_log_source: Signal<String>,
    pub diagnostics_loading: Signal<bool>,
    pub discovery: Signal<DiscoveryCatalog>,
    pub discovery_loading: Signal<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairedDevicesPreview {
    pub devices: Vec<PairedDevice>,
    pub server_pubkey_fingerprint: Option<String>,
    pub tcp_enabled: bool,
    pub listening: bool,
    pub listen_addrs: Vec<String>,
    pub trust_tailscale_self: bool,
    pub generated_uri: Option<String>,
    pub generated_expires_at: Option<u64>,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}
fn ordered_enabled_profiles(all: &[Profile], default_agent: &str) -> Vec<Profile> {
    let mut enabled: Vec<Profile> = all.iter().filter(|p| p.enabled).cloned().collect();
    enabled.sort_by_key(|p| {
        let rank = if p.id == default_agent {
            0usize
        } else if p.is_identity() {
            1
        } else {
            2
        };
        (rank, p.id.clone())
    });
    enabled
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceDismiss {
    CloseNewCard,
    CloseCard,
}

pub fn workspace_dismiss_action(page: &str, blocked: bool) -> Option<WorkspaceDismiss> {
    if blocked {
        return None;
    }
    match page {
        "new-card" => Some(WorkspaceDismiss::CloseNewCard),
        "card" => Some(WorkspaceDismiss::CloseCard),
        _ => None,
    }
}

impl Ui {
    /// Apply a server snapshot without losing writes acknowledged since it was
    /// read (including remote hosts whose cached state has not caught up yet).
    pub(crate) fn accept_snapshot(self, mut snapshot: Snapshot) {
        if self
            .committed_writes
            .try_with(|writes| writes.reconcile(&mut snapshot))
            .is_some()
        {
            self.snapshot.try_set(snapshot);
        }
    }

    pub(crate) fn accept_runtime_state(self, runtime: RuntimeState) -> bool {
        let current = self.snapshot.get_untracked();
        let Some(next) = self
            .committed_writes
            .try_with(|writes| apply_runtime_state_to_snapshot(&current, runtime, writes))
            .flatten()
        else {
            return false;
        };
        let summaries = snapshot_summaries(&next);
        let different = self.snapshot.with_untracked(|old| old != &next);
        let hosts_changed =
            !summaries.is_empty() && self.hosts.with_untracked(|old| *old != summaries);
        if different || hosts_changed {
            batch(move || {
                if hosts_changed {
                    self.hosts.try_set(summaries);
                }
                if different {
                    self.runtime_clock.try_set(now());
                    self.snapshot.try_set(next);
                }
            });
        }
        true
    }

    pub fn apply_config(self, cfg: &Config) {
        let unchanged = self
            .default_agent
            .with_untracked(|v| v == &cfg.default_agent)
            && self.onboarded.get_untracked() == cfg.onboarded
            && self.all_profiles.with_untracked(|v| v == &cfg.profiles)
            && self.max_agents.get_untracked() == cfg.scheduler.max_agents
            && self.idle_minutes.get_untracked() == cfg.scheduler.idle_minutes
            && self.terminal_env_mode.get_untracked() == cfg.terminal_env_mode;
        if unchanged {
            return;
        }
        batch(move || {
            self.default_agent.set(cfg.default_agent.clone());
            self.onboarded.set(cfg.onboarded);
            self.all_profiles.set(cfg.profiles.clone());
            self.profiles
                .set(ordered_enabled_profiles(&cfg.profiles, &cfg.default_agent));
            self.max_agents.set(cfg.scheduler.max_agents);
            self.idle_minutes.set(cfg.scheduler.idle_minutes);
            self.terminal_env_mode.set(cfg.terminal_env_mode);
        });
    }

    pub(crate) fn update_profiles(self, f: impl FnOnce(&mut Vec<Profile>, &mut String)) {
        let mut all = self.all_profiles.get_untracked();
        let mut def = self.default_agent.get_untracked();
        f(&mut all, &mut def);
        let enabled = ordered_enabled_profiles(&all, &def);
        batch(move || {
            self.default_agent.set(def);
            self.all_profiles.set(all);
            self.profiles.set(enabled);
        });
    }
    pub fn active_project_host(self) -> String {
        let pid = self.project.get_untracked();
        if pid.is_empty() {
            return "local".into();
        }
        self.snapshot
            .with_untracked(|s| s.project_host(&pid).to_owned())
    }
    pub fn is_active_project_offline(self) -> bool {
        let pid = self.project.get();
        if pid.is_empty() {
            return false;
        }
        self.snapshot
            .with(|s| s.is_host_offline(s.project_host(&pid)))
    }
    pub fn effective_config_host(self) -> String {
        let page = self.page.get_untracked();
        if page == "settings" || page == "agent-setup" {
            let h = self.settings_host.get_untracked();
            if h.is_empty() { "local".into() } else { h }
        } else {
            self.active_project_host()
        }
    }
    pub fn spawn<T: 'static>(self, future: impl std::future::Future<Output = T> + 'static) {
        self.owner.run(|| {
            spawn(future);
        });
    }
    pub fn clear_output(self) {
        batch(move || {
            self.output_request
                .set(self.output_request.get_untracked() + 1);
            self.output.set(String::new());
            self.output_observed_at.set(None);
        });
    }
    pub fn new_card(self) {
        if self.page.get_untracked() == "new-card" {
            return;
        }
        batch(move || {
            self.utility_popup.set(String::new());
            self.agent_popover.close();
            self.quick_open.set(false);
            self.new_draft.set(format!("new-card:{}", id()));
            self.card_target.set(self.project.get_untracked());
            self.navigate("new-card");
        });
    }
    /// Resolve only the visible Card/Terminal when the user clicks Board or Git.
    /// This is a navigation target, not a separately maintained default Project.
    pub(crate) fn navigation_project(self) -> Option<String> {
        let project = match self.page.get_untracked().as_str() {
            "card" => untracked(|| self.current_card()).map(|c| c.project_id),
            "terminals" => untracked(|| crate::terminals::current(self))
                .and_then(|e| e.card.map(|c| c.project_id).or(e.project_id)),
            _ => None,
        };
        project.filter(|id| {
            self.snapshot
                .with_untracked(|s| s.projects.iter().any(|p| &p.id == id))
        })
    }
    pub fn open_board(self) {
        batch(|| {
            if let Some(project) = self.navigation_project() {
                self.project.set(project);
            }
            if self.page.get_untracked() == "card" {
                self.close_card();
            } else {
                self.navigate("board");
            }
        });
    }
    pub fn navigate(self, page: &str) {
        let dest = page.to_owned();
        batch(move || {
            self.agent_popover.close();
            self.card_standalone.set(false);
            if dest == "terminals" {
                self.terminal_focus.update(|v| *v += 1);
                self.terminal_card_context.set(String::new());
                crate::terminals::restore_selection(self);
                if self.narrow.get_untracked()
                    && !self.terminal_selection.get_untracked().is_empty()
                {
                    self.terminal_sidebar.set(false);
                }
            }
            self.clear_output();
            self.review_request
                .set(self.review_request.get_untracked() + 1);
            self.page.set(dest);
        });
    }
    pub fn open_settings_section(self, section: &str) {
        let sec = section.to_owned();
        batch(move || {
            self.settings_section.set(sec);
            self.navigate("settings");
        });
    }
    pub fn set_popover_open(self, open: bool) {
        untracked(|| {
            if let Some(cur) = self.open_popovers.try_get() {
                let next = if open {
                    cur.saturating_add(1)
                } else {
                    cur.saturating_sub(1)
                };
                let _ = self.open_popovers.try_set(next);
            }
        });
    }
    pub fn has_blocking_overlay(self) -> bool {
        self.quick_open.get_untracked()
            || !self.utility_popup.get_untracked().is_empty()
            || self.native_modal.get_untracked()
            || self.open_popovers.get_untracked() > 0
            || self.quick_merge.get_untracked().is_some()
            || self.resolve_confirm.get_untracked().is_some()
            || self.terminal_rename.get_untracked().is_some()
            || self.terminal_close_confirm.get_untracked().is_some()
            || self.worktree_modal.get_untracked().is_some()
            || self.worktree_promote.get_untracked().is_some()
            || self.worktree_remove_confirm.get_untracked().is_some()
            || self.project_archive_confirm.get_untracked().is_some()
            || self.project_restore_modal.get_untracked().is_some()
            || self.review_state.discard_confirm.get_untracked().is_some()
            || self.schedules.create_modal.get_untracked()
            || self.schedules.yaml_open.get_untracked()
            || self.schedules.agent_open.get_untracked()
            || self.schedules.artifact.with_untracked(Option::is_some)
    }
    pub fn close_card(self) {
        batch(move || {
            self.editing_comment.set(None);
            self.deleting_comment.set(None);
            self.card_tab.set(String::new());
            self.card_terminal_open.set(false);
            self.card_terminal_focused.set(false);
            self.card.set(String::new());
            self.card_standalone.set(false);
            self.clear_output();
            self.page.set("board".into());
        });
    }
    pub fn dismiss_workspace_pane(self) {
        match workspace_dismiss_action(&self.page.get_untracked(), self.has_blocking_overlay()) {
            Some(WorkspaceDismiss::CloseNewCard) => self.navigate("board"),
            Some(WorkspaceDismiss::CloseCard) => self.close_card(),
            None => {}
        }
    }
    pub fn new() -> Self {
        let snapshot = signal(Snapshot::default());
        let local_tags = signal(BTreeSet::new());
        let project = signal(String::new());
        let card = signal(String::new());
        let prev_index = stored(Option::<Rc<crate::index::SnapshotIndex>>::None);
        let index = computed(move || {
            let next = snapshot.with(|s| {
                prev_index
                    .with(|prev| crate::index::SnapshotIndex::reuse_or_build(prev.as_ref(), s))
            });
            prev_index.replace(Some(Rc::clone(&next)));
            next
        });
        let card_status =
            computed(move || snapshot.with(|s| Rc::new(crate::index::build_card_status(s))));
        let link_lookup = computed(move || Rc::new(crate::index::LinkLookup::build(&index.get())));
        let known_tags = computed(move || {
            local_tags.with(|lt| Rc::new(crate::index::KnownTagsIndex::build(&index.get(), lt)))
        });
        let overview_rows =
            computed(move || snapshot.with(|s| Rc::new(canban_notifications::overview(s))));
        let reminders = computed(move || snapshot.with(canban_notifications::reminders));
        let sound_items = computed(move || {
            snapshot.with(|s| {
                reminders.with(|r| canban_notifications::sound_items_with_reminders(s, r))
            })
        });
        let reminder_counts = computed(move || reminders.with(|r| canban_notifications::counts(r)));
        let terminal_entries = computed(move || snapshot.with(crate::terminals::entries));
        let active_card = computed(move || {
            let cid = card.get();
            if cid.is_empty() {
                return None;
            }
            snapshot.with(|s| s.cards.iter().find(|c| c.id == cid).cloned())
        });
        let active_project = computed(move || {
            let pid = project.get();
            if pid.is_empty() {
                return None;
            }
            snapshot.with(|s| s.projects.iter().find(|p| p.id == pid).cloned())
        });
        let github_account = computed(move || snapshot.with(|s| s.github.clone()));
        let inbox = crate::inbox::State::new();
        let utility = signal(json!({"preferences":{"theme":"canban"},"awake":false}));
        let utility_clock = signal(now() / 1000);
        let daily_inbox = computed(move || {
            let cutoff = utility.with(crate::inbox::cutoff_minutes_from_utility);
            let now_ms = utility_clock.get().saturating_mul(1000).max(now());
            let tz = crate::inbox::local_tz_offset_secs();
            snapshot.with(|s| {
                inbox.github_notifications.with(|gh| {
                    Rc::new(canban_notifications::build_daily_inbox(
                        s, gh, now_ms, tz, cutoff,
                    ))
                })
            })
        });
        Self {
            owner: Owner::current(),
            github: crate::github::Browser::new(),
            inbox,
            insights: crate::insights::State::new(),
            schedules: crate::schedules::State::new(),
            workflows: crate::workflows::State::new(),
            review_state: crate::review::State::new(),
            board_move: stored(None),
            new_draft: signal(String::new()),
            draft_epoch: signal(0),
            draft_status: signal(0),
            card_target: signal(String::new()),
            card_submit: signal(0),
            comment_submit: signal(0),
            update: signal(json!({"build":crate::backend::build_info(),"state":{}})),
            update_loading: signal(false),
            update_busy: signal(false),
            task_submit: signal(0),
            editor_focus: signal(0),
            utility,
            utility_popup: signal(String::new()),
            agent_popover: crate::components::agent_popover::AgentPopoverState::new(),
            native_modal: signal(false),
            open_popovers: signal(0),
            utility_query: signal(String::new()),
            theme_preview: signal(None),
            utility_loading: signal(false),
            utility_epoch: signal(0),
            usage: signal(json!({"readings":[]})),
            usage_host: signal(String::new()),
            usage_loading: signal(false),
            usage_epoch: signal(0),
            terminal_selection: signal(String::new()),
            terminal_tabs: signal(Vec::new()),
            terminal_details: signal(false),
            terminal_focus: signal(0),
            terminal_rename: signal(None),
            terminal_close_confirm: signal(None),
            closing_terminals: signal(BTreeSet::new()),
            terminal_git_syncing: signal(BTreeSet::new()),
            terminal_entries,
            operation_error: signal(String::new()),
            backend_notice: signal(String::new()),
            history: signal(crate::navigation::History::default()),
            card_standalone: signal(false),
            card_tab: signal(String::new()),
            terminal_card_context: signal(String::new()),
            terminal_sidebar: signal(true),
            terminal_folds: signal(BTreeSet::new()),
            terminal_width: signal(260.),
            snapshot,
            snapshot_ready: signal(false),
            index,
            card_status,
            link_lookup,
            known_tags,
            overview_rows,
            daily_inbox,
            reminders,
            sound_items,
            reminder_counts,
            active_card,
            card_view_card: None,
            active_project,
            github_account,
            notification_delivery: stored(Default::default()),
            notification_resync: stored(true),
            pending_remote_project: stored(None),
            quick_open: signal(false),
            project_picker: signal(false),
            search_query: signal(String::new()),
            local_tags,
            recent_destinations: stored(Vec::new()),
            link_previews: stored(crate::cache::LruCache::new(LINK_PREVIEW_CACHE_CAP)),
            report_bodies: stored(crate::cache::LruCache::new(REPORT_BODY_CACHE_CAP)),
            report_inflight: stored(BTreeSet::new()),
            parsed_markdown: stored(crate::cache::LruCache::new(PARSED_MARKDOWN_CACHE_CAP)),
            idle_minutes: signal(
                crate::backend::config_for_host("local")
                    .scheduler
                    .idle_minutes,
            ),
            overview_selection: signal(String::new()),
            overview_closed: signal(false),
            overview_idle: signal(false),
            terminal_recent: stored(BTreeMap::new()),
            dwell: stored(Default::default()),
            activity_flush_seq: signal(0),
            terminal_pending_open: signal(None),
            card_event: signal(None),
            editing_comment: signal(None),
            deleting_comment: signal(None),
            max_agents: signal(
                crate::backend::config_for_host("local")
                    .scheduler
                    .max_agents,
            ),
            terminal_env_mode: signal(crate::backend::config_for_host("local").terminal_env_mode),
            profiles: signal({
                let cfg = crate::backend::config_for_host("local");
                ordered_enabled_profiles(&cfg.profiles, &cfg.default_agent)
            }),
            all_profiles: signal(crate::backend::config_for_host("local").profiles),
            default_agent: signal(crate::backend::config_for_host("local").default_agent),
            onboarded: signal(crate::backend::config_for_host("local").onboarded),
            agent_catalog: signal(Value::Null),
            agent_catalog_loading: signal(false),
            agent_config_epoch: signal(0),
            agent_catalog_epoch: signal(0),
            agent_catalog_loading_host: stored(None),
            agent_catalog_requested: stored(None),
            pending_catalog_ops: stored(Vec::new()),
            project,
            archive_query: signal(String::new()),
            archive_folds: signal(BTreeSet::new()),
            card,
            card_view: None,
            request: signal(String::new()),
            page: signal("board".into()),
            settings_section: signal(String::new()),
            status: signal(String::new()),
            toast: signal(String::new()),
            toast_epoch: signal(0),
            pending: signal(false),
            pending_methods: signal(BTreeSet::new()),
            merging_cards: signal(BTreeSet::new()),
            runtime_clock: signal(now()),
            utility_clock,
            refreshing: signal(false),
            refresh_pending: signal(RefreshPending::None),
            snapshot_epoch: signal(0),
            env_sync_epoch: signal(0),
            card_timelines: signal(BTreeMap::new()),
            committed_writes: stored(Default::default()),
            output: signal(String::new()),
            output_observed_at: signal(None),
            output_request: signal(0),
            review: signal(None),
            review_request: signal(0),
            quick_merge: signal(None),
            resolve_confirm: signal(None),
            drafts: stored(crate::drafts::persistence::initial()),
            scrolls: stored(BTreeMap::new()),
            window_size: signal((0.0, 0.0)),
            narrow: signal(false),
            expanded: signal(false),
            focused_card: signal(false),
            board_width: signal(700.0),
            focus_width: signal(420.0),
            card_terminal_open: signal(false),
            card_terminal_width: signal(520.0),
            card_terminal_focused: signal(false),
            worktree_modal: signal(None),
            worktree_promote: signal(None),
            worktree_remove_confirm: signal(None),
            project_archive_confirm: signal(None),
            project_restore_modal: signal(None),
            archived_projects_reload: signal(0),
            archived_projects_preview: signal(None),
            hosts: signal(crate::host_summaries()),
            host_connect_epochs: signal(BTreeMap::new()),
            paired_devices_preview: signal(None),
            sync_preview: signal(Vec::new()),
            settings_host: signal("local".into()),
            installed_fonts: signal(canban_terminal::try_installed_fonts()),
            installed_fonts_loading: signal(false),
            diagnostics: signal(Value::Null),
            diagnostics_logs: signal(Value::Null),
            diagnostics_log_source: signal("server".into()),
            diagnostics_loading: signal(false),
            discovery: signal(DiscoveryCatalog::default()),
            discovery_loading: signal(false),
        }
    }
    pub fn location(self) -> Location {
        Location {
            new_draft: if self.page.get_untracked() == "new-card" {
                self.new_draft.get_untracked()
            } else {
                String::new()
            },
            page: self.page.get_untracked(),
            project: self.project.get_untracked(),
            card: self.card.get_untracked(),
            request: self.request.get_untracked(),
            terminal: if self.page.get_untracked() == "terminals" {
                self.terminal_selection.get_untracked()
            } else {
                String::new()
            },
            card_terminal_open: self.page.get_untracked() == "card"
                && self.card_terminal_open.get_untracked(),
            card_tab: if self.page.get_untracked() == "card" {
                self.card_tab.get_untracked()
            } else {
                String::new()
            },
            standalone: self.card_standalone.get_untracked(),
            terminal_card: if self.page.get_untracked() == "terminals" {
                self.terminal_card_context.get_untracked()
            } else {
                String::new()
            },
        }
    }
    pub fn apply_hosts(self, summaries: Vec<HostSummary>) {
        let prev = self.hosts.get_untracked();
        if prev == summaries {
            return;
        }
        let mut next_epochs = self.host_connect_epochs.get_untracked();
        let to_bump = apply_host_summaries(&prev, &summaries, &mut next_epochs);
        if !to_bump.is_empty() {
            self.host_connect_epochs.set(next_epochs);
            for hid in to_bump {
                canban_terminal::evict_idle_host_sessions(&hid);
            }
        }
        self.hosts.set(summaries);
    }
    pub fn open_terminal(self, conversation: String) {
        self.open_terminal_internal(conversation, true);
    }
    /// Activates a Terminal in the current WorktreeScope without unfolding a collapsed Scope header.
    pub fn select_terminal_in_scope(self, conversation: String) {
        self.open_terminal_internal(conversation, false);
    }
    fn open_terminal_internal(self, conversation: String, reveal_scope: bool) {
        if self
            .closing_terminals
            .with_untracked(|c| c.contains(&conversation))
        {
            return;
        }
        let Some(entry) = self
            .terminal_entries
            .with_untracked(|rows| rows.iter().find(|e| e.saved.id == conversation).cloned())
        else {
            self.operation_error
                .set("This Terminal is no longer available".into());
            return;
        };
        if !entry.listed() || entry.agent.is_some() && entry.binding().is_none() {
            if let Some(row) = entry.agent {
                crate::terminals::navigation::history(self, &row);
            } else {
                self.operation_error.set("This Terminal has ended".into());
            }
            return;
        }
        if let Some(card) = &entry.card {
            self.terminal_recent.with(|recent| {
                recent.insert(card.id.clone(), conversation.clone());
            });
        }
        let scope_key = self
            .snapshot
            .with_untracked(|s| crate::terminals::worktrees::entry_scope(s, &entry).recent_key());
        self.terminal_recent.with(|recent| {
            recent.insert(scope_key, conversation.clone());
        });
        batch(|| {
            self.card_standalone.set(false);
            self.clear_output();
            self.review_request
                .set(self.review_request.get_untracked() + 1);
            self.terminal_details.set(false);
            self.terminal_focus.update(|v| *v += 1);
            self.terminal_tabs.update(|tabs| {
                if !tabs.contains(&conversation) {
                    tabs.push(conversation.clone());
                }
            });
            self.terminal_selection.set(conversation.clone());
            if reveal_scope {
                crate::terminals::reveal(self, &conversation);
            } else {
                crate::terminals::reveal_project(self, &conversation);
            }
            self.page.set("terminals".into());
        });
    }
    pub fn terminal_card(self, card: String, terminal: Option<String>) {
        let target_terminal = terminal.or_else(|| {
            untracked(|| crate::terminals::current(self))
                .filter(|e| e.card.as_ref().is_some_and(|c| c.id == card))
                .map(|e| e.saved.id)
        });
        self.open_card_with_terminal(card, target_terminal);
        self.card_standalone.set(true);
    }
    pub fn show_search(self, projects: bool) {
        self.utility_popup.set(String::new());
        self.agent_popover.close();
        self.search_query.set(String::new());
        self.project_picker.set(projects);
        self.quick_open.set(true);
    }
    pub fn open_tag_search(self, raw_tag: &str) {
        let Some(tag) = canban_types::normalize_tag(raw_tag) else {
            return;
        };
        self.utility_popup.set(String::new());
        self.agent_popover.close();
        self.project_picker.set(false);
        self.search_query.set(format!("#{tag}"));
        self.quick_open.set(true);
    }
    pub fn select_project(self, id: String) {
        let previous = self.project.get_untracked();
        self.recent_destinations.with(|ids| {
            for project in [previous, id.clone()] {
                if !project.is_empty() {
                    let key = format!("project:{project}");
                    ids.retain(|id| id != &key);
                    ids.insert(0, key);
                }
            }
            ids.truncate(32);
        });
        batch(move || {
            self.project.set(id);
            self.card.set(String::new());
            self.navigate("board");
        });
        self.prefetch_active_reports();
    }
    pub fn card_id(self) -> String {
        self.card_view
            .and_then(|id| id.try_get())
            .unwrap_or_else(|| self.card.get())
    }
    pub fn current_card(self) -> Option<Card> {
        if let Some(memo) = self.card_view_card {
            return memo.try_get().flatten();
        }
        if let Some(id_sig) = self.card_view {
            let cid = id_sig.try_get()?;
            return self.index.with(|idx| idx.cards.get(&cid).cloned());
        }
        self.active_card.get()
    }
    pub fn current_project(self) -> Option<Project> {
        self.active_project.get()
    }
    pub fn parse_markdown_cached(self, source: &str) -> nsh_base::text_view::Document {
        if source.len() <= MAX_CACHED_MARKDOWN_BYTES {
            if let Some(doc) = self
                .parsed_markdown
                .with(|cache| cache.get(source).cloned())
            {
                return doc;
            }
            let doc = nsh_base::text_view::Parser::new().markdown(source);
            self.parsed_markdown
                .with(|cache| cache.insert(source.to_owned(), doc.clone()));
            return doc;
        }
        nsh_base::text_view::Parser::new().markdown(source)
    }
    pub fn cached_report_body(self, card_id: &str, path: &str, min_version: u64) -> Option<String> {
        let key = (card_id.to_owned(), path.to_owned());
        self.report_bodies.with(|cache| {
            cache
                .get(&key)
                .filter(|entry| entry.version >= min_version)
                .map(|entry| entry.body.clone())
        })
    }
    pub fn store_report_body(self, card_id: String, path: String, version: u64, body: String) {
        self.report_bodies
            .with(|cache| cache.insert((card_id, path), CachedReportBody { version, body }));
    }
    pub fn prefetch_active_reports(self) {
        if !crate::backend::is_connected() {
            return;
        }
        let project = self.project.get_untracked();
        let pending_keys = self.snapshot.with_untracked(|s| {
            self.index.with_untracked(|idx| {
                let mut candidates: Vec<(u64, String, String, u64)> = Vec::new();
                for report in &s.reports {
                    let Some(path) = report.path.as_deref() else {
                        continue;
                    };
                    let Some(card) = idx.cards.get(&report.card_id) else {
                        continue;
                    };
                    if !card.active() || (!project.is_empty() && card.project_id != project) {
                        continue;
                    }
                    candidates.push((
                        report.created_at,
                        card.id.clone(),
                        path.to_owned(),
                        report.created_at,
                    ));
                }
                candidates.sort_by_key(|(created_at, _, _, _)| std::cmp::Reverse(*created_at));
                let mut selected = Vec::new();
                let mut seen = BTreeSet::new();
                for (_, cid, path, ver) in candidates {
                    if selected.len() >= PREFETCH_REPORT_BATCH_LIMIT {
                        break;
                    }
                    let key = (cid, path);
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let already_cached = self
                        .report_bodies
                        .with(|cache| cache.peek(&key).is_some_and(|entry| entry.version >= ver));
                    if already_cached {
                        continue;
                    }
                    let newly_inflight = self.report_inflight.with(|set| set.insert(key.clone()));
                    if newly_inflight {
                        selected.push((key.0, key.1, ver));
                    }
                }
                selected
            })
        });
        if pending_keys.is_empty() {
            return;
        }
        self.spawn(async move {
            let loaded = blocking(move || {
                let mut out = Vec::with_capacity(pending_keys.len());
                for (cid, path, ver) in pending_keys {
                    let res = crate::routed_call(
                        "resource.read",
                        json!({"card_id": cid, "path": path}),
                        None,
                    );
                    let body = res
                        .ok()
                        .and_then(|v| v["body"].as_str().or(v["text"].as_str()).map(str::to_owned));
                    out.push((cid, path, ver, body));
                }
                out
            })
            .await;
            if let Ok(items) = loaded {
                for (cid, path, ver, body) in items {
                    self.report_inflight.with(|set| {
                        set.remove(&(cid.clone(), path.clone()));
                    });
                    if let Some(text) = body {
                        self.store_report_body(cid, path, ver, text);
                    }
                }
            }
        });
    }
    pub fn open_card(self, id: String) {
        let project = self
            .index
            .with_untracked(|idx| idx.cards.get(&id).map(|c| c.project_id.clone()));
        let recent = self.terminal_recent.with(|m| m.get(&id).cloned());
        let default_tab = untracked(|| {
            crate::terminals::default_card_terminal_for_ui(self, &id, recent.as_deref())
        })
        .unwrap_or_default();
        if !default_tab.is_empty() {
            self.terminal_recent.with(|m| {
                m.insert(id.clone(), default_tab.clone());
            });
        }
        let timeline_id = id.clone();
        batch(move || {
            self.agent_popover.close();
            if self.card.get_untracked() != id {
                self.editing_comment.set(None);
                self.deleting_comment.set(None);
            }
            self.request.set(String::new());
            self.card_event.set(None);
            self.card_terminal_focused.set(false);
            self.card_standalone.set(
                matches!(
                    self.page.get_untracked().as_str(),
                    "terminals" | "overview" | "inbox" | "insights"
                ) || self.card_standalone.get_untracked()
                    && matches!(self.page.get_untracked().as_str(), "card" | "review"),
            );
            self.review.set(None);
            self.review_request
                .set(self.review_request.get_untracked() + 1);
            if let Some(project) = project {
                self.project.set(project);
            }
            self.card_tab.set(default_tab);
            self.card_terminal_open.set(false);
            self.card.set(id);
            self.expanded.set(false);
            self.focused_card.set(false);
            self.output.set(String::new());
            self.output_observed_at.set(None);
            self.output_request
                .set(self.output_request.get_untracked() + 1);
            self.page.set("card".into());
        });
        self.ensure_card_timeline(timeline_id);
    }

    pub fn card_timeline_events(self, card_id: &str) -> Rc<Vec<Event>> {
        self.card_timelines.with(|timelines| {
            Rc::new(
                timelines
                    .get(card_id)
                    .map(|state| state.events.clone())
                    .unwrap_or_default(),
            )
        })
    }

    pub fn card_timeline_state(self, card_id: &str) -> CardTimelineState {
        self.card_timelines
            .with(|timelines| timelines.get(card_id).cloned().unwrap_or_default())
    }

    pub fn overlay_card_timeline_writes(self, card_id: &str) {
        if card_id.is_empty() {
            return;
        }
        self.committed_writes.with(|writes| {
            self.card_timelines.update(|timelines| {
                let entry = timelines.entry(card_id.to_owned()).or_default();
                writes.overlay_card_events(card_id, &mut entry.events);
                if let Some(first) = entry.events.first() {
                    entry.oldest_seq = Some(
                        entry
                            .oldest_seq
                            .map_or(first.sequence, |old| old.min(first.sequence)),
                    );
                }
            });
        });
    }

    pub fn ensure_card_timeline(self, card_id: String) {
        if card_id.is_empty() || !crate::backend::is_connected() {
            return;
        }
        let existing = self
            .card_timelines
            .with_untracked(|m| m.get(&card_id).cloned())
            .unwrap_or_default();
        if existing.loading {
            return;
        }
        let project_id = self
            .index
            .with_untracked(|idx| idx.cards.get(&card_id).map(|c| c.project_id.clone()));
        if !existing.initialized {
            self.card_timelines.update(|m| {
                m.entry(card_id.clone()).or_default().loading = true;
            });
            let cid = card_id.clone();
            self.spawn(async move {
                let cid_call = cid.clone();
                let res = blocking(move || {
                    crate::routed_call(
                        "timeline.list",
                        json!({"card_id": cid_call, "tail": true, "limit": 200}),
                        project_id.as_deref(),
                    )
                })
                .await;
                match res {
                    Ok(Ok(val)) => {
                        let parsed = parse_timeline_page(&val);
                        self.committed_writes.with(|writes| {
                            self.card_timelines.update(|m| {
                                let state = m.entry(cid.clone()).or_default();
                                apply_tail_page(
                                    state,
                                    parsed.events,
                                    parsed.highwater,
                                    parsed.has_more_before,
                                    parsed.oldest_sequence,
                                    writes,
                                    &cid,
                                );
                            });
                        });
                        if let Some(target_seq) = self.card_event.get_untracked() {
                            let needs_around = self.card_timelines.with_untracked(|m| {
                                m.get(&cid).is_some_and(|st| {
                                    st.oldest_seq.is_some_and(|oldest| target_seq < oldest)
                                        && !st.loading_older
                                })
                            });
                            if needs_around {
                                self.load_card_timeline_around(cid, target_seq);
                            }
                        }
                    }
                    _ => {
                        self.card_timelines.update(|m| {
                            if let Some(st) = m.get_mut(&cid) {
                                st.loading = false;
                            }
                        });
                    }
                }
            });
            return;
        }

        let snapshot_last_seq = self
            .index
            .with_untracked(|idx| idx.last_event_by_card.get(&card_id).copied().unwrap_or(0));
        if snapshot_last_seq > existing.highwater {
            let after_seq = existing.highwater;
            self.card_timelines.update(|m| {
                m.entry(card_id.clone()).or_default().loading = true;
            });
            let cid = card_id.clone();
            self.spawn(async move {
                let cid_call = cid.clone();
                let res = blocking(move || {
                    crate::routed_call(
                        "timeline.list",
                        json!({"card_id": cid_call, "after_sequence": after_seq, "limit": 200}),
                        project_id.as_deref(),
                    )
                })
                .await;
                match res {
                    Ok(Ok(val)) => {
                        let parsed = parse_timeline_page(&val);
                        self.committed_writes.with(|writes| {
                            self.card_timelines.update(|m| {
                                let state = m.entry(cid.clone()).or_default();
                                apply_incremental_page(
                                    state,
                                    parsed.events,
                                    parsed.highwater,
                                    writes,
                                    &cid,
                                );
                            });
                        });
                    }
                    _ => {
                        self.card_timelines.update(|m| {
                            if let Some(st) = m.get_mut(&cid) {
                                st.loading = false;
                            }
                        });
                    }
                }
            });
        }

        if let Some(target_seq) = self.card_event.get_untracked()
            && existing
                .oldest_seq
                .is_some_and(|oldest| target_seq < oldest)
            && !existing.loading_older
        {
            self.load_card_timeline_around(card_id, target_seq);
        }
    }

    pub fn load_older_card_timeline(self, card_id: String) {
        if card_id.is_empty() || !crate::backend::is_connected() {
            return;
        }
        let existing = self
            .card_timelines
            .with_untracked(|m| m.get(&card_id).cloned())
            .unwrap_or_default();
        let Some(before_seq) = existing.oldest_seq else {
            return;
        };
        if !existing.has_more_before || existing.loading_older {
            return;
        }
        let project_id = self
            .index
            .with_untracked(|idx| idx.cards.get(&card_id).map(|c| c.project_id.clone()));
        self.card_timelines.update(|m| {
            m.entry(card_id.clone()).or_default().loading_older = true;
        });
        let cid = card_id.clone();
        self.spawn(async move {
            let cid_call = cid.clone();
            let res = blocking(move || {
                crate::routed_call(
                    "timeline.list",
                    json!({"card_id": cid_call, "before_sequence": before_seq, "limit": 200}),
                    project_id.as_deref(),
                )
            })
            .await;
            match res {
                Ok(Ok(val)) => {
                    let parsed = parse_timeline_page(&val);
                    self.committed_writes.with(|writes| {
                        self.card_timelines.update(|m| {
                            let state = m.entry(cid.clone()).or_default();
                            apply_older_page(
                                state,
                                parsed.events,
                                parsed.has_more_before,
                                parsed.oldest_sequence,
                                writes,
                                &cid,
                            );
                        });
                    });
                }
                _ => {
                    self.card_timelines.update(|m| {
                        if let Some(st) = m.get_mut(&cid) {
                            st.loading_older = false;
                        }
                    });
                }
            }
        });
    }

    pub fn load_card_timeline_around(self, card_id: String, target_seq: u64) {
        if card_id.is_empty() || !crate::backend::is_connected() {
            return;
        }
        let existing = self
            .card_timelines
            .with_untracked(|m| m.get(&card_id).cloned())
            .unwrap_or_default();
        if existing.loading_older {
            return;
        }
        let project_id = self
            .index
            .with_untracked(|idx| idx.cards.get(&card_id).map(|c| c.project_id.clone()));
        let after_seq = target_seq.saturating_sub(20);
        self.card_timelines.update(|m| {
            m.entry(card_id.clone()).or_default().loading_older = true;
        });
        let cid = card_id.clone();
        self.spawn(async move {
            let cid_call = cid.clone();
            let res = blocking(move || {
                crate::routed_call(
                    "timeline.list",
                    json!({"card_id": cid_call, "after_sequence": after_seq, "limit": 200}),
                    project_id.as_deref(),
                )
            })
            .await;
            match res {
                Ok(Ok(val)) => {
                    let parsed = parse_timeline_page(&val);
                    self.committed_writes.with(|writes| {
                        self.card_timelines.update(|m| {
                            let state = m.entry(cid.clone()).or_default();
                            apply_around_page(
                                state,
                                parsed.events,
                                parsed.has_more_before,
                                writes,
                                &cid,
                            );
                        });
                    });
                }
                _ => {
                    self.card_timelines.update(|m| {
                        if let Some(st) = m.get_mut(&cid) {
                            st.loading_older = false;
                        }
                    });
                }
            }
        });
    }
    pub fn open_card_with_terminal(self, card_id: String, conversation: Option<String>) {
        if let Some(conv) = &conversation {
            self.terminal_recent.with(|recent| {
                recent.insert(card_id.clone(), conv.clone());
            });
            self.terminal_tabs.update(|tabs| {
                if !tabs.contains(conv) {
                    tabs.push(conv.clone());
                }
            });
        }
        let open_existing = batch(|| {
            self.open_card(card_id.clone());
            if let Some(ref conv) = conversation {
                let is_live =
                    untracked(|| crate::terminals::card_live_entries_for_ui(self, &card_id))
                        .iter()
                        .any(|e| &e.saved.id == conv);
                if is_live {
                    self.card_tab.set(conv.clone());
                }
            }
            if !self.card_tab.get_untracked().is_empty() {
                self.card_terminal_open.set(true);
                self.card_terminal_focused.set(true);
                self.terminal_focus.update(|v| *v += 1);
                true
            } else {
                false
            }
        });
        if open_existing {
            return;
        }
        if let Some(c) = self
            .index
            .with_untracked(|idx| idx.cards.get(&card_id).cloned())
            .filter(|c| c.archived_at.is_none())
        {
            crate::terminals::create_in_card(self, c.project_id, c.id, false);
        }
    }
    pub fn back(self) {
        crate::navigation::go(self, false);
    }
    pub fn is_method_pending(self, method: &'static str) -> bool {
        self.pending_methods.with(|s| s.contains(method))
    }
    pub fn is_card_merging(self, snapshot: &Snapshot, card: &Card) -> bool {
        crate::components::activity::is_merging(snapshot, card)
            || card.active()
                && !card.resolved()
                && self.merging_cards.with(|ids| ids.contains(&card.id))
    }
    pub fn notify(self, message: impl Into<String>) {
        let message = message.into();
        self.status.set(message.clone());
        if message.trim().is_empty() {
            self.toast.set(String::new());
            return;
        }
        let next = self.toast_epoch.get_untracked() + 1;
        self.toast_epoch.set(next);
        self.toast.set(message);
        self.owner.run(|| {
            set_timeout(std::time::Duration::from_millis(2500), move || {
                if self.toast_epoch.try_get() == Some(next) {
                    self.toast.try_set(String::new());
                }
            });
        });
    }
}
/// Pure projection for `Ui::accept_runtime_state`: applies a lightweight runtime slice,
/// reconciles runtime reminders and terminal receipts, and returns `None` if the runtime
/// references an unknown Card or Project (signaling the caller to escalate to full `app.state`).
pub fn apply_runtime_state_to_snapshot(
    current: &Snapshot,
    runtime: RuntimeState,
    writes: &mut submission::CommittedWrites,
) -> Option<Snapshot> {
    apply_runtime_state_to_snapshot_at(current, runtime, writes, canban_types::now())
}

/// Deterministic clock-injected variant of [`apply_runtime_state_to_snapshot`].
pub fn apply_runtime_state_to_snapshot_at(
    current: &Snapshot,
    runtime: RuntimeState,
    writes: &mut submission::CommittedWrites,
    now_ms: u64,
) -> Option<Snapshot> {
    let mut next = current.clone();
    if runtime_references_unknown_entity(&next, &runtime) {
        return None;
    }
    next.apply_runtime_state(runtime);
    canban_notifications::reconcile_runtime_reminders_at(&mut next, now_ms);
    writes.reconcile_runtime(&mut next);
    Some(next)
}

/// Pure projection for `Ui::apply_hosts`: bumps per-remote-host connect epochs strictly when a
/// remote host transitions to `Connected` or increments `reconnect_count`, returning bumped IDs.
pub fn apply_host_summaries(
    prev: &[HostSummary],
    summaries: &[HostSummary],
    epochs: &mut BTreeMap<String, u64>,
) -> Vec<String> {
    if prev == summaries {
        return Vec::new();
    }
    let mut to_bump = Vec::new();
    for h in summaries {
        if h.is_local || !h.connection.is_connected() {
            continue;
        }
        let reconnected = prev.iter().find(|old| old.id == h.id).is_none_or(|old| {
            !old.connection.is_connected() || old.reconnect_count != h.reconnect_count
        });
        if reconnected {
            to_bump.push(h.id.clone());
        }
    }
    for hid in &to_bump {
        let next = epochs.get(hid).copied().unwrap_or(0).saturating_add(1);
        epochs.insert(hid.clone(), next);
    }
    to_bump
}

fn runtime_references_unknown_entity(snapshot: &Snapshot, runtime: &RuntimeState) -> bool {
    let has_card = |cid: &str| snapshot.cards.iter().any(|c| c.id == cid);
    let has_project = |pid: &str| {
        canban_types::is_global_project_id(pid) || snapshot.projects.iter().any(|p| p.id == pid)
    };
    runtime.terminals.iter().any(|t| {
        t.agent.as_ref().is_some_and(|a| {
            !has_card(&a.card_id) || (!a.project_id.is_empty() && !has_project(&a.project_id))
        }) || t
            .origin_card_id
            .as_deref()
            .is_some_and(|cid| !has_card(cid))
    }) || runtime.terminal_contexts.iter().any(|ctx| {
        ctx.card_id.as_deref().is_some_and(|cid| !has_card(cid))
            || ctx
                .project_id
                .as_deref()
                .is_some_and(|pid| !has_project(pid))
    })
}

fn finish_refresh(ui: Ui) {
    ui.refreshing.set(false);
    let next = ui.refresh_pending.get_untracked();
    ui.refresh_pending.set(RefreshPending::None);
    match next {
        RefreshPending::Full => refresh(ui),
        RefreshPending::Runtime => refresh_runtime(ui),
        RefreshPending::None => {}
    }
}

pub fn refresh(ui: Ui) {
    if ui.refreshing.get_untracked() {
        ui.refresh_pending.set(RefreshPending::Full);
        return;
    }
    let Some(backend) = crate::backend::get() else {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    };
    ui.refreshing.set(true);
    ui.refresh_pending.set(RefreshPending::None);
    let epoch = ui.snapshot_epoch.get_untracked();
    let restore_project = ui.project.get_untracked().is_empty();
    let cfg_host = ui.effective_config_host();
    ui.spawn(async move {
        let result = blocking(move || {
            let snapshot = crate::merged_snapshot()?;
            let summaries = snapshot_summaries(&snapshot);
            let saved_project = if restore_project {
                backend
                    .call_local("utilities.state", json!({}))
                    .ok()
                    .and_then(|state| {
                        state["preferences"]["last_project"]
                            .as_str()
                            .map(str::to_owned)
                    })
            } else {
                None
            };
            Ok::<_, anyhow::Error>((
                snapshot,
                crate::config_for_host(&cfg_host),
                summaries,
                saved_project,
            ))
        })
        .await;
        if ui.snapshot_epoch.get_untracked() != epoch {
            ui.refreshing.set(false);
            refresh(ui);
            return;
        }
        match result {
            Ok(Ok((mut snapshot, config, summaries, saved_project))) => {
                let _span = canban_profiler::Span::new("ui.apply");
                ui.committed_writes
                    .with(|writes| writes.reconcile(&mut snapshot));
                if ui.notification_resync.replace(false) {
                    ui.notification_delivery
                        .with(|t| t.baseline(&canban_notifications::sound_items(&snapshot)));
                }
                let different = ui.snapshot.with_untracked(|old| old != &snapshot);
                batch(move || {
                    let cur_project = ui.project.get_untracked();
                    let restored_remote = if cur_project.is_empty() {
                        None
                    } else {
                        ui.pending_remote_project.with(|slot| {
                            crate::projects::selection::PendingRemoteRestore::resolve(
                                slot,
                                &cur_project,
                                &snapshot.projects,
                                &summaries,
                            )
                        })
                    };
                    if !summaries.is_empty() {
                        ui.apply_hosts(summaries);
                    }
                    ui.apply_config(&config);
                    if cur_project.is_empty() {
                        let restored = crate::projects::selection::restore(
                            &snapshot.projects,
                            saved_project.as_deref(),
                        );
                        ui.pending_remote_project.replace(
                            crate::projects::selection::PendingRemoteRestore::from_bootstrap(
                                &snapshot.projects,
                                saved_project.as_deref(),
                                restored.as_deref(),
                            ),
                        );
                        if let Some(id) = restored {
                            ui.project.set(id);
                        }
                    } else if let Some(restored_remote) = restored_remote {
                        ui.project.set(restored_remote);
                    } else if !snapshot.projects.iter().any(|p| p.id == cur_project) {
                        let next = crate::projects::selection::restore(
                            &snapshot.projects,
                            saved_project.as_deref(),
                        )
                        .unwrap_or_default();
                        ui.project.set(next);
                        ui.card.set(String::new());
                        ui.review.set(None);
                        ui.card_standalone.set(false);
                        if matches!(
                            ui.page.get_untracked().as_str(),
                            "card"
                                | "new-card"
                                | "project-location"
                                | "project-agents"
                                | "github-repository"
                                | "workflows"
                        ) {
                            ui.page.set("board".into());
                        }
                    }
                    if different {
                        ui.runtime_clock.set(now());
                        ui.snapshot.set(snapshot);
                    }
                    ui.snapshot_ready.set(true);
                });
                if different {
                    ui.prefetch_active_reports();
                    let open_card = ui.card.get_untracked();
                    if !open_card.is_empty() {
                        ui.ensure_card_timeline(open_card);
                    }
                }
            }
            Ok(Err(e)) => ui.operation_error.set(format!("Could not refresh: {e:#}")),
            Err(e) => ui.operation_error.set(format!("Refresh worker: {e}")),
        }
        finish_refresh(ui);
    });
}

pub fn refresh_runtime(ui: Ui) {
    if !ui.snapshot_ready.get_untracked() {
        refresh(ui);
        return;
    }
    if ui.refreshing.get_untracked() {
        ui.refresh_pending.update(|pending| {
            if *pending == RefreshPending::None {
                *pending = RefreshPending::Runtime;
            }
        });
        return;
    }
    if crate::backend::get().is_none() {
        return;
    }
    ui.refreshing.set(true);
    ui.refresh_pending.set(RefreshPending::None);
    let epoch = ui.snapshot_epoch.get_untracked();
    ui.spawn(async move {
        let result = blocking(crate::merged_runtime_state).await;
        if ui.snapshot_epoch.get_untracked() != epoch {
            ui.refresh_pending.update(|pending| {
                if *pending == RefreshPending::None {
                    *pending = RefreshPending::Runtime;
                }
            });
            finish_refresh(ui);
            return;
        }
        match result {
            Ok(Ok(Some(runtime))) => {
                if !ui.accept_runtime_state(runtime) {
                    ui.refresh_pending.set(RefreshPending::Full);
                }
            }
            Ok(Ok(None)) | Ok(Err(_)) => {
                ui.refresh_pending.set(RefreshPending::Full);
            }
            Err(e) => ui
                .operation_error
                .set(format!("Runtime refresh worker: {e}")),
        }
        finish_refresh(ui);
    });
}

fn snapshot_summaries(snapshot: &Snapshot) -> Vec<HostSummary> {
    let mut summaries: Vec<_> = snapshot.host_summaries.values().cloned().collect();
    summaries.sort_by_key(|s| !s.is_local);
    summaries
}
pub fn dispatch(ui: Ui, method: &'static str, params: Value) {
    dispatch_then(ui, method, params, |_| {});
}
pub fn dispatch_then(
    ui: Ui,
    method: &'static str,
    params: Value,
    after: impl FnOnce(Value) + 'static,
) {
    dispatch_complete(ui, method, params, move |result| {
        if let Ok(value) = result {
            after(value);
        }
    });
}
pub fn dispatch_complete(
    ui: Ui,
    method: &'static str,
    mut params: Value,
    after: impl FnOnce(Result<Value, String>) + 'static,
) {
    if ui.pending_methods.with_untracked(|s| s.contains(method)) {
        after(Err(
            "This operation is already in progress; your draft is retained".into(),
        ));
        return;
    }
    if !crate::backend::is_connected() {
        after(Err("Gallery preview: no AppServer connected".into()));
        return;
    }
    ui.pending_methods.update(|s| {
        s.insert(method);
    });
    let merging_card = matches!(
        method,
        "review.merge" | "github.pr.merge" | "workflow.deliver"
    )
    .then(|| params["card_id"].as_str().map(str::to_owned))
    .flatten();
    if let Some(ref cid) = merging_card {
        ui.merging_cards.update(|ids| {
            ids.insert(cid.clone());
        });
    }
    ui.pending.set(true);
    ui.operation_error.set(String::new());
    ui.status.set(format!("{method}…"));
    if params.get("request_key").is_none() {
        params["request_key"] = json!(id());
    }
    if params.get("host_id").is_none()
        && matches!(
            method,
            "config.agents.save"
                | "config.reload"
                | "agent.setup"
                | "scheduler.configure"
                | "host.env.refresh"
                | "herdr.status"
                | "herdr.check"
                | "project.archived.list"
                | "project.restore"
        )
    {
        params["host_id"] = json!(ui.effective_config_host());
    }
    let is_terminal_env_save =
        method == "config.agents.save" && params["op"].as_str() == Some("set_terminal_env_mode");
    let source = ui.location();
    let active_project = ui.project.get_untracked();
    let target_host =
        crate::backend::resolve_target_host(method, &params, Some(active_project.as_str()));
    let cfg_host = ui.effective_config_host();
    let config_only = submission::config_only(method);
    let immediate = submission::immediate(method);
    let skip_snapshot = immediate || config_only;
    let terminal_launch = (method == "terminal.create").then(|| params.clone());
    let epoch = ui.snapshot_epoch.get_untracked();
    ui.spawn(async move {
        let result = blocking(move || {
            let value = crate::routed_call(method, params, Some(active_project.as_str()))?;
            let refreshed = if skip_snapshot {
                None
            } else {
                Some((crate::merged_snapshot()?, crate::config_for_host(&cfg_host)))
            };
            Ok::<_, anyhow::Error>((value, refreshed))
        })
        .await;
        ui.pending_methods.update(|s| {
            s.remove(method);
        });
        if let Some(ref cid) = merging_card {
            ui.merging_cards.update(|ids| {
                ids.remove(cid);
            });
        }
        ui.pending
            .set(!ui.pending_methods.with_untracked(BTreeSet::is_empty));
        match result {
            Ok(Ok((value, refreshed))) => {
                let stale = ui.snapshot_epoch.get_untracked() != epoch;
                batch(|| {
                    if !config_only {
                        ui.snapshot_epoch.update(|v| *v += 1);
                        ui.committed_writes
                            .with(|writes| writes.record(method, &value, &target_host));
                    }
                    if let Some(params) = terminal_launch.as_ref()
                        && let Ok(terminal) = serde_json::from_value(value.clone())
                        && let Some(hint) = ui.snapshot.with_untracked(|s| {
                            ui.terminal_entries.with_untracked(|rows| {
                                crate::terminals::creation::location_hint(
                                    s,
                                    rows,
                                    params,
                                    &terminal,
                                    &target_host,
                                )
                            })
                        })
                    {
                        ui.committed_writes
                            .with(|writes| writes.record_terminal_hint(hint));
                    }
                    if config_only {
                        match method {
                            "config.agents.save" | "agent.setup" => {
                                if let Ok(cfg) = serde_json::from_value::<Config>(value.clone()) {
                                    ui.apply_config(&cfg);
                                }
                            }
                            "scheduler.configure" => {
                                if let Ok(settings) =
                                    serde_json::from_value::<SchedulerSettings>(value.clone())
                                {
                                    ui.max_agents.set(settings.max_agents);
                                    ui.idle_minutes.set(settings.idle_minutes);
                                }
                            }
                            "utilities.configure" | "utilities.awake" => {
                                ui.utility_epoch.update(|n| *n += 1);
                                ui.utility.set(value.clone());
                            }
                            _ => {}
                        }
                    } else if let Some((mut snapshot, config)) = refreshed.filter(|_| !stale) {
                        ui.committed_writes
                            .with(|writes| writes.reconcile(&mut snapshot));
                        let summaries = snapshot_summaries(&snapshot);
                        if !summaries.is_empty() {
                            ui.apply_hosts(summaries);
                        }
                        ui.apply_config(&config);
                        ui.snapshot.set(snapshot);
                    } else if immediate {
                        ui.committed_writes.with(|writes| {
                            ui.snapshot.update(|snapshot| writes.overlay(snapshot));
                        });
                    }
                    ui.runtime_clock.set(now());
                });
                if !config_only
                    && matches!(method, "comment.add" | "comment.update" | "comment.delete")
                {
                    let cid = value["card_id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| ui.card.get_untracked());
                    if !cid.is_empty() {
                        ui.overlay_card_timeline_writes(&cid);
                    }
                }
                if !skip_snapshot && !stale {
                    ui.prefetch_active_reports();
                    let open_card = ui.card.get_untracked();
                    if !open_card.is_empty() {
                        ui.ensure_card_timeline(open_card);
                    }
                }
                let failed = matches!(value["state"].as_str(), Some("failed" | "unknown"));
                if failed {
                    let msg = format_incomplete_outcome(method, &value);
                    ui.status.set(msg.clone());
                    ui.operation_error.set(msg);
                } else if value["offline_queued"] == true {
                    let host_name = crate::hosts::host_name(ui, &target_host);
                    ui.notify(format!("Saved offline · will sync when {host_name} reconnects"));
                } else {
                    match method {
                        "herdr.status" => ui.notify(format!(
                            "Herdr connected · core {}",
                            value["core"]["id"].as_str().unwrap_or("")
                        )),
                        "config.reload" => ui.notify("Configuration reloaded"),
                        "agent.setup" => ui.notify("Agent setup saved"),
                        "config.agents.save" => ui.notify(if is_terminal_env_save {
                            "Terminal environment settings saved"
                        } else {
                            "Agent settings saved"
                        }),
                        "host.env.refresh" => {
                            let shell = value["shell"].as_str().unwrap_or("shell");
                            let var_count = value["var_count"].as_u64().unwrap_or(0);
                            let full = value["full_env_captured"].as_bool().unwrap_or(false);
                            if full {
                                ui.notify(format!(
                                    "Refreshed terminal environment ({var_count} vars from {shell})"
                                ));
                            } else {
                                ui.notify(format!(
                                    "Refreshed terminal PATH from {shell} (full environment unavailable)"
                                ));
                            }
                        }
                        "project.agents" => ui.notify("Project Agents settings saved"),
                        "env.save" => ui.notify("Environment & Secrets saved"),
                        "scheduler.configure" => ui.notify("Scheduling settings saved"),
                        "project.update" => ui.notify("Project updated"),
                        "project.archive" => {
                            let name = value["name"].as_str().unwrap_or("Project");
                            ui.notify(format!(
                                "Archived project \"{name}\" · restore in Settings → Storage"
                            ));
                        }
                        "project.restore" => {
                            let name = value["name"].as_str().unwrap_or("Project");
                            let has_missing_secrets = value["missing_secrets"]
                                .as_array()
                                .is_some_and(|a| !a.is_empty());
                            let has_missing_profiles = value["missing_profiles"]
                                .as_array()
                                .is_some_and(|a| !a.is_empty());
                            if has_missing_secrets || has_missing_profiles {
                                ui.notify(format!(
                                    "Restored project \"{name}\" · reconfigure missing secrets/profiles in Settings"
                                ));
                            } else {
                                ui.notify(format!("Restored project \"{name}\""));
                            }
                        }
                        "project.delete" => {
                            let name = value["name"].as_str().unwrap_or("Project");
                            ui.notify(format!("Deleted project \"{name}\""));
                        }
                        "project.github" => {
                            if let Some(repo) = value["github"]["repository"].as_str() {
                                ui.notify(format!("Linked to {repo}"));
                            } else {
                                ui.notify("Repository settings saved");
                            }
                        }
                        "github.sync.request" => ui.notify("GitHub synchronization queued"),
                        "github.source.disconnect" => ui.notify("GitHub repository disconnected"),
                        "workflow.save" => ui.notify("Workflow saved"),
                        "workflow.default.set" => {
                            ui.notify("Saved as default workflow for new Projects")
                        }
                        "workflow.default.reset" => {
                            ui.notify("Reset new Project workflow to built-in default")
                        }
                        "review.merge" if value["state"] == "merged" => ui.notify(format!(
                            "Merged into {}",
                            value["target_branch"].as_str().unwrap_or("target branch")
                        )),
                        "utilities.awake" => ui.notify(if value["awake"] == true {
                            "Host awake enabled"
                        } else {
                            "Host awake disabled"
                        }),
                        "utilities.configure"
                        | "notifications.read"
                        | "notifications.read_all"
                        | "comment.add"
                        | "comment.update"
                        | "card.move"
                        | "project.create"
                        | "tag.create" => ui.status.set("Saved".into()),
                        "card.create" | "github.issue.add" => {
                            if let Some(number) = value["number"].as_u64() {
                                ui.notify(format!("Created Card #{number}"));
                            } else {
                                ui.notify("Card created");
                            }
                        }
                        "worktree.create" => ui.notify("Managed worktree created"),
                        "worktree.promote" => ui.notify("Promoted worktree to Card"),
                        "worktree.remove" => ui.notify("Worktree deleted"),
                        "git.stage" => ui.notify("Staged changes"),
                        "git.unstage" => ui.notify("Unstaged changes"),
                        "git.discard" => ui.notify("Discarded local changes"),
                        "git.commit" => {
                            let short = value["short_sha"].as_str().unwrap_or("");
                            if value["pushed"] == true {
                                ui.notify(format!("Committed {short} & pushed"));
                            } else if value["amended"] == true {
                                ui.notify(format!("Amended commit {short}"));
                            } else {
                                ui.notify(format!("Committed {short}"));
                            }
                        }
                        "git.fetch" => {
                            let r = value["remote"].as_str().unwrap_or("remote");
                            ui.notify(format!("Fetched from {r}"));
                        }
                        "git.pull" => {
                            if value["fast_forwarded"] == true {
                                ui.notify("Fast-forwarded current branch");
                            } else {
                                ui.notify("Already up to date");
                            }
                        }
                        "git.push" => {
                            let r = value["remote"].as_str().unwrap_or("remote");
                            let b = value["branch"].as_str().unwrap_or("branch");
                            ui.notify(format!("Pushed {b} to {r}"));
                        }
                        "git.sync" => {
                            ui.notify(crate::terminals::worktrees::format_git_sync_summary(
                                &value, "branch",
                            ));
                        }
                        _ => ui.notify("Saved"),
                    }
                }
                let active = ui.location() == source;
                match method {
                    "project.create" if active => {
                        if let Some(id) = value["id"].as_str() {
                            ui.project.set(id.into());
                            ui.card.set(String::new());
                            ui.page.set("board".into());
                        }
                    }
                    "project.archive" => {
                        ui.archived_projects_reload.update(|n| *n += 1);
                        if let Some(id) = value["project_id"].as_str() {
                            crate::projects::after_archive(ui, id);
                        }
                    }
                    "project.restore" => {
                        ui.archived_projects_reload.update(|n| *n += 1);
                        ui.project_restore_modal.set(None);
                    }
                    "project.delete" => {
                        if let Some(id) = value["project_id"].as_str() {
                            crate::projects::after_archive(ui, id);
                        }
                    }
                    "card.create" | "github.issue.add" | "github.item.add" if active => {
                        if let Some(id) = value["id"].as_str() {
                            ui.open_card(id.into());
                            if source.page == "new-card" {
                                crate::navigation::replace(ui);
                            }
                        }
                    }
                    "card.archive" if active => crate::card_actions::after_archive(ui),
                    "review.merge"
                    | "github.pr.merge"
                    | "github.pr.refresh"
                    | "workflow.deliver"
                    | "workflow.decide"
                    | "card.hold_auto_merge"
                        if active && ui.current_card().is_some_and(|c| c.archived_at.is_some()) =>
                    {
                        ui.review.set(None);
                        ui.card.set(String::new());
                        ui.card_standalone.set(false);
                        ui.page.set("board".into());
                    }
                    "card.remove" if active => {
                        ui.card.set(String::new());
                        ui.page.set("board".into());
                    }
                    _ => {}
                }
                after(Ok(value));
                if immediate || (!config_only && stale) {
                    refresh(ui);
                }
            }
            Ok(Err(e)) => {
                let error = format!("{e:#}");
                ui.status.set(error.clone());
                ui.operation_error.set(error.clone());
                after(Err(error));
            }
            Err(e) => {
                let error = e.to_string();
                ui.status.set(error.clone());
                ui.operation_error.set(error.clone());
                after(Err(error));
            }
        }
    });
}
pub fn move_card(ui: Ui, card_id: &str, target: &str) {
    let card = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned());
    if let Some(c) = card {
        let prev = c.clone();
        let target_stage = target.to_owned();
        if !crate::backend::is_connected() {
            ui.snapshot.update(|s| {
                if let Some(cur) = s.cards.iter_mut().find(|x| x.id == prev.id) {
                    cur.stage = target_stage;
                    cur.attention = None;
                    cur.human_gate = None;
                    cur.revision += 1;
                }
            });
            return;
        }
        if ui.is_method_pending("card.move") {
            return;
        }
        ui.snapshot.update(|s| {
            if let Some(cur) = s.cards.iter_mut().find(|x| x.id == prev.id) {
                cur.stage = target_stage;
                cur.attention = None;
                cur.human_gate = None;
            }
        });
        dispatch_complete(
            ui,
            "card.move",
            json!({"card_id":c.id,"revision":c.revision,"stage":target}),
            move |res| {
                if res.is_err() {
                    ui.snapshot.update(|s| {
                        if let Some(cur) = s.cards.iter_mut().find(|x| x.id == prev.id)
                            && cur.revision == prev.revision
                        {
                            *cur = prev;
                        }
                    });
                }
            },
        );
    }
}
pub fn hold_auto_merge(ui: Ui, card_id: &str, fallback_revision: u64, hold: bool) {
    let prev = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned());
    let revision = prev
        .as_ref()
        .map(|c| c.revision)
        .unwrap_or(fallback_revision);
    if !crate::backend::is_connected() {
        ui.snapshot.update(|s| {
            if let Some(cur) = s.cards.iter_mut().find(|x| x.id == card_id) {
                cur.hold_auto_merge = hold;
                cur.revision += 1;
            }
        });
        return;
    }
    if ui.is_method_pending("card.hold_auto_merge") {
        return;
    }
    ui.snapshot.update(|s| {
        if let Some(cur) = s.cards.iter_mut().find(|x| x.id == card_id) {
            cur.hold_auto_merge = hold;
        }
    });
    let cid = card_id.to_owned();
    dispatch_complete(
        ui,
        "card.hold_auto_merge",
        json!({"card_id":cid,"revision":revision,"hold":hold}),
        move |res| {
            if res.is_err()
                && let Some(prev_card) = prev
            {
                ui.snapshot.update(|s| {
                    if let Some(cur) = s.cards.iter_mut().find(|x| x.id == prev_card.id)
                        && cur.revision == prev_card.revision
                    {
                        *cur = prev_card;
                    }
                });
            }
        },
    );
}
pub fn assign_card(ui: Ui, card_id: &str, assignee: Option<&str>) {
    let prev = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned());
    let Some(prev_card) = prev else {
        return;
    };
    let next_assignee = assignee
        .map(str::trim)
        .map(|s| s.trim_start_matches('@'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if !crate::backend::is_connected() {
        ui.snapshot.update(|s| {
            if let Some(cur) = s.cards.iter_mut().find(|x| x.id == card_id) {
                cur.assignee = next_assignee.clone();
                cur.revision += 1;
            }
        });
        return;
    }
    let cid = prev_card.id.clone();
    let rev = prev_card.revision;
    let rollback = prev_card.clone();
    ui.snapshot.update(|s| {
        if let Some(cur) = s.cards.iter_mut().find(|x| x.id == cid) {
            cur.assignee = next_assignee.clone();
        }
    });
    dispatch_complete(
        ui,
        "card.update",
        json!({"card_id": cid, "revision": rev, "assignee": next_assignee}),
        move |res| {
            if res.is_err() {
                ui.snapshot.update(|s| {
                    if let Some(cur) = s.cards.iter_mut().find(|x| x.id == rollback.id)
                        && cur.revision == rollback.revision
                    {
                        *cur = rollback;
                    }
                });
            }
        },
    );
}
pub fn load_text(ui: Ui, method: &'static str, params: Value) {
    let generation = ui.output_request.get_untracked() + 1;
    ui.output_request.set(generation);
    ui.output.set("Loading…".into());
    if !crate::backend::is_connected() {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    }
    let active_project = ui.project.get_untracked();
    ui.spawn(async move {
        let result =
            blocking(move || crate::routed_call(method, params, Some(active_project.as_str())))
                .await;
        if ui.output_request.get_untracked() != generation {
            return;
        }
        ui.output_observed_at.set(if method == "overview.preview" {
            result
                .as_ref()
                .ok()
                .and_then(|r| r.as_ref().ok())
                .and_then(|v| v["observed_at"].as_u64())
        } else {
            None
        });
        ui.output.set(match result {
            Ok(Ok(v)) if method == "overview.preview" => format!(
                "{} · {}\n\n{}",
                v["source"].as_str().unwrap_or("Execution context"),
                v["terminal"].as_str().unwrap_or(""),
                v["text"].as_str().unwrap_or("")
            ),
            Ok(Ok(v)) => v["body"]
                .as_str()
                .or(v["text"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string()),
            Ok(Err(e)) => format!("Unable to load: {e:#}"),
            Err(e) => e.to_string(),
        });
    });
}

pub fn open_resource(ui: Ui, card_id: String, path: String) {
    if !crate::backend::is_connected() {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    }
    let host_id = ui
        .snapshot
        .with_untracked(|s| s.card_host(&card_id).to_owned());
    ui.spawn(async move {
        match blocking(move || {
            let local_open_path = if host_id == "local" {
                let value = crate::routed_call(
                    "resource.materialize",
                    json!({"card_id": card_id, "path": path, "host_id": "local"}),
                    None,
                )?;
                value["path"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Attachment path is missing"))?
                    .to_owned()
            } else {
                // Materialize remote attachment bytes into local scratch cache before opening locally.
                let value = crate::routed_call(
                    "resource.read",
                    json!({"card_id": card_id, "path": path, "host_id": host_id}),
                    None,
                )?;
                let scratch_dir = std::env::temp_dir()
                    .join("canban-remote-resources")
                    .join(&card_id);
                std::fs::create_dir_all(&scratch_dir)?;
                let filename = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("attachment");
                let dest = scratch_dir.join(filename);
                if let Some(bytes) = value["bytes"].as_array() {
                    let raw: Vec<u8> = bytes
                        .iter()
                        .filter_map(|b| b.as_u64().map(|n| n as u8))
                        .collect();
                    std::fs::write(&dest, raw)?;
                } else if let Some(text) = value["body"].as_str().or(value["text"].as_str()) {
                    std::fs::write(&dest, text)?;
                } else {
                    anyhow::bail!("Remote attachment content is empty");
                }
                dest.to_string_lossy().into_owned()
            };
            crate::platform::open_external(&local_open_path)?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        {
            Ok(Ok(())) => ui.notify("Attachment opened"),
            Ok(Err(e)) => ui
                .operation_error
                .set(format!("Unable to open attachment: {e:#}")),
            Err(e) => ui.status.set(e.to_string()),
        }
    });
}

pub fn copy_text(ui: Ui, value: String) {
    drop(set_clipboard(&value));
    ui.notify("Copied to clipboard");
}
pub struct ParsedTimelinePage {
    pub events: Vec<Event>,
    pub highwater: u64,
    pub has_more_before: bool,
    pub oldest_sequence: Option<u64>,
}

pub fn parse_timeline_page(val: &Value) -> ParsedTimelinePage {
    let events = val["events"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let mut normalized = v.clone();
                    if let Some(obj) = normalized.as_object_mut()
                        && obj.contains_key("message")
                        && obj.contains_key("body")
                    {
                        obj.remove("body");
                    }
                    serde_json::from_value::<Event>(normalized).ok()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let highwater = val["highwater"]
        .as_u64()
        .unwrap_or_else(|| confirmed_max_sequence(&events));
    let has_more_before = val["has_more_before"].as_bool().unwrap_or(false);
    let oldest_sequence = val["oldest_sequence"].as_u64().or_else(|| {
        events
            .iter()
            .find(|e| e.data["pending"] != true)
            .map(|e| e.sequence)
    });
    ParsedTimelinePage {
        events,
        highwater,
        has_more_before,
        oldest_sequence,
    }
}

fn merge_events_by_id(target: &mut Vec<Event>, incoming: Vec<Event>) {
    for ev in incoming {
        if let Some(existing) = target.iter_mut().find(|e| e.id == ev.id) {
            *existing = ev;
        } else {
            target.push(ev);
        }
    }
    target.sort_by_key(|e| e.sequence);
}

pub fn confirmed_max_sequence(events: &[Event]) -> u64 {
    events
        .iter()
        .filter(|e| e.data["pending"] != true)
        .map(|e| e.sequence)
        .max()
        .unwrap_or(0)
}

pub fn apply_tail_page(
    state: &mut CardTimelineState,
    mut events: Vec<Event>,
    highwater: u64,
    has_more_before: bool,
    oldest_seq: Option<u64>,
    writes: &mut submission::CommittedWrites,
    card_id: &str,
) {
    writes.reconcile_card_events(card_id, &events);
    writes.overlay_card_events(card_id, &mut events);
    state.oldest_seq = oldest_seq.or_else(|| {
        events
            .iter()
            .find(|e| e.data["pending"] != true)
            .map(|e| e.sequence)
    });
    state.highwater = state
        .highwater
        .max(highwater)
        .max(confirmed_max_sequence(&events));
    state.has_more_before = has_more_before;
    state.events = events;
    state.loading = false;
    state.initialized = true;
}

pub fn apply_incremental_page(
    state: &mut CardTimelineState,
    events: Vec<Event>,
    highwater: u64,
    writes: &mut submission::CommittedWrites,
    card_id: &str,
) {
    writes.reconcile_card_events(card_id, &events);
    for ev in &events {
        if ev.kind == "comment.updated"
            && let Some(target_id) = ev.data["comment_id"].as_str()
            && let Some(target) = state.events.iter_mut().find(|e| e.id == target_id)
            && target.kind != "comment.deleted"
        {
            if let Some(body) = ev
                .data
                .get("body")
                .and_then(Value::as_str)
                .or_else(|| (!ev.message.is_empty()).then_some(ev.message.as_str()))
            {
                target.message = body.to_owned();
                target.data["body"] = json!(body);
            }
            if let Some(rev) = ev.data["revision"].as_u64() {
                target.data["revision"] = json!(rev);
            }
            if let Some(edited_at) = ev.data["edited_at"].as_u64() {
                target.data["edited_at"] = json!(edited_at);
            }
        } else if ev.kind == "comment.deleted"
            && let Some(target_id) = ev.data["comment_id"].as_str()
            && let Some(target) = state.events.iter_mut().find(|e| e.id == target_id)
        {
            target.kind = "comment.deleted".into();
            target.message = "Comment deleted".into();
            target.data["comment_id"] = json!(target_id);
            if let Some(rev) = ev.data["revision"].as_u64() {
                target.data["revision"] = json!(rev);
            }
            if let Some(deleted_at) = ev.data["deleted_at"].as_u64() {
                target.data["deleted_at"] = json!(deleted_at);
            }
        }
    }
    merge_events_by_id(&mut state.events, events);
    writes.overlay_card_events(card_id, &mut state.events);
    if state.oldest_seq.is_none() {
        state.oldest_seq = state
            .events
            .iter()
            .find(|e| e.data["pending"] != true)
            .map(|e| e.sequence);
    }
    state.highwater = state
        .highwater
        .max(highwater)
        .max(confirmed_max_sequence(&state.events));
    state.loading = false;
    state.initialized = true;
}

pub fn apply_older_page(
    state: &mut CardTimelineState,
    events: Vec<Event>,
    has_more_before: bool,
    oldest_seq: Option<u64>,
    writes: &mut submission::CommittedWrites,
    card_id: &str,
) {
    writes.reconcile_card_events(card_id, &events);
    merge_events_by_id(&mut state.events, events);
    writes.overlay_card_events(card_id, &mut state.events);
    state.oldest_seq = match (state.oldest_seq, oldest_seq) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
    .or_else(|| state.events.first().map(|e| e.sequence));
    state.has_more_before = has_more_before;
    state.loading_older = false;
}

pub fn apply_around_page(
    state: &mut CardTimelineState,
    events: Vec<Event>,
    has_more_before: bool,
    writes: &mut submission::CommittedWrites,
    card_id: &str,
) {
    writes.reconcile_card_events(card_id, &events);
    merge_events_by_id(&mut state.events, events);
    writes.overlay_card_events(card_id, &mut state.events);
    state.oldest_seq = state.events.first().map(|e| e.sequence);
    state.has_more_before = has_more_before;
    state.loading_older = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_dismiss_closes_only_card_and_new_card_when_unblocked() {
        assert_eq!(
            workspace_dismiss_action("card", false),
            Some(WorkspaceDismiss::CloseCard)
        );
        assert_eq!(
            workspace_dismiss_action("new-card", false),
            Some(WorkspaceDismiss::CloseNewCard)
        );
        assert_eq!(workspace_dismiss_action("board", false), None);
        assert_eq!(workspace_dismiss_action("review", false), None);
        assert_eq!(workspace_dismiss_action("card", true), None);
        assert_eq!(workspace_dismiss_action("new-card", true), None);
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct DynamicViolation {
        line: usize,
        snippet: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ScanTok {
        Ident(String, usize),
        Punct(char, usize),
        DoublePipe(usize),
        Arrow(usize),
        FatArrow(usize),
    }

    fn lex_non_test_tokens(src: &str) -> Vec<ScanTok> {
        // Store each char with its 1-based line number, replacing comments and literals with spaces.
        let raw: Vec<(char, usize)> = {
            let mut line = 1usize;
            src.chars()
                .map(|c| {
                    let cur = line;
                    if c == '\n' {
                        line += 1;
                    }
                    (c, cur)
                })
                .collect()
        };
        let mut clean = raw.clone();
        let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let raw_str_start = |pos: usize, prefix_b: bool| -> Option<(usize, usize)> {
            let mut p = pos;
            if prefix_b {
                if raw.get(p)?.0 != 'b' {
                    return None;
                }
                p += 1;
            }
            if raw.get(p)?.0 != 'r' {
                return None;
            }
            p += 1;
            let mut hashes = 0usize;
            while raw.get(p).is_some_and(|(c, _)| *c == '#') {
                hashes += 1;
                p += 1;
            }
            (raw.get(p).is_some_and(|(c, _)| *c == '"')).then_some((p + 1, hashes))
        };

        let mut i = 0usize;
        while i < raw.len() {
            let ch = raw[i].0;
            if ch == '/' && raw.get(i + 1).is_some_and(|(c, _)| *c == '/') {
                while i < raw.len() && raw[i].0 != '\n' {
                    clean[i].0 = ' ';
                    i += 1;
                }
                continue;
            }
            if ch == '/' && raw.get(i + 1).is_some_and(|(c, _)| *c == '*') {
                clean[i].0 = ' ';
                clean[i + 1].0 = ' ';
                i += 2;
                let mut depth = 1usize;
                while i < raw.len() && depth > 0 {
                    if raw[i].0 == '/' && raw.get(i + 1).is_some_and(|(c, _)| *c == '*') {
                        clean[i].0 = ' ';
                        clean[i + 1].0 = ' ';
                        depth += 1;
                        i += 2;
                    } else if raw[i].0 == '*' && raw.get(i + 1).is_some_and(|(c, _)| *c == '/') {
                        clean[i].0 = ' ';
                        clean[i + 1].0 = ' ';
                        depth -= 1;
                        i += 2;
                    } else {
                        clean[i].0 = ' ';
                        i += 1;
                    }
                }
                continue;
            }
            if let Some((start, hashes)) =
                raw_str_start(i, false).or_else(|| raw_str_start(i, true))
                && (i == 0 || !is_ident(raw[i - 1].0))
            {
                for item in &mut clean[i..start] {
                    item.0 = ' ';
                }
                i = start;
                while i < raw.len() {
                    if raw[i].0 == '"' {
                        let mut matched = 0usize;
                        while matched < hashes
                            && raw.get(i + 1 + matched).is_some_and(|(c, _)| *c == '#')
                        {
                            matched += 1;
                        }
                        if matched == hashes {
                            for item in &mut clean[i..=i + hashes] {
                                item.0 = ' ';
                            }
                            i += 1 + hashes;
                            break;
                        }
                    }
                    clean[i].0 = ' ';
                    i += 1;
                }
                continue;
            }
            if (ch == '"' || (ch == 'b' && raw.get(i + 1).is_some_and(|(c, _)| *c == '"')))
                && (ch == '"' || i == 0 || !is_ident(raw[i - 1].0))
            {
                let end_prefix = i + if ch == 'b' { 2 } else { 1 };
                for item in &mut clean[i..end_prefix] {
                    item.0 = ' ';
                }
                i = end_prefix;
                while i < raw.len() {
                    if raw[i].0 == '\\' {
                        clean[i].0 = ' ';
                        if let Some(next) = clean.get_mut(i + 1) {
                            next.0 = ' ';
                        }
                        i += 2;
                    } else if raw[i].0 == '"' {
                        clean[i].0 = ' ';
                        i += 1;
                        break;
                    } else {
                        clean[i].0 = ' ';
                        i += 1;
                    }
                }
                continue;
            }
            if ch == '\'' || (ch == 'b' && raw.get(i + 1).is_some_and(|(c, _)| *c == '\'')) {
                let q = if ch == 'b' { i + 1 } else { i };
                if raw.get(q + 1).is_some_and(|(c, _)| *c == '\\') {
                    let end = (q + 1).min(clean.len() - 1);
                    for item in &mut clean[i..=end] {
                        item.0 = ' ';
                    }
                    i = q + 2;
                    while i < raw.len() {
                        if raw[i].0 == '\\' {
                            clean[i].0 = ' ';
                            if let Some(next) = clean.get_mut(i + 1) {
                                next.0 = ' ';
                            }
                            i += 2;
                        } else if raw[i].0 == '\'' {
                            clean[i].0 = ' ';
                            i += 1;
                            break;
                        } else {
                            clean[i].0 = ' ';
                            i += 1;
                        }
                    }
                    continue;
                } else if raw.get(q + 1).is_some_and(|(c, _)| *c != '\'')
                    && raw.get(q + 2).is_some_and(|(c, _)| *c == '\'')
                {
                    for item in &mut clean[i..=q + 2] {
                        item.0 = ' ';
                    }
                    i = q + 3;
                    continue;
                }
            }
            i += 1;
        }

        // Tokenize cleaned source
        let mut raw_tokens = Vec::new();
        let mut p = 0usize;
        while p < clean.len() {
            let (ch, line) = clean[p];
            if ch.is_whitespace() {
                p += 1;
                continue;
            }
            if ch.is_ascii_alphabetic() || ch == '_' {
                let start = p;
                while p < clean.len() && is_ident(clean[p].0) {
                    p += 1;
                }
                let text: String = clean[start..p].iter().map(|(c, _)| *c).collect();
                raw_tokens.push(ScanTok::Ident(text, line));
                continue;
            }
            if ch == '|' && clean.get(p + 1).is_some_and(|(c, _)| *c == '|') {
                raw_tokens.push(ScanTok::DoublePipe(line));
                p += 2;
                continue;
            }
            if ch == '-' && clean.get(p + 1).is_some_and(|(c, _)| *c == '>') {
                raw_tokens.push(ScanTok::Arrow(line));
                p += 2;
                continue;
            }
            if ch == '=' && clean.get(p + 1).is_some_and(|(c, _)| *c == '>') {
                raw_tokens.push(ScanTok::FatArrow(line));
                p += 2;
                continue;
            }
            raw_tokens.push(ScanTok::Punct(ch, line));
            p += 1;
        }

        // Filter out `#[cfg(test)]` annotated items
        let mut tokens = Vec::with_capacity(raw_tokens.len());
        let mut k = 0usize;
        while k < raw_tokens.len() {
            let is_cfg_test = matches!(
                raw_tokens.get(k..k + 7),
                Some([
                    ScanTok::Punct('#', _),
                    ScanTok::Punct('[', _),
                    ScanTok::Ident(cfg, _),
                    ScanTok::Punct('(', _),
                    ScanTok::Ident(test, _),
                    ScanTok::Punct(')', _),
                    ScanTok::Punct(']', _),
                ]) if cfg == "cfg" && test == "test"
            );
            if is_cfg_test {
                k += 7;
                // Skip any chained outer attributes `#[...]`
                while matches!(
                    raw_tokens.get(k..k + 2),
                    Some([ScanTok::Punct('#', _), ScanTok::Punct('[', _)])
                ) {
                    k += 2;
                    let mut adepth = 1usize;
                    while k < raw_tokens.len() && adepth > 0 {
                        match raw_tokens[k] {
                            ScanTok::Punct('[', _) => adepth += 1,
                            ScanTok::Punct(']', _) => adepth -= 1,
                            _ => {}
                        }
                        k += 1;
                    }
                }
                // Skip item up to matching `{ ... }` or `;`
                let mut depth = 0usize;
                while k < raw_tokens.len() {
                    match raw_tokens[k] {
                        ScanTok::Punct('{', _) => {
                            depth += 1;
                            k += 1;
                            while k < raw_tokens.len() && depth > 0 {
                                match raw_tokens[k] {
                                    ScanTok::Punct('{', _) => depth += 1,
                                    ScanTok::Punct('}', _) => depth -= 1,
                                    _ => {}
                                }
                                k += 1;
                            }
                            break;
                        }
                        ScanTok::Punct(';', _) if depth == 0 => {
                            k += 1;
                            break;
                        }
                        _ => k += 1,
                    }
                }
                continue;
            }
            tokens.push(raw_tokens[k].clone());
            k += 1;
        }
        tokens
    }

    fn skip_balanced(tokens: &[ScanTok], start: usize, open: char, close: char) -> usize {
        let mut depth = 0usize;
        let mut i = start;
        while i < tokens.len() {
            match tokens[i] {
                ScanTok::Punct(c, _) if c == open => depth += 1,
                ScanTok::Punct(c, _) if c == close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        tokens.len()
    }

    fn can_start_closure_before(prev: Option<&ScanTok>) -> bool {
        match prev {
            None => true,
            Some(
                ScanTok::Punct('(' | '[' | '{' | ',' | ';' | '=' | ':', _)
                | ScanTok::Arrow(_)
                | ScanTok::FatArrow(_),
            ) => true,
            Some(ScanTok::Ident(id, _)) => matches!(id.as_str(), "move" | "return" | "else"),
            _ => false,
        }
    }

    fn match_closure_header(tokens: &[ScanTok], pos: usize) -> Option<usize> {
        let mut p = pos;
        let has_move = matches!(tokens.get(p), Some(ScanTok::Ident(id, _)) if id == "move");
        if has_move {
            p += 1;
        } else if !can_start_closure_before(pos.checked_sub(1).and_then(|i| tokens.get(i))) {
            return None;
        }
        match tokens.get(p) {
            Some(ScanTok::DoublePipe(_)) => p += 1,
            Some(ScanTok::Punct('|', _)) => {
                p += 1;
                let mut angle = 0usize;
                let mut paren = 0usize;
                while p < tokens.len() {
                    match &tokens[p] {
                        ScanTok::Punct('|', _) if angle == 0 && paren == 0 => {
                            p += 1;
                            break;
                        }
                        ScanTok::Punct('<', _) => angle += 1,
                        ScanTok::Punct('>', _) => angle = angle.saturating_sub(1),
                        ScanTok::Punct('(', _) => paren += 1,
                        ScanTok::Punct(')', _) => paren = paren.saturating_sub(1),
                        ScanTok::Punct(';' | '{' | '}', _) => return None,
                        _ => {}
                    }
                    p += 1;
                }
            }
            _ => return None,
        }
        Some(p)
    }

    fn skip_closure_body(tokens: &[ScanTok], after_params: usize, limit: usize) -> usize {
        let mut p = after_params;
        if matches!(tokens.get(p), Some(ScanTok::Arrow(_))) {
            while p < limit && !matches!(tokens.get(p), Some(ScanTok::Punct('{' | ',' | ')', _))) {
                p += 1;
            }
        }
        if p < limit && matches!(tokens.get(p), Some(ScanTok::Punct('{', _))) {
            return skip_balanced(tokens, p, '{', '}').min(limit);
        }
        let mut p_depth = 0usize;
        let mut b_depth = 0usize;
        let mut c_depth = 0usize;
        while p < limit {
            match &tokens[p] {
                ScanTok::Punct('(', _) => p_depth += 1,
                ScanTok::Punct(')', _) => {
                    if p_depth == 0 && b_depth == 0 && c_depth == 0 {
                        return p;
                    }
                    p_depth = p_depth.saturating_sub(1);
                }
                ScanTok::Punct('[', _) => b_depth += 1,
                ScanTok::Punct(']', _) => b_depth = b_depth.saturating_sub(1),
                ScanTok::Punct('{', _) => c_depth += 1,
                ScanTok::Punct('}', _) => {
                    if p_depth == 0 && b_depth == 0 && c_depth == 0 {
                        return p;
                    }
                    c_depth = c_depth.saturating_sub(1);
                }
                ScanTok::Punct(',' | ';', _) if p_depth == 0 && b_depth == 0 && c_depth == 0 => {
                    return p;
                }
                _ => {}
            }
            p += 1;
        }
        limit
    }

    fn scan_dynamic_violations(src: &str) -> Vec<DynamicViolation> {
        let tokens = lex_non_test_tokens(src);
        let mut violations = Vec::new();
        let mut i = 0usize;
        while i + 1 < tokens.len() {
            let is_dynamic = matches!(
                (&tokens[i], &tokens[i + 1]),
                (ScanTok::Ident(name, _), ScanTok::Punct('(', _)) if name == "dynamic"
            );
            if !is_dynamic {
                i += 1;
                continue;
            }
            let arg_start = i + 2;
            let arg_end = skip_balanced(&tokens, i + 1, '(', ')');
            let body_end = arg_end.saturating_sub(1);
            let mut cursor = match_closure_header(&tokens, arg_start).unwrap_or(arg_start);
            if matches!(tokens.get(cursor), Some(ScanTok::Arrow(_))) {
                while cursor < body_end
                    && !matches!(tokens.get(cursor), Some(ScanTok::Punct('{', _)))
                {
                    cursor += 1;
                }
            }
            while cursor < body_end {
                if matches!(
                    (tokens.get(cursor), tokens.get(cursor + 1)),
                    (Some(ScanTok::Ident(name, _)), Some(ScanTok::Punct('(', _)))
                        if name == "computed" || name == "untracked"
                ) {
                    cursor = skip_balanced(&tokens, cursor + 1, '(', ')').min(body_end);
                    continue;
                }
                if let Some(after_params) = match_closure_header(&tokens, cursor) {
                    cursor = skip_closure_body(&tokens, after_params, body_end);
                    continue;
                }
                if let (
                    Some(ScanTok::Punct('.', _)),
                    Some(ScanTok::Ident(field, line)),
                    Some(ScanTok::Punct('.', _)),
                    Some(ScanTok::Ident(method, _)),
                    Some(ScanTok::Punct('(', _)),
                ) = (
                    tokens.get(cursor),
                    tokens.get(cursor + 1),
                    tokens.get(cursor + 2),
                    tokens.get(cursor + 3),
                    tokens.get(cursor + 4),
                ) && matches!(field.as_str(), "snapshot" | "runtime_clock")
                    && matches!(method.as_str(), "get" | "with")
                {
                    violations.push(DynamicViolation {
                        line: *line,
                        snippet: format!("{field}.{method}(...)"),
                    });
                }
                cursor += 1;
            }
            i += 1;
        }
        violations
    }

    #[test]
    fn dynamic_guard_scanner_catches_direct_reads_and_allows_leaf_closures() {
        let bad = r#"
            fn broken(ui: Ui) {
                dynamic(move || {
                    let s = ui.snapshot.get();
                    view().child(label(move || ui.runtime_clock.get().to_string()))
                });
                dynamic(move || if flag || ui.runtime_clock.get() > 0 { 1 } else { 0 });
            }
        "#;
        let v = scan_dynamic_violations(bad);
        assert_eq!(
            v.iter().map(|x| x.snippet.as_str()).collect::<Vec<_>>(),
            vec!["snapshot.get(...)", "runtime_clock.get(...)"]
        );

        let good = r#"
            fn safe(ui: Ui) {
                let model = computed(move || ui.snapshot.with(|s| s.cards.len()));
                dynamic(move || {
                    let n = model.get();
                    let now_untracked = ui.runtime_clock.get_untracked();
                    view()
                        .child(label(move || format!("{n} {}", ui.runtime_clock.get())))
                        .child(compact_action("Go", Variant::Quiet, move |_| {
                            let _ = ui.snapshot.with_untracked(|s| s.cards.len());
                        }))
                        .disabled(move || ui.snapshot.with(|s| s.cards.is_empty()))
                        .child(when(move || ui.snapshot.with(|s| !s.cards.is_empty()), move || {
                            view()
                        }))
                });
            }
            #[cfg(test)]
            mod tests {
                fn allowed_in_tests(ui: Ui) {
                    dynamic(move || ui.snapshot.get());
                }
            }
        "#;
        assert!(scan_dynamic_violations(good).is_empty());
    }

    #[test]
    fn dynamic_blocks_do_not_directly_track_full_snapshot_or_runtime_clock() {
        fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect_rs(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs(&src_dir, &mut files);
        files.sort();
        let mut all_violations = Vec::new();
        for path in files {
            let src = std::fs::read_to_string(&path).unwrap();
            for v in scan_dynamic_violations(&src) {
                let rel = path.strip_prefix(&src_dir).unwrap_or(&path);
                all_violations.push(format!("{}:{}: {}", rel.display(), v.line, v.snippet));
            }
        }
        assert!(
            all_violations.is_empty(),
            "Found direct tracked ui.snapshot / ui.runtime_clock reads inside dynamic(...) blocks:\n{}",
            all_violations.join("\n")
        );
    }
}
