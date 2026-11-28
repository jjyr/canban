//! Phone-viewport (`390×844` / `412×915`) Gallery regression scenes covering every screen
//! specified in `docs/mobile-interaction.md`.
use crate::fixtures;
use canban_client::AddrProbeResult;
use canban_ui::{
    mobile::{CardSegment, MobileSheet, MobileState, MobileTab, pairing::ProbeState},
    presenter::Ui,
};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

pub const SCENES: &[(&str, &str)] = &[
    ("mobile-inbox", "Mobile · Inbox"),
    ("mobile-github-pr", "Mobile · GitHub PR"),
    ("mobile-board", "Mobile · Board"),
    ("mobile-card-conversation", "Mobile · Card"),
    ("mobile-terminal", "Mobile · Terminal"),
    ("mobile-terminals-tab", "Mobile · Terminals"),
    ("mobile-diff", "Mobile · Diff"),
    ("mobile-settings", "Mobile · Settings"),
    ("mobile-pairing", "Mobile · Pairing"),
    ("mobile-sync-conflicts", "Mobile · Sync"),
    ("mobile-offline-states", "Mobile · Offline"),
];

pub fn apply_scene(ui: Ui, state: MobileState, name: &str) {
    state.close_sheet();
    state.force_onboarding.set(false);
    state.stack.set(Vec::new());
    state.board_stage.set(String::new());
    state.terminal_fullscreen.set(false);
    ui.inbox.drawer.set("today".into());
    ui.inbox.focus_filter.set(String::new());
    ui.inbox.undo_banner.set(None);
    ui.sync_preview.set(Vec::new());

    match name {
        "mobile-inbox" => {
            let (_, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            fixtures::daily_inbox(ui, false);
            state.tab.set(MobileTab::Inbox);
        }
        "mobile-github-pr" => {
            let (_, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            fixtures::daily_inbox(ui, false);
            state.tab.set(MobileTab::Inbox);
            let gh_item = ui.daily_inbox.with_untracked(|inbox| {
                inbox
                    .needs_you
                    .iter()
                    .find_map(|n| match &n.kind {
                        canban_notifications::NeedsYouKind::GitHub { item, .. } => {
                            Some((**item).clone())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        inbox
                            .github_stacks
                            .iter()
                            .flat_map(|s| s.items.iter())
                            .next()
                            .cloned()
                    })
            });
            if let Some(item) = gh_item {
                state.push_github_item(item);
            }
        }
        "mobile-board" => {
            let (snap, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            ui.snapshot.set(snap);
            ui.select_project("remote-linux-core".into());
            state.tab.set(MobileTab::Board);
        }
        "mobile-card-conversation" => {
            let (_, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            let (snap, events) = fixtures::conversation();
            ui.snapshot.set(snap);
            fixtures::seed_ui_timelines(ui, &events);
            ui.select_project("gallery-project".into());
            state.tab.set(MobileTab::Board);
            state.push_card("card-1".into(), CardSegment::Conversation);
        }
        "mobile-terminal" => {
            let (_, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            ui.snapshot.set(fixtures::terminal_workspace());
            ui.select_project("gallery-project".into());
            state.tab.set(MobileTab::Terminals);
            let tid = canban_types::Conversation::key("overview-card-2", "developer");
            state.push_terminal(tid);
        }
        "mobile-terminals-tab" => {
            let (_, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            ui.snapshot.set(fixtures::terminal_workspace());
            ui.select_project("gallery-project".into());
            state.tab.set(MobileTab::Terminals);
        }
        "mobile-diff" => {
            let (_, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            let (mut snap, events) = fixtures::conversation();
            snap.cards[0].worktree = Some("/tmp/canban-worktree".into());
            snap.cards[0].branch = Some("feat/mobile-diff".into());
            ui.snapshot.set(snap);
            fixtures::seed_ui_timelines(ui, &events);
            ui.review_state.compare_data.set(fixtures::git_compare());
            ui.select_project("gallery-project".into());
            state.tab.set(MobileTab::Board);
            state.push_card("card-1".into(), CardSegment::Diff);
        }
        "mobile-settings" => {
            let (snap, hosts) = fixtures::multi_host_workspace(false);
            ui.hosts.set(hosts);
            ui.snapshot.set(snap);
            state.tab.set(MobileTab::Settings);
        }
        "mobile-pairing" => {
            ui.hosts.set(Vec::new());
            ui.snapshot.set(canban_types::Snapshot::default());
            let uri = "canban://100.84.0.2:7680?pk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&sid=srv-home-linux&name=Home+Linux#pair_482913";
            state.pairing_uri_input.set(uri.into());
            state.pairing_probe.set(ProbeState::Result(AddrProbeResult {
                addr: "100.84.0.2:7680".into(),
                is_tailscale: true,
                reachable: true,
                rtt_ms: Some(14),
                error: None,
            }));
            state.pairing_error.set(String::new());
            state.force_onboarding.set(true);
        }
        "mobile-sync-conflicts" => {
            let (mut snap, mut hosts) = fixtures::multi_host_workspace(true);
            if let Some(h) = hosts.iter_mut().find(|h| h.id == "home-linux") {
                h.pending_mutations = 2;
                h.sync_conflicts = 1;
                snap.host_summaries.insert(h.id.clone(), h.clone());
            }
            snap.pending_sync
                .insert("remote-card-1".into(), canban_types::SyncMark::Conflict);
            ui.hosts.set(hosts);
            ui.snapshot.set(snap);
            ui.sync_preview.set(fixtures::sync_preview_items());
            ui.select_project("remote-linux-core".into());
            state.tab.set(MobileTab::Settings);
            state.open_sheet(MobileSheet::SyncConflicts);
        }
        "mobile-offline-states" => {
            let (mut snap, mut hosts) = fixtures::multi_host_workspace(true);
            if let Some(h) = hosts.iter_mut().find(|h| h.id == "home-linux") {
                h.pending_mutations = 2;
                snap.host_summaries.insert(h.id.clone(), h.clone());
            }
            if let Some(c) = snap
                .cards
                .iter_mut()
                .find(|c| c.project_id == "remote-linux-core")
            {
                snap.pending_sync
                    .insert(c.id.clone(), canban_types::SyncMark::Pending);
            }
            ui.hosts.set(hosts);
            ui.snapshot.set(snap);
            ui.select_project("remote-linux-core".into());
            state.tab.set(MobileTab::Board);
        }
        _ => {}
    }
}

pub fn apply_website_showcase(ui: Ui, state: MobileState, name: &str) {
    state.close_sheet();
    state.force_onboarding.set(false);
    state.stack.set(Vec::new());
    state.board_stage.set(String::new());
    state.terminal_fullscreen.set(false);
    ui.inbox.drawer.set("today".into());
    ui.inbox.focus_filter.set(String::new());
    ui.inbox.undo_banner.set(None);
    ui.sync_preview.set(Vec::new());

    let (_, hosts) = fixtures::multi_host_workspace(false);
    ui.hosts.set(hosts);
    fixtures::website_showcase(ui, "website-inbox");

    match name {
        "website-mobile-inbox" => {
            state.tab.set(MobileTab::Inbox);
        }
        "website-mobile-board" => {
            fixtures::website_showcase(ui, "website-board");
            state.tab.set(MobileTab::Board);
            state.board_stage.set("develop".into());
        }
        "website-mobile-terminal" => {
            state.tab.set(MobileTab::Terminals);
            state.push_terminal("showcase-claude-1".into());
        }
        _ => {
            state.tab.set(MobileTab::Inbox);
        }
    }
}

pub fn viewport_wrapper(
    ui: Ui,
    state: MobileState,
    viewport_preset: Signal<(f32, f32)>,
) -> impl IntoElement {
    let presets = [
        ("iPhone (390×844)", 390.0_f32, 844.0_f32),
        ("Android (412×915)", 412.0_f32, 915.0_f32),
    ];
    view()
        .flex_1()
        .w_full()
        .min_h(px(0.))
        .bg(INSET)
        .items_center()
        .gap_3()
        .p_3()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(presets.into_iter().map(move |(title, w, h)| {
                    compact_action(title, Variant::Secondary, move |_| {
                        viewport_preset.set((w, h))
                    })
                })),
        )
        .child(dynamic(move || {
            let (w, h) = viewport_preset.get();
            view()
                .w(px(w))
                .h(px(h))
                .rounded(px(28.))
                .border_1()
                .border_color(BORDER)
                .bg(BACKGROUND)
                .overflow_hidden()
                .child(canban_ui::mobile::screen(ui, state))
        }))
}
