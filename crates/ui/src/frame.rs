//! Shared production window frame, including split Terminal chrome and overlays.
use crate::{presenter::*, quick_open, screens, sidebar, terminals};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

pub const CHROME_LEFT_OFFSET: f32 = 10.0;

pub fn chrome_left_padding() -> f32 {
    nsh_core::element::desktop::title_bar().left.0.max(78.) + CHROME_LEFT_OFFSET
}

/// Keep production and Gallery custom chrome on the same native window API.
pub fn configure_window_chrome() {
    let mut title_bar = nsh_core::desktop::TitleBarOptions::default();
    title_bar.style = nsh_core::desktop::TitleBarStyle::Hidden;
    title_bar.height = Some(px(42.));
    let mut attributes = nsh_core::desktop::WindowAttributes::default();
    attributes.title_bar = Some(title_bar);
    nsh_core::element::desktop::current_window().set(attributes);
}

fn subpage_chrome(
    ui: Ui,
    back_label: Option<(&'static str, &'static str)>,
    title: impl IntoReactive<SharedString> + 'static,
) -> View {
    let mut left = view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .window_drag(false);
    if let Some((label_text, dest)) = back_label {
        left = left.child(compact_action(label_text, Variant::Quiet, move |_| {
            ui.navigate(dest)
        }));
    }
    left = left.child(
        label(title)
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(13.5))
            .truncate()
            .min_w(px(0.)),
    );
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_2()
        .pl_2()
        .window_drag(true)
        .child(left)
        .child(view().flex_1().h_full().window_drag(true))
}

fn route_chrome(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let p = ui.page.get();
        let full_card = p == "card"
            && (ui.card_standalone.get() || ui.narrow.get() || ui.card_terminal_open.get());
        if p == "board" || (p == "card" && !full_card) || p == "new-card" {
            return crate::board::chrome_bar(ui).into_element();
        }
        if full_card {
            return view()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .flex_row()
                .items_center()
                .gap_1p5()
                .pl_2()
                .window_drag(true)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .min_w(px(0.))
                        .window_drag(false)
                        .child(compact_action("← Board", Variant::Quiet, move |_| {
                            ui.close_card();
                        }))
                        .child(
                            label(move || {
                                let proj =
                                    ui.current_project().map(|pr| pr.name).unwrap_or_default();
                                if let Some(c) = ui.current_card() {
                                    format!("{proj} / Card #{} · {}", c.number, c.title)
                                } else {
                                    proj
                                }
                            })
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.5))
                            .truncate()
                            .min_w(px(0.)),
                        ),
                )
                .child(view().flex_1().h_full().window_drag(true))
                .into_element();
        }
        match p.as_str() {
            "github" => crate::github::workspace::chrome_bar(ui).into_element(),
            "github-account" => subpage_chrome(
                ui,
                Some(("← GitHub", "github")),
                "GitHub / Account & access",
            )
            .into_element(),
            "github-repository" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                ui.current_project()
                    .map(|pr| format!("{} / GitHub repository", pr.name))
                    .unwrap_or_else(|| "GitHub repository".into())
            })
            .into_element(),
            "workflows" => crate::workflows::chrome_bar(ui).into_element(),
            "review" => view()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .flex_row()
                .items_center()
                .gap_1p5()
                .pl_2()
                .window_drag(true)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .min_w(px(0.))
                        .window_drag(false)
                        .child(icon(Icon::Compare).w(px(14.)).h(px(14.)))
                        .child(
                            label(move || {
                                if let Some(c) = ui.current_card() {
                                    format!("Git · #{} {}", c.number, c.title)
                                } else if let Some(pr) = ui.current_project() {
                                    format!("Git · {}", pr.name)
                                } else {
                                    "Git".into()
                                }
                            })
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.5))
                            .truncate()
                            .min_w(px(0.)),
                        ),
                )
                .child(view().flex_1().h_full().window_drag(true))
                .into_element(),
            "settings" => subpage_chrome(ui, None, "Settings").into_element(),
            "machines" => {
                subpage_chrome(ui, Some(("← Settings", "settings")), "Settings / Machines")
                    .into_element()
            }
            "archive" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                ui.current_project()
                    .map(|pr| format!("{} / Archive", pr.name))
                    .unwrap_or_else(|| "Archive".into())
            })
            .into_element(),
            "drafts" => subpage_chrome(ui, Some(("← Board", "board")), "Drafts").into_element(),
            "new-project" => {
                subpage_chrome(ui, Some(("← Board", "board")), "New Project").into_element()
            }
            "project-location" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                ui.current_project()
                    .map(|pr| format!("{} / Work location", pr.name))
                    .unwrap_or_else(|| "Work location".into())
            })
            .into_element(),
            "project-agents" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                ui.current_project()
                    .map(|pr| format!("{} / Project Agents", pr.name))
                    .unwrap_or_else(|| "Project Agents".into())
            })
            .into_element(),
            "agent-setup" => {
                subpage_chrome(ui, Some(("← Settings", "settings")), "Agent Setup").into_element()
            }
            "overview" | "agents" => subpage_chrome(ui, None, "Activity & Overview").into_element(),
            _ => view().flex_1().h_full().window_drag(true).into_element(),
        }
    })
}

pub fn backend_notice(ui: Ui) -> impl IntoElement {
    when(
        move || !ui.backend_notice.get().is_empty(),
        move || {
            canban_ui_kit::notice(
                move || ui.backend_notice.get(),
                canban_ui_kit::Tone::Attention,
            )
        },
    )
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let route = computed(move || {
        let page = ui.page.get();
        if matches!(page.as_str(), "board" | "card" | "new-card") {
            ("workspace".to_owned(), String::new(), ui.project.get())
        } else {
            (page, ui.card.get(), ui.project.get())
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .on_layout(move |b: &Bounds<Points>| ui.narrow.set(b.size.width.0 < 950.))
        .child(dynamic(move || {
            let (page, _, _) = route.get();
            untracked(|| {
                if page == "terminals" {
                    terminals::screen(ui).into_element()
                } else {
                    view()
                        .flex_1()
                        .min_h(px(0.))
                        .child(
                            view()
                                .h(px(42.))
                                .flex_shrink_0()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .bg(canban_ui_kit::tokens::SURFACE)
                                .border_b_1()
                                .border_color(canban_ui_kit::tokens::BORDER)
                                .window_drag(true)
                                .pl(move || px(chrome_left_padding()))
                                .pr(px(10.))
                                .child(crate::navigation::controls(ui))
                                .child(route_chrome(ui))
                                .child(crate::hosts::trigger_pill(ui))
                                .child(crate::overview::trigger_pill(ui)),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_h(px(0.))
                                .flex_row()
                                .child(sidebar(ui))
                                .child(
                                    view()
                                        .flex_1()
                                        .min_h(px(0.))
                                        .min_w(px(0.))
                                        .child(
                                            view()
                                                .flex_1()
                                                .min_h(px(0.))
                                                .min_w(px(0.))
                                                .child(screens::screen(ui, &page)),
                                        )
                                        .child(backend_notice(ui))
                                        .child(crate::utilities::bar(ui)),
                                ),
                        )
                        .into_element()
                }
            })
        }))
        .child(when(
            move || ui.operation_error.get().is_empty() && !ui.toast.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(44.))
                    .right(px(16.))
                    .max_w(px(420.))
                    .px_3()
                    .py_2()
                    .bg(canban_ui_kit::tokens::SURFACE)
                    .border_1()
                    .border_color(canban_ui_kit::tokens::BORDER)
                    .rounded(px(8.))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .pointer_events(PointerEvents::None)
                    .child(
                        canban_ui_kit::icon_tinted(
                            canban_ui_kit::Icon::Check,
                            canban_ui_kit::tokens::ACCENT,
                        )
                        .w(px(15.))
                        .h(px(15.)),
                    )
                    .child(
                        canban_ui_kit::label(move || ui.toast.get())
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .min_w(px(0.)),
                    )
            },
        ))
        .child(when(
            move || !ui.operation_error.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(44.))
                    .right(px(16.))
                    .max_w(px(520.))
                    .px_3()
                    .py_2()
                    .bg(canban_ui_kit::tokens::ATTENTION_SURFACE)
                    .border_1()
                    .border_color(canban_ui_kit::tokens::ATTENTION)
                    .rounded(px(8.))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        canban_ui_kit::label(move || ui.operation_error.get())
                            .text_color(canban_ui_kit::tokens::ATTENTION)
                            .flex_1()
                            .min_w(px(0.)),
                    )
                    .child(canban_ui_kit::chrome_button(
                        canban_ui_kit::Icon::Close,
                        "Dismiss message",
                        move |_| ui.operation_error.set(String::new()),
                    ))
            },
        ))
        .child(quick_open::overlay(ui))
        .child(crate::utilities::overlay(ui))
        .child(terminals::tabs::rename_dialog(ui))
        .child(terminals::tabs::close_confirm_dialog(ui))
        .child(crate::terminals::worktrees::modals(ui))
        .child(crate::review::git_ops::render_discard_modal(ui))
}
