//! Herdr-backed Agent Setup Onboarding Wizard, Global Agents & Identities Settings,
//! and Project-level Identity Overrides (`Project / Agents`).
use crate::{machines, presenter::*};
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Refresh `ui.agent_catalog` for a given machine (`"local"` or remote Core ID).
pub fn refresh_catalog(ui: Ui, machine: &str) {
    if ui.agent_catalog_loading.get_untracked() {
        return;
    }
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    let target = machine.to_owned();
    ui.agent_catalog_loading.set(true);
    ui.spawn(async move {
        let m = target.clone();
        let result = blocking(move || app.call("agent.catalog", json!({"machine": m}))).await;
        let _ = ui.agent_catalog_loading.try_set(false);
        match result {
            Ok(Ok(val)) => {
                let _ = ui.agent_catalog.try_set(val);
            }
            Ok(Err(e)) => {
                let _ = ui.operation_error.try_set(format!("{e:#}"));
            }
            Err(e) => {
                let _ = ui.operation_error.try_set(e.to_string());
            }
        }
    });
}

fn fallback_catalog_entries(all_profiles: &[Profile]) -> Vec<Value> {
    let builtin = canban_profiles::builtin_catalog();
    let mut rows = Vec::new();
    for entry in &builtin {
        let existing = all_profiles
            .iter()
            .find(|p| p.id == entry.id && !p.is_identity());
        rows.push(json!({
            "id": entry.id,
            "display_name": existing.and_then(|p| p.name.clone()).unwrap_or_else(|| entry.display_name.clone()),
            "binary": entry.binary,
            "kind": "builtin",
            "resumable": entry.resumable,
            "installed": matches!(entry.id.as_str(), "codex" | "claude"),
            "path": Value::Null,
            "configured": existing.is_some(),
            "enabled": existing.is_some_and(|p| p.enabled),
        }));
    }
    for prof in all_profiles {
        if !prof.is_identity() && !builtin.iter().any(|b| b.id == prof.id) {
            rows.push(json!({
                "id": prof.id,
                "display_name": prof.name.clone().unwrap_or_else(|| prof.id.clone()),
                "binary": prof.command_name(),
                "kind": "custom",
                "resumable": !prof.resume_command().is_empty(),
                "installed": true,
                "path": Value::Null,
                "configured": true,
                "enabled": prof.enabled,
            }));
        }
    }
    rows
}

fn catalog_agents(ui: Ui) -> Vec<Value> {
    let catalog = ui.agent_catalog.get();
    if let Some(arr) = catalog["agents"].as_array() {
        return arr.clone();
    }
    fallback_catalog_entries(&ui.all_profiles.get())
}

fn engine_display_name(ui: Ui, id: &str) -> String {
    if let Some(tpl) = id.strip_prefix("herdr:") {
        return format!("Herdr · {tpl}");
    }
    if let Some(name) = canban_profiles::display_name(id) {
        return name.to_owned();
    }
    ui.all_profiles
        .with_untracked(|list| {
            list.iter()
                .find(|p| p.id == id)
                .and_then(|p| p.name.clone())
        })
        .unwrap_or_else(|| id.to_owned())
}

fn engine_icon(id: &str) -> Icon {
    crate::profiles::agent_icon(id)
}

fn enabled_engine_options(ui: Ui) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for prof in ui.all_profiles.get() {
        if !prof.is_identity() && prof.enabled {
            let name = prof
                .name
                .clone()
                .or_else(|| canban_profiles::display_name(&prof.id).map(str::to_owned))
                .unwrap_or_else(|| prof.id.clone());
            out.push((prof.id.clone(), name));
        }
    }
    out
}

fn builtin_role_summary(role_id: &str, role_instruction: &str) -> String {
    match role_id {
        "planner" => "Read-only analysis & implementation planning".into(),
        "developer" => "Implementation & default interactive Agent".into(),
        "reviewer" => "Independent diff review & verification".into(),
        _ if role_instruction.trim().is_empty() => "Custom identity".into(),
        _ => {
            let trimmed = role_instruction.lines().next().unwrap_or_default().trim();
            if trimmed.chars().count() > 72 {
                format!("{}…", trimmed.chars().take(72).collect::<String>())
            } else {
                trimmed.to_owned()
            }
        }
    }
}

/// Soft agent tile chip used across Agent Setup and Settings (replaces full-width heavy borders).
fn agent_tile(
    id: String,
    display: String,
    installed: bool,
    checked: bool,
    on_toggle: impl Fn() + 'static,
) -> AnyElement {
    let mark = engine_icon(&id);
    nsh_base::button()
        .w(px(192.))
        .min_h(px(44.))
        .px_3()
        .py_2()
        .rounded(px(RADIUS))
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .bg(if checked { SELECTED } else { SURFACE })
        .border_1()
        .border_color(if checked { ACCENT } else { BORDER })
        .hover(|s| s.bg(if checked { SELECTED } else { INSET }))
        .focus(|s| s.border_color(ACCENT))
        .active(|s| s.opacity(0.75))
        .accessibility_label(Some(
            format!(
                "{} @{}{} · {}",
                display,
                id,
                if installed { "" } else { " (not installed)" },
                if checked { "enabled" } else { "disabled" }
            )
            .into(),
        ))
        .on_press(move |_: &()| on_toggle())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2p5()
                .min_w(px(0.))
                .flex_1()
                .child(
                    icon_tinted(mark, if checked { ACCENT } else { MUTED })
                        .w(px(17.))
                        .h(px(17.)),
                )
                .child(
                    view()
                        .gap_0p5()
                        .min_w(px(0.))
                        .flex_1()
                        .child(
                            label(display)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if checked { TEXT } else { MUTED })
                                .truncate(),
                        )
                        .child(
                            muted(if installed {
                                format!("@{id}")
                            } else {
                                format!("@{id} · missing")
                            })
                            .text_size(px(11.))
                            .truncate(),
                        ),
                ),
        )
        .child(if checked {
            icon_tinted(Icon::Check, ACCENT)
                .w(px(14.))
                .h(px(14.))
                .into_element()
        } else {
            view().w(px(14.)).h(px(14.)).into_element()
        })
        .into_element()
}

/// First-run banner shown on empty Board before onboarding has been completed.
pub fn onboarding_banner(ui: Ui) -> impl IntoElement {
    panel()
        .gap_2p5()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap_3()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_1()
                        .min_w(px(240.))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .px_2()
                                .py_1p5()
                                .rounded(px(RADIUS))
                                .bg(INSET)
                                .child(icon(Icon::Codex).w(px(15.)).h(px(15.)))
                                .child(icon(Icon::Claude).w(px(15.)).h(px(15.)))
                                .child(icon(Icon::Agy).w(px(15.)).h(px(15.)))
                                .child(icon(Icon::Gemini).w(px(15.)).h(px(15.))),
                        )
                        .child(
                            view()
                                .gap_0p5()
                                .min_w(px(0.))
                                .flex_1()
                                .child(
                                    label("Choose your enabled Agents")
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .child(muted(
                                    "Pick which installed Herdr CLI Agents appear in @mentions and back @developer.",
                                )),
                        ),
                )
                .child(compact_action(
                    "Set up Agents…",
                    Variant::Primary,
                    move |_| ui.navigate("agent-setup"),
                )),
        )
}

/// Dedicated Onboarding & Re-runnable Agent Setup screen (`page == "agent-setup"`).
pub fn onboarding_screen(ui: Ui) -> impl IntoElement {
    if ui.agent_catalog.with_untracked(Value::is_null) {
        refresh_catalog(ui, "local");
    }

    let enabled_set = signal(BTreeSet::<String>::new());
    let primary_engine = signal(String::new());
    let per_role_open = signal(false);
    let show_uninstalled = signal(false);
    let role_engines = signal(BTreeMap::<String, String>::new());
    let seeded = stored(false);

    effect(move || {
        let agents = catalog_agents(ui);
        let all = ui.all_profiles.get();
        let onboarded = ui.onboarded.get();
        if seeded.with(|s| *s) && !enabled_set.with_untracked(BTreeSet::is_empty) {
            return;
        }
        let mut chosen = BTreeSet::new();
        if onboarded {
            for p in &all {
                if !p.is_identity() && p.enabled {
                    chosen.insert(p.id.clone());
                }
            }
        } else {
            for row in &agents {
                if row["installed"] == true
                    && let Some(id) = row["id"].as_str()
                {
                    chosen.insert(id.to_owned());
                }
            }
        }
        if chosen.is_empty() {
            for p in &all {
                if !p.is_identity() && p.enabled {
                    chosen.insert(p.id.clone());
                }
            }
        }
        if chosen.is_empty() {
            chosen.insert("codex".into());
        }

        let current_dev_engine = all
            .iter()
            .find(|p| p.id == "developer")
            .and_then(|p| p.preset.clone())
            .filter(|e| chosen.contains(e))
            .unwrap_or_else(|| {
                chosen
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "codex".into())
            });

        let mut map = BTreeMap::new();
        for p in &all {
            if p.is_identity() {
                let eng = p
                    .preset
                    .clone()
                    .filter(|e| chosen.contains(e))
                    .unwrap_or_else(|| current_dev_engine.clone());
                map.insert(p.id.clone(), eng);
            }
        }
        for role in ["planner", "developer", "reviewer"] {
            map.entry(role.into())
                .or_insert_with(|| current_dev_engine.clone());
        }
        enabled_set.set(chosen);
        primary_engine.set(current_dev_engine);
        role_engines.set(map);
        seeded.replace(true);
    });

    let back_target = if ui.onboarded.get_untracked() {
        "settings"
    } else {
        "board"
    };
    let back_label = if back_target == "settings" {
        "Back to Settings"
    } else {
        "Back to Board"
    };

    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(subpage_header(back_label, "Agent Setup", move |_| {
            ui.navigate(back_target)
        }))
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .w_full()
                    .max_w(px(680.))
                    .gap_6()
                    .pb_6()
                    .child(when(
                        move || !ui.operation_error.get().is_empty(),
                        move || notice(move || ui.operation_error.get(), Tone::Attention),
                    ))
                    .child(
                        settings_section(
                            "Enabled Agents",
                            "Selected Agents appear in @mentions, pickers, and role options; unselected ones stay hidden.",
                        )
                        .trailing(compact_action(
                            move || {
                                if ui.agent_catalog_loading.get() {
                                    "Scanning…"
                                } else {
                                    "Rescan"
                                }
                            },
                            Variant::Quiet,
                            move |_| {
                                seeded.replace(false);
                                refresh_catalog(ui, "local");
                            },
                        ))
                        .child(dynamic(move || {
                            let rows = catalog_agents(ui);
                            let installed: Vec<Value> = rows
                                .iter()
                                .filter(|r| r["installed"] == true)
                                .cloned()
                                .collect();
                            let missing: Vec<Value> = rows
                                .iter()
                                .filter(|r| r["installed"] != true)
                                .cloned()
                                .collect();
                            let current_enabled = enabled_set.get();

                            let render_chip = |row: Value| -> AnyElement {
                                let id = row["id"].as_str().unwrap_or("").to_owned();
                                let display =
                                    row["display_name"].as_str().unwrap_or(&id).to_owned();
                                let is_inst = row["installed"] == true;
                                let checked = current_enabled.contains(&id);
                                let toggle_id = id.clone();
                                agent_tile(id, display, is_inst, checked, move || {
                                    let mut next = enabled_set.get_untracked();
                                    if next.contains(&toggle_id) {
                                        if next.len() > 1 {
                                            next.remove(&toggle_id);
                                        }
                                    } else {
                                        next.insert(toggle_id.clone());
                                    }
                                    if !next.contains(&primary_engine.get_untracked())
                                        && let Some(first) = next.iter().next().cloned()
                                    {
                                        primary_engine.set(first.clone());
                                        role_engines.update(|m| {
                                            for val in m.values_mut() {
                                                if !next.contains(val) {
                                                    *val = first.clone();
                                                }
                                            }
                                        });
                                    }
                                    enabled_set.set(next);
                                })
                            };

                            let missing_count = missing.len();
                            let show_missing = show_uninstalled.get();
                            view()
                                .gap_2p5()
                                .child(if installed.is_empty() {
                                    muted("No CLI binaries detected on PATH — expand below to pick manually.").into_element()
                                } else {
                                    view()
                                        .flex_row()
                                        .flex_wrap()
                                        .gap_2()
                                        .children(installed.into_iter().map(&render_chip))
                                        .into_element()
                                })
                                .child(if missing_count > 0 {
                                    view()
                                        .gap_2()
                                        .child(
                                            action(
                                                format!(
                                                    "{} More supported Agents ({missing_count})",
                                                    if show_missing { "▾" } else { "▸" }
                                                ),
                                                Variant::Quiet,
                                                move |_| show_uninstalled.update(|v| *v = !*v),
                                            )
                                            .self_start(),
                                        )
                                        .child(if show_missing {
                                            view()
                                                .flex_row()
                                                .flex_wrap()
                                                .gap_2()
                                                .children(missing.into_iter().map(&render_chip))
                                                .into_element()
                                        } else {
                                            view().into_element()
                                        })
                                        .into_element()
                                } else {
                                    view().into_element()
                                })
                                .into_element()
                        })),
                    )
                    .child(
                        settings_section(
                            "Default Agent",
                            "Powers @planner, @developer, and @reviewer unless customized below or overridden in a Project.",
                        )
                        .child(dynamic(move || {
                            let enabled: Vec<String> = enabled_set.get().into_iter().collect();
                            let current_primary = primary_engine.get();
                            view()
                                .gap_3()
                                .child(
                                    segmented().children(enabled.iter().map(|eng| {
                                        let id = eng.clone();
                                        let sel_id = eng.clone();
                                        let cp = current_primary.clone();
                                        let title = engine_display_name(ui, eng);
                                        let mark = engine_icon(eng);
                                        compact_segment(
                                            title,
                                            Some(mark),
                                            move || cp == sel_id,
                                            move |_| {
                                                let chosen = id.clone();
                                                primary_engine.set(chosen.clone());
                                                role_engines.update(|m| {
                                                    for role in
                                                        ["planner", "developer", "reviewer"]
                                                    {
                                                        m.insert(role.into(), chosen.clone());
                                                    }
                                                });
                                            },
                                        )
                                        .into_element()
                                    })),
                                )
                                .child(
                                    action(
                                        move || {
                                            let arrow = if per_role_open.get() { "▾" } else { "▸" };
                                            format!("{arrow} Customize per identity")
                                        },
                                        Variant::Quiet,
                                        move |_| per_role_open.update(|v| *v = !*v),
                                    )
                                    .self_start(),
                                )
                                .child(if !per_role_open.get() {
                                    view().into_element()
                                } else {
                                    let map = role_engines.get();
                                    let enabled_for_roles = enabled.clone();
                                    view()
                                        .border_t_1()
                                        .border_color(BORDER)
                                        .children(
                                            [
                                                ("developer", "Implementation & interactive"),
                                                ("planner", "Read-only planning"),
                                                ("reviewer", "Independent code review"),
                                            ]
                                            .into_iter()
                                            .map(|(role_id, desc)| {
                                                let current = map
                                                    .get(role_id)
                                                    .cloned()
                                                    .unwrap_or_else(|| {
                                                        primary_engine.get_untracked()
                                                    });
                                                let role_key = role_id.to_owned();
                                                view()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_between()
                                                    .flex_wrap()
                                                    .gap_3()
                                                    .py_2p5()
                                                    .border_b_1()
                                                    .border_color(BORDER)
                                                    .child(
                                                        view()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap_2()
                                                            .child(
                                                                icon(engine_icon(&current))
                                                                    .w(px(15.))
                                                                    .h(px(15.)),
                                                            )
                                                            .child(
                                                                label(format!("@{role_id}"))
                                                                    .font_weight(FontWeight::MEDIUM),
                                                            )
                                                            .child(muted(desc).text_size(px(12.))),
                                                    )
                                                    .child(segmented().children(
                                                        enabled_for_roles.iter().map(|eng| {
                                                            let eid = eng.clone();
                                                            let check_eid = eng.clone();
                                                            let cur = current.clone();
                                                            let rk = role_key.clone();
                                                            compact_segment(
                                                                engine_display_name(ui, eng),
                                                                Some(engine_icon(eng)),
                                                                move || cur == check_eid,
                                                                move |_| {
                                                                    let chosen = eid.clone();
                                                                    let k = rk.clone();
                                                                    role_engines.update(|m| {
                                                                        m.insert(k, chosen);
                                                                    });
                                                                },
                                                            )
                                                            .into_element()
                                                        }),
                                                    ))
                                                    .into_element()
                                            }),
                                        )
                                        .into_element()
                                })
                                .into_element()
                        })),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(action("Cancel", Variant::Quiet, move |_| {
                                ui.navigate(back_target)
                            }))
                            .child(
                                action("Save Setup", Variant::Primary, move |_| {
                                    let enabled: Vec<String> =
                                        enabled_set.get_untracked().into_iter().collect();
                                    let primary = primary_engine.get_untracked();
                                    let roles = role_engines.get_untracked();
                                    dispatch_then(
                                        ui,
                                        "agent.setup",
                                        json!({
                                            "enabled_agents": enabled,
                                            "default_engine": primary,
                                            "identity_engines": roles,
                                        }),
                                        move |_| {
                                            refresh_catalog(ui, "local");
                                            ui.navigate(back_target);
                                        },
                                    );
                                })
                                .disabled(move || {
                                    ui.pending.get() || enabled_set.with(BTreeSet::is_empty)
                                }),
                            ),
                    ),
            ),
        )
}

/// Global Settings section for Agents, Custom CLI Agents, Default `@identity`, and Role Identities.
pub fn global_settings_section(ui: Ui, config_path: String) -> impl IntoElement {
    if ui.agent_catalog.with_untracked(Value::is_null) {
        refresh_catalog(ui, "local");
    }

    let show_all_catalog = signal(false);
    let adding_identity = signal(false);
    let new_role_id = signal(String::new());
    let new_role_engine = signal(String::new());
    let new_role_prompt = signal(String::new());
    let editing_role = signal(Option::<String>::None);
    let editing_prompt = signal(String::new());

    let adding_custom_agent = signal(false);
    let custom_agent_id = signal(String::new());
    let custom_agent_cmd = signal(String::new());

    let copy_path = config_path.clone();

    settings_section(
        "Agents & Identities",
        "Manage enabled Herdr CLI Agents, @identities, and the default interactive Agent.",
    )
    .trailing(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(compact_action(
                "Run Setup…",
                Variant::Secondary,
                move |_| ui.navigate("agent-setup"),
            ))
            .child(compact_action("Reload", Variant::Quiet, move |_| {
                dispatch_then(ui, "config.reload", json!({}), move |_| {
                    refresh_catalog(ui, "local");
                });
            })),
    )
    .child(
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap_2()
            .py_1()
            .child(
                view()
                    .gap_0p5()
                    .child(label("Default Agent").font_weight(FontWeight::MEDIUM))
                    .child(
                        muted("Used when a Workflow step omits profile, and pre-selected for reviews, Workflows, and @.")
                            .text_size(px(12.)),
                    ),
            )
            .child(dynamic(move || {
                let current_default = ui.default_agent.get();
                let enabled_profiles = ui.profiles.get();
                let cur_mark = enabled_profiles
                    .iter()
                    .find(|p| p.id == current_default)
                    .map(|p| crate::profiles::provider(p).0)
                    .unwrap_or(Icon::Agent);
                action_with_icon(
                    cur_mark,
                    format!("@{current_default} ▾"),
                    Variant::Secondary,
                    move |_| {
                        use nsh_core::element::desktop::{MenuItem, popup_menu};
                        let list = enabled_profiles.clone();
                        ui.spawn(async move {
                            let items: Vec<MenuItem> = list
                                .into_iter()
                                .map(|p| {
                                    let pid = p.id.clone();
                                    let (_, prov) = crate::profiles::provider(&p);
                                    MenuItem::new(format!("@{} · {prov}", p.id)).on_select(
                                        move || {
                                            dispatch(
                                                ui,
                                                "config.agents.save",
                                                json!({"op": "set_default", "default_agent": pid}),
                                            );
                                        },
                                    )
                                })
                                .collect();
                            if !items.is_empty() {
                                popup_menu(items).await;
                            }
                        });
                    },
                )
            })),
    )
    .child(
        view()
            .gap_2()
            .pt_1()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(label("Identities").font_weight(FontWeight::SEMIBOLD))
                    .child(compact_action(
                        move || {
                            if adding_identity.get() {
                                "Cancel"
                            } else {
                                "+ Identity"
                            }
                        },
                        Variant::Quiet,
                        move |_| {
                            if !adding_identity.get_untracked() {
                                let first_eng = enabled_engine_options(ui)
                                    .first()
                                    .map(|(id, _)| id.clone())
                                    .unwrap_or_else(|| "codex".into());
                                new_role_engine.set(first_eng);
                                new_role_id.set(String::new());
                                new_role_prompt.set(String::new());
                            }
                            adding_identity.update(|v| *v = !*v);
                        },
                    )),
            )
            .child(when(
                move || adding_identity.get(),
                move || {
                    panel()
                        .gap_2p5()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(
                                    view().w(px(200.)).child(
                                        bound_field(
                                            move || new_role_id.get(),
                                            move |v| new_role_id.set(v.to_owned()),
                                            "qa, architect…",
                                        )
                                        .label("Identity ID"),
                                    ),
                                )
                                .child(dynamic(move || {
                                    let engines = enabled_engine_options(ui);
                                    let cur = new_role_engine.get();
                                    segmented()
                                        .children(engines.into_iter().map(|(eid, ename)| {
                                            let click_id = eid.clone();
                                            let check_id = eid.clone();
                                            let cur_c = cur.clone();
                                            let mark = engine_icon(&eid);
                                            compact_segment(
                                                ename,
                                                Some(mark),
                                                move || cur_c == check_id,
                                                move |_| new_role_engine.set(click_id.clone()),
                                            )
                                            .into_element()
                                        }))
                                        .into_element()
                                })),
                        )
                        .child(
                            bound_field(
                                move || new_role_prompt.get(),
                                move |v| new_role_prompt.set(v.to_owned()),
                                "Role instruction (optional)…",
                            )
                            .label("Role instruction"),
                        )
                        .child(
                            view()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(compact_action(
                                    "Add Identity",
                                    Variant::Primary,
                                    move |_| {
                                        let id = new_role_id.get_untracked();
                                        let eng = new_role_engine.get_untracked();
                                        let prompt = new_role_prompt.get_untracked();
                                        dispatch_then(
                                            ui,
                                            "config.agents.save",
                                            json!({
                                                "op": "upsert_identity",
                                                "id": id,
                                                "engine": eng,
                                                "role_instruction": prompt,
                                            }),
                                            move |_| {
                                                adding_identity.set(false);
                                                refresh_catalog(ui, "local");
                                            },
                                        );
                                    },
                                )),
                        )
                },
            ))
            .child(dynamic(move || {
                let all = ui.all_profiles.get();
                let engines = enabled_engine_options(ui);
                let identities: Vec<Profile> =
                    all.into_iter().filter(|p| p.is_identity()).collect();
                let active_edit = editing_role.get();
                view()
                    .border_t_1()
                    .border_color(BORDER)
                    .children(identities.into_iter().map(|prof| {
                        let pid = prof.id.clone();
                        let (mark, prov_name) = crate::profiles::provider(&prof);
                        let is_builtin_role =
                            matches!(pid.as_str(), "planner" | "developer" | "reviewer");
                        let is_editing = active_edit.as_deref() == Some(pid.as_str());
                        let engines_for_menu = engines.clone();
                        let switch_pid = pid.clone();
                        let role_text = prof.role_instruction.clone();
                        let short_desc = builtin_role_summary(&pid, &role_text);
                        let edit_pid = pid.clone();
                        let edit_seed = role_text.clone();
                        let del_pid = pid.clone();
                        let toggle_pid = pid.clone();
                        let prof_enabled = prof.enabled;

                        view()
                            .py_2p5()
                            .border_b_1()
                            .border_color(BORDER)
                            .gap_2()
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2p5()
                                            .min_w(px(0.))
                                            .flex_1()
                                            .child(
                                                icon_tinted(
                                                    mark,
                                                    if prof_enabled { TEXT } else { MUTED },
                                                )
                                                .w(px(16.))
                                                .h(px(16.)),
                                            )
                                            .child(
                                                label(format!("@{pid}"))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(if prof_enabled {
                                                        TEXT
                                                    } else {
                                                        MUTED
                                                    }),
                                            )
                                            .child(
                                                muted(short_desc)
                                                    .text_size(px(12.))
                                                    .truncate()
                                                    .min_w(px(0.))
                                                    .flex_1(),
                                            )
                                            .child(when(
                                                move || !prof_enabled,
                                                || badge("Disabled"),
                                            )),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .child(compact_action(
                                                format!("{prov_name} ▾"),
                                                Variant::Secondary,
                                                move |_| {
                                                    use nsh_core::element::desktop::{
                                                        MenuItem, popup_menu,
                                                    };
                                                    let opts = engines_for_menu.clone();
                                                    let target_role = switch_pid.clone();
                                                    ui.spawn(async move {
                                                        let items: Vec<MenuItem> = opts
                                                            .into_iter()
                                                            .map(|(eid, ename)| {
                                                                let r = target_role.clone();
                                                                MenuItem::new(format!(
                                                                    "{ename} (@{eid})"
                                                                ))
                                                                .on_select(move || {
                                                                    dispatch(
                                                                        ui,
                                                                        "config.agents.save",
                                                                        json!({
                                                                            "op": "upsert_identity",
                                                                            "id": r,
                                                                            "engine": eid,
                                                                            "enabled": true,
                                                                        }),
                                                                    );
                                                                })
                                                            })
                                                            .collect();
                                                        if !items.is_empty() {
                                                            popup_menu(items).await;
                                                        }
                                                    });
                                                },
                                            ))
                                            .child(compact_action(
                                                if is_editing { "Done" } else { "Prompt" },
                                                Variant::Quiet,
                                                move |_| {
                                                    if editing_role.get_untracked().as_deref()
                                                        == Some(edit_pid.as_str())
                                                    {
                                                        editing_role.set(None);
                                                    } else {
                                                        editing_prompt.set(edit_seed.clone());
                                                        editing_role.set(Some(edit_pid.clone()));
                                                    }
                                                },
                                            ))
                                            .child(compact_action(
                                                if prof_enabled { "Disable" } else { "Enable" },
                                                Variant::Quiet,
                                                move |_| {
                                                    dispatch(
                                                        ui,
                                                        "config.agents.save",
                                                        json!({
                                                            "op": "toggle_agent",
                                                            "id": toggle_pid,
                                                            "enabled": !prof_enabled,
                                                        }),
                                                    );
                                                },
                                            ))
                                            .child(when(
                                                move || !is_builtin_role,
                                                move || {
                                                    let d = del_pid.clone();
                                                    compact_action(
                                                        "Delete",
                                                        Variant::Danger,
                                                        move |_| {
                                                            dispatch(
                                                                ui,
                                                                "config.agents.save",
                                                                json!({
                                                                    "op": "delete_profile",
                                                                    "id": d,
                                                                }),
                                                            );
                                                        },
                                                    )
                                                },
                                            )),
                                    ),
                            )
                            .child(if is_editing {
                                let save_id = pid.clone();
                                let cur_engine =
                                    prof.preset.clone().unwrap_or_else(|| prof.command_name());
                                view()
                                    .gap_2()
                                    .pt_1()
                                    .child(
                                        bound_field(
                                            move || editing_prompt.get(),
                                            move |v| editing_prompt.set(v.to_owned()),
                                            "Role instruction…",
                                        )
                                        .label("Edit role instruction"),
                                    )
                                    .child(view().flex_row().justify_end().child(compact_action(
                                        "Save prompt",
                                        Variant::Primary,
                                        move |_| {
                                            let next_prompt = editing_prompt.get_untracked();
                                            let id_c = save_id.clone();
                                            let eng_c = cur_engine.clone();
                                            dispatch_then(
                                                ui,
                                                "config.agents.save",
                                                json!({
                                                    "op": "upsert_identity",
                                                    "id": id_c,
                                                    "engine": eng_c,
                                                    "role_instruction": next_prompt,
                                                    "enabled": prof_enabled,
                                                }),
                                                move |_| editing_role.set(None),
                                            );
                                        },
                                    )))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
                            .into_element()
                    }))
            })),
    )
    .child(
        view()
            .gap_2p5()
            .pt_2()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(label("Enabled CLI Agents").font_weight(FontWeight::SEMIBOLD))
                    .child(compact_action(
                        move || {
                            if adding_custom_agent.get() {
                                "Cancel"
                            } else {
                                "+ Custom CLI"
                            }
                        },
                        Variant::Quiet,
                        move |_| adding_custom_agent.update(|v| *v = !*v),
                    )),
            )
            .child(when(
                move || adding_custom_agent.get(),
                move || {
                    panel()
                        .gap_2p5()
                        .child(
                            bound_field(
                                move || custom_agent_id.get(),
                                move |v| custom_agent_id.set(v.to_owned()),
                                "Agent ID (e.g. my-agent)",
                            )
                            .label("Custom Agent ID"),
                        )
                        .child(
                            bound_field(
                                move || custom_agent_cmd.get(),
                                move |v| custom_agent_cmd.set(v.to_owned()),
                                "Command tokens (e.g. my-agent --interactive {prompt})",
                            )
                            .label("Launch command"),
                        )
                        .child(view().flex_row().justify_end().child(compact_action(
                            "Save Custom Agent",
                            Variant::Primary,
                            move |_| {
                                let id = custom_agent_id.get_untracked();
                                let argv: Vec<String> = custom_agent_cmd
                                    .get_untracked()
                                    .split_whitespace()
                                    .map(str::to_owned)
                                    .collect();
                                dispatch_then(
                                    ui,
                                    "config.agents.save",
                                    json!({
                                        "op": "upsert_custom_agent",
                                        "id": id,
                                        "argv": argv,
                                        "enabled": true,
                                    }),
                                    move |_| {
                                        adding_custom_agent.set(false);
                                        custom_agent_id.set(String::new());
                                        custom_agent_cmd.set(String::new());
                                        refresh_catalog(ui, "local");
                                    },
                                );
                            },
                        )))
                },
            ))
            .child(dynamic(move || {
                let rows = catalog_agents(ui);
                let primary_rows: Vec<Value> = rows
                    .iter()
                    .filter(|r| r["installed"] == true || r["enabled"] == true)
                    .cloned()
                    .collect();
                let extra_rows: Vec<Value> = rows
                    .iter()
                    .filter(|r| r["installed"] != true && r["enabled"] != true)
                    .cloned()
                    .collect();

                let render_agent = move |row: Value| -> AnyElement {
                    let id = row["id"].as_str().unwrap_or("").to_owned();
                    let display = row["display_name"].as_str().unwrap_or(&id).to_owned();
                    let enabled = row["enabled"] == true;
                    let installed = row["installed"] == true;
                    let toggle_id = id.clone();
                    agent_tile(id, display, installed, enabled, move || {
                        dispatch_then(
                            ui,
                            "config.agents.save",
                            json!({
                                "op": "toggle_agent",
                                "id": toggle_id,
                                "enabled": !enabled,
                            }),
                            move |_| refresh_catalog(ui, "local"),
                        );
                    })
                };

                let count = extra_rows.len();
                let show_extra = show_all_catalog.get();
                view()
                    .gap_2()
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .children(primary_rows.into_iter().map(render_agent)),
                    )
                    .child(if count > 0 {
                        view()
                            .gap_2()
                            .child(
                                action(
                                    format!(
                                        "{} Show {count} uninstalled Agents",
                                        if show_extra { "▾" } else { "▸" }
                                    ),
                                    Variant::Quiet,
                                    move |_| show_all_catalog.update(|v| *v = !*v),
                                )
                                .self_start(),
                            )
                            .child(if show_extra {
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .gap_2()
                                    .children(extra_rows.into_iter().map(render_agent))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
                            .into_element()
                    } else {
                        view().into_element()
                    })
                    .into_element()
            })),
    )
    .child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .pt_1()
            .child(muted("Config:"))
            .child(muted(config_path).font_family("Menlo").text_size(px(11.5)))
            .child(compact_action("Copy path", Variant::Quiet, move |_| {
                copy_text(ui, copy_path.clone())
            })),
    )
}

/// Reusable Project-level Identity -> Agent Override editor, used both on the
/// dedicated `Project / Agents` screen and inside `New Project`.
pub fn project_overrides_editor(
    ui: Ui,
    overrides: Signal<BTreeMap<String, String>>,
    machine: Signal<String>,
) -> impl IntoElement {
    effect(move || {
        let m = machine.get();
        refresh_catalog(ui, &m);
    });

    dynamic(move || {
        let all = ui.all_profiles.get();
        let identities: Vec<Profile> = all
            .iter()
            .filter(|p| p.is_identity() && p.enabled)
            .cloned()
            .collect();
        let engines = enabled_engine_options(ui);
        let cur_map = overrides.get();

        let uniform_override = if !identities.is_empty() {
            let first = cur_map.get(&identities[0].id);
            if let Some(target) = first
                && identities
                    .iter()
                    .all(|id| cur_map.get(&id.id) == Some(target))
            {
                Some(target.clone())
            } else {
                None
            }
        } else {
            None
        };

        let ids_for_all: Vec<String> = identities.iter().map(|p| p.id.clone()).collect();

        view()
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(muted("All roles:"))
                    .child(
                        segmented()
                            .child(compact_segment(
                                "Follow Global",
                                None,
                                {
                                    let is_empty = cur_map.is_empty();
                                    move || is_empty
                                },
                                move |_| overrides.set(BTreeMap::new()),
                            ))
                            .children(engines.iter().map(|(eid, ename)| {
                                let target_id = eid.clone();
                                let check_id = eid.clone();
                                let uni = uniform_override.clone();
                                let role_ids = ids_for_all.clone();
                                let mark = engine_icon(eid);
                                compact_segment(
                                    ename.clone(),
                                    Some(mark),
                                    move || uni.as_deref() == Some(check_id.as_str()),
                                    move |_| {
                                        let mut next = BTreeMap::new();
                                        for r in &role_ids {
                                            next.insert(r.clone(), target_id.clone());
                                        }
                                        overrides.set(next);
                                    },
                                )
                                .into_element()
                            })),
                    ),
            )
            .child(
                view()
                    .border_t_1()
                    .border_color(BORDER)
                    .children(identities.into_iter().map(|prof| {
                        let role_id = prof.id.clone();
                        let (global_icon, global_prov) = crate::profiles::provider(&prof);
                        let current_override = cur_map.get(&role_id).cloned();
                        let effective_mark = current_override
                            .as_deref()
                            .map(engine_icon)
                            .unwrap_or(global_icon);
                        let engines_row = engines.clone();
                        let clear_role = role_id.clone();
                        let short_desc = builtin_role_summary(&role_id, &prof.role_instruction);

                        view()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .flex_wrap()
                            .gap_3()
                            .py_2p5()
                            .border_b_1()
                            .border_color(BORDER)
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2p5()
                                    .min_w(px(180.))
                                    .flex_1()
                                    .child(
                                        icon_tinted(
                                            effective_mark,
                                            if current_override.is_some() {
                                                ACCENT
                                            } else {
                                                MUTED
                                            },
                                        )
                                        .w(px(16.))
                                        .h(px(16.)),
                                    )
                                    .child(
                                        view()
                                            .gap_0p5()
                                            .min_w(px(0.))
                                            .flex_1()
                                            .child(
                                                view()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        label(format!("@{role_id}"))
                                                            .font_weight(FontWeight::MEDIUM),
                                                    )
                                                    .child(if current_override.is_some() {
                                                        status("Override", Tone::Info)
                                                            .into_element()
                                                    } else {
                                                        view().into_element()
                                                    }),
                                            )
                                            .child(muted(short_desc).text_size(px(12.)).truncate()),
                                    ),
                            )
                            .child(
                                segmented()
                                    .child(compact_segment(
                                        format!("Global ({global_prov})"),
                                        Some(global_icon),
                                        {
                                            let is_none = current_override.is_none();
                                            move || is_none
                                        },
                                        move |_| {
                                            let k = clear_role.clone();
                                            overrides.update(|m| {
                                                m.remove(&k);
                                            });
                                        },
                                    ))
                                    .children(engines_row.into_iter().map(|(eid, ename)| {
                                        let set_eid = eid.clone();
                                        let check_eid = eid.clone();
                                        let cur_ov = current_override.clone();
                                        let rk = role_id.clone();
                                        let mark = engine_icon(&eid);
                                        compact_segment(
                                            ename,
                                            Some(mark),
                                            move || cur_ov.as_deref() == Some(check_eid.as_str()),
                                            move |_| {
                                                let k = rk.clone();
                                                let v = set_eid.clone();
                                                overrides.update(|m| {
                                                    m.insert(k, v);
                                                });
                                            },
                                        )
                                        .into_element()
                                    })),
                            )
                            .into_element()
                    })),
            )
    })
}

/// Dedicated `Project / Agents` screen (`page == "project-agents"`).
pub fn project_agents_screen(ui: Ui) -> impl IntoElement {
    let proj = ui.current_project();
    let initial_overrides = proj
        .as_ref()
        .map(|p| p.agent_overrides.clone())
        .unwrap_or_default();
    let machine_id = proj
        .as_ref()
        .map(|p| p.work_location().machine)
        .unwrap_or_else(|| "local".into());

    let overrides = signal(initial_overrides);
    let machine = signal(machine_id);

    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(subpage_header(
            "Back to Board",
            move || {
                format!(
                    "{} / Project Agents",
                    ui.current_project()
                        .map(|p| p.name)
                        .unwrap_or_else(|| "Project".into())
                )
            },
            move |_| ui.navigate("board"),
        ))
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .w_full()
                    .max_w(px(680.))
                    .gap_5()
                    .pb_6()
                    .child(muted(move || {
                        format!(
                            "Choose which Agent runs each @identity in this Project ({}) without editing Workflow YAML.",
                            machines::name(ui, &machine.get())
                        )
                    }))
                    .child(when(
                        move || !ui.operation_error.get().is_empty(),
                        move || notice(move || ui.operation_error.get(), Tone::Attention),
                    ))
                    .child(project_overrides_editor(ui, overrides, machine))
                    .child(
                        view()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .pt_1()
                            .child(compact_action(
                                "Global Agents & Identities…",
                                Variant::Quiet,
                                move |_| ui.navigate("settings"),
                            ))
                            .child(
                                view()
                                    .flex_row()
                                    .gap_2()
                                    .child(action("Cancel", Variant::Quiet, move |_| {
                                        ui.navigate("board")
                                    }))
                                    .child(
                                        action("Save", Variant::Primary, move |_| {
                                            let pid = ui.project.get_untracked();
                                            let map = overrides.get_untracked();
                                            dispatch_then(
                                                ui,
                                                "project.agents",
                                                json!({
                                                    "project_id": pid,
                                                    "agent_overrides": map,
                                                }),
                                                move |_| ui.navigate("board"),
                                            );
                                        })
                                        .disabled(move || {
                                            ui.pending.get() || ui.project.get().is_empty()
                                        }),
                                    ),
                            ),
                    ),
            ),
        )
}
