//! Collapsed Board Card Agent summary pill and floating live Inspector Popover.
//!
//! Keeps Board Card height constant regardless of how many Agents are running
//! on a Card, prevents horizontal footer overlap in narrow columns, and lets
//! users inspect live Agent status, blocked reasons, recent terminal output,
//! and quick controls directly from the Board without opening the Card.
use crate::{components::card_tile::EphemeralAgentBadge, index::RunningAgentBadge, presenter::*};
use canban_types::{HerdrStatus, RuntimeWait};
use canban_ui_kit::{motion, tokens::*, *};
use nsh_base::PopoverEvent;
use nsh_core::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum width of the collapsed Agent chip on a Board Card tile so it never
/// overflows or overlaps adjacent footer items even in a 220px narrow column.
const PILL_MAX_WIDTH: f32 = 188.;
/// Size of each circular provider avatar in the collapsed stack.
const AVATAR_SIZE: f32 = 18.;
/// Horizontal step between overlapped avatars in the collapsed stack.
const AVATAR_STEP: f32 = 13.;
/// Maximum provider icons rendered in the collapsed multi-agent cluster.
const MAX_CLUSTER_ICONS: usize = 3;
/// Maximum lines shown in the inline on-demand terminal output preview.
const OUTPUT_TAIL_LINES: usize = 14;
/// Maximum UTF-8 characters retained for one inline output preview.
const OUTPUT_MAX_CHARS: usize = 2400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPreviewState {
    pub loading: bool,
    pub text: String,
    pub error: Option<String>,
    pub generation: u64,
}

/// Window-scoped state for the Board Card Agent Inspector Popover.
/// Hoisted onto `Ui` so Board tile model updates do not unexpectedly dismiss
/// an open popover or lose an expanded output preview.
#[derive(Clone, Copy)]
pub struct AgentPopoverState {
    pub open_card: Signal<Option<String>>,
    pub expanded_outputs: Signal<BTreeSet<String>>,
    pub outputs: Signal<BTreeMap<String, OutputPreviewState>>,
    pub request_gen: Signal<u64>,
}

impl AgentPopoverState {
    pub fn new() -> Self {
        Self {
            open_card: signal(None),
            expanded_outputs: signal(BTreeSet::new()),
            outputs: signal(BTreeMap::new()),
            request_gen: signal(0),
        }
    }

    pub fn is_open(self, card_id: &str) -> bool {
        self.open_card.with(|open| open.as_deref() == Some(card_id))
    }

    pub fn open(self, card_id: String) {
        let same = self
            .open_card
            .with_untracked(|open| open.as_deref() == Some(card_id.as_str()));
        if same {
            return;
        }
        batch(move || {
            self.request_gen.update(|g| *g += 1);
            self.expanded_outputs.set(BTreeSet::new());
            self.outputs.set(BTreeMap::new());
            self.open_card.set(Some(card_id));
        });
    }

    pub fn close(self) {
        if self.open_card.with_untracked(Option::is_none) {
            return;
        }
        batch(move || {
            self.open_card.set(None);
            self.request_gen.update(|g| *g += 1);
            self.expanded_outputs.set(BTreeSet::new());
            self.outputs.set(BTreeMap::new());
        });
    }

    pub fn close_if_card(self, card_id: &str) {
        if self
            .open_card
            .with_untracked(|open| open.as_deref() == Some(card_id))
        {
            self.close();
        }
    }
}

impl Default for AgentPopoverState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentKind {
    Workflow,
    Ephemeral,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PillAgentRef {
    kind: AgentKind,
    label: String,
    provider_icon: Icon,
    provider_name: String,
    herdr_status: HerdrStatus,
    started_at: Option<u64>,
    hint_detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InspectorAgentRow {
    terminal_id: String,
    card_id: String,
    kind: AgentKind,
    primary_name: String,
    provider_icon: Icon,
    provider_name: String,
    herdr_status: HerdrStatus,
    status_label: String,
    started_at: Option<u64>,
    wait_title: Option<String>,
    wait_message: Option<String>,
    error_detail: Option<String>,
    instruction_excerpt: Option<String>,
    workflow_step: Option<String>,
    turn: Option<u64>,
    shell_command: Option<String>,
    queued_next: Option<String>,
    reclaim_notice: Option<String>,
    host_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InspectorModel {
    card_id: String,
    card_number: u64,
    assignee: Option<String>,
    summary_text: String,
    rows: Vec<InspectorAgentRow>,
}

fn urgency_rank(status: HerdrStatus) -> u8 {
    match status {
        HerdrStatus::Blocked => 0,
        HerdrStatus::Error | HerdrStatus::Disconnected => 1,
        HerdrStatus::Working | HerdrStatus::Starting => 2,
        HerdrStatus::Queued | HerdrStatus::Closing => 3,
        HerdrStatus::Done => 4,
        HerdrStatus::Idle | HerdrStatus::PlainShell | HerdrStatus::Ended => 5,
    }
}

fn status_visuals(status: HerdrStatus) -> (ThemeColor, ThemeColor, Tone) {
    match status {
        HerdrStatus::Blocked => (ATTENTION_SURFACE, ATTENTION, Tone::Attention),
        HerdrStatus::Error | HerdrStatus::Disconnected => (DANGER_SURFACE, DANGER, Tone::Danger),
        HerdrStatus::Working | HerdrStatus::Starting => (SELECTED, ACCENT, Tone::Info),
        HerdrStatus::Done => (SUCCESS_SURFACE, SUCCESS, Tone::Success),
        HerdrStatus::Queued | HerdrStatus::Closing => (INSET, MUTED, Tone::Neutral),
        HerdrStatus::Idle | HerdrStatus::PlainShell | HerdrStatus::Ended => {
            (INSET, MUTED, Tone::Neutral)
        }
    }
}

fn chip_surface_style(
    status: HerdrStatus,
) -> (ThemeColor, ThemeColor, ThemeColor, ThemeColor, ThemeColor) {
    // Returns: (chip_bg, chip_border, primary_text, chevron_color, focus_ring)
    // Status tint/text highlights urgency while borders stay quiet and neutral.
    match status {
        HerdrStatus::Blocked => (ATTENTION_SURFACE, BORDER, ATTENTION, ATTENTION, ATTENTION),
        HerdrStatus::Error | HerdrStatus::Disconnected => {
            (DANGER_SURFACE, BORDER, DANGER, DANGER, DANGER)
        }
        HerdrStatus::Working | HerdrStatus::Starting | HerdrStatus::Done => {
            (INSET, BORDER, TEXT, MUTED, ACCENT)
        }
        HerdrStatus::Queued
        | HerdrStatus::Closing
        | HerdrStatus::Idle
        | HerdrStatus::PlainShell
        | HerdrStatus::Ended => (INSET, BORDER, MUTED, MUTED, ACCENT),
    }
}

fn corner_dot_color(status: HerdrStatus) -> ThemeColor {
    match status {
        HerdrStatus::Blocked => ATTENTION,
        HerdrStatus::Error | HerdrStatus::Disconnected => DANGER,
        HerdrStatus::Working | HerdrStatus::Starting => ACCENT,
        HerdrStatus::Done => SUCCESS,
        _ => MUTED,
    }
}

fn corner_status_dot(status: HerdrStatus, halo_bg: ThemeColor) -> View {
    let color = corner_dot_color(status);
    let mut dot_wrap = view()
        .relative()
        .w(px(6.))
        .h(px(6.))
        .pointer_events(PointerEvents::None);
    let core = view()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .w(px(6.))
        .h(px(6.))
        .rounded_full()
        .bg(color);
    if status.is_working() && !motion::reduced() {
        dot_wrap = dot_wrap.child(
            view()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(color)
                .opacity(0.6)
                .animate_ping(),
        );
    }
    dot_wrap = dot_wrap.child(core);

    view()
        .absolute()
        .right(px(-2.))
        .bottom(px(-2.))
        .w(px(9.))
        .h(px(9.))
        .rounded_full()
        .bg(halo_bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(dot_wrap)
}

fn avatar_stack(items: &[PillAgentRef], chip_bg: ThemeColor) -> View {
    let visible: Vec<&PillAgentRef> = items.iter().take(MAX_CLUSTER_ICONS).collect();
    let count = visible.len().max(1);
    let total_w = AVATAR_SIZE + (count.saturating_sub(1) as f32) * AVATAR_STEP;

    let mut stack = view()
        .relative()
        .flex_shrink_0()
        .w(px(total_w))
        .h(px(AVATAR_SIZE))
        .pointer_events(PointerEvents::None);

    // Paint back-to-front so the highest-urgency agent (index 0) sits cleanly on top.
    for (idx, item) in visible.iter().enumerate().rev() {
        let left = (idx as f32) * AVATAR_STEP;
        let avatar_bg = match item.herdr_status {
            HerdrStatus::Blocked => ATTENTION_SURFACE,
            HerdrStatus::Error | HerdrStatus::Disconnected => DANGER_SURFACE,
            _ => SURFACE,
        };
        let avatar_border = if count > 1 { chip_bg } else { BORDER };
        let mut slot = view()
            .absolute()
            .left(px(left))
            .top(px(0.))
            .w(px(AVATAR_SIZE))
            .h(px(AVATAR_SIZE))
            .rounded_full()
            .bg(avatar_bg)
            .border_1()
            .border_color(avatar_border)
            .items_center()
            .justify_center()
            .child(icon(item.provider_icon).w(px(11.)).h(px(11.)));
        if idx == 0 {
            slot = slot.child(corner_status_dot(item.herdr_status, chip_bg));
        }
        stack = stack.child(slot);
    }
    stack
}

fn summarize_statuses(statuses: &[HerdrStatus]) -> String {
    let total = statuses.len();
    if total == 0 {
        return String::new();
    }
    let blocked = statuses
        .iter()
        .filter(|&&s| s == HerdrStatus::Blocked)
        .count();
    let errors = statuses
        .iter()
        .filter(|&&s| matches!(s, HerdrStatus::Error | HerdrStatus::Disconnected))
        .count();
    let working = statuses.iter().filter(|&&s| s.is_working()).count();
    let done = statuses.iter().filter(|&&s| s == HerdrStatus::Done).count();
    if blocked > 0 {
        format!("{blocked} blocked · {total}")
    } else if errors > 0 {
        format!("{errors} error · {total}")
    } else if working == total {
        format!("{total} active")
    } else if working > 0 {
        format!("{working}/{total} active")
    } else if done == total {
        format!("{total} done")
    } else {
        format!("{total} agents")
    }
}

fn dominant_status(statuses: &[HerdrStatus]) -> HerdrStatus {
    statuses
        .iter()
        .copied()
        .min_by_key(|&s| urgency_rank(s))
        .unwrap_or(HerdrStatus::Idle)
}

fn collect_pill_refs(
    running: &[RunningAgentBadge],
    ephemeral: &[EphemeralAgentBadge],
) -> Vec<PillAgentRef> {
    let mut items = Vec::with_capacity(running.len() + ephemeral.len());
    for badge in running {
        let hint_detail = if badge.herdr_status == HerdrStatus::Blocked {
            badge
                .wait_detail
                .clone()
                .unwrap_or_else(|| "Blocked on terminal input".into())
        } else {
            badge.herdr_status.label().to_owned()
        };
        items.push(PillAgentRef {
            kind: AgentKind::Workflow,
            label: format!("@{}", badge.profile_id),
            provider_icon: badge.provider_icon,
            provider_name: badge.provider_name.clone(),
            herdr_status: badge.herdr_status,
            started_at: badge.started_at,
            hint_detail,
        });
    }
    for badge in ephemeral {
        let hint_detail = badge
            .attention_reason
            .clone()
            .unwrap_or_else(|| badge.display_status.clone());
        items.push(PillAgentRef {
            kind: AgentKind::Ephemeral,
            label: badge.title.clone(),
            provider_icon: badge.provider_icon,
            provider_name: badge.provider_name.clone(),
            herdr_status: badge.herdr_status,
            started_at: badge.started_at,
            hint_detail,
        });
    }
    items.sort_by_key(|item| urgency_rank(item.herdr_status));
    items
}

fn pill_tooltip(items: &[PillAgentRef]) -> String {
    if items.len() == 1 {
        let one = &items[0];
        let mut head = if one.label.eq_ignore_ascii_case(&one.provider_name) {
            one.label.clone()
        } else {
            format!("{} · {}", one.label, one.provider_name)
        };
        head.push_str(&format!(" · {}", one.hint_detail));
        format!("{head}\nClick to inspect status & output")
    } else {
        let mut lines = Vec::with_capacity(items.len() + 1);
        for item in items.iter().take(5) {
            lines.push(format!("{} · {}", item.label, item.hint_detail));
        }
        if items.len() > 5 {
            lines.push(format!("+{} more agents", items.len() - 5));
        }
        lines.push("Click to inspect all Agents".into());
        lines.join("\n")
    }
}

/// Renders a single bounded collapsed Agent pill on a Board Card tile, anchored
/// to a floating Inspector Popover that shows live details without opening the Card.
pub(crate) fn agent_summary_popover(
    ui: Ui,
    card_id: &str,
    card_number: u64,
    running: &[RunningAgentBadge],
    ephemeral: &[EphemeralAgentBadge],
) -> View {
    let items = collect_pill_refs(running, ephemeral);
    let statuses: Vec<HerdrStatus> = items.iter().map(|i| i.herdr_status).collect();
    let dominant = dominant_status(&statuses);
    let (chip_bg, chip_border, primary_text_color, chevron_color, focus_ring) =
        chip_surface_style(dominant);
    let hint = pill_tooltip(&items);
    let a11y = hint.replace('\n', " — ");

    let open_card_id = card_id.to_owned();
    let toggle_card_id = card_id.to_owned();
    let body_card_id = card_id.to_owned();
    let is_open_bg_card = card_id.to_owned();

    let avatars = avatar_stack(&items, chip_bg);

    let label_content: AnyElement = if items.len() == 1 {
        let single = items[0].clone();
        let mut row = view()
            .flex_row()
            .items_center()
            .gap_1()
            .min_w(px(0.))
            .flex_shrink(1.)
            .child(
                text(single.label.clone())
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(primary_text_color)
                    .min_w(px(0.))
                    .flex_shrink(1.)
                    .truncate(),
            );
        if single.herdr_status.is_working()
            && let Some(ts) = single.started_at
        {
            row = row.child(
                text(move || {
                    crate::components::activity::compact_duration(
                        ui.runtime_clock.get().saturating_sub(ts),
                    )
                })
                .text_size(px(10.5))
                .font_family(MONO_FONT)
                .text_color(MUTED)
                .tabular_nums()
                .flex_shrink_0(),
            );
        } else if let Some((suffix, suffix_color)) = match single.herdr_status {
            HerdrStatus::Blocked => Some(("Blocked", ATTENTION)),
            HerdrStatus::Done => Some(("Done", SUCCESS)),
            HerdrStatus::Error => Some(("Error", DANGER)),
            HerdrStatus::Disconnected => Some(("Offline", DANGER)),
            _ => None,
        } {
            row = row.child(
                text(suffix)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(suffix_color)
                    .flex_shrink_0(),
            );
        }
        row.into_element()
    } else {
        let summary = summarize_statuses(&statuses);
        text(summary)
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(primary_text_color)
            .min_w(px(0.))
            .flex_shrink(1.)
            .truncate()
            .into_element()
    };

    let pill_trigger = press_target()
        .h(px(24.))
        .pl_1()
        .pr_1p5()
        .min_w(px(0.))
        .max_w(px(PILL_MAX_WIDTH))
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap_1p5()
        .rounded(px(6.))
        .bg(move || {
            if ui.agent_popover.is_open(&is_open_bg_card) {
                CARD_HOVER.resolve()
            } else {
                chip_bg.resolve()
            }
        })
        .border_1()
        .border_color(chip_border)
        .hover(|s| s.bg(CARD_HOVER))
        .focus(move |s| s.ring(px(1.)).ring_color(focus_ring))
        .accessibility_label(move || Some(a11y.clone().into()))
        .child(avatars)
        .child(label_content)
        .child(
            icon_tinted(Icon::ChevronDown, chevron_color)
                .w(px(10.))
                .h(px(10.))
                .flex_shrink_0(),
        );

    let popover = nsh_base::popover()
        .open(move || Some(ui.agent_popover.is_open(&open_card_id)))
        .placement(Placement::Bottom)
        .align(Align::End)
        .offset(px(6.))
        .margin(px(8.))
        .popup_class(motion::POPUP_CLASS)
        .popup_starting_class(motion::POPUP_STARTING_CLASS)
        .popup_ending_class(motion::POPUP_ENDING_CLASS)
        .on_open_change(move |e: &PopoverEvent| {
            if let PopoverEvent::OpenChange(next, _) = e {
                if *next {
                    ui.agent_popover.open(toggle_card_id.clone());
                } else {
                    ui.agent_popover.close_if_card(&toggle_card_id);
                }
            }
        })
        .trigger(pill_trigger)
        .child(when(move || ui.agent_popover.is_open(&body_card_id), {
            let cid = card_id.to_owned();
            move || popover_content(ui, cid.clone(), card_number)
        }));

    view()
        .key("footer-agents")
        .flex_shrink_0()
        .max_w(px(PILL_MAX_WIDTH))
        .window_drag(false)
        .tooltip(hint_tooltip(hint))
        .child(popover)
}

fn first_meaningful_line(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let trimmed: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        Some(format!("{trimmed}…"))
    } else {
        Some(trimmed)
    }
}

fn split_wait(wait: Option<&RuntimeWait>) -> (Option<String>, Option<String>) {
    let Some(w) = wait else {
        return (None, None);
    };
    let title = w.title.trim();
    let msg = w.message.trim();
    let title_opt = (!title.is_empty()).then(|| title.to_owned());
    let msg_opt = (!msg.is_empty() && msg != title).then(|| msg.to_owned());
    (title_opt, msg_opt)
}

fn build_inspector_model(ui: Ui, card_id: &str, card_number: u64) -> InspectorModel {
    let card_status = ui.card_status.get();
    let running_badges = card_status
        .get(card_id)
        .map(|d| d.running.clone())
        .unwrap_or_default();
    let multi_host = ui.hosts.with(|h| h.len() > 1);
    let live_entries: Vec<crate::terminals::Entry> = ui.terminal_entries.with(|rows| {
        rows.iter()
            .filter(|e| crate::terminals::is_card_live_entry(e, card_id))
            .cloned()
            .collect()
    });

    let mut rows: Vec<InspectorAgentRow> = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut assignee: Option<String> = None;

    ui.snapshot.with(|snapshot| {
        assignee = snapshot
            .cards
            .iter()
            .find(|c| c.id == card_id)
            .and_then(|c| c.assignee.clone())
            .filter(|a| !a.is_empty());
        for badge in &running_badges {
            seen_ids.insert(badge.terminal_id.clone());
            let saved = snapshot
                .terminals
                .iter()
                .find(|t| t.id == badge.terminal_id);
            let live_entry = live_entries
                .iter()
                .find(|e| e.saved.id == badge.terminal_id);
            let agent = saved.and_then(|t| t.agent.as_ref());
            let (mut wait_title, wait_message) = split_wait(saved.and_then(|t| t.runtime_wait()));
            if badge.herdr_status == HerdrStatus::Blocked
                && wait_title.is_none()
                && wait_message.is_none()
            {
                wait_title = badge
                    .wait_detail
                    .clone()
                    .or_else(|| Some("Waiting on terminal input".into()));
            }
            let error_detail = if matches!(
                badge.herdr_status,
                HerdrStatus::Error | HerdrStatus::Disconnected
            ) {
                agent
                    .and_then(|a| a.error.clone())
                    .or_else(|| live_entry.and_then(|e| e.attention_reason()))
                    .or_else(|| Some("Agent requires inspection".into()))
            } else {
                None
            };
            let instruction_excerpt = agent.and_then(|a| first_meaningful_line(&a.instruction));
            let workflow_step = agent
                .and_then(|a| a.workflow_step.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let queued_next = agent.and_then(|a| a.pending.as_ref()).map(|p| {
                first_meaningful_line(&p.instruction)
                    .unwrap_or_else(|| "Follow-up task queued".into())
            });
            let reclaim_notice = live_entry.and_then(|e| {
                if e.is_auto_reclaiming() {
                    Some("Auto-reclaiming…".into())
                } else if e.is_reclaiming_soon() {
                    Some("Reclaiming soon".into())
                } else {
                    None
                }
            });
            let host_label = multi_host.then(|| {
                let hid = snapshot.card_host(card_id);
                crate::hosts::host_name(ui, hid)
            });
            let status_label = live_entry
                .map(|e| e.display_status())
                .unwrap_or_else(|| badge.herdr_status.label().to_owned());

            rows.push(InspectorAgentRow {
                terminal_id: badge.terminal_id.clone(),
                card_id: card_id.to_owned(),
                kind: AgentKind::Workflow,
                primary_name: format!("@{}", badge.profile_id),
                provider_icon: badge.provider_icon,
                provider_name: badge.provider_name.clone(),
                herdr_status: badge.herdr_status,
                status_label,
                started_at: badge.started_at,
                wait_title,
                wait_message,
                error_detail,
                instruction_excerpt,
                workflow_step,
                turn: None,
                shell_command: None,
                queued_next,
                reclaim_notice,
                host_label,
            });
        }
    });

    for entry in &live_entries {
        if entry.agent.is_some()
            || !entry.saved.has_active_agent()
            || seen_ids.contains(&entry.saved.id)
        {
            continue;
        }
        let (provider_icon, provider_name) = entry
            .provider()
            .unwrap_or_else(|| (Icon::Agent, "Agent".into()));
        let primary_name = entry
            .saved
            .name
            .clone()
            .unwrap_or_else(|| provider_name.clone());
        let herdr_status = entry.herdr_status();
        let started_at =
            (entry.saved.created_at > 1_000_000_000_000).then_some(entry.saved.created_at);
        let (mut wait_title, wait_message) = split_wait(entry.runtime_wait());
        if herdr_status == HerdrStatus::Blocked && wait_title.is_none() && wait_message.is_none() {
            wait_title = entry
                .attention_reason()
                .or_else(|| Some("Agent needs terminal input".into()));
        }
        let error_detail = if matches!(herdr_status, HerdrStatus::Error | HerdrStatus::Disconnected)
        {
            entry
                .attention_reason()
                .or_else(|| entry.saved.runtime.as_ref().and_then(|r| r.error.clone()))
                .or_else(|| Some("Agent reported an error".into()))
        } else {
            None
        };
        let turn = entry
            .saved
            .runtime
            .as_ref()
            .and_then(|r| r.agent_turn)
            .filter(|&t| t > 0);
        let shell_command = entry
            .saved
            .runtime
            .as_ref()
            .and_then(|r| r.command.as_ref())
            .or_else(|| entry.saved.shell_command())
            .and_then(|cmd| {
                if cmd.label.is_empty() {
                    None
                } else {
                    match cmd.exit_code {
                        None => Some(format!("Running {}", cmd.label)),
                        Some(0) => Some(format!("{} (exit 0)", cmd.label)),
                        Some(code) => Some(format!("{} (exit {code})", cmd.label)),
                    }
                }
            });
        let reclaim_notice = if entry.is_auto_reclaiming() {
            Some("Auto-reclaiming…".into())
        } else if entry.is_reclaiming_soon() {
            Some("Reclaiming soon".into())
        } else {
            None
        };
        let host_label = multi_host.then(|| crate::hosts::host_name(ui, &entry.host_id));

        rows.push(InspectorAgentRow {
            terminal_id: entry.saved.id.clone(),
            card_id: card_id.to_owned(),
            kind: AgentKind::Ephemeral,
            primary_name,
            provider_icon,
            provider_name,
            herdr_status,
            status_label: entry.display_status(),
            started_at,
            wait_title,
            wait_message,
            error_detail,
            instruction_excerpt: None,
            workflow_step: None,
            turn,
            shell_command,
            queued_next: None,
            reclaim_notice,
            host_label,
        });
    }

    rows.sort_by_key(|r| urgency_rank(r.herdr_status));
    let statuses: Vec<HerdrStatus> = rows.iter().map(|r| r.herdr_status).collect();
    let summary_text = if rows.is_empty() {
        "No active agents".into()
    } else {
        summarize_statuses(&statuses)
    };

    InspectorModel {
        card_id: card_id.to_owned(),
        card_number,
        assignee,
        summary_text,
        rows,
    }
}

fn popover_content(ui: Ui, card_id: String, card_number: u64) -> impl IntoElement {
    let model_cid = card_id.clone();
    let model = computed(move || build_inspector_model(ui, &model_cid, card_number));
    dynamic(move || {
        let m = model.get();
        let open_card_id = m.card_id.clone();
        let card_tag = if m.card_number == 0 {
            "Local".to_owned()
        } else {
            format!("#{}", m.card_number)
        };
        let mut list = view().w_full().p_2().gap_2();
        if let Some(assignee) = &m.assignee {
            list = list.child(
                view()
                    .px_1()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(muted(format!("Assigned to @{assignee}")).text_size(px(11.))),
            );
        }
        if m.rows.is_empty() {
            list = list.child(
                view()
                    .p_3()
                    .items_center()
                    .justify_center()
                    .child(muted("All Agents on this Card have settled.")),
            );
        } else {
            for row in m.rows {
                let row_key = format!("inspector-agent:{}", row.terminal_id);
                list = list.child(inspector_agent_row(ui, row).key(row_key));
            }
        }
        view()
            .w(px(348.))
            .max_h(px(460.))
            .bg(SURFACE)
            .rounded(px(10.))
            .border_1()
            .border_color(BORDER)
            .shadow_lg()
            .overflow_hidden()
            .child(
                view()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(BORDER)
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .min_w(px(0.))
                            .flex_1()
                            .child(
                                muted(card_tag)
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.5))
                                    .flex_shrink_0(),
                            )
                            .child(
                                label("Agents")
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .flex_shrink_0(),
                            )
                            .child(
                                muted(format!("· {}", m.summary_text))
                                    .text_size(px(11.5))
                                    .truncate()
                                    .min_w(px(0.)),
                            ),
                    )
                    .child(
                        press_target()
                            .h(px(22.))
                            .px_1p5()
                            .rounded(px(5.))
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .hover(|s| s.bg(INSET))
                            .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                            .accessibility_label(|| Some("Open Card".into()))
                            .child(
                                text("Open Card ↗")
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ACCENT),
                            )
                            .on_press(move |_: &()| {
                                ui.agent_popover.close();
                                ui.open_card(open_card_id.clone());
                            }),
                    ),
            )
            .child(scroll().w_full().max_h(px(410.)).child(list))
    })
}

fn inspector_agent_row(ui: Ui, row: InspectorAgentRow) -> View {
    let (_, _, status_tone) = status_visuals(row.herdr_status);
    let show_provider_sub = !row.primary_name.eq_ignore_ascii_case(&row.provider_name);
    let sub_label = match (row.kind, show_provider_sub) {
        (AgentKind::Workflow, true) => Some(row.provider_name.clone()),
        (AgentKind::Ephemeral, true) => Some(format!("{} · Interactive", row.provider_name)),
        (AgentKind::Ephemeral, false) => Some("Interactive".into()),
        (AgentKind::Workflow, false) => None,
    };

    let started_at = row.started_at;
    let is_working = row.herdr_status.is_working();
    let status_chip: AnyElement = if is_working {
        live_status(row.status_label.clone(), status_tone).into_element()
    } else {
        status(row.status_label.clone(), status_tone).into_element()
    };

    let mut card = view()
        .w_full()
        .p_2p5()
        .gap_1p5()
        .rounded(px(7.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER);

    // Row 1: Identity + elapsed duration + live status chip
    let mut id_left = view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .flex_1()
        .child(
            icon(row.provider_icon)
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0(),
        )
        .child(
            text(row.primary_name.clone())
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(TEXT)
                .min_w(px(0.))
                .truncate(),
        );
    if let Some(sub) = sub_label {
        id_left = id_left.child(muted(sub).text_size(px(11.)).min_w(px(0.)).truncate());
    }

    let mut id_right = view().flex_row().items_center().gap_1p5().flex_shrink_0();
    if let Some(ts) = started_at {
        id_right = id_right.child(
            text(move || {
                crate::components::activity::compact_duration(
                    ui.runtime_clock.get().saturating_sub(ts),
                )
            })
            .text_size(px(11.))
            .font_family(MONO_FONT)
            .text_color(MUTED)
            .tabular_nums(),
        );
    }
    id_right = id_right.child(status_chip);

    card = card.child(
        view()
            .w_full()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(id_left)
            .child(id_right),
    );

    // Row 2: Urgent blocked / error callout
    if row.wait_title.is_some() || row.wait_message.is_some() {
        let mut box_body = view().flex_1().min_w(px(0.)).gap_0p5();
        if let Some(title) = &row.wait_title {
            box_body = box_body.child(
                text(title.clone())
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ATTENTION),
            );
        }
        if let Some(msg) = &row.wait_message {
            box_body = box_body.child(
                text(msg.clone())
                    .text_size(px(11.))
                    .text_color(TEXT)
                    .number_of_lines(3),
            );
        }
        card = card.child(
            view()
                .w_full()
                .px_2()
                .py_1p5()
                .rounded(px(5.))
                .bg(ATTENTION_SURFACE)
                .flex_row()
                .items_start()
                .gap_1p5()
                .child(
                    icon_tinted(Icon::ShieldAlert, ATTENTION)
                        .w(px(13.))
                        .h(px(13.))
                        .mt_0p5()
                        .flex_shrink_0(),
                )
                .child(box_body),
        );
    } else if let Some(err) = &row.error_detail {
        card = card.child(
            view()
                .w_full()
                .px_2()
                .py_1p5()
                .rounded(px(5.))
                .bg(DANGER_SURFACE)
                .flex_row()
                .items_start()
                .gap_1p5()
                .child(
                    icon_tinted(Icon::TriangleAlert, DANGER)
                        .w(px(13.))
                        .h(px(13.))
                        .mt_0p5()
                        .flex_shrink_0(),
                )
                .child(
                    text(err.clone())
                        .text_size(px(11.))
                        .text_color(DANGER)
                        .flex_1()
                        .min_w(px(0.))
                        .number_of_lines(3),
                ),
        );
    }

    // Row 3: Structured context lines (instruction / workflow step / turn / shell command / reclaim / host)
    if let Some(excerpt) = &row.instruction_excerpt {
        card = card.child(
            text(excerpt.clone())
                .text_size(px(11.5))
                .text_color(MUTED)
                .number_of_lines(2),
        );
    }

    let mut meta_parts: Vec<String> = Vec::new();
    if let Some(step) = &row.workflow_step {
        meta_parts.push(format!("Step: {step}"));
    }
    if let Some(turn) = row.turn {
        meta_parts.push(format!("Turn {turn}"));
    }
    if let Some(cmd) = &row.shell_command {
        meta_parts.push(cmd.clone());
    }
    if let Some(reclaim) = &row.reclaim_notice {
        meta_parts.push(reclaim.clone());
    }
    if let Some(host) = &row.host_label {
        meta_parts.push(format!("Host: {host}"));
    }
    if !meta_parts.is_empty() {
        card = card.child(muted(meta_parts.join(" · ")).text_size(px(11.)).truncate());
    }
    if let Some(next) = &row.queued_next {
        card = card.child(
            muted(format!("Queued next: {next}"))
                .text_size(px(11.))
                .truncate(),
        );
    }

    // Row 4: Action bar — Output preview toggle on the left; Interrupt / Close / Open on the right
    let tid_for_expand_check = row.terminal_id.clone();
    let is_expanded = computed(move || {
        ui.agent_popover
            .expanded_outputs
            .with(|set| set.contains(&tid_for_expand_check))
    });
    let tid_toggle = row.terminal_id.clone();
    let cid_toggle = row.card_id.clone();
    let output_toggle = press_target()
        .h(px(22.))
        .px_1p5()
        .rounded(px(5.))
        .flex_row()
        .items_center()
        .gap_1()
        .hover(|s| s.bg(CARD_HOVER))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(|| Some("Toggle recent terminal output preview".into()))
        .child(dynamic(move || {
            let ic = if is_expanded.get() {
                Icon::ChevronDown
            } else {
                Icon::Forward
            };
            icon_tinted(ic, MUTED).w(px(11.)).h(px(11.)).into_element()
        }))
        .child(
            text("Output")
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(MUTED),
        )
        .on_press(move |_: &()| {
            let was_open = ui
                .agent_popover
                .expanded_outputs
                .with_untracked(|set| set.contains(&tid_toggle));
            if was_open {
                ui.agent_popover.expanded_outputs.update(|set| {
                    set.remove(&tid_toggle);
                });
            } else {
                ui.agent_popover.expanded_outputs.update(|set| {
                    set.insert(tid_toggle.clone());
                });
                fetch_output(ui, cid_toggle.clone(), tid_toggle.clone());
            }
        });

    let mut right_actions = view().flex_row().items_center().gap_1().flex_shrink_0();

    let can_interrupt = matches!(
        row.herdr_status,
        HerdrStatus::Working | HerdrStatus::Starting | HerdrStatus::Blocked
    );
    let can_close = matches!(
        row.herdr_status,
        HerdrStatus::Done | HerdrStatus::Idle | HerdrStatus::Error | HerdrStatus::Disconnected
    );

    if can_interrupt {
        let tid_int = row.terminal_id.clone();
        let method = match row.kind {
            AgentKind::Workflow => "agent.interrupt",
            AgentKind::Ephemeral => "terminal.interrupt",
        };
        right_actions = right_actions.child(
            press_target()
                .h(px(22.))
                .px_1p5()
                .rounded(px(5.))
                .border_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .flex_row()
                .items_center()
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ATTENTION))
                .disabled(move || ui.pending.get())
                .disabled_style(|s| s.opacity(0.45))
                .accessibility_label(|| Some("Interrupt Agent".into()))
                .child(
                    text("Interrupt")
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(TEXT),
                )
                .on_press(move |_: &()| {
                    dispatch(ui, method, json!({ "terminal_id": tid_int }));
                }),
        );
    } else if can_close {
        let tid_close = row.terminal_id.clone();
        right_actions = right_actions.child(
            press_target()
                .h(px(22.))
                .px_1p5()
                .rounded(px(5.))
                .border_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .flex_row()
                .items_center()
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(DANGER))
                .disabled(move || ui.pending.get())
                .disabled_style(|s| s.opacity(0.45))
                .accessibility_label(|| Some("Close Agent terminal".into()))
                .child(
                    text("Close")
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(MUTED),
                )
                .on_press(move |_: &()| {
                    crate::terminals::close_entry(ui, &tid_close);
                }),
        );
    }

    let tid_open = row.terminal_id.clone();
    let cid_open = row.card_id.clone();
    let open_kind = row.kind;
    right_actions = right_actions.child(
        press_target()
            .h(px(22.))
            .px_2()
            .rounded(px(5.))
            .bg(SELECTED)
            .flex_row()
            .items_center()
            .gap_1()
            .hover(|s| s.bg(CARD_HOVER))
            .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
            .accessibility_label(|| Some("Open Agent terminal in Card".into()))
            .child(
                text("Open ↗")
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ACCENT),
            )
            .on_press(move |_: &()| {
                ui.agent_popover.close();
                match open_kind {
                    AgentKind::Workflow => crate::terminals::open_request(ui, &tid_open),
                    AgentKind::Ephemeral => {
                        ui.open_card_with_terminal(cid_open.clone(), Some(tid_open.clone()))
                    }
                }
            }),
    );

    card = card.child(
        view()
            .w_full()
            .pt_0p5()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_1p5()
            .child(output_toggle)
            .child(right_actions),
    );

    // Row 5: Collapsible on-demand terminal output tail preview
    let tid_output = row.terminal_id.clone();
    let cid_output = row.card_id.clone();
    card = card.child(when(
        move || is_expanded.get(),
        move || output_preview_box(ui, cid_output.clone(), tid_output.clone()),
    ));

    card
}

fn output_preview_box(ui: Ui, card_id: String, terminal_id: String) -> impl IntoElement {
    let tid_state = terminal_id.clone();
    let state = computed(move || {
        ui.agent_popover
            .outputs
            .with(|all| all.get(&tid_state).cloned())
    });
    let tid_refresh = terminal_id.clone();
    let cid_refresh = card_id.clone();
    view()
        .w_full()
        .rounded(px(5.))
        .bg(BACKGROUND)
        .border_1()
        .border_color(BORDER)
        .p_2()
        .gap_1()
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .child(muted("Recent output").text_size(px(10.5)))
                .child(
                    press_target()
                        .w(px(20.))
                        .h(px(20.))
                        .rounded(px(4.))
                        .items_center()
                        .justify_center()
                        .hover(|s| s.bg(INSET))
                        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                        .disabled(move || state.get().is_some_and(|s| s.loading))
                        .disabled_style(|s| s.opacity(0.45))
                        .accessibility_label(|| Some("Refresh output preview".into()))
                        .child(icon_tinted(Icon::Refresh, MUTED).w(px(11.)).h(px(11.)))
                        .on_press(move |_: &()| {
                            fetch_output(ui, cid_refresh.clone(), tid_refresh.clone());
                        }),
                ),
        )
        .child(dynamic(move || match state.get() {
            None => muted("Loading output…").text_size(px(11.)).into_element(),
            Some(st) if st.loading && st.text.is_empty() => {
                muted("Loading output…").text_size(px(11.)).into_element()
            }
            Some(st) if st.error.is_some() => text(st.error.unwrap_or_default())
                .text_size(px(11.))
                .text_color(DANGER)
                .number_of_lines(3)
                .into_element(),
            Some(st) if st.text.trim().is_empty() => muted("No terminal output yet")
                .text_size(px(11.))
                .into_element(),
            Some(st) => scroll()
                .w_full()
                .max_h(px(148.))
                .child(
                    text(st.text)
                        .font_family(MONO_FONT)
                        .text_size(px(10.5))
                        .text_color(TEXT),
                )
                .into_element(),
        }))
}

fn fetch_output(ui: Ui, card_id: String, terminal_id: String) {
    let generation = ui.agent_popover.request_gen.get_untracked() + 1;
    ui.agent_popover.request_gen.set(generation);
    let existing = ui
        .agent_popover
        .outputs
        .with_untracked(|all| all.get(&terminal_id).map(|s| s.text.clone()))
        .unwrap_or_default();
    ui.agent_popover.outputs.update(|all| {
        all.insert(
            terminal_id.clone(),
            OutputPreviewState {
                loading: true,
                text: existing,
                error: None,
                generation,
            },
        );
    });

    if !crate::backend::is_connected() {
        let preview = gallery_preview_output(&terminal_id);
        ui.agent_popover.outputs.update(|all| {
            all.insert(
                terminal_id,
                OutputPreviewState {
                    loading: false,
                    text: preview,
                    error: None,
                    generation,
                },
            );
        });
        return;
    }

    let project = ui.snapshot.with_untracked(|s| {
        s.cards
            .iter()
            .find(|c| c.id == card_id)
            .map(|c| c.project_id.clone())
    });
    let tid = terminal_id.clone();
    ui.spawn(async move {
        let result = blocking(move || {
            crate::routed_call(
                "terminal.output",
                json!({ "terminal_id": tid }),
                project.as_deref(),
            )
        })
        .await;

        let still_open = ui
            .agent_popover
            .open_card
            .with_untracked(|open| open.as_deref() == Some(card_id.as_str()));
        if !still_open {
            return;
        }
        let still_current = ui.agent_popover.outputs.with_untracked(|all| {
            all.get(&terminal_id)
                .is_some_and(|st| st.generation == generation)
        });
        if !still_current {
            return;
        }

        let next = match result {
            Ok(Ok(v)) => {
                let raw = v["text"]
                    .as_str()
                    .or_else(|| v["body"].as_str())
                    .unwrap_or("");
                OutputPreviewState {
                    loading: false,
                    text: format_output_tail(raw),
                    error: None,
                    generation,
                }
            }
            Ok(Err(e)) => OutputPreviewState {
                loading: false,
                text: String::new(),
                error: Some(format!("Unable to load output: {e:#}")),
                generation,
            },
            Err(e) => OutputPreviewState {
                loading: false,
                text: String::new(),
                error: Some(e.to_string()),
                generation,
            },
        };
        ui.agent_popover.outputs.update(|all| {
            all.insert(terminal_id, next);
        });
    });
}

fn gallery_preview_output(terminal_id: &str) -> String {
    if terminal_id.contains("blocked") || terminal_id.contains("review") {
        [
            "$ cargo test -p canban-ui --lib",
            "   Compiling canban-ui v0.1.0",
            "test components::card_tile::tests ... ok",
            "⚠ Permission required: allow `git push origin feat/scroll`?",
            "  [y] Allow once   [a] Always allow   [n] Deny",
        ]
        .join("\n")
    } else {
        [
            "$ cargo check -p canban-ui",
            "   Checking canban-ui-kit v0.1.0",
            "   Checking canban-ui v0.1.0",
            "Finished `dev` profile in 3.42s",
            "Updating board card agent popover layout...",
        ]
        .join("\n")
    }
}

/// Strips ANSI CSI/OSC escape sequences and returns the trailing non-empty lines
/// capped to `OUTPUT_TAIL_LINES` lines and `OUTPUT_MAX_CHARS` chars.
fn format_output_tail(raw: &str) -> String {
    let cleaned = strip_ansi(raw);
    let mut lines: Vec<&str> = cleaned.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(OUTPUT_TAIL_LINES);
    let joined = lines[start..].join("\n");
    if joined.chars().count() > OUTPUT_MAX_CHARS {
        let skip = joined.chars().count().saturating_sub(OUTPUT_MAX_CHARS);
        let tail: String = joined.chars().skip(skip).collect();
        format!("…{tail}")
    } else {
        joined
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    let mut prev_esc = false;
                    for c in chars.by_ref() {
                        if c == '\x07' || (prev_esc && c == '\\') {
                            break;
                        }
                        prev_esc = c == '\x1b';
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if ch == '\r' {
            continue;
        } else {
            out.push(ch);
        }
    }
    out
}
