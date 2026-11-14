//! Standalone native component gallery. Never opens a Canban home or runtime.
mod fixtures;
use canban_ui::{presenter::Ui, profiles, review};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

fn gallery_herdr_setup_fixture(ui: Ui, mode: &str) -> impl IntoElement {
    let initial = match mode {
        "herdr-installing" => canban_ui::startup::HerdrSetupView {
            home: "/preview/.canban".into(),
            installing: true,
            logs: vec![
                "Downloading installer from https://releases.herdr.dev/core/install.sh".into(),
                "detected macos/aarch64".into(),
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
            error: Some(
                "Herdr Core (`hc`) was not found in ~/.local/bin, /opt/homebrew/bin, /usr/local/bin, or PATH.".into(),
            ),
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
                            "detected macos/aarch64".into(),
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
    let directory = signal(String::new());
    let markdown = canban_ui_kit::markdown::MarkdownInput::new();
    let group_open = signal(true);
    let nav_selected = signal(0_usize);
    let gutter_scroll = element_ref();
    let split_width = signal(240.0_f32);
    view().flex_1().min_h(px(0.0)).p_5().gap_4().bg(BACKGROUND)
    .child(heading("Component Gallery"))
    .child(muted("Production components · local fixtures · no AppServer or Agent required"))
    .child(
        scroll().flex_1().child(
            view()
                .gap_4()
                .child(
                    section_group("Borderless Section Group")
                        .description("Reusable header + 1px bottom divider without an enclosing card border, shared across Overview, GitHub, and Settings.")
                        .count(2)
                        .tone(Tone::Info)
                        .child(entry_card(Icon::Plus, "Create a Project", "One accessible clickable surface.", move |_| {
                            count.set(count.get_untracked() + 1);
                        }))
                        .child(directory_field(directory, |_| {})),
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
                            [Icon::Search, Icon::Terminal, Icon::Compare, Icon::Board, Icon::Overview,
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
                    panel().child(label("Native libghostty terminal")).child(
                        native("canban/terminal")
                            .prop("theme", canban_ui::utilities::terminal_theme)
                            .prop("font", move || canban_ui::utilities::terminal_font(ui))
                            .prop("content", json!(fixtures::TERMINAL))
                            .prop("read_only", json!(true))
                            .h(px(320.0)),
                    ),
                ),
        ),
    )
}

#[component]
pub fn Gallery() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    let scene = signal("workspace".to_string());
    let constrained = signal(false);
    let ui = Ui::new();
    canban_ui::links::install(ui);
    let reduced_motion = signal(false);
    provide_context(canban_ui_kit::motion::ReducedMotion(reduced_motion));
    ui.board_move.replace(Some(std::rc::Rc::new(move |drop| {
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
    ui.machines.set(json!({"machines":[{"id":"gallery-remote","name":"Build machine","state":"connected","target":"ssh-host"},{"id":"gallery-offline","name":"Offline machine","state":"unreachable","target":"another-host"}]}));
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
        Ok("agent-setup" | "settings-agents")
    ) {
        ui.card.set(String::new());
        if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("agent-setup") {
            ui.page.set("agent-setup".into());
        } else {
            ui.open_settings_section("agents");
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
            manual_number: None,
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
                                manual_number: None,
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
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-bottom-drag") {
        ui.snapshot.set(fixtures::bottom_card_drag());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    // Card tile states (running Agent pulse, gates, branch); `-open` keeps a
    // Card pane open so a narrow Board switches to one stage with stage tabs.
    if let Ok(scene @ ("board-cards" | "board-cards-open" | "board-cards-conflict")) =
        std::env::var("CANBAN_GALLERY_SCENE").as_deref()
    {
        ui.snapshot.set(fixtures::board_cards());
        if scene == "board-cards-open" {
            ui.card.set("card-2".into());
            ui.page.set("card".into());
        } else if scene == "board-cards-conflict" {
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
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminals") {
        canban_ui::frame::configure_window_chrome();
        ui.snapshot.set(fixtures::terminal_workspace());
        ui.open_terminal("gallery-manual".into());
        return canban_ui::frame::screen(ui).into_element();
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
                model: Some("gemini-4-carbon-high".into()),
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
        "github-pr-review" | "github-pr-review-delete-confirm"
    ) {
        fixtures::github_browser(ui, true);
        let mut compare = fixtures::git_compare();
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
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("review-working-tree") {
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
        ui.review_state.compare_ref.set("WORKDIR".into());
        ui.review_state
            .commit_summary
            .set("feat(review): add working tree stage & commit controls".into());
        ui.page.set("review".into());
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
                        ui.card.set(String::new());
                        ui.page.set("board".into());
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
                .child(action("Overflowing Board", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::overflowing_board());
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
                    ui.review_state.compare_ref.set("WORKDIR".into());
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
                .child(action("Usage fixtures", Variant::Quiet, move |_| {
                    ui.usage.set(json!({"machine":"local","updated_at":canban_types::now(),"readings":[{"provider":"codex","auth":"present","account":"demo@example.invalid","windows":[{"name":"Session","used":22,"resets_at":chrono::DateTime::from_timestamp((canban_types::now()/1000+5*86400) as i64,0).unwrap().to_rfc3339()}]},{"provider":"claude","auth":"present","account":"other@example.invalid","stale":true,"error":"HTTP 429","windows":[{"name":"Weekly","used":68,"resets_at":chrono::DateTime::from_timestamp((canban_types::now()/1000+7*86400) as i64,0).unwrap().to_rfc3339()}]}]}));
                }))
                .child(action("No signed-in providers", Variant::Quiet, move |_| {ui.usage.set(json!({"readings":[]}));}))
                .child(action("New Project", Variant::Quiet, move |_| {
                    scene.set("workspace".into());
                    ui.navigate("new-project");
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
                            model: Some("gemini-4-carbon-high".into()),
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
                    ui.navigate("workflows");
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
            let narrow_preview = constrained.get();
            view()
                .flex_1()
                .w_full()
                .max_w(px(if narrow_preview { 720.0 } else { 10000.0 }))
                .min_h(px(0.0))
                .child(canban_ui::frame::screen(ui))
                .into_element()
        })).into_element()
}

fn main() {
    let app_id =
        std::env::var("CANBAN_GALLERY_APP_ID").unwrap_or_else(|_| "com.canban.gallery".into());
    nsh_platform_macos::set_components(canban_terminal::native::register);
    nsh_platform_macos::set_ui_sets(vec![nsh_base::registry()]);
    let result = nsh_platform_macos::run_app(
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<Gallery>().app_id(app_id.clone()),
        ),
        nsh_platform_macos::AppOptions {
            title: "Canban Gallery".into(),
            app_id,
            size: (
                std::env::var("CANBAN_GALLERY_WIDTH")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1440),
                std::env::var("CANBAN_GALLERY_HEIGHT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(960),
            ),
            ..Default::default()
        },
    );
    std::process::exit(if result == std::process::ExitCode::SUCCESS {
        0
    } else {
        1
    });
}
