//! Standalone native component gallery. Never opens a Canban home or runtime.
mod fixtures;
use canban_ui::{components::timeline_item, presenter::Ui, profiles, review};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

fn controls() -> impl IntoElement {
    let ui = Ui::new();
    let input = signal("中文与 English · long text fixture".into());
    let profile_fixture = signal(canban_types::Config::default().profiles);
    let count = signal(0_u32);
    let directory = signal(String::new());
    let markdown = canban_ui_kit::markdown::MarkdownInput::new();
    view().flex_1().min_h(px(0.0)).p_5().gap_4().bg(BACKGROUND)
    .child(heading("Component Gallery"))
    .child(muted("Production components · local fixtures · no AppServer or Agent required"))
    .child(
        scroll().flex_1().child(
            view()
                .gap_4()
                .child(
                    settings_section("Entry and directory field", "Shared production controls; choosing a directory does not create or save anything.")
                        .child(entry_card(Icon::Plus, "Create a Project", "One accessible clickable surface.", move |_| {
                            count.set(count.get_untracked() + 1);
                        }))
                        .child(directory_field(directory, |_| {})),
                )
                .child(
                    panel()
                        .child(label("Shared Lucide icons · muted / selected / attention"))
                        .child(view().flex_row().flex_wrap().gap_2().children(
                            [Icon::Search, Icon::Terminal, Icon::Board, Icon::Overview,
                             Icon::Archive, Icon::Settings, Icon::Projects, Icon::Agent, Icon::GitHub,
                             Icon::ChevronDown, Icon::Plus, Icon::Check]
                            .into_iter().enumerate().map(|(i, kind)|
                                icon_action(kind, format!("{kind:?}"), move || i == 1,
                                    move || if i == 2 { 3 } else { 0 }, |_| {}).into_element()
                            ).collect::<Vec<_>>())),
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
                            action("Disabled action", Variant::Secondary, |_| {}).disabled(true),
                        ),
                )
                .child(
                    panel()
                        .child(label("Buttons · hover / press / keyboard focus"))
                        .child(
                            view()
                                .flex_row()
                                .gap_3()
                                .child(action("Primary", Variant::Primary, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(action("Secondary", Variant::Secondary, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(action("Danger", Variant::Danger, move |_| {
                                    count.set(count.get_untracked() + 1)
                                })),
                        )
                        .child(muted(move || format!("Press count: {}", count.get()))),
                )
                .child(
                    panel()
                        .child(label("Markdown editor / shared reading styles"))
                        .child(markdown.render(input, markdown.source(input, "Gallery Markdown", "Write Markdown…"), false, signal(220.)))
                        .child(canban_ui_kit::markdown::reader(move || input.get())),
                )
                .child(
                    panel()
                        .child(label("Shared product Timeline item"))
                        .child(timeline_item(ui, fixtures::timeline())),
                )
                .child(canban_ui::components::scheduling::settings(ui))
                .child(profiles::registry(profile_fixture))
                .child(review::diff_panel(fixtures::review()))
                .child(
                    panel().child(label("Native libghostty terminal")).child(
                        native("canban/terminal")
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
        ui.snapshot.update(|s| {
            for a in &mut s.attempts {
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
    ui.profiles.set(canban_types::Config::default().profiles);
    ui.machines.set(json!({"machines":[{"id":"gallery-remote","name":"Build machine","state":"connected","target":"ssh-host"},{"id":"gallery-offline","name":"Offline machine","state":"unreachable","target":"another-host"}]}));
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("update-recovery") {
        return canban_ui::updates::recovery_screen(
            "/preview/.canban".into(),
            "Down migration 10 → 9 failed. Current schema remains 10.".into(),
        )
        .into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("updates") {
        ui.card.set(String::new());
        ui.page.set("board".into());
        ui.update.set(json!({"build":canban_updater::Build::current(),"state":{"phase":"available","available":{"manifest":{"version":"20261110-3"}}}}));
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("archive") {
        ui.snapshot.set(fixtures::archive());
        ui.project.set("archive-empty".into());
        ui.card.set(String::new());
        ui.page.set("archive".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("reminders") {
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
                    .child(action("Board", Variant::Quiet, move |_| {
                        ui.navigate("board")
                    }))
                    .child(action("Overview", Variant::Quiet, move |_| {
                        ui.navigate("overview")
                    }))
                    .child(action("Focus window", Variant::Quiet, move |_| {
                        nsh_core::element::desktop::order_front()
                    })),
            )
            .child(canban_ui::frame::screen(ui))
            .into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("project-activity") {
        nsh_core::element::desktop::set_title_bar(
            nsh_core::desktop::TitleBarStyle::Hidden,
            Some(px(42.)),
        );
        ui.snapshot.set(fixtures::project_activity());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("board-overflow") {
        ui.snapshot.set(fixtures::overflowing_board());
        ui.page.set("board".into());
        return canban_ui::frame::screen(ui).into_element();
    }
    if std::env::var("CANBAN_GALLERY_SCENE").as_deref() == Ok("terminals") {
        nsh_core::element::desktop::set_title_bar(
            nsh_core::desktop::TitleBarStyle::Hidden,
            Some(px(42.)),
        );
        ui.snapshot.set(fixtures::terminal_workspace());
        ui.open_terminal("gallery-manual".into());
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
                    scene.set("workspace".into()); ui.snapshot.set(fixtures::conversation());
                    ui.card.set("card-1".into()); ui.page.set("card".into());
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
                    ui.open_terminal(canban_types::Conversation::key("overview-card-2","codex"));
                }))
                .child(action("Inactive Terminals",Variant::Secondary,move |_|{
                    let mut snapshot=fixtures::terminal_workspace();
                    for card in &mut snapshot.cards {card.human_gate=None;card.attention=None;}
                    for r in &mut snapshot.requests {r.state="completed".into();}
                    for a in &mut snapshot.attempts {
                        if !a.finished() {a.observed=Some(json!({"state":"live","status":{"state":"done"}}));a.connection_state="connected".into();a.lifecycle.observed_at=Some(canban_types::now());a.lifecycle.last_interaction_at=None;}
                    }
                    ui.snapshot.set(snapshot);ui.terminal_tabs.set(Vec::new());ui.terminal_selection.set(String::new());
                    ui.terminal_inactive.set(false);ui.navigate("terminals");scene.set("workspace".into());
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
                .child(action("Archived / deleted", Variant::Quiet, move |_| {
                    ui.snapshot.set(fixtures::archive());ui.card.set(String::new());ui.page.set("archive".into());scene.set("workspace".into());
                }))
                .child(action("Card states", Variant::Quiet, move |_| {
                    scene.set("workspace".into());ui.snapshot.set(fixtures::board_cards());ui.card.set(String::new());ui.page.set("board".into());
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
                    scene.set("workspace".into());ui.snapshot.set(snapshot);ui.page.set("new-card".into());
                }))
                .child(action("GitHub inbox",Variant::Quiet,move |_|{scene.set("workspace".into());fixtures::github_browser(ui,false);}))
                .child(action("GitHub PR diff",Variant::Quiet,move |_|{scene.set("workspace".into());fixtures::github_browser(ui,true);}))
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
                .child(action("Work location",Variant::Quiet,move |_|{scene.set("workspace".into());ui.navigate("project-location");}))
                .child(action("Controls", Variant::Quiet, move |_| {
                    scene.set("controls".into())
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
