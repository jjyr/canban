//! Mobile UI (`Needs you`, `Board`, `Terminals`, `Settings`, QR/URI pairing, pushed `Card`
//! & `Terminal` routes) shared between the native iOS app and Gallery phone-viewport scenes.
pub mod board;
pub mod card;
pub mod chrome;
pub mod diff;
pub mod inbox;
pub mod pairing;
pub mod settings;
pub mod terminals;

use crate::presenter::Ui;
use canban_client::TransportSecurity;
use canban_types::HostSummary;
use canban_ui_kit::{tokens::*, *};
use nsh_base::{nav_page, nav_stack};
use nsh_core::{platform::Lifecycle, prelude::*};
use std::rc::Rc;

/// Bottom tab bar destinations on mobile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MobileTab {
    #[default]
    NeedsYou,
    Board,
    Terminals,
    Settings,
}

impl MobileTab {
    pub fn index(self) -> u32 {
        match self {
            Self::NeedsYou => 0,
            Self::Board => 1,
            Self::Terminals => 2,
            Self::Settings => 3,
        }
    }

    pub fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(Self::NeedsYou),
            1 => Some(Self::Board),
            2 => Some(Self::Terminals),
            3 => Some(Self::Settings),
            _ => None,
        }
    }
}

/// Active segment inside a pushed Card workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CardSegment {
    #[default]
    Conversation,
    Terminal,
    Diff,
}

impl CardSegment {
    pub fn index(self) -> usize {
        match self {
            Self::Conversation => 0,
            Self::Terminal => 1,
            Self::Diff => 2,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Conversation),
            1 => Some(Self::Terminal),
            2 => Some(Self::Diff),
            _ => None,
        }
    }
}

/// Pushed navigation routes above the root 4-tab view.
#[derive(Clone, Debug, PartialEq)]
pub enum MobileRoute {
    Card {
        card_id: String,
        segment: Signal<CardSegment>,
    },
    Terminal {
        terminal_id: String,
    },
}

impl MobileRoute {
    fn key(&self) -> String {
        match self {
            Self::Card { card_id, .. } => format!("card:{card_id}"),
            Self::Terminal { terminal_id } => format!("term:{terminal_id}"),
        }
    }
}

/// Bottom sheet modal destinations on mobile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MobileSheet {
    Pairing(Option<String>),
    CreateCard(String),
    MoveCard(String),
    CreateTerminal,
    SyncConflicts,
    Diagnostics(String),
    ProjectPicker,
    Rework(String),
}

/// Reactive state for the mobile shell (also constructed directly by Gallery mobile scenes).
#[derive(Clone, Copy)]
pub struct MobileState {
    pub tab: Signal<MobileTab>,
    pub stack: Signal<Vec<MobileRoute>>,
    pub sheet: Signal<Option<MobileSheet>>,
    pub board_stage: Signal<String>,
    pub terminal_fullscreen: Signal<bool>,
    pub pairing_uri_input: Signal<String>,
    pub pairing_probe: Signal<pairing::ProbeState>,
    pub pairing_busy: Signal<bool>,
    pub pairing_error: Signal<String>,
    pub force_onboarding: Signal<bool>,
}

impl Default for MobileState {
    fn default() -> Self {
        Self::new()
    }
}

impl MobileState {
    pub fn new() -> Self {
        Self {
            tab: signal(MobileTab::NeedsYou),
            stack: signal(Vec::new()),
            sheet: signal(None),
            board_stage: signal(String::new()),
            terminal_fullscreen: signal(false),
            pairing_uri_input: signal(String::new()),
            pairing_probe: signal(pairing::ProbeState::Idle),
            pairing_busy: signal(false),
            pairing_error: signal(String::new()),
            force_onboarding: signal(false),
        }
    }

    pub fn select_tab(self, next: MobileTab) {
        batch(move || {
            self.terminal_fullscreen.set(false);
            self.stack.set(Vec::new());
            self.tab.set(next);
        });
    }

    pub fn push_card(self, card_id: String, segment: CardSegment) {
        batch(move || {
            self.terminal_fullscreen.set(false);
            self.stack.update(|routes| {
                routes.push(MobileRoute::Card {
                    card_id,
                    segment: signal(segment),
                });
            });
        });
    }

    pub fn push_terminal(self, terminal_id: String) {
        batch(move || {
            self.terminal_fullscreen.set(false);
            self.stack.update(|routes| {
                routes.push(MobileRoute::Terminal { terminal_id });
            });
        });
    }

    pub fn pop(self) {
        batch(move || {
            self.terminal_fullscreen.set(false);
            self.stack.update(|routes| {
                routes.pop();
            });
        });
    }

    pub fn open_sheet(self, sheet: MobileSheet) {
        if let MobileSheet::Pairing(ref uri_opt) = sheet
            && let Some(uri) = uri_opt
        {
            self.pairing_uri_input.set(uri.clone());
        }
        self.sheet.set(Some(sheet));
    }

    pub fn open_pairing(self, prefill_uri: Option<String>) {
        batch(move || {
            self.pairing_error.set(String::new());
            if let Some(uri) = prefill_uri.clone() {
                self.pairing_uri_input.set(uri);
            } else {
                self.pairing_uri_input.set(String::new());
                self.pairing_probe.set(pairing::ProbeState::Idle);
            }
            self.sheet.set(Some(MobileSheet::Pairing(prefill_uri)));
        });
    }

    pub fn close_sheet(self) {
        batch(move || {
            self.pairing_busy.set(false);
            self.sheet.set(None);
        });
    }
}

/// Returns known hosts on mobile without synthesizing a fake local host when none are paired.
pub fn mobile_hosts(ui: Ui) -> Vec<HostSummary> {
    let direct = ui.hosts.get();
    if !direct.is_empty() {
        return direct;
    }
    let mut from_snap: Vec<HostSummary> = ui
        .snapshot
        .with(|s| s.host_summaries.values().cloned().collect());
    from_snap.sort_by(|a, b| {
        b.is_local
            .cmp(&a.is_local)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    from_snap
}

/// Classifies the active transport security of a host summary for 🔒/⚠ UI badges.
pub fn host_security(_ui: Ui, summary: &HostSummary) -> TransportSecurity {
    if summary.is_local {
        return TransportSecurity::Local;
    }
    if let Some(backend) = crate::backend::get()
        && let Some(saved) = backend
            .saved_hosts()
            .into_iter()
            .find(|h| h.id == summary.id)
    {
        return TransportSecurity::classify_transport(&saved.transport);
    }
    if summary.transport_label.eq_ignore_ascii_case("SSH")
        || summary.transport_label.contains("SSH")
    {
        return TransportSecurity::Ssh;
    }
    TransportSecurity::classify_tcp_addr(&summary.endpoint_label)
}

fn render_sheet_modal(ui: Ui, state: MobileState, active: MobileSheet) -> AnyElement {
    let close_cb: Rc<dyn Fn()> = Rc::new(move || state.close_sheet());
    let (title, body): (String, AnyElement) = match active {
        MobileSheet::Pairing(_) => (
            "Pair Machine".into(),
            pairing::pairing_panel(ui, state).into_element(),
        ),
        MobileSheet::CreateCard(pid) => (
            "New Card".into(),
            board::create_card_sheet_body(ui, state, pid).into_element(),
        ),
        MobileSheet::MoveCard(cid) => (
            "Card Actions".into(),
            board::move_card_sheet_body(ui, state, cid).into_element(),
        ),
        MobileSheet::CreateTerminal => (
            "New Terminal".into(),
            terminals::create_terminal_sheet_body(ui, state).into_element(),
        ),
        MobileSheet::SyncConflicts => (
            "Offline Sync Queue".into(),
            settings::sync_conflicts_sheet_body(ui).into_element(),
        ),
        MobileSheet::Diagnostics(hid) => (
            "Host Diagnostics".into(),
            settings::diagnostics_sheet_body(ui, hid).into_element(),
        ),
        MobileSheet::ProjectPicker => (
            "Switch Project".into(),
            board::project_picker_sheet_body(ui, state).into_element(),
        ),
        MobileSheet::Rework(cid) => (
            "Return for Rework".into(),
            card::rework_sheet_body(ui, state, cid).into_element(),
        ),
    };
    chrome::mobile_sheet(title, close_cb, body).into_element()
}

/// Reusable production mobile shell rendered both by [`CanbanMobile`] and Gallery phone scenes.
pub fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let show_onboarding = computed(move || {
        state.force_onboarding.get()
            || (mobile_hosts(ui).is_empty() && ui.snapshot.with(|s| s.projects.is_empty()))
    });
    let has_pushed_routes = computed(move || state.stack.with(|r| !r.is_empty()));

    let main_shell = view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            nav_stack()
                .flex_1()
                .min_h(px(0.))
                .child(
                    nav_page()
                        .key("mobile-tabs-root")
                        .header_visible(false)
                        .child(view().size_full().flex_col().min_h(px(0.)).child(dynamic(
                            move || match state.tab.get() {
                                MobileTab::NeedsYou => inbox::screen(ui, state).into_element(),
                                MobileTab::Board => board::screen(ui, state).into_element(),
                                MobileTab::Terminals => terminals::screen(ui, state).into_element(),
                                MobileTab::Settings => settings::screen(ui, state).into_element(),
                            },
                        ))),
                )
                .child(each(
                    move || state.stack.get(),
                    MobileRoute::key,
                    move |route_sig, _| match route_sig.get_untracked() {
                        MobileRoute::Card { card_id, segment } => {
                            card::pushed_page(ui, state, card_id, segment).into_element()
                        }
                        MobileRoute::Terminal { terminal_id } => {
                            terminals::pushed_page(ui, state, terminal_id).into_element()
                        }
                    },
                )),
        )
        .child(chrome::mobile_tab_bar(
            state.tab,
            move || inbox::badge_count(ui),
            move || has_pushed_routes.get(),
            move |next| state.select_tab(next),
        ));

    view()
        .size_full()
        .relative()
        .overflow_hidden()
        .bg(BACKGROUND)
        .pt(move || {
            if chrome::is_ios() {
                px(safe_area().top.0)
            } else {
                px(0.0)
            }
        })
        .child(dynamic(move || {
            if show_onboarding.get() {
                pairing::onboarding_screen(ui, state).into_element()
            } else {
                main_shell.clone().into_element()
            }
        }))
        .child(dynamic(move || {
            if let Some(active) = state.sheet.get() {
                render_sheet_modal(ui, state, active)
            } else {
                view().into_element()
            }
        }))
        .child(when(
            move || !ui.operation_error.with(String::is_empty),
            move || {
                view()
                    .absolute()
                    .top(move || {
                        px(if chrome::is_ios() {
                            safe_area().top.0 + 8.0
                        } else {
                            8.0
                        })
                    })
                    .left(px(12.))
                    .right(px(12.))
                    .child(
                        panel()
                            .p_2p5()
                            .border_color(DANGER)
                            .bg(DANGER_SURFACE)
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                label(move || ui.operation_error.get())
                                    .text_size(px(12.))
                                    .text_color(DANGER)
                                    .flex_1()
                                    .min_w(px(0.)),
                            )
                            .child(compact_action("Dismiss", Variant::Quiet, move |_| {
                                ui.operation_error.set(String::new());
                            })),
                    )
            },
        ))
        .child(when(
            move || !ui.toast.with(String::is_empty),
            move || {
                view()
                    .absolute()
                    .bottom(move || {
                        px(if chrome::is_ios() {
                            safe_area().bottom.0 + 68.0
                        } else {
                            68.0
                        })
                    })
                    .left(px(16.))
                    .right(px(16.))
                    .pointer_events(PointerEvents::None)
                    .items_center()
                    .child(
                        view()
                            .px_3()
                            .py_2()
                            .rounded_full()
                            .border_1()
                            .border_color(BORDER)
                            .bg(SURFACE)
                            .child(label(move || ui.toast.get()).text_size(px(12.))),
                    )
            },
        ))
}

fn has_configured_host(ui: Ui) -> bool {
    crate::backend::get().is_some_and(|backend| {
        backend.remote_client().is_none() || !backend.saved_hosts().is_empty()
    }) || ui.hosts.with_untracked(|h| !h.is_empty())
}

/// Standalone mobile application root component hosted on `nsh-platform-ios` (or mobile targets).
#[component]
pub fn CanbanMobile() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    let ui = Ui::new();
    let state = MobileState::new();
    crate::links::install(ui);

    // Seed project selection once snapshots arrive.
    effect(move || {
        if ui.project.get_untracked().is_empty()
            && let Some(pid) = ui
                .snapshot
                .with(|s| s.projects.first().map(|p| p.id.clone()))
        {
            ui.project.set(pid);
        }
    });

    if let Some(mut events_rx) = crate::backend::subscribe() {
        ui.spawn(async move {
            while let Some(first) = events_rx.recv().await {
                let mut events = vec![first];
                while let Ok(more) = events_rx.try_recv() {
                    events.push(more);
                }
                crate::handle_backend_events(ui, events);
            }
        });
    }

    set_interval(std::time::Duration::from_secs(5), move || {
        let has_active_runtime = ui.snapshot.with_untracked(|s| {
            s.workflow_runs.iter().any(|r| {
                r.active()
                    && matches!(
                        r.state.as_str(),
                        "waiting_agent" | "running" | "waiting_script"
                    )
            }) || s.terminals.iter().any(|t| {
                matches!(
                    t.herdr_status(),
                    canban_types::HerdrStatus::Working
                        | canban_types::HerdrStatus::Starting
                        | canban_types::HerdrStatus::Blocked
                )
            })
        });
        if has_active_runtime {
            ui.runtime_clock.set(canban_types::now());
        }
    });

    // Trigger foreground reconnect & refresh when iOS/Android returns to Active.
    let prev_lifecycle = stored(untracked(lifecycle));
    effect(move || {
        let cur = lifecycle();
        let was = prev_lifecycle.replace(cur);
        if cur == Lifecycle::Active && was != Lifecycle::Active {
            crate::backend::on_foreground();
            if has_configured_host(ui) {
                crate::presenter::refresh(ui);
            }
        }
    });

    // Listen for cold-launch or runtime `canban://` deep links (e.g. from iPhone Camera QR scan).
    ui.spawn(async move {
        loop {
            let incoming = next_url_scheme_open().await;
            if incoming.trim().is_empty() {
                break;
            }
            if incoming.trim().starts_with("canban://") {
                if state.force_onboarding.get_untracked() || mobile_hosts(ui).is_empty() {
                    batch(move || {
                        state.pairing_error.set(String::new());
                        state.pairing_uri_input.set(incoming);
                    });
                } else {
                    state.open_pairing(Some(incoming));
                }
            }
        }
    });

    if has_configured_host(ui) {
        crate::presenter::refresh(ui);
    }
    screen(ui, state)
}
