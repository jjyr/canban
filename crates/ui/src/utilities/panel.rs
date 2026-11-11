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
                        let show_all = signal(false);
                        let catalog = canban_terminal::installed_fonts();
                        let mono_count = catalog.monospaced.len();
                        let other_count = catalog.others.len();
                        let choices = computed(move || {
                            let query = ui.utility_query.get().to_lowercase();
                            let all = show_all.get();
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
                            move || format!("Search {} installed monospaced fonts…", mono_count),
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
                                )),
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
                        crate::commands::CATALOG
                            .iter()
                            .filter(|c| {
                                format!("{} {} {}", c.title, c.context, c.display)
                                    .to_lowercase()
                                    .contains(&query)
                            })
                            .map(|c| SearchChoice {
                                attention: 0,
                                id: c.id.into(),
                                title: c.title.into(),
                                detail: format!("{} · {}", c.display, c.context),
                                icon: Icon::Keyboard,
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
                let title = if popup == "usage" {
                    "Subscription usage"
                } else {
                    "Theme"
                };
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
                                    .max_h(Percentage(0.76))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(BORDER)
                                    .child(
                                        view()
                                            .flex_shrink_0()
                                            .flex_row()
                                            .justify_between()
                                            .items_center()
                                            .child(label(title).font_weight(FontWeight::SEMIBOLD))
                                            .child(chrome_button(
                                                Icon::Close,
                                                "Close panel",
                                                move |_| close(ui),
                                            )),
                                    )
                                    .child(
                                        scroll().flex_1().min_h(px(0.)).overflow_hidden().child(
                                            if popup == "usage" {
                                                usage_panel(ui).into_element()
                                            } else {
                                                theme_panel(ui).into_element()
                                            },
                                        ),
                                    ),
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

fn machine_menu(ui: Ui) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    ui.spawn(async move {
        let mut items = vec![
            MenuItem::new("Follow current context")
                .on_select(move || ui.usage_machine.set(String::new())),
            MenuItem::new("AppServer host").on_select(move || ui.usage_machine.set("local".into())),
        ];
        for m in ui
            .machines
            .with_untracked(|v| v["machines"].as_array().cloned().unwrap_or_default())
        {
            let id = m["id"].as_str().unwrap_or("").to_owned();
            if id.is_empty() {
                continue;
            }
            items.push(
                MenuItem::new(m["name"].as_str().unwrap_or(&id))
                    .on_select(move || ui.usage_machine.set(id.clone())),
            );
        }
        popup_menu(items).await;
    });
}
fn usage_panel(ui: Ui) -> impl IntoElement {
    view()
        .gap_3()
        .child(action(
            move || {
                format!(
                    "Usage machine: {} ▾",
                    crate::machines::name(ui, &machine(ui))
                )
            },
            Variant::Secondary,
            move |_| machine_menu(ui),
        ))
        .child(dynamic(move || {
            let v = ui.usage.get();
            let mut content = view().gap_3();
            if v["busy"] == true {
                content = content.child(muted("Refreshing usage…"));
            }
            if let Some(error) = v["error"].as_str() {
                content = content.child(notice(error.to_owned(), Tone::Attention));
            }
            for r in readings(&v) {
                let provider = r["provider"].as_str().unwrap_or("");
                let mut account = view().gap_2().child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(icon(if provider == "codex" {
                            Icon::Codex
                        } else {
                            Icon::Claude
                        }))
                        .child(label(format!(
                            "{} · {}",
                            if provider == "codex" {
                                "Codex"
                            } else {
                                "Claude"
                            },
                            r["account"].as_str().unwrap_or("Current account")
                        ))),
                );
                if r["stale"] == true {
                    account = account.child(status("Last known reading", Tone::Attention));
                }
                for w in r["windows"].as_array().into_iter().flatten() {
                    let used = w["used"].as_f64().unwrap_or(0.);
                    account = account
                        .child(label(format!(
                            "{} · {used:.0}% used",
                            w["name"].as_str().unwrap_or("Usage")
                        )))
                        .child(
                            view().h(px(4.)).rounded(px(2.)).bg(INSET).child(
                                view()
                                    .h_full()
                                    .w(Percentage((used / 100.) as f32))
                                    .rounded(px(2.))
                                    .bg(if used >= 90. { ATTENTION } else { ACCENT }),
                            ),
                        );
                    if let Some(reset) = w["resets_at"].as_str() {
                        account = account.child(muted(format!("Resets {reset}")));
                    }
                }
                if let Some(error) = r["error"].as_str() {
                    account = account.child(notice(error.to_owned(), Tone::Attention));
                }
                let url = if provider == "codex" {
                    "https://chatgpt.com/codex/settings/usage"
                } else {
                    "https://claude.ai/settings/usage"
                };
                content = content.child(account.child(action(
                    "Open usage page",
                    Variant::Quiet,
                    move |_| report::external(ui, url),
                )));
            }
            if let Some(at) = v["updated_at"].as_u64() {
                content = content.child(muted(format!(
                    "Updated {}",
                    crate::components::activity::age(at)
                )));
            }
            content
        }))
        .child(
            action("Refresh usage", Variant::Secondary, move |_| {
                usage(ui, machine(ui), true)
            })
            .disabled(move || ui.usage_loading.get() || ui.usage.with(|v| v["busy"] == true)),
        )
}
