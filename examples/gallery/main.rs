//! Standalone native component gallery. Never opens a Canban home or runtime.
mod fixtures;
use canban_ui::{components::timeline_item, presenter::Ui, profiles, review};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

fn controls() -> impl IntoElement {
    let ui = Ui::new();
    let input = signal("中文与 English · long text fixture".into());
    let profile_draft = signal(String::new());
    let profile_fixture = signal(canban_types::Config::default().profiles);
    let count = signal(0_u32);
    view().flex_1().min_h(px(0.0)).p_5().gap_4().bg(BACKGROUND)
    .child(heading("Component Gallery"))
    .child(muted("Production components · local fixtures · no AppServer or Agent required"))
    .child(
        view()
            .flex_row()
            .gap_2()
            .child(action("Light appearance", Variant::Secondary, |_| {
                set_color_scheme(Appearance::Light)
            }))
            .child(action("Dark appearance", Variant::Secondary, |_| {
                set_color_scheme(Appearance::Dark)
            })),
    )
    .child(
        scroll().flex_1().child(
            view()
                .gap_4()
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
                        .child(label("Controlled input / IME / Markdown preview"))
                        .child(field(input, "Type here").multiline(true).h(px(110.0)))
                        .child(nsh_base::text_view().markdown(move || Some(input.get().into()))),
                )
                .child(
                    panel()
                        .child(label("Shared product Timeline item"))
                        .child(timeline_item(ui, fixtures::timeline())),
                )
                .child(
                    panel()
                        .child(label("Agent profile mention picker"))
                        .child(profiles::mention_picker(
                            profile_fixture,
                            profile_draft,
                        ))
                        .child(
                            field(profile_draft, "Question with @profile")
                                .multiline(true)
                                .h(px(80.0)),
                        ),
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
    let scene = signal("workspace".to_string());
    let constrained = signal(false);
    let ui = Ui::new();
    canban_ui::quick_open::bindings(ui);
    ui.snapshot.set(fixtures::workspace());
    ui.project.set("gallery-project".into());
    ui.card.set("card-1".into());
    ui.page.set("card".into());
    ui.note.set("note-1".into());
    ui.profiles.set(canban_types::Config::default().profiles);
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
                    ui.page.set("card".into());
                }))
                .child(action("Notes workspace", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.page.set("notes".into());
                }))
                .child(action("Overview", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::overview());
                    ui.page.set("overview".into());
                }))
                .child(action("Quick Open", Variant::Secondary, move |_| {
                    scene.set("workspace".into());
                    ui.snapshot.set(fixtures::search());
                    ui.card.set(String::new());
                    ui.page.set("board".into());
                    ui.show_search(false);
                }))
                .child(action("Scheduling", Variant::Quiet, move |_| {
                    scene.set("scheduling".into());
                }))
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
        .child(canban_ui::quick_open::overlay(ui))
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
                .max_w(px(if narrow_preview { 1000.0 } else { 10000.0 }))
                .min_h(px(0.0))
                .flex_row()
                .child(canban_ui::sidebar(ui))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .on_layout(move |b: &Bounds<Points>| ui.narrow.set(b.size.width.0 < 900.0))
                        .child(dynamic(move || {
                            let page = ui.page.get();
                            let _ = ui.card.get();
                            untracked(|| match page.as_str() {
                                "notes" => canban_ui::notes::screen(ui).into_element(),
                                "overview" => canban_ui::overview::screen(ui).into_element(),
                                "board" | "card" | "terminal" | "review" => {
                                    canban_ui::workspace::screen(ui).into_element()
                                }
                                _ => notice(
                                    "Component preview: use the scene selector above",
                                    Tone::Neutral,
                                )
                                .into_element(),
                            })
                        })),
                )
                .into_element()
        }))
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
            size: (1440, 960),
            ..Default::default()
        },
    );
    std::process::exit(if result == std::process::ExitCode::SUCCESS {
        0
    } else {
        1
    });
}
