//! Settings workspace: middle section-jump column, single outer scroll, and
//! Acknowledgements & Licenses section.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub const SECTION_COUNT: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionId {
    Theme,
    Terminal,
    Hosts,
    Diagnostics,
    Updates,
    Scheduling,
    Machines,
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
        Self::Diagnostics,
        Self::Updates,
        Self::Scheduling,
        Self::Machines,
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
            Self::Diagnostics => 3,
            Self::Updates => 4,
            Self::Scheduling => 5,
            Self::Machines => 6,
            Self::Agents => 7,
            Self::Herdr => 8,
            Self::Storage => 9,
            Self::Acknowledgements => 10,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Terminal => "terminal",
            Self::Hosts => "hosts",
            Self::Diagnostics => "diagnostics",
            Self::Updates => "updates",
            Self::Scheduling => "scheduling",
            Self::Machines => "machines",
            Self::Agents => "agents",
            Self::Herdr => "herdr",
            Self::Storage => "storage",
            Self::Acknowledgements => "acknowledgements",
        }
    }

    pub fn from_id(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.id().eq_ignore_ascii_case(raw.trim()))
    }

    pub const fn nav_label(self) -> &'static str {
        match self {
            Self::Theme => "Theme & support",
            Self::Terminal => "Terminal",
            Self::Hosts => "Hosts",
            Self::Diagnostics => "Diagnostics",
            Self::Updates => "Updates",
            Self::Scheduling => "Agent scheduling",
            Self::Machines => "Machines",
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
        let body = view()
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
            }));

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

pub fn screen(ui: Ui) -> impl IntoElement {
    let active_host_home = move || {
        let hid = ui.settings_host.get();
        if let Some(reg) = crate::HOSTS.get() {
            reg.home_for(&hid).to_string_lossy().into_owned()
        } else if let Some(app) = crate::APP.get() {
            app.home.to_string_lossy().into_owned()
        } else if hid == "local" || hid.is_empty() {
            "/preview/.canban".into()
        } else {
            "/home/builder/.canban".into()
        }
    };
    let config_path = format!("{}/config/agents.json", untracked(active_host_home));

    let scroll_ref = element_ref();
    let offsets = signal([0.0_f32; SECTION_COUNT]);
    let viewport_h = signal(0.0_f32);
    let content_h = signal(0.0_f32);
    let scroll_y = signal(0.0_f32);
    let initial_target = SectionId::from_id(&ui.settings_section.get_untracked());
    let jump_pin = signal(initial_target.map(|section| JumpPin {
        section,
        started_at_ms: canban_types::now(),
        last_scroll_y: 0.0,
        reached: false,
    }));

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
        ui.settings_section.set(String::new());
        jump_pin.set(Some(JumpPin {
            section,
            started_at_ms: canban_types::now(),
            last_scroll_y: scroll_y.get_untracked(),
            reached: false,
        }));
        ui.spawn(async move {
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
                if (offsets.get_untracked()[idx] - y).abs() > 0.5 {
                    offsets.update(|arr| arr[idx] = y);
                }
            })
            .child(body)
            .into_element()
    };

    let herdr_section = dynamic(move || {
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
                    label(runtime.hc).font_family("Menlo").text_size(px(12.)),
                )))
                .child(
                    view().flex_1().min_w(px(220.)).child(settings_field(
                        "Session",
                        "The configured Herdr runtime session.",
                        label(runtime.hc_session)
                            .font_family("Menlo")
                            .text_size(px(12.)),
                    )),
                ),
        )
    });

    let storage_section = dynamic(move || {
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
                    label(home_str).font_family("Menlo").text_size(px(12.)),
                )),
            )
            .child(muted(
                "Closing this window keeps AppServer and scheduling running. Herdr owns the terminal programs.",
            ))
    });

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
                    .child(muted("HOST SETTINGS · This Mac"))
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
                            .unwrap_or_else(|| "This Mac".into());
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
                            viewport_h.set(b.size.height.0);
                        })
                        .on_scroll(move |e: &ScrollEvent| {
                            let y = e.offset.y.0;
                            let vh = e.size.height.0;
                            let ch = e.content_size.height.0;
                            viewport_h.set(vh);
                            content_h.set(ch);
                            scroll_y.set(y);
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
                        .child(
                            view()
                                .w_full()
                                .p_5()
                                .pb_10()
                                .on_layout(move |b: &Bounds<Points>| {
                                    content_h.set(b.size.height.0);
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
                                            SectionId::Diagnostics,
                                            crate::diagnostics::section_view(ui).into_element(),
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
                                            SectionId::Machines,
                                            crate::machines::settings(ui).into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Agents,
                                            crate::agents_setup::global_settings_section(
                                                ui,
                                                config_path,
                                            )
                                            .into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Herdr,
                                            herdr_section.into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Storage,
                                            storage_section.into_element(),
                                        ))
                                        .child(wrap_section(
                                            SectionId::Acknowledgements,
                                            acknowledgements_section(ui).into_element(),
                                        )),
                                ),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OFFSETS: [f32; SECTION_COUNT] = [
        32.0, 220.0, 340.0, 480.0, 600.0, 720.0, 1050.0, 1380.0, 1820.0, 2060.0, 2280.0,
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
            scroll_spy_section(680.0, &SAMPLE_OFFSETS, 2800.0, 800.0),
            SectionId::Scheduling
        );
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
    fn credits_include_herdr_sheprd_and_nsh_without_shear_or_herdr_app() {
        let names: Vec<_> = RUNTIME_CREDITS
            .iter()
            .chain(ASSET_CREDITS.iter())
            .map(|c| c.name)
            .collect();
        assert!(names.contains(&"Herdr"));
        assert!(names.contains(&"sheprd"));
        assert!(names.contains(&"NSH"));
        assert!(names.contains(&"Ghostty"));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("shear")));

        let sheprd = RUNTIME_CREDITS
            .iter()
            .find(|c| c.name == "sheprd")
            .expect("sheprd credit");
        assert_eq!(sheprd.subtitle, "formerly herdr-gpui");
        assert_eq!(sheprd.license, "Apache-2.0");
        assert_eq!(
            sheprd.links,
            &[("GitHub", "https://github.com/sheprd-labs/sheprd")]
        );

        for credit in RUNTIME_CREDITS.iter().chain(ASSET_CREDITS.iter()) {
            assert!(!credit.subtitle.contains("herdr-app"));
            assert!(!credit.description.contains("herdr-app"));
        }
    }
}
