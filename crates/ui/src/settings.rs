//! Settings workspace: 1-of-1 section switching, unified host scope, and single-boundary groups.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub const SECTION_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionId {
    Appearance,
    Terminal,
    Environment,
    Hosts,
    Updates,
    Scheduling,
    Agents,
    Diagnostics,
    Storage,
    Acknowledgements,
}

impl SectionId {
    pub const ALL: [Self; SECTION_COUNT] = [
        Self::Appearance,
        Self::Terminal,
        Self::Environment,
        Self::Hosts,
        Self::Updates,
        Self::Scheduling,
        Self::Agents,
        Self::Diagnostics,
        Self::Storage,
        Self::Acknowledgements,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Terminal => "terminal",
            Self::Environment => "environment",
            Self::Hosts => "hosts",
            Self::Updates => "updates",
            Self::Scheduling => "scheduling",
            Self::Agents => "agents",
            Self::Diagnostics => "diagnostics",
            Self::Storage => "storage",
            Self::Acknowledgements => "acknowledgements",
        }
    }

    pub fn from_id(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "appearance" | "theme" => Some(Self::Appearance),
            "terminal" | "terminal-env" => Some(Self::Terminal),
            "environment" => Some(Self::Environment),
            "hosts" => Some(Self::Hosts),
            "updates" => Some(Self::Updates),
            "scheduling" => Some(Self::Scheduling),
            "agents" => Some(Self::Agents),
            "diagnostics" | "herdr" => Some(Self::Diagnostics),
            "storage" => Some(Self::Storage),
            "acknowledgements" | "about" | "licenses" => Some(Self::Acknowledgements),
            _ => None,
        }
    }

    pub fn resolve(raw: &str) -> Self {
        Self::from_id(raw).unwrap_or(Self::Appearance)
    }

    pub const fn nav_label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Terminal => "Terminal",
            Self::Environment => "Environment & Secrets",
            Self::Hosts => "Hosts & Pairing",
            Self::Updates => "Updates",
            Self::Scheduling => "Scheduling",
            Self::Agents => "Agents & Identities",
            Self::Diagnostics => "Diagnostics & Runtime",
            Self::Storage => "Storage & Archives",
            Self::Acknowledgements => "About & Licenses",
        }
    }

    pub const fn group_heading(self) -> Option<&'static str> {
        match self {
            Self::Appearance => Some("GENERAL"),
            Self::Scheduling => Some("HOST"),
            _ => None,
        }
    }

    pub const fn is_host_scoped(self) -> bool {
        matches!(
            self,
            Self::Scheduling | Self::Agents | Self::Diagnostics | Self::Storage
        )
    }
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

fn credit_card(
    ui: Ui,
    group_title: &'static str,
    entries: &'static [CreditEntry],
) -> impl IntoElement {
    settings_group()
        .title(group_title)
        .rows(entries.iter().map(|entry| {
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

            view()
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
                .child(muted(entry.description))
        }))
}

pub fn acknowledgements_section(ui: Ui) -> impl IntoElement {
    let notices_open = signal(false);
    let selected_doc = signal(0_usize);

    settings_section(
        "About & Licenses",
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
                    view()
                        .rounded(px(RADIUS))
                        .bg(SURFACE)
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
                "Local only — connect a remote host in Hosts & Pairing to replicate global env & secrets."
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
        .gap_1p5()
        .p_3()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(SURFACE)
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
        box_view = box_view.child(
            text(format!("{}: {}", failure.host_name, failure.message))
                .text_size(px(12.))
                .text_color(DANGER),
        );
    }
    if let Some(err) = trigger_error.filter(|s| !s.trim().is_empty()) {
        box_view = box_view.child(text(err.to_owned()).text_size(px(12.)).text_color(DANGER));
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

fn open_host_menu(ui: Ui, hosts: Vec<canban_types::HostSummary>, cur: String) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    ui.spawn(async move {
        let items: Vec<MenuItem> = hosts
            .into_iter()
            .map(|h| {
                let hid = h.id.clone();
                let prefix = if h.id == cur { "✓ " } else { "" };
                MenuItem::new(format!("{prefix}{}", h.name)).on_select(move || {
                    ui.settings_host.set(hid.clone());
                })
            })
            .collect();
        if !items.is_empty() {
            popup_menu(items).await;
        }
    });
}

fn terminal_section(ui: Ui) -> impl IntoElement {
    use canban_types::HostTerminalEnvMode;

    let save_error = signal(String::new());
    let refresh_note = signal(Option::<(String, Tone)>::None);
    let tracked_host = stored(ui.settings_host.get_untracked());
    effect(move || {
        let cur_host = ui.settings_host.get();
        if tracked_host.get() != cur_host {
            tracked_host.replace(cur_host);
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

    let hosts_memo = computed(move || crate::hosts::effective_hosts(ui));
    let env_group = settings_group()
        .title("Automation Environment")
        .subtitle("Host login-shell environment inherited by automated Agent and Script workflow steps.")
        .trailing(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .child(dynamic(move || {
                    let hosts = hosts_memo.get();
                    if hosts.len() <= 1 {
                        return view().into_element();
                    }
                    let cur = ui.settings_host.get();
                    let cur_name = hosts
                        .iter()
                        .find(|h| h.id == cur)
                        .map(|h| h.name.clone())
                        .unwrap_or_else(|| crate::platform::LOCAL_HOST_NAME.into());
                    compact_action(format!("{cur_name} ▾"), Variant::Quiet, move |_| {
                        open_host_menu(ui, hosts.clone(), cur.clone());
                    })
                    .into_element()
                }))
                .child(
                    compact_action_with_icon(
                        Icon::Refresh,
                        move || {
                            if ui.is_method_pending("host.env.refresh") {
                                "Refreshing…"
                            } else {
                                "Refresh env"
                            }
                        },
                        Variant::Secondary,
                        move |_| {
                            if ui.is_method_pending("host.env.refresh") {
                                return;
                            }
                            dispatch_complete(ui, "host.env.refresh", json!({}), move |res| {
                                match res {
                                    Ok(v) => {
                                        let shell = v["shell"].as_str().unwrap_or("shell");
                                        let var_count = v["var_count"].as_u64().unwrap_or(0);
                                        let full =
                                            v["full_env_captured"].as_bool().unwrap_or(false);
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
                                }
                            });
                        },
                    )
                    .disabled(move || ui.is_method_pending("host.env.refresh")),
                ),
        )
        .row(settings_row(
            "Environment mode",
            "Trusted automated runs inherit your login-shell environment by default, or restrict to minimal PATH-only env.",
            segmented()
                .child(
                    compact_segment(
                        "Inherit Shell Env",
                        None,
                        move || ui.terminal_env_mode.get() == HostTerminalEnvMode::Inherit,
                        move |_| save_mode(HostTerminalEnvMode::Inherit),
                    )
                    .disabled(move || ui.is_method_pending("config.agents.save")),
                )
                .child(
                    compact_segment(
                        "Minimal (PATH only)",
                        None,
                        move || ui.terminal_env_mode.get() == HostTerminalEnvMode::Minimal,
                        move |_| save_mode(HostTerminalEnvMode::Minimal),
                    )
                    .disabled(move || ui.is_method_pending("config.agents.save")),
                ),
        ));

    settings_section(
        "Terminal",
        "Font appearance for open Terminals and login-shell environment for automated workflow steps.",
    )
    .child(crate::utilities::terminal_appearance_group(ui))
    .child(env_group)
    .child(
        muted(
            "External fork PRs (external_pr) and untrusted external cardless runs default to minimal env unless overridden in workflow/step env:. Interactive Card Terminals remain unrestricted.",
        )
        .text_size(px(12.)),
    )
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

fn storage_section(ui: Ui) -> impl IntoElement {
    let home_str = move || crate::backend::home_for(&ui.settings_host.get());

    settings_section(
        "Storage & Archives",
        "Application home directory, structured persistence, and cold-archived Projects on this host.",
    )
    .child(
        settings_group()
            .title("Application Home")
            .row(
                view()
                    .px_3p5()
                    .py_2p5()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .min_w(px(0.))
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(200.))
                            .gap_0p5()
                            .child(label("Application home").font_weight(FontWeight::MEDIUM))
                            .child(
                                label(home_str)
                                    .font_family(MONO_FONT)
                                    .text_size(px(12.))
                                    .text_color(MUTED)
                                    .truncate(),
                            ),
                    )
                    .child(compact_action("Copy path", Variant::Quiet, move |_| {
                        copy_text(ui, crate::backend::home_for(&ui.settings_host.get_untracked()));
                    })),
            )
            .row(
                view().px_3p5().py_2p5().child(
                    muted("Closing this window keeps AppServer and scheduling running. Herdr owns the terminal programs.")
                        .text_size(px(12.)),
                ),
            ),
    )
    .child(archived_projects_section(ui))
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let scroll_ref = element_ref();
    let active = computed(move || SectionId::resolve(&ui.settings_section.get()));
    let hosts_memo = computed(move || crate::hosts::effective_hosts(ui));

    let last_section = stored(active.get_untracked());
    effect(move || {
        let cur = active.get();
        if last_section.get() != cur {
            last_section.replace(cur);
            scroll_ref.scroll_to(Point::new(px(0.), px(0.)), false);
        }
    });

    let last_host = stored(ui.settings_host.get_untracked());
    effect(move || {
        let cur = ui.settings_host.get();
        if last_host.get() != cur {
            last_host.replace(cur);
            ui.diagnostics.set(serde_json::Value::Null);
            ui.diagnostics_logs.set(serde_json::Value::Null);
            ui.agent_catalog.set(serde_json::Value::Null);
        }
    });

    let select_section = move |section: SectionId| {
        ui.settings_section.set(section.id().into());
        scroll_ref.scroll_to(Point::new(px(0.), px(0.)), false);
    };

    let middle_nav = move || {
        let mut col = view().p_2p5().gap_0p5().min_w(px(0.));
        for section in SectionId::ALL {
            if let Some(heading_text) = section.group_heading() {
                col = col.child(settings_nav_heading(heading_text));
                if section == SectionId::Scheduling {
                    col = col.child(dynamic(move || {
                        let hosts = hosts_memo.get();
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
                                    format!("{cur_name} ▾"),
                                    Variant::Secondary,
                                    move |_| {
                                        open_host_menu(ui, hosts.clone(), cur.clone());
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
                move |_| select_section(section),
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
                move |_| select_section(section),
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

    let host_scope_bar = move || {
        dynamic(move || {
            if !active.get().is_host_scoped() {
                return view().into_element();
            }
            let hosts = hosts_memo.get();
            if hosts.len() <= 1 {
                return view().into_element();
            }
            let cur = ui.settings_host.get();
            let cur_host = hosts.iter().find(|h| h.id == cur);
            let cur_name = cur_host
                .map(|h| h.name.clone())
                .unwrap_or_else(|| crate::platform::LOCAL_HOST_NAME.into());
            let is_local = cur_host.is_none_or(|h| h.is_local);

            view()
                .pb_2p5()
                .border_b_1()
                .border_color(BORDER)
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(muted("Host").text_size(px(12.)))
                        .child(
                            icon_tinted(
                                if is_local {
                                    Icon::Projects
                                } else {
                                    Icon::Server
                                },
                                TEXT,
                            )
                            .w(px(13.))
                            .h(px(13.)),
                        )
                        .child(label(cur_name).font_weight(FontWeight::MEDIUM)),
                )
                .child(compact_action(
                    "Switch host ▾",
                    Variant::Quiet,
                    move |_| {
                        open_host_menu(ui, hosts.clone(), cur.clone());
                    },
                ))
                .into_element()
        })
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
                        .child(
                            view().w_full().p_5().pb_10().child(
                                view()
                                    .w_full()
                                    .max_w(px(880.))
                                    .min_w(px(0.))
                                    .gap_5()
                                    .child(host_scope_bar())
                                    .child(dynamic(move || match active.get() {
                                        SectionId::Appearance => {
                                            crate::utilities::settings(ui).into_element()
                                        }
                                        SectionId::Terminal => terminal_section(ui).into_element(),
                                        SectionId::Environment => {
                                            global_env_section(ui).into_element()
                                        }
                                        SectionId::Hosts => {
                                            crate::hosts::settings_section_view(ui).into_element()
                                        }
                                        SectionId::Updates => {
                                            crate::updates::settings(ui).into_element()
                                        }
                                        SectionId::Scheduling => {
                                            crate::components::scheduling::settings(ui)
                                                .into_element()
                                        }
                                        SectionId::Agents => {
                                            crate::agents_setup::global_settings_section(ui)
                                                .into_element()
                                        }
                                        SectionId::Diagnostics => {
                                            crate::diagnostics::section_view(ui).into_element()
                                        }
                                        SectionId::Storage => storage_section(ui).into_element(),
                                        SectionId::Acknowledgements => {
                                            acknowledgements_section(ui).into_element()
                                        }
                                    })),
                            ),
                        ),
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

    view()
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
                        .child(label("Archived Projects").font_weight(FontWeight::SEMIBOLD))
                        .child(
                            muted("Cold-archived Projects use zero background polling or scheduling and can be restored anytime.")
                                .text_size(px(CAPTION_SIZE)),
                        ),
                )
                .child(
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
                ),
        )
        .child(dynamic(move || {
            let err = error.get();
            if !err.is_empty() {
                return notice(err, Tone::Danger).into_element();
            }
            let list = items.get();
            if loading.get() && list.is_empty() {
                return loading_panel(
                    "Loading archived Projects…",
                    "Checking cold archive bundles on this host.",
                )
                .into_element();
            }
            if list.is_empty() {
                return view()
                    .rounded(px(RADIUS))
                    .bg(SURFACE)
                    .border_1()
                    .border_color(BORDER)
                    .px_3p5()
                    .py_3()
                    .child(muted("No archived Projects on this host."))
                    .into_element();
            }
            let hid = ui.settings_host.get();
            settings_group()
                .rows(list.into_iter().map(|item| {
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
                    view()
                        .px_3p5()
                        .py_3()
                        .min_w(px(0.))
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
                        )
                }))
                .into_element()
        }))
}
