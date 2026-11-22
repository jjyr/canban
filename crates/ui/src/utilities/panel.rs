use super::*;
use nsh_core::keymap::When;
pub fn overlay(ui: Ui) -> impl IntoElement {
    when(
        move || !ui.utility_popup.get().is_empty(),
        move || {
            dynamic(move || {
                let popup = ui.utility_popup.get();
                if popup == "shortcuts" || popup == "card-project" || popup == "terminal-font" {
                    if popup == "terminal-font" {
                        ensure_fonts_loaded(ui);
                        let show_all = signal(false);
                        let choices = computed(move || {
                            let query = ui.utility_query.get().to_lowercase();
                            let all = show_all.get();
                            let catalog = ui.installed_fonts.get();
                            let current = ui.utility.with(|v| {
                                v["preferences"]["terminal_font_family"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_owned()
                            });
                            let mut rows = Vec::new();
                            let default_title = "System Mono";
                            let default_detail = "Default system monospaced font";
                            if query.is_empty()
                                || default_title.to_lowercase().contains(&query)
                                || default_detail.to_lowercase().contains(&query)
                            {
                                rows.push(SearchChoice {
                                    attention: 0,
                                    id: "__system__".into(),
                                    title: default_title.into(),
                                    detail: default_detail.into(),
                                    icon: Icon::Terminal,
                                    selected: current.is_empty(),
                                });
                            }
                            if let Some(catalog) = catalog {
                                for fam in &catalog.monospaced {
                                    if query.is_empty() || fam.to_lowercase().contains(&query) {
                                        rows.push(SearchChoice {
                                            attention: 0,
                                            id: fam.clone(),
                                            title: fam.clone(),
                                            detail: "Monospaced · Installed".into(),
                                            icon: Icon::Terminal,
                                            selected: current == *fam,
                                        });
                                    }
                                }
                                if all {
                                    for fam in &catalog.others {
                                        if query.is_empty() || fam.to_lowercase().contains(&query) {
                                            rows.push(SearchChoice {
                                                attention: 0,
                                                id: fam.clone(),
                                                title: fam.clone(),
                                                detail: "Proportional · Installed".into(),
                                                icon: Icon::Note,
                                                selected: current == *fam,
                                            });
                                        }
                                    }
                                }
                            }
                            rows
                        });
                        return search_dialog(
                            move || "Choose Terminal Font".into(),
                            ui.utility_query,
                            choices,
                            move |id| {
                                let payload = if id == "__system__" || id.trim().is_empty() {
                                    json!({"terminal_font_family":null})
                                } else {
                                    json!({"terminal_font_family":id})
                                };
                                configure_preferences(ui, payload);
                                close(ui);
                            },
                            move || close(ui),
                            move || {
                                if let Some(catalog) = ui.installed_fonts.get() {
                                    format!(
                                        "Search {} installed monospaced fonts…",
                                        catalog.monospaced.len()
                                    )
                                } else {
                                    "Scanning installed monospaced fonts…".into()
                                }
                            },
                            dynamic(move || {
                                let Some(catalog) = ui.installed_fonts.get() else {
                                    return loading_indicator(
                                        "Scanning installed system font families…",
                                    )
                                    .into_element();
                                };
                                let mono_count = catalog.monospaced.len();
                                let other_count = catalog.others.len();
                                view()
                                    .flex_row()
                                    .justify_between()
                                    .items_center()
                                    .gap_2()
                                    .child(muted(move || {
                                        if show_all.get() {
                                            format!(
                                                "{} monospaced + {} other installed families",
                                                mono_count, other_count
                                            )
                                        } else {
                                            format!("{} monospaced families installed", mono_count)
                                        }
                                    }))
                                    .child(compact_action(
                                        move || {
                                            if show_all.get() {
                                                "Monospaced only".into()
                                            } else {
                                                format!("Include all fonts ({other_count})")
                                            }
                                        },
                                        Variant::Quiet,
                                        move |_| show_all.update(|v| *v = !*v),
                                    ))
                                    .into_element()
                            }),
                        )
                        .into_element();
                    }
                    let project_mode = popup == "card-project";
                    let choices = computed(move || {
                        let query = ui.utility_query.get().to_lowercase();
                        if project_mode {
                            return ui.snapshot.with(|s| {
                                s.projects
                                    .iter()
                                    .filter(|p| p.name.to_lowercase().contains(&query))
                                    .map(|p| SearchChoice {
                                        attention: 0,
                                        id: p.id.clone(),
                                        title: p.name.clone(),
                                        detail: p.work_location().directory,
                                        icon: Icon::Projects,
                                        selected: ui.card_target.get() == p.id,
                                    })
                                    .collect()
                            });
                        }
                        crate::quick_open::matching_commands(&query)
                            .into_iter()
                            .map(|c| SearchChoice {
                                attention: 0,
                                id: c.id.into(),
                                title: c.title.into(),
                                detail: if c.display.is_empty() {
                                    c.context.into()
                                } else {
                                    format!("{} · {}", c.display, c.context)
                                },
                                icon: if c.id.contains("agent") {
                                    Icon::Agent
                                } else {
                                    Icon::Keyboard
                                },
                                selected: false,
                            })
                            .collect()
                    });
                    return search_dialog(
                        move || {
                            if project_mode {
                                "Choose Card Project".into()
                            } else {
                                "Shortcuts".into()
                            }
                        },
                        ui.utility_query,
                        choices,
                        move |id| {
                            if project_mode {
                                ui.card_target.set(id);
                                close(ui);
                            } else {
                                crate::commands::execute(ui, &id);
                            }
                        },
                        move || close(ui),
                        move || {
                            if project_mode {
                                "Search Projects…".into()
                            } else {
                                "Search actions or shortcuts…".into()
                            }
                        },
                        if project_mode {
                            action("Create a Project…", Variant::Quiet, move |_| {
                                close(ui);
                                ui.navigate("new-project");
                            })
                            .into_element()
                        } else {
                            muted("Select an action to run it · Esc to close").into_element()
                        },
                    )
                    .into_element();
                }
                if popup == "notifications" {
                    return crate::overview::popover(ui).into_element();
                }
                if popup == "hosts" {
                    return crate::hosts::popover(ui).into_element();
                }
                if popup == "connect-host" {
                    return crate::hosts::connect_modal(ui).into_element();
                }
                if popup == "sync" {
                    return crate::hosts::sync_modal(ui).into_element();
                }
                if popup != "theme" {
                    return view().into_element();
                }
                on_key_with(
                    "escape",
                    KeyOptions {
                        when: When::Always,
                        ..Default::default()
                    },
                    move |_| close(ui),
                );
                nsh_base::dialog()
                    .animation(canban_ui_kit::motion::modal)
                    .close_on_escape(true)
                    .close_on_backdrop_press(true)
                    .on_cancel(move || close(ui))
                    .child(
                        view()
                            .flex_1()
                            .justify_end()
                            .items_end()
                            .p_4()
                            .pb(px(44.))
                            .bg(rgba(0x00000033))
                            .child(
                                view()
                                    .absolute()
                                    .inset_0()
                                    .label("Dismiss utility panel")
                                    .on_press(move || close(ui)),
                            )
                            .child(
                                panel()
                                    .w(px(460.))
                                    .max_w(Percentage(0.95))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(BORDER)
                                    .child(
                                        view()
                                            .flex_shrink_0()
                                            .flex_row()
                                            .justify_between()
                                            .items_center()
                                            .child(label("Theme").font_weight(FontWeight::SEMIBOLD))
                                            .child(chrome_button(
                                                Icon::Close,
                                                "Close panel",
                                                move |_| close(ui),
                                            )),
                                    )
                                    .child(scroll().max_h(px(460.)).child(theme_panel(ui))),
                            ),
                    )
                    .into_element()
            })
        },
    )
}
fn theme_panel(ui: Ui) -> impl IntoElement {
    view().gap_3().child(
        view().flex_row().flex_wrap().gap_2().children(
            canban_themes::THEMES
                .iter()
                .map(|theme| {
                    theme_choice(
                        theme,
                        move || {
                            ui.utility.with(|v| {
                                v["preferences"]["theme"]
                                    .as_str()
                                    .unwrap_or(canban_themes::DEFAULT)
                                    == theme.id
                            })
                        },
                        move |hovered| {
                            if hovered && ui.utility_popup.get_untracked() == "theme" {
                                ui.theme_preview.set(Some(theme.id.into()));
                            } else if ui
                                .theme_preview
                                .with_untracked(|v| v.as_deref() == Some(theme.id))
                            {
                                // A late leave from the previous tile cannot clear the next preview.
                                ui.theme_preview.set(None);
                            }
                        },
                        move || ui.is_method_pending("utilities.configure"),
                        move |_| configure(ui, theme.id.into()),
                    )
                    .into_element()
                })
                .collect::<Vec<_>>(),
        ),
    )
}
