//! Shared bottom utility bar and overlays, outside workspace navigation.
pub mod panel;
pub(crate) mod report;
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};
pub fn close(ui: Ui) {
    ui.utility_popup.set(String::new());
    ui.terminal_focus.update(|n| *n += 1);
}
pub fn open(ui: Ui, name: &str) {
    ui.quick_open.set(false);
    ui.agent_popover.close();
    ui.utility_query.set(String::new());
    let opening_usage = ui.utility_popup.get_untracked() != "usage" && name == "usage";
    ui.utility_popup
        .set(if ui.utility_popup.get_untracked() == name {
            String::new()
        } else {
            name.into()
        });
    if opening_usage {
        usage(ui, untracked(|| host(ui)), false);
    }
    ui.terminal_focus.update(|n| *n += 1);
}
pub fn host(ui: Ui) -> String {
    let pinned = ui.usage_host.get();
    if !pinned.is_empty() {
        return pinned;
    }
    if ui.page.get() == "terminals"
        && let Some(e) = crate::terminals::current(ui)
    {
        return e.host_id;
    }
    ui.active_project_host()
}
pub fn init(ui: Ui) {
    ensure_fonts_loaded(ui);
    refresh(ui);
    let target = computed(move || host(ui));
    effect(move || {
        let host_id = target.get();
        untracked(|| usage(ui, host_id, false));
    });
    let usage_minutes = stored(0_u32);
    set_interval(std::time::Duration::from_secs(60), move || {
        ui.utility_clock.set(canban_types::now() / 1000);
        crate::activity::tick(ui);
        let next = usage_minutes.get() + 1;
        if next >= 10 {
            usage_minutes.replace(0);
            usage(ui, untracked(|| host(ui)), false);
        } else {
            usage_minutes.replace(next);
        }
    });
    effect(move || {
        let v = ui.utility.get();
        let saved = v["preferences"]["theme"]
            .as_str()
            .unwrap_or(canban_themes::DEFAULT);
        let preview_allowed = ui.utility_popup.get() == "theme"
            || (ui.page.get() == "settings"
                && crate::settings::SectionId::resolve(&ui.settings_section.get())
                    == crate::settings::SectionId::Appearance);
        if preview_allowed {
            let preview = ui.theme_preview.get();
            set_theme(preview.as_deref().unwrap_or(saved));
        } else {
            // Every dismissal/navigation path discards temporary preview state.
            ui.theme_preview.set(None);
            set_theme(saved);
        }
    });
}
pub fn should_apply_utilities_push(
    pending_methods: &std::collections::BTreeSet<&'static str>,
) -> bool {
    !pending_methods.contains("utilities.configure") && !pending_methods.contains("utilities.awake")
}

pub fn should_apply_usage_push(active_host: &str, event_host: &str) -> bool {
    active_host == event_host
}

pub(crate) fn apply_push(ui: Ui, v: Value) {
    if ui
        .pending_methods
        .try_get()
        .is_some_and(|s| !should_apply_utilities_push(&s))
    {
        return;
    }
    if ui.utility.try_get().is_some_and(|cur| cur != v) {
        let _ = ui.utility.try_set(v);
    }
}
pub(crate) fn apply_usage_push(ui: Ui, host_id: &str, mut v: Value) {
    if ui.snapshot_ready.try_get().is_none() {
        return;
    }
    if !should_apply_usage_push(&untracked(|| host(ui)), host_id) {
        return;
    }
    v["host_id"] = json!(host_id);
    let _ = ui.usage_loading.try_set(false);
    if ui.usage.try_get().is_some_and(|cur| cur != v) {
        let _ = ui.usage.try_set(v);
    }
}
pub(crate) fn refresh(ui: Ui) {
    let Some(backend) = crate::backend::get() else {
        return;
    };
    if ui.utility_loading.get_untracked() {
        return;
    }
    ui.utility_loading.set(true);
    let epoch = ui.utility_epoch.get_untracked();
    ui.spawn(async move {
        let r = blocking(move || backend.call_local("utilities.state", json!({}))).await;
        let _ = ui.utility_loading.try_set(false);
        if ui.utility_epoch.try_get() != Some(epoch) {
            return;
        }
        if let Ok(Ok(v)) = r {
            apply_push(ui, v);
        }
    });
}
pub const DEFAULT_TERMINAL_FONT_SIZE: u16 = 13;
pub const MIN_TERMINAL_FONT_SIZE: u16 = 9;
pub const MAX_TERMINAL_FONT_SIZE: u16 = 32;

fn apply_preferences_patch(state: &mut Value, patch: &Value) {
    if let Some(obj) = patch.as_object() {
        for (k, val) in obj {
            if val.is_null() {
                if let Some(prefs) = state["preferences"].as_object_mut() {
                    prefs.remove(k);
                }
            } else {
                state["preferences"][k] = val.clone();
            }
        }
    }
}

pub fn configure_preferences(ui: Ui, patch: Value) {
    if !crate::backend::is_connected() {
        ui.utility.update(|v| apply_preferences_patch(v, &patch));
        return;
    }
    if ui.is_method_pending("utilities.configure") {
        return;
    }
    let prev = ui.utility.get_untracked();
    ui.utility_epoch.update(|n| *n += 1);
    ui.utility.update(|v| apply_preferences_patch(v, &patch));
    dispatch_complete(ui, "utilities.configure", patch, move |r| {
        if let Ok(v) = r {
            ui.utility_epoch.update(|n| *n += 1);
            ui.utility.set(v);
        } else {
            ui.utility.set(prev);
            ui.theme_preview.set(None);
        }
    });
}

pub fn configure(ui: Ui, theme: String) {
    configure_preferences(ui, json!({"theme":theme}));
}

pub fn terminal_font(ui: Ui) -> Value {
    ui.utility.with(|v| {
        let family = v["preferences"]["terminal_font_family"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let size = v["preferences"]["terminal_font_size"]
            .as_u64()
            .map(|n| (n as u16).clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE))
            .unwrap_or(DEFAULT_TERMINAL_FONT_SIZE);
        json!({"family":family,"size":size})
    })
}

/// Pre-warms the installed terminal font catalog on a background thread without blocking UI frames.
pub fn ensure_fonts_loaded(ui: Ui) {
    if ui.installed_fonts.with_untracked(Option::is_some) {
        return;
    }
    if let Some(cached) = canban_terminal::try_installed_fonts() {
        ui.installed_fonts.set(Some(cached));
        return;
    }
    if ui.installed_fonts_loading.get_untracked() {
        return;
    }
    ui.installed_fonts_loading.set(true);
    ui.spawn(async move {
        let res = blocking(canban_terminal::installed_fonts).await;
        let _ = ui.installed_fonts_loading.try_set(false);
        if let Ok(catalog) = res {
            let _ = ui.installed_fonts.try_set(Some(catalog));
        }
    });
}

pub(crate) fn terminal_appearance_group(ui: Ui) -> impl IntoElement {
    ensure_fonts_loaded(ui);
    let adjust_size = move |delta: i32| {
        let current = ui.utility.with_untracked(|v| {
            v["preferences"]["terminal_font_size"]
                .as_u64()
                .map(|n| n as i32)
                .unwrap_or(i32::from(DEFAULT_TERMINAL_FONT_SIZE))
        });
        let next = (current + delta).clamp(
            i32::from(MIN_TERMINAL_FONT_SIZE),
            i32::from(MAX_TERMINAL_FONT_SIZE),
        ) as u16;
        let payload = if next == DEFAULT_TERMINAL_FONT_SIZE {
            json!({"terminal_font_size":null})
        } else {
            json!({"terminal_font_size":next})
        };
        configure_preferences(ui, payload);
    };

    let font_family_row = view()
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
                .child(label("Font family").font_weight(FontWeight::MEDIUM))
                .child(dynamic(move || {
                    if let Some(catalog) = ui.installed_fonts.get() {
                        muted(format!(
                            "Choose from {} installed monospaced families {} (or System Mono).",
                            catalog.monospaced.len(),
                            crate::platform::ON_THIS_MACHINE
                        ))
                        .text_size(px(CAPTION_SIZE))
                        .into_element()
                    } else {
                        loading_indicator("Scanning installed monospaced font families…")
                            .into_element()
                    }
                })),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .child(
                    compact_action(
                        move || {
                            let current = ui.utility.with(|v| {
                                v["preferences"]["terminal_font_family"]
                                    .as_str()
                                    .unwrap_or("")
                                    .trim()
                                    .to_owned()
                            });
                            if current.is_empty() {
                                "System Mono ▾".into()
                            } else {
                                format!("{current} ▾")
                            }
                        },
                        Variant::Secondary,
                        move |_| open(ui, "terminal-font"),
                    )
                    .accessibility_label("Choose terminal font family")
                    .disabled(move || ui.is_method_pending("utilities.configure")),
                )
                .child(when(
                    move || {
                        ui.utility.with(|v| {
                            v["preferences"]["terminal_font_family"]
                                .as_str()
                                .is_some_and(|s| !s.trim().is_empty())
                        })
                    },
                    move || {
                        compact_action("Use System Mono", Variant::Quiet, move |_| {
                            configure_preferences(ui, json!({"terminal_font_family":null}));
                        })
                        .disabled(move || ui.is_method_pending("utilities.configure"))
                    },
                )),
        );

    let size_stepper = view()
        .flex_row()
        .items_center()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(INSET)
        .overflow_hidden()
        .child(
            press_target()
                .w(px(28.))
                .h(px(30.))
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(BORDER)
                .hover(|s| s.bg(SURFACE))
                .active(|s| s.opacity(0.6))
                .focus(|s| s.bg(SELECTED))
                .disabled(move || {
                    ui.is_method_pending("utilities.configure")
                        || ui.utility.with(|v| {
                            v["preferences"]["terminal_font_size"]
                                .as_u64()
                                .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE))
                                <= u64::from(MIN_TERMINAL_FONT_SIZE)
                        })
                })
                .disabled_style(|s| s.opacity(0.4))
                .accessibility_label("Decrease terminal font size")
                .child(
                    label("−")
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM),
                )
                .on_press(move |_: &()| adjust_size(-1)),
        )
        .child(
            view()
                .px_2p5()
                .h(px(30.))
                .items_center()
                .justify_center()
                .bg(SURFACE)
                .child(
                    label(move || {
                        let sz = ui.utility.with(|v| {
                            v["preferences"]["terminal_font_size"]
                                .as_u64()
                                .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE))
                        });
                        format!("{sz} pt")
                    })
                    .font_family(MONO_FONT)
                    .text_size(px(CAPTION_SIZE)),
                ),
        )
        .child(
            press_target()
                .w(px(28.))
                .h(px(30.))
                .items_center()
                .justify_center()
                .border_l_1()
                .border_color(BORDER)
                .hover(|s| s.bg(SURFACE))
                .active(|s| s.opacity(0.6))
                .focus(|s| s.bg(SELECTED))
                .disabled(move || {
                    ui.is_method_pending("utilities.configure")
                        || ui.utility.with(|v| {
                            v["preferences"]["terminal_font_size"]
                                .as_u64()
                                .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE))
                                >= u64::from(MAX_TERMINAL_FONT_SIZE)
                        })
                })
                .disabled_style(|s| s.opacity(0.4))
                .accessibility_label("Increase terminal font size")
                .child(
                    label("+")
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM),
                )
                .on_press(move |_: &()| adjust_size(1)),
        );

    let font_size_row = settings_row(
        "Font size",
        "Applies live to open Terminals (9–32 pt).",
        view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(size_stepper)
            .child(
                compact_action("Reset", Variant::Quiet, move |_| {
                    configure_preferences(
                        ui,
                        json!({"terminal_font_family":null,"terminal_font_size":null}),
                    );
                })
                .disabled(move || {
                    ui.is_method_pending("utilities.configure")
                        || ui.utility.with(|v| {
                            v["preferences"]["terminal_font_family"].is_null()
                                && v["preferences"]["terminal_font_size"].is_null()
                        })
                }),
            ),
    );

    settings_group()
        .title("Appearance")
        .trailing(badge(move || {
            ui.utility.with(|v| {
                let fam = v["preferences"]["terminal_font_family"]
                    .as_str()
                    .unwrap_or("")
                    .trim();
                let sz = v["preferences"]["terminal_font_size"]
                    .as_u64()
                    .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE));
                let name = if fam.is_empty() { "System Mono" } else { fam };
                format!("{name} · {sz} pt")
            })
        }))
        .row(font_family_row)
        .row(font_size_row)
}

pub fn terminal_settings(ui: Ui) -> impl IntoElement {
    settings_section(
        "Terminal",
        "Font family and size apply immediately to open Terminals without reconnecting.",
    )
    .child(terminal_appearance_group(ui))
}
pub fn usage(ui: Ui, host_id: String, force: bool) {
    if !crate::backend::is_connected() {
        if force {
            ui.notify("Subscription usage refreshed");
        }
        return;
    }
    if !force
        && ui.usage_loading.get_untracked()
        && ui.usage.with_untracked(|v| v["host_id"] == host_id)
    {
        return;
    }
    if ui.usage.with_untracked(|v| v["host_id"] != host_id) {
        ui.usage.set(json!({"host_id":host_id,"readings":[]}));
    }
    if force {
        ui.notify("Refreshing subscription usage…");
    }
    ui.usage_loading.set(true);
    ui.usage_epoch.update(|n| *n += 1);
    let epoch = ui.usage_epoch.get_untracked();
    let req_host = host_id.clone();
    ui.spawn(async move {
        let r = blocking(move || {
            crate::routed_call(
                if force {
                    "usage.refresh"
                } else {
                    "usage.status"
                },
                json!({"host_id":req_host}),
                None,
            )
        })
        .await;
        if ui.usage_epoch.get_untracked() != epoch {
            return;
        }
        ui.usage_loading.set(false);
        match r {
            Ok(Ok(mut v)) => {
                v["host_id"] = json!(host_id);
                if v != ui.usage.get_untracked() {
                    ui.usage.set(v);
                }
                if force {
                    ui.notify("Subscription usage refreshed");
                }
            }
            Ok(Err(e)) if force => ui.operation_error.set(format!("{e:#}")),
            Err(e) if force => ui.operation_error.set(e.to_string()),
            _ => {}
        }
    });
}
pub fn readings(v: &Value) -> Vec<Value> {
    v["readings"].as_array().cloned().unwrap_or_default()
}
pub fn toggle_awake(ui: Ui) {
    if ui.is_method_pending("utilities.awake") {
        return;
    }
    let on = ui.utility.with_untracked(|v| v["awake"] == true);
    if !crate::backend::is_connected() {
        ui.utility.update(|v| v["awake"] = json!(!on));
        return;
    }
    let prev = ui.utility.get_untracked();
    ui.utility_epoch.update(|n| *n += 1);
    ui.utility.update(|v| v["awake"] = json!(!on));
    dispatch_complete(ui, "utilities.awake", json!({"enabled":!on}), move |r| {
        if let Ok(v) = r {
            ui.utility_epoch.update(|n| *n += 1);
            ui.utility.set(v);
        } else {
            ui.utility.set(prev);
        }
    });
}
pub fn bar(ui: Ui) -> impl IntoElement {
    view()
        .h(px(32.))
        .flex_shrink_0()
        .min_w(px(0.))
        .border_t_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .bg(SURFACE)
        .child(dynamic(move || usage_summary(ui, ui.utility_clock.get())))
        .child(view().flex_1().min_w(px(0.)))
        .child(utility_button(
            Icon::Coffee,
            "Keep AppServer host awake · display may sleep",
            move || ui.utility.with(|v| v["awake"] == true),
            move || ui.is_method_pending("utilities.awake"),
            move |_| toggle_awake(ui),
        ))
        .child(utility_button(
            Icon::Theme,
            "Theme",
            move || ui.utility_popup.get() == "theme",
            || false,
            move |_| open(ui, "theme"),
        ))
        .child(utility_button(
            Icon::Keyboard,
            format!(
                "Commands & Shortcuts · / or {}",
                crate::commands::shortcuts::COMMANDS_DISPLAY
            ),
            move || {
                ui.quick_open.get() && crate::quick_open::is_command_query(&ui.search_query.get())
            },
            || false,
            move |_| crate::commands::execute(ui, "shortcuts"),
        ))
        .child(utility_button(
            Icon::Bug,
            "Report issue",
            || false,
            || false,
            move |_| report::open(ui),
        ))
}
fn usage_summary(ui: Ui, now: u64) -> View {
    let v = ui.usage.get();
    let busy = v["busy"] == true;
    let rows = readings(&v);
    if rows.is_empty() {
        return view();
    }
    let mut area = view()
        .flex_row()
        .min_w(px(0.))
        .items_center()
        .gap_1()
        .overflow_hidden();
    for provider in ["codex", "claude"] {
        let values: Vec<_> = rows.iter().filter(|r| r["provider"] == provider).collect();
        if values.is_empty() {
            continue;
        }
        let provider_name = if provider == "codex" {
            "Codex"
        } else {
            "Claude"
        };
        let account = values.iter().find_map(|r| {
            r["account"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        });
        let all_windows: Vec<_> = values
            .iter()
            .flat_map(|r| r["windows"].as_array().into_iter().flatten())
            .filter(|w| w["used"].as_f64().is_some())
            .collect();
        let window = all_windows.iter().copied().max_by(|a, b| {
            a["used"]
                .as_f64()
                .unwrap()
                .total_cmp(&b["used"].as_f64().unwrap())
        });
        let stale = values
            .iter()
            .any(|r| r["stale"] == true || r["error"].is_string());
        let text = window
            .map(|w| window_summary(w, now, stale))
            .unwrap_or_else(|| "Unavailable".into());

        let header = match account {
            Some(acc) => format!("{provider_name} · {acc}"),
            None => format!("{provider_name} subscription"),
        };
        let mut tip = view()
            .gap_1()
            .child(label(header).font_weight(FontWeight::SEMIBOLD));
        if all_windows.is_empty() {
            tip = tip.child(muted("Subscription usage unavailable"));
        } else {
            for w in &all_windows {
                let win_name = w["name"].as_str().unwrap_or("Quota");
                let summary = window_summary(w, now, false);
                tip = tip.child(label(format!("{win_name}: {summary}")));
            }
        }
        if let Some(err) = values.iter().find_map(|r| r["error"].as_str()) {
            tip = tip.child(muted(err.to_owned()));
        }
        tip = tip.child(muted(if busy {
            "Refreshing subscription usage…"
        } else {
            "Click to refresh usage"
        }));

        area = area.child(
            view()
                .tooltip(tooltip_box().placement(Placement::Top).child(tip))
                .child(
                    canban_ui_kit::press_target()
                        .min_h(px(28.))
                        .px_2()
                        .rounded(px(5.))
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .bg(CLEAR)
                        .hover(|s| s.bg(INSET))
                        .focus(|s| s.border_1().border_color(ACCENT))
                        .accessibility_label(move || {
                            Some(format!("Refresh {provider_name} subscription usage").into())
                        })
                        .child(
                            icon(if provider == "codex" {
                                Icon::Codex
                            } else {
                                Icon::Claude
                            })
                            .w(px(14.))
                            .h(px(14.)),
                        )
                        .child(muted(text))
                        .on_press(move |_: &()| usage(ui, host(ui), true)),
                ),
        );
    }
    area
}
/// Compact reset countdown for the exact quota window shown beside it.
fn window_summary(window: &Value, now: u64, stale: bool) -> String {
    let mut text = format!("{:.0}%", window["used"].as_f64().unwrap_or(0.));
    if let Some(reset) = window["resets_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    {
        let seconds = reset.timestamp().saturating_sub(now as i64).max(0) as u64;
        let remaining = if seconds == 0 {
            "due".into()
        } else if seconds >= 86400 {
            format!("{}d", seconds.div_ceil(86400))
        } else if seconds >= 3600 {
            format!("{}h", seconds.div_ceil(3600))
        } else {
            format!("{}m", seconds.div_ceil(60))
        };
        text.push_str(&format!(" · {remaining}"));
    }
    if stale {
        text.push_str(" · stale");
    }
    text
}
pub fn overlay(ui: Ui) -> impl IntoElement {
    panel::overlay(ui)
}
pub(crate) fn theme_palette_grid(
    ui: Ui,
    width: impl Into<Length>,
    allow_preview: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    let width = width.into();
    view().flex_row().flex_wrap().gap_2().children(
        canban_themes::THEMES
            .iter()
            .map(|theme| {
                theme_choice_card(
                    theme,
                    width,
                    move || {
                        ui.utility.with(|v| {
                            v["preferences"]["theme"]
                                .as_str()
                                .unwrap_or(canban_themes::DEFAULT)
                                == theme.id
                        })
                    },
                    move |hovered| {
                        if hovered && allow_preview() {
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
    )
}

pub fn settings(ui: Ui) -> impl IntoElement {
    let sounds = signal(crate::reminders::sound_enabled(ui));
    effect(move || sounds.set(crate::reminders::sound_enabled(ui)));
    settings_section(
        "Appearance & General",
        "Theme palette, notification sounds, shortcuts, and support.",
    )
    .trailing(badge(move || {
        ui.utility.with(|v| {
            canban_themes::resolve(
                v["preferences"]["theme"]
                    .as_str()
                    .unwrap_or(canban_themes::DEFAULT),
            )
            .name
            .to_owned()
        })
    }))
    .child(
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
                        label("Theme")
                            .text_size(px(BODY_SIZE))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(muted("Hover to preview, click to apply").text_size(px(CAPTION_SIZE))),
            )
            .child(dynamic(move || {
                let w = if ui.narrow.get() {
                    Percentage(0.48)
                } else {
                    Percentage(0.32)
                };
                theme_palette_grid(ui, w, move || {
                    ui.page.get_untracked() == "settings"
                        && crate::settings::SectionId::resolve(&ui.settings_section.get_untracked())
                            == crate::settings::SectionId::Appearance
                })
                .into_element()
            })),
    )
    .child(
        settings_group()
            .title("Preferences & Support")
            .row(settings_row(
                "Notification sounds",
                "Play an audible alert when an Agent needs attention or completes a step.",
                switch(
                    "Notification sounds",
                    move || sounds.get(),
                    move |enabled| {
                        sounds.set(enabled);
                        configure_preferences(ui, json!({"notification_sound": enabled}));
                    },
                )
                .disabled(move || ui.is_method_pending("utilities.configure")),
            ))
            .row(settings_row(
                "Commands & Shortcuts",
                "Search keyboard shortcuts and invoke any workspace action.",
                compact_action(
                    format!(
                        "Shortcuts · {}",
                        crate::commands::shortcuts::COMMANDS_DISPLAY
                    ),
                    Variant::Secondary,
                    move |_| crate::commands::execute(ui, "shortcuts"),
                ),
            ))
            .row(settings_row(
                "Support & Feedback",
                "File a bug report or copy build diagnostics to clipboard.",
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_1p5()
                    .child(compact_action(
                        "Report issue",
                        Variant::Secondary,
                        move |_| report::open(ui),
                    ))
                    .child(compact_action(
                        "Copy diagnostics",
                        Variant::Quiet,
                        move |_| copy_text(ui, report::diagnostics()),
                    )),
            )),
    )
}

/// Exact terminal theme background as an `AdaptiveColor` for seamless placeholder/viewer mounts.
pub fn terminal_bg_color() -> AdaptiveColor {
    let color = rgb(current_theme().background).to_hsla();
    AdaptiveColor {
        light: color,
        dark: color,
    }
}

/// Use the exact Terminal palette; App chrome derives separate semantic colors.
pub fn terminal_theme() -> Value {
    let theme = current_theme();
    let rgb = |value: u32| [(value >> 16) as u8, (value >> 8) as u8, value as u8];
    json!({"foreground":rgb(theme.foreground),"background":rgb(theme.background),
        "cursor":rgb(theme.cursor),"ansi":theme.ansi.map(rgb)})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_countdown_tracks_its_window_and_handles_subday_and_invalid_resets() {
        let mut window = json!({"used":22,"resets_at":"2026-11-16T00:00:00Z"});
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-09T00:00:00Z")
            .unwrap()
            .timestamp() as u64;
        assert_eq!(window_summary(&window, now, false), "22% · 7d");
        assert_eq!(window_summary(&window, now + 2 * 86400, false), "22% · 5d");
        assert_eq!(
            window_summary(&window, now + 7 * 86400 - 3600, false),
            "22% · 1h"
        );
        assert_eq!(
            window_summary(&window, now + 7 * 86400, true),
            "22% · due · stale"
        );
        window["resets_at"] = json!("invalid");
        assert_eq!(window_summary(&window, now, false), "22%");
    }

    #[test]
    fn preferences_patch_updates_and_clears_keys_immediately() {
        let mut state =
            json!({"awake": false, "preferences": {"theme": "nord", "terminal_font_size": 14}});
        apply_preferences_patch(
            &mut state,
            &json!({"theme": "dracula", "notification_sound": false, "terminal_font_size": null}),
        );
        assert_eq!(state["preferences"]["theme"], json!("dracula"));
        assert_eq!(state["preferences"]["notification_sound"], json!(false));
        assert!(state["preferences"].get("terminal_font_size").is_none());
    }
}
