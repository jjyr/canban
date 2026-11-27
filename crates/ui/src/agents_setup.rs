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
    PathBuf::from(crate::backend::home_for("local"))
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
                        .font_family(MONO_FONT)
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
                            format!(
                                "Open Terminal  {}",
                                crate::commands::shortcuts::NEW_TERMINAL_DISPLAY
                            ),
                            Variant::Secondary,
                            move |_| crate::commands::execute(ui, "new-terminal"),
                        )),
                ),
        )
        .child(
            muted(format!(
                "Tip: Copy the command first, then press {} to open a standalone Terminal and paste it to install or update.",
                crate::commands::shortcuts::NEW_TERMINAL_DISPLAY
            ))
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

/// Refresh `ui.agent_catalog` on the effective configuration host.
pub fn refresh_catalog(ui: Ui) {
    let host_id = ui.effective_config_host();
    refresh_catalog_for_host(ui, &host_id);
}

/// Refresh `ui.agent_catalog` for a specific `canband` host (`"local"` or remote `host_id`).
pub fn refresh_catalog_for_host(ui: Ui, host_id: &str) {
    if !crate::backend::is_connected() {
        return;
    }
    let target_host = if host_id.trim().is_empty() {
        "local".to_owned()
    } else {
        host_id.trim().to_owned()
    };
    if ui.agent_catalog_loading.get_untracked() {
        let same_host = ui
            .agent_catalog_loading_host
            .try_with(|h| h.as_deref() == Some(target_host.as_str()))
            .unwrap_or(false);
        if same_host {
            let _ = ui
                .agent_catalog_requested
                .try_with(|r| *r = Some(target_host));
            return;
        }
    }
    let catalog_epoch = ui.agent_catalog_epoch.get_untracked() + 1;
    ui.agent_catalog_epoch.set(catalog_epoch);
    ui.agent_catalog_loading.set(true);
    let _ = ui
        .agent_catalog_loading_host
        .try_with(|h| *h = Some(target_host.clone()));
    let _ = ui.agent_catalog_requested.try_with(|r| *r = None);
    let start_cfg_epoch = ui.agent_config_epoch.get_untracked();
    ui.spawn(async move {
        let req_host = target_host.clone();
        let result = blocking(move || {
            crate::routed_call("agent.catalog", json!({"host_id": req_host}), None)
        })
        .await;
        if ui.agent_catalog_epoch.try_get() != Some(catalog_epoch) {
            return;
        }
        let _ = ui.agent_catalog_loading.try_set(false);
        let _ = ui.agent_catalog_loading_host.try_with(|h| *h = None);
        match result {
            Ok(Ok(mut val)) => {
                let cur_profiles = ui.all_profiles.try_get().unwrap_or_default();
                let ops: Vec<Value> = ui
                    .pending_catalog_ops
                    .try_with(|list| {
                        let ops = list
                            .iter()
                            .filter(|(ep, _)| *ep > start_cfg_epoch)
                            .map(|(_, p)| p.clone())
                            .collect::<Vec<_>>();
                        list.clear();
                        ops
                    })
                    .unwrap_or_default();
                for op in &ops {
                    apply_op_to_catalog(&mut val, op, None);
                }
                sync_catalog_agents_with_profiles(&mut val, &cur_profiles);
                let _ = ui.agent_catalog.try_set(val);
            }
            Ok(Err(e)) => {
                let _ = ui.operation_error.try_set(format!("{e:#}"));
            }
            Err(e) => {
                let _ = ui.operation_error.try_set(e.to_string());
            }
        }
        let follow_up = ui.agent_catalog_requested.try_with(|r| r.take()).flatten();
        if let Some(next_host) = follow_up {
            refresh_catalog_for_host(ui, &next_host);
        }
    });
}

fn sync_catalog_agents_with_profiles(cat: &mut Value, profiles: &[Profile]) {
    let Some(agents) = cat["agents"].as_array_mut() else {
        return;
    };
    for row in agents.iter_mut() {
        let Some(id) = row["id"].as_str().map(str::to_owned) else {
            continue;
        };
        let prof = profiles.iter().find(|p| p.id == id && !p.is_identity());
        row["configured"] = json!(prof.is_some());
        row["enabled"] = json!(prof.is_some_and(|p| p.enabled));
        if let Some(p) = prof {
            if let Some(name) = &p.name {
                row["display_name"] = json!(name);
            }
            if let Some(bin) = p.command().first() {
                row["binary"] = json!(bin);
            }
        }
    }
}

fn trim_non_empty(val: Option<&Value>) -> Option<String> {
    val.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn trim_orchestrator(val: Option<&Value>) -> Option<OrchestratorConfig> {
    let v = val?;
    if v.is_null() {
        return None;
    }
    serde_json::from_value::<OrchestratorConfig>(v.clone())
        .ok()
        .and_then(OrchestratorConfig::into_non_empty)
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct RawProfileOptions {
    model: Option<String>,
    effort: Option<String>,
    orchestrator: Option<OrchestratorConfig>,
}

fn apply_op_to_catalog(cat: &mut Value, payload: &Value, resolved_cfg: Option<&Config>) {
    if cat.is_null() {
        if let Some(cfg) = resolved_cfg {
            sync_catalog_agents_with_profiles(cat, &cfg.profiles);
        }
        return;
    }
    let op = payload["op"].as_str().unwrap_or("");
    match op {
        "set_profile_options" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(|s| s.trim().trim_start_matches('@'))
                .filter(|s| !s.is_empty())
                && let Some(arr) = cat["raw_profiles"].as_array_mut()
            {
                let item =
                    if let Some(existing) = arr.iter_mut().find(|v| v["id"].as_str() == Some(id)) {
                        existing
                    } else {
                        arr.push(json!({ "id": id }));
                        arr.last_mut().unwrap()
                    };
                if let Some(obj) = item.as_object_mut() {
                    if let Some(m) = payload.get("model") {
                        match trim_non_empty(Some(m)) {
                            Some(s) if is_orchestrator_model_id(&s) => {
                                obj.insert("model".into(), json!(ORCHESTRATOR_MODEL_ID));
                                obj.remove("effort");
                                if let Some(leg) = legacy_agy_orchestrator_preset(&s)
                                    && payload.get("orchestrator").is_none()
                                    && obj.get("orchestrator").is_none_or(Value::is_null)
                                {
                                    obj.insert("orchestrator".into(), json!(leg));
                                }
                            }
                            Some(s) => {
                                obj.insert("model".into(), json!(s));
                                if payload.get("orchestrator").is_none() {
                                    obj.remove("orchestrator");
                                }
                            }
                            None => {
                                obj.remove("model");
                                if payload.get("orchestrator").is_none() {
                                    obj.remove("orchestrator");
                                }
                            }
                        }
                    }
                    if let Some(e) = payload.get("effort") {
                        let is_orch = obj
                            .get("model")
                            .and_then(Value::as_str)
                            .is_some_and(is_orchestrator_model_id);
                        if is_orch {
                            obj.remove("effort");
                        } else {
                            match trim_non_empty(Some(e)) {
                                Some(s) => {
                                    obj.insert("effort".into(), json!(s));
                                }
                                None => {
                                    obj.remove("effort");
                                }
                            }
                        }
                    }
                    if let Some(o) = payload.get("orchestrator") {
                        match trim_orchestrator(Some(o)) {
                            Some(orch) => {
                                obj.insert("orchestrator".into(), json!(orch));
                                obj.remove("effort");
                            }
                            None => {
                                obj.remove("orchestrator");
                            }
                        }
                    }
                    if let Some(argv) = payload.get("argv") {
                        if argv.is_null() {
                            obj.remove("argv");
                        } else {
                            obj.insert("argv".into(), argv.clone());
                        }
                    }
                }
            }
        }
        "toggle_agent" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let enabled = payload["enabled"].as_bool().unwrap_or(true);
                if let Some(arr) = cat["raw_profiles"].as_array_mut() {
                    if let Some(existing) = arr.iter_mut().find(|v| v["id"].as_str() == Some(id)) {
                        existing["enabled"] = json!(enabled);
                    } else if enabled {
                        arr.push(json!({
                            "id": id,
                            "enabled": true,
                            "identity": false,
                            "preset": id,
                        }));
                    }
                }
                if let Some(agents) = cat["agents"].as_array_mut()
                    && let Some(row) = agents.iter_mut().find(|r| r["id"].as_str() == Some(id))
                {
                    row["enabled"] = json!(enabled);
                    if enabled {
                        row["configured"] = json!(true);
                    }
                }
            }
        }
        "upsert_identity" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(|s| s.trim().trim_start_matches('@'))
                .filter(|s| !s.is_empty())
                && let Some(arr) = cat["raw_profiles"].as_array_mut()
            {
                let engine = trim_non_empty(payload.get("engine"));
                let item =
                    if let Some(existing) = arr.iter_mut().find(|v| v["id"].as_str() == Some(id)) {
                        existing
                    } else {
                        arr.push(json!({ "id": id, "identity": true }));
                        arr.last_mut().unwrap()
                    };
                if let Some(obj) = item.as_object_mut() {
                    let prev_preset = obj.get("preset").and_then(Value::as_str).map(str::to_owned);
                    let engine_changed = engine.as_deref() != prev_preset.as_deref();
                    obj.insert("identity".into(), json!(true));
                    obj.insert(
                        "enabled".into(),
                        json!(payload["enabled"].as_bool().unwrap_or(true)),
                    );
                    if let Some(eng) = engine {
                        obj.insert("preset".into(), json!(eng));
                    }
                    if let Some(m) = payload.get("model") {
                        match trim_non_empty(Some(m)) {
                            Some(s) if is_orchestrator_model_id(&s) => {
                                obj.insert("model".into(), json!(ORCHESTRATOR_MODEL_ID));
                                obj.remove("effort");
                                if let Some(leg) = legacy_agy_orchestrator_preset(&s)
                                    && payload.get("orchestrator").is_none()
                                    && obj.get("orchestrator").is_none_or(Value::is_null)
                                {
                                    obj.insert("orchestrator".into(), json!(leg));
                                }
                            }
                            Some(s) => {
                                obj.insert("model".into(), json!(s));
                                if payload.get("orchestrator").is_none() {
                                    obj.remove("orchestrator");
                                }
                            }
                            None => {
                                obj.remove("model");
                                if payload.get("orchestrator").is_none() {
                                    obj.remove("orchestrator");
                                }
                            }
                        }
                    } else if engine_changed {
                        obj.remove("model");
                    }
                    if let Some(e) = payload.get("effort") {
                        let is_orch = obj
                            .get("model")
                            .and_then(Value::as_str)
                            .is_some_and(is_orchestrator_model_id);
                        if is_orch {
                            obj.remove("effort");
                        } else {
                            match trim_non_empty(Some(e)) {
                                Some(s) => {
                                    obj.insert("effort".into(), json!(s));
                                }
                                None => {
                                    obj.remove("effort");
                                }
                            }
                        }
                    } else if engine_changed {
                        obj.remove("effort");
                    }
                    if let Some(o) = payload.get("orchestrator") {
                        match trim_orchestrator(Some(o)) {
                            Some(orch) => {
                                obj.insert("orchestrator".into(), json!(orch));
                                obj.remove("effort");
                            }
                            None => {
                                obj.remove("orchestrator");
                            }
                        }
                    } else if engine_changed {
                        obj.remove("orchestrator");
                    }
                    if let Some(instr) = payload.get("role_instruction") {
                        obj.insert("role_instruction".into(), instr.clone());
                    }
                }
            }
        }
        "delete_profile" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if let Some(arr) = cat["raw_profiles"].as_array_mut() {
                    arr.retain(|v| v["id"].as_str() != Some(id));
                }
                if let Some(agents) = cat["agents"].as_array_mut() {
                    agents.retain(|r| !(r["id"].as_str() == Some(id) && r["kind"] == "custom"));
                    for r in agents.iter_mut() {
                        if r["id"].as_str() == Some(id) {
                            r["enabled"] = json!(false);
                            r["configured"] = json!(false);
                        }
                    }
                }
            }
        }
        "upsert_custom_agent" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let enabled = payload["enabled"].as_bool().unwrap_or(true);
                let argv: Vec<String> = payload["argv"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let bin = argv.first().cloned().unwrap_or_else(|| id.to_owned());
                if let Some(arr) = cat["raw_profiles"].as_array_mut() {
                    if let Some(existing) = arr.iter_mut().find(|v| v["id"].as_str() == Some(id)) {
                        existing["enabled"] = json!(enabled);
                        existing["argv"] = json!(argv);
                    } else {
                        arr.push(json!({
                            "id": id,
                            "enabled": enabled,
                            "identity": false,
                            "argv": argv,
                        }));
                    }
                }
                if let Some(agents) = cat["agents"].as_array_mut() {
                    if let Some(row) = agents.iter_mut().find(|r| r["id"].as_str() == Some(id)) {
                        row["enabled"] = json!(enabled);
                        row["configured"] = json!(true);
                        row["binary"] = json!(bin);
                    } else {
                        let supports_effort = argv.iter().any(|a| a.contains("{effort}"));
                        agents.push(json!({
                            "id": id,
                            "display_name": trim_non_empty(payload.get("name")).unwrap_or_else(|| id.to_owned()),
                            "binary": bin,
                            "default_argv": argv,
                            "kind": "custom",
                            "resumable": false,
                            "installed": true,
                            "path": Value::Null,
                            "configured": true,
                            "enabled": enabled,
                            "models": [],
                            "supported_efforts": [],
                            "supports_model": true,
                            "supports_effort": supports_effort,
                        }));
                    }
                }
            }
        }
        "replace_profiles" if payload["profiles"].is_array() => {
            cat["raw_profiles"] = payload["profiles"].clone();
        }
        _ => {}
    }
    if let Some(cfg) = resolved_cfg {
        sync_catalog_agents_with_profiles(cat, &cfg.profiles);
    }
}

fn builtin_profile_seed(id: &str) -> Option<Profile> {
    canban_profiles::builtin_catalog()
        .into_iter()
        .find(|e| e.id == id)
        .map(|entry| Profile {
            id: entry.id.clone(),
            name: Some(entry.display_name),
            enabled: true,
            identity: false,
            preset: Some(entry.id),
            argv: Some(entry.argv),
            ..Default::default()
        })
}

fn peer_inherited_options(all: &[Profile], prof: &Profile) -> RawProfileOptions {
    let Some(preset) = prof.preset.as_deref() else {
        return RawProfileOptions::default();
    };
    let Some(peer) = all.iter().find(|p| p.id == preset && !p.is_identity()) else {
        return RawProfileOptions::default();
    };
    RawProfileOptions {
        model: peer.model.clone(),
        effort: if peer.is_orchestrator() {
            None
        } else {
            peer.effort.clone()
        },
        orchestrator: peer.effective_orchestrator(),
    }
}

fn merge_orchestrator_pair(
    child: Option<OrchestratorConfig>,
    base: Option<OrchestratorConfig>,
) -> Option<OrchestratorConfig> {
    match (child, base) {
        (Some(c), Some(b)) => c.merge_fallback(&b).into_non_empty(),
        (Some(c), None) => c.into_non_empty(),
        (None, Some(b)) => b.into_non_empty(),
        (None, None) => None,
    }
}

fn fallback_default_agent(all: &[Profile], default_agent: &mut String) {
    if !all.iter().any(|p| p.id == *default_agent && p.enabled)
        && let Some(fallback) = all
            .iter()
            .find(|p| p.id == "developer" && p.enabled)
            .or_else(|| all.iter().find(|p| p.enabled))
    {
        *default_agent = fallback.id.clone();
    }
}

fn apply_op_to_profiles(
    all: &mut Vec<Profile>,
    default_agent: &mut String,
    payload: &Value,
    raw_catalog: &Value,
) {
    let op = payload["op"].as_str().unwrap_or("");
    match op {
        "set_default" => {
            if let Some(target) = trim_non_empty(payload.get("default_agent")) {
                *default_agent = target;
            }
        }
        "toggle_agent" => {
            if let Some(id) = trim_non_empty(payload.get("id")) {
                let enabled = payload["enabled"].as_bool().unwrap_or(true);
                if let Some(existing) = all.iter_mut().find(|p| p.id == id) {
                    existing.enabled = enabled;
                } else if enabled && let Some(seed) = builtin_profile_seed(&id) {
                    all.push(seed);
                }
                fallback_default_agent(all, default_agent);
            }
        }
        "set_profile_options" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(|s| s.trim().trim_start_matches('@').to_owned())
                .filter(|s| !s.is_empty())
            {
                if !all.iter().any(|p| p.id == id)
                    && let Some(seed) = builtin_profile_seed(&id)
                {
                    all.push(seed);
                }
                let snapshot_all = all.clone();
                if let Some(prof) = all.iter_mut().find(|p| p.id == id) {
                    let peer_opts = peer_inherited_options(&snapshot_all, prof);
                    if let Some(m) = payload.get("model") {
                        match trim_non_empty(Some(m)) {
                            Some(s) if prof.is_agy() && is_orchestrator_model_id(&s) => {
                                prof.model = Some(ORCHESTRATOR_MODEL_ID.to_owned());
                                prof.effort = None;
                                if let Some(leg) = legacy_agy_orchestrator_preset(&s)
                                    && payload.get("orchestrator").is_none()
                                    && prof.orchestrator.is_none()
                                {
                                    prof.orchestrator = Some(leg);
                                }
                            }
                            Some(s) => {
                                prof.model = Some(s);
                                if payload.get("orchestrator").is_none() {
                                    prof.orchestrator = None;
                                }
                            }
                            None => {
                                if prof.is_identity() {
                                    prof.model = peer_opts.model.clone();
                                    if payload.get("orchestrator").is_none() {
                                        if prof.is_orchestrator() {
                                            let raw_o = raw_profile_options(raw_catalog, &prof.id)
                                                .and_then(|r| r.orchestrator);
                                            prof.orchestrator = merge_orchestrator_pair(
                                                raw_o,
                                                peer_opts.orchestrator.clone(),
                                            );
                                        } else {
                                            prof.orchestrator = None;
                                        }
                                    }
                                    if prof.is_orchestrator() {
                                        prof.effort = None;
                                    } else if prof.effort.is_none() {
                                        prof.effort = peer_opts.effort.clone();
                                    }
                                } else {
                                    prof.model = None;
                                    if payload.get("orchestrator").is_none() {
                                        prof.orchestrator = None;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(e) = payload.get("effort") {
                        if prof.is_orchestrator() {
                            prof.effort = None;
                        } else {
                            prof.effort = trim_non_empty(Some(e)).or_else(|| {
                                if prof.is_identity() {
                                    peer_opts.effort.clone()
                                } else {
                                    None
                                }
                            });
                        }
                    }
                    if let Some(o) = payload.get("orchestrator") {
                        let explicit_o = trim_orchestrator(Some(o));
                        if prof.is_orchestrator() {
                            prof.orchestrator = if prof.is_identity() {
                                merge_orchestrator_pair(explicit_o, peer_opts.orchestrator.clone())
                            } else {
                                explicit_o
                            };
                            prof.effort = None;
                        } else {
                            prof.orchestrator = None;
                        }
                    }
                    if let Some(argv_val) = payload.get("argv") {
                        if argv_val.is_null() {
                            if let Some(preset) = prof.preset.as_deref()
                                && let Some(entry) = canban_profiles::builtin_catalog()
                                    .into_iter()
                                    .find(|e| e.id == preset)
                            {
                                prof.argv = Some(entry.argv);
                            }
                        } else if let Some(arr) = argv_val.as_array() {
                            let parsed: Vec<String> = arr
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect();
                            if !parsed.is_empty() {
                                prof.argv = Some(parsed);
                            }
                        }
                    }
                    // Propagate direct agent defaults to inheriting identities immediately.
                    if !prof.is_identity() {
                        let updated_id = prof.id.clone();
                        let updated_m = prof.model.clone();
                        let updated_e = prof.effort.clone();
                        let updated_o = prof.effective_orchestrator();
                        for child in all.iter_mut() {
                            if child.is_identity()
                                && child.preset.as_deref() == Some(updated_id.as_str())
                            {
                                let raw_opts = raw_profile_options(raw_catalog, &child.id);
                                if payload.get("model").is_some()
                                    && raw_opts.as_ref().is_none_or(|r| r.model.is_none())
                                {
                                    child.model = updated_m.clone();
                                }
                                if payload.get("effort").is_some() || payload.get("model").is_some()
                                {
                                    if child.is_orchestrator() {
                                        child.effort = None;
                                    } else if raw_opts.as_ref().is_none_or(|r| r.effort.is_none()) {
                                        child.effort = updated_e.clone();
                                    }
                                }
                                if payload.get("orchestrator").is_some()
                                    || payload.get("model").is_some()
                                {
                                    if child.is_orchestrator() {
                                        let child_raw_o =
                                            raw_opts.as_ref().and_then(|r| r.orchestrator.clone());
                                        child.orchestrator =
                                            merge_orchestrator_pair(child_raw_o, updated_o.clone());
                                    } else {
                                        child.orchestrator = None;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "upsert_identity" => {
            if let Some(id) = payload["id"]
                .as_str()
                .map(|s| s.trim().trim_start_matches('@').to_owned())
                .filter(|s| !s.is_empty())
                && let Some(engine) = trim_non_empty(payload.get("engine"))
            {
                if !all.iter().any(|p| p.id == engine && !p.is_identity())
                    && let Some(seed) = builtin_profile_seed(&engine)
                {
                    all.push(seed);
                }
                let peer = all
                    .iter()
                    .find(|p| p.id == engine && !p.is_identity())
                    .cloned();
                let preset_ref = peer
                    .as_ref()
                    .and_then(|p| p.preset.clone())
                    .unwrap_or_else(|| engine.clone());
                let enabled = payload["enabled"].as_bool().unwrap_or(true);
                if let Some(existing) = all.iter_mut().find(|p| p.id == id) {
                    let engine_changed = existing.preset.as_deref() != Some(preset_ref.as_str());
                    existing.identity = true;
                    existing.enabled = enabled;
                    existing.preset = Some(preset_ref);
                    if let Some(peer_p) = &peer {
                        existing.argv = peer_p.argv.clone();
                    }
                    if let Some(m) = payload.get("model") {
                        existing.model = trim_non_empty(Some(m))
                            .or_else(|| peer.as_ref().and_then(|p| p.model.clone()));
                    } else if engine_changed {
                        existing.model = peer.as_ref().and_then(|p| p.model.clone());
                    }
                    if let Some(e) = payload.get("effort") {
                        existing.effort = trim_non_empty(Some(e))
                            .or_else(|| peer.as_ref().and_then(|p| p.effort.clone()));
                    } else if engine_changed {
                        existing.effort = peer.as_ref().and_then(|p| p.effort.clone());
                    }
                    if let Some(o) = payload.get("orchestrator") {
                        existing.orchestrator = merge_orchestrator_pair(
                            trim_orchestrator(Some(o)),
                            peer.as_ref().and_then(|p| p.orchestrator.clone()),
                        );
                    } else if engine_changed {
                        existing.orchestrator = peer.as_ref().and_then(|p| p.orchestrator.clone());
                    }
                    if !existing.is_orchestrator() {
                        existing.orchestrator = None;
                    } else {
                        existing.effort = None;
                    }
                    if let Some(instr) = payload["role_instruction"].as_str() {
                        existing.role_instruction = instr.trim().to_owned();
                    }
                } else {
                    all.push(Profile {
                        id: id.clone(),
                        name: trim_non_empty(payload.get("name")),
                        enabled,
                        identity: true,
                        preset: Some(preset_ref),
                        model: trim_non_empty(payload.get("model"))
                            .or_else(|| peer.as_ref().and_then(|p| p.model.clone())),
                        effort: trim_non_empty(payload.get("effort"))
                            .or_else(|| peer.as_ref().and_then(|p| p.effort.clone())),
                        orchestrator: merge_orchestrator_pair(
                            trim_orchestrator(payload.get("orchestrator")),
                            peer.as_ref().and_then(|p| p.orchestrator.clone()),
                        ),
                        role_instruction: payload["role_instruction"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("You are the Card's {id} Agent.")),
                        argv: peer.and_then(|p| p.argv),
                        ..Default::default()
                    });
                }
            }
        }
        "delete_profile" => {
            if let Some(id) = trim_non_empty(payload.get("id")) {
                all.retain(|p| p.id != id);
                fallback_default_agent(all, default_agent);
            }
        }
        "upsert_custom_agent" => {
            if let Some(id) = trim_non_empty(payload.get("id")) {
                let argv: Vec<String> = payload["argv"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let new_prof = Profile {
                    id: id.clone(),
                    name: trim_non_empty(payload.get("name")),
                    enabled: payload["enabled"].as_bool().unwrap_or(true),
                    identity: false,
                    preset: None,
                    model: trim_non_empty(payload.get("model")),
                    effort: trim_non_empty(payload.get("effort")),
                    orchestrator: None,
                    role_instruction: String::new(),
                    argv: (!argv.is_empty()).then_some(argv),
                    resume_argv: None,
                    exit: Some(ExitAction::Manual),
                    session_instruction: None,
                };
                if let Some(existing) = all.iter_mut().find(|p| p.id == id) {
                    *existing = new_prof;
                } else {
                    all.push(new_prof);
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn dispatch_agents_save(ui: Ui, payload: Value, on_ok: impl FnOnce(&Config) + 'static) {
    let raw_cat = ui.agent_catalog.get_untracked();
    if !crate::backend::is_connected() {
        ui.update_profiles(|all, def| apply_op_to_profiles(all, def, &payload, &raw_cat));
        ui.agent_catalog
            .update(|cat| apply_op_to_catalog(cat, &payload, None));
        let cfg = Config {
            default_agent: ui.default_agent.get_untracked(),
            onboarded: ui.onboarded.get_untracked(),
            profiles: ui.all_profiles.get_untracked(),
            ..Default::default()
        };
        on_ok(&cfg);
        return;
    }
    if ui.is_method_pending("config.agents.save") {
        return;
    }
    let prev_all = ui.all_profiles.get_untracked();
    let prev_def = ui.default_agent.get_untracked();
    let prev_onboarded = ui.onboarded.get_untracked();
    let prev_cat = raw_cat.clone();

    let next_epoch = ui.agent_config_epoch.get_untracked() + 1;
    ui.agent_config_epoch.set(next_epoch);
    if ui.agent_catalog_loading.get_untracked() || prev_cat.is_null() {
        let _ = ui
            .pending_catalog_ops
            .try_with(|list| list.push((next_epoch, payload.clone())));
    }
    ui.update_profiles(|all, def| apply_op_to_profiles(all, def, &payload, &raw_cat));
    ui.agent_catalog
        .update(|cat| apply_op_to_catalog(cat, &payload, None));

    let payload_for_cb = payload.clone();
    dispatch_complete(ui, "config.agents.save", payload, move |res| match res {
        Ok(val) => {
            let cfg = serde_json::from_value::<Config>(val)
                .unwrap_or_else(|_| crate::config_for_host(&ui.effective_config_host()));
            ui.agent_catalog
                .update(|cat| apply_op_to_catalog(cat, &payload_for_cb, Some(&cfg)));
            on_ok(&cfg);
        }
        Err(_) => {
            batch(|| {
                ui.update_profiles(|all, def| {
                    *all = prev_all;
                    *def = prev_def;
                });
                ui.onboarded.set(prev_onboarded);
                ui.agent_catalog.set(prev_cat);
            });
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
            "supports_orchestrator": entry.supports_orchestrator,
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
            let supports_orchestrator = !is_herdr
                && canban_profiles::catalog_entry_for_profile(prof)
                    .is_some_and(|e| e.supports_orchestrator);
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
                "supports_orchestrator": supports_orchestrator,
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
    supports_orchestrator: bool,
}

impl EngineCatalogCaps {
    fn single_models(&self) -> Vec<canban_profiles::ModelOption> {
        self.models
            .iter()
            .filter(|m| !is_orchestrator_model_id(&m.id))
            .cloned()
            .collect()
    }

    fn planner_models(&self) -> Vec<canban_profiles::ModelOption> {
        canban_profiles::agy_orchestrator_planner_models(&self.models)
    }

    fn worker_models(&self) -> Vec<canban_profiles::ModelOption> {
        canban_profiles::agy_orchestrator_worker_models(&self.models)
    }

    fn model_label(&self, model_id: &str) -> String {
        self.models
            .iter()
            .find(|m| m.id == model_id)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| short_model_badge(model_id))
    }

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
                        let effort_labels: BTreeMap<String, String> = v
                            .get("effort_labels")
                            .and_then(Value::as_object)
                            .map(|obj| {
                                obj.iter()
                                    .filter_map(|(k, val)| {
                                        Some((k.clone(), val.as_str()?.trim().to_owned()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(canban_profiles::ModelOption {
                            id: id.to_owned(),
                            label: if label.is_empty() {
                                id.to_owned()
                            } else {
                                label.to_owned()
                            },
                            supported_efforts,
                            effort_labels,
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
        let supports_orchestrator = row["supports_orchestrator"]
            .as_bool()
            .unwrap_or(engine_id == "agy");
        return EngineCatalogCaps {
            engine_id,
            models,
            efforts,
            supports_model,
            supports_effort,
            supports_orchestrator,
        };
    }
    EngineCatalogCaps {
        engine_id: engine_key.to_owned(),
        models: Vec::new(),
        efforts: vec!["low".into(), "medium".into(), "high".into()],
        supports_model: true,
        supports_effort: false,
        supports_orchestrator: engine_key == "agy",
    }
}

fn raw_profile_options(catalog_val: &Value, profile_id: &str) -> Option<RawProfileOptions> {
    let arr = catalog_val["raw_profiles"].as_array()?;
    let item = arr.iter().find(|v| v["id"].as_str() == Some(profile_id))?;
    let mut m = item["model"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let mut e = item["effort"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let mut o = trim_orchestrator(item.get("orchestrator"));
    if let Some(ref model_id) = m
        && is_orchestrator_model_id(model_id)
    {
        if let Some(legacy) = legacy_agy_orchestrator_preset(model_id)
            && o.is_none()
        {
            o = Some(legacy);
        }
        m = Some(ORCHESTRATOR_MODEL_ID.to_owned());
        e = None;
    }
    Some(RawProfileOptions {
        model: m,
        effort: e,
        orchestrator: o,
    })
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
                                dispatch_agents_save(
                                    ui,
                                    json!({
                                        "op": "set_profile_options",
                                        "id": id_c,
                                        "argv": Value::Null,
                                    }),
                                    move |_| {
                                        if let Some(def) = def_c.as_ref() {
                                            let next_bin = def.first().cloned().unwrap_or_default();
                                            let next_args = if def.len() > 1 {
                                                canban_profiles::join_posix_words(&def[1..])
                                            } else {
                                                String::new()
                                            };
                                            command_bin_draft.set(next_bin);
                                            command_args_draft.set(next_args);
                                        }
                                        command_error.set(String::new());
                                        command_target.set(None);
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
                            dispatch_agents_save(
                                ui,
                                json!({
                                    "op": "set_profile_options",
                                    "id": id_c,
                                    "argv": argv,
                                }),
                                move |_| {
                                    command_error.set(String::new());
                                    command_target.set(None);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrchestratorRoleKind {
    Planner,
    Worker,
}

impl OrchestratorRoleKind {
    fn key_suffix(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Worker => "worker",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::Planner => canban_profiles::AGY_DEFAULT_PLANNER_MODEL,
            Self::Worker => canban_profiles::AGY_DEFAULT_WORKER_MODEL,
        }
    }
}

#[derive(Clone)]
enum OrchestratorFieldUpdate {
    Model(OrchestratorRoleKind, Option<String>, bool),
    Effort(OrchestratorRoleKind, Option<String>),
}

fn apply_orchestrator_update(orch: &mut OrchestratorConfig, update: OrchestratorFieldUpdate) {
    match update {
        OrchestratorFieldUpdate::Model(OrchestratorRoleKind::Planner, next_m, clear_eff) => {
            orch.planner_model = next_m;
            if clear_eff {
                orch.planner_effort = None;
            }
        }
        OrchestratorFieldUpdate::Model(OrchestratorRoleKind::Worker, next_m, clear_eff) => {
            orch.worker_model = next_m;
            if clear_eff {
                orch.worker_effort = None;
            }
        }
        OrchestratorFieldUpdate::Effort(OrchestratorRoleKind::Planner, next_e) => {
            orch.planner_effort = next_e;
        }
        OrchestratorFieldUpdate::Effort(OrchestratorRoleKind::Worker, next_e) => {
            orch.worker_effort = next_e;
        }
    }
}

type ModelSelectFn = std::rc::Rc<dyn Fn(Option<String>, bool)>;
type EffortSelectFn = std::rc::Rc<dyn Fn(Option<String>)>;

#[allow(clippy::too_many_arguments)]
fn render_model_effort_buttons(
    ui: Ui,
    target_id: String,
    inherited_model: Option<String>,
    explicit_model: Option<String>,
    inherited_effort: Option<String>,
    explicit_effort: Option<String>,
    has_parent: bool,
    inherit_default_prefix: &str,
    caps: EngineCatalogCaps,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
    on_select_model: ModelSelectFn,
    on_select_effort: EffortSelectFn,
) -> AnyElement {
    let active_model_id = explicit_model.as_deref().or(inherited_model.as_deref());
    let is_orch_mode =
        caps.supports_orchestrator && active_model_id.is_some_and(is_orchestrator_model_id);
    let model_efforts = if is_orch_mode {
        None
    } else {
        caps.efforts_for_model(active_model_id)
    };
    if !caps.supports_model && model_efforts.is_none() {
        return view().into_element();
    }

    let model_btn = if caps.supports_model {
        let btn_label = if is_orch_mode {
            match (&explicit_model, &inherited_model) {
                (Some(_), _) => "Orchestrator ▾".into(),
                (None, Some(_)) if has_parent => "Orchestrator (auto) ▾".into(),
                _ => "Orchestrator ▾".into(),
            }
        } else {
            match (&explicit_model, &inherited_model) {
                (Some(m), _) => format!("{} ▾", short_model_badge(m)),
                (None, Some(inherited)) if has_parent => {
                    format!("{} (auto) ▾", short_model_badge(inherited))
                }
                _ => "Model: Default ▾".into(),
            }
        };
        let pid_m = target_id.clone();
        let models_list = caps.single_models();
        let caps_for_menu = caps.clone();
        let cur_explicit_m = explicit_model.clone();
        let cur_inherited_m = inherited_model.clone();
        let cur_explicit_e_for_model = explicit_effort.clone();
        let default_prefix = inherit_default_prefix.to_owned();
        let supports_orch = caps.supports_orchestrator;
        let on_model = on_select_model.clone();
        compact_action(
            btn_label,
            if explicit_model.is_some() {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| {
                if models_list.is_empty() && !supports_orch {
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
                let exp_e_m = cur_explicit_e_for_model.clone();
                let def_prefix = default_prefix.clone();
                let on_m = on_model.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_base = if has_parent {
                        match inh_m.as_deref() {
                            Some(m) if supports_orch && is_orchestrator_model_id(m) => {
                                format!("{def_prefix} (Orchestrator)")
                            }
                            Some(m) if def_prefix.contains("Inherit Global") => {
                                format!("Default (Inherit: {})", short_model_badge(m))
                            }
                            _ => def_prefix.clone(),
                        }
                    } else {
                        "Default (CLI default)".into()
                    };
                    let def_label = if exp_m.is_none() {
                        format!("✓ {def_base}")
                    } else {
                        def_base
                    };
                    let on_clear = on_m.clone();
                    items.push(MenuItem::new(def_label).on_select(move || {
                        custom_model_target.set(None);
                        on_clear(None, false);
                    }));
                    items.push(MenuItem::separator());
                    if supports_orch {
                        let checked = exp_m.as_deref().is_some_and(is_orchestrator_model_id);
                        let title = if checked {
                            "✓ Orchestrator (Planner + Worker)"
                        } else {
                            "Orchestrator (Planner + Worker)"
                        };
                        let on_orch = on_m.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            custom_model_target.set(None);
                            on_orch(Some(ORCHESTRATOR_MODEL_ID.to_owned()), true);
                        }));
                        items.push(MenuItem::separator());
                    }
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
                        let chosen = mid.clone();
                        let clear_eff = caps_m
                            .cleaned_effort_for_model(Some(&chosen), exp_e_m.as_deref())
                            .is_some();
                        let on_pick = on_m.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            custom_model_target.set(None);
                            on_pick(Some(chosen.clone()), clear_eff);
                        }));
                    }
                    items.push(MenuItem::separator());
                    let custom_id = p_id.clone();
                    let seed = exp_m
                        .filter(|m| !is_orchestrator_model_id(m))
                        .or_else(|| inh_m.filter(|m| !is_orchestrator_model_id(m)))
                        .unwrap_or_default();
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
            (None, Some(inherited)) if has_parent => format!("Effort: {inherited} (auto) ▾"),
            _ => "Effort: Default ▾".into(),
        };
        let cur_explicit_e = valid_explicit_effort.clone();
        let cur_inherited_e = valid_inherited_effort.clone();
        let def_prefix = inherit_default_prefix.to_owned();
        let on_eff = on_select_effort;
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
                let exp_e = cur_explicit_e.clone();
                let inh_e = cur_inherited_e.clone();
                let def_pref = def_prefix.clone();
                let on_e = on_eff.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_base = if has_parent {
                        match inh_e.as_deref() {
                            Some(e) if def_pref.contains("Inherit Global") => {
                                format!("Default (Inherit: {e})")
                            }
                            _ => def_pref,
                        }
                    } else {
                        "Default (CLI default)".into()
                    };
                    let def_label = if exp_e.is_none() {
                        format!("✓ {def_base}")
                    } else {
                        def_base
                    };
                    let on_clear = on_e.clone();
                    items.push(MenuItem::new(def_label).on_select(move || {
                        on_clear(None);
                    }));
                    items.push(MenuItem::separator());
                    for eff in opts {
                        let checked = exp_e.as_deref() == Some(eff.as_str());
                        let title = if checked {
                            format!("✓ {eff}")
                        } else {
                            eff.clone()
                        };
                        let chosen = eff.clone();
                        let on_pick = on_e.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            on_pick(Some(chosen.clone()));
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
    let pid_m = profile_id.clone();
    let on_model: ModelSelectFn = std::rc::Rc::new(move |chosen, clear_eff| {
        let mut payload = json!({
            "op": "set_profile_options",
            "id": pid_m,
            "model": chosen.as_deref().map(|s| json!(s)).unwrap_or(Value::Null),
        });
        if chosen.as_deref().is_some_and(is_orchestrator_model_id) {
            payload["effort"] = Value::Null;
        } else {
            payload["orchestrator"] = Value::Null;
            if clear_eff {
                payload["effort"] = Value::Null;
            }
        }
        dispatch_agents_save(ui, payload, |_| {});
    });
    let pid_e = profile_id.clone();
    let on_effort: EffortSelectFn = std::rc::Rc::new(move |chosen| {
        dispatch_agents_save(
            ui,
            json!({
                "op": "set_profile_options",
                "id": pid_e,
                "effort": chosen.as_deref().map(|s| json!(s)).unwrap_or(Value::Null),
            }),
            |_| {},
        );
    });
    render_model_effort_buttons(
        ui,
        profile_id,
        effective_model,
        explicit_model,
        effective_effort,
        explicit_effort,
        is_identity,
        "Default (Inherit from Agent)",
        caps,
        custom_model_target,
        custom_model_draft,
        on_model,
        on_effort,
    )
}

#[allow(clippy::too_many_arguments)]
fn orchestrator_role_row(
    ui: Ui,
    role: OrchestratorRoleKind,
    row_label: &'static str,
    candidate_models: Vec<canban_profiles::ModelOption>,
    explicit_model: Option<String>,
    inherited_model: Option<String>,
    explicit_effort: Option<String>,
    inherited_effort: Option<String>,
    has_parent: bool,
    target_key: String,
    caps: &EngineCatalogCaps,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
    on_update: std::rc::Rc<dyn Fn(OrchestratorFieldUpdate)>,
) -> AnyElement {
    let default_fallback = role.default_model();
    let model_label = match (&explicit_model, &inherited_model) {
        (Some(m), _) => format!("{} ▾", caps.model_label(m)),
        (None, Some(inh)) if has_parent => format!("{} (auto) ▾", caps.model_label(inh)),
        _ => format!("{} (default) ▾", caps.model_label(default_fallback)),
    };
    let caps_m = caps.clone();
    let cand_m = candidate_models;
    let exp_m = explicit_model.clone();
    let inh_m = inherited_model.clone();
    let exp_e_for_m = explicit_effort.clone();
    let tk_m = target_key;
    let on_upd_m = on_update.clone();

    let model_btn = compact_action(
        model_label,
        if explicit_model.is_some() {
            Variant::Secondary
        } else {
            Variant::Quiet
        },
        move |_| {
            use nsh_core::element::desktop::{MenuItem, popup_menu};
            let opts = cand_m.clone();
            let caps_c = caps_m.clone();
            let cur_exp_m = exp_m.clone();
            let cur_inh_m = inh_m.clone();
            let cur_exp_e = exp_e_for_m.clone();
            let custom_k = tk_m.clone();
            let cb = on_upd_m.clone();
            ui.spawn(async move {
                let mut items = Vec::new();
                let def_base = if has_parent && let Some(inh) = cur_inh_m.as_deref() {
                    format!("Default (Inherit: {})", caps_c.model_label(inh))
                } else {
                    format!("Default ({})", caps_c.model_label(default_fallback))
                };
                let def_title = if cur_exp_m.is_none() {
                    format!("✓ {def_base}")
                } else {
                    def_base
                };
                let clear_cb = cb.clone();
                let clear_eff_on_reset = cur_inh_m.as_deref().is_none_or(|inh| {
                    caps_c
                        .cleaned_effort_for_model(Some(inh), cur_exp_e.as_deref())
                        .is_some()
                });
                items.push(MenuItem::new(def_title).on_select(move || {
                    custom_model_target.set(None);
                    clear_cb(OrchestratorFieldUpdate::Model(
                        role,
                        None,
                        clear_eff_on_reset,
                    ));
                }));
                items.push(MenuItem::separator());
                for opt in opts {
                    let mid = opt.id;
                    let checked = cur_exp_m.as_deref() == Some(mid.as_str());
                    let prefix = if checked { "✓ " } else { "" };
                    let title = if opt.label == mid {
                        format!("{prefix}{mid}")
                    } else {
                        format!("{prefix}{} ({mid})", opt.label)
                    };
                    let chosen = mid.clone();
                    let clear_eff = caps_c
                        .cleaned_effort_for_model(Some(&chosen), cur_exp_e.as_deref())
                        .is_some();
                    let pick_cb = cb.clone();
                    items.push(MenuItem::new(title).on_select(move || {
                        custom_model_target.set(None);
                        pick_cb(OrchestratorFieldUpdate::Model(
                            role,
                            Some(chosen.clone()),
                            clear_eff,
                        ));
                    }));
                }
                items.push(MenuItem::separator());
                let seed = cur_exp_m.or(cur_inh_m).unwrap_or_default();
                items.push(MenuItem::new("Custom model ID…").on_select(move || {
                    custom_model_draft.set(seed.clone());
                    custom_model_target.set(Some(custom_k.clone()));
                }));
                popup_menu(items).await;
            });
        },
    );

    let active_model = explicit_model
        .as_deref()
        .or(inherited_model.as_deref())
        .unwrap_or(default_fallback);
    let model_changed_from_parent =
        explicit_model.is_some() && inherited_model.is_some() && explicit_model != inherited_model;
    let role_efforts = caps.efforts_for_model(Some(active_model));
    let effort_btn = if let Some(efforts_list) = role_efforts {
        let valid_exp_e = explicit_effort.filter(|e| efforts_list.iter().any(|l| l == e));
        let valid_inh_e = if has_parent && !model_changed_from_parent {
            inherited_effort.filter(|e| efforts_list.iter().any(|l| l == e))
        } else {
            None
        };
        let default_eff = canban_profiles::default_agy_effort_for_model(&caps.models, active_model);
        let eff_label = match (&valid_exp_e, &valid_inh_e) {
            (Some(e), _) => format!("Effort: {e} ▾"),
            (None, Some(inh)) => format!("Effort: {inh} (auto) ▾"),
            _ => match default_eff.as_deref() {
                Some(de) => format!("Effort: {de} (default) ▾"),
                None => "Effort: Default ▾".into(),
            },
        };
        let exp_e_menu = valid_exp_e.clone();
        let inh_e_menu = valid_inh_e;
        let on_upd_e = on_update;
        compact_action(
            eff_label,
            if valid_exp_e.is_some() {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| {
                use nsh_core::element::desktop::{MenuItem, popup_menu};
                let opts = efforts_list.clone();
                let cur_exp = exp_e_menu.clone();
                let cur_inh = inh_e_menu.clone();
                let def_e = default_eff.clone();
                let cb = on_upd_e.clone();
                ui.spawn(async move {
                    let mut items = Vec::new();
                    let def_base = if let Some(inh) = cur_inh.as_deref() {
                        format!("Default (Inherit: {inh})")
                    } else if let Some(de) = def_e.as_deref() {
                        format!("Default ({de})")
                    } else {
                        "Default".into()
                    };
                    let def_title = if cur_exp.is_none() {
                        format!("✓ {def_base}")
                    } else {
                        def_base
                    };
                    let clear_cb = cb.clone();
                    items.push(MenuItem::new(def_title).on_select(move || {
                        clear_cb(OrchestratorFieldUpdate::Effort(role, None));
                    }));
                    items.push(MenuItem::separator());
                    for eff in opts {
                        let checked = cur_exp.as_deref() == Some(eff.as_str());
                        let title = if checked {
                            format!("✓ {eff}")
                        } else {
                            eff.clone()
                        };
                        let chosen = eff.clone();
                        let pick_cb = cb.clone();
                        items.push(MenuItem::new(title).on_select(move || {
                            pick_cb(OrchestratorFieldUpdate::Effort(role, Some(chosen.clone())));
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
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(view().w(px(88.)).child(muted(row_label).text_size(px(12.))))
        .child(model_btn)
        .child(effort_btn)
        .into_element()
}

#[allow(clippy::too_many_arguments)]
fn orchestrator_subpanel(
    ui: Ui,
    target_id: String,
    caps: EngineCatalogCaps,
    explicit_orch: Option<OrchestratorConfig>,
    inherited_orch: Option<OrchestratorConfig>,
    has_parent: bool,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
    on_update: std::rc::Rc<dyn Fn(OrchestratorFieldUpdate)>,
) -> AnyElement {
    let exp = explicit_orch.unwrap_or_default().normalized();
    let inh = inherited_orch.unwrap_or_default().normalized();
    let planner_key = format!("{target_id}:{}", OrchestratorRoleKind::Planner.key_suffix());
    let worker_key = format!("{target_id}:{}", OrchestratorRoleKind::Worker.key_suffix());
    let active_custom = custom_model_target.get();
    let editing_role = if active_custom.as_deref() == Some(planner_key.as_str()) {
        Some((OrchestratorRoleKind::Planner, exp.planner_effort.clone()))
    } else if active_custom.as_deref() == Some(worker_key.as_str()) {
        Some((OrchestratorRoleKind::Worker, exp.worker_effort.clone()))
    } else {
        None
    };

    let planner_row = orchestrator_role_row(
        ui,
        OrchestratorRoleKind::Planner,
        "Orchestrator",
        caps.planner_models(),
        exp.planner_model,
        inh.planner_model,
        exp.planner_effort,
        inh.planner_effort,
        has_parent,
        planner_key,
        &caps,
        custom_model_target,
        custom_model_draft,
        on_update.clone(),
    );
    let worker_row = orchestrator_role_row(
        ui,
        OrchestratorRoleKind::Worker,
        "Worker",
        caps.worker_models(),
        exp.worker_model,
        inh.worker_model,
        exp.worker_effort,
        inh.worker_effort,
        has_parent,
        worker_key,
        &caps,
        custom_model_target,
        custom_model_draft,
        on_update.clone(),
    );

    let custom_editor = if let Some((role, cur_eff)) = editing_role {
        let caps_c = caps.clone();
        let cb_save = on_update.clone();
        let cb_reset = on_update;
        let placeholder = match role {
            OrchestratorRoleKind::Planner => "e.g. gemini-4-neon, claude-opus-5-5…",
            OrchestratorRoleKind::Worker => "e.g. gemini-4-hydrogen, claude-sonnet-5-5…",
        };
        let role_title = match role {
            OrchestratorRoleKind::Planner => "Custom Orchestrator model ID",
            OrchestratorRoleKind::Worker => "Custom Worker model ID",
        };
        view()
            .flex_row()
            .flex_wrap()
            .items_end()
            .gap_2()
            .pt_1()
            .child(
                view().flex_1().min_w(px(200.)).child(
                    bound_field(
                        move || custom_model_draft.get(),
                        move |v| custom_model_draft.set(v.to_owned()),
                        placeholder,
                    )
                    .label(role_title),
                ),
            )
            .child(compact_action("Apply", Variant::Primary, move |_| {
                let trimmed = custom_model_draft.get_untracked().trim().to_owned();
                if trimmed.is_empty() {
                    cb_save(OrchestratorFieldUpdate::Model(role, None, true));
                } else {
                    let clear_eff = caps_c
                        .cleaned_effort_for_model(Some(&trimmed), cur_eff.as_deref())
                        .is_some();
                    cb_save(OrchestratorFieldUpdate::Model(
                        role,
                        Some(trimmed),
                        clear_eff,
                    ));
                }
                custom_model_target.set(None);
            }))
            .child(compact_action("Reset default", Variant::Quiet, move |_| {
                cb_reset(OrchestratorFieldUpdate::Model(role, None, true));
                custom_model_target.set(None);
            }))
            .child(compact_action("Cancel", Variant::Quiet, move |_| {
                custom_model_target.set(None);
            }))
            .into_element()
    } else {
        view().into_element()
    };

    view()
        .pl_3()
        .py_1()
        .border_l_1()
        .border_color(BORDER)
        .gap_1p5()
        .child(planner_row)
        .child(worker_row)
        .child(custom_editor)
        .into_element()
}

#[allow(clippy::too_many_arguments)]
fn global_orchestrator_subpanel(
    ui: Ui,
    profile_id: String,
    explicit_model: Option<String>,
    explicit_orch: Option<OrchestratorConfig>,
    inherited_orch: Option<OrchestratorConfig>,
    is_identity: bool,
    caps: EngineCatalogCaps,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
) -> AnyElement {
    let pid = profile_id.clone();
    let exp_m = explicit_model;
    let exp_o = explicit_orch.clone();
    let on_update: std::rc::Rc<dyn Fn(OrchestratorFieldUpdate)> = std::rc::Rc::new(move |upd| {
        let mut next = exp_o.clone().unwrap_or_default();
        apply_orchestrator_update(&mut next, upd);
        let normalized = next.into_non_empty();
        let mut payload = json!({
            "op": "set_profile_options",
            "id": pid,
            "orchestrator": normalized.as_ref().map(|o| json!(o)).unwrap_or(Value::Null),
        });
        if exp_m.is_none() && !is_identity {
            payload["model"] = json!(ORCHESTRATOR_MODEL_ID);
        }
        dispatch_agents_save(ui, payload, |_| {});
    });
    orchestrator_subpanel(
        ui,
        profile_id,
        caps,
        explicit_orch,
        inherited_orch,
        is_identity,
        custom_model_target,
        custom_model_draft,
        on_update,
    )
}

#[allow(clippy::too_many_arguments)]
fn project_orchestrator_subpanel(
    ui: Ui,
    role_id: String,
    explicit_orch: Option<OrchestratorConfig>,
    inherited_orch: Option<OrchestratorConfig>,
    caps: EngineCatalogCaps,
    overrides: Signal<BTreeMap<String, ProjectAgentOverride>>,
    custom_model_target: Signal<Option<String>>,
    custom_model_draft: Signal<String>,
) -> AnyElement {
    let rk = role_id.clone();
    let on_update: std::rc::Rc<dyn Fn(OrchestratorFieldUpdate)> = std::rc::Rc::new(move |upd| {
        let k = rk.clone();
        overrides.update(|m| {
            let entry = m.entry(k.clone()).or_default();
            let mut next = entry.orchestrator.clone().unwrap_or_default();
            apply_orchestrator_update(&mut next, upd);
            entry.orchestrator = next.into_non_empty();
            if entry.is_empty() {
                m.remove(&k);
            }
        });
    });
    orchestrator_subpanel(
        ui,
        role_id,
        caps,
        explicit_orch,
        inherited_orch,
        true,
        custom_model_target,
        custom_model_draft,
        on_update,
    )
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
                    "e.g. gemini-4-carbon, gpt-6.1-sol, claude-opus-4-6…",
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
            if !is_orchestrator_model_id(&trimmed) {
                payload["orchestrator"] = Value::Null;
            }
            if let Some(clear_eff) = cleaned_eff {
                payload["effort"] = clear_eff;
            }
            dispatch_agents_save(ui, payload, move |_| {
                custom_model_target.set(None);
            });
        }))
        .child(compact_action("Reset default", Variant::Quiet, move |_| {
            let id_c = clear_id.clone();
            dispatch_agents_save(
                ui,
                json!({
                    "op": "set_profile_options",
                    "id": id_c,
                    "model": Value::Null,
                    "orchestrator": Value::Null,
                }),
                move |_| {
                    custom_model_target.set(None);
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
    probing: bool,
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
                if installed {
                    ""
                } else if probing {
                    " (checking)"
                } else {
                    " (not installed)"
                },
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
                            } else if probing {
                                format!("@{id} · checking…")
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
    if ui.agent_catalog.with_untracked(Value::is_null) || !ui.agent_catalog_loading.get_untracked()
    {
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
                            let probing = ui.agent_catalog.with(Value::is_null)
                                && ui.agent_catalog_loading.get();
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
                                agent_tile(id, display, is_inst, probing, checked, move || {
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
                            let loading_catalog = rows.is_empty() && ui.agent_catalog_loading.get();
                            view()
                                .gap_2p5()
                                .child(if loading_catalog {
                                    loading_panel(
                                        "Scanning installed CLI Agents…",
                                        "Detecting local agent binaries and presets.",
                                    )
                                    .into_element()
                                } else if installed.is_empty() {
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
                                    let roles_for_cb = roles.clone();
                                    dispatch_then(
                                        ui,
                                        "agent.setup",
                                        json!({
                                            "enabled_agents": enabled,
                                            "default_engine": primary,
                                            "identity_engines": roles,
                                        }),
                                        move |val| {
                                            let next_ep = ui.agent_config_epoch.get_untracked() + 1;
                                            ui.agent_config_epoch.set(next_ep);
                                            if let Ok(cfg) = serde_json::from_value::<Config>(val) {
                                                ui.agent_catalog.update(|cat| {
                                                    if cat.is_null() {
                                                        return;
                                                    }
                                                    if let Some(arr) =
                                                        cat["raw_profiles"].as_array_mut()
                                                    {
                                                        for item in arr.iter_mut() {
                                                            let Some(id) = item["id"].as_str().map(str::to_owned) else {
                                                                continue;
                                                            };
                                                            if let Some(prof) = cfg.profiles.iter().find(|p| p.id == id) {
                                                                item["enabled"] = json!(prof.enabled);
                                                                if prof.is_identity() {
                                                                    let new_eng = roles_for_cb.get(&id).cloned().or_else(|| prof.preset.clone());
                                                                    if let Some(eng) = new_eng {
                                                                        if item["preset"].as_str() != Some(eng.as_str())
                                                                            && let Some(obj) = item.as_object_mut()
                                                                        {
                                                                            obj.remove("model");
                                                                            obj.remove("effort");
                                                                            obj.remove("orchestrator");
                                                                        }
                                                                        item["preset"] = json!(eng);
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    sync_catalog_agents_with_profiles(cat, &cfg.profiles);
                                                });
                                            }
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

fn agent_env_inline_editor(
    ui: Ui,
    profile_id: String,
    host_id: Signal<String>,
    env_target: Signal<Option<String>>,
) -> AnyElement {
    let pid_scope = profile_id.clone();
    let scope_key = computed(move || format!("{}:{pid_scope}", host_id.get()));
    let pid_meta = profile_id.clone();
    let source_meta = computed(move || {
        let hid = host_id.get();
        ui.snapshot.with(|s| s.agent_env(&hid, &pid_meta))
    });
    let on_save =
        crate::components::env_table::env_save_callback(ui, host_id, "agent", move || {
            Some(profile_id.clone())
        });

    view()
        .ml_2()
        .pl_3p5()
        .py_2()
        .border_l_1()
        .border_color(BORDER)
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    muted(
                        "Per-profile Secrets & Variables (local to this host; overrides Global).",
                    )
                    .text_size(px(12.)),
                )
                .child(compact_action("Done", Variant::Quiet, move |_| {
                    env_target.set(None);
                })),
        )
        .child(crate::components::env_table::env_table(
            ui,
            crate::components::env_table::EnvTableConfig {
                host_id,
                project_id: None,
                scope_key,
                source_meta,
                save_label: "Save Env",
                on_save,
                dirty_out: None,
            },
        ))
        .into_element()
}

fn agent_env_toggle_button(
    ui: Ui,
    profile_id: String,
    env_target: Signal<Option<String>>,
) -> impl IntoElement {
    let pid_state = profile_id.clone();
    let state = computed(move || {
        let is_open = env_target.get().as_deref() == Some(pid_state.as_str());
        let host = ui.settings_host.get();
        let count = ui.snapshot.with(|s| s.agent_env(&host, &pid_state).len());
        (is_open, count)
    });
    dynamic(move || {
        let (is_open, count) = state.get();
        let label = if count > 0 {
            format!("{} Env · {count}", if is_open { "▾" } else { "▸" })
        } else if is_open {
            "▾ Env".into()
        } else {
            "▸ Env".into()
        };
        let variant = if is_open || count > 0 {
            Variant::Secondary
        } else {
            Variant::Quiet
        };
        let toggle_pid = profile_id.clone();
        compact_action(label, variant, move |_| {
            if env_target.get_untracked().as_deref() == Some(toggle_pid.as_str()) {
                env_target.set(None);
            } else {
                env_target.set(Some(toggle_pid.clone()));
            }
        })
        .into_element()
    })
}

/// Global Settings section for Agents, Custom CLI Agents, Default `@identity`, and Role Identities.
pub fn global_settings_section(ui: Ui) -> impl IntoElement {
    if ui.agent_catalog.with_untracked(Value::is_null) || !ui.agent_catalog_loading.get_untracked()
    {
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
    let env_target = signal(Option::<String>::None);
    let skill_copied = signal(false);
    let skill_cmd = skill_install_command();
    let skill_cmd_copy = skill_cmd.clone();

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
                        format!("Copy profiles from {}", crate::platform::LOCAL_HOST_NAME),
                        Variant::Secondary,
                        move |_| {
                            let target = ui.settings_host.get_untracked();
                            let local_cfg = crate::config_for_host("local");
                            dispatch_agents_save(
                                ui,
                                json!({
                                    "host_id": target,
                                    "op": "replace_profiles",
                                    "profiles": local_cfg.profiles,
                                    "default_agent": local_cfg.default_agent,
                                }),
                                move |_| {
                                    refresh_catalog(ui);
                                    ui.notify(format!(
                                        "Copied Agent & @identity profiles from {} (existing target Env & Secrets preserved)",
                                        crate::platform::LOCAL_HOST_NAME
                                    ));
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
            .child(compact_action(
                move || {
                    if ui.agent_catalog_loading.get() {
                        "Scanning…"
                    } else {
                        "Reload"
                    }
                },
                Variant::Quiet,
                move |_| {
                    dispatch_then(ui, "config.reload", json!({}), move |_| {
                        ui.agent_config_epoch.update(|v| *v += 1);
                        refresh_catalog(ui);
                    });
                },
            )),
    )
    .child(
        settings_group().row(settings_row(
            "Default Agent",
            "Used when a Workflow step omits profile, and pre-selected for reviews, Workflows, and @.",
            dynamic(move || {
                let current_default = ui.default_agent.get();
                let enabled_profiles = ui.profiles.get();
                let cur_mark = enabled_profiles
                    .iter()
                    .find(|p| p.id == current_default)
                    .map(|p| crate::profiles::provider(p).0)
                    .unwrap_or(Icon::Agent);
                compact_action_with_icon(
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
                                            dispatch_agents_save(
                                                ui,
                                                json!({"op": "set_default", "default_agent": pid}),
                                                |_| {},
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
            }),
        )),
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
                    view()
                        .p_3()
                        .gap_2p5()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
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
                                        dispatch_agents_save(
                                            ui,
                                            json!({
                                                "op": "upsert_identity",
                                                "id": id,
                                                "engine": eng,
                                                "role_instruction": prompt,
                                            }),
                                            move |_| {
                                                adding_identity.set(false);
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
                    all.iter().filter(|p| p.is_identity()).cloned().collect();
                let active_edit = editing_role.get();
                let active_custom_model = custom_model_target.get();
                let total_identities = identities.len();
                view()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(BORDER)
                    .bg(SURFACE)
                    .overflow_hidden()
                    .min_w(px(0.))
                    .children(identities.into_iter().enumerate().map(|(idx, prof)| {
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
                        let raw_opts = raw_profile_options(&cat_val, &pid);
                        let explicit_model = raw_opts
                            .as_ref()
                            .map(|r| r.model.clone())
                            .unwrap_or_else(|| prof.model.clone());
                        let explicit_effort = raw_opts
                            .as_ref()
                            .map(|r| r.effort.clone())
                            .unwrap_or_else(|| prof.effort.clone());
                        let explicit_orch = raw_opts
                            .as_ref()
                            .map(|r| r.orchestrator.clone())
                            .unwrap_or_else(|| prof.effective_orchestrator());
                        let peer_opts = peer_inherited_options(&all, &prof);
                        let is_orch_mode = caps.supports_orchestrator
                            && explicit_model
                                .as_deref()
                                .or(prof.model.as_deref())
                                .is_some_and(is_orchestrator_model_id);
                        let engines_for_menu = engines.clone();
                        let switch_pid = pid.clone();
                        let role_text = prof.role_instruction.clone();
                        let short_desc = builtin_role_summary(&pid, &role_text);
                        let edit_pid = pid.clone();
                        let edit_seed = role_text.clone();
                        let del_pid = pid.clone();
                        let toggle_pid = pid.clone();
                        let prof_enabled = prof.enabled;

                        let mut row = view().px_3p5().py_2p5().gap_2();
                        if idx + 1 < total_identities {
                            row = row.border_b_1().border_color(BORDER);
                        }
                        row
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
                                                                    dispatch_agents_save(
                                                                        ui,
                                                                        json!({
                                                                            "op": "upsert_identity",
                                                                            "id": r,
                                                                            "engine": eid,
                                                                            "enabled": true,
                                                                        }),
                                                                        |_| {},
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
                                                explicit_model.clone(),
                                                prof.effort.clone(),
                                                explicit_effort.clone(),
                                                true,
                                                caps.clone(),
                                                custom_model_target,
                                                custom_model_draft,
                                            ))
                                            .child(agent_env_toggle_button(
                                                ui,
                                                pid.clone(),
                                                env_target,
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
                                                    dispatch_agents_save(
                                                        ui,
                                                        json!({
                                                            "op": "toggle_agent",
                                                            "id": toggle_pid,
                                                            "enabled": !prof_enabled,
                                                        }),
                                                        |_| {},
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
                                                            dispatch_agents_save(
                                                                ui,
                                                                json!({
                                                                    "op": "delete_profile",
                                                                    "id": d,
                                                                }),
                                                                |_| {},
                                                            );
                                                        },
                                                    )
                                                },
                                            )),
                                    ),
                            )
                            .child(if is_orch_mode {
                                view()
                                    .pl(px(23.))
                                    .child(global_orchestrator_subpanel(
                                        ui,
                                        pid.clone(),
                                        explicit_model,
                                        explicit_orch,
                                        peer_opts.orchestrator,
                                        true,
                                        caps.clone(),
                                        custom_model_target,
                                        custom_model_draft,
                                    ))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
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
                                            dispatch_agents_save(
                                                ui,
                                                json!({
                                                    "op": "upsert_identity",
                                                    "id": id_c,
                                                    "engine": eng_c,
                                                    "role_instruction": next_prompt,
                                                    "enabled": prof_enabled,
                                                }),
                                                move |_| {
                                                    editing_role.set(None);
                                                },
                                            );
                                        },
                                    )))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
                            .child({
                                let check_pid = pid.clone();
                                let mount_pid = pid;
                                when(
                                    move || {
                                        env_target.get().as_deref() == Some(check_pid.as_str())
                                    },
                                    move || {
                                        agent_env_inline_editor(
                                            ui,
                                            mount_pid.clone(),
                                            ui.settings_host,
                                            env_target,
                                        )
                                    },
                                )
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
                    view()
                        .p_3()
                        .gap_2p5()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
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
                                        dispatch_agents_save(
                                            ui,
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
                let probing = cat_val.is_null() && ui.agent_catalog_loading.get();
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
                    agent_tile(id, display, installed, probing, enabled, move || {
                        dispatch_agents_save(
                            ui,
                            json!({
                                "op": "toggle_agent",
                                "id": toggle_id,
                                "enabled": !enabled,
                            }),
                            |_| {},
                        );
                    })
                };

                let count = extra_rows.len();
                let show_extra = show_all_catalog.get();
                let loading_catalog = rows.is_empty() && ui.agent_catalog_loading.get();
                view()
                    .gap_3()
                    .child(if loading_catalog {
                        loading_panel(
                            "Scanning installed CLI Agents…",
                            "Detecting local agent binaries, templates, and supported models.",
                        )
                        .into_element()
                    } else {
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .children(primary_rows.into_iter().map(render_agent))
                            .into_element()
                    })
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
                        let total_direct = enabled_direct.len();
                        view()
                            .gap_2()
                            .pt_1()
                            .child(
                                muted("Agent defaults (inherited by @identities unless overridden above):")
                                    .text_size(px(12.)),
                            )
                            .child(
                                view()
                                    .rounded(px(RADIUS))
                                    .border_1()
                                    .border_color(BORDER)
                                    .bg(SURFACE)
                                    .overflow_hidden()
                                    .min_w(px(0.))
                                    .children(enabled_direct.into_iter().enumerate().map(|(idx, prof)| {
                                        let pid = prof.id.clone();
                                        let (mark, prov_name) = crate::profiles::provider(&prof);
                                        let eng_key = engine_key_for_profile(&prof);
                                        let caps = catalog_caps_for_engine(&rows, &eng_key);
                                        let raw_opts = raw_profile_options(&cat_val, &pid);
                                        let explicit_model = raw_opts
                                            .as_ref()
                                            .map(|r| r.model.clone())
                                            .unwrap_or_else(|| prof.model.clone());
                                        let explicit_effort = raw_opts
                                            .as_ref()
                                            .map(|r| r.effort.clone())
                                            .unwrap_or_else(|| prof.effort.clone());
                                        let explicit_orch = raw_opts
                                            .as_ref()
                                            .map(|r| r.orchestrator.clone())
                                            .unwrap_or_else(|| prof.effective_orchestrator());
                                        let is_orch_mode = caps.supports_orchestrator
                                            && explicit_model
                                                .as_deref()
                                                .or(prof.model.as_deref())
                                                .is_some_and(is_orchestrator_model_id);
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

                                        let mut row = view().px_3p5().py_2p5().gap_1p5();
                                        if idx + 1 < total_direct {
                                            row = row.border_b_1().border_color(BORDER);
                                        }
                                        row
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
                                                                explicit_model.clone(),
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
                                                            .child(agent_env_toggle_button(
                                                                ui,
                                                                pid.clone(),
                                                                env_target,
                                                            ))
                                                            .child(when(
                                                                move || is_pure_custom,
                                                                move || {
                                                                    let d = del_pid.clone();
                                                                    compact_action(
                                                                        "Delete",
                                                                        Variant::Danger,
                                                                        move |_| {
                                                                            dispatch_agents_save(
                                                                                ui,
                                                                                json!({
                                                                                    "op": "delete_profile",
                                                                                    "id": d,
                                                                                }),
                                                                                |_| {},
                                                                            );
                                                                        },
                                                                    )
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .child(if is_orch_mode {
                                                view()
                                                    .pl(px(23.))
                                                    .child(global_orchestrator_subpanel(
                                                        ui,
                                                        pid.clone(),
                                                        explicit_model,
                                                        explicit_orch,
                                                        None,
                                                        false,
                                                        caps.clone(),
                                                        custom_model_target,
                                                        custom_model_draft,
                                                    ))
                                                    .into_element()
                                            } else {
                                                view().into_element()
                                            })
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
                                                    pid.clone(),
                                                    preset_default,
                                                    command_target,
                                                    command_bin_draft,
                                                    command_args_draft,
                                                    command_error,
                                                )
                                            } else {
                                                view().into_element()
                                            })
                                            .child({
                                                let check_pid = pid.clone();
                                                let mount_pid = pid;
                                                when(
                                                    move || {
                                                        env_target.get().as_deref()
                                                            == Some(check_pid.as_str())
                                                    },
                                                    move || {
                                                        agent_env_inline_editor(
                                                            ui,
                                                            mount_pid.clone(),
                                                            ui.settings_host,
                                                            env_target,
                                                        )
                                                    },
                                                )
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
        settings_group()
            .title("CLI Skill & Configuration")
            .row(
                view()
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
                            .child(label("$CANBAN_CLI skill").font_weight(FontWeight::MEDIUM))
                            .child(
                                muted(skill_cmd)
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.5))
                                    .truncate(),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .flex_shrink_0()
                            .child(compact_action(
                                move || if skill_copied.get() { "✓ Copied" } else { "Copy" },
                                Variant::Secondary,
                                move |_| {
                                    copy_text(ui, skill_cmd_copy.clone());
                                    skill_copied.set(true);
                                },
                            ))
                            .child(compact_action(
                                format!(
                                    "Terminal {}",
                                    crate::commands::shortcuts::NEW_TERMINAL_DISPLAY
                                ),
                                Variant::Quiet,
                                move |_| crate::commands::execute(ui, "new-terminal"),
                            )),
                    ),
            )
            .row(
                view()
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
                            .child(label("Config file").font_weight(FontWeight::MEDIUM))
                            .child(
                                label(move || {
                                    format!(
                                        "{}/config/agents.json",
                                        crate::backend::home_for(&ui.settings_host.get())
                                    )
                                })
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .text_color(MUTED)
                                .truncate(),
                            ),
                    )
                    .child(compact_action("Copy path", Variant::Quiet, move |_| {
                        let path = format!(
                            "{}/config/agents.json",
                            crate::backend::home_for(&ui.settings_host.get_untracked())
                        );
                        copy_text(ui, path);
                    })),
            ),
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
    let rk_m = role_id.clone();
    let on_model: ModelSelectFn = std::rc::Rc::new(move |chosen, clear_eff| {
        let k = rk_m.clone();
        overrides.update(|m| match chosen {
            None => {
                if let Some(ov) = m.get_mut(&k) {
                    ov.model = None;
                    ov.orchestrator = None;
                    if ov.is_empty() {
                        m.remove(&k);
                    }
                }
            }
            Some(v) if is_orchestrator_model_id(&v) => {
                let entry = m.entry(k).or_default();
                entry.model = Some(ORCHESTRATOR_MODEL_ID.to_owned());
                entry.effort = None;
            }
            Some(v) => {
                let entry = m.entry(k).or_default();
                entry.model = Some(v);
                entry.orchestrator = None;
                if clear_eff {
                    entry.effort = None;
                }
            }
        });
    });
    let rk_e = role_id.clone();
    let on_effort: EffortSelectFn = std::rc::Rc::new(move |chosen| {
        let k = rk_e.clone();
        overrides.update(|m| match chosen {
            None => {
                if let Some(ov) = m.get_mut(&k) {
                    ov.effort = None;
                    if ov.is_empty() {
                        m.remove(&k);
                    }
                }
            }
            Some(v) => {
                m.entry(k).or_default().effort = Some(v);
            }
        });
    });
    render_model_effort_buttons(
        ui,
        role_id,
        inherited_model,
        explicit_model,
        inherited_effort,
        explicit_effort,
        true,
        "Default (Inherit Global)",
        caps,
        custom_model_target,
        custom_model_draft,
        on_model,
        on_effort,
    )
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
                    "e.g. gemini-4-carbon, gpt-6.1-sol, claude-opus-4-6…",
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
                        ov.orchestrator = None;
                        if ov.is_empty() {
                            m.remove(&k);
                        }
                    }
                } else {
                    let entry = m.entry(k).or_default();
                    if !is_orchestrator_model_id(&trimmed) {
                        entry.orchestrator = None;
                    }
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
                    ov.orchestrator = None;
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
) -> RawProfileOptions {
    let global_eng = engine_key_for_profile(prof);
    let Some(target) = override_agent.filter(|s| !s.trim().is_empty()) else {
        return RawProfileOptions {
            model: prof.model.clone(),
            effort: if prof.is_orchestrator() {
                None
            } else {
                prof.effort.clone()
            },
            orchestrator: prof
                .effective_orchestrator()
                .and_then(OrchestratorConfig::into_non_empty),
        };
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
    let peer_is_orch = peer.is_some_and(Profile::is_orchestrator);
    let peer_effort = if peer_is_orch {
        None
    } else {
        peer.and_then(|p| p.effort.as_deref().map(str::trim))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let peer_orch = peer.and_then(Profile::effective_orchestrator);
    if same_engine {
        let model = prof.model.clone().or(peer_model);
        let is_orch = model.as_deref().is_some_and(is_orchestrator_model_id);
        RawProfileOptions {
            model,
            effort: if is_orch {
                None
            } else {
                prof.effort.clone().or(peer_effort)
            },
            orchestrator: if is_orch {
                merge_orchestrator_pair(prof.effective_orchestrator(), peer_orch)
            } else {
                None
            },
        }
    } else {
        RawProfileOptions {
            model: peer_model,
            effort: peer_effort,
            orchestrator: peer_orch,
        }
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
                                                    entry.orchestrator = None;
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
                        let inherited =
                            inherited_project_options(&all, &prof, current_agent_ov.as_deref());
                        let is_orch_mode = caps.supports_orchestrator
                            && current_ov
                                .model
                                .as_deref()
                                .or(inherited.model.as_deref())
                                .is_some_and(is_orchestrator_model_id);
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
                                                                ov.orchestrator = None;
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
                                                                    entry.orchestrator = None;
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
                                        inherited.model,
                                        current_ov.model,
                                        inherited.effort,
                                        current_ov.effort,
                                        caps.clone(),
                                        overrides,
                                        custom_model_target,
                                        custom_model_draft,
                                    )),
                            )
                            .child(if is_orch_mode {
                                view()
                                    .pl(px(26.))
                                    .child(project_orchestrator_subpanel(
                                        ui,
                                        role_id.clone(),
                                        current_ov.orchestrator,
                                        inherited.orchestrator,
                                        caps.clone(),
                                        overrides,
                                        custom_model_target,
                                        custom_model_draft,
                                    ))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
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

    let env_scope_key = computed(move || format!("{}:{}", host_id.get(), ui.project.get()));
    let env_source_meta = computed(move || {
        let pid = ui.project.get();
        ui.snapshot.with(|s| s.project_env(&pid))
    });
    let on_project_env_save =
        crate::components::env_table::env_save_callback(ui, host_id, "project", move || {
            Some(ui.project.get_untracked())
        });

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
                                move |_| ui.open_settings_section("agents"),
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
                    )
                    .child(
                        settings_section(
                            "Project Environment & Secrets",
                            "Overrides Global and Agent/Identity secrets & variables for this Project; local to the Project's host.",
                        )
                        .child(crate::components::env_table::env_table(
                            ui,
                            crate::components::env_table::EnvTableConfig {
                                host_id,
                                project_id: Some(ui.project),
                                scope_key: env_scope_key,
                                source_meta: env_source_meta,
                                save_label: "Save Project Env",
                                on_save: on_project_env_save,
                                dirty_out: None,
                            },
                        )),
                    ),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimistic_agent_and_catalog_ops_update_explicit_and_inherited_options_without_rescan() {
        let mut catalog = json!({
            "agents": [
                {"id": "codex", "enabled": true, "configured": true, "installed": true, "supports_orchestrator": false},
                {"id": "agy", "enabled": false, "configured": false, "installed": true, "supports_orchestrator": true},
            ],
            "raw_profiles": [
                {"id": "codex", "enabled": true, "identity": false, "preset": "codex", "model": "gpt-5.4", "effort": "medium"},
                {"id": "developer", "enabled": true, "identity": true, "preset": "codex"}
            ]
        });
        let mut profiles = vec![
            Profile {
                id: "codex".into(),
                enabled: true,
                identity: false,
                preset: Some("codex".into()),
                model: Some("gpt-5.4".into()),
                effort: Some("medium".into()),
                ..Default::default()
            },
            Profile {
                id: "developer".into(),
                enabled: true,
                identity: true,
                preset: Some("codex".into()),
                model: Some("gpt-5.4".into()),
                effort: Some("medium".into()),
                ..Default::default()
            },
        ];
        let mut default_agent = "developer".to_owned();

        // 1. Set explicit model on @developer -> updates both `profiles` and `raw_profiles` immediately.
        let set_model = json!({
            "op": "set_profile_options",
            "id": "developer",
            "model": "gpt-5.4-mini",
        });
        apply_op_to_profiles(&mut profiles, &mut default_agent, &set_model, &catalog);
        apply_op_to_catalog(&mut catalog, &set_model, None);
        assert_eq!(
            raw_profile_options(&catalog, "developer"),
            Some(RawProfileOptions {
                model: Some("gpt-5.4-mini".into()),
                effort: None,
                orchestrator: None,
            })
        );
        assert_eq!(profiles[1].model.as_deref(), Some("gpt-5.4-mini"));

        // 2. Clear explicit model on @developer -> `raw_profiles` becomes None while `profiles` inherits peer model.
        let clear_model = json!({
            "op": "set_profile_options",
            "id": "developer",
            "model": Value::Null,
        });
        apply_op_to_profiles(&mut profiles, &mut default_agent, &clear_model, &catalog);
        apply_op_to_catalog(&mut catalog, &clear_model, None);
        assert_eq!(
            raw_profile_options(&catalog, "developer"),
            Some(RawProfileOptions::default())
        );
        assert_eq!(profiles[1].model.as_deref(), Some("gpt-5.4"));

        // 3. Toggle @agy enabled -> both `profiles` and `catalog["agents"]` reflect enabled immediately.
        let enable_agy = json!({
            "op": "toggle_agent",
            "id": "agy",
            "enabled": true,
        });
        apply_op_to_profiles(&mut profiles, &mut default_agent, &enable_agy, &catalog);
        apply_op_to_catalog(&mut catalog, &enable_agy, None);
        assert!(profiles.iter().any(|p| p.id == "agy" && p.enabled));
        assert_eq!(catalog["agents"][1]["enabled"], json!(true));
        assert_eq!(catalog["agents"][1]["configured"], json!(true));

        // 4. Switch @developer backing engine to agy -> clears stale explicit options and switches preset.
        let switch_eng = json!({
            "op": "upsert_identity",
            "id": "developer",
            "engine": "agy",
            "enabled": true,
        });
        apply_op_to_profiles(&mut profiles, &mut default_agent, &switch_eng, &catalog);
        apply_op_to_catalog(&mut catalog, &switch_eng, None);
        assert_eq!(
            profiles
                .iter()
                .find(|p| p.id == "developer")
                .and_then(|p| p.preset.as_deref()),
            Some("agy")
        );
        assert_eq!(
            raw_profile_options(&catalog, "developer"),
            Some(RawProfileOptions::default())
        );

        // 5. Switch @agy to Orchestrator mode -> propagates to inheriting @developer, allows partial override,
        //    and clears cleanly when switching back to a single model or another engine.
        let set_agy_orch = json!({
            "op": "set_profile_options",
            "id": "agy",
            "model": "orchestrator",
            "effort": Value::Null,
            "orchestrator": {
                "planner_model": "claude-opus-5-5",
                "planner_effort": "high",
                "worker_model": "gemini-4-hydrogen",
                "worker_effort": "high"
            }
        });
        apply_op_to_profiles(&mut profiles, &mut default_agent, &set_agy_orch, &catalog);
        apply_op_to_catalog(&mut catalog, &set_agy_orch, None);
        assert_eq!(
            profiles
                .iter()
                .find(|p| p.id == "developer")
                .and_then(|p| p.model.as_deref()),
            Some("orchestrator")
        );
        assert_eq!(
            profiles
                .iter()
                .find(|p| p.id == "developer")
                .and_then(|p| p.orchestrator.as_ref())
                .and_then(|o| o.planner_model.as_deref()),
            Some("claude-opus-5-5")
        );

        let override_dev_worker = json!({
            "op": "set_profile_options",
            "id": "developer",
            "orchestrator": { "worker_effort": "medium" }
        });
        apply_op_to_profiles(
            &mut profiles,
            &mut default_agent,
            &override_dev_worker,
            &catalog,
        );
        apply_op_to_catalog(&mut catalog, &override_dev_worker, None);
        let dev_orch = profiles
            .iter()
            .find(|p| p.id == "developer")
            .unwrap()
            .orchestrator
            .clone()
            .unwrap();
        assert_eq!(dev_orch.planner_model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(dev_orch.worker_effort.as_deref(), Some("medium"));

        let agy_single = json!({
            "op": "set_profile_options",
            "id": "agy",
            "model": "gemini-4-carbon",
            "orchestrator": Value::Null,
            "effort": "medium",
        });
        apply_op_to_profiles(&mut profiles, &mut default_agent, &agy_single, &catalog);
        apply_op_to_catalog(&mut catalog, &agy_single, None);
        assert_eq!(
            profiles
                .iter()
                .find(|p| p.id == "developer")
                .unwrap()
                .orchestrator,
            None
        );
    }
}
