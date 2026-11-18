//! Standalone native component gallery. Never opens a Canban home or runtime.
mod fixtures;
use canban_ui::{presenter::Ui, profiles, review};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

fn gallery_herdr_setup_fixture(ui: Ui, mode: &str) -> impl IntoElement {
    let detected_platform = format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH);
    #[cfg(target_os = "macos")]
    let missing_hint = "Herdr Core (`hc`) was not found in ~/.local/bin, /opt/homebrew/bin, /usr/local/bin, or PATH.";
    #[cfg(not(target_os = "macos"))]
    let missing_hint =
        "Herdr Core (`hc`) was not found in ~/.local/bin, /usr/local/bin, /usr/bin, or PATH.";
    let initial = match mode {
        "herdr-installing" => canban_ui::startup::HerdrSetupView {
            home: "/preview/.canban".into(),
            installing: true,
            logs: vec![
                "Downloading installer from https://releases.herdr.dev/core/install.sh".into(),
                format!("detected {detected_platform}"),
                "downloading herdr-core 0.1.18…".into(),
            ],
            ..Default::default()
        },
        "herdr-error" => canban_ui::startup::HerdrSetupView {
            home: "/preview/.canban".into(),
            error: Some(
                "Herdr Core installer failed (exit code 22): curl: (6) Could not resolve host: releases.herdr.dev".into(),
            ),
            logs: vec![
                "Downloading installer from https://releases.herdr.dev/core/install.sh".into(),
                "curl: (6) Could not resolve host: releases.herdr.dev".into(),
            ],
            ..Default::default()
        },
        _ => canban_ui::startup::HerdrSetupView {
            home: "/preview/.canban".into(),
            error: Some(missing_hint.into()),
            ..Default::default()
        },
    };
    let fixture_owner = Owner::current();
    let state = signal(initial);
    let installed_in_fixture = signal(false);
    let entered_workspace = signal(false);
    let copy_status = signal(String::new());
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(dynamic(move || {
            if entered_workspace.get() {
                return canban_ui::frame::screen(ui).into_element();
            }
            let current = state.get();
            let detected_platform = detected_platform.clone();
            canban_ui::startup::herdr_setup_screen(
                current,
                copy_status,
                move || {
                    copy_status.set(String::new());
                    state.update(|s| {
                        s.installing = true;
                        s.error = None;
                        s.logs = vec![
                            "Starting Herdr Core installer…".into(),
                            format!("detected {detected_platform}"),
                            "installed ~/.local/bin/hc (0.1.18)".into(),
                        ];
                    });
                    installed_in_fixture.set(true);
                    fixture_owner.run(move || {
                        set_timeout(std::time::Duration::from_millis(350), move || {
                            entered_workspace.set(true);
                        });
                    });
                },
                move || {
                    copy_status.set(String::new());
                    if installed_in_fixture.get_untracked() {
                        entered_workspace.set(true);
                    } else {
                        state.update(|s| {
                            s.error = Some(
                                "Herdr Core (`hc`) is still missing in this fixture. Click Install Herdr Core to simulate installation and enter the workspace.".into(),
                            );
                        });
                    }
                },
                move || {
                    copy_status.set("Copied".into());
                },
            )
            .into_element()
        }))
}

fn controls() -> impl IntoElement {
    let ui = Ui::new();
    ui.apply_config(&fixtures::agent_config());
    let input = signal("Card 现在编辑用中文时 mixed English @developer 与 `code` 对齐测试".into());
    let profile_fixture = signal(canban_types::Config::default().profiles);
    let count = signal(0_u32);
    let directory = signal("~/Workspace/".to_owned());
    let directory_listing = signal(PathListing {
        base_dir: "/Users/demo/Workspace".into(),
        home: "/Users/demo".into(),
        exists: true,
        entries: vec![
            PathEntry::new("Machines", "/Users/demo/Workspace/Machines"),
            PathEntry::new("canban", "/Users/demo/Workspace/canban"),
            PathEntry::new("canban-site", "/Users/demo/Workspace/canban-site"),
            PathEntry::new("herdr", "/Users/demo/Workspace/herdr"),
            PathEntry::new("sheprd", "/Users/demo/Workspace/sheprd"),
        ],
        loading: false,
        error: String::new(),
    });
    let markdown = canban_ui_kit::markdown::MarkdownInput::new();
    let title_line = canban_ui_kit::markdown::MarkdownInput::new();
    let title_text = signal(String::from("@developer Fix #ui-kit title completion"));
    let group_open = signal(true);
    let nav_selected = signal(0_usize);
    let gutter_scroll = element_ref();
    let split_width = signal(240.0_f32);
    let chosen_ref = signal("feat/git-client".to_string());
    view().flex_1().min_h(px(0.0)).p_5().gap_4().bg(BACKGROUND)
    .child(heading("Component Gallery"))
    .child(muted("Production components · local fixtures · no AppServer or Agent required"))
    .child(
        scroll().flex_1().child(
            view()
                .gap_4()
                .child(
                    panel()
                        .child(label("Selectable Markdown · Card #109"))
                        .child(muted("Drag within a paragraph, list item or code block and copy with Command+C. Text stays read-only."))
                        .child(canban_ui_kit::markdown::reader(|| {
                            "Selection fixture: English and 中文 text.\n\nSecond paragraph with **bold** and `inline code`.\n\n- List item can be copied.\n\n```text\ncopy this code\n```".into()
                        })),
                )
                .child(
                    section_group("Borderless Section Group")
                        .description("Reusable header + 1px bottom divider without an enclosing card border, shared across Overview, GitHub, and Settings.")
                        .count(2)
                        .tone(Tone::Info)
                        .child(entry_card(Icon::Plus, "Create a Project", "One accessible clickable surface.", move |_| {
                            count.set(count.get_untracked() + 1);
                        }))
                        .child(path_combobox(
                            directory,
                            move || directory_listing.get(),
                            move |base_dir| {
                                let home = "/Users/demo".to_owned();
                                let resolved = if base_dir.is_empty() {
                                    home.clone()
                                } else {
                                    expand_tilde(&base_dir, &home)
                                };
                                let clean = if resolved.len() > 1 {
                                    resolved.trim_end_matches('/').to_owned()
                                } else {
                                    resolved
                                };
                                let names: &[&str] = match clean.as_str() {
                                    "/" => &["Applications", "Users", "opt", "tmp"],
                                    "/Users" => &["Shared", "demo"],
                                    "/Users/demo" => &[".canban", "Desktop", "Documents", "Workspace"],
                                    "/Users/demo/Workspace" => &[
                                        ".git",
                                        "Machines",
                                        "canban",
                                        "canban-site",
                                        "herdr",
                                        "sheprd",
                                    ],
                                    _ => &[],
                                };
                                let exists = !names.is_empty()
                                    || clean.starts_with("/Users/demo/Workspace/canban");
                                directory_listing.set(PathListing {
                                    base_dir: clean.clone(),
                                    home,
                                    exists,
                                    entries: names
                                        .iter()
                                        .map(|n| {
                                            let p = if clean == "/" {
                                                format!("/{n}")
                                            } else {
                                                format!("{clean}/{n}")
                                            };
                                            PathEntry::new(*n, p)
                                        })
                                        .collect(),
                                    loading: false,
                                    error: String::new(),
                                });
                            },
                            |_| {},
                        )),
                )
                .child(dynamic(move || {
                    let is_open = group_open.get();
                    collapsible_section_group(
                        "Collapsible Section Group",
                        Some(1),
                        is_open,
                        move |_| group_open.update(|v| *v = !*v),
                        muted("Collapsed/expanded disclosure shares the same borderless divider header."),
                    )
                    .into_element()
                }))
                .child(
                    panel()
                        .child(label("Shared Lucide icons · muted / selected / attention"))
                        .child(view().flex_row().flex_wrap().gap_2().children(
                            [Icon::Search, Icon::Terminal, Icon::Compare, Icon::Board, Icon::Insights, Icon::Overview,
                             Icon::Archive, Icon::Settings, Icon::Projects, Icon::Agent, Icon::GitHub,
                             Icon::ChevronDown, Icon::Plus, Icon::Check]
                            .into_iter().enumerate().map(|(i, kind)|
                                icon_action(kind, format!("{kind:?}"), move || i == 1,
                                    move || if i == 2 { 3 } else { 0 }, |_| {}).into_element()
                            ).collect::<Vec<_>>())),
                )
                .child(
                    panel()
                        .child(label("Colored Agent comment avatars · brand & multi-color provider marks"))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_2()
                                .children(
                                    [
                                        ("@codex", Icon::Codex),
                                        ("@claude", Icon::Claude),
                                        ("@agy", Icon::Agy),
                                        ("@gemini", Icon::Gemini),
                                        ("@copilot", Icon::Copilot),
                                        ("@devin", Icon::Devin),
                                        ("@cursor", Icon::Cursor),
                                        ("@opencode", Icon::OpenCode),
                                        ("@amp", Icon::Amp),
                                        ("@kimi", Icon::Kimi),
                                        ("@kiro", Icon::Kiro),
                                        ("@qwen", Icon::Qwen),
                                        ("@qoder", Icon::Qoder),
                                        ("@muse", Icon::Muse),
                                        ("@cline", Icon::Cline),
                                        ("@grok", Icon::Grok),
                                        ("@hermes", Icon::Hermes),
                                        ("@kilo", Icon::Kilo),
                                        ("@pi", Icon::Pi),
                                        ("@omp", Icon::Omp),
                                        ("@droid", Icon::Droid),
                                        ("@letta", Icon::Letta),
                                        ("@mastra", Icon::Mastra),
                                        ("@maki", Icon::Maki),
                                        ("@agent", Icon::Agent),
                                    ]
                                    .into_iter()
                                    .map(|(handle, mark)| {
                                        view()
                                            .w(px(28.))
                                            .h(px(28.))
                                            .rounded(px(14.))
                                            .bg(INSET)
                                            .border_1()
                                            .border_color(BORDER)
                                            .items_center()
                                            .justify_center()
                                            .tooltip(hint_tooltip(handle))
                                            .child(icon_colored(mark, 16.))
                                            .into_element()
                                    })
                                    .collect::<Vec<_>>(),
                                ),
                        ),
                )
                .child(
                    panel()
                        .child(heading("Typography and surfaces"))
                        .child(label("Body · a reusable semantic text style"))
                        .child(muted(
                            "Secondary text · long text and narrow windows should remain readable",
                        ))
                        .child(badge("Needs attention"))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_2()
                                .child(live_status("Merging", Tone::Info))
                                .child(status("Merged", Tone::Success))
                                .child(status("Needs review", Tone::Attention))
                                .child(status("Merge held", Tone::Neutral)),
                        )
                        .child(
                            action("Disabled action", Variant::Secondary, |_| {}).disabled(true),
                        ),
                )
                .child(
                    panel()
                        .child(label("Buttons & shared tooltips · hover / press / keyboard focus"))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_3()
                                .child(action("Primary", Variant::Primary, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(action("Secondary", Variant::Secondary, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(action("Danger", Variant::Danger, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(
                                    view()
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(5.))
                                        .bg(INSET)
                                        .hover(|s| s.bg(CARD_HOVER))
                                        .tooltip(hint_tooltip(
                                            "Styled tooltip surface · readable over dense Card text",
                                        ))
                                        .child(muted("Hover for shared tooltip")),
                                ),
                        )
                        .child(muted(move || format!("Press count: {}", count.get()))),
                )
                .child(
                    panel()
                        .child(label("Custom scrollbars · built-in overlay & outer-gutter rail"))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_5()
                                .child(
                                    view()
                                        .w(px(260.))
                                        .gap_1p5()
                                        .child(muted("Built-in scroll() indicator"))
                                        .child(
                                            scroll()
                                                .h(px(136.))
                                                .rounded(px(RADIUS))
                                                .border_1()
                                                .border_color(BORDER)
                                                .bg(INSET)
                                                .p_2p5()
                                                .child(view().gap_1p5().children((1..=10).map(
                                                    |i| {
                                                        muted(format!(
                                                            "Row #{i} · theme-reactive subtle pill"
                                                        ))
                                                    },
                                                ))),
                                        ),
                                )
                                .child(
                                    view()
                                        .w(px(260.))
                                        .gap_1p5()
                                        .child(muted("Board outer-gutter vertical_scrollbar"))
                                        .child(
                                            view()
                                                .relative()
                                                .h(px(136.))
                                                .child(
                                                    scroll()
                                                        .element_ref(&gutter_scroll)
                                                        .scrollbar(false)
                                                        .flex_1()
                                                        .child(view().gap_2().children(
                                                            (1..=8).map(|i| {
                                                                panel()
                                                                    .py_2()
                                                                    .px_3()
                                                                    .border_1()
                                                                    .border_color(BORDER)
                                                                    .child(label(format!(
                                                                        "Card #{i} · crisp border"
                                                                    )))
                                                            }),
                                                        )),
                                                )
                                                .child(
                                                    vertical_scrollbar(gutter_scroll)
                                                        .absolute()
                                                        .top_0()
                                                        .bottom_0()
                                                        .right(px(-8.)),
                                                ),
                                        ),
                                ),
                        ),
                )
                .child(
                    panel()
                        .child(label("Markdown editor / shared reading styles"))
                        .child(title_line.render_line(
                            title_line.line_source(
                                title_text,
                                "Gallery single-line title",
                                "Card title · @Agent, #Card or #tag",
                                || false,
                            ),
                            || false,
                        ))
                        .child(markdown.render(input, markdown.source(input, "Gallery Markdown", "Write Markdown…"), false, signal(220.)))
                        .child(canban_ui_kit::markdown::reader(move || input.get())),
                )
                .child({
                    let tl_expanded = signal(false);
                    panel()
                        .child(label("Connected Card Timeline · vertical spine & milestone folding"))
                        .child(dynamic(move || {
                            use canban_ui::components::timeline_item::{
                                MilestoneFold, Spine, render_connected,
                            };
                            let [e1, e2, e3, e4, e5] = fixtures::timeline_connected();
                            let open = tl_expanded.get();
                            let fold = MilestoneFold {
                                hidden_count: 1,
                                expanded: open,
                                hidden_alert: false,
                                toggle: std::rc::Rc::new(move || tl_expanded.update(|v| *v = !*v)),
                            };
                            let mut stack = view().gap_0().child(render_connected(
                                ui,
                                e1,
                                true,
                                Spine {
                                    first: true,
                                    last: false,
                                },
                                None,
                                None,
                            ));
                            if open {
                                stack = stack.child(render_connected(
                                    ui,
                                    e2,
                                    false,
                                    Spine {
                                        first: false,
                                        last: false,
                                    },
                                    None,
                                    None,
                                ));
                            }
                            stack
                                .child(render_connected(
                                    ui,
                                    e3,
                                    false,
                                    Spine {
                                        first: false,
                                        last: false,
                                    },
                                    Some(fold),
                                    None,
                                ))
                                .child(render_connected(
                                    ui,
                                    e4,
                                    true,
                                    Spine {
                                        first: false,
                                        last: false,
                                    },
                                    None,
                                    None,
                                ))
                                .child(render_connected(
                                    ui,
                                    e5,
                                    true,
                                    Spine {
                                        first: false,
                                        last: true,
                                    },
                                    None,
                                    None,
                                ))
                                .into_element()
                        }))
                })
                .child(canban_ui::components::scheduling::settings(ui))
                .child(canban_ui::agents_setup::canban_skill_onboarding_section(ui))
                .child(profiles::registry(profile_fixture))
                .child(canban_ui::components::workflow::run_view(fixtures::workflow()))
                .child(review::diff_panel(fixtures::review()))
                .child(canban_ui::utilities::terminal_settings(ui))
                .child(
                    panel()
                        .child(label("Searchable + typeable popover picker (canban-ui-kit)"))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(
                                    search_popover(
                                        press_target()
                                            .h(px(28.))
                                            .px_2p5()
                                            .rounded(px(RADIUS))
                                            .border_1()
                                            .border_color(BORDER)
                                            .bg(INSET)
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .child(icon(Icon::Branch).w(px(12.)).h(px(12.)))
                                            .child(
                                                label(move || chosen_ref.get())
                                                    .font_family(MONO_FONT)
                                                    .text_size(px(11.5)),
                                            )
                                            .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.))),
                                        move || {
                                            let cur = chosen_ref.get();
                                            vec![
                                                PickerOption::new("main", "main")
                                                    .group("Branches")
                                                    .detail("default")
                                                    .icon(Icon::Branch)
                                                    .mono(true)
                                                    .selected(cur == "main"),
                                                PickerOption::new("feat/git-client", "feat/git-client")
                                                    .group("Branches")
                                                    .detail("↑2")
                                                    .icon(Icon::Branch)
                                                    .mono(true)
                                                    .selected(cur == "feat/git-client"),
                                                PickerOption::new("origin/release", "origin/release")
                                                    .group("Remote Branches")
                                                    .icon(Icon::Branch)
                                                    .mono(true)
                                                    .selected(cur == "origin/release"),
                                            ]
                                        },
                                        move |val| chosen_ref.set(val),
                                    )
                                    .placeholder("Search or type custom ref…")
                                    .custom_option(|q| {
                                        Some(
                                            PickerOption::new(q.to_owned(), format!("Use ref \"{q}\""))
                                                .detail("custom")
                                                .icon(Icon::Branch)
                                                .mono(true),
                                        )
                                    }),
                                )
                                .child(muted(move || format!("Selected: {}", chosen_ref.get()))),
                        ),
                )
                .child(
                    panel()
                        .child(label("Settings jump column controls"))
                        .child(
                            view()
                                .w(px(208.))
                                .p_2()
                                .rounded(px(RADIUS))
                                .border_1()
                                .border_color(BORDER)
                                .bg(SURFACE)
                                .gap_0p5()
                                .child(settings_nav_heading("INTERFACE"))
                                .child(settings_nav_item(
                                    "Theme & support",
                                    move || nav_selected.get() == 0,
                                    move |_| nav_selected.set(0),
                                ))
                                .child(settings_nav_item(
                                    "Terminal",
                                    move || nav_selected.get() == 1,
                                    move |_| nav_selected.set(1),
                                ))
                                .child(view().my_1p5().mx_2().h(px(1.)).bg(BORDER))
                                .child(settings_nav_item(
                                    "Acknowledgements",
                                    move || nav_selected.get() == 2,
                                    move |_| nav_selected.set(2),
                                )),
                        ),
                )
                .child(
                    panel()
                        .child(label("Resizable horizontal split · 1pt BORDER hairline + 3pt ACCENT hover/drag"))
                        .child(
                            view()
                                .h(px(120.))
                                .rounded(px(RADIUS))
                                .border_1()
                                .border_color(BORDER)
                                .overflow_hidden()
                                .child(split(
                                    view()
                                        .flex_1()
                                        .p_3()
                                        .bg(SURFACE)
                                        .child(label("Left pane"))
                                        .child(muted("Drag the boundary to resize"))
                                        .into_element(),
                                    view()
                                        .flex_1()
                                        .p_3()
                                        .bg(BACKGROUND)
                                        .child(label("Right pane"))
                                        .child(muted("Full-height 1pt BORDER divider"))
                                        .into_element(),
                                    split_width,
                                    160.0,
                                    160.0,
                                )),
                        ),
                )
                .child(canban_ui::settings::acknowledgements_section(ui))
                .child(
                    panel().child(label("Native libghostty terminal")).child({
                        #[cfg(target_os = "macos")]
                        {
                            native("canban/terminal")
                                .prop("theme", canban_ui::utilities::terminal_theme)
                                .prop("font", move || canban_ui::utilities::terminal_font(ui))
                                .prop("content", json!(fixtures::TERMINAL))
                                .prop("read_only", json!(true))
                                .w_full()
                                .h(px(320.0))
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            canban_terminal::linux::preview_terminal(
                                fixtures::TERMINAL,
                                canban_ui::utilities::terminal_theme,
                                move || canban_ui::utilities::terminal_font(ui),
                                canban_terminal::linux::FocusRequest::default,
                            )
                            .w_full()
                            .h(px(320.0))
                        }
                    }),
                ),
        ),
    )
}

fn board_transitions_bar(
    ui: Ui,
    reduced_motion: Signal<bool>,
    reject_drop: Signal<bool>,
) -> impl IntoElement {
    let next_id = signal(100_u64);
    let stages = ["backlog", "develop", "review"];
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(SURFACE)
        .border_b_1()
        .border_color(BORDER)
        .child(label("Board Transitions"))
        .child(action("Move next stage", Variant::Primary, move |_| {
            ui.snapshot.update(|s| {
                if let Some(c) = s.cards.iter_mut().find(|c| c.active()) {
                    let pos = stages.iter().position(|st| *st == c.stage).unwrap_or(0);
                    c.stage = stages[(pos + 1) % stages.len()].into();
                    c.revision += 1;
                }
            });
        }))
        .child(action("Batch move (2)", Variant::Secondary, move |_| {
            ui.snapshot.update(|s| {
                for c in s.cards.iter_mut().filter(|c| c.active()).take(2) {
                    let pos = stages.iter().position(|st| *st == c.stage).unwrap_or(0);
                    c.stage = stages[(pos + 1) % stages.len()].into();
                    c.revision += 1;
                }
            });
        }))
        .child(action("Rapid redirect", Variant::Secondary, move |_| {
            let target_id = ui
                .snapshot
                .with_untracked(|s| s.cards.iter().find(|c| c.active()).map(|c| c.id.clone()));
            let Some(cid) = target_id else {
                return;
            };
            let first = cid.clone();
            ui.snapshot.update(|s| {
                if let Some(c) = s.cards.iter_mut().find(|c| c.id == first) {
                    c.stage = "develop".into();
                    c.revision += 1;
                }
            });
            set_timeout(std::time::Duration::from_millis(80), move || {
                ui.snapshot.update(|s| {
                    if let Some(c) = s.cards.iter_mut().find(|c| c.id == cid) {
                        c.stage = "review".into();
                        c.revision += 1;
                    }
                });
            });
        }))
        .child(action("New card (duang)", Variant::Secondary, move |_| {
            let n = next_id.get_untracked();
            next_id.set(n + 1);
            ui.snapshot.update(|s| {
                let mut template = fixtures::workspace().cards.remove(2);
                template.id = format!("gallery-new-{n}");
                template.number = n;
                template.title = format!("Freshly popped Card #{n}");
                template.stage = "backlog".into();
                template.board_order = Some(-(n as i64) * 1024);
                template.archived_at = None;
                template.removed_at = None;
                template.revision = 1;
                template.created_at = canban_types::now();
                s.cards.push(template);
            });
        }))
        .child(action("Archive card (fade)", Variant::Quiet, move |_| {
            ui.snapshot.update(|s| {
                if let Some(c) = s.cards.iter_mut().find(|c| c.active()) {
                    c.archived_at = Some(canban_types::now());
                    c.revision += 1;
                }
            });
        }))
        .child(action(
            move || {
                if reject_drop.get() {
                    SharedString::from("Drop rollback: On")
                } else {
                    SharedString::from("Drop rollback: Off")
                }
            },
            Variant::Quiet,
            move |_| reject_drop.set(!reject_drop.get_untracked()),
        ))
        .child(action("Reset", Variant::Quiet, move |_| {
            ui.snapshot.set(fixtures::board_cards());
            ui.card.set(String::new());
            ui.page.set("board".into());
        }))
        .child(action(
            move || {
                if reduced_motion.get() {
                    SharedString::from("Reduced motion: On")
                } else {
                    SharedString::from("Reduced motion: Off")
                }
            },
            Variant::Quiet,
            move |_| reduced_motion.set(!reduced_motion.get_untracked()),
        ))
}

#[component]
pub fn Gallery() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    let scene = signal("workspace".to_string());
    let constrained = signal(false);
    let ui = Ui::new();
    canban_ui::links::install(ui);
    let reduced_motion = signal(false);
    let reject_drop = signal(false);
    provide_context(canban_ui_kit::motion::ReducedMotion(reduced_motion));
    ui.board_move.replace(Some(std::rc::Rc::new(move |drop| {
        let previous = ui.snapshot.get_untracked();
        ui.snapshot.update(|s| {
            let Some(index) = s.cards.iter().position(|c| c.id == drop.card_id) else {
                return;
            };
            let mut peers = s
                .cards
                .iter()
                .filter(|c| {
                    c.project_id == s.cards[index].project_id
                        && c.stage == drop.stage
                        && c.id != drop.card_id
                        && c.active()
                })
                .cloned()
                .collect::<Vec<_>>();
            peers.sort_by(canban_types::Card::board_cmp);
            let position = drop
                .before_card_id
                .as_ref()
                .and_then(|id| peers.iter().position(|c| &c.id == id))
                .unwrap_or(peers.len());
            let mut ids = peers.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
            ids.insert(position, drop.card_id.clone());
            s.cards[index].stage = drop.stage;
            s.cards[index].revision += 1;
            for (i, id) in ids.iter().enumerate() {
                if let Some(c) = s.cards.iter_mut().find(|c| &c.id == id) {
                    c.board_order = Some(i as i64 * 1024);
                }
            }
        });
        if reject_drop.get_untracked() {
            set_timeout(std::time::Duration::from_millis(240), move || {
                ui.operation_error
                    .set("Simulated RPC rejection: rolling Card back to previous slot".into());
                ui.snapshot.set(previous);
            });
        }
    })));
    canban_ui::commands::bindings(ui);
    canban_ui::utilities::init(ui);
    canban_ui::terminals::bindings(ui);
    set_interval(std::time::Duration::from_secs(5), move || {
        ui.runtime_clock.set(canban_types::now());
        ui.snapshot.update(|s| {
            for a in s.terminals.iter_mut().filter_map(|t| t.agent.as_mut()) {
                if a.connection_state == "connected" && !a.finished() {
                    a.lifecycle.observed_at = Some(canban_types::now());
                }
            }
        });
    });
    ui.snapshot.set(fixtures::workspace());
    ui.project.set("gallery-project".into());
    ui.card.set("card-1".into());
    ui.page.set("card".into());
    ui.apply_config(&fixtures::agent_config());
    ui.agent_catalog.set(fixtures::agent_catalog());
    ui.review_state.refs_data.set(fixtures::git_refs());
    ui.review_state.compare_data.set(fixtures::git_compare());
    ui.review_state.base_ref.set("main".into());
    ui.review_state
        .compare_ref
        .set("feat/diff-workbench".into());
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("board-drafts" | "board-drafts-empty")
    ) {
        ui.card.set(String::new());
        ui.page.set("board".into());
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-drafts") {
            let mut snapshot = fixtures::workspace();
            let mut project = snapshot.projects[0].clone();
            project.id = "draft-other-project".into();
            project.name = "Other Project".into();
            snapshot.projects.push(project);
            for number in 4..=30 {
                let mut card = snapshot.cards[0].clone();
                card.id = format!("draft-navigation-card-{number}");
                card.number = number;
                card.title = format!("Draft navigation scroll fixture {number}");
                card.stage = "backlog".into();
                snapshot.cards.push(card);
            }
            ui.snapshot.set(snapshot);
            for (key, project, title) in [
                (
                    "new-card:gallery-draft",
                    "gallery-project",
                    "Unfinished local Card",
                ),
                (
                    "new-card:other-draft",
                    "draft-other-project",
                    "Cross-Project draft",
                ),
            ] {
                canban_ui::drafts::put(
                    ui,
                    key.into(),
                    canban_ui::drafts::Draft {
                        project_id: project.into(),
                        title: title.into(),
                        body: "Retain content when navigating between Board, Drafts and Settings."
                            .into(),
                        ..Default::default()
                    },
                );
            }
            ui.update.set(json!({"state":{"phase":"available","available":{"manifest":{"version":"20261111-99"}}}}));
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("startup-error") {
        canban_ui::frame::configure_window_chrome();
        return canban_ui::startup::error_screen("/preview/development-home".into(), "AppServer API 4 is incompatible with this UI (API 8). Start the matching instance or update the service on its owning host.".into()).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("backend-older") {
        canban_ui::frame::configure_window_chrome();
        return canban_ui::startup::backend_update_screen(
            "/preview/.canban".into(),
            "The backend is older than this App (backend API 4, App API 8).".into(),
            || async { anyhow::bail!("Fixture: the real action uses the installed backend") },
        )
        .into_element();
    }
    if let Ok(scene @ ("herdr-missing" | "herdr-installing" | "herdr-error")) =
        std::env::var("CANBAN_GALLERY_SCENE").as_deref()
    {
        canban_ui::frame::configure_window_chrome();
        return gallery_herdr_setup_fixture(ui, scene).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("backend-newer") {
        ui.backend_notice
            .set("The backend is newer than this App. Please use a newer Canban App.".into());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("update-recovery") {
        return canban_ui::updates::recovery_screen(
            "/preview/.canban".into(),
            "Down migration 10 → 9 failed. Current schema remains 10.".into(),
        )
        .into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("updates" | "updates-disabled")
    ) {
        ui.card.set(String::new());
        ui.page.set("board".into());
        ui.update.set(json!({"build":canban_updater::Build::current(),"disabled":std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("updates-disabled"),"state":{"phase":"available","available":{"manifest":{"version":"20261110-3"}}}}));
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("updates-disabled") {
            ui.page.set("settings".into());
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("agent-setup" | "settings-agents" | "project-agents")
    ) {
        ui.card.set(String::new());
        match std::env::var("CANBAN_GALLERY_SCENE").as_deref() {
            Ok("agent-setup") => ui.page.set("agent-setup".into()),
            Ok("project-agents") => ui.page.set("project-agents".into()),
            _ => {
                let mut cfg = fixtures::agent_config();
                let custom_argv = vec![
                    "/bin/agy-modified".into(),
                    "--add-dir".into(),
                    "{home}".into(),
                    "--add-dir".into(),
                    "{cwd}".into(),
                    "--add-dir".into(),
                    "{project_dir}".into(),
                    "--settings".into(),
                    "{agy_trusted_settings}".into(),
                    "-i".into(),
                    "{prompt}".into(),
                ];
                if let Some(agy) = cfg.profiles.iter_mut().find(|p| p.id == "agy") {
                    agy.argv = Some(custom_argv.clone());
                }
                ui.apply_config(&cfg);
                ui.agent_catalog.update(|cat| {
                    if let Some(arr) = cat["raw_profiles"].as_array_mut()
                        && let Some(item) = arr.iter_mut().find(|v| v["id"].as_str() == Some("agy"))
                    {
                        item["argv"] = json!(custom_argv);
                    }
                });
                ui.open_settings_section("agents");
            }
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("card-local" | "card-local-delete-confirm" | "card-issue" | "card-pr")
    ) {
        let mut snapshot = fixtures::workspace();
        let scene = std::env::var("CANBAN_GALLERY_SCENE").unwrap();
        snapshot.cards[0].body = "## Conversation layout\n\nKeep the task and discussion readable. Reference [Issue #42](https://github.com/acme/app/issues/42) or paste https://github.com/acme/app/pull/57.\n\n- [ ] Verify narrow layout\n- [ ] Keep draft on blur".into();
        snapshot.cards[0].title = "Improve Card discussion and content references".into();
        if !matches!(scene.as_str(), "card-local" | "card-local-delete-confirm") {
            let pr = scene == "card-pr";
            let source = canban_types::GitHubIssueRef {
                repository: "acme/app".into(),
                repository_id: 1,
                issue_id: if pr { 57 } else { 42 },
                number: if pr { 57 } else { 42 },
                url: format!(
                    "https://github.com/acme/app/{}/{}",
                    if pr { "pull" } else { "issues" },
                    if pr { 57 } else { 42 }
                ),
            };
            snapshot.cards[0].content = if pr {
                canban_types::CardContent::GitHubPr {
                    source: source.clone(),
                }
            } else {
                canban_types::CardContent::GitHubIssue {
                    source: source.clone(),
                }
            };
            snapshot.github_issues.push(canban_types::GitHubIssueSnapshot { id:"source".into(),project_id:snapshot.cards[0].project_id.clone(),source,title:snapshot.cards[0].title.clone(),body:"## GitHub description\n\nThis is the remote source. Local context remains separate.".into(),state:if pr {"draft".into()} else {"open".into()},author_id:1,author_login:"octocat".into(),labels:vec![],assignees:vec![],updated_at:String::new(),synced_at:canban_types::now() });
        }
        let prof = canban_types::Config::default().profiles[0].clone();
        let fixture_tid = canban_types::Conversation::key("card-1", &prof.id);
        let mut attempt = canban_scheduler::agent_run(
            &snapshot.cards[0].project_id,
            "card-1",
            1,
            "develop",
            "stage_entry",
            prof,
            "Card conversation fixture".into(),
            None,
        );
        attempt.state = "completed".into();
        snapshot.terminals.push(canban_types::TerminalEntry {
            id: fixture_tid.clone(),
            conversation_id: Some(fixture_tid.clone()),
            number: 1,
            name: None,
            origin_project_id: None,
            origin_card_id: Some("card-1".into()),
            agent_engine: None,
            runtime: None,
            agent: Some(attempt),
            created_at: 1,
        });
        snapshot.events.retain(|e| e.card_id != "card-1");
        for (index, kind) in [
            "comment.created",
            "execution.queued",
            "execution.started",
            "comment.replied",
            "execution.failed",
            // Bookkeeping that Card Activity hides (Card #16 regression).
            "card.updated",
        ]
        .iter()
        .enumerate()
        {
            let mut event = fixtures::timeline();
            event.id = format!("card-conversation-{index}");
            event.card_id = "card-1".into();
            event.sequence = index as u64 + 1;
            event.kind = (*kind).into();
            event.actor = if index == 0 || index == 5 {
                "user".into()
            } else {
                "agent:developer".into()
            };
            if index != 0 && index != 5 {
                event.data = json!({"terminal_id": fixture_tid});
            }
            event.message = match index { 0 => "Please preserve the review decision and local discussion scope.".into(), 3 => "## Implementation report\n\n".to_owned() + &"A readable paragraph with **Markdown** and a stable reference [Issue #42](https://github.com/acme/app/issues/42).\n\n".repeat(18), 4 => "Execution failed: check the terminal before continuing.".into(), _ => kind.replace('.', " ") };
            snapshot.events.push(event);
        }
        ui.snapshot.set(snapshot);
        ui.card.set("card-1".into());
        ui.page.set("card".into());
        ui.card_standalone.set(true);
        if scene == "card-local-delete-confirm" {
            ui.deleting_comment.set(Some("card-conversation-0".into()));
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("references") {
        let mut snapshot = fixtures::workspace();
        for number in 4..=65 {
            let mut card = snapshot.cards[2].clone();
            card.id = format!("reference-card-{number}");
            card.number = number;
            card.title = if number == 65 {
                "Beyond fifty unique needle".into()
            } else {
                format!("Completion fixture {number}")
            };
            card.stage = "backlog".into();
            snapshot.cards.push(card);
        }
        snapshot.cards[0].body = "## Body Markdown\n\n**Bold body** and *italic body*.\n\nLocal: [Related Card](card:card-2) · Tag: #search\n\nProfile: @developer\n\n[Example website](https://example.com/) and [GitHub Issue](https://github.com/jjyr/canban/issues/1)\n\nLiteral: `@developer #2 #search`".into();
        let mut comment = fixtures::timeline();
        comment.id = "reference-comment".into();
        comment.card_id = "card-1".into();
        comment.kind = "comment.created".into();
        comment.actor = "user".into();
        comment.message = "### Comment Markdown\n\n**Bold comment** and *italic comment*.\n\n[Comment local](card:card-2) · @developer · #ux · [Comment website](https://example.com/)".into();
        comment.data = json!({"body":comment.message});
        snapshot.events.retain(|e| e.card_id != "card-1");
        snapshot.events.push(comment);
        ui.snapshot.set(snapshot);
        ui.card.set("card-1".into());
        ui.page.set("card".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("editor") {
        let mut snapshot = fixtures::workspace();
        snapshot.projects[0].github = Some(canban_types::GitHubSource {
            pr_intake: Default::default(),
            authors: vec![],
            labels: vec![],
            include_drafts: false,
            paused: false,
            repository: "jjyr/canban".into(),
            repository_id: 1,
            generation: "gallery".into(),
            intake: canban_types::GitHubIntake::All,
            synced_at: None,
            error: None,
        });
        ui.snapshot.set(snapshot);
        ui.new_card();
        let key = ui.new_draft.get_untracked();
        canban_ui::drafts::put(ui,key,canban_ui::drafts::Draft {
            project_id:"gallery-project".into(), title:"Markdown editor regression".into(),
            body:"## Editing\n\n- [ ] Keep selection and undo while expanding\n- [ ] Enter and Shift+Enter insert newlines\n\n```rust\nlet value = 42;\n```\n\nType @ or # for references.\n".into(), ..Default::default()
        });
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("overview") {
        ui.snapshot.set(fixtures::overview());
        ui.page.set("overview".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("archive") {
        ui.snapshot.set(fixtures::archive());
        ui.project.set("gallery-project".into());
        ui.card.set(String::new());
        ui.page.set("archive".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("insights"
            | "insights-weekly"
            | "insights-empty"
            | "insights-efficiency"
            | "insights-short-history")
    ) {
        canban_ui::frame::configure_window_chrome();
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("insights-empty") {
            let mut empty = canban_types::Snapshot::default();
            empty
                .projects
                .push(fixtures::workspace().projects.remove(0));
            ui.snapshot.set(empty);
        } else {
            ui.snapshot.set(fixtures::insights());
        }
        ui.insights.anchor_ms.set(Some(fixtures::INSIGHTS_NOW));
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("insights-weekly") {
            ui.insights
                .cadence
                .set(canban_ui::insights::model::TrendCadence::Weekly);
        }
        ui.insights
            .efficiency_open
            .set(std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("insights-efficiency"));
        ui.all_profiles.set(vec![
            canban_types::Profile {
                id: "developer".into(),
                preset: Some("codex".into()),
                ..Default::default()
            },
            canban_types::Profile {
                id: "reviewer".into(),
                preset: Some("claude".into()),
                ..Default::default()
            },
            canban_types::Profile {
                id: "planner".into(),
                preset: Some("codex".into()),
                ..Default::default()
            },
        ]);
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("insights-short-history") {
            let cutoff = fixtures::INSIGHTS_NOW - 3_600_000;
            ui.snapshot.update(|s| {
                s.cards.retain(|c| c.created_at >= cutoff);
                s.events.retain(|e| e.created_at >= cutoff);
                for p in &mut s.projects {
                    p.created_at = cutoff;
                }
            });
        }
        ui.project.set("gallery-project".into());
        ui.card.set(String::new());
        ui.page.set("insights".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("reminders") {
        let window = nsh_core::element::desktop::current_window();
        let mut snapshot = fixtures::workspace();
        snapshot.cards[2].body = (0..30)
            .map(|i| {
                format!(
                    "### Context {}\n\nLong context keeps the unread reply below the fold.\n\n",
                    i + 1
                )
            })
            .collect();
        let mut project = snapshot.projects[0].clone();
        project.id = "gallery-other".into();
        project.name = "Another Project".into();
        snapshot.projects.push(project);
        let mut card = snapshot.cards[2].clone();
        card.id = "card-other".into();
        card.project_id = "gallery-other".into();
        card.body = "Another Project reply".into();
        snapshot.cards.push(card);
        for (id, project) in [
            ("card-3", "gallery-project"),
            ("card-other", "gallery-other"),
        ] {
            let mut event = fixtures::timeline();
            event.id = format!("unread-{id}");
            event.kind = "comment.replied".into();
            event.actor = "agent:developer".into();
            event.card_id = id.into();
            event.project_id = project.into();
            event.sequence = 2;
            event.message = "**Agent reply:** ready for you to read.".into();
            snapshot.events.push(event);
        }
        ui.snapshot.set(snapshot);
        ui.page.set("board".into());
        ui.card.set(String::new());
        canban_ui::reminders::init(ui);
        let append = move |card: &str, count: usize| {
            ui.snapshot.update(|s| {
                for _ in 0..count {
                    let seq = s
                        .events
                        .iter()
                        .filter(|e| e.card_id == card)
                        .map(|e| e.sequence)
                        .max()
                        .unwrap_or(0)
                        + 1;
                    let mut e = fixtures::timeline();
                    e.id = format!("reply-{card}-{seq}");
                    e.card_id = card.into();
                    e.sequence = seq;
                    e.actor = "agent:developer".into();
                    e.kind = "comment.replied".into();
                    e.message = format!("Agent reply **{seq}**: notification acceptance.");
                    s.events.push(e);
                }
            });
        };
        return view()
            .flex_1()
            .min_h(px(0.))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .p_2()
                    .gap_2()
                    .child(action("Reply to #1", Variant::Quiet, move |_| {
                        append("card-1", 1)
                    }))
                    .child(action("Burst replies to #3", Variant::Quiet, move |_| {
                        append("card-3", 2)
                    }))
                    .child(action("Reply in 3s", Variant::Quiet, move |_| {
                        set_timeout(std::time::Duration::from_secs(3), move || {
                            append("card-1", 1)
                        });
                    }))
                    .child(action("Silent stage done #1", Variant::Quiet, move |_| {
                        ui.snapshot.update(|s| {
                            let seq = s
                                .events
                                .iter()
                                .filter(|e| e.card_id == "card-1")
                                .map(|e| e.sequence)
                                .max()
                                .unwrap_or(0)
                                + 1;
                            let mut e = fixtures::timeline();
                            e.id = format!("auto-completed-{seq}");
                            e.card_id = "card-1".into();
                            e.sequence = seq;
                            e.actor = "agent:developer".into();
                            e.kind = "execution.reported".into();
                            e.message = "completed: Stage completed automatically".into();
                            e.data = json!({"report_id": format!("auto-{seq}"), "state": "completed"});
                            s.events.push(e);
                        });
                    }))
                    .child(action("Silent idle #1", Variant::Quiet, move |_| {
                        ui.snapshot.update(|s| {
                            let prof = canban_types::Config::default().profiles[0].clone();
                            let idle_tid = canban_types::Conversation::key("card-1", &prof.id);
                            let turn = s
                                .terminals
                                .iter()
                                .find(|t| t.id == idle_tid)
                                .and_then(|t| t.agent.as_ref())
                                .and_then(|a| a.observed.as_ref())
                                .and_then(|v| v["turn"].as_u64())
                                .unwrap_or(0)
                                + 1;
                            s.terminals.retain(|t| t.id != idle_tid);
                            let attempt: canban_types::AgentRun = serde_json::from_value(json!({
                                "project_id": "gallery-project",
                                "card_id": "card-1",
                                "number": 1,
                                "stage": "develop",
                                "trigger": "interactive",
                                "profile": prof,
                                "instruction": "Interactive check",
                                "state": "running",
                                "terminal": "pty-idle-1",
                                "cwd": "/tmp",
                                "input_path": "in",
                                "connection_state": "connected",
                                "observed": {"state": "live", "turn": turn, "status": {"state": "idle"}},
                                "lifecycle": {"observed_at": canban_types::now(), "idle_since": canban_types::now()},
                                "updated_at": 1
                            }))
                            .unwrap();
                            let num = s.terminals.iter().map(|t| t.number).max().unwrap_or(0) + 1;
                            s.terminals.push(canban_types::TerminalEntry {
                                id: idle_tid.clone(),
                                conversation_id: Some(idle_tid),
                                number: num,
                                name: None,
                                origin_project_id: None,
                                origin_card_id: Some("card-1".into()),
                                agent_engine: None,
                                runtime: None,
                                agent: Some(attempt),
                                created_at: 1,
                            });
                        });
                    }))
                    .child(action("Return #1 to Backlog", Variant::Quiet, move |_| {
                        ui.snapshot.update(|s| {
                            let seq = s
                                .events
                                .iter()
                                .filter(|e| e.card_id == "card-1")
                                .map(|e| e.sequence)
                                .max()
                                .unwrap_or(0)
                                + 1;
                            let mut e = fixtures::timeline();
                            e.id = format!("backlog-{seq}");
                            e.card_id = "card-1".into();
                            e.sequence = seq;
                            e.actor = "agent:developer".into();
                            e.kind = "execution.reported".into();
                            e.message = "Returned to Backlog: Already implemented".into();
                            e.data = json!({"report_id": format!("backlog-{seq}"), "state": "backlog"});
                            s.events.push(e);
                        });
                    }))
                    .child(action("Auto-merge #1", Variant::Quiet, move |_| {
                        ui.snapshot.update(|s| {
                            let seq = s
                                .events
                                .iter()
                                .filter(|e| e.card_id == "card-1")
                                .map(|e| e.sequence)
                                .max()
                                .unwrap_or(0)
                                + 1;
                            let mut e = fixtures::timeline();
                            e.id = format!("merged-{seq}");
                            e.card_id = "card-1".into();
                            e.sequence = seq;
                            e.actor = "system".into();
                            e.kind = "review.merged".into();
                            e.message = "Auto-merged into main".into();
                            e.data = json!({"target_branch": "main", "request_key": format!("auto-merge-{seq}")});
                            s.events.push(e);
                        });
                    }))
                    .child(action("Merge failed #1", Variant::Quiet, move |_| {
                        ui.snapshot.update(|s| {
                            let seq = s
                                .events
                                .iter()
                                .filter(|e| e.card_id == "card-1")
                                .map(|e| e.sequence)
                                .max()
                                .unwrap_or(0)
                                + 1;
                            let mut e = fixtures::timeline();
                            e.id = format!("merge-failed-{seq}");
                            e.card_id = "card-1".into();
                            e.sequence = seq;
                            e.actor = "system".into();
                            e.kind = "review.merge_failed".into();
                            e.message = "Auto-merge failed: dirty target branch".into();
                            e.data = json!({"target_branch": "main"});
                            s.events.push(e);
                        });
                    }))
                    .child(action("Board", Variant::Quiet, move |_| {
                        ui.navigate("board")
                    }))
                    .child(action("Overview", Variant::Quiet, move |_| {
                        ui.navigate("overview")
                    }))
                    .child(action("Focus window", Variant::Quiet, move |_| {
                        window.focus()
                    })),
            )
            .child(canban_ui::frame::screen(ui))
            .into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("column-automations") {
        ui.snapshot.set(fixtures::automation_board());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("workflows") | Ok("workflows-yaml") | Ok("workflow-agent") | Ok("workflows-conflict")
    ) {
        let default_raw = canban_workflow_engine::source::encode(
            &canban_workflow_engine::source::default_document(
                "delivery:gallery-project",
                canban_types::WorkspaceSettings::default(),
            ),
        )
        .unwrap();
        let plan_raw = include_str!("../workflows/plan-and-develop.yaml").to_string();
        let mut draft = canban_ui::workflows::Draft {
            base: json!({"revision":"gallery","sources":[
                {"filename":"development.yaml","source":default_raw,"active":{"revision":1,"workflow_id":"delivery:gallery-project"}},
                {"filename":"issue-planning.yaml","source":plan_raw,"active":{"revision":1,"workflow_id":"gallery-workflow"}}
            ]}),
            ..Default::default()
        };
        draft.sources.insert("development.yaml".into(), default_raw);
        draft.sources.insert("issue-planning.yaml".into(), plan_raw);
        draft.selected =
            if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("workflows-conflict") {
                "issue-planning.yaml".into()
            } else {
                "development.yaml".into()
            };
        ui.workflows.drafts.update(|d| {
            d.insert(ui.project.get_untracked(), draft);
        });
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("workflows-yaml") {
            ui.workflows.mode.set("yaml".into());
        }
        ui.workflows.prompt.set("You are the Workflow Configuration Assistant for the current Canban Project.\nYour scope covers all Workflows in this Project, not just a single file.\n\nFirst, perform a read-only analysis of all Workflow YAML sources, present an ASCII overview of triggers, Agents, approvals, and Board columns, and wait for the user's specific modification instructions before validating and publishing.\n".repeat(4));
        ui.workflows.profile.set(ui.default_agent.get_untracked());
        ui.page.set("workflows".into());
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("workflows-conflict") {
            ui.workflows.drafts.update(|drafts| {
                let draft = drafts.get_mut(&ui.project.get_untracked()).unwrap();
                draft.sources.insert(
                    "issue-planning.yaml".into(),
                    "# Keep this unsaved editor draft\n".to_owned()
                        + &draft.sources["issue-planning.yaml"],
                );
                draft.sources.insert(
                    "a-workflow-source-with-a-long-descriptive-filename.yaml".into(),
                    "schema: 1\nname: Long source fixture\n".into(),
                );
            });
            ui.workflows.latest.set(json!({"revision":"newer-gallery","sources":[{"filename":"issue-planning.yaml","source":"# Saved elsewhere; local draft must survive.\nschema: 1\nname: Latest saved source\n"}]}));
        }
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("workflow-agent") {
            ui.workflows.agent.set(true);
            return canban_ui::workflows::agent_panel(ui).into_element();
        }
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("workflows-conflict") {
            return view()
                .w(px(800.))
                .flex_1()
                .child(canban_ui::frame::screen(ui))
                .into_element();
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("project-activity") {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::project_activity());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-large") {
        let s = fixtures::large_board();
        let pid = s.projects[0].id.clone();
        ui.snapshot.set(s);
        ui.project.set(pid);
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-bottom-drag") {
        ui.snapshot.set(fixtures::bottom_card_drag());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if let Ok(
        scene @ ("board-filters"
        | "board-filters-mine"
        | "board-filters-empty"
        | "board-filters-local"),
    ) = std::env::var("CANBAN_GALLERY_SCENE").as_deref()
    {
        let mut s = fixtures::board_filters();
        if scene == "board-filters-local" {
            s.cards.retain(|c| c.github_issue().is_none());
        }
        ui.snapshot.set(s);
        ui.project.set("gallery-project".into());
        ui.card.set(String::new());
        ui.page.set("board".into());
        if scene == "board-filters-mine" {
            canban_ui::github::filters::mine(ui);
        }
        if scene == "board-filters-empty" {
            canban_ui::github::filters::edit(ui, "gallery-project", |f| {
                f.set_author("mine", "");
                f.tags = vec!["missing-tag".into()];
            });
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    // Card tile states (running Agent pulse, gates, branch); `-open` keeps a
    // Card pane open so a narrow Board switches to one stage with stage tabs.
    if let Ok(
        scene @ ("board-cards"
        | "board-cards-open"
        | "board-cards-conflict"
        | "board-cards-merger-failed"),
    ) = std::env::var("CANBAN_GALLERY_SCENE").as_deref()
    {
        let mut snap = fixtures::board_cards();
        if scene == "board-cards-merger-failed" {
            if let Some(run) = snap
                .workflow_runs
                .iter_mut()
                .find(|r| r.id == "gallery-conflict-run")
            {
                run.state = "failed".into();
                run.error = Some("Execution failed: inspect or explicitly retry".into());
                if let Some(obj) = run.inputs.as_object_mut() {
                    obj.remove("queued_at");
                }
            }
            for term in &mut snap.terminals {
                if let Some(a) = term.agent.as_mut()
                    && a.card_id == "conflict-card"
                    && a.profile.id == "merger"
                {
                    a.state = "failed".into();
                    a.terminal = None;
                    a.error = Some("Conflict resolution process exited non-zero".into());
                }
            }
        }
        ui.snapshot.set(snap);
        if scene == "board-cards-open" {
            ui.card.set("card-2".into());
            ui.page.set("card".into());
        } else if matches!(scene, "board-cards-conflict" | "board-cards-merger-failed") {
            ui.card.set("conflict-card".into());
            ui.page.set("card".into());
        } else {
            ui.card.set(String::new());
            ui.page.set("board".into());
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-overflow") {
        ui.snapshot.set(fixtures::overflowing_board());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-deep-review") {
        ui.snapshot.set(fixtures::deep_review_board());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-transitions") {
        ui.snapshot.set(fixtures::board_cards());
        ui.project.set("gallery-project".into());
        ui.card.set(String::new());
        ui.page.set("board".into());
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(board_transitions_bar(ui, reduced_motion, reject_drop))
            .child(canban_ui::frame::screen(ui))
            .into_element();
    }
    // Card header quick merge: `card-quick-merge` shows the header Merge action;
    // `card-quick-merge-warning` opens the running-Agent confirmation it raises.
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("card-quick-merge" | "card-quick-merge-warning")
    ) {
        ui.card_standalone.set(true);
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("card-quick-merge-warning") {
            let mut preview = fixtures::review();
            preview.warning = Some(
                "Card Agent is still running; merging now only includes current worktree changes"
                    .into(),
            );
            ui.quick_merge.set(Some(preview));
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    // Card header stage advance: covers Backlog -> Develop (Primary + ⚡),
    // Develop -> Review (Secondary while WorkflowRun active vs Primary when idle),
    // and final Review stage (advance button hidden).
    if let Ok(
        scene @ ("card-advance-backlog"
        | "card-advance-develop-running"
        | "card-advance-develop-idle"
        | "card-advance-review"),
    ) = std::env::var("CANBAN_GALLERY_SCENE").as_deref()
    {
        let mut snap = fixtures::automation_board();
        if scene != "card-advance-develop-running" {
            snap.workflow_runs.clear();
            snap.terminals.clear();
        } else {
            snap.workflow_runs.push(
                serde_json::from_value(json!({
                    "id": "gallery-run-develop",
                    "project_id": "gallery-project",
                    "card_id": "card-1",
                    "workflow_id": "delivery:gallery-project",
                    "definition_revision": 1,
                    "definition": snap.workflow_revisions[0].definition,
                    "state": "waiting_agent",
                    "step": "implement",
                    "visit": 1,
                    "input_digest": "gallery",
                    "inputs": {},
                    "outputs": {},
                    "history": [],
                    "created_at": 1,
                    "updated_at": 1
                }))
                .unwrap(),
            );
        }
        if let Some(card) = snap.cards.iter_mut().find(|c| c.id == "card-1") {
            card.stage = match scene {
                "card-advance-backlog" => "backlog",
                "card-advance-develop-running" | "card-advance-develop-idle" => "develop",
                _ => "review",
            }
            .into();
        }
        ui.snapshot.set(snap);
        ui.card.set("card-1".into());
        ui.page.set("card".into());
        ui.card_standalone.set(true);
        return canban_ui::frame::screen(ui).into_element();
    }
    if let Ok(
        scene @ ("card-changes"
        | "card-changes-loading"
        | "card-changes-error"
        | "card-changes-empty"
        | "card-changes-missing"),
    ) = std::env::var("CANBAN_GALLERY_SCENE").as_deref()
    {
        ui.card_standalone.set(true);
        ui.snapshot.update(|s| {
            let card = s.cards.iter_mut().find(|c| c.id == "card-1").unwrap();
            card.stage = "develop".into();
            card.branch = Some("canban/104-long-branch-name-with-中文".into());
            if scene == "card-changes-missing" {
                card.worktree = None;
                card.branch = None;
            }
        });
        let data = match scene {
            "card-changes-loading" => json!({"summary_loading":true}),
            "card-changes-error" => json!({"summary_error":"Host offline"}),
            "card-changes-empty" => json!({"files":[],"total_additions":0,"total_deletions":0}),
            _ => fixtures::git_compare(),
        };
        ui.review_state.compare_data.set(data);
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminal-directory") {
        canban_ui::frame::configure_window_chrome();
        let mut initial = fixtures::terminal_workspace();
        for ctx in &mut initial.terminal_contexts {
            ctx.directory_root = ctx.cwd.clone();
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let home_scratch = format!("{}/Workspace/scratch", home.trim_end_matches('/'));
        ui.snapshot.set(initial.clone());
        ui.open_terminal("gallery-project-root-shell".into());
        let reset = initial.clone();
        return view()
            .flex_1()
            .min_h(px(0.))
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(action("External cd", Variant::Secondary, move |_| {
                        let root = home_scratch.clone();
                        ui.snapshot.update(|s| {
                            if let Some(c) = s
                                .terminal_contexts
                                .iter_mut()
                                .find(|c| c.terminal_id == "gallery-project-root-shell")
                            {
                                c.cwd = Some(format!("{root}/src"));
                                c.directory_root = Some(root);
                                c.project_id = None;
                                c.card_id = None;
                                c.worktree_id = None;
                                c.branch = Some("feature".into());
                            }
                        })
                    }))
                    .child(action("Status only", Variant::Secondary, move |_| {
                        ui.snapshot.update(|s| {
                            for c in &mut s.terminal_contexts {
                                c.dirty = !c.dirty;
                                c.additions += 1;
                            }
                        })
                    }))
                    .child(action("New in current", Variant::Secondary, move |_| {
                        canban_ui::terminals::new_tab(ui);
                    }))
                    .child(action("New unknown", Variant::Secondary, move |_| {
                        ui.terminal_selection.set(String::new());
                        ui.project.set(String::new());
                        canban_ui::terminals::new_tab(ui);
                    }))
                    .child(action("Restore", Variant::Secondary, move |_| {
                        ui.snapshot.set(reset.clone())
                    }))
                    .child(action("Empty", Variant::Secondary, move |_| {
                        ui.snapshot.update(|s| {
                            s.terminals.clear();
                            s.terminal_contexts.clear();
                        })
                    })),
            )
            .child(canban_ui::frame::screen(ui))
            .into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminal-merged") {
        canban_ui::frame::configure_window_chrome();
        let mut initial = fixtures::terminal_workspace();
        let cid = initial
            .terminal_contexts
            .iter()
            .find(|c| c.terminal_id == "gallery-linked-shell")
            .unwrap()
            .card_id
            .clone()
            .unwrap();
        initial
            .cards
            .iter_mut()
            .find(|c| c.id == cid)
            .unwrap()
            .delivery = Some(canban_types::Delivery {
            target_branch: "main".into(),
            commit: "gallery-merge".into(),
            request_key: "gallery-merge".into(),
            merged_at: 1,
        });
        ui.snapshot.set(initial.clone());
        ui.open_terminal("gallery-linked-shell".into());
        let reset = initial;
        let unmerged_id = cid.clone();
        let archived_id = cid;
        return view()
            .flex_1()
            .min_h(px(0.))
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(action("Unmerged", Variant::Secondary, move |_| {
                        ui.snapshot.update(|s| {
                            s.cards
                                .iter_mut()
                                .find(|c| c.id == unmerged_id)
                                .unwrap()
                                .delivery = None;
                        });
                    }))
                    .child(action("Merged into main", Variant::Secondary, move |_| {
                        ui.snapshot.set(reset.clone());
                    }))
                    .child(action("Archive Card", Variant::Secondary, move |_| {
                        ui.snapshot.update(|s| {
                            s.cards
                                .iter_mut()
                                .find(|c| c.id == archived_id)
                                .unwrap()
                                .archived_at = Some(2);
                        });
                    })),
            )
            .child(canban_ui::frame::screen(ui))
            .into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminals") {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::terminal_workspace());
        ui.open_terminal("gallery-project-root-shell".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminal-folds") {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::terminal_workspace());
        ui.open_terminal("gallery-project-root-shell".into());
        ui.terminal_folds.update(|folds| {
            folds.insert(
                "scope:location:local:local:/tmp/gallery/worktrees/standalone-spike".into(),
            );
        });
        return view()
            .flex_1()
            .min_h(px(0.))
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(action("Collapse others", Variant::Secondary, move |_| {
                        canban_ui::commands::execute(ui, "terminals-collapse-others");
                    }))
                    .child(action("Expand all", Variant::Secondary, move |_| {
                        canban_ui::commands::execute(ui, "terminals-expand-all");
                    }))
                    .child(action("Narrow 210px", Variant::Secondary, move |_| {
                        ui.terminal_width.set(210.);
                    }))
                    .child(action("Default 260px", Variant::Secondary, move |_| {
                        ui.terminal_width.set(260.);
                    })),
            )
            .child(canban_ui::frame::screen(ui))
            .into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminal-close-confirm") {
        canban_ui::frame::configure_window_chrome();
        let mut s = fixtures::terminal_workspace();
        for a in s.terminals.iter_mut().filter_map(|t| t.agent.as_mut()) {
            if a.card_id == "overview-card-2" {
                a.observed = Some(json!({"state":"live","status":{"state":"working"}}));
                a.connection_state = "connected".into();
                a.lifecycle.observed_at = Some(canban_types::now());
            }
        }
        ui.snapshot.set(s);
        let id = canban_types::Conversation::key(
            "overview-card-2",
            &canban_types::Config::default().profiles[0].id,
        );
        ui.open_terminal(id.clone());
        canban_ui::terminals::close_tab(ui, &id);
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("card-terminals") {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::workspace_with_card_terminals());
        ui.open_card_with_terminal("card-2".into(), None);
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("agent-setup") {
        ui.onboarded.set(false);
        ui.page.set("agent-setup".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("project-agents") {
        let mut snapshot = fixtures::workspace();
        snapshot.projects[0].agent_overrides.insert(
            "developer".into(),
            canban_types::ProjectAgentOverride {
                agent: Some("agy".into()),
                model: Some("gemini-4-carbon".into()),
                effort: Some("high".into()),
            },
        );
        snapshot.projects[0].agent_overrides.insert(
            "merger".into(),
            canban_types::ProjectAgentOverride {
                agent: None,
                model: Some("gpt-5.4".into()),
                effort: Some("low".into()),
            },
        );
        ui.snapshot.set(snapshot);
        ui.page.set("project-agents".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("new-project" | "new-project-custom-default")
    ) {
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("new-project-custom-default") {
            let mut snapshot = fixtures::workspace();
            let default_raw = canban_workflow_engine::source::encode(
                &canban_workflow_engine::source::default_document(
                    "delivery:gallery-project",
                    canban_types::WorkspaceSettings::default(),
                ),
            )
            .unwrap();
            let plan_raw = include_str!("../workflows/plan-and-develop.yaml").to_string();
            snapshot
                .workflow_templates
                .push(canban_types::WorkflowTemplate {
                    id: canban_types::DEFAULT_WORKFLOW_TEMPLATE_ID.into(),
                    name: "Default".into(),
                    source_project_name: Some("Canban".into()),
                    sources: vec![
                        canban_types::WorkflowTemplateFile {
                            filename: "development.yaml".into(),
                            source: default_raw,
                        },
                        canban_types::WorkflowTemplateFile {
                            filename: "issue-planning.yaml".into(),
                            source: plan_raw,
                        },
                    ],
                    updated_at: canban_types::now(),
                });
            ui.snapshot.set(snapshot);
        }
        ui.card.set(String::new());
        ui.page.set("new-project".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("project-location") {
        ui.card.set(String::new());
        ui.page.set("project-location".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("settings") {
        ui.page.set("settings".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("acknowledgements") {
        ui.open_settings_section("acknowledgements");
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("notifications") {
        ui.snapshot.set(fixtures::overview());
        ui.page.set("board".into());
        canban_ui::overview::toggle(ui);
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("card-terminal") {
        ui.snapshot.set(fixtures::conversation());
        ui.open_card_with_terminal("card-1".into(), None);
        return canban_ui::frame::screen(ui).into_element();
    }
    let scene_env = std::env::var("CANBAN_GALLERY_SCENE").unwrap_or_default();
    if let Some(phase) = scene_env.strip_prefix("github-sync-") {
        let mut snapshot = fixtures::workspace();
        let project = &mut snapshot.projects[0];
        project.github = Some(canban_types::GitHubSource {
            repository: "nervosnetwork/fiber".into(),
            repository_id: 1,
            generation: "gallery".into(),
            intake: canban_types::GitHubIntake::All,
            pr_intake: canban_types::GitHubPrIntake::All,
            authors: vec![],
            labels: vec![],
            include_drafts: false,
            paused: phase == "paused",
            synced_at: Some(canban_types::now()),
            error: None,
        });
        let id = project.id.clone();
        snapshot.github_sync.insert(
            id.clone(),
            canban_types::GitHubSyncStatus {
                generation: "gallery".into(),
                phase: phase.into(),
                issues_added: 3,
                prs_added: 2,
                error: (phase == "failed").then(|| "PRs: repository access denied".into()),
            },
        );
        ui.snapshot.set(snapshot);
        ui.select_project(id);
        if phase == "settings" {
            ui.navigate("github-repository");
        }
        return canban_ui::frame::screen(ui).into_element();
    }

    if matches!(
        scene_env.as_str(),
        "github-discussion" | "github-discussion-delete-confirm"
    ) {
        fixtures::github_browser(ui, true);
        ui.github.section.set("discussion".into());
        ui.github.detail.set(json!({
            "data": [
                {
                    "id": 501,
                    "html_url": "https://github.com/fixture/repo/pull/2#issuecomment-501",
                    "user": {"login": "octocat"},
                    "author_association": "CONTRIBUTOR",
                    "created_at": "2026-11-12T08:00:00Z",
                    "updated_at": "2026-11-12T08:00:00Z",
                    "body": "Ready for review — verifies external `GitHub ↗` comment anchor jump and `#1` cross-links."
                },
                {
                    "id": 502,
                    "html_url": "https://github.com/fixture/repo/pull/2#issuecomment-502",
                    "user": {"login": "jjy"},
                    "author_association": "OWNER",
                    "created_at": "2026-11-12T08:10:00Z",
                    "updated_at": "2026-11-12T08:15:00Z",
                    "body": "Looks great! Own comments (`@jjy`) fold **Edit** and **Delete** under `⋯`, keeping only `GitHub ↗` outside."
                }
            ],
            "page": 1,
            "has_next": false
        }));
        if scene_env == "github-discussion-delete-confirm" {
            ui.github
                .deleting_comment
                .set(Some(("discussion".into(), 502)));
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        scene_env.as_str(),
        "github-pr-review" | "github-pr-review-delete-confirm" | "pr-agent-review-draft"
    ) {
        fixtures::github_browser(ui, true);
        let mut compare = fixtures::git_compare();
        compare["compare_sha"] = json!("8f3a1c2");
        compare["threads"] = json!([
            {
                "id": 801,
                "html_url": "https://github.com/fixture/repo/pull/2#discussion_r801",
                "path": "crates/ui/src/rail.rs",
                "line": 56,
                "side": "RIGHT",
                "user": {"login": "octocat"},
                "created_at": "2026-11-12T08:05:00Z",
                "updated_at": "2026-11-12T08:05:00Z",
                "body": "Please verify the pending comment badge count."
            },
            {
                "id": 802,
                "html_url": "https://github.com/fixture/repo/pull/2#discussion_r802",
                "path": "crates/ui/src/rail.rs",
                "line": 56,
                "side": "RIGHT",
                "user": {"login": "jjy"},
                "created_at": "2026-11-12T08:12:00Z",
                "updated_at": "2026-11-12T08:14:00Z",
                "body": "Verified — `GitHub ↗` stays outside and own-comment Edit/Delete live inside `⋯`."
            }
        ]);
        ui.review_state.mode.set("github_pr".into());
        ui.review_state.pr_target.set(Some(json!({
            "repository": "fixture/repo",
            "number": 2,
            "url": "https://github.com/fixture/repo/pull/2"
        })));
        ui.review_state.compare_data.set(compare);
        if scene_env == "pr-agent-review-draft" {
            ui.snapshot.update(|s| {
                let cid = s.cards[0].id.clone();
                s.review_drafts.push(canban_types::ReviewDraft {
                    card_id: cid,
                    repository: Some("fixture/repo".into()),
                    pr_number: Some(2),
                    compare_sha: Some("8f3a1c2".into()),
                    summary: "Reviewed diff: ensure rail navigation clears stale selection.".into(),
                    verdict: "request_changes".into(),
                    comments: vec![canban_types::ReviewDraftComment {
                        id: "agent-draft-1".into(),
                        path: "crates/ui/src/rail.rs".into(),
                        section: String::new(),
                        line: 56,
                        side: "RIGHT".into(),
                        snippet: "move |_| crate::review::open_compare(ui)".into(),
                        body: "Consider validating active project before opening Diff.".into(),
                        author: Some("@reviewer".into()),
                        edited_by_user: false,
                    }],
                    author: Some("@reviewer".into()),
                    updated_at: 1_763_000_000_000,
                });
            });
            let cid = ui.snapshot.with_untracked(|s| s.cards[0].id.clone());
            ui.card.set(cid.clone());
            canban_ui::review::hydrate_card_review_draft(ui, &cid, true);
            ui.review_state.submit_open.set(true);
            ui.review_state.submit_action.set("github_review".into());
        }
        ui.page.set("review".into());
        if scene_env == "github-pr-review-delete-confirm" {
            ui.review_state.deleting_remote_comment.set(Some(802));
        }
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("review-commits") {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::workspace());
        ui.select_project("gallery-project".into());
        ui.card.set(String::new());
        ui.review.set(None);
        let refs = fixtures::git_refs();
        ui.review_state.mode.set("git".into());
        ui.review_state.refs_data.set(refs.clone());
        ui.review_state.log_branch.set("feat/diff-workbench".into());
        ui.review_state
            .log_commits
            .set(refs["commits"].as_array().cloned().unwrap_or_default());
        ui.review_state
            .compare_data
            .set(fixtures::git_commit_compare());
        ui.review_state.base_ref.set("c207c08~1".into());
        ui.review_state
            .compare_ref
            .set("c207c08a91fe001234567890abcdef0123456789".into());
        ui.review_state.range_backup.set(Some((
            "git".into(),
            "main".into(),
            "feat/diff-workbench".into(),
        )));
        ui.page.set("review".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if matches!(
        std::env::var("CANBAN_GALLERY_SCENE").as_deref(),
        Ok("review-working-tree"
            | "review-uncommitted"
            | "review-async"
            | "review-large-diff"
            | "review-context-expand"
            | "review-refresh")
    ) {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::workspace());
        ui.select_project("gallery-project".into());
        ui.card.set(String::new());
        ui.review.set(None);
        let refs = fixtures::git_refs();
        ui.review_state.mode.set("git".into());
        ui.review_state.refs_data.set(refs.clone());
        ui.review_state.log_branch.set("feat/diff-workbench".into());
        ui.review_state
            .log_commits
            .set(refs["commits"].as_array().cloned().unwrap_or_default());
        ui.review_state
            .compare_data
            .set(fixtures::git_workdir_compare());
        ui.review_state.base_ref.set("HEAD".into());
        ui.review_state.compare_ref.set("UNCOMMITTED".into());
        ui.review_state
            .commit_summary
            .set("feat(review): add working tree stage & commit controls".into());
        let scene = std::env::var("CANBAN_GALLERY_SCENE").unwrap_or_default();
        if scene == "review-uncommitted" {
            let st = ui.review_state;
            st.checkout_cwd.set("/preview/worktrees/card-1".into());
            st.refs_data.update(|v| {
                v["cwd"] = json!("/preview/worktrees/card-1");
                v["related_card_id"] = json!("card-1");
            });
            let mut data = fixtures::git_workdir_compare();
            let mut extra = data["files"][0].clone();
            extra["section"] = json!("unstaged");
            extra["patch"] = json!("@@ -1 +1 @@\n-index version\n+working directory version");
            data["files"].as_array_mut().unwrap().push(extra);
            data["status_files"][0]["unstaged"] = json!(true);
            data["status_files"][0]["worktree_code"] = json!("M");
            st.refs_data.update(|v| {
                v["status_files"] = data["status_files"].clone();
                v["unstaged_count"] = json!(3);
            });
            let prepared = review::cache::ReadValue::new(data.clone());
            st.prepared_lines.set(prepared.lines);
            st.compare_data.set(data);
        }
        if scene == "review-async" {
            let st = ui.review_state;
            set_interval(std::time::Duration::from_millis(800), move || {
                let loading = !st.refs_loading.get_untracked();
                st.refs_loading.set(loading);
                st.log_loading.set(loading);
                st.refs_data
                    .update(|v| v["ahead"] = json!(if loading { 2 } else { 1 }));
                st.log_commits.update(|v| {
                    if v.len() > 1 {
                        v.rotate_left(1);
                    }
                });
            });
        }
        if scene == "review-refresh" {
            let st = ui.review_state;
            set_interval(std::time::Duration::from_millis(800), move || {
                batch(|| {
                    let loading = !st.compare_loading.get_untracked();
                    st.refs_loading.set(loading);
                    st.compare_loading.set(loading);
                    // Unrelated snapshot changes must not remount the code view.
                    ui.snapshot.update(|s| {
                        s.projects[0].name = if loading {
                            "Refresh fixture"
                        } else {
                            "Gallery"
                        }
                        .into()
                    });
                });
            });
        }
        if scene == "review-large-diff" {
            let mut data = fixtures::git_workdir_compare();
            let patch = "@@ -1,200 +1,200 @@\n".to_owned() + &"-old line\n+new line\n".repeat(200);
            data["files"] = json!((0..60).map(|i| json!({"filename":format!("src/module-{i}.rs"), "status":"modified", "additions":200, "deletions":200, "patch":patch})).collect::<Vec<_>>());
            let prepared = review::cache::ReadValue::new(data.clone());
            ui.review_state.prepared_lines.set(prepared.lines);
            ui.review_state.compare_data.set(data);
            ui.review_state
                .compare_ref
                .set("feat/diff-workbench".into());
        }
        if scene == "review-context-expand" {
            let data = fixtures::git_context_expand_compare();
            let prepared = review::cache::ReadValue::new(data.clone());
            ui.review_state.prepared_lines.set(prepared.lines);
            ui.review_state.compare_data.set(data);
            ui.review_state.base_ref.set("main".into());
            ui.review_state
                .compare_ref
                .set("feat/diff-context-expand".into());
        }
        ui.page.set("review".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("schedules") {
        canban_ui::frame::configure_window_chrome();
        let (snapshot, draft) = fixtures::schedules();
        ui.snapshot.set(snapshot);
        ui.select_project("gallery-project".into());
        ui.workflows.drafts.update(|d| {
            d.insert("gallery-project".into(), draft);
        });
        canban_ui::schedules::open(ui, None, None);
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("card-scheduled") {
        canban_ui::frame::configure_window_chrome();
        let (snapshot, draft) = fixtures::schedules();
        ui.snapshot.set(snapshot);
        ui.select_project("gallery-project".into());
        ui.workflows.drafts.update(|d| {
            d.insert("gallery-project".into(), draft);
        });
        ui.open_card("card-5".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("motion") {
        ui.page.set("board".into());
        return view()
            .flex_1()
            .min_h(px(0.))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .child(action("Open first", Variant::Quiet, move |_| {
                        ui.open_card("card-1".into())
                    }))
                    .child(action("Open second", Variant::Quiet, move |_| {
                        ui.open_card("card-2".into())
                    }))
                    .child(action("Close Card", Variant::Quiet, move |_| {
                        ui.close_card()
                    }))
                    .child(action("Terminals", Variant::Quiet, move |_| {
                        ui.navigate("terminals")
                    }))
                    .child(action("Board", Variant::Quiet, move |_| {
                        ui.navigate("board")
                    }))
                    .child(action("Reverse in 80ms", Variant::Quiet, move |_| {
                        let was_open = ui.page.get_untracked() == "card";
                        ui.page.set(if was_open { "board" } else { "card" }.into());
                        ui.card.set("card-1".into());
                        set_timeout(std::time::Duration::from_millis(80), move || {
                            ui.page.set(if was_open { "card" } else { "board" }.into());
                        });
                    }))
                    .child(action("Reduce motion", Variant::Quiet, move |_| {
                        reduced_motion.set(!reduced_motion.get_untracked())
                    }))
                    .child(action("Quick Open", Variant::Quiet, move |_| {
                        ui.quick_open.set(true)
                    }))
                    .child(action("Narrow / wide", Variant::Quiet, move |_| {
                        constrained.set(!constrained.get_untracked())
                    })),
            )
            .child(
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .max_w(move || px(if constrained.get() { 720. } else { 10000. }))
                    .child(canban_ui::frame::screen(ui)),
            )
            .into_element();
    }

    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(BACKGROUND)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .p_3()
                .gap_2()
                .bg(SURFACE)
                .child(label("Canban Gallery"))
                .child(action("Board workspace", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace());
                    ui.select_project("gallery-project".into());
                    ui.card.set("card-1".into());
                    ui.page.set("card".into());
                }))
                .child(action("Board transitions", Variant::Secondary, move |_| {
                    scene.set("board-transitions".into());
                    ui.snapshot.set(fixtures::board_cards());
                    ui.select_project("gallery-project".into());
                    ui.card.set(String::new());
                    ui.page.set("board".into());
                }))
                .child(action("Overflowing Board", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::overflowing_board());
                    ui.select_project("gallery-project".into());
                    ui.page.set("board".into());
                }))
                .child(action("Deep Review (36)", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::deep_review_board());
                    ui.select_project("gallery-project".into());
                    ui.page.set("board".into());
                }))
                .child(action("Project activity", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::project_activity());
                    ui.select_project("gallery-project".into());
                }))
                .child(action("Card conversation", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::conversation());
                    ui.open_card("card-1".into());
                }))
                .child(action("Card Terminals", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace_with_card_terminals());
                    ui.open_card_with_terminal("card-2".into(), None);
                }))
                .child(action("Card + Live Terminal", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::conversation());
                    ui.open_card_with_terminal("card-1".into(), None);
                }))
                .child(action("Activity Popover", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::overview());
                    ui.page.set("board".into());
                    canban_ui::overview::toggle(ui);
                }))
                .child(action("Git Compare", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace());
                    ui.select_project("gallery-project".into());
                    ui.card.set(String::new());
                    ui.review.set(None);
                    let refs = fixtures::git_refs();
                    ui.review_state.mode.set("git".into());
                    ui.review_state.refs_data.set(refs.clone());
                    ui.review_state.log_branch.set("feat/diff-workbench".into());
                    ui.review_state.log_commits.set(
                        refs["commits"].as_array().cloned().unwrap_or_default(),
                    );
                    ui.review_state.compare_data.set(fixtures::git_compare());
                    ui.review_state.base_ref.set("main".into());
                    ui.review_state.compare_ref.set("feat/diff-workbench".into());
                    ui.page.set("review".into());
                }))
                .child(action("Diff Context Expand", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace());
                    ui.select_project("gallery-project".into());
                    ui.card.set(String::new());
                    ui.review.set(None);
                    let refs = fixtures::git_refs();
                    let data = fixtures::git_context_expand_compare();
                    let prepared = review::cache::ReadValue::new(data.clone());
                    ui.review_state.mode.set("git".into());
                    ui.review_state.refs_data.set(refs.clone());
                    ui.review_state.log_branch.set("feat/diff-workbench".into());
                    ui.review_state.log_commits.set(
                        refs["commits"].as_array().cloned().unwrap_or_default(),
                    );
                    ui.review_state.prepared_lines.set(prepared.lines);
                    ui.review_state.compare_data.set(data);
                    ui.review_state.base_ref.set("main".into());
                    ui.review_state.compare_ref.set("feat/diff-context-expand".into());
                    ui.page.set("review".into());
                }))
                .child(action("Git Commit Detail", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace());
                    ui.select_project("gallery-project".into());
                    ui.card.set(String::new());
                    ui.review.set(None);
                    let refs = fixtures::git_refs();
                    ui.review_state.mode.set("git".into());
                    ui.review_state.refs_data.set(refs.clone());
                    ui.review_state.log_branch.set("feat/diff-workbench".into());
                    ui.review_state.log_commits.set(
                        refs["commits"].as_array().cloned().unwrap_or_default(),
                    );
                    ui.review_state.compare_data.set(fixtures::git_commit_compare());
                    ui.review_state.base_ref.set("c207c08~1".into());
                    ui.review_state
                        .compare_ref
                        .set("c207c08a91fe001234567890abcdef0123456789".into());
                    ui.review_state.range_backup.set(Some((
                        "git".into(),
                        "main".into(),
                        "feat/diff-workbench".into(),
                    )));
                    ui.page.set("review".into());
                }))
                .child(action("Git Working Tree", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace());
                    ui.select_project("gallery-project".into());
                    ui.card.set(String::new());
                    ui.review.set(None);
                    let refs = fixtures::git_refs();
                    ui.review_state.mode.set("git".into());
                    ui.review_state.refs_data.set(refs.clone());
                    ui.review_state.log_branch.set("feat/diff-workbench".into());
                    ui.review_state.log_commits.set(
                        refs["commits"].as_array().cloned().unwrap_or_default(),
                    );
                    ui.review_state.compare_data.set(fixtures::git_workdir_compare());
                    ui.review_state.base_ref.set("HEAD".into());
                    ui.review_state.compare_ref.set("UNCOMMITTED".into());
                    ui.page.set("review".into());
                }))
                .child(action("Review delivery", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::workspace());
                    ui.card.set("card-1".into());
                    ui.review_state.mode.set("card_local".into());
                    ui.review_state.base_ref.set("main".into());
                    ui.review_state.compare_ref.set("canban/gallery/1".into());
                    ui.review.set(Some(fixtures::review()));
                    ui.page.set("review".into());
                }))
                .child(action("Delivered receipt", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    let mut snapshot = fixtures::workspace();
                    snapshot.cards[0].delivery = Some(canban_types::Delivery {
                        target_branch: "main".into(),
                        commit: "91ab162fe5e5b9".into(),
                        merged_at: canban_types::now(),
                        request_key: "gallery-delivery".into(),
                    });
                    ui.snapshot.set(snapshot);
                    ui.card.set("card-1".into());
                    ui.review_state.mode.set("card_local".into());
                    ui.review.set(None);
                    ui.page.set("review".into());
                }))
                .child(action("Append reply", Variant::Quiet, move |_| {
                    ui.snapshot.update(|s| {
                        let mut event=fixtures::timeline();
                        event.sequence=s.events.iter().map(|e|e.sequence).max().unwrap_or(0)+1;
                        event.id=format!("new-{}",event.sequence); event.card_id="card-1".into();
                        event.kind="comment.replied".into(); event.actor="agent:developer".into();
                        event.message="New reply arrived while you were reading.\n\nContinue from your current position, or jump to latest.".into();
                        s.events.push(event);
                    });
                }))
                .child(action("Reply in 5s", Variant::Quiet, move |_| {
                    set_timeout(std::time::Duration::from_secs(5), move || {
                        ui.snapshot.update(|s| {
                            let mut event = fixtures::timeline();
                            event.sequence = s.events.iter().map(|e| e.sequence).max().unwrap_or(0) + 1;
                            event.id = format!("delayed-{}", event.sequence);
                            event.card_id = "card-1".into();
                            event.kind = "comment.replied".into();
                            event.message = "**Background reply**: editor focus, selection and undo should survive.".into();
                            s.events.push(event);
                        });
                    });
                }))
                .child(action("Overview", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::overview());
                    ui.page.set("overview".into());
                }))
                .child(action("Terminals", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::terminal_workspace());
                    let mut profiles=canban_types::Config::default().profiles;
                    for p in &mut profiles {p.argv=Some(vec!["fixture-agent".into()]);p.resume_argv=Some(vec!["fixture-agent".into(),"--resume".into(),"{session_id}".into()]);}
                    ui.profiles.set(profiles);
                    ui.navigate("terminals");
                    ui.terminal_sidebar.set(true);
                    ui.open_terminal(canban_types::Conversation::key(
                        "overview-card-2",
                        &canban_types::Config::default().profiles[0].id,
                    ));
                }))
                .child(action("Close Running Agent?", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    let mut s = fixtures::terminal_workspace();
                    for a in s.terminals.iter_mut().filter_map(|t| t.agent.as_mut()) {
                        if a.card_id == "overview-card-2" {
                            a.observed = Some(json!({"state":"live","status":{"state":"working"}}));
                            a.connection_state = "connected".into();
                            a.lifecycle.observed_at = Some(canban_types::now());
                        }
                    }
                    ui.snapshot.set(s);
                    ui.navigate("terminals");
                    ui.terminal_sidebar.set(true);
                    let id = canban_types::Conversation::key(
                        "overview-card-2",
                        &canban_types::Config::default().profiles[0].id,
                    );
                    ui.open_terminal(id.clone());
                    canban_ui::terminals::close_tab(ui, &id);
                }))
                .child(action("Idle & Attention Terminals",Variant::Secondary,move |_|{
                    let mut snapshot=fixtures::terminal_workspace();
                    for card in &mut snapshot.cards {card.human_gate=None;card.attention=None;}
                    for (idx, a) in snapshot.terminals.iter_mut().filter_map(|t| t.agent.as_mut()).enumerate() {
                        if a.terminal.is_some() && !a.finished() {
                            a.state = if idx == 0 { "running".into() } else { "completed".into() };
                            let status = if idx == 0 { "idle" } else { "done" };
                            a.observed=Some(json!({"state":"live","turn":1,"status":{"state":status}}));
                            a.connection_state="connected".into();
                            a.lifecycle.observed_at=Some(canban_types::now());
                            a.lifecycle.last_interaction_at=None;
                        }
                    }
                    ui.snapshot.set(snapshot);ui.terminal_tabs.set(Vec::new());ui.terminal_selection.set(String::new());
                    ui.navigate("terminals");scene.set("workspace".into());
                }))
                .child(action("Quick Open", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::search());
                    ui.card.set(String::new());
                    ui.page.set("board".into());
                    ui.show_search(false);
                }))
                .child(action("Empty Board", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(Default::default());
                    ui.select_project(String::new());
                }))
                .child(action("Long Project", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    let mut snapshot = fixtures::workspace();
                    snapshot.projects[0].name = "Canban · 中文架构与一个足够长的项目名称 · Context and workflow decisions across the workspace".into();
                    ui.snapshot.set(snapshot);
                    ui.select_project("gallery-project".into());
                }))
                .child(action("Scheduling", Variant::Quiet, move |_| {
                    scene.set("scheduling".into());
                }))
                .child(action("Update available",Variant::Quiet,move |_|{scene.set("workspace".into());ui.update.set(json!({"build":canban_updater::Build::current(),"state":{"phase":"available","available":{"manifest":{"version":"20261110-3"}}}}));}))
                .child(action("Update failed",Variant::Quiet,move |_|{scene.set("workspace".into());ui.update.set(json!({"build":canban_updater::Build::current(),"state":{"phase":"error","error":"Download interrupted. Retry the update.","available":{"manifest":{"version":"20261110-3"}}}}));}))
                .child(action("Archived Cards", Variant::Quiet, move |_| {
                    ui.snapshot.set(fixtures::archive());ui.project.set("gallery-project".into());ui.card.set(String::new());ui.page.set("archive".into());scene.set("workspace".into());
                }))
                .child(action("Card states", Variant::Quiet, move |_| {
                    scene.set("workspace".into());ui.snapshot.set(fixtures::board_cards());ui.card.set(String::new());ui.page.set("board".into());
                }))
                .child(action("Card states · Card open", Variant::Quiet, move |_| {
                    scene.set("workspace".into());ui.snapshot.set(fixtures::board_cards());ui.card.set("card-2".into());ui.page.set("card".into());
                }))
                .child(action("Column automations", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::automation_board());
                    ui.card.set(String::new());
                    ui.page.set("board".into());
                }))
                .child(action("Card advance", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    let mut snap = fixtures::automation_board();
                    snap.workflow_runs.clear();
                    snap.terminals.clear();
                    if let Some(card) = snap.cards.iter_mut().find(|c| c.id == "card-1") {
                        card.stage = "backlog".into();
                    }
                    ui.snapshot.set(snap);
                    ui.card.set("card-1".into());
                    ui.page.set("card".into());
                }))
                .child(action("Usage fixtures", Variant::Quiet, move |_| {
                    ui.usage.set(json!({"machine":"local","updated_at":canban_types::now(),"readings":[{"provider":"codex","auth":"present","account":"demo@example.invalid","windows":[{"name":"Session","used":22,"resets_at":chrono::DateTime::from_timestamp((canban_types::now()/1000+5*86400) as i64,0).unwrap().to_rfc3339()}]},{"provider":"claude","auth":"present","account":"other@example.invalid","stale":true,"error":"HTTP 429","windows":[{"name":"Weekly","used":68,"resets_at":chrono::DateTime::from_timestamp((canban_types::now()/1000+7*86400) as i64,0).unwrap().to_rfc3339()}]}]}));
                }))
                .child(action("No signed-in providers", Variant::Quiet, move |_| {ui.usage.set(json!({"readings":[]}));}))
                .child(action("New Project", Variant::Quiet, move |_| {
                    let mut snapshot = fixtures::workspace();
                    let default_raw = canban_workflow_engine::source::encode(
                        &canban_workflow_engine::source::default_document(
                            "delivery:gallery-project",
                            canban_types::WorkspaceSettings::default(),
                        ),
                    )
                    .unwrap();
                    snapshot
                        .workflow_templates
                        .push(canban_types::WorkflowTemplate {
                            id: canban_types::DEFAULT_WORKFLOW_TEMPLATE_ID.into(),
                            name: "Default".into(),
                            source_project_name: Some("Canban".into()),
                            sources: vec![canban_types::WorkflowTemplateFile {
                                filename: "development.yaml".into(),
                                source: default_raw,
                            }],
                            updated_at: canban_types::now(),
                        });
                    ui.snapshot.set(snapshot);
                    scene.set("new-project-detected".into());
                }))
                .child(action("GitHub Repo · Auto-linked", Variant::Quiet, move |_| {
                    let mut snapshot = fixtures::workspace();
                    snapshot.projects[0].github = Some(canban_types::GitHubSource {
                        pr_intake: canban_types::GitHubPrIntake::Off,
                        authors: vec![],
                        labels: vec![],
                        include_drafts: false,
                        paused: false,
                        repository: "jjyr/canban".into(),
                        repository_id: 1,
                        generation: "gallery".into(),
                        intake: canban_types::GitHubIntake::Off,
                        synced_at: Some(canban_types::now()),
                        error: None,
                    });
                    snapshot.projects[0].detected_github = Some(canban_types::DetectedGitHub {
                        repository: "jjyr/canban".into(),
                        remote: "origin".into(),
                    });
                    scene.set("workspace".into());
                    ui.snapshot.set(snapshot);
                    ui.select_project("gallery-project".into());
                    ui.navigate("github-repository");
                }))
                .child(action("GitHub Repo · Disconnected", Variant::Quiet, move |_| {
                    let mut snapshot = fixtures::workspace();
                    snapshot.projects[0].github = None;
                    snapshot.projects[0].github_disconnected = true;
                    snapshot.projects[0].detected_github = Some(canban_types::DetectedGitHub {
                        repository: "jjyr/canban".into(),
                        remote: "origin".into(),
                    });
                    scene.set("workspace".into());
                    ui.snapshot.set(snapshot);
                    ui.select_project("gallery-project".into());
                    ui.navigate("github-repository");
                }))
                .child(action("New Card", Variant::Quiet, move |_| {
                    let mut snapshot=fixtures::workspace();
                    snapshot.projects[0].github=Some(canban_types::GitHubSource { pr_intake:Default::default(),authors:vec![],labels:vec![],include_drafts:false,paused:false,repository:"jjyr/canban".into(),repository_id:1,generation:"gallery".into(),intake:canban_types::GitHubIntake::All,synced_at:None,error:None });
                    scene.set("workspace".into());ui.snapshot.set(snapshot);ui.new_card();
                }))
                .child(action("GitHub inbox",Variant::Quiet,move |_|{scene.set("workspace".into());fixtures::github_browser(ui,false);}))
                .child(action("GitHub onboarding",Variant::Quiet,move |_|{
                    scene.set("workspace".into());
                    fixtures::github_browser(ui,false);
                    ui.github.gh_status.set(json!({"state":"signed_out","installed":true,"message":"GitHub CLI returned no HTTP response; check gh auth status"}));
                    ui.github.list.set(json!({"items":[]}));
                    ui.github.error.set("GitHub CLI returned no HTTP response; check gh auth status".into());
                }))
                .child(action("GitHub PR diff",Variant::Quiet,move |_|{scene.set("workspace".into());fixtures::github_browser(ui,true);}))
                .child(action("GitHub discussion",Variant::Quiet,move |_|{
                    scene.set("workspace".into());
                    fixtures::github_browser(ui,true);
                    ui.github.section.set("discussion".into());
                    ui.github.detail.set(json!({
                        "data": [
                            {
                                "id": 501,
                                "html_url": "https://github.com/fixture/repo/pull/2#issuecomment-501",
                                "user": {"login": "octocat"},
                                "author_association": "CONTRIBUTOR",
                                "created_at": "2026-11-12T08:00:00Z",
                                "updated_at": "2026-11-12T08:00:00Z",
                                "body": "Ready for review — verifies inline diff and `#1` cross-links."
                            },
                            {
                                "id": 502,
                                "html_url": "https://github.com/fixture/repo/pull/2#issuecomment-502",
                                "user": {"login": "jjy"},
                                "author_association": "OWNER",
                                "created_at": "2026-11-12T08:10:00Z",
                                "updated_at": "2026-11-12T08:15:00Z",
                                "body": "Looks great! Own comments (`@jjy`) fold **Edit** and **Delete** under `⋯`, keeping only `GitHub ↗` outside."
                            }
                        ],
                        "page": 1,
                        "has_next": false
                    }));
                }))
                .child(action("GitHub offline cache",Variant::Quiet,move |_|{scene.set("workspace".into());fixtures::github_browser(ui,false);ui.github.error.set("Offline · showing cached GitHub content".into());}))
                .child(action("GitHub Card",Variant::Quiet,move |_|{
                    let mut snapshot=fixtures::workspace();
                    let source=canban_types::GitHubIssueRef{repository:"jjyr/canban".into(),repository_id:1,issue_id:186,number:186,url:"https://github.com/jjyr/canban/issues/186".into()};
                    snapshot.cards[0].id="github-card".into();
                    snapshot.cards[0].content=canban_types::CardContent::GitHubIssue{source:source.clone()};
                    snapshot.github_issues.push(canban_types::GitHubIssueSnapshot{assignees:vec![],id:"gallery-issue".into(),project_id:"gallery-project".into(),source,title:"Improve search".into(),body:"## Remote issue\nPreserve context when searching.".into(),state:"open".into(),author_id:1,author_login:"jjyr".into(),labels:vec![],updated_at:"2026-11-09".into(),synced_at:0});
                    ui.snapshot.set(snapshot);scene.set("workspace".into());ui.open_card("github-card".into());
                }))
                .child(action("Settings", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.navigate("settings");
                }))
                .child(action("Settings · Agents", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.open_settings_section("agents");
                }))
                .child(action("Multi-host · Online", Variant::Quiet, move |_| {
                    let (snap, hosts) = fixtures::multi_host_workspace(false);
                    ui.discovery.set(fixtures::discovery_catalog());
                    ui.hosts.set(hosts);
                    ui.snapshot.set(snap);
                    ui.select_project("remote-linux-core".into());
                    scene.set("workspace".into());
                    ui.navigate("board");
                    canban_ui::hosts::toggle_popover(ui);
                }))
                .child(action("Multi-host · Offline Cache", Variant::Quiet, move |_| {
                    let (snap, hosts) = fixtures::multi_host_workspace(true);
                    ui.discovery.set(fixtures::discovery_catalog());
                    ui.hosts.set(hosts);
                    ui.snapshot.set(snap);
                    ui.select_project("remote-linux-core".into());
                    scene.set("workspace".into());
                    canban_ui::hosts::close_overlays(ui);
                    ui.navigate("board");
                }))
                .child(action("Multi-host · API Mismatch", Variant::Quiet, move |_| {
                    let (mut snap, mut hosts) = fixtures::multi_host_workspace(false);
                    if let Some(h) = hosts.iter_mut().find(|h| h.id == "home-linux") {
                        h.connection = canban_types::HostConnectionState::ApiMismatch {
                            server_api: canban_updater::DESKTOP_API.saturating_sub(2),
                            app_api: canban_updater::DESKTOP_API,
                            version: "20261115-2".into(),
                        };
                        snap.host_summaries.insert(h.id.clone(), h.clone());
                    }
                    ui.discovery.set(fixtures::discovery_catalog());
                    ui.hosts.set(hosts);
                    ui.snapshot.set(snap);
                    ui.select_project("remote-linux-core".into());
                    scene.set("workspace".into());
                    ui.navigate("board");
                    canban_ui::hosts::toggle_popover(ui);
                }))
                .child(action("Multi-host · Updating", Variant::Quiet, move |_| {
                    let (mut snap, mut hosts) = fixtures::multi_host_workspace(false);
                    if let Some(h) = hosts.iter_mut().find(|h| h.id == "home-linux") {
                        h.connection = canban_types::HostConnectionState::Updating {
                            version: "20261116-1".into(),
                        };
                        snap.host_summaries.insert(h.id.clone(), h.clone());
                    }
                    ui.discovery.set(fixtures::discovery_catalog());
                    ui.hosts.set(hosts);
                    ui.snapshot.set(snap);
                    ui.select_project("remote-linux-core".into());
                    scene.set("workspace".into());
                    ui.navigate("board");
                    canban_ui::hosts::toggle_popover(ui);
                }))
                .child(action("Connect Machine", Variant::Quiet, move |_| {
                    let (snap, hosts) = fixtures::multi_host_workspace(false);
                    ui.discovery.set(fixtures::discovery_catalog());
                    ui.hosts.set(hosts);
                    ui.snapshot.set(snap);
                    scene.set("workspace".into());
                    ui.navigate("board");
                    canban_ui::hosts::open_connect_modal(ui);
                }))
                .child(action("Diagnostics", Variant::Quiet, move |_| {
                    let (snap, hosts) = fixtures::multi_host_workspace(false);
                    ui.discovery.set(fixtures::discovery_catalog());
                    ui.hosts.set(hosts);
                    ui.snapshot.set(snap);
                    scene.set("workspace".into());
                    canban_ui::hosts::open_host_settings(ui, "home-linux".into(), "diagnostics");
                    canban_ui::diagnostics::refresh_diagnostics(ui);
                }))
                .child(action("Insights", Variant::Quiet, move |_| {
                    ui.snapshot.set(fixtures::insights());
                    ui.project.set("gallery-project".into());
                    ui.insights.scope_project.set(String::new());
                    scene.set("workspace".into());
                    ui.navigate("insights");
                }))
                .child(action("Acknowledgements", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.open_settings_section("acknowledgements");
                }))
                .child(action("Agent Setup", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.navigate("agent-setup");
                }))
                .child(action("Project Agents", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    let mut snapshot = fixtures::workspace();
                    snapshot.projects[0].agent_overrides.insert(
                        "developer".into(),
                        canban_types::ProjectAgentOverride {
                            agent: Some("agy".into()),
                            model: Some("gemini-4-carbon".into()),
                            effort: Some("high".into()),
                        },
                    );
                    snapshot.projects[0].agent_overrides.insert(
                        "merger".into(),
                        canban_types::ProjectAgentOverride {
                            agent: None,
                            model: Some("gpt-5.4".into()),
                            effort: Some("low".into()),
                        },
                    );
                    ui.snapshot.set(snapshot);
                    ui.select_project("gallery-project".into());
                    ui.navigate("project-agents");
                }))
                .child(action("Workflows", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    let default_raw = canban_workflow_engine::source::encode(
                        &canban_workflow_engine::source::default_document(
                            "delivery:gallery-project",
                            canban_types::WorkspaceSettings::default(),
                        ),
                    )
                    .unwrap();
                    let plan_raw = include_str!("../workflows/plan-and-develop.yaml").to_string();
                    let mut draft = canban_ui::workflows::Draft {
                        base: json!({"revision":"gallery","sources":[
                            {"filename":"development.yaml","source":default_raw,"active":{"revision":1,"workflow_id":"delivery:gallery-project"}},
                            {"filename":"issue-planning.yaml","source":plan_raw,"active":{"revision":1,"workflow_id":"gallery-workflow"}}
                        ]}),
                        ..Default::default()
                    };
                    draft.sources.insert("development.yaml".into(), default_raw);
                    draft
                        .sources
                        .insert("issue-planning.yaml".into(), plan_raw);
                    draft.selected = "development.yaml".into();
                    ui.workflows.drafts.update(|d| {
                        d.insert(ui.project.get_untracked(), draft);
                    });
                    ui.workflows.section.set("workflows".into());
                    ui.navigate("workflows");
                }))
                .child(action("Schedules", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    let (snapshot, draft) = fixtures::schedules();
                    ui.snapshot.set(snapshot);
                    ui.select_project("gallery-project".into());
                    ui.workflows.drafts.update(|d| {
                        d.insert("gallery-project".into(), draft);
                    });
                    canban_ui::schedules::open(ui, None, None);
                }))
                .child(action("Toast preview", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.notify("Copied to clipboard");
                }))
                .child(action("Work location",Variant::Quiet,move |_|{scene.set("workspace".into());ui.navigate("project-location");}))
                .child(action("Controls", Variant::Quiet, move |_| {
                    scene.set("controls".into())
                }))
                .child(action("Herdr setup", Variant::Quiet, move |_| {
                    scene.set("herdr-missing".into())
                }))
                .child(action("Narrow viewport", Variant::Quiet, move |_| {
                    constrained.set(true)
                }))
                .child(action("Full viewport", Variant::Quiet, move |_| {
                    constrained.set(false)
                })),
        )
        .child(dynamic(move || {
            if scene.get() == "scheduling" {
                return view()
                    .flex_1()
                    .p_5()
                    .child(canban_ui::components::scheduling::settings(ui))
                    .into_element();
            }
            if scene.get() == "controls" {
                return controls().into_element();
            }
            if scene.get() == "herdr-missing" {
                return gallery_herdr_setup_fixture(ui, "herdr-missing").into_element();
            }
            if scene.get() == "new-project-detected" {
                return canban_ui::projects::new_project_form(
                    ui,
                    "/Users/jjy/Workspace/canban".into(),
                    Some(canban_types::DetectedGitHub {
                        repository: "jjyr/canban".into(),
                        remote: "origin".into(),
                    }),
                )
                .into_element();
            }
            let narrow_preview = constrained.get();
            let current_scene = scene.get();
            view()
                .flex_1()
                .w_full()
                .max_w(px(if narrow_preview { 720.0 } else { 10000.0 }))
                .min_h(px(0.0))
                .children(
                    (current_scene == "board-transitions")
                        .then(|| board_transitions_bar(ui, reduced_motion, reject_drop)),
                )
                .child(canban_ui::frame::screen(ui))
                .into_element()
        })).into_element()
}

fn main() {
    let app_id =
        std::env::var("CANBAN_GALLERY_APP_ID").unwrap_or_else(|_| "com.canban.gallery".into());
    canban_ui::platform::init_host(vec![nsh_base::registry()]);
    let size = (
        std::env::var("CANBAN_GALLERY_WIDTH")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1440),
        std::env::var("CANBAN_GALLERY_HEIGHT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(960),
    );
    let ok = canban_ui::platform::run_root::<Gallery>(app_id, "Canban Gallery", size);
    std::process::exit(if ok { 0 } else { 1 });
}
