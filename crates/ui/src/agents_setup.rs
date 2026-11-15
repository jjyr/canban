//! Herdr-backed Agent Setup Onboarding Wizard, Global Agents & Identities Settings,
//! and Project-level Identity Overrides (`Project / Agents`).
use crate::presenter::*;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

fn active_home_path() -> PathBuf {
    crate::APP
        .get()
        .map(|app| app.home.clone())
        .unwrap_or_else(|| PathBuf::from("/preview/.canban"))
}

pub fn skill_install_command() -> String {
    canban_context::skill_install_command(&active_home_path())
}

pub fn copy_skill_command(ui: Ui) {
    copy_text(ui, skill_install_command());
}

fn canban_skill_box(ui: Ui) -> View {
    let copied = signal(false);
    let command = skill_install_command();
    let command_for_copy = command.clone();

    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(INSET)
                .child(
                    label(command)
                        .font_family("Menlo")
                        .text_size(px(12.))
                        .min_w(px(0.))
                        .flex_1(),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap_1p5()
                        .child(compact_action(
                            move || if copied.get() { "✓ Copied" } else { "Copy" },
                            Variant::Secondary,
                            move |_| {
                                copy_text(ui, command_for_copy.clone());
                                copied.set(true);
                            },
                        ))
                        .child(compact_action(
                            "Open Terminal  ⌘T",
                            Variant::Secondary,
                            move |_| crate::commands::execute(ui, "new-terminal"),
                        )),
                ),
        )
        .child(
            muted(
                "Tip: Copy the command first, then press ⌘T to open a standalone Terminal and paste it to install or update.",
            )
            .text_size(px(11.5)),
        )
}

pub fn canban_skill_onboarding_section(ui: Ui) -> SectionGroup {
    settings_section(
        "Canban CLI Skill",
        "Optional · Teach your enabled Agents how to operate Cards, comments, reviews, and workflows via $CANBAN_CLI.",
    )
    .trailing(badge("Optional"))
    .child(canban_skill_box(ui))
}

fn canban_skill_settings_block(ui: Ui) -> View {
    view()
        .gap_2()
        .py_2()
        .border_t_1()
        .border_b_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap_2()
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
                                .child(label("Canban CLI Skill").font_weight(FontWeight::MEDIUM))
                                .child(badge("Optional")),
                        )
                        .child(
                            muted(
                                "Teach your enabled Agents how to operate Cards, comments, reviews, and workflows via $CANBAN_CLI.",
                            )
                            .text_size(px(12.)),
                        ),
                ),
        )
        .child(canban_skill_box(ui))
}

/// Refresh `ui.agent_catalog` on the effective configuration host.
pub fn refresh_catalog(ui: Ui) {
    let host_id = ui.effective_config_host();
    refresh_catalog_for_host(ui, &host_id);
}

/// Refresh `ui.agent_catalog` for a specific `canband` host (`"local"` or remote `host_id`).
pub fn refresh_catalog_for_host(ui: Ui, host_id: &str) {
    if ui.agent_catalog_loading.get_untracked() {
        return;
    }
    if crate::APP.get().is_none() {
        return;
    }
    let target_host = host_id.to_owned();
    ui.agent_catalog_loading.set(true);
    ui.spawn(async move {
        let req_host = target_host.clone();
        let result = blocking(move || {
            crate::routed_call(
                "agent.catalog",
                json!({"machine": "local", "host_id": req_host}),
                None,
            )
        })
        .await;
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
        let effective_bin = existing
            .and_then(|p| p.command().first().cloned())
            .unwrap_or_else(|| entry.binary.clone());
        rows.push(json!({
            "id": entry.id,
            "display_name": existing.and_then(|p| p.name.clone()).unwrap_or_else(|| entry.display_name.clone()),
            "binary": effective_bin,
            "default_argv": entry.argv,
            "kind": "builtin",
            "resumable": entry.resumable,
            "installed": matches!(entry.id.as_str(), "codex" | "claude"),
            "path": Value::Null,
            "configured": existing.is_some(),
            "enabled": existing.is_some_and(|p| p.enabled),
            "models": entry.default_models,
            "supported_efforts": entry.supported_efforts,
            "supports_model": !entry.model_args.is_empty(),
            "supports_effort": !entry.effort_args.is_empty(),
        }));
    }
    for prof in all_profiles {
        if !prof.is_identity() && !builtin.iter().any(|b| b.id == prof.id) {
            let is_herdr = prof
                .preset
                .as_deref()
                .is_some_and(|s| s.starts_with("herdr:"));
            let default_argv = prof
                .preset
                .as_deref()
                .and_then(|pn| pn.strip_prefix("herdr:"))
                .map(|tpl| vec![tpl.to_owned(), "{prompt}".to_owned()])
                .unwrap_or_default();
            rows.push(json!({
                "id": prof.id,
                "display_name": prof.name.clone().unwrap_or_else(|| prof.id.clone()),
                "binary": prof.command_name(),
                "default_argv": default_argv,
                "kind": "custom",
                "resumable": !prof.resume_command().is_empty(),
                "installed": true,
                "path": Value::Null,
                "configured": true,
                "enabled": prof.enabled,
                "models": [],
                "supported_efforts": [],
                "supports_model": !is_herdr,
                "supports_effort": !is_herdr && prof.command().iter().any(|a| a.contains("{effort}")),
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

#[derive(Clone, Default)]
struct EngineCatalogCaps {
    engine_id: String,
    models: Vec<canban_profiles::ModelOption>,
    efforts: Vec<String>,
    supports_model: bool,
    supports_effort: bool,
}

impl EngineCatalogCaps {
    /// Resolve the supported effort levels for `model_id` (`None` when this engine or the
    /// chosen model does not support `--effort`).
    fn efforts_for_model(&self, model_id: Option<&str>) -> Option<Vec<String>> {
        if !self.supports_effort {
            return None;
        }
        let default_efforts = if self.efforts.is_empty() {
            vec!["low".into(), "medium".into(), "high".into()]
        } else {
            self.efforts.clone()
        };
        let Some(mid) = model_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return Some(default_efforts);
        };
        if let Some(opt) = self.models.iter().find(|m| m.id == mid)
            && let Some(levels) = &opt.supported_efforts
        {
            return (!levels.is_empty()).then(|| levels.clone());
        }
        if let Some(entry) = canban_profiles::builtin_catalog()
            .into_iter()
            .find(|e| e.id == self.engine_id || e.binary == self.engine_id)
        {
            let resolved =
                canban_profiles::model_supported_efforts(&entry, &self.models, Some(mid))?;
            return (!resolved.is_empty()).then_some(resolved);
        }
        Some(default_efforts)
    }

    fn cleaned_effort_for_model(
        &self,
        model_id: Option<&str>,
        current_effort: Option<&str>,
    ) -> Option<Value> {
        let cur = current_effort.map(str::trim).filter(|s| !s.is_empty())?;
        match self.efforts_for_model(model_id) {
            None => Some(Value::Null),
            Some(levels) if !levels.iter().any(|l| l == cur) => Some(Value::Null),
            Some(_) => None,
        }
    }
}

fn engine_key_for_profile(profile: &Profile) -> String {
    if let Some(preset) = profile.preset.as_deref() {
        return preset.to_owned();
    }
    if profile.is_identity() {
        return profile.command_name();
    }
    profile.id.clone()
}

fn catalog_caps_for_engine(rows: &[Value], engine_key: &str) -> EngineCatalogCaps {
    if engine_key.starts_with("herdr:") {
        return EngineCatalogCaps::default();
    }
    if let Some(row) = rows
        .iter()
        .find(|r| r["id"].as_str() == Some(engine_key) || r["binary"].as_str() == Some(engine_key))
    {
        let engine_id = row["id"].as_str().unwrap_or(engine_key).to_owned();
        let models: Vec<canban_profiles::ModelOption> = row["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let id = v["id"].as_str()?.trim();
                        if id.is_empty() {
                            return None;
                        }
                        let label = v["label"].as_str().unwrap_or(id).trim();
                        let supported_efforts = v.get("supported_efforts").and_then(|se| {
                            se.as_array().map(|list| {
                                list.iter()
                                    .filter_map(|x| x.as_str().map(str::trim))
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned)
                                    .collect()
                            })
                        });
                        Some(canban_profiles::ModelOption {
                            id: id.to_owned(),
                            label: if label.is_empty() {
                                id.to_owned()
                            } else {
                                label.to_owned()
                            },
                            supported_efforts,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let efforts: Vec<String> = row["supported_efforts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::trim))
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let supports_model = row["supports_model"].as_bool().unwrap_or(true);
        let supports_effort = row["supports_effort"]
            .as_bool()
            .unwrap_or(!efforts.is_empty());
        return EngineCatalogCaps {
            engine_id,
            models,
            efforts,
            supports_model,
            supports_effort,
        };
    }
    EngineCatalogCaps {
        engine_id: engine_key.to_owned(),
        models: Vec::new(),
        efforts: vec!["low".into(), "medium".into(), "high".into()],
        supports_model: true,
        supports_effort: false,
    }
}

fn raw_profile_options(
    catalog_val: &Value,
    profile_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    let arr = catalog_val["raw_profiles"].as_array()?;
    let item = arr.iter().find(|v| v["id"].as_str() == Some(profile_id))?;
    let m = item["model"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let e = item["effort"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some((m, e))
}

fn preset_default_argv(rows: &[Value], profile: &Profile) -> Option<Vec<String>> {
    if let Some(row) = rows
        .iter()
        .find(|r| r["id"].as_str() == Some(profile.id.as_str()))
        && let Some(arr) = row["default_argv"].as_array()
    {
        let parsed: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        if !parsed.is_empty() {
            return Some(parsed);
        }
    }
    if let Some(preset) = profile.preset.as_deref() {
        if let Some(tpl) = preset.strip_prefix("herdr:") {
            return Some(vec![tpl.to_owned(), "{prompt}".to_owned()]);
        }
        if let Some(entry) = canban_profiles::builtin_catalog()
            .into_iter()
            .find(|e| e.id == preset)
        {
            return Some(entry.argv);
        }
    }
    None
}

fn raw_profile_has_custom_argv(catalog_val: &Value, rows: &[Value], profile: &Profile) -> bool {
    let Some(default_argv) = preset_default_argv(rows, profile) else {
        return false;
    };
    if let Some(arr) = catalog_val["raw_profiles"].as_array()
        && let Some(item) = arr
            .iter()
            .find(|v| v["id"].as_str() == Some(profile.id.as_str()))
    {
        if let Some(raw_argv) = item["argv"].as_array() {
            let parsed: Vec<String> = raw_argv
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            return !parsed.is_empty() && parsed != default_argv;
        }
        return false;
    }
    let cur = profile.command();
    !cur.is_empty() && cur != default_argv.as_slice()
}

const COMMAND_VARIABLES_HINT: &str = "Variables: {prompt} {cwd} {project_dir} {home} {session_id} {model} {effort} {cli} {agy_trusted_settings}";

fn assemble_command_argv(command_raw: &str, args_raw: &str) -> Result<Vec<String>, String> {
    let cmd = command_raw.trim();
    if cmd.is_empty() {
        return Err("Command cannot be empty".into());
    }
    let mut argv = vec![cmd.to_owned()];
    let rest = canban_profiles::split_posix_words(args_raw).map_err(|e| format!("{e:#}"))?;
    argv.extend(rest);
    Ok(argv)
}

#[allow(clippy::too_many_arguments)]
fn agent_command_inline_editor(
    ui: Ui,
    profile_id: String,
    preset_default: Option<Vec<String>>,
    command_target: Signal<Option<String>>,
    command_bin_draft: Signal<String>,
    command_args_draft: Signal<String>,
    command_error: Signal<String>,
) -> AnyElement {
    let save_id = profile_id.clone();
    let reset_id = profile_id;
    let has_preset = preset_default.is_some();
    let preset_for_reset = preset_default;

    view()
        .gap_2()
        .pt_1p5()
        .pb_0p5()
        .pl(px(23.))
        .child(
            view()
                .gap_1()
                .child(muted("Command").text_size(px(12.)))
                .child(
                    bound_field(
                        move || command_bin_draft.get(),
                        move |v| {
                            command_error.set(String::new());
                            command_bin_draft.set(v.to_owned());
                        },
                        "Executable name or path (e.g. agy or /bin/agy-modified)",
                    )
                    .font_family(MONO_FONT)
                    .label("Command"),
                ),
        )
        .child(
            view()
                .gap_1()
                .child(muted("Argv").text_size(px(12.)))
                .child(
                    bound_field(
                        move || command_args_draft.get(),
                        move |v| {
                            command_error.set(String::new());
                            command_args_draft.set(v.to_owned());
                        },
                        "Arguments template (e.g. --add-dir {cwd} -i {prompt})",
                    )
                    .font_family(MONO_FONT)
                    .label("Argv"),
                ),
        )
        .child(when(
            move || !command_error.get().is_empty(),
            move || notice(move || command_error.get(), Tone::Attention),
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    muted(COMMAND_VARIABLES_HINT)
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .min_w(px(0.))
                        .flex_1(),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(if has_preset {
                            compact_action("Reset", Variant::Quiet, move |_| {
                                let id_c = reset_id.clone();
                                let def_c = preset_for_reset.clone();
                                if crate::APP.get().is_none() {
                                    if let Some(def) = def_c {
                                        let next_bin = def.first().cloned().unwrap_or_default();
                                        let next_args = if def.len() > 1 {
                                            canban_profiles::join_posix_words(&def[1..])
                                        } else {
                                            String::new()
                                        };
                                        command_bin_draft.set(next_bin);
                                        command_args_draft.set(next_args);
                                        command_error.set(String::new());
                                        ui.all_profiles.update(|list| {
                                            if let Some(p) = list.iter_mut().find(|x| x.id == id_c)
                                            {
                                                p.argv = Some(def.clone());
                                            }
                                        });
                                        ui.profiles.update(|list| {
                                            if let Some(p) = list.iter_mut().find(|x| x.id == id_c)
                                            {
                                                p.argv = Some(def);
                                            }
                                        });
                                        ui.agent_catalog.update(|cat| {
                                            if let Some(arr) = cat["raw_profiles"].as_array_mut()
                                                && let Some(item) = arr.iter_mut().find(|v| {
                                                    v["id"].as_str() == Some(id_c.as_str())
                                                })
                                                && let Some(obj) = item.as_object_mut()
                                            {
                                                obj.remove("argv");
                                            }
                                        });
                                    }
                                    command_target.set(None);
                                    return;
                                }
                                dispatch_then(
                                    ui,
                                    "config.agents.save",
                                    json!({
                                        "op": "set_profile_options",
                                        "id": id_c,
                                        "argv": Value::Null,
                                    }),
                                    move |_| {
                                        command_error.set(String::new());
                                        command_target.set(None);
                                        refresh_catalog(ui);
                                    },
                                );
                            })
                            .into_element()
                        } else {
                            view().into_element()
                        })
                        .child(compact_action("Save", Variant::Primary, move |_| {
                            let bin_val = command_bin_draft.get_untracked();
                            let args_val = command_args_draft.get_untracked();
                            let argv = match assemble_command_argv(&bin_val, &args_val) {
                                Ok(v) => v,
                                Err(msg) => {
                                    command_error.set(msg);
                                    return;
                                }
                            };
                            let id_c = save_id.clone();
                            if crate::APP.get().is_none() {
                                ui.all_profiles.update(|list| {
                                    if let Some(p) = list.iter_mut().find(|x| x.id == id_c) {
                                        p.argv = Some(argv.clone());
                                    }
                                });
                                ui.profiles.update(|list| {
                                    if let Some(p) = list.iter_mut().find(|x| x.id == id_c) {
                                        p.argv = Some(argv.clone());
                                    }
                                });
                                ui.agent_catalog.update(|cat| {
                                    if let Some(arr) = cat["raw_profiles"].as_array_mut() {
                                        if let Some(item) = arr
                                            .iter_mut()
                                            .find(|v| v["id"].as_str() == Some(id_c.as_str()))
                                        {
                                            item["argv"] = json!(argv);
                                        } else {
                                            arr.push(json!({ "id": id_c, "argv": argv }));
                                        }
                                    }
                                });
                                command_error.set(String::new());
                                command_target.set(None);
                                return;
                            }
                            dispatch_then(
                                ui,
                                "config.agents.save",
                                json!({
                                    "op": "set_profile_options",
                                    "id": id_c,
                                    "argv": argv,
                                }),
                                move |_| {
                                    command_error.set(String::new());
                                    command_target.set(None);
                                    refresh_catalog(ui);
                                },
                            );
                        })),
                ),
        )
        .into_element()
}

fn short_model_badge(model_id: &str) -> String {
    if model_id.chars().count() > 24 {
        format!("{}…", model_id.chars().take(23).collect::<String>())
    } else {
        model_id.to_owned()
    }
}

#[allow(clippy::too_many_arguments)]
fn profile_model_effort_buttons(
    ui: Ui,
    profile_id: String,
    effective_model: Option<String>,
    explicit_model: Option<String>,
    effective_effort: Option<String>,
    explicit_effort: Option<String>,
    is_identity: bool,
    caps: EngineCatalogCaps,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
) -> AnyElement {
    let active_model_id = explicit_model.as_deref().or(effective_model.as_deref());
    let model_efforts = caps.efforts_for_model(active_model_id);
    if !caps.supports_model && model_efforts.is_none() {
        return view().into_element();
    }

    let model_btn = if caps.supports_model {
        let btn_label = match (&explicit_model, &effective_model) {
            (Some(m), _) => format!("{} ▾", short_model_badge(m)),
            (None, Some(inherited)) if is_identity => {
                format!("{} (auto) ▾", short_model_badge(inherited))
            }
            _ => "Model: Default ▾".into(),
        };
        let pid_m = profile_id.clone();
        let models_list = caps.models.clone();
        let caps_for_menu = caps.clone();
        let cur_explicit_m = explicit_model.clone();
        let cur_effective_m = effective_model.clone();
        let cur_explicit_e_for_model = explicit_effort.clone();
        compact_action(
            btn_label,
            if explicit_model.is_some() {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| {
                if models_list.is_empty() {
                    let seed = cur_explicit_m
                        .clone()
                        .or_else(|| cur_effective_m.clone())
                        .unwrap_or_default();
                    if custom_model_target.get_untracked().as_deref() == Some(pid_m.as_str()) {
                        custom_model_target.set(None);
                    } else {
                        custom_model_draft.set(seed);
                        custom_model_target.set(Some(pid_m.clone()));
                    }
                    return;
                }
                use nsh_core::element::desktop::{MenuItem, popup_menu};
                let opts = models_list.clone();
                let caps_m = caps_for_menu.clone();
                let p_id = pid_m.clone();
                let exp_m = cur_explicit_m.clone();
                let eff_m = cur_effective_m.clone();
                let exp_e_m = cur_explicit_e_for_model.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_label = if exp_m.is_none() {
                        if is_identity {
                            "✓ Default (Inherit from Agent)"
                        } else {
                            "✓ Default (CLI default)"
                        }
                    } else if is_identity {
                        "Default (Inherit from Agent)"
                    } else {
                        "Default (CLI default)"
                    };
                    let clear_id = p_id.clone();
                    items.push(MenuItem::new(def_label).on_select(move || {
                        custom_model_target.set(None);
                        dispatch_then(
                            ui,
                            "config.agents.save",
                            json!({
                                "op": "set_profile_options",
                                "id": clear_id,
                                "model": Value::Null,
                            }),
                            move |_| refresh_catalog(ui),
                        );
                    }));
                    items.push(MenuItem::separator());
                    for opt in opts {
                        let mid = opt.id;
                        let mlabel = opt.label;
                        let checked = exp_m.as_deref() == Some(mid.as_str());
                        let prefix = if checked { "✓ " } else { "" };
                        let title = if mlabel == mid {
                            format!("{prefix}{mid}")
                        } else {
                            format!("{prefix}{mlabel} ({mid})")
                        };
                        let set_id = p_id.clone();
                        let chosen = mid.clone();
                        let cleaned_eff =
                            caps_m.cleaned_effort_for_model(Some(&chosen), exp_e_m.as_deref());
                        items.push(MenuItem::new(title).on_select(move || {
                            custom_model_target.set(None);
                            let mut payload = json!({
                                "op": "set_profile_options",
                                "id": set_id,
                                "model": chosen,
                            });
                            if let Some(clear_eff) = &cleaned_eff {
                                payload["effort"] = clear_eff.clone();
                            }
                            dispatch_then(ui, "config.agents.save", payload, move |_| {
                                refresh_catalog(ui)
                            });
                        }));
                    }
                    items.push(MenuItem::separator());
                    let custom_id = p_id.clone();
                    let seed = exp_m.or(eff_m).unwrap_or_default();
                    items.push(MenuItem::new("Custom model ID…").on_select(move || {
                        custom_model_draft.set(seed.clone());
                        custom_model_target.set(Some(custom_id.clone()));
                    }));
                    popup_menu(items).await;
                });
            },
        )
        .into_element()
    } else {
        view().into_element()
    };

    let effort_btn = if let Some(efforts_list) = model_efforts {
        let valid_explicit_effort = explicit_effort
            .as_ref()
            .filter(|e| efforts_list.iter().any(|l| l == *e))
            .cloned();
        let valid_effective_effort = effective_effort
            .as_ref()
            .filter(|e| efforts_list.iter().any(|l| l == *e))
            .cloned();
        let eff_label = match (&valid_explicit_effort, &valid_effective_effort) {
            (Some(e), _) => format!("Effort: {e} ▾"),
            (None, Some(inherited)) if is_identity => format!("Effort: {inherited} (auto) ▾"),
            _ => "Effort: Default ▾".into(),
        };
        let pid_e = profile_id;
        let cur_explicit_e = valid_explicit_effort.clone();
        compact_action(
            eff_label,
            if valid_explicit_effort.is_some() {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| {
                use nsh_core::element::desktop::{MenuItem, popup_menu};
                let opts = efforts_list.clone();
                let p_id = pid_e.clone();
                let exp_e = cur_explicit_e.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_label = if exp_e.is_none() {
                        if is_identity {
                            "✓ Default (Inherit from Agent)"
                        } else {
                            "✓ Default (CLI default)"
                        }
                    } else if is_identity {
                        "Default (Inherit from Agent)"
                    } else {
                        "Default (CLI default)"
                    };
                    let clear_id = p_id.clone();
                    items.push(MenuItem::new(def_label).on_select(move || {
                        dispatch_then(
                            ui,
                            "config.agents.save",
                            json!({
                                "op": "set_profile_options",
                                "id": clear_id,
                                "effort": Value::Null,
                            }),
                            move |_| refresh_catalog(ui),
                        );
                    }));
                    items.push(MenuItem::separator());
                    for eff in opts {
                        let checked = exp_e.as_deref() == Some(eff.as_str());
                        let title = if checked {
                            format!("✓ {eff}")
                        } else {
                            eff.clone()
                        };
                        let set_id = p_id.clone();
                        let chosen = eff.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            dispatch_then(
                                ui,
                                "config.agents.save",
                                json!({
                                    "op": "set_profile_options",
                                    "id": set_id,
                                    "effort": chosen,
                                }),
                                move |_| refresh_catalog(ui),
                            );
                        }));
                    }
                    popup_menu(items).await;
                });
            },
        )
        .into_element()
    } else {
        view().into_element()
    };

    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(model_btn)
        .child(effort_btn)
        .into_element()
}

fn custom_model_inline_editor(
    ui: Ui,
    profile_id: String,
    explicit_effort: Option<String>,
    caps: EngineCatalogCaps,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
) -> AnyElement {
    let save_id = profile_id.clone();
    let clear_id = profile_id;
    view()
        .flex_row()
        .flex_wrap()
        .items_end()
        .gap_2()
        .pt_1()
        .child(
            view().flex_1().min_w(px(220.)).child(
                bound_field(
                    move || custom_model_draft.get(),
                    move |v| custom_model_draft.set(v.to_owned()),
                    "e.g. gemini-4-carbon-high, gpt-6.1-sol, claude-opus-4-6…",
                )
                .label("Custom model ID"),
            ),
        )
        .child(compact_action("Save model", Variant::Primary, move |_| {
            let val = custom_model_draft.get_untracked();
            let trimmed = val.trim().to_owned();
            let id_c = save_id.clone();
            let cleaned_eff = if trimmed.is_empty() {
                None
            } else {
                caps.cleaned_effort_for_model(Some(&trimmed), explicit_effort.as_deref())
            };
            let mut payload = json!({
                "op": "set_profile_options",
                "id": id_c,
                "model": if trimmed.is_empty() { Value::Null } else { json!(trimmed) },
            });
            if let Some(clear_eff) = cleaned_eff {
                payload["effort"] = clear_eff;
            }
            dispatch_then(ui, "config.agents.save", payload, move |_| {
                custom_model_target.set(None);
                refresh_catalog(ui);
            });
        }))
        .child(compact_action("Reset default", Variant::Quiet, move |_| {
            let id_c = clear_id.clone();
            dispatch_then(
                ui,
                "config.agents.save",
                json!({
                    "op": "set_profile_options",
                    "id": id_c,
                    "model": Value::Null,
                }),
                move |_| {
                    custom_model_target.set(None);
                    refresh_catalog(ui);
                },
            );
        }))
        .child(compact_action("Cancel", Variant::Quiet, move |_| {
            custom_model_target.set(None);
        }))
        .into_element()
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
        "merger" => "Merge queue & conflict resolution".into(),
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
    canban_ui_kit::press_target()
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
        refresh_catalog(ui);
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
        for role in ["planner", "developer", "reviewer", "merger"] {
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
                                refresh_catalog(ui);
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
                            "Powers @planner, @developer, @reviewer, and @merger unless customized below or overridden in a Project.",
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
                                                    for role in [
                                                        "planner",
                                                        "developer",
                                                        "reviewer",
                                                        "merger",
                                                    ] {
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
                                                ("merger", "Merge queue & conflict resolution"),
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
                    .child(canban_skill_onboarding_section(ui))
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
                                            refresh_catalog(ui);
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
        refresh_catalog(ui);
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
    let custom_agent_args = signal(String::new());
    let custom_agent_error = signal(String::new());
    let custom_model_target = signal(Option::<String>::None);
    let custom_model_draft = signal(String::new());
    let command_target = signal(Option::<String>::None);
    let command_bin_draft = signal(String::new());
    let command_args_draft = signal(String::new());
    let command_error = signal(String::new());

    let copy_path = config_path.clone();

    settings_section(
        "Agents & Identities",
        "Manage enabled Herdr CLI Agents, @identities, and the default interactive Agent.",
    )
    .trailing(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(when(
                move || ui.settings_host.get() != "local",
                move || {
                    compact_action(
                        "Copy profiles from This Mac",
                        Variant::Secondary,
                        move |_| {
                            let target = ui.settings_host.get_untracked();
                            let local_cfg = crate::config_for_host("local");
                            dispatch_then(
                                ui,
                                "config.agents.save",
                                json!({
                                    "host_id": target,
                                    "op": "replace_profiles",
                                    "profiles": local_cfg.profiles,
                                    "default_agent": local_cfg.default_agent,
                                }),
                                move |_| {
                                    refresh_catalog(ui);
                                    ui.notify("Copied Agent & @identity profiles from This Mac");
                                },
                            );
                        },
                    )
                },
            ))
            .child(compact_action(
                "Run Setup…",
                Variant::Secondary,
                move |_| ui.navigate("agent-setup"),
            ))
            .child(compact_action("Reload", Variant::Quiet, move |_| {
                dispatch_then(ui, "config.reload", json!({}), move |_| {
                    refresh_catalog(ui);
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
    .child(canban_skill_settings_block(ui))
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
                                                refresh_catalog(ui);
                                            },
                                        );
                                    },
                                )),
                        )
                },
            ))
            .child(dynamic(move || {
                let all = ui.all_profiles.get();
                let cat_val = ui.agent_catalog.get();
                let cat_rows = catalog_agents(ui);
                let engines = enabled_engine_options(ui);
                let identities: Vec<Profile> =
                    all.into_iter().filter(|p| p.is_identity()).collect();
                let active_edit = editing_role.get();
                let active_custom_model = custom_model_target.get();
                view()
                    .border_t_1()
                    .border_color(BORDER)
                    .children(identities.into_iter().map(|prof| {
                        let pid = prof.id.clone();
                        let (mark, prov_name) = crate::profiles::provider(&prof);
                        let is_builtin_role = matches!(
                            pid.as_str(),
                            "planner" | "developer" | "reviewer" | "merger"
                        );
                        let is_editing = active_edit.as_deref() == Some(pid.as_str());
                        let is_editing_custom_model =
                            active_custom_model.as_deref() == Some(pid.as_str());
                        let eng_key = engine_key_for_profile(&prof);
                        let caps = catalog_caps_for_engine(&cat_rows, &eng_key);
                        let (explicit_model, explicit_effort) =
                            raw_profile_options(&cat_val, &pid)
                                .unwrap_or_else(|| (prof.model.clone(), prof.effort.clone()));
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
                                            .flex_wrap()
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
                                                                    dispatch_then(
                                                                        ui,
                                                                        "config.agents.save",
                                                                        json!({
                                                                            "op": "upsert_identity",
                                                                            "id": r,
                                                                            "engine": eid,
                                                                            "enabled": true,
                                                                        }),
                                                                        move |_| {
                                                                            refresh_catalog(ui)
                                                                        },
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
                                            .child(profile_model_effort_buttons(
                                                ui,
                                                pid.clone(),
                                                prof.model.clone(),
                                                explicit_model,
                                                prof.effort.clone(),
                                                explicit_effort.clone(),
                                                true,
                                                caps.clone(),
                                                custom_model_target,
                                                custom_model_draft,
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
                                                    dispatch_then(
                                                        ui,
                                                        "config.agents.save",
                                                        json!({
                                                            "op": "toggle_agent",
                                                            "id": toggle_pid,
                                                            "enabled": !prof_enabled,
                                                        }),
                                                        move |_| refresh_catalog(ui),
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
                                                            dispatch_then(
                                                                ui,
                                                                "config.agents.save",
                                                                json!({
                                                                    "op": "delete_profile",
                                                                    "id": d,
                                                                }),
                                                                move |_| {
                                                                    refresh_catalog(ui)
                                                                },
                                                            );
                                                        },
                                                    )
                                                },
                                            )),
                                    ),
                            )
                            .child(if is_editing_custom_model {
                                custom_model_inline_editor(
                                    ui,
                                    pid.clone(),
                                    explicit_effort,
                                    caps,
                                    custom_model_target,
                                    custom_model_draft,
                                )
                            } else {
                                view().into_element()
                            })
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
                                                move |_| {
                                                    editing_role.set(None);
                                                    refresh_catalog(ui);
                                                },
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
                        move |_| {
                            custom_agent_error.set(String::new());
                            if !adding_custom_agent.get_untracked()
                                && custom_agent_args.get_untracked().is_empty()
                            {
                                custom_agent_args.set("{prompt}".into());
                            }
                            adding_custom_agent.update(|v| *v = !*v);
                        },
                    )),
            )
            .child(when(
                move || adding_custom_agent.get(),
                move || {
                    panel()
                        .gap_2p5()
                        .child(
                            view()
                                .gap_1()
                                .child(muted("Agent ID").text_size(px(12.)))
                                .child(
                                    bound_field(
                                        move || custom_agent_id.get(),
                                        move |v| {
                                            custom_agent_error.set(String::new());
                                            custom_agent_id.set(v.to_owned());
                                        },
                                        "Agent ID (e.g. agy-modified)",
                                    )
                                    .label("Agent ID"),
                                ),
                        )
                        .child(
                            view()
                                .gap_1()
                                .child(muted("Command").text_size(px(12.)))
                                .child(
                                    bound_field(
                                        move || custom_agent_cmd.get(),
                                        move |v| {
                                            custom_agent_error.set(String::new());
                                            custom_agent_cmd.set(v.to_owned());
                                        },
                                        "Executable name or path (e.g. /bin/agy-modified)",
                                    )
                                    .font_family(MONO_FONT)
                                    .label("Command"),
                                ),
                        )
                        .child(
                            view()
                                .gap_1()
                                .child(muted("Argv").text_size(px(12.)))
                                .child(
                                    bound_field(
                                        move || custom_agent_args.get(),
                                        move |v| {
                                            custom_agent_error.set(String::new());
                                            custom_agent_args.set(v.to_owned());
                                        },
                                        "Arguments template (e.g. --add-dir {cwd} -i {prompt})",
                                    )
                                    .font_family(MONO_FONT)
                                    .label("Argv"),
                                ),
                        )
                        .child(when(
                            move || !custom_agent_error.get().is_empty(),
                            move || notice(move || custom_agent_error.get(), Tone::Attention),
                        ))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    muted(COMMAND_VARIABLES_HINT)
                                        .font_family(MONO_FONT)
                                        .text_size(px(11.))
                                        .min_w(px(0.))
                                        .flex_1(),
                                )
                                .child(compact_action(
                                    "Save Custom Agent",
                                    Variant::Primary,
                                    move |_| {
                                        let id = custom_agent_id.get_untracked().trim().to_owned();
                                        if id.is_empty() {
                                            custom_agent_error.set("Agent ID cannot be empty".into());
                                            return;
                                        }
                                        let argv = match assemble_command_argv(
                                            &custom_agent_cmd.get_untracked(),
                                            &custom_agent_args.get_untracked(),
                                        ) {
                                            Ok(v) => v,
                                            Err(msg) => {
                                                custom_agent_error.set(msg);
                                                return;
                                            }
                                        };
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
                                                custom_agent_args.set(String::new());
                                                custom_agent_error.set(String::new());
                                                refresh_catalog(ui);
                                            },
                                        );
                                    },
                                )),
                        )
                },
            ))
            .child(dynamic(move || {
                let rows = catalog_agents(ui);
                let all_profs = ui.all_profiles.get();
                let cat_val = ui.agent_catalog.get();
                let active_custom_model = custom_model_target.get();
                let active_command = command_target.get();
                let enabled_direct: Vec<Profile> = all_profs
                    .into_iter()
                    .filter(|p| !p.is_identity() && p.enabled)
                    .collect();

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
                            move |_| refresh_catalog(ui),
                        );
                    })
                };

                let count = extra_rows.len();
                let show_extra = show_all_catalog.get();
                view()
                    .gap_3()
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
                    .child(if enabled_direct.is_empty() {
                        view().into_element()
                    } else {
                        view()
                            .gap_1()
                            .pt_1()
                            .child(
                                muted("Agent defaults (inherited by @identities unless overridden above):")
                                    .text_size(px(12.)),
                            )
                            .child(
                                view()
                                    .border_t_1()
                                    .border_color(BORDER)
                                    .children(enabled_direct.into_iter().map(|prof| {
                                        let pid = prof.id.clone();
                                        let (mark, prov_name) = crate::profiles::provider(&prof);
                                        let eng_key = engine_key_for_profile(&prof);
                                        let caps = catalog_caps_for_engine(&rows, &eng_key);
                                        let (explicit_model, explicit_effort) =
                                            raw_profile_options(&cat_val, &pid).unwrap_or_else(
                                                || (prof.model.clone(), prof.effort.clone()),
                                            );
                                        let is_editing_custom =
                                            active_custom_model.as_deref() == Some(pid.as_str());
                                        let is_editing_command =
                                            active_command.as_deref() == Some(pid.as_str());
                                        let preset_default = preset_default_argv(&rows, &prof);
                                        let has_custom_cmd =
                                            raw_profile_has_custom_argv(&cat_val, &rows, &prof);
                                        let is_pure_custom = rows
                                            .iter()
                                            .find(|r| r["id"].as_str() == Some(pid.as_str()))
                                            .is_some_and(|r| r["kind"].as_str() == Some("custom"));
                                        let effective_argv: Vec<String> = if !prof.command().is_empty() {
                                            prof.command().to_vec()
                                        } else {
                                            preset_default.clone().unwrap_or_default()
                                        };
                                        let seed_bin = effective_argv
                                            .first()
                                            .cloned()
                                            .unwrap_or_else(|| pid.clone());
                                        let seed_args = if effective_argv.len() > 1 {
                                            canban_profiles::join_posix_words(&effective_argv[1..])
                                        } else {
                                            String::new()
                                        };
                                        let cmd_pid = pid.clone();
                                        let del_pid = pid.clone();

                                        view()
                                            .py_2()
                                            .border_b_1()
                                            .border_color(BORDER)
                                            .gap_1p5()
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
                                                            .gap_2()
                                                            .child(
                                                                icon_tinted(mark, TEXT)
                                                                    .w(px(15.))
                                                                    .h(px(15.)),
                                                            )
                                                            .child(
                                                                label(format!("@{pid}"))
                                                                    .font_weight(FontWeight::MEDIUM),
                                                            )
                                                            .child(
                                                                muted(prov_name).text_size(px(12.)),
                                                            )
                                                            .child(if has_custom_cmd {
                                                                badge("Custom").into_element()
                                                            } else {
                                                                view().into_element()
                                                            }),
                                                    )
                                                    .child(
                                                        view()
                                                            .flex_row()
                                                            .flex_wrap()
                                                            .items_center()
                                                            .gap_1p5()
                                                            .child(profile_model_effort_buttons(
                                                                ui,
                                                                pid.clone(),
                                                                prof.model.clone(),
                                                                explicit_model,
                                                                prof.effort.clone(),
                                                                explicit_effort.clone(),
                                                                false,
                                                                caps.clone(),
                                                                custom_model_target,
                                                                custom_model_draft,
                                                            ))
                                                            .child(compact_action(
                                                                if is_editing_command {
                                                                    "▾ Command"
                                                                } else {
                                                                    "▸ Command"
                                                                },
                                                                if is_editing_command || has_custom_cmd {
                                                                    Variant::Secondary
                                                                } else {
                                                                    Variant::Quiet
                                                                },
                                                                move |_| {
                                                                    command_error.set(String::new());
                                                                    if command_target
                                                                        .get_untracked()
                                                                        .as_deref()
                                                                        == Some(cmd_pid.as_str())
                                                                    {
                                                                        command_target.set(None);
                                                                    } else {
                                                                        command_bin_draft
                                                                            .set(seed_bin.clone());
                                                                        command_args_draft
                                                                            .set(seed_args.clone());
                                                                        command_target.set(Some(
                                                                            cmd_pid.clone(),
                                                                        ));
                                                                    }
                                                                },
                                                            ))
                                                            .child(when(
                                                                move || is_pure_custom,
                                                                move || {
                                                                    let d = del_pid.clone();
                                                                    compact_action(
                                                                        "Delete",
                                                                        Variant::Danger,
                                                                        move |_| {
                                                                            dispatch_then(
                                                                                ui,
                                                                                "config.agents.save",
                                                                                json!({
                                                                                    "op": "delete_profile",
                                                                                    "id": d,
                                                                                }),
                                                                                move |_| {
                                                                                    refresh_catalog(
                                                                                        ui,
                                                                                    )
                                                                                },
                                                                            );
                                                                        },
                                                                    )
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .child(if is_editing_custom {
                                                custom_model_inline_editor(
                                                    ui,
                                                    pid.clone(),
                                                    explicit_effort,
                                                    caps,
                                                    custom_model_target,
                                                    custom_model_draft,
                                                )
                                            } else {
                                                view().into_element()
                                            })
                                            .child(if is_editing_command {
                                                agent_command_inline_editor(
                                                    ui,
                                                    pid,
                                                    preset_default,
                                                    command_target,
                                                    command_bin_draft,
                                                    command_args_draft,
                                                    command_error,
                                                )
                                            } else {
                                                view().into_element()
                                            })
                                            .into_element()
                                    })),
                            )
                            .into_element()
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

#[allow(clippy::too_many_arguments)]
fn project_model_effort_buttons(
    ui: Ui,
    role_id: String,
    inherited_model: Option<String>,
    explicit_model: Option<String>,
    inherited_effort: Option<String>,
    explicit_effort: Option<String>,
    caps: EngineCatalogCaps,
    overrides: Signal<BTreeMap<String, ProjectAgentOverride>>,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
) -> AnyElement {
    let active_model_id = explicit_model.as_deref().or(inherited_model.as_deref());
    let model_efforts = caps.efforts_for_model(active_model_id);
    if !caps.supports_model && model_efforts.is_none() {
        return view().into_element();
    }

    let model_btn = if caps.supports_model {
        let btn_label = match (&explicit_model, &inherited_model) {
            (Some(m), _) => format!("{} ▾", short_model_badge(m)),
            (None, Some(inherited)) => format!("{} (auto) ▾", short_model_badge(inherited)),
            _ => "Model: Default ▾".into(),
        };
        let pid_m = role_id.clone();
        let models_list = caps.models.clone();
        let caps_for_menu = caps.clone();
        let cur_explicit_m = explicit_model.clone();
        let cur_inherited_m = inherited_model.clone();
        compact_action(
            btn_label,
            if explicit_model.is_some() {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| {
                if models_list.is_empty() {
                    let seed = cur_explicit_m
                        .clone()
                        .or_else(|| cur_inherited_m.clone())
                        .unwrap_or_default();
                    if custom_model_target.get_untracked().as_deref() == Some(pid_m.as_str()) {
                        custom_model_target.set(None);
                    } else {
                        custom_model_draft.set(seed);
                        custom_model_target.set(Some(pid_m.clone()));
                    }
                    return;
                }
                use nsh_core::element::desktop::{MenuItem, popup_menu};
                let opts = models_list.clone();
                let caps_m = caps_for_menu.clone();
                let p_id = pid_m.clone();
                let exp_m = cur_explicit_m.clone();
                let inh_m = cur_inherited_m.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_base = match inh_m.as_deref() {
                        Some(m) => format!("Default (Inherit: {})", short_model_badge(m)),
                        None => "Default (Inherit Global)".into(),
                    };
                    let def_label = if exp_m.is_none() {
                        format!("✓ {def_base}")
                    } else {
                        def_base
                    };
                    let clear_id = p_id.clone();
                    items.push(MenuItem::new(def_label).on_select(move || {
                        custom_model_target.set(None);
                        let k = clear_id.clone();
                        overrides.update(|m| {
                            if let Some(ov) = m.get_mut(&k) {
                                ov.model = None;
                                if ov.is_empty() {
                                    m.remove(&k);
                                }
                            }
                        });
                    }));
                    items.push(MenuItem::separator());
                    for opt in opts {
                        let mid = opt.id;
                        let mlabel = opt.label;
                        let checked = exp_m.as_deref() == Some(mid.as_str());
                        let prefix = if checked { "✓ " } else { "" };
                        let title = if mlabel == mid {
                            format!("{prefix}{mid}")
                        } else {
                            format!("{prefix}{mlabel} ({mid})")
                        };
                        let set_id = p_id.clone();
                        let chosen = mid.clone();
                        let caps_opt = caps_m.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            custom_model_target.set(None);
                            let k = set_id.clone();
                            let v = chosen.clone();
                            overrides.update(|m| {
                                let entry = m.entry(k).or_default();
                                if caps_opt
                                    .cleaned_effort_for_model(Some(&v), entry.effort.as_deref())
                                    .is_some()
                                {
                                    entry.effort = None;
                                }
                                entry.model = Some(v);
                            });
                        }));
                    }
                    items.push(MenuItem::separator());
                    let custom_id = p_id.clone();
                    let seed = exp_m.or(inh_m).unwrap_or_default();
                    items.push(MenuItem::new("Custom model ID…").on_select(move || {
                        custom_model_draft.set(seed.clone());
                        custom_model_target.set(Some(custom_id.clone()));
                    }));
                    popup_menu(items).await;
                });
            },
        )
        .into_element()
    } else {
        view().into_element()
    };

    let effort_btn = if let Some(efforts_list) = model_efforts {
        let valid_explicit_effort = explicit_effort
            .as_ref()
            .filter(|e| efforts_list.iter().any(|l| l == *e))
            .cloned();
        let valid_inherited_effort = inherited_effort
            .as_ref()
            .filter(|e| efforts_list.iter().any(|l| l == *e))
            .cloned();
        let eff_label = match (&valid_explicit_effort, &valid_inherited_effort) {
            (Some(e), _) => format!("Effort: {e} ▾"),
            (None, Some(inherited)) => format!("Effort: {inherited} (auto) ▾"),
            _ => "Effort: Default ▾".into(),
        };
        let pid_e = role_id;
        let cur_explicit_e = valid_explicit_effort.clone();
        let cur_inherited_e = valid_inherited_effort.clone();
        compact_action(
            eff_label,
            if valid_explicit_effort.is_some() {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| {
                use nsh_core::element::desktop::{MenuItem, popup_menu};
                let opts = efforts_list.clone();
                let p_id = pid_e.clone();
                let exp_e = cur_explicit_e.clone();
                let inh_e = cur_inherited_e.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_base = match inh_e.as_deref() {
                        Some(e) => format!("Default (Inherit: {e})"),
                        None => "Default (Inherit Global)".into(),
                    };
                    let def_label = if exp_e.is_none() {
                        format!("✓ {def_base}")
                    } else {
                        def_base
                    };
                    let clear_id = p_id.clone();
                    items.push(MenuItem::new(def_label).on_select(move || {
                        let k = clear_id.clone();
                        overrides.update(|m| {
                            if let Some(ov) = m.get_mut(&k) {
                                ov.effort = None;
                                if ov.is_empty() {
                                    m.remove(&k);
                                }
                            }
                        });
                    }));
                    items.push(MenuItem::separator());
                    for eff in opts {
                        let checked = exp_e.as_deref() == Some(eff.as_str());
                        let title = if checked {
                            format!("✓ {eff}")
                        } else {
                            eff.clone()
                        };
                        let set_id = p_id.clone();
                        let chosen = eff.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            let k = set_id.clone();
                            let v = chosen.clone();
                            overrides.update(|m| {
                                m.entry(k).or_default().effort = Some(v);
                            });
                        }));
                    }
                    popup_menu(items).await;
                });
            },
        )
        .into_element()
    } else {
        view().into_element()
    };

    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(model_btn)
        .child(effort_btn)
        .into_element()
}

fn project_custom_model_inline_editor(
    role_id: String,
    caps: EngineCatalogCaps,
    overrides: Signal<BTreeMap<String, ProjectAgentOverride>>,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
) -> AnyElement {
    let apply_id = role_id.clone();
    let clear_id = role_id;
    view()
        .flex_row()
        .flex_wrap()
        .items_end()
        .gap_2()
        .pt_1()
        .child(
            view().flex_1().min_w(px(220.)).child(
                bound_field(
                    move || custom_model_draft.get(),
                    move |v| custom_model_draft.set(v.to_owned()),
                    "e.g. gemini-4-carbon-high, gpt-6.1-sol, claude-opus-4-6…",
                )
                .label("Custom model ID"),
            ),
        )
        .child(compact_action("Apply", Variant::Primary, move |_| {
            let val = custom_model_draft.get_untracked();
            let trimmed = val.trim().to_owned();
            let k = apply_id.clone();
            let caps_c = caps.clone();
            overrides.update(|m| {
                if trimmed.is_empty() {
                    if let Some(ov) = m.get_mut(&k) {
                        ov.model = None;
                        if ov.is_empty() {
                            m.remove(&k);
                        }
                    }
                } else {
                    let entry = m.entry(k).or_default();
                    if caps_c
                        .cleaned_effort_for_model(Some(&trimmed), entry.effort.as_deref())
                        .is_some()
                    {
                        entry.effort = None;
                    }
                    entry.model = Some(trimmed);
                }
            });
            custom_model_target.set(None);
        }))
        .child(compact_action("Reset default", Variant::Quiet, move |_| {
            let k = clear_id.clone();
            overrides.update(|m| {
                if let Some(ov) = m.get_mut(&k) {
                    ov.model = None;
                    if ov.is_empty() {
                        m.remove(&k);
                    }
                }
            });
            custom_model_target.set(None);
        }))
        .child(compact_action("Cancel", Variant::Quiet, move |_| {
            custom_model_target.set(None);
        }))
        .into_element()
}

fn inherited_project_options(
    all_profiles: &[Profile],
    prof: &Profile,
    override_agent: Option<&str>,
) -> (Option<String>, Option<String>) {
    let global_eng = engine_key_for_profile(prof);
    let Some(target) = override_agent.filter(|s| !s.trim().is_empty()) else {
        return (prof.model.clone(), prof.effort.clone());
    };
    let same_engine = target == global_eng
        || prof.preset.as_deref() == Some(target)
        || prof.command_name() == target;
    let peer = all_profiles
        .iter()
        .find(|p| p.id == target && !p.is_identity());
    let peer_model = peer
        .and_then(|p| p.model.as_deref().map(str::trim))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let peer_effort = peer
        .and_then(|p| p.effort.as_deref().map(str::trim))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if same_engine {
        (
            prof.model.clone().or(peer_model),
            prof.effort.clone().or(peer_effort),
        )
    } else {
        (peer_model, peer_effort)
    }
}

/// Reusable Project-level Identity -> Agent / Model / Effort Override editor, used both on the
/// dedicated `Project / Agents` screen and inside `New Project`.
pub fn project_overrides_editor(
    ui: Ui,
    overrides: Signal<BTreeMap<String, ProjectAgentOverride>>,
    host_id: Signal<String>,
) -> impl IntoElement {
    let custom_model_target = signal(Option::<String>::None);
    let custom_model_draft = signal(String::new());

    effect(move || {
        let h = host_id.get();
        refresh_catalog_for_host(ui, &h);
    });

    dynamic(move || {
        let all = ui.all_profiles.get();
        let cat_rows = catalog_agents(ui);
        let identities: Vec<Profile> = all
            .iter()
            .filter(|p| p.is_identity() && p.enabled)
            .cloned()
            .collect();
        let engines = enabled_engine_options(ui);
        let cur_map = overrides.get();
        let active_custom_model = custom_model_target.get();

        let has_any_override = cur_map.values().any(|ov| !ov.is_empty());
        let uniform_override = if !identities.is_empty() {
            let first = cur_map
                .get(&identities[0].id)
                .and_then(|ov| ov.normalized().agent);
            if let Some(target) = first
                && identities.iter().all(|id| {
                    cur_map
                        .get(&id.id)
                        .and_then(|ov| ov.normalized().agent)
                        .as_deref()
                        == Some(target.as_str())
                })
            {
                Some(target)
            } else {
                None
            }
        } else {
            None
        };

        let ids_for_all: Vec<String> = identities.iter().map(|p| p.id.clone()).collect();
        let all_for_all = all.clone();

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
                                    let is_empty = !has_any_override;
                                    move || is_empty
                                },
                                move |_| {
                                    custom_model_target.set(None);
                                    overrides.set(BTreeMap::new());
                                },
                            ))
                            .children(engines.iter().map(|(eid, ename)| {
                                let target_id = eid.clone();
                                let check_id = eid.clone();
                                let uni = uniform_override.clone();
                                let role_ids = ids_for_all.clone();
                                let all_c = all_for_all.clone();
                                let mark = engine_icon(eid);
                                compact_segment(
                                    ename.clone(),
                                    Some(mark),
                                    move || uni.as_deref() == Some(check_id.as_str()),
                                    move |_| {
                                        custom_model_target.set(None);
                                        overrides.update(|m| {
                                            for r in &role_ids {
                                                let prev_eff = m
                                                    .get(r)
                                                    .and_then(|ov| ov.normalized().agent)
                                                    .or_else(|| {
                                                        all_c
                                                            .iter()
                                                            .find(|p| p.id == *r)
                                                            .map(engine_key_for_profile)
                                                    });
                                                let entry = m.entry(r.clone()).or_default();
                                                if prev_eff.as_deref() != Some(target_id.as_str()) {
                                                    entry.model = None;
                                                    entry.effort = None;
                                                }
                                                entry.agent = Some(target_id.clone());
                                            }
                                        });
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
                        let global_eng_key = engine_key_for_profile(&prof);
                        let current_ov = cur_map
                            .get(&role_id)
                            .map(ProjectAgentOverride::normalized)
                            .unwrap_or_default();
                        let is_overridden = !current_ov.is_empty();
                        let current_agent_ov = current_ov.agent.clone();
                        let effective_eng_key = current_agent_ov
                            .clone()
                            .unwrap_or_else(|| global_eng_key.clone());
                        let effective_mark = current_agent_ov
                            .as_deref()
                            .map(engine_icon)
                            .unwrap_or(global_icon);
                        let caps = catalog_caps_for_engine(&cat_rows, &effective_eng_key);
                        let (inherited_model, inherited_effort) =
                            inherited_project_options(&all, &prof, current_agent_ov.as_deref());
                        let is_editing_custom_model =
                            active_custom_model.as_deref() == Some(role_id.as_str());
                        let engines_row = engines.clone();
                        let clear_role = role_id.clone();
                        let global_eng_for_clear = global_eng_key.clone();
                        let short_desc = builtin_role_summary(&role_id, &prof.role_instruction);

                        view()
                            .py_2p5()
                            .border_b_1()
                            .border_color(BORDER)
                            .gap_2()
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2p5()
                                    .min_w(px(0.))
                                    .child(
                                        icon_tinted(
                                            effective_mark,
                                            if is_overridden { ACCENT } else { MUTED },
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
                                                    .child(if is_overridden {
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
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .pl(px(26.))
                                    .child(
                                        segmented()
                                            .child(compact_segment(
                                                format!("Global ({global_prov})"),
                                                Some(global_icon),
                                                {
                                                    let is_none = current_agent_ov.is_none();
                                                    move || is_none
                                                },
                                                move |_| {
                                                    let k = clear_role.clone();
                                                    let g_eng = global_eng_for_clear.clone();
                                                    if custom_model_target
                                                        .get_untracked()
                                                        .as_deref()
                                                        == Some(k.as_str())
                                                    {
                                                        custom_model_target.set(None);
                                                    }
                                                    overrides.update(|m| {
                                                        if let Some(ov) = m.get_mut(&k) {
                                                            let prev_eng = ov.normalized().agent;
                                                            ov.agent = None;
                                                            if prev_eng
                                                                .is_some_and(|pe| pe != g_eng)
                                                            {
                                                                ov.model = None;
                                                                ov.effort = None;
                                                            }
                                                            if ov.is_empty() {
                                                                m.remove(&k);
                                                            }
                                                        }
                                                    });
                                                },
                                            ))
                                            .children(engines_row.into_iter().map(
                                                |(eid, ename)| {
                                                    let set_eid = eid.clone();
                                                    let check_eid = eid.clone();
                                                    let cur_ov = current_agent_ov.clone();
                                                    let rk = role_id.clone();
                                                    let g_eng = global_eng_key.clone();
                                                    let mark = engine_icon(&eid);
                                                    compact_segment(
                                                        ename,
                                                        Some(mark),
                                                        move || {
                                                            cur_ov.as_deref()
                                                                == Some(check_eid.as_str())
                                                        },
                                                        move |_| {
                                                            let k = rk.clone();
                                                            let v = set_eid.clone();
                                                            if custom_model_target
                                                                .get_untracked()
                                                                .as_deref()
                                                                == Some(k.as_str())
                                                            {
                                                                custom_model_target.set(None);
                                                            }
                                                            overrides.update(|m| {
                                                                let prev_eff = m
                                                                    .get(&k)
                                                                    .and_then(|ov| {
                                                                        ov.normalized().agent
                                                                    })
                                                                    .unwrap_or_else(|| {
                                                                        g_eng.clone()
                                                                    });
                                                                let entry = m.entry(k).or_default();
                                                                if prev_eff != v {
                                                                    entry.model = None;
                                                                    entry.effort = None;
                                                                }
                                                                entry.agent = Some(v);
                                                            });
                                                        },
                                                    )
                                                    .into_element()
                                                },
                                            )),
                                    )
                                    .child(project_model_effort_buttons(
                                        ui,
                                        role_id.clone(),
                                        inherited_model,
                                        current_ov.model,
                                        inherited_effort,
                                        current_ov.effort,
                                        caps.clone(),
                                        overrides,
                                        custom_model_target,
                                        custom_model_draft,
                                    )),
                            )
                            .child(if is_editing_custom_model {
                                view()
                                    .pl(px(26.))
                                    .child(project_custom_model_inline_editor(
                                        role_id,
                                        caps,
                                        overrides,
                                        custom_model_target,
                                        custom_model_draft,
                                    ))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
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

    let overrides = signal(initial_overrides);
    let host_id = signal(ui.active_project_host());

    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .w_full()
                    .max_w(px(760.))
                    .gap_5()
                    .pb_6()
                    .child(muted(move || {
                        format!(
                            "Choose which Agent, model, and effort run each @identity in this Project ({}) without editing Workflow YAML.",
                            crate::hosts::host_name(ui, &host_id.get())
                        )
                    }))
                    .child(when(
                        move || !ui.operation_error.get().is_empty(),
                        move || notice(move || ui.operation_error.get(), Tone::Attention),
                    ))
                    .child(project_overrides_editor(ui, overrides, host_id))
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
