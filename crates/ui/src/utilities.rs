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
    ui.utility_query.set(String::new());
    ui.utility_popup
        .set(if ui.utility_popup.get_untracked() == name {
            String::new()
        } else {
            name.into()
        });
    ui.terminal_focus.update(|n| *n += 1);
}
pub fn machine(ui: Ui) -> String {
    let pinned = ui.usage_machine.get();
    if !pinned.is_empty() {
        return pinned;
    }
    if ui.page.get() == "terminals"
        && let Some(e) = crate::terminals::current(ui)
    {
        return e.machine().into();
    }
    ui.current_project()
        .map(|p| p.work_location().machine)
        .unwrap_or_else(|| "local".into())
}
pub fn init(ui: Ui) {
    refresh(ui);
    set_interval(std::time::Duration::from_secs(5), move || refresh(ui));
    let target = computed(move || machine(ui));
    effect(move || {
        let machine = target.get();
        untracked(|| usage(ui, machine, false));
    });
    set_interval(std::time::Duration::from_secs(5), move || {
        usage(ui, machine(ui), false)
    });
    set_interval(std::time::Duration::from_secs(60), move || {
        ui.utility_clock.set(canban_types::now() / 1000);
    });
    effect(move || {
        let v = ui.utility.get();
        let saved = v["preferences"]["theme"]
            .as_str()
            .unwrap_or(canban_themes::DEFAULT);
        if ui.utility_popup.get() == "theme" {
            let preview = ui.theme_preview.get();
            set_theme(preview.as_deref().unwrap_or(saved));
        } else {
            // Every dismissal/navigation path discards temporary preview state.
            ui.theme_preview.set(None);
            set_theme(saved);
        }
    });
}
fn refresh(ui: Ui) {
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    if ui.utility_loading.get_untracked() {
        return;
    }
    ui.utility_loading.set(true);
    let epoch = ui.utility_epoch.get_untracked();
    ui.spawn(async move {
        let r = blocking(move || app.call("utilities.state", json!({}))).await;
        ui.utility_loading.set(false);
        if epoch != ui.utility_epoch.get_untracked() {
            return;
        }
        if let Ok(Ok(v)) = r
            && v != ui.utility.get_untracked()
        {
            ui.utility.set(v);
        }
    });
}
pub const DEFAULT_TERMINAL_FONT_SIZE: u16 = 13;
pub const MIN_TERMINAL_FONT_SIZE: u16 = 9;
pub const MAX_TERMINAL_FONT_SIZE: u16 = 32;

pub fn configure_preferences(ui: Ui, patch: Value) {
    if crate::APP.get().is_none() {
        ui.utility.update(|v| {
            if let Some(obj) = patch.as_object() {
                for (k, val) in obj {
                    if val.is_null() {
                        if let Some(prefs) = v["preferences"].as_object_mut() {
                            prefs.remove(k);
                        }
                    } else {
                        v["preferences"][k] = val.clone();
                    }
                }
            }
        });
        return;
    }
    ui.utility_epoch.update(|n| *n += 1);
    dispatch_complete(ui, "utilities.configure", patch, move |r| {
        if let Ok(v) = r {
            ui.utility_epoch.update(|n| *n += 1);
            ui.utility.set(v);
        } else {
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

pub fn terminal_settings(ui: Ui) -> impl IntoElement {
    let catalog = canban_terminal::installed_fonts();
    let mono_count = catalog.monospaced.len();
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

    settings_section(
        "Terminal",
        "Font family and size apply immediately to open Terminals without reconnecting.",
    )
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
    .child(settings_field(
        "Font family",
        &format!(
            "Choose from {mono_count} installed monospaced font families on this Mac (or System Mono)."
        ),
        view()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap_2()
            .child(
                action(
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
                .disabled(move || ui.pending.get()),
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
                    .disabled(move || ui.pending.get())
                },
            )),
    ))
    .child(settings_field(
        "Font size",
        "Point size for terminal cells (9–32 pt).",
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                compact_action("−", Variant::Secondary, move |_| adjust_size(-1)).disabled(
                    move || {
                        ui.pending.get()
                            || ui.utility.with(|v| {
                                v["preferences"]["terminal_font_size"]
                                    .as_u64()
                                    .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE))
                                    <= u64::from(MIN_TERMINAL_FONT_SIZE)
                            })
                    },
                ),
            )
            .child(label(move || {
                let sz = ui.utility.with(|v| {
                    v["preferences"]["terminal_font_size"]
                        .as_u64()
                        .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE))
                });
                format!("{sz} pt")
            }))
            .child(
                compact_action("+", Variant::Secondary, move |_| adjust_size(1)).disabled(
                    move || {
                        ui.pending.get()
                            || ui.utility.with(|v| {
                                v["preferences"]["terminal_font_size"]
                                    .as_u64()
                                    .unwrap_or(u64::from(DEFAULT_TERMINAL_FONT_SIZE))
                                    >= u64::from(MAX_TERMINAL_FONT_SIZE)
                            })
                    },
                ),
            )
            .child(
                compact_action("Reset font", Variant::Quiet, move |_| {
                    configure_preferences(
                        ui,
                        json!({"terminal_font_family":null,"terminal_font_size":null}),
                    );
                })
                .disabled(move || {
                    ui.pending.get()
                        || ui.utility.with(|v| {
                            v["preferences"]["terminal_font_family"].is_null()
                                && v["preferences"]["terminal_font_size"].is_null()
                        })
                }),
            ),
    ))
}
pub fn usage(ui: Ui, machine: String, force: bool) {
    let Some(app) = crate::APP.get().cloned() else {
        if force {
            ui.notify("Subscription usage refreshed");
        }
        return;
    };
    if !force
        && ui.usage_loading.get_untracked()
        && ui.usage.with_untracked(|v| v["machine"] == machine)
    {
        return;
    }
    if ui.usage.with_untracked(|v| v["machine"] != machine) {
        ui.usage.set(json!({"machine":machine,"readings":[]}));
    }
    if force {
        ui.notify("Refreshing subscription usage…");
    }
    ui.usage_loading.set(true);
    ui.usage_epoch.update(|n| *n += 1);
    let epoch = ui.usage_epoch.get_untracked();
    ui.spawn(async move {
        let r = blocking(move || {
            app.call(
                if force {
                    "usage.refresh"
                } else {
                    "usage.status"
                },
                json!({"machine":machine}),
            )
        })
        .await;
        if ui.usage_epoch.get_untracked() != epoch {
            return;
        }
        ui.usage_loading.set(false);
        match r {
            Ok(Ok(v)) => {
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
    if crate::APP.get().is_none() {
        ui.utility.update(|v| v["awake"] = json!(!on));
        return;
    }
    ui.utility_epoch.update(|n| *n += 1);
    dispatch_complete(ui, "utilities.awake", json!({"enabled":!on}), move |r| {
        if let Ok(v) = r {
            ui.utility_epoch.update(|n| *n += 1);
            ui.utility.set(v);
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
            "Commands & Shortcuts · / or ⌘/",
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
                        .on_press(move |_: &()| usage(ui, machine(ui), true)),
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
pub fn settings(ui: Ui) -> impl IntoElement {
    let sounds = signal(crate::reminders::sound_enabled(ui));
    effect(move || sounds.set(crate::reminders::sound_enabled(ui)));
    settings_section(
        "Theme and support",
        "Theme and power controls are also available from the bottom bar.",
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
        checkbox("Notification sounds".into(), sounds, move |enabled| {
            let enabled = *enabled;
            if crate::APP.get().is_none() {
                ui.utility
                    .update(|v| v["preferences"]["notification_sound"] = json!(enabled));
                return;
            }
            dispatch_complete(
                ui,
                "utilities.configure",
                json!({"notification_sound":enabled}),
                move |result| {
                    if let Ok(state) = result {
                        ui.utility_epoch.update(|v| *v += 1);
                        ui.utility.set(state);
                    }
                },
            );
        })
        .disabled(move || ui.is_method_pending("utilities.configure")),
    )
    .child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(action("Choose theme…", Variant::Secondary, move |_| {
                open(ui, "theme")
            }))
            .child(action(
                "Commands & Shortcuts · ⌘/",
                Variant::Secondary,
                move |_| crate::commands::execute(ui, "shortcuts"),
            ))
            .child(action("Report issue", Variant::Secondary, move |_| {
                report::open(ui)
            }))
            .child(action("Copy diagnostics", Variant::Quiet, move |_| {
                copy_text(ui, report::diagnostics())
            })),
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
}
