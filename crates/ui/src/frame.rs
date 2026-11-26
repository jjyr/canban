//! Shared production window frame, including split Terminal chrome and overlays.
use crate::{presenter::*, quick_open, screens, sidebar, terminals};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

pub const CHROME_LEFT_OFFSET: f32 = 10.0;
pub const CHROME_RIGHT_OFFSET: f32 = 10.0;

pub(crate) fn compute_chrome_padding(
    left_controls: f32,
    right_controls: f32,
    macos: bool,
) -> (f32, f32) {
    let left = if macos {
        left_controls.max(78.) + CHROME_LEFT_OFFSET
    } else {
        left_controls + CHROME_LEFT_OFFSET
    };
    let right = if macos {
        CHROME_RIGHT_OFFSET
    } else {
        right_controls + CHROME_RIGHT_OFFSET
    };
    (left, right)
}

pub fn chrome_left_padding() -> f32 {
    let tb = nsh_core::element::desktop::title_bar();
    compute_chrome_padding(tb.left.0, tb.right.0, cfg!(target_os = "macos")).0
}

pub fn chrome_right_padding() -> f32 {
    let tb = nsh_core::element::desktop::title_bar();
    compute_chrome_padding(tb.left.0, tb.right.0, cfg!(target_os = "macos")).1
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

#[derive(Clone, PartialEq, Eq)]
enum ChromeMode {
    Board,
    FullCard,
    Subpage(String),
}

fn route_chrome(ui: Ui) -> impl IntoElement {
    let mode = computed(move || {
        let p = ui.page.get();
        let full_card = p == "card"
            && (ui.card_standalone.get() || ui.narrow.get() || ui.card_terminal_open.get());
        if matches!(p.as_str(), "board" | "new-card" | "terminals") || (p == "card" && !full_card) {
            ChromeMode::Board
        } else if full_card {
            ChromeMode::FullCard
        } else {
            ChromeMode::Subpage(p)
        }
    });
    dynamic(move || {
        let current = mode.get();
        untracked(|| match current {
            ChromeMode::Board => crate::board::chrome_bar(ui).into_element(),
            ChromeMode::FullCard => view()
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
                                let proj = ui
                                    .current_project()
                                    .map(|pr| ui.snapshot.with(|s| s.project_display_name(&pr.id)))
                                    .unwrap_or_default();
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
                .into_element(),
            ChromeMode::Subpage(p) => match p.as_str() {
                "insights" => crate::insights::chrome_bar(ui).into_element(),
                "schedules" => crate::schedules::chrome_bar(ui).into_element(),
                "github" => crate::github::workspace::chrome_bar(ui).into_element(),
                "github-account" => subpage_chrome(
                    ui,
                    Some(("← GitHub", "github")),
                    "GitHub / Account & access",
                )
                .into_element(),
                "github-repository" => {
                    subpage_chrome(ui, Some(("← Board", "board")), move || {
                        ui.current_project()
                            .map(|pr| {
                                let name = ui.snapshot.with(|s| s.project_display_name(&pr.id));
                                format!("{name} / GitHub repository")
                            })
                            .unwrap_or_else(|| "GitHub repository".into())
                    })
                    .into_element()
                }
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
                                        let proj = ui
                                            .snapshot
                                            .with(|s| s.project_display_name(&c.project_id));
                                        if ui.snapshot.with(|s| s.has_remote_hosts()) {
                                            format!("Git · {proj} · #{} {}", c.number, c.title)
                                        } else {
                                            format!("Git · #{} {}", c.number, c.title)
                                        }
                                    } else if let Some(pr) = ui.current_project() {
                                        let proj =
                                            ui.snapshot.with(|s| s.project_display_name(&pr.id));
                                        format!("Git · {proj}")
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
                "archive" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                    ui.current_project()
                        .map(|pr| {
                            let name = ui.snapshot.with(|s| s.project_display_name(&pr.id));
                            format!("{name} / Archive")
                        })
                        .unwrap_or_else(|| "Archive".into())
                })
                .into_element(),
                "drafts" => subpage_chrome(ui, Some(("← Board", "board")), "Drafts").into_element(),
                "new-project" => {
                    subpage_chrome(ui, Some(("← Board", "board")), "New Project").into_element()
                }
                "project-location" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                    ui.current_project()
                        .map(|pr| {
                            let name = ui.snapshot.with(|s| s.project_display_name(&pr.id));
                            format!("{name} / Work location")
                        })
                        .unwrap_or_else(|| "Work location".into())
                })
                .into_element(),
                "project-agents" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                    ui.current_project()
                        .map(|pr| {
                            let name = ui.snapshot.with(|s| s.project_display_name(&pr.id));
                            format!("{name} / Project Agents")
                        })
                        .unwrap_or_else(|| "Project Agents".into())
                })
                .into_element(),
                "agent-setup" => {
                    subpage_chrome(ui, Some(("← Settings", "settings")), "Agent Setup")
                        .into_element()
                }
                "overview" | "agents" => {
                    subpage_chrome(ui, None, "Activity & Overview").into_element()
                }
                _ => view().flex_1().h_full().window_drag(true).into_element(),
            },
        })
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

fn is_workspace_page(page: &str) -> bool {
    matches!(page, "board" | "card" | "new-card")
        || !matches!(
            page,
            "agent-setup"
                | "project-agents"
                | "workflows"
                | "project-location"
                | "new-project"
                | "terminals"
                | "terminal"
                | "overview"
                | "agents"
                | "review"
                | "drafts"
                | "archive"
                | "settings"
                | "insights"
                | "schedules"
                | "github"
                | "github-account"
                | "github-repository"
        )
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let in_terminals = computed(move || matches!(ui.page.get().as_str(), "terminals" | "terminal"));
    let is_workspace = computed(move || is_workspace_page(&ui.page.get()));
    let is_settings = computed(move || ui.page.get() == "settings");
    let shell_mounted = signal(!in_terminals.get_untracked());
    effect(move || {
        if !in_terminals.get() && !shell_mounted.get_untracked() {
            shell_mounted.set(true);
        }
    });
    let terminals_mounted = signal(in_terminals.get_untracked());
    effect(move || {
        if in_terminals.get() && !terminals_mounted.get_untracked() {
            terminals_mounted.set(true);
        }
    });
    let settings_mounted = signal(is_settings.get_untracked());
    effect(move || {
        if is_settings.get() && !settings_mounted.get_untracked() {
            settings_mounted.set(true);
        }
    });
    let workspace_mounted = signal(is_workspace.get_untracked());
    effect(move || {
        if is_workspace.get() && !workspace_mounted.get_untracked() {
            workspace_mounted.set(true);
        }
    });
    let subpage_route = computed(move || {
        let page = ui.page.get();
        if matches!(page.as_str(), "terminals" | "terminal" | "settings")
            || is_workspace_page(&page)
        {
            None
        } else if matches!(
            page.as_str(),
            "agent-setup" | "new-project" | "github-account"
        ) {
            Some((page, String::new(), String::new()))
        } else {
            Some((page, ui.card.get(), ui.project.get()))
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .on_layout(move |b: &Bounds<Points>| {
            let next = (b.size.width.0, b.size.height.0);
            let prev = ui.window_size.get_untracked();
            if (next.0 - prev.0).abs() > 0.5 || (next.1 - prev.1).abs() > 0.5 {
                ui.window_size.set(next);
            }
            ui.narrow.set(b.size.width.0 < 950.);
        })
        .child(when(
            move || terminals_mounted.get(),
            move || {
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .style_when(move || !in_terminals.get(), |s| s.hidden())
                    .accessibility_elements_hidden(move || !in_terminals.get())
                    .pointer_events(move || {
                        if in_terminals.get() {
                            PointerEvents::Auto
                        } else {
                            PointerEvents::None
                        }
                    })
                    .child(terminals::screen(ui))
            },
        ))
        .child(when(
            move || shell_mounted.get(),
            move || {
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .style_when(move || in_terminals.get(), |s| s.hidden())
                    .accessibility_elements_hidden(move || in_terminals.get())
                    .pointer_events(move || {
                        if in_terminals.get() {
                            PointerEvents::None
                        } else {
                            PointerEvents::Auto
                        }
                    })
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
                            .pr(move || px(chrome_right_padding()))
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
                                            .child(when(
                                                move || workspace_mounted.get(),
                                                move || {
                                                    view()
                                                        .flex_1()
                                                        .min_h(px(0.))
                                                        .min_w(px(0.))
                                                        .style_when(
                                                            move || !is_workspace.get(),
                                                            |s| s.hidden(),
                                                        )
                                                        .accessibility_elements_hidden(move || {
                                                            !is_workspace.get()
                                                        })
                                                        .pointer_events(move || {
                                                            if is_workspace.get() {
                                                                PointerEvents::Auto
                                                            } else {
                                                                PointerEvents::None
                                                            }
                                                        })
                                                        .child(crate::workspace::screen(ui))
                                                },
                                            ))
                                            .child(when(
                                                move || settings_mounted.get(),
                                                move || {
                                                    view()
                                                        .flex_1()
                                                        .min_h(px(0.))
                                                        .min_w(px(0.))
                                                        .style_when(
                                                            move || !is_settings.get(),
                                                            |s| s.hidden(),
                                                        )
                                                        .accessibility_elements_hidden(move || {
                                                            !is_settings.get()
                                                        })
                                                        .pointer_events(move || {
                                                            if is_settings.get() {
                                                                PointerEvents::Auto
                                                            } else {
                                                                PointerEvents::None
                                                            }
                                                        })
                                                        .child(crate::settings::screen(ui))
                                                },
                                            ))
                                            .child(dynamic(move || {
                                                let Some((page, _, _)) = subpage_route.get() else {
                                                    return view().into_element();
                                                };
                                                untracked(|| screens::screen(ui, &page))
                                            })),
                                    )
                                    .child(backend_notice(ui))
                                    .child(crate::utilities::bar(ui)),
                            ),
                    )
            },
        ))
        .child({
            let toast_open =
                computed(move || ui.operation_error.get().is_empty() && !ui.toast.get().is_empty());
            let toast_presence = canban_ui_kit::motion::Presence::new(toast_open);
            let toast_text = signal(ui.toast.get_untracked());
            effect(move || {
                let next = ui.toast.get();
                if !next.is_empty() {
                    toast_text.set(next);
                }
            });
            when(
                move || toast_presence.mounted.get(),
                move || {
                    view()
                        .element_ref(&toast_presence.region)
                        .bind_opacity(&toast_presence.opacity)
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
                            canban_ui_kit::label(move || toast_text.get())
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .min_w(px(0.)),
                        )
                },
            )
        })
        .child({
            let err_open = computed(move || !ui.operation_error.get().is_empty());
            let err_presence = canban_ui_kit::motion::Presence::new(err_open);
            let err_text = signal(ui.operation_error.get_untracked());
            effect(move || {
                let next = ui.operation_error.get();
                if !next.is_empty() {
                    err_text.set(next);
                }
            });
            when(
                move || err_presence.mounted.get(),
                move || {
                    view()
                        .element_ref(&err_presence.region)
                        .bind_opacity(&err_presence.opacity)
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
                        .pointer_events(move || {
                            if err_open.get() {
                                PointerEvents::Auto
                            } else {
                                PointerEvents::None
                            }
                        })
                        .child(
                            canban_ui_kit::label(move || err_text.get())
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
            )
        })
        .child(quick_open::overlay(ui))
        .child(crate::utilities::overlay(ui))
        .child(terminals::tabs::rename_dialog(ui))
        .child(terminals::tabs::close_confirm_dialog(ui))
        .child(crate::terminals::worktrees::modals(ui))
        .child(crate::projects::archive_confirm_modal(ui))
        .child(crate::projects::restore_project_modal(ui))
        .child(crate::review::git_ops::render_discard_modal(ui))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_padding_preserves_macos_traffic_lights_and_clears_linux_right_csd_buttons() {
        assert_eq!(compute_chrome_padding(0., 0., true), (88., 10.));
        assert_eq!(compute_chrome_padding(84., 0., true), (94., 10.));
        // GNOME / KDE right-side window buttons: no 78pt phantom left gap, full right inset.
        assert_eq!(compute_chrome_padding(0., 92., false), (10., 102.));
        // Linux left-side window buttons (custom gtk-decoration-layout): left inset grows cleanly.
        assert_eq!(compute_chrome_padding(76., 0., false), (86., 10.));
    }
}
