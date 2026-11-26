//! Mobile UI (`Needs you`, `Board`, `Terminals`, `Settings`, QR/URI pairing, pushed `Card`,
//! `Terminal`, & `HostDetail` routes) shared across iOS, Android, and Desktop Gallery phone scenes.
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
use canban_types::{DiscoveredCanbanHost, HostSummary, HostTransport, is_tailscale_auth};
use canban_ui_kit::{tokens::*, *};
use nsh_base::{MenuButtonItem, menu_button, nav_page, nav_stack};
use nsh_core::{
    Haptic, HeaderTitleAlign,
    element::{desktop::MenuItem, header::HeaderItem},
    platform::Lifecycle,
    prelude::*,
};
use std::{collections::BTreeSet, rc::Rc};

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
    HostDetail {
        host_id: String,
    },
}

impl MobileRoute {
    fn key(&self) -> String {
        match self {
            Self::Card { card_id, .. } => format!("card:{card_id}"),
            Self::Terminal { terminal_id } => format!("term:{terminal_id}"),
            Self::HostDetail { host_id } => format!("host:{host_id}"),
        }
    }
}

/// Bottom sheet modal destinations on mobile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MobileSheet {
    Pairing(Option<String>),
    DiscoverHosts,
    CreateCard(String),
    MoveCard(String),
    CreateTerminal,
    SyncConflicts,
    Diagnostics(String),
    ProjectPicker,
    Rework(String),
}

impl MobileSheet {
    fn is_medium_default(&self) -> bool {
        matches!(self, Self::ProjectPicker | Self::MoveCard(_))
    }
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
    pub discovered_hosts: Signal<Vec<DiscoveredCanbanHost>>,
    pub discovering_hosts: Signal<bool>,
    pub discover_error: Signal<String>,
    pub joining_host_addrs: Signal<BTreeSet<String>>,
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
            discovered_hosts: signal(Vec::new()),
            discovering_hosts: signal(false),
            discover_error: signal(String::new()),
            joining_host_addrs: signal(BTreeSet::new()),
            force_onboarding: signal(false),
        }
    }

    pub fn select_tab(self, next: MobileTab) {
        if self.tab.get_untracked() == MobileTab::Board
            && next == MobileTab::Board
            && self.stack.with_untracked(Vec::is_empty)
        {
            self.open_sheet(MobileSheet::ProjectPicker);
            return;
        }
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

    pub fn push_host_detail(self, host_id: String) {
        batch(move || {
            self.terminal_fullscreen.set(false);
            self.stack.update(|routes| {
                routes.push(MobileRoute::HostDetail { host_id });
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

/// Returns `true` when `host_id` is configured to use automatic Tailscale `WhoIs` sign-in (`ts_auto`).
pub(crate) fn is_tailscale_auto_host(host_id: &str) -> bool {
    crate::backend::get()
        .and_then(|b| b.saved_hosts().into_iter().find(|h| h.id == host_id))
        .is_some_and(|saved| {
            matches!(
                &saved.transport,
                HostTransport::Tcp { token, .. } if is_tailscale_auth(token)
            )
        })
}

type NavProjectEntry = (String, String, usize);
type NavHostGroup = (String, String, Vec<NavProjectEntry>);

#[derive(Clone, PartialEq)]
struct RootNavModel {
    tab: MobileTab,
    active_project_id: String,
    active_project_name: String,
    unread_count: usize,
    host_projects: Vec<NavHostGroup>,
}

fn build_root_nav_model(ui: Ui, state: MobileState) -> RootNavModel {
    let tab = state.tab.get();
    let selected_pid = ui.project.get();
    let hosts = mobile_hosts(ui);
    let unread_count: usize = ui
        .reminder_counts
        .with(|counts| counts.values().map(|c| c.unread).sum());

    ui.snapshot.with(|s| {
        let active_proj = s
            .projects
            .iter()
            .find(|p| p.id == selected_pid)
            .or_else(|| s.projects.first());
        let active_project_id = active_proj.map(|p| p.id.clone()).unwrap_or_default();
        let active_project_name = active_proj
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Board".to_owned());

        let count_active = |pid: &str| {
            s.cards
                .iter()
                .filter(|c| c.project_id == pid && c.active())
                .count()
        };

        let mut host_projects = Vec::new();
        for host in &hosts {
            let projects: Vec<(String, String, usize)> = s
                .projects
                .iter()
                .filter(|p| s.project_host(&p.id) == host.id)
                .map(|p| (p.id.clone(), p.name.clone(), count_active(&p.id)))
                .collect();
            if !projects.is_empty() {
                host_projects.push((host.id.clone(), host.name.clone(), projects));
            }
        }
        let unowned: Vec<(String, String, usize)> = s
            .projects
            .iter()
            .filter(|p| {
                let h = s.project_host(&p.id);
                !hosts.iter().any(|known| known.id == h)
            })
            .map(|p| (p.id.clone(), p.name.clone(), count_active(&p.id)))
            .collect();
        if !unowned.is_empty() {
            host_projects.push((String::new(), "Projects".to_owned(), unowned));
        }

        RootNavModel {
            tab,
            active_project_id,
            active_project_name,
            unread_count,
            host_projects,
        }
    })
}

fn switch_project(ui: Ui, state: MobileState, pid: String) {
    chrome::trigger_haptic(Haptic::Selection);
    batch(move || {
        ui.project.set(pid);
        state.board_stage.set(String::new());
    });
}

fn root_title(m: &RootNavModel) -> String {
    match m.tab {
        MobileTab::NeedsYou => "Needs you".to_owned(),
        MobileTab::Board => m.active_project_name.clone(),
        MobileTab::Terminals => "Terminals".to_owned(),
        MobileTab::Settings => "Settings".to_owned(),
    }
}

fn root_back_title(tab: MobileTab) -> &'static str {
    match tab {
        MobileTab::NeedsYou => "Inbox",
        MobileTab::Board => "Board",
        MobileTab::Terminals => "Terminals",
        MobileTab::Settings => "Settings",
    }
}

fn render_sheet_modal(ui: Ui, state: MobileState, active: MobileSheet) -> AnyElement {
    let medium_default = active.is_medium_default();
    let close_cb: Rc<dyn Fn()> = Rc::new(move || state.close_sheet());
    let (title, body): (String, AnyElement) = match active {
        MobileSheet::Pairing(_) => (
            "Pair Machine".into(),
            pairing::pairing_panel(ui, state).into_element(),
        ),
        MobileSheet::DiscoverHosts => (
            "Tailnet Hosts".into(),
            pairing::discover_hosts_sheet_body(ui, state).into_element(),
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
    chrome::mobile_sheet(title, medium_default, close_cb, body).into_element()
}

fn android_root_actions(
    ui: Ui,
    state: MobileState,
    nav_model: Memo<RootNavModel>,
) -> impl IntoElement {
    let header_btn = |ic: Icon, label_str: &'static str, on_press: Rc<dyn Fn()>| {
        press_target()
            .w(px(44.))
            .h(px(44.))
            .rounded_full()
            .items_center()
            .justify_center()
            .active(|s| s.opacity(0.65))
            .accessibility_label(Some(SharedString::from(label_str)))
            .child(icon_tinted(ic, ACCENT).w(px(21.)).h(px(21.)))
            .on_press(move |_: &()| {
                chrome::trigger_haptic(Haptic::Selection);
                on_press();
            })
    };

    dynamic(move || -> AnyElement {
        let m = nav_model.get();
        match m.tab {
            MobileTab::NeedsYou => {
                if m.unread_count == 0 {
                    return view().into_element();
                }
                header_btn(
                    Icon::Check,
                    "Mark all read",
                    Rc::new(move || crate::overview::mark_all_read(ui, None)),
                )
                .into_element()
            }
            MobileTab::Board => {
                let mut items: Vec<MenuButtonItem> = m
                    .host_projects
                    .iter()
                    .map(|(_, host_name, projs)| {
                        let children = projs.iter().map(|(pid, pname, count)| {
                            MenuButtonItem::new(format!("proj:{pid}"), format!("{pname} ({count})"))
                                .checked(pid == &m.active_project_id)
                        });
                        MenuButtonItem::section(host_name.clone(), children).single_selection(true)
                    })
                    .collect();
                items.push(MenuButtonItem::section(
                    "",
                    [
                        MenuButtonItem::new("all-projects", "All Projects & Search…")
                            .icon("magnifyingglass"),
                    ],
                ));

                let pid_create = m.active_project_id.clone();
                let has_proj = !pid_create.is_empty();
                let picker = menu_button()
                    .title("Switch Project")
                    .accessibility_label(Some(SharedString::from("Switch Project")))
                    .items(items)
                    .on_select(move |id: &SharedString| {
                        let raw = id.as_ref();
                        if let Some(pid) = raw.strip_prefix("proj:") {
                            switch_project(ui, state, pid.to_owned());
                        } else if raw == "all-projects" {
                            state.open_sheet(MobileSheet::ProjectPicker);
                        }
                    })
                    .child(
                        view()
                            .w(px(44.))
                            .h(px(44.))
                            .items_center()
                            .justify_center()
                            .child(icon_tinted(Icon::Projects, ACCENT).w(px(20.)).h(px(20.))),
                    );

                view()
                    .flex_row()
                    .items_center()
                    .gap_0p5()
                    .child(picker)
                    .child(
                        header_btn(
                            Icon::Plus,
                            "New Card",
                            Rc::new(move || {
                                if has_proj {
                                    state.open_sheet(MobileSheet::CreateCard(pid_create.clone()));
                                }
                            }),
                        )
                        .disabled(move || !has_proj),
                    )
                    .into_element()
            }
            MobileTab::Terminals => header_btn(
                Icon::Plus,
                "New Terminal",
                Rc::new(move || state.open_sheet(MobileSheet::CreateTerminal)),
            )
            .into_element(),
            MobileTab::Settings => header_btn(
                Icon::Plus,
                "Pair Machine",
                Rc::new(move || state.open_pairing(None)),
            )
            .into_element(),
        }
    })
}

fn gallery_root_header(
    ui: Ui,
    state: MobileState,
    nav_model: Memo<RootNavModel>,
) -> impl IntoElement {
    dynamic(move || {
        let m = nav_model.get();
        let right: AnyElement = match m.tab {
            MobileTab::NeedsYou if m.unread_count > 0 => press_target()
                .min_h(px(36.))
                .px_2p5()
                .rounded(px(9.))
                .items_center()
                .justify_center()
                .active(|s| s.opacity(0.65))
                .child(
                    text("Mark all read")
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT),
                )
                .on_press(move |_: &()| crate::overview::mark_all_read(ui, None))
                .into_element(),
            MobileTab::Board => {
                let pid = m.active_project_id.clone();
                let has_proj = !pid.is_empty();
                press_target()
                    .w(px(36.))
                    .h(px(36.))
                    .rounded_full()
                    .bg(ACCENT_FILL)
                    .items_center()
                    .justify_center()
                    .disabled(move || !has_proj)
                    .disabled_style(|s| s.opacity(0.4))
                    .child(icon_tinted(Icon::Plus, ON_ACCENT).w(px(18.)).h(px(18.)))
                    .on_press(move |_: &()| {
                        if has_proj {
                            state.open_sheet(MobileSheet::CreateCard(pid.clone()));
                        }
                    })
                    .into_element()
            }
            MobileTab::Terminals => press_target()
                .w(px(36.))
                .h(px(36.))
                .rounded_full()
                .bg(ACCENT_FILL)
                .items_center()
                .justify_center()
                .child(icon_tinted(Icon::Plus, ON_ACCENT).w(px(18.)).h(px(18.)))
                .on_press(move |_: &()| state.open_sheet(MobileSheet::CreateTerminal))
                .into_element(),
            MobileTab::Settings => press_target()
                .min_h(px(36.))
                .px_3()
                .rounded(px(10.))
                .bg(ACCENT_FILL)
                .flex_row()
                .items_center()
                .gap_1()
                .child(icon_tinted(Icon::Plus, ON_ACCENT).w(px(15.)).h(px(15.)))
                .child(
                    text("Pair")
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ON_ACCENT),
                )
                .on_press(move |_: &()| state.open_pairing(None))
                .into_element(),
            _ => view().into_element(),
        };

        let left: AnyElement = if m.tab == MobileTab::Board {
            press_target()
                .h(px(38.))
                .px_3()
                .rounded(px(10.))
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(
                    text(m.active_project_name.clone())
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT)
                        .truncate(),
                )
                .child(icon_tinted(Icon::ChevronDown, MUTED).w(px(14.)).h(px(14.)))
                .on_press(move |_: &()| state.open_sheet(MobileSheet::ProjectPicker))
                .into_element()
        } else {
            text(root_title(&m))
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD)
                .letter_spacing(px(-0.3))
                .text_color(TEXT)
                .into_element()
        };

        view()
            .w_full()
            .h(px(54.))
            .px_4()
            .border_b_1()
            .border_color(BORDER)
            .bg(SURFACE)
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .child(left)
            .child(right)
    })
}

fn render_pushed_route(ui: Ui, state: MobileState, route: MobileRoute) -> AnyElement {
    match route {
        MobileRoute::Card { card_id, segment } => {
            card::pushed_page(ui, state, card_id, segment).into_element()
        }
        MobileRoute::Terminal { terminal_id } => {
            terminals::pushed_page(ui, state, terminal_id).into_element()
        }
        MobileRoute::HostDetail { host_id } => {
            settings::host_detail_page(ui, state, host_id).into_element()
        }
    }
}

fn ios_tab_right_items(
    ui: Ui,
    state: MobileState,
    root_nav_model: Memo<RootNavModel>,
    tab: MobileTab,
) -> Vec<HeaderItem> {
    match tab {
        MobileTab::NeedsYou => {
            let unread = root_nav_model.get().unread_count;
            if unread == 0 {
                return Vec::new();
            }
            vec![
                HeaderItem::symbol("checkmark.circle")
                    .key("inbox-mark-read")
                    .label("Mark all read")
                    .on_press(move || {
                        chrome::trigger_haptic(Haptic::Selection);
                        crate::overview::mark_all_read(ui, None);
                    }),
            ]
        }
        MobileTab::Board => {
            let m = root_nav_model.get();
            let active_pid = m.active_project_id;
            let pid_create = active_pid.clone();
            let has_proj = !active_pid.is_empty();

            let mut menu_items: Vec<MenuItem> = Vec::new();
            for (_, host_name, projs) in &m.host_projects {
                let children: Vec<MenuItem> = projs
                    .iter()
                    .map(|(pid, pname, count)| {
                        let pid_sel = pid.clone();
                        MenuItem::new(format!("{pname} ({count})"))
                            .checked(pid == &active_pid)
                            .on_select(move || switch_project(ui, state, pid_sel.clone()))
                    })
                    .collect();
                if !children.is_empty() {
                    menu_items.push(MenuItem::submenu(host_name.clone(), children).inline(true));
                }
            }
            if !menu_items.is_empty() {
                menu_items.push(MenuItem::separator());
            }
            menu_items.push(
                MenuItem::new("All Projects & Search…")
                    .symbol("magnifyingglass")
                    .on_select(move || {
                        chrome::trigger_haptic(Haptic::Selection);
                        state.open_sheet(MobileSheet::ProjectPicker);
                    }),
            );

            vec![
                HeaderItem::symbol("plus")
                    .key("board-add-card")
                    .label("New Card")
                    .disabled(!has_proj)
                    .on_press(move || {
                        if has_proj {
                            chrome::trigger_haptic(Haptic::Selection);
                            state.open_sheet(MobileSheet::CreateCard(pid_create.clone()));
                        }
                    }),
                HeaderItem::symbol("folder")
                    .key("board-switch-project")
                    .label("Switch Project")
                    .menu(menu_items),
            ]
        }
        MobileTab::Terminals => vec![
            HeaderItem::symbol("plus")
                .key("terminals-add")
                .label("New Terminal")
                .on_press(move || {
                    chrome::trigger_haptic(Haptic::Selection);
                    state.open_sheet(MobileSheet::CreateTerminal);
                }),
        ],
        MobileTab::Settings => vec![
            HeaderItem::symbol("plus")
                .key("settings-pair")
                .label("Pair Machine")
                .on_press(move || {
                    chrome::trigger_haptic(Haptic::Selection);
                    state.open_pairing(None);
                }),
        ],
    }
}

fn tab_root_content(ui: Ui, state: MobileState, tab: MobileTab) -> AnyElement {
    match tab {
        MobileTab::NeedsYou => inbox::screen(ui, state).into_element(),
        MobileTab::Board => board::screen(ui, state).into_element(),
        MobileTab::Terminals => terminals::screen(ui, state).into_element(),
        MobileTab::Settings => settings::screen(ui, state).into_element(),
    }
}

fn ios_retained_tab_stack(
    ui: Ui,
    state: MobileState,
    root_nav_model: Memo<RootNavModel>,
    tab: MobileTab,
) -> View {
    let key = match tab {
        MobileTab::NeedsYou => "tab-needs-you",
        MobileTab::Board => "tab-board",
        MobileTab::Terminals => "tab-terminals",
        MobileTab::Settings => "tab-settings",
    };

    // Keep one retained `NavStack` per iOS tab so each tab preserves its scroll position and header chrome.
    let large_title = tab != MobileTab::Board;
    let root_page = nav_page()
        .key(key)
        .header_visible(true)
        .header_large_title(large_title)
        .header_shadow_visible(false)
        .header_title_align(HeaderTitleAlign::Center)
        .header_style_with(|s| s.text_color(TEXT).tint_color(ACCENT))
        .title(move || match tab {
            MobileTab::NeedsYou => "Needs you".to_owned(),
            MobileTab::Board => root_nav_model.get().active_project_name,
            MobileTab::Terminals => "Terminals".to_owned(),
            MobileTab::Settings => "Settings".to_owned(),
        })
        .header_back_title(root_back_title(tab))
        .header_right_items(move || ios_tab_right_items(ui, state, root_nav_model, tab))
        .child(
            view()
                .size_full()
                .flex_col()
                .min_h(px(0.))
                .child(tab_root_content(ui, state, tab)),
        );

    let stack = nav_stack().child(root_page).child(each(
        move || {
            if state.tab.get() == tab {
                state.stack.get()
            } else {
                Vec::new()
            }
        },
        MobileRoute::key,
        move |route_sig, _| render_pushed_route(ui, state, route_sig.get_untracked()),
    ));

    view()
        .absolute()
        .inset_0()
        .size_full()
        .style_when(move || state.tab.get() != tab, |s| s.invisible())
        .child(stack.size_full())
}

/// Reusable production mobile shell rendered by [`CanbanMobile`] and Gallery phone scenes.
pub fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let show_onboarding = computed(move || {
        state.force_onboarding.get()
            || (mobile_hosts(ui).is_empty() && ui.snapshot.with(|s| s.projects.is_empty()))
    });
    let has_pushed_routes = computed(move || state.stack.with(|r| !r.is_empty()));
    let root_nav_model = computed(move || build_root_nav_model(ui, state));

    let tab_bar_el = chrome::mobile_tab_bar(
        state.tab,
        move || inbox::badge_count(ui),
        move || has_pushed_routes.get(),
        move |next| state.select_tab(next),
    );

    let main_shell = if chrome::is_ios() {
        view()
            .size_full()
            .relative()
            .bg(BACKGROUND)
            .child(ios_retained_tab_stack(
                ui,
                state,
                root_nav_model,
                MobileTab::NeedsYou,
            ))
            .child(ios_retained_tab_stack(
                ui,
                state,
                root_nav_model,
                MobileTab::Board,
            ))
            .child(ios_retained_tab_stack(
                ui,
                state,
                root_nav_model,
                MobileTab::Terminals,
            ))
            .child(ios_retained_tab_stack(
                ui,
                state,
                root_nav_model,
                MobileTab::Settings,
            ))
            .child(tab_bar_el)
    } else {
        let mut root_page = nav_page()
            .key("mobile-tabs-root")
            .header_visible(true)
            .header_large_title(false)
            .header_shadow_visible(false)
            .header_title_align(if chrome::is_android() {
                HeaderTitleAlign::Left
            } else {
                HeaderTitleAlign::Center
            })
            .header_style_with(|s| s.text_color(TEXT).tint_color(ACCENT))
            .title(move || root_title(&root_nav_model.get()))
            .header_back_title(move || root_back_title(root_nav_model.get().tab))
            .child(
                view()
                    .size_full()
                    .flex_col()
                    .min_h(px(0.))
                    .child(dynamic(move || {
                        tab_root_content(ui, state, state.tab.get())
                    })),
            );

        if chrome::is_android() {
            root_page = root_page.header_right(android_root_actions(ui, state, root_nav_model));
        } else {
            root_page = root_page.header(gallery_root_header(ui, state, root_nav_model));
        }

        let stack = nav_stack().child(root_page).child(each(
            move || state.stack.get(),
            MobileRoute::key,
            move |route_sig, _| render_pushed_route(ui, state, route_sig.get_untracked()),
        ));

        if chrome::is_android() {
            view()
                .size_full()
                .relative()
                .bg(BACKGROUND)
                .child(stack.size_full())
                .child(tab_bar_el)
        } else {
            view()
                .size_full()
                .flex_col()
                .min_h(px(0.))
                .bg(BACKGROUND)
                .child(stack.flex_1().min_h(px(0.)))
                .child(tab_bar_el)
        }
    };

    view()
        .size_full()
        .relative()
        .overflow_hidden()
        .bg(BACKGROUND)
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
                view().hidden().into_element()
            }
        }))
        .child(when(
            move || !ui.operation_error.with(String::is_empty),
            move || {
                view()
                    .absolute()
                    .top(move || {
                        px(if chrome::is_mobile_os() {
                            safe_area().top.0 + 8.0
                        } else {
                            8.0
                        })
                    })
                    .left(px(16.))
                    .right(px(16.))
                    .child(
                        view()
                            .w_full()
                            .rounded(px(chrome::MOBILE_RADIUS))
                            .p_3()
                            .border_1()
                            .border_color(DANGER)
                            .bg(DANGER_SURFACE)
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2p5()
                            .child(
                                text(move || ui.operation_error.get())
                                    .text_size(px(13.5))
                                    .line_height(px(18.))
                                    .text_color(DANGER)
                                    .select_text()
                                    .flex_1()
                                    .min_w(px(0.)),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .flex_shrink_0()
                                    .child(copy_icon_button(
                                        "Copy error message",
                                        "Copy error",
                                        34.,
                                        16.,
                                        CARD_HOVER,
                                        Placement::Bottom,
                                        move || {
                                            let msg = ui.operation_error.get_untracked();
                                            if !msg.is_empty() {
                                                drop(set_clipboard(&msg));
                                                ui.status.set("Copied error to clipboard".into());
                                            }
                                        },
                                    ))
                                    .child(
                                        press_target()
                                            .min_h(px(36.))
                                            .px_2p5()
                                            .rounded(px(8.))
                                            .items_center()
                                            .justify_center()
                                            .active(|s| s.opacity(0.65))
                                            .child(
                                                text("Dismiss")
                                                    .text_size(px(13.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(DANGER),
                                            )
                                            .on_press(move |_: &()| {
                                                ui.operation_error.set(String::new())
                                            }),
                                    ),
                            ),
                    )
            },
        ))
        .child(when(
            move || !ui.toast.with(String::is_empty),
            move || {
                view()
                    .absolute()
                    .bottom(move || {
                        px(if chrome::is_mobile_os() {
                            safe_area().bottom.0 + 76.0
                        } else {
                            72.0
                        })
                    })
                    .left(px(16.))
                    .right(px(16.))
                    .pointer_events(PointerEvents::None)
                    .items_center()
                    .child(
                        view()
                            .px_4()
                            .py_2p5()
                            .rounded_full()
                            .border_1()
                            .border_color(BORDER)
                            .bg(SURFACE)
                            .child(
                                text(move || ui.toast.get())
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT),
                            ),
                    )
            },
        ))
}

fn has_configured_host(ui: Ui) -> bool {
    crate::backend::get().is_some_and(|backend| {
        backend.remote_client().is_none() || !backend.saved_hosts().is_empty()
    }) || ui.hosts.with_untracked(|h| !h.is_empty())
}

/// Standalone mobile application root component hosted on `nsh-platform-ios` (and mobile targets).
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

    // Trigger foreground reconnect & refresh when mobile returns to Active.
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

    // Listen for cold-launch or runtime `canban://` deep links (e.g. from Camera QR scan).
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
