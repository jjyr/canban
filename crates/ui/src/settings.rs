//! Settings workspace: middle section-jump column, single outer scroll, and
//! Acknowledgements & Licenses section.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub const SECTION_COUNT: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionId {
    Theme,
    Terminal,
    Hosts,
    Environment,
    Diagnostics,
    Updates,
    Scheduling,
    TerminalEnv,
    Agents,
    Herdr,
    Storage,
    Acknowledgements,
}

impl SectionId {
    pub const ALL: [Self; SECTION_COUNT] = [
        Self::Theme,
        Self::Terminal,
        Self::Hosts,
        Self::Environment,
        Self::Diagnostics,
        Self::Updates,
        Self::Scheduling,
        Self::TerminalEnv,
        Self::Agents,
        Self::Herdr,
        Self::Storage,
        Self::Acknowledgements,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Theme => 0,
            Self::Terminal => 1,
            Self::Hosts => 2,
            Self::Environment => 3,
            Self::Diagnostics => 4,
            Self::Updates => 5,
            Self::Scheduling => 6,
            Self::TerminalEnv => 7,
            Self::Agents => 8,
            Self::Herdr => 9,
            Self::Storage => 10,
            Self::Acknowledgements => 11,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Terminal => "terminal",
            Self::Hosts => "hosts",
            Self::Environment => "environment",
            Self::Diagnostics => "diagnostics",
            Self::Updates => "updates",
            Self::Scheduling => "scheduling",
            Self::TerminalEnv => "terminal-env",
            Self::Agents => "agents",
            Self::Herdr => "herdr",
            Self::Storage => "storage",
            Self::Acknowledgements => "acknowledgements",
        }
    }

    pub fn from_id(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        Self::ALL
            .into_iter()
            .find(|s| s.id().eq_ignore_ascii_case(trimmed))
    }

    pub const fn nav_label(self) -> &'static str {
        match self {
            Self::Theme => "Theme & support",
            Self::Terminal => "Terminal",
            Self::Hosts => "Hosts",
            Self::Environment => "Environment & Secrets",
            Self::Diagnostics => "Diagnostics",
            Self::Updates => "Updates",
            Self::Scheduling => "Agent scheduling",
            Self::TerminalEnv => "Terminal Environment",
            Self::Agents => "Agents & Identities",
            Self::Herdr => "Herdr",
            Self::Storage => "Storage",
            Self::Acknowledgements => "Acknowledgements",
        }
    }

    pub const fn group_heading(self) -> Option<&'static str> {
        match self {
            Self::Theme => Some("APP"),
            Self::Scheduling => Some("HOST SETTINGS"),
            Self::Acknowledgements => None,
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpPin {
    pub section: SectionId,
    pub started_at_ms: u64,
    pub last_scroll_y: f32,
    pub reached: bool,
}

pub fn clamped_target_y(
    section: SectionId,
    offsets: &[f32; SECTION_COUNT],
    content_h: f32,
    viewport_h: f32,
) -> f32 {
    let raw = if section == SectionId::Theme {
        0.0
    } else {
        offsets[section.index()].max(0.0)
    };
    if content_h > 0.0 && viewport_h > 0.0 {
        let max_scroll = (content_h - viewport_h).max(0.0);
        raw.clamp(0.0, max_scroll)
    } else {
        raw
    }
}

pub fn advance_jump_pin(
    pin: JumpPin,
    scroll_y: f32,
    now_ms: u64,
    offsets: &[f32; SECTION_COUNT],
    content_h: f32,
    viewport_h: f32,
) -> Option<JumpPin> {
    let target = clamped_target_y(pin.section, offsets, content_h, viewport_h);
    let dist = (scroll_y - target).abs();
    let prev_dist = (pin.last_scroll_y - target).abs();
    let settled_by_time = now_ms.saturating_sub(pin.started_at_ms) > 500;
    let reached = pin.reached || dist <= 6.0 || settled_by_time;
    if reached {
        if dist > 12.0 {
            return None;
        }
        return Some(JumpPin {
            last_scroll_y: scroll_y,
            reached: true,
            ..pin
        });
    }
    if dist > prev_dist + 6.0 {
        return None;
    }
    Some(JumpPin {
        last_scroll_y: scroll_y,
        reached,
        ..pin
    })
}

pub fn scroll_spy_section(
    scroll_y: f32,
    offsets: &[f32; SECTION_COUNT],
    content_h: f32,
    viewport_h: f32,
) -> SectionId {
    if content_h > viewport_h + 8.0 && viewport_h > 0.0 && scroll_y + viewport_h >= content_h - 12.0
    {
        return SectionId::Acknowledgements;
    }
    let reading_line = scroll_y + 72.0;
    let mut current = SectionId::Theme;
    for section in SectionId::ALL {
        let y = offsets[section.index()];
        if y <= reading_line && (section == SectionId::Theme || y > 0.0) {
            current = section;
        }
    }
    current
}

pub fn resolve_active_section(
    pin: Option<JumpPin>,
    scroll_y: f32,
    offsets: &[f32; SECTION_COUNT],
    content_h: f32,
    viewport_h: f32,
) -> SectionId {
    if let Some(pin) = pin {
        return pin.section;
    }
    scroll_spy_section(scroll_y, offsets, content_h, viewport_h)
}

#[derive(Clone, Copy)]
pub struct CreditEntry {
    pub name: &'static str,
    pub subtitle: &'static str,
    pub license: &'static str,
    pub description: &'static str,
    pub links: &'static [(&'static str, &'static str)],
}

pub const RUNTIME_CREDITS: &[CreditEntry] = &[
    CreditEntry {
        name: "Herdr",
        subtitle: "herdr-core",
        license: "Apache-2.0",
        description: "Persistent terminal session daemon, JSON-RPC protocol, and runtime supervision.",
        links: &[("GitHub", "https://github.com/herdrdev/herdr-core")],
    },
    CreditEntry {
        name: "sheprd",
        subtitle: "formerly herdr-gpui",
        license: "Apache-2.0",
        description: "Referenced for VT/frame/input terminal integration, subscription usage contracts, host power utilities, and provider marks.",
        links: &[("GitHub", "https://github.com/sheprd-labs/sheprd")],
    },
    CreditEntry {
        name: "NSH",
        subtitle: "longbridge/nsh",
        license: "Apache-2.0",
        description: "Cross-platform native Rust UI framework powering Canban's desktop interface.",
        links: &[("GitHub", "https://github.com/longbridge/nsh")],
    },
    CreditEntry {
        name: "Ghostty",
        subtitle: "libghostty-vt",
        license: "MIT",
        description: "Terminal VT parser and state machine built from ghostty-org/ghostty.",
        links: &[("GitHub", "https://github.com/ghostty-org/ghostty")],
    },
];

pub const ASSET_CREDITS: &[CreditEntry] = &[
    CreditEntry {
        name: "CodexBar",
        subtitle: "",
        license: "MIT",
        description: "Upstream subscription quota probe contracts referenced via sheprd (herdr-gpui).",
        links: &[("GitHub", "https://github.com/steipete/CodexBar")],
    },
    CreditEntry {
        name: "iTerm2-Color-Schemes",
        subtitle: "",
        license: "MIT",
        description: "16 classic terminal ANSI palettes adapted from Ghostty-format themes.",
        links: &[(
            "GitHub",
            "https://github.com/mbadolato/iTerm2-Color-Schemes",
        )],
    },
    CreditEntry {
        name: "Lucide Icons",
        subtitle: "",
        license: "ISC",
        description: "24-unit stroke interface icons rendered natively across Canban.",
        links: &[("GitHub", "https://github.com/lucide-icons/lucide")],
    },
    CreditEntry {
        name: "Lobe Icons & Primer Octicons",
        subtitle: "",
        license: "MIT",
        description: "Provider brand (monochrome & multi-color) and GitHub vector marks.",
        links: &[
            ("Lobe Icons", "https://github.com/lobehub/lobe-icons"),
            ("Octicons", "https://github.com/primer/octicons"),
        ],
    },
];

struct BundledDoc {
    title: &'static str,
    markdown: bool,
    content: &'static str,
}

const BUNDLED_DOCS: &[BundledDoc] = &[
    BundledDoc {
        title: "Third-Party Notices",
        markdown: true,
        content: include_str!("../../../THIRD_PARTY_NOTICES.md"),
    },
    BundledDoc {
        title: "Canban · Apache-2.0",
        markdown: false,
        content: include_str!("../../../LICENSE"),
    },
    BundledDoc {
        title: "Herdr & sheprd · Apache-2.0",
        markdown: false,
        content: include_str!("../../../third_party/Herdr-LICENSE.txt"),
    },
    BundledDoc {
        title: "Ghostty · MIT",
        markdown: false,
        content: include_str!("../../../third_party/Ghostty-LICENSE.txt"),
    },
    BundledDoc {
        title: "Lucide · ISC",
        markdown: false,
        content: include_str!("../../../third_party/Lucide-LICENSE.txt"),
    },
    BundledDoc {
        title: "Octicons · MIT",
        markdown: false,
        content: include_str!("../../../third_party/octicons-LICENSE.txt"),
    },
    BundledDoc {
        title: "iTerm2 Themes · MIT",
        markdown: false,
        content: include_str!("../../../third_party/iTerm2-Color-Schemes-LICENSE.txt"),
    },
    BundledDoc {
        title: "Provider Marks Notice",
        markdown: true,
        content: include_str!("../../ui-kit/assets/providers/NOTICE.md"),
    },
];

fn credit_card(ui: Ui, group_title: &'static str, entries: &'static [CreditEntry]) -> View {
    let mut rows = view().min_w(px(0.)).gap_0();
    for (idx, entry) in entries.iter().enumerate() {
        let mut title_row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(label(entry.name).font_weight(FontWeight::SEMIBOLD));
        if !entry.subtitle.is_empty() {
            title_row = title_row.child(
                muted(entry.subtitle)
                    .font_family(MONO_FONT)
                    .text_size(px(11.5)),
            );
        }

        let mut actions_row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .child(badge(entry.license));
        for &(link_label, url) in entry.links {
            actions_row = actions_row.child(
                view().tooltip(hint_tooltip(url)).child(
                    compact_action(format!("{link_label} ↗"), Variant::Quiet, move |_| {
                        crate::links::open(ui, url);
                    })
                    .accessibility_label(format!("Open {} {}", entry.name, link_label)),
                ),
            );
        }

        let mut item = view()
            .px_3p5()
            .py_3()
            .gap_1()
            .min_w(px(0.))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .min_w(px(0.))
                    .child(title_row)
                    .child(actions_row),
            )
            .child(muted(entry.description));
        if idx + 1 < entries.len() {
            item = item.border_b_1().border_color(BORDER);
        }
        rows = rows.child(item);
    }

    panel()
        .p_0()
        .gap_0()
        .border_1()
        .border_color(BORDER)
        .overflow_hidden()
        .min_w(px(0.))
        .child(
            view()
                .px_3p5()
                .py_2()
                .bg(INSET)
                .border_b_1()
                .border_color(BORDER)
                .child(muted(group_title).font_weight(FontWeight::SEMIBOLD)),
        )
        .child(rows)
}

pub fn acknowledgements_section(ui: Ui) -> impl IntoElement {
    let notices_open = signal(false);
    let selected_doc = signal(0_usize);

    settings_section(
        "Acknowledgements & Licenses",
        "Canban is Copyright 2026 JJy <jjyruby@gmail.com>, licensed under Apache-2.0. Built with and inspired by open-source software:",
    )
    .trailing(badge("Apache-2.0"))
    .child(credit_card(ui, "Core & UI Runtime", RUNTIME_CREDITS))
    .child(credit_card(ui, "Assets & Upstream Credits", ASSET_CREDITS))
    .child(dynamic(move || {
        let open = notices_open.get();
        let body = if open {
            view()
                .gap_3()
                .pt_1()
                .min_w(px(0.))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .min_w(px(0.))
                        .child(segmented().children(
                            BUNDLED_DOCS.iter().enumerate().map(|(idx, doc)| {
                                compact_segment(
                                    doc.title,
                                    None,
                                    move || selected_doc.get() == idx,
                                    move |_| selected_doc.set(idx),
                                )
                            }),
                        ))
                        .child(compact_action("Copy text", Variant::Quiet, move |_| {
                            let doc = &BUNDLED_DOCS[selected_doc.get_untracked()];
                            copy_text(ui, doc.content.to_owned());
                        })),
                )
                .child(dynamic(move || {
                    let doc = &BUNDLED_DOCS[selected_doc.get()];
                    let content = if doc.markdown {
                        crate::links::reader(ui, move || doc.content.to_owned()).into_element()
                    } else {
                        text(doc.content)
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(MUTED)
                            .min_w(px(0.))
                            .into_element()
                    };
                    panel()
                        .bg(INSET)
                        .border_1()
                        .border_color(BORDER)
                        .p_3p5()
                        .min_w(px(0.))
                        .child(content)
                        .into_element()
                }))
                .into_element()
        } else {
            view().into_element()
        };

        collapsible_section_group(
            "Third-Party Notices & License Texts",
            Some(BUNDLED_DOCS.len()),
            open,
            move |_| notices_open.update(|v| *v = !*v),
            body,
        )
        .into_element()
    }))
}

#[derive(Clone)]
struct SyncBarPresentation {
    status_text: String,
    button_label: &'static str,
    variant: Variant,
    disabled: bool,
    highlights_unsynced: bool,
}

fn describe_sync_bar(
    status: &canban_types::GlobalEnvSyncStatus,
    dirty: bool,
) -> SyncBarPresentation {
    let total = status.total_remote_hosts;
    let synced_count = status.synced_hosts.len();
    if dirty {
        return SyncBarPresentation {
            status_text: "Save changes before syncing.".into(),
            button_label: "Sync to hosts",
            variant: Variant::Secondary,
            disabled: true,
            highlights_unsynced: false,
        };
    }
    if total == 0 {
        return SyncBarPresentation {
            status_text:
                "Local only — connect a remote host in Hosts to replicate global env & secrets."
                    .into(),
            button_label: "Sync to hosts",
            variant: Variant::Secondary,
            disabled: true,
            highlights_unsynced: false,
        };
    }
    if !status.syncing_hosts.is_empty() {
        let mut msg = format!(
            "Syncing to {} in background…",
            status.syncing_hosts.join(", ")
        );
        if !status.queued_hosts.is_empty() {
            msg.push_str(&format!(
                " Queued for offline: {}.",
                status.queued_hosts.join(", ")
            ));
        }
        return SyncBarPresentation {
            status_text: msg,
            button_label: "Syncing…",
            variant: Variant::Secondary,
            disabled: true,
            highlights_unsynced: false,
        };
    }
    if !status.failed_hosts.is_empty() {
        let failed_names: Vec<&str> = status
            .failed_hosts
            .iter()
            .map(|f| f.host_name.as_str())
            .collect();
        let mut msg = format!("Sync failed for {}.", failed_names.join(", "));
        if synced_count > 0 {
            msg.push_str(&format!(" {synced_count}/{total} hosts synced."));
        }
        if !status.unsynced_hosts.is_empty() {
            msg.push_str(&format!(
                " Not yet synced: {}.",
                status.unsynced_hosts.join(", ")
            ));
        }
        return SyncBarPresentation {
            status_text: msg,
            button_label: "Retry sync",
            variant: Variant::Primary,
            disabled: false,
            highlights_unsynced: false,
        };
    }
    if !status.queued_hosts.is_empty() {
        let mut parts = Vec::new();
        if synced_count > 0 {
            parts.push(format!("{synced_count}/{total} synced"));
        }
        parts.push(format!(
            "Queued for offline: {} (will sync automatically on reconnect)",
            status.queued_hosts.join(", ")
        ));
        if !status.unsynced_hosts.is_empty() {
            parts.push(format!(
                "Not yet synced: {}",
                status.unsynced_hosts.join(", ")
            ));
        }
        return SyncBarPresentation {
            status_text: format!("{}.", parts.join(" · ")),
            button_label: "Sync to hosts",
            variant: if status.unsynced_hosts.is_empty() {
                Variant::Secondary
            } else {
                Variant::Primary
            },
            disabled: false,
            highlights_unsynced: !status.unsynced_hosts.is_empty(),
        };
    }
    if !status.unsynced_hosts.is_empty() {
        let msg = if synced_count == 0 {
            format!(
                "Not yet synced to {total} host{}: {}.",
                if total == 1 { "" } else { "s" },
                status.unsynced_hosts.join(", ")
            )
        } else {
            format!(
                "Not yet synced to {} ({synced_count}/{total} synced).",
                status.unsynced_hosts.join(", ")
            )
        };
        return SyncBarPresentation {
            status_text: msg,
            button_label: "Sync to hosts",
            variant: Variant::Primary,
            disabled: false,
            highlights_unsynced: true,
        };
    }
    SyncBarPresentation {
        status_text: format!(
            "Synced to {synced_count}/{total} host{} ({}).",
            if total == 1 { "" } else { "s" },
            status.synced_hosts.join(", ")
        ),
        button_label: "Sync to hosts",
        variant: Variant::Secondary,
        disabled: false,
        highlights_unsynced: false,
    }
}

/// Reusable bottom cross-host sync status & trigger bar for Global Environment & Secrets.
pub fn global_env_sync_bar(
    status: canban_types::GlobalEnvSyncStatus,
    dirty: bool,
    trigger_error: Option<&str>,
    on_sync: impl Fn() + 'static,
) -> impl IntoElement {
    let pres = describe_sync_bar(&status, dirty);
    let status_line = if !status.failed_hosts.is_empty() || trigger_error.is_some() {
        text(pres.status_text)
            .text_size(px(12.))
            .text_color(DANGER)
            .into_element()
    } else if pres.highlights_unsynced {
        text(pres.status_text)
            .text_size(px(12.))
            .text_color(ATTENTION)
            .into_element()
    } else {
        muted(pres.status_text).text_size(px(12.)).into_element()
    };

    let mut box_view = view()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(INSET)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    view()
                        .flex_col()
                        .gap_0p5()
                        .flex_1()
                        .min_w_0()
                        .child(
                            label("Cross-host sync")
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(status_line),
                )
                .child(
                    compact_action_with_icon(
                        Icon::Refresh,
                        pres.button_label,
                        pres.variant,
                        move |_| on_sync(),
                    )
                    .flex_shrink_0()
                    .disabled(pres.disabled),
                ),
        );

    for failure in &status.failed_hosts {
        box_view = box_view.child(notice(
            format!("{}: {}", failure.host_name, failure.message),
            Tone::Danger,
        ));
    }
    if let Some(err) = trigger_error.filter(|s| !s.trim().is_empty()) {
        box_view = box_view.child(notice(err.to_owned(), Tone::Danger));
    }
    box_view
}

pub(crate) fn global_env_section(ui: Ui) -> impl IntoElement {
    let local_host = signal("local".to_owned());
    let scope_key = computed(|| "global".to_owned());
    let source_meta = computed(move || ui.snapshot.with(|s| s.global_env.clone()));
    let on_save =
        crate::components::env_table::env_save_callback(ui, local_host, "global", || None);
    let env_dirty = signal(false);
    let sync_error = signal(String::new());
    let triggering = signal(false);

    settings_section(
        "Environment & Secrets",
        "Global secrets (${{ secrets.KEY }}) and variables (${{ vars.KEY }}) injected into Agents, Scripts, and Terminals; replicated on demand via Cross-host sync.",
    )
    .child(crate::components::env_table::env_table(
        ui,
        crate::components::env_table::EnvTableConfig {
            host_id: local_host,
            project_id: None,
            scope_key,
            source_meta,
            save_label: "Save Global Env",
            on_save,
            dirty_out: Some(env_dirty),
        },
    ))
    .child(dynamic(move || {
        let _ = ui.env_sync_epoch.get();
        let _ = ui.snapshot_epoch.get();
        let _ = ui.hosts.get();
        let is_dirty = env_dirty.get();
        let is_triggering = triggering.get();
        let err_msg = sync_error.get();
        let mut status = crate::backend::global_env_sync_status();
        if is_triggering
            && status.total_remote_hosts > 0
            && status.syncing_hosts.is_empty()
            && !is_dirty
        {
            let mut active: Vec<String> = Vec::new();
            active.append(&mut status.unsynced_hosts);
            active.append(&mut status.synced_hosts);
            active.extend(status.failed_hosts.drain(..).map(|f| f.host_name));
            if active.is_empty() {
                active.push(format!(
                    "{} remote host{}",
                    status.total_remote_hosts,
                    if status.total_remote_hosts == 1 { "" } else { "s" }
                ));
            }
            status.syncing_hosts = active;
        }
        let trigger_err = (!err_msg.is_empty()).then_some(err_msg.as_str());
        global_env_sync_bar(status, is_dirty, trigger_err, move || {
            if triggering.get_untracked() || env_dirty.get_untracked() {
                return;
            }
            triggering.set(true);
            sync_error.set(String::new());
            ui.spawn(async move {
                let res = blocking(crate::backend::trigger_global_env_sync).await;
                triggering.set(false);
                match res {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => sync_error.set(format!("{e:#}")),
                    Err(e) => sync_error.set(format!("Sync trigger failed: {e}")),
                }
                ui.env_sync_epoch.update(|v| *v += 1);
            });
        })
        .into_element()
    }))
    .child(
        muted("Secrets are encrypted locally per host (vault.key + SQLite WAL) and replicated on demand over the unified host Outbox with zero plaintext secrets on disk.")
            .text_size(px(12.)),
    )
}

fn terminal_env_section(ui: Ui) -> impl IntoElement {
    use canban_types::HostTerminalEnvMode;

    let save_error = signal(String::new());
    let refresh_note = signal(Option::<(String, Tone)>::None);
    let tracked_host = signal(ui.settings_host.get_untracked());
    effect(move || {
        let cur_host = ui.settings_host.get();
        if tracked_host.get_untracked() != cur_host {
            tracked_host.set(cur_host);
            save_error.set(String::new());
            refresh_note.set(None);
        }
    });

    let save_mode = move |mode: HostTerminalEnvMode| {
        if ui.is_method_pending("config.agents.save")
            || ui.terminal_env_mode.get_untracked() == mode
        {
            return;
        }
        dispatch_complete(
            ui,
            "config.agents.save",
            json!({
                "op": "set_terminal_env_mode",
                "terminal_env_mode": mode.as_str(),
            }),
            move |res| match res {
                Ok(_) => {
                    ui.terminal_env_mode.set(mode);
                    save_error.try_set(String::new());
                }
                Err(e) => {
                    save_error.try_set(e);
                }
            },
        );
    };

    settings_section(
        "Terminal Environment",
        "Host login-shell environment inherited by automated Agent and Script workflow steps.",
    )
    .trailing(
        compact_action(
            move || {
                if ui.is_method_pending("host.env.refresh") {
                    "Refreshing…"
                } else {
                    "Refresh"
                }
            },
            Variant::Secondary,
            move |_| {
                if ui.is_method_pending("host.env.refresh") {
                    return;
                }
                dispatch_complete(ui, "host.env.refresh", json!({}), move |res| match res {
                    Ok(v) => {
                        let shell = v["shell"].as_str().unwrap_or("shell");
                        let var_count = v["var_count"].as_u64().unwrap_or(0);
                        let full = v["full_env_captured"].as_bool().unwrap_or(false);
                        let (msg, tone) = if full {
                            (
                                format!(
                                    "Captured {var_count} environment variables from {shell}."
                                ),
                                Tone::Success,
                            )
                        } else {
                            (
                                format!(
                                    "Captured PATH from {shell}; full environment snapshot was unavailable."
                                ),
                                Tone::Attention,
                            )
                        };
                        refresh_note.try_set(Some((msg, tone)));
                    }
                    Err(e) => {
                        refresh_note.try_set(Some((e, Tone::Danger)));
                    }
                });
            },
        )
        .disabled(move || ui.is_method_pending("host.env.refresh")),
    )
    .child(settings_field(
        "Environment mode",
        "Trusted automated runs inherit your login-shell environment by default, or can be restricted to minimal PATH-only env.",
        segmented()
            .child(
                segment(
                    "Inherit User Terminal Environment",
                    None,
                    move || ui.terminal_env_mode.get() == HostTerminalEnvMode::Inherit,
                    move |_| save_mode(HostTerminalEnvMode::Inherit),
                )
                .disabled(move || ui.is_method_pending("config.agents.save")),
            )
            .child(
                segment(
                    "Minimal (PATH only)",
                    None,
                    move || ui.terminal_env_mode.get() == HostTerminalEnvMode::Minimal,
                    move |_| save_mode(HostTerminalEnvMode::Minimal),
                )
                .disabled(move || ui.is_method_pending("config.agents.save")),
            ),
    ))
    .child(muted(
        "External fork PRs (external_pr) and untrusted external cardless runs default to minimal env unless overridden in workflow/step env:. Interactive Card Terminals remain unrestricted.",
    ))
    .child(dynamic(move || {
        let err = save_error.get();
        if !err.is_empty() {
            return notice(err, Tone::Danger).into_element();
        }
        if let Some((msg, tone)) = refresh_note.get() {
            return notice(msg, tone).into_element();
        }
        view().into_element()
    }))
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let active_host_home = move || {
        let hid = ui.settings_host.get();
        crate::backend::home_for(&hid)
    };
    let config_path = format!("{}/config/agents.json", untracked(active_host_home));

    let scroll_ref = element_ref();
    let hydrated = signal(false);
    let offsets = signal([0.0_f32; SECTION_COUNT]);
    let viewport_h = signal(0.0_f32);
    let content_h = signal(0.0_f32);
    let scroll_y = signal(0.0_f32);
    let initial_target = SectionId::from_id(&ui.settings_section.get_untracked());
    let diagnostics_activated = signal(initial_target == Some(SectionId::Diagnostics));
    let jump_pin = signal(initial_target.map(|section| JumpPin {
        section,
        started_at_ms: canban_types::now(),
        last_scroll_y: 0.0,
        reached: false,
    }));

    ui.spawn(async move {
        next_frame().await;
        let _ = hydrated.try_set(true);
    });

    let maybe_activate_diagnostics = move |y: f32, vh: f32| {
        if diagnostics_activated.get_untracked() || vh <= 0.0 {
            return;
        }
        let diag_y = offsets.get_untracked()[SectionId::Diagnostics.index()];
        let threshold = if y > 12.0 { 40.0 } else { 180.0 };
        if diag_y > 0.0 && y + vh >= diag_y + threshold {
            diagnostics_activated.set(true);
        }
    };

    let active = computed(move || {
        resolve_active_section(
            jump_pin.get(),
            scroll_y.get(),
            &offsets.get(),
            content_h.get(),
            viewport_h.get(),
        )
    });

    let jump_to = move |section: SectionId, animated: bool| {
        if section == SectionId::Diagnostics {
            diagnostics_activated.set(true);
        }
        if !hydrated.get_untracked() {
            ui.settings_section.set(section.id().into());
            return;
        }
        let cur_y = scroll_y.get_untracked();
        let offs = offsets.get_untracked();
        let ch = content_h.get_untracked();
        let vh = viewport_h.get_untracked();
        let target_y = clamped_target_y(section, &offs, ch, vh);
        jump_pin.set(Some(JumpPin {
            section,
            started_at_ms: canban_types::now(),
            last_scroll_y: cur_y,
            reached: (cur_y - target_y).abs() <= 6.0,
        }));
        scroll_ref.scroll_to(
            Point::new(px(0.), px(target_y)),
            animated && !canban_ui_kit::motion::reduced(),
        );
    };

    effect(move || {
        let requested = ui.settings_section.get();
        let Some(section) = SectionId::from_id(&requested) else {
            return;
        };
        if section == SectionId::Diagnostics {
            diagnostics_activated.set(true);
        }
        ui.settings_section.set(String::new());
        jump_pin.set(Some(JumpPin {
            section,
            started_at_ms: canban_types::now(),
            last_scroll_y: scroll_y.get_untracked(),
            reached: false,
        }));
        ui.spawn(async move {
            while hydrated.try_get() == Some(false) {
                next_frame().await;
            }
            next_frame().await;
            next_frame().await;
            if jump_pin.try_get().is_none() || ui.page.get_untracked() != "settings" {
                return;
            }
            let offs = offsets.get_untracked();
            let ch = content_h.get_untracked();
            let vh = viewport_h.get_untracked();
            let target_y = clamped_target_y(section, &offs, ch, vh);
            let cur_y = scroll_y.get_untracked();
            jump_pin.set(Some(JumpPin {
                section,
                started_at_ms: canban_types::now(),
                last_scroll_y: cur_y,
                reached: (cur_y - target_y).abs() <= 6.0,
            }));
            scroll_ref.scroll_to(Point::new(px(0.), px(target_y)), false);
        });
    });

    let wrap_section = move |section: SectionId, body: AnyElement| -> AnyElement {
        let idx = section.index();
        view()
            .min_w(px(0.))
            .on_layout(move |b: &Bounds<Points>| {
                let y = b.origin.y.0;
                if (offsets.get_untracked()[idx] - y).abs() > 1.0 {
                    offsets.update(|arr| arr[idx] = y);
                    if section == SectionId::Diagnostics {
                        maybe_activate_diagnostics(
                            scroll_y.get_untracked(),
                            viewport_h.get_untracked(),
                        );
                    }
                }
            })
            .child(body)
            .into_element()
    };

    let herdr_section = move || {
        dynamic(move || {
            let hid = ui.settings_host.get();
            let runtime = crate::config_for_host(&hid);
            settings_section(
                "Herdr",
                "Terminal runtime connection; configure it in agents.json on the selected Canban host.",
            )
            .trailing(compact_action(
                "Check connection",
                Variant::Secondary,
                move |_| dispatch(ui, "herdr.status", json!({})),
            ))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_6()
                    .child(view().flex_1().min_w(px(220.)).child(settings_field(
                        "Executable",
                        "Used to connect to the configured runtime.",
                        label(runtime.hc).font_family(MONO_FONT).text_size(px(12.)),
                    )))
                    .child(
                        view().flex_1().min_w(px(220.)).child(settings_field(
                            "Session",
                            "The configured Herdr runtime session.",
                            label(runtime.hc_session)
                                .font_family(MONO_FONT)
                                .text_size(px(12.)),
                        )),
                    ),
            )
        })
    };

    let storage_section = move || {
        view()
            .gap_5()
            .min_w(px(0.))
            .child(dynamic(move || {
                let home_str = active_host_home();
                let home_for_copy = home_str.clone();
                view()
                    .gap_4()
                    .min_w(px(0.))
                    .child(
                        settings_section(
                            "Storage",
                            "Canban keeps structured data, configuration and managed artifacts in its application home.",
                        )
                        .trailing(compact_action(
                            "Copy path",
                            Variant::Quiet,
                            move |_| copy_text(ui, home_for_copy.clone()),
                        ))
                        .child(settings_field(
                            "Application home",
                            "Configuration, documents and execution history are stored here.",
                            label(home_str).font_family(MONO_FONT).text_size(px(12.)),
                        )),
                    )
                    .child(muted(
                        "Closing this window keeps AppServer and scheduling running. Herdr owns the terminal programs.",
                    ))
            }))
            .child(archived_projects_section(ui))
    };

    let host_target_bar = move || {
        dynamic(move || {
            let hosts = crate::hosts::effective_hosts(ui);
            let cur_host = ui.settings_host.get();
            if hosts.len() <= 1 {
                return view()
                    .pt_2()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(muted(format!(
                        "HOST SETTINGS · {}",
                        crate::platform::LOCAL_HOST_NAME
                    )))
                    .child(compact_action(
                        "+ Connect Remote Machine…",
                        Variant::Quiet,
                        move |_| crate::hosts::open_connect_modal(ui),
                    ))
                    .into_element();
            }
            panel()
                .p_3()
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .gap_0p5()
                        .child(label("Target Canban Host").font_weight(FontWeight::SEMIBOLD))
                        .child(muted(
                            "Scheduling, Agents & Identities, Herdr, and Storage below configure this host.",
                        )),
                )
                .child(
                    segmented().children(hosts.into_iter().map(|h| {
                        let hid = h.id.clone();
                        let hid_sel = h.id.clone();
                        let cur_sel = cur_host.clone();
                        compact_segment(
                            h.name,
                            Some(if h.is_local {
                                Icon::Projects
                            } else {
                                Icon::Server
                            }),
                            move || cur_sel == hid_sel,
                            move |_| ui.settings_host.set(hid.clone()),
                        )
                        .into_element()
                    })),
                )
                .into_element()
        })
    };

    let middle_nav = move || {
        let mut col = view().p_2p5().gap_0p5().min_w(px(0.));
        for section in SectionId::ALL {
            if let Some(heading_text) = section.group_heading() {
                col = col.child(settings_nav_heading(heading_text));
                if section == SectionId::Scheduling {
                    col = col.child(dynamic(move || {
                        let hosts = crate::hosts::effective_hosts(ui);
                        if hosts.len() <= 1 {
                            return view().into_element();
                        }
                        let cur = ui.settings_host.get();
                        let cur_name = hosts
                            .iter()
                            .find(|h| h.id == cur)
                            .map(|h| h.name.clone())
                            .unwrap_or_else(|| crate::platform::LOCAL_HOST_NAME.into());
                        view()
                            .px_2()
                            .pb_1p5()
                            .child(
                                compact_action(
                                    format!("Target: {cur_name} ▾"),
                                    Variant::Secondary,
                                    move |_| {
                                        use nsh_core::element::desktop::{MenuItem, popup_menu};
                                        let list = hosts.clone();
                                        let active_h = cur.clone();
                                        ui.spawn(async move {
                                            let items: Vec<MenuItem> = list
                                                .into_iter()
                                                .map(|h| {
                                                    let hid = h.id.clone();
                                                    let prefix =
                                                        if h.id == active_h { "✓ " } else { "" };
                                                    MenuItem::new(format!("{prefix}{}", h.name))
                                                        .on_select(move || {
                                                            ui.settings_host.set(hid.clone());
                                                        })
                                                })
                                                .collect();
                                            popup_menu(items).await;
                                        });
                                    },
                                )
                                .w_full(),
                            )
                            .into_element()
                    }));
                }
            }
            if section == SectionId::Acknowledgements {
                col = col.child(view().my_2().mx_2().h(px(1.)).bg(BORDER));
            }
            col = col.child(settings_nav_item(
                section.nav_label(),
                move || active.get() == section,
                move |_| jump_to(section, true),
            ));
        }
        view()
            .w(px(208.))
            .h_full()
            .flex_shrink_0()
            .bg(SURFACE)
            .border_r_1()
            .border_color(BORDER)
            .child(scroll().flex_1().min_h(px(0.)).child(col))
    };

    let narrow_nav = move || {
        let mut strip = view().px_3().py_2().flex_row().items_center().gap_1();
        for section in SectionId::ALL {
            if section == SectionId::Acknowledgements {
                strip = strip.child(view().mx_1().w(px(1.)).h(px(16.)).bg(BORDER));
            }
            strip = strip.child(compact_segment(
                section.nav_label(),
                None,
                move || active.get() == section,
                move |_| jump_to(section, true),
            ));
        }
        view()
            .w_full()
            .flex_shrink_0()
            .bg(SURFACE)
            .border_b_1()
            .border_color(BORDER)
            .child(scroll().horizontal().scrollbar(false).child(strip))
    };

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex_row()
        .child(when(move || !ui.narrow.get(), middle_nav))
        .child(
            view()
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .child(when(move || ui.narrow.get(), narrow_nav))
                .child(
                    scroll()
                        .flex_1()
                        .min_h(px(0.))
                        .element_ref(&scroll_ref)
                        .on_layout(move |b: &Bounds<Points>| {
                            let h = b.size.height.0;
                            if (viewport_h.get_untracked() - h).abs() > 0.5 {
                                viewport_h.set(h);
                                maybe_activate_diagnostics(scroll_y.get_untracked(), h);
                            }
                        })
                        .on_scroll(move |e: &ScrollEvent| {
                            let y = e.offset.y.0;
                            let vh = e.size.height.0;
                            let ch = e.content_size.height.0;
                            if (viewport_h.get_untracked() - vh).abs() > 0.5 {
                                viewport_h.set(vh);
                            }
                            if (content_h.get_untracked() - ch).abs() > 0.5 {
                                content_h.set(ch);
                            }
                            scroll_y.set(y);
                            maybe_activate_diagnostics(y, vh);
                            if let Some(pin) = jump_pin.get_untracked() {
                                let offs = offsets.get_untracked();
                                jump_pin.set(advance_jump_pin(
                                    pin,
                                    y,
                                    canban_types::now(),
                                    &offs,
                                    ch,
                                    vh,
                                ));
                            }
                        })
                        .child(when(
                            move || !hydrated.get(),
                            move || {
                                view()
                                    .w_full()
                                    .p_5()
                                    .pb_10()
                                    .child(
                                        view()
                                            .w_full()
                                            .max_w(px(880.))
                                            .min_w(px(0.))
                                            .gap_4()
                                            .child(muted(
                                                "Application preferences and Agent configuration.",
                                            ))
                                            .child(loading_panel(
                                                "Loading Settings…",
                                                "Preparing preferences, connected hosts, and agent configuration.",
                                            )),
                                    )
                            },
                        )
                        .otherwise(move || {
                            let cfg_path = config_path.clone();
                            view()
                                .w_full()
                                .p_5()
                                .pb_10()
                                .on_layout(move |b: &Bounds<Points>| {
                                    let h = b.size.height.0;
                                    if (content_h.get_untracked() - h).abs() > 0.5 {
                                        content_h.set(h);
                                    }
                                })
                                .child(
                                    view()
                                        .w_full()
                                        .max_w(px(880.))
                                        .min_w(px(0.))
                                        .gap_7()
                                        .child(muted(
                                            "Application preferences and Agent configuration.",
                                        ))
                                        .child(wrap_section(
                                            SectionId::Theme,
                                            crate::utilities::settings(ui).into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Terminal,
                                            crate::utilities::terminal_settings(ui).into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Hosts,
                                            crate::hosts::settings_section_view(ui).into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Environment,
                                            global_env_section(ui).into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Diagnostics,
                                            crate::diagnostics::section_view_with_activation(
                                                ui,
                                                diagnostics_activated,
                                            )
                                            .into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Updates,
                                            crate::updates::settings(ui).into_element(),
                                        ))
                                        .child(host_target_bar())
                                        .child(wrap_section(
                                            SectionId::Scheduling,
                                            crate::components::scheduling::settings(ui)
                                                .into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::TerminalEnv,
                                            terminal_env_section(ui).into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Agents,
                                            crate::agents_setup::global_settings_section(
                                                ui,
                                                cfg_path,
                                            )
                                            .into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Herdr,
                                            herdr_section().into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Storage,
                                            storage_section().into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Acknowledgements,
                                            acknowledgements_section(ui).into_element(),
                                        )),
                                )
                        })),
                ),
        )
}

fn gallery_archived_projects(host_id: &str) -> Vec<crate::projects::ArchivedProjectSummary> {
    if host_id.is_empty() || host_id == "local" {
        vec![
            crate::projects::ArchivedProjectSummary {
                project_id: "arch-fiber".into(),
                name: "Fiber".into(),
                source_directory: "/Users/demo/Workspace/fiber".into(),
                archived_at: canban_types::now().saturating_sub(2 * 86_400_000),
                bytes: 428_032,
                archive_path: "/Users/demo/.canban/project-archives/arch-fiber.canban.tar.zst"
                    .into(),
                active_cards: 12,
                archived_cards: 120,
                total_cards: 132,
                files_count: 18,
                required_profiles: vec!["developer".into(), "reviewer".into()],
                secret_keys: vec!["GITHUB_PAT".into()],
            },
            crate::projects::ArchivedProjectSummary {
                project_id: "arch-docs".into(),
                name: "Docs Portal".into(),
                source_directory: "/Users/demo/Workspace/docs-portal".into(),
                archived_at: canban_types::now().saturating_sub(14 * 86_400_000),
                bytes: 64_512,
                archive_path: "/Users/demo/.canban/project-archives/arch-docs.canban.tar.zst"
                    .into(),
                active_cards: 3,
                archived_cards: 4,
                total_cards: 7,
                files_count: 2,
                required_profiles: Vec::new(),
                secret_keys: Vec::new(),
            },
        ]
    } else {
        vec![crate::projects::ArchivedProjectSummary {
            project_id: "arch-linux".into(),
            name: "Core Daemon".into(),
            source_directory: "/home/builder/workspaces/core-daemon".into(),
            archived_at: canban_types::now().saturating_sub(5 * 86_400_000),
            bytes: 192_000,
            archive_path: "/home/builder/.canban/project-archives/arch-linux.canban.tar.zst".into(),
            active_cards: 6,
            archived_cards: 9,
            total_cards: 15,
            files_count: 5,
            required_profiles: vec!["developer".into()],
            secret_keys: Vec::new(),
        }]
    }
}

pub fn archived_projects_section(ui: Ui) -> impl IntoElement {
    let items: Signal<Vec<crate::projects::ArchivedProjectSummary>> = signal(Vec::new());
    let loading = signal(false);
    let error = signal(String::new());
    let ticket = signal(0_u64);

    effect(move || {
        let hid = ui.settings_host.get();
        let _reload = ui.archived_projects_reload.get();
        if !crate::backend::is_connected() {
            let preview = ui
                .archived_projects_preview
                .get()
                .unwrap_or_else(|| gallery_archived_projects(&hid));
            items.set(preview);
            loading.set(false);
            error.set(String::new());
            return;
        }
        let next_ticket = ticket.get_untracked() + 1;
        ticket.set(next_ticket);
        loading.set(true);
        error.set(String::new());
        let req_host = hid.clone();
        ui.spawn(async move {
            let res = blocking(move || {
                crate::routed_call(
                    "project.archived.list",
                    json!({ "host_id": req_host }),
                    None,
                )
            })
            .await;
            if ticket.try_get() != Some(next_ticket) {
                return;
            }
            let _ = loading.try_set(false);
            match res {
                Ok(Ok(val)) => {
                    let parsed: Vec<crate::projects::ArchivedProjectSummary> = val["projects"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = items.try_set(parsed);
                    let _ = error.try_set(String::new());
                }
                Ok(Err(e)) => {
                    let _ = error.try_set(format!("{e:#}"));
                }
                Err(e) => {
                    let _ = error.try_set(e.to_string());
                }
            }
        });
    });

    settings_section(
        "Archived Projects",
        "Cold-archived Projects on this host. Archived Projects use zero background polling or scheduling and can be restored anytime.",
    )
    .trailing(
        compact_action(
            move || {
                if loading.get() {
                    "Refreshing…"
                } else {
                    "Refresh"
                }
            },
            Variant::Secondary,
            move |_| {
                ui.archived_projects_reload.update(|n| *n += 1);
            },
        )
        .disabled(move || loading.get()),
    )
    .child(dynamic(move || {
        let err = error.get();
        if !err.is_empty() {
            return notice(err, Tone::Danger).into_element();
        }
        let list = items.get();
        if loading.get() && list.is_empty() {
            return loading_panel("Loading archived Projects…", "Checking cold archive bundles on this host.")
                .into_element();
        }
        if list.is_empty() {
            return muted("No archived Projects on this host.").into_element();
        }
        let hid = ui.settings_host.get();
        view()
            .gap_2()
            .min_w(px(0.))
            .children(list.into_iter().map(|item| {
                let host_for_restore = hid.clone();
                let item_for_restore = item.clone();
                let path_for_copy = item.archive_path.clone();
                let cards_text = if item.archived_cards > 0 {
                    format!(
                        "{} active · {} archived Cards",
                        item.active_cards, item.archived_cards
                    )
                } else if item.total_cards == 1 {
                    "1 Card".to_owned()
                } else {
                    format!("{} Cards", item.total_cards)
                };
                let meta_text = format!(
                    "{} · {cards_text} · archived {}",
                    crate::diagnostics::format_bytes(item.bytes),
                    crate::components::activity::age(item.archived_at),
                );
                panel()
                    .p_3()
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .gap_2()
                    .min_w(px(0.))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(200.))
                                    .gap_0p5()
                                    .child(
                                        view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                label(item.name.clone())
                                                    .font_weight(FontWeight::SEMIBOLD),
                                            )
                                            .child(
                                                label(item.project_id.clone())
                                                    .font_family(MONO_FONT)
                                                    .text_xs()
                                                    .text_color(MUTED),
                                            ),
                                    )
                                    .child(label(meta_text).text_xs().text_color(MUTED))
                                    .child(
                                        label(item.source_directory.clone())
                                            .font_family(MONO_FONT)
                                            .text_xs()
                                            .text_color(MUTED)
                                            .truncate(),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .child(compact_action(
                                        "Copy archive path",
                                        Variant::Quiet,
                                        move |_| copy_text(ui, path_for_copy.clone()),
                                    ))
                                    .child(compact_action(
                                        "Restore…",
                                        Variant::Secondary,
                                        move |_| {
                                            ui.project_restore_modal.set(Some((
                                                host_for_restore.clone(),
                                                item_for_restore.clone(),
                                            )));
                                        },
                                    )),
                            ),
                    )
            }))
            .into_element()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OFFSETS: [f32; SECTION_COUNT] = [
        32.0, 220.0, 340.0, 410.0, 480.0, 600.0, 720.0, 1060.0, 1380.0, 1820.0, 2060.0, 2280.0,
    ];

    #[test]
    fn scroll_spy_defaults_to_first_section_before_layout_and_tracks_scroll() {
        assert_eq!(
            scroll_spy_section(0.0, &[0.0; SECTION_COUNT], 0.0, 0.0),
            SectionId::Theme
        );
        assert_eq!(
            scroll_spy_section(0.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Theme
        );
        assert_eq!(
            scroll_spy_section(180.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Terminal
        );
        assert_eq!(
            scroll_spy_section(350.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Environment
        );
        assert_eq!(
            SectionId::from_id("environment"),
            Some(SectionId::Environment)
        );
        assert_eq!(SectionId::Environment.nav_label(), "Environment & Secrets");
        assert_eq!(
            scroll_spy_section(680.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Scheduling
        );
        assert_eq!(
            scroll_spy_section(1020.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::TerminalEnv
        );
        assert_eq!(
            SectionId::from_id("terminal-env"),
            Some(SectionId::TerminalEnv)
        );
        assert_eq!(SectionId::TerminalEnv.nav_label(), "Terminal Environment");
        assert_eq!(
            scroll_spy_section(1760.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Herdr
        );
        // Reaching the bottom of the scroll viewport highlights Acknowledgements.
        assert_eq!(
            scroll_spy_section(1992.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Acknowledgements
        );
    }

    #[test]
    fn jump_pin_holds_clicked_section_during_animation_and_clamped_bottom_until_user_scrolls_away()
    {
        // Storage (2060) and Acknowledgements (2280) both clamp to max_scroll = 2000 when content=2800, viewport=800.
        assert_eq!(
            clamped_target_y(SectionId::Theme, &SAMPLE_OFFSETS, 2800.0, 800.0),
            0.0
        );
        assert_eq!(
            clamped_target_y(SectionId::Storage, &SAMPLE_OFFSETS, 2800.0, 800.0),
            2000.0
        );
        assert_eq!(
            clamped_target_y(SectionId::Acknowledgements, &SAMPLE_OFFSETS, 2800.0, 800.0),
            2000.0
        );

        let mut pin = Some(JumpPin {
            section: SectionId::Storage,
            started_at_ms: 1000,
            last_scroll_y: 0.0,
            reached: false,
        });

        // Mid-animation frames moving toward 2000 keep Storage highlighted rather than intermediate sections.
        for (t, y) in [(1050, 400.0), (1120, 1100.0), (1200, 1998.0)] {
            pin = advance_jump_pin(pin.unwrap(), y, t, &SAMPLE_OFFSETS, 2800.0, 800.0);
            assert!(pin.is_some());
            assert_eq!(
                resolve_active_section(pin, y, &SAMPLE_OFFSETS, 2800.0, 800.0),
                SectionId::Storage
            );
        }
        assert!(pin.unwrap().reached);

        // User scrolls upward away from 2000 -> pin releases and scroll-spy resumes.
        pin = advance_jump_pin(pin.unwrap(), 1960.0, 1500, &SAMPLE_OFFSETS, 2800.0, 800.0);
        assert!(pin.is_none());
        assert_eq!(
            resolve_active_section(pin, 1960.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Herdr
        );
    }

    #[test]
    fn global_env_sync_bar_renders_expected_copy_across_all_states() {
        use canban_types::{GlobalEnvSyncHostFailure, GlobalEnvSyncStatus};
        use nsh_core::{ApproxTextEngine, Core, MountBatch, MountItem, geometry::size};

        fn collect_texts(batch: MountBatch) -> Vec<String> {
            batch
                .items
                .into_iter()
                .filter_map(|item| match item {
                    MountItem::Text { text, .. } => Some(text.text.to_string()),
                    _ => None,
                })
                .collect()
        }

        #[component]
        fn ProbeSyncBars() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            view()
                .w(px(900.))
                .flex_col()
                .gap_3()
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus::default(),
                    true,
                    None,
                    || {},
                ))
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus::default(),
                    false,
                    None,
                    || {},
                ))
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus {
                        total_remote_hosts: 2,
                        synced_hosts: vec!["Studio Mac".into()],
                        unsynced_hosts: vec!["Home Linux".into()],
                        ..Default::default()
                    },
                    false,
                    None,
                    || {},
                ))
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus {
                        total_remote_hosts: 1,
                        syncing_hosts: vec!["Home Linux".into()],
                        ..Default::default()
                    },
                    false,
                    None,
                    || {},
                ))
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus {
                        total_remote_hosts: 1,
                        queued_hosts: vec!["Offline Box".into()],
                        ..Default::default()
                    },
                    false,
                    None,
                    || {},
                ))
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus {
                        total_remote_hosts: 1,
                        failed_hosts: vec![GlobalEnvSyncHostFailure {
                            host_id: "h1".into(),
                            host_name: "Home Linux".into(),
                            message: "unauthorized token".into(),
                        }],
                        ..Default::default()
                    },
                    false,
                    None,
                    || {},
                ))
                .child(global_env_sync_bar(
                    GlobalEnvSyncStatus {
                        total_remote_hosts: 2,
                        synced_hosts: vec!["Home Linux".into(), "Studio Mac".into()],
                        ..Default::default()
                    },
                    false,
                    None,
                    || {},
                ))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeSyncBars>());
        let texts = collect_texts(core.commit().expect("mount sync bars"));

        assert!(texts.iter().any(|t| t == "Save changes before syncing."));
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Local only — connect a remote host"))
        );
        assert!(
            texts
                .iter()
                .any(|t| t == "Not yet synced to Home Linux (1/2 synced).")
        );
        assert!(
            texts
                .iter()
                .any(|t| t == "Syncing to Home Linux in background…")
        );
        assert!(texts.iter().any(|t| t == "Syncing…"));
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Queued for offline: Offline Box"))
        );
        assert!(texts.iter().any(|t| t == "Home Linux: unauthorized token"));
        assert!(texts.iter().any(|t| t == "Retry sync"));
        assert!(
            texts
                .iter()
                .any(|t| t == "Synced to 2/2 hosts (Home Linux, Studio Mac).")
        );
    }
}
