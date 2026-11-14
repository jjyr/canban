//! Built-in Herdr interactive CLI Agent catalog.
//!
//! Covers all 24 official interactive CLI agents recognized by Herdr's terminal
//! detector and resume planner (`codex`, `claude`, `agy`, `gemini`, `opencode`,
//! `cursor`, `pi`, `omp`, `copilot`, `droid`, `amp`, `kimi`, `kiro`, `qwen`,
//! `devin`, `cline`, `grok`, `hermes`, `kilo`, `qodercli`, `letta`, `mastracode`,
//! `maki`, `muse`), plus helper construction for dynamic Herdr templates.

use canban_types::{ExitAction, Profile};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    /// Optional per-model effort capability override:
    /// - `None`: fall back to the parent `CatalogEntry.supported_efforts`.
    /// - `Some([])`: this model explicitly does not support reasoning effort flags.
    /// - `Some(levels)`: exact effort levels supported by this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_efforts: Option<Vec<String>>,
}

const AGY_EFFORT_ORDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Split an Antigravity (`agy`) model ID into `(base_family, effort_suffix)` when it ends
/// with a known reasoning effort suffix (`-low`, `-medium`, `-high`, `-xhigh`, `-max`).
pub fn split_agy_effort_suffix(model_id: &str) -> Option<(&str, &'static str)> {
    let trimmed = model_id.trim();
    for &eff in AGY_EFFORT_ORDER {
        if let Some(base) = trimmed.strip_suffix(eff).and_then(|p| p.strip_suffix('-'))
            && !base.is_empty()
        {
            return Some((base, eff));
        }
    }
    None
}

/// Populate `ModelOption.supported_efforts` for a flat list of `agy models` entries by
/// grouping sibling variants by base family (matching `agy`'s own family grouping):
/// - Multi-variant families (such as `gemini-4-carbon-{low,medium,high}` or
///   `gemini-4-hydrogen-{low,medium,high,xhigh}`) expose the exact sibling effort set.
/// - Standalone models with no effort variants (such as `gemini-pro-orchestrator` or
///   `gemini-pro-orchestrator-argon`) or single-fixed-tier families (such as
///   `gpt-oss-120b-medium`) receive `Some(vec![])` (effort unsupported).
pub fn annotate_agy_model_efforts(models: &mut [ModelOption]) {
    let mut family_efforts: std::collections::BTreeMap<String, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for opt in models.iter() {
        if let Some((base, eff)) = split_agy_effort_suffix(&opt.id) {
            let entry = family_efforts.entry(base.to_owned()).or_default();
            if !entry.contains(&eff) {
                entry.push(eff);
            }
        }
    }
    for list in family_efforts.values_mut() {
        list.sort_by_key(|eff| {
            AGY_EFFORT_ORDER
                .iter()
                .position(|candidate| candidate == eff)
                .unwrap_or(usize::MAX)
        });
    }
    for opt in models.iter_mut() {
        if let Some((base, _)) = split_agy_effort_suffix(&opt.id) {
            let siblings = family_efforts.get(base).map(Vec::as_slice).unwrap_or(&[]);
            if siblings.len() > 1 {
                opt.supported_efforts = Some(siblings.iter().map(|s| (*s).to_owned()).collect());
            } else {
                opt.supported_efforts = Some(Vec::new());
            }
        } else if let Some(siblings) = family_efforts.get(opt.id.trim())
            && !siblings.is_empty()
        {
            opt.supported_efforts = Some(siblings.iter().map(|s| (*s).to_owned()).collect());
        } else {
            opt.supported_efforts = Some(Vec::new());
        }
    }
}

/// Resolve the supported reasoning effort levels for a given model on an Agent catalog entry.
/// Returns `None` when the Agent itself does not support effort flags, `Some([])` when the
/// chosen model explicitly does not support effort, or `Some(non_empty)` when supported.
pub fn model_supported_efforts(
    entry: &CatalogEntry,
    models: &[ModelOption],
    model_id: Option<&str>,
) -> Option<Vec<String>> {
    if entry.effort_args.is_empty() && entry.supported_efforts.is_empty() {
        return None;
    }
    let Some(mid) = model_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(entry.supported_efforts.clone());
    };
    if let Some(opt) = models.iter().find(|m| m.id == mid)
        && let Some(effs) = &opt.supported_efforts
    {
        return Some(effs.clone());
    }
    if entry.id == "agy" {
        // Handle custom/unlisted `agy` model IDs consistently: known orchestrator slugs and
        // single fixed-tier slugs without multi-tier siblings in `models` omit `--effort`.
        let lower = mid.to_ascii_lowercase();
        if lower.contains("orchestrator") {
            return Some(Vec::new());
        }
        if let Some((base, _)) = split_agy_effort_suffix(mid) {
            let sibling_efforts: Vec<String> = AGY_EFFORT_ORDER
                .iter()
                .filter(|eff| {
                    let candidate = format!("{base}-{eff}");
                    models.iter().any(|m| m.id == candidate)
                })
                .map(|eff| (*eff).to_owned())
                .collect();
            if sibling_efforts.len() > 1 {
                return Some(sibling_efforts);
            }
            if sibling_efforts.len() == 1 {
                return Some(Vec::new());
            }
        }
    }
    if entry.id == "claude" && mid.to_ascii_lowercase().contains("haiku") {
        return Some(Vec::new());
    }
    Some(entry.supported_efforts.clone())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CatalogEntry {
    pub id: String,
    pub display_name: String,
    pub binary: String,
    pub resumable: bool,
    pub argv: Vec<String>,
    pub resume_argv: Vec<String>,
    pub exit: ExitAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_instruction: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_models: Vec<ModelOption>,
}

impl CatalogEntry {
    pub fn to_preset_profile(&self) -> Profile {
        Profile {
            id: self.id.clone(),
            name: Some(self.display_name.clone()),
            enabled: true,
            identity: false,
            preset: None,
            model: None,
            effort: None,
            role_instruction: String::new(),
            argv: Some(self.argv.clone()),
            resume_argv: (!self.resume_argv.is_empty()).then(|| self.resume_argv.clone()),
            exit: Some(self.exit.clone()),
            session_instruction: self.session_instruction.clone(),
        }
    }
}

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|v| (*v).to_owned()).collect()
}

fn m(items: &[(&str, &str)]) -> Vec<ModelOption> {
    items
        .iter()
        .map(|(id, label)| ModelOption {
            id: (*id).to_owned(),
            label: (*label).to_owned(),
            supported_efforts: None,
        })
        .collect()
}

fn agy_default_models() -> Vec<ModelOption> {
    let mut list = m(&[
        ("gemini-4-carbon-high", "Gemini 4 Carbon (High)"),
        ("gemini-4-carbon-medium", "Gemini 4 Carbon (Medium)"),
        ("gemini-4-carbon-low", "Gemini 4 Carbon (Low)"),
        ("gemini-4-argon-high", "Gemini 4 Argon (High)"),
        ("gemini-4-argon-medium", "Gemini 4 Argon (Medium)"),
        ("gemini-4-argon-low", "Gemini 4 Argon (Low)"),
        ("gemini-4-hydrogen-high", "Gemini 4 Hydrogen (High)"),
        ("gemini-4-hydrogen-xhigh", "Gemini 4 Hydrogen (XHigh)"),
        ("gemini-4-hydrogen-medium", "Gemini 4 Hydrogen (Medium)"),
        ("gemini-4-hydrogen-low", "Gemini 4 Hydrogen (Low)"),
        ("gemini-pro-orchestrator", "Orchestrator (Carbon)"),
        ("gemini-pro-orchestrator-argon", "Orchestrator (Argon)"),
        ("gemini-3.8-flash-high", "Gemini 3.8 Flash (High)"),
        ("gemini-3.8-flash-medium", "Gemini 3.8 Flash (Medium)"),
        ("gemini-3.8-flash-low", "Gemini 3.8 Flash (Low)"),
        ("claude-opus-5-5-high", "Claude Opus 5.5 (High)"),
        ("claude-opus-5-5-medium", "Claude Opus 5.5 (Medium)"),
        ("claude-opus-5-5-low", "Claude Opus 5.5 (Low)"),
        ("claude-sonnet-5-5-high", "Claude Sonnet 5.5 (High)"),
        ("claude-sonnet-5-5-medium", "Claude Sonnet 5.5 (Medium)"),
        ("claude-sonnet-5-5-low", "Claude Sonnet 5.5 (Low)"),
        ("gpt-oss-120b-medium", "GPT-OSS 120B (Medium)"),
    ]);
    annotate_agy_model_efforts(&mut list);
    list
}

fn bind_session_instruction(env_var: &str) -> String {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../prompts/profiles/bind_session.txt"
    ))
    .trim_end()
    .replace("{{env_var}}", env_var)
}

/// Returns all 24 built-in Herdr CLI agents in recommended onboarding order.
pub fn builtin_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            id: "codex".into(),
            display_name: "Codex".into(),
            binary: "codex".into(),
            resumable: true,
            argv: s(&[
                "codex",
                "--no-daemon",
                "--sandbox",
                "workspace-write",
                "--add-dir",
                "{home}",
                "{prompt}",
            ]),
            resume_argv: s(&[
                "codex",
                "--no-daemon",
                "--sandbox",
                "workspace-write",
                "--add-dir",
                "{home}",
                "resume",
                "{session_id}",
                "{prompt}",
            ]),
            exit: ExitAction::Input {
                text: "/exit".into(),
            },
            session_instruction: Some(bind_session_instruction("CODEX_THREAD_ID")),
            model_args: s(&["-m", "{model}"]),
            effort_args: s(&["-c", "model_reasoning_effort=\"{effort}\""]),
            supported_efforts: s(&["low", "medium", "high", "xhigh", "max"]),
            default_models: m(&[
                ("gpt-6.1-sol", "GPT-6.1-Sol"),
                ("gpt-6-astra", "GPT-6-Astra"),
                ("gpt-6.1-terra", "GPT-6.1 Terra"),
                ("gpt-5.4", "GPT-5.4"),
                ("gpt-5.3-codex", "GPT-5.3 Codex"),
            ]),
        },
        CatalogEntry {
            id: "claude".into(),
            display_name: "Claude Code".into(),
            binary: "claude".into(),
            resumable: true,
            argv: s(&["claude", "--session-id", "{session_id}", "{prompt}"]),
            resume_argv: s(&["claude", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Input {
                text: "/exit".into(),
            },
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: s(&["--effort", "{effort}"]),
            supported_efforts: s(&["low", "medium", "high", "xhigh", "max"]),
            default_models: vec![
                ModelOption {
                    id: "opus".into(),
                    label: "Claude Opus (Latest)".into(),
                    supported_efforts: None,
                },
                ModelOption {
                    id: "sonnet".into(),
                    label: "Claude Sonnet (Latest)".into(),
                    supported_efforts: None,
                },
                ModelOption {
                    id: "haiku".into(),
                    label: "Claude Haiku (Latest)".into(),
                    supported_efforts: Some(Vec::new()),
                },
                ModelOption {
                    id: "fable".into(),
                    label: "Claude Fable (Latest)".into(),
                    supported_efforts: None,
                },
            ],
        },
        CatalogEntry {
            id: "agy".into(),
            display_name: "Antigravity".into(),
            binary: "agy".into(),
            resumable: true,
            argv: s(&[
                "agy",
                "--add-dir",
                "{home}",
                "--add-dir",
                "{cwd}",
                "--add-dir",
                "{project_dir}",
                "--settings",
                "{agy_trusted_settings}",
                "-i",
                "{prompt}",
            ]),
            resume_argv: s(&[
                "agy",
                "--add-dir",
                "{home}",
                "--add-dir",
                "{cwd}",
                "--add-dir",
                "{project_dir}",
                "--settings",
                "{agy_trusted_settings}",
                "--conversation",
                "{session_id}",
                "-i",
                "{prompt}",
            ]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: s(&["--effort", "{effort}"]),
            supported_efforts: s(&["low", "medium", "high", "xhigh", "max"]),
            default_models: agy_default_models(),
        },
        CatalogEntry {
            id: "gemini".into(),
            display_name: "Gemini CLI".into(),
            binary: "gemini".into(),
            resumable: true,
            argv: s(&[
                "gemini",
                "--include-directories",
                "{home}",
                "--session-id",
                "{session_id}",
                "-i",
                "{prompt}",
            ]),
            resume_argv: s(&[
                "gemini",
                "--include-directories",
                "{home}",
                "--resume",
                "{session_id}",
                "-i",
                "{prompt}",
            ]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: m(&[
                ("gemini-2.5-pro", "Gemini 2.5 Pro"),
                ("gemini-2.5-flash", "Gemini 2.5 Flash"),
            ]),
        },
        CatalogEntry {
            id: "opencode".into(),
            display_name: "OpenCode".into(),
            binary: "opencode".into(),
            resumable: true,
            argv: s(&["opencode", "--prompt", "{prompt}"]),
            resume_argv: s(&[
                "opencode",
                "--session",
                "{session_id}",
                "--prompt",
                "{prompt}",
            ]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: s(&["--variant", "{effort}"]),
            supported_efforts: s(&["low", "medium", "high", "max"]),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "cursor".into(),
            display_name: "Cursor Agent".into(),
            binary: "cursor-agent".into(),
            resumable: true,
            argv: s(&["cursor-agent", "{prompt}"]),
            resume_argv: s(&["cursor-agent", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "pi".into(),
            display_name: "Pi".into(),
            binary: "pi".into(),
            resumable: true,
            argv: s(&["pi", "{prompt}"]),
            resume_argv: s(&["pi", "--session", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: s(&["--thinking", "{effort}"]),
            supported_efforts: s(&["low", "medium", "high", "xhigh"]),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "omp".into(),
            display_name: "Omp".into(),
            binary: "omp".into(),
            resumable: true,
            argv: s(&["omp", "{prompt}"]),
            resume_argv: s(&["omp", "--resume={session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "copilot".into(),
            display_name: "GitHub Copilot".into(),
            binary: "copilot".into(),
            resumable: true,
            argv: s(&["copilot", "-i", "{prompt}"]),
            resume_argv: s(&["copilot", "--resume={session_id}", "-i", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "droid".into(),
            display_name: "Droid".into(),
            binary: "droid".into(),
            resumable: true,
            argv: s(&["droid", "{prompt}"]),
            resume_argv: s(&["droid", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "amp".into(),
            display_name: "Amp".into(),
            binary: "amp".into(),
            resumable: false,
            argv: s(&["amp", "{prompt}"]),
            resume_argv: Vec::new(),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: Vec::new(),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "kimi".into(),
            display_name: "Kimi".into(),
            binary: "kimi".into(),
            resumable: true,
            argv: s(&["kimi", "{prompt}"]),
            resume_argv: s(&["kimi", "--session", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "kiro".into(),
            display_name: "Kiro".into(),
            binary: "kiro-cli".into(),
            resumable: false,
            argv: s(&["kiro-cli", "{prompt}"]),
            resume_argv: Vec::new(),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "qwen".into(),
            display_name: "Qwen Code".into(),
            binary: "qwen".into(),
            resumable: true,
            argv: s(&["qwen", "-i", "{prompt}"]),
            resume_argv: s(&["qwen", "--resume", "{session_id}", "-i", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "devin".into(),
            display_name: "Devin".into(),
            binary: "devin".into(),
            resumable: true,
            argv: s(&["devin", "{prompt}"]),
            resume_argv: s(&["devin", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: Vec::new(),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "cline".into(),
            display_name: "Cline".into(),
            binary: "cline".into(),
            resumable: false,
            argv: s(&["cline", "{prompt}"]),
            resume_argv: Vec::new(),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: Vec::new(),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "grok".into(),
            display_name: "Grok".into(),
            binary: "grok".into(),
            resumable: true,
            argv: s(&["grok", "{prompt}"]),
            resume_argv: s(&["grok", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "hermes".into(),
            display_name: "Hermes".into(),
            binary: "hermes".into(),
            resumable: true,
            argv: s(&["hermes", "{prompt}"]),
            resume_argv: s(&["hermes", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "kilo".into(),
            display_name: "Kilo".into(),
            binary: "kilo".into(),
            resumable: true,
            argv: s(&["kilo", "{prompt}"]),
            resume_argv: s(&["kilo", "--session", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "qodercli".into(),
            display_name: "Qoder".into(),
            binary: "qodercli".into(),
            resumable: true,
            argv: s(&["qodercli", "{prompt}"]),
            resume_argv: s(&["qodercli", "--resume", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "letta".into(),
            display_name: "Letta".into(),
            binary: "letta".into(),
            resumable: true,
            argv: s(&["letta", "{prompt}"]),
            resume_argv: s(&["letta", "--conversation", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["-m", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "mastracode".into(),
            display_name: "Mastra Code".into(),
            binary: "mastracode".into(),
            resumable: true,
            argv: s(&["mastracode", "{prompt}"]),
            resume_argv: s(&["mastracode", "--thread", "{session_id}", "{prompt}"]),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: s(&["--model", "{model}"]),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "maki".into(),
            display_name: "Maki".into(),
            binary: "maki".into(),
            resumable: false,
            argv: s(&["maki", "{prompt}"]),
            resume_argv: Vec::new(),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: Vec::new(),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
        CatalogEntry {
            id: "muse".into(),
            display_name: "Muse".into(),
            binary: "muse".into(),
            resumable: false,
            argv: s(&["muse", "{prompt}"]),
            resume_argv: Vec::new(),
            exit: ExitAction::Manual,
            session_instruction: None,
            model_args: Vec::new(),
            effort_args: Vec::new(),
            supported_efforts: Vec::new(),
            default_models: Vec::new(),
        },
    ]
}

/// Friendly display name for a known built-in preset or binary.
pub fn display_name(id_or_binary: &str) -> Option<&'static str> {
    match id_or_binary {
        "codex" => Some("Codex"),
        "claude" => Some("Claude Code"),
        "agy" | "antigravity" => Some("Antigravity"),
        "gemini" => Some("Gemini CLI"),
        "opencode" => Some("OpenCode"),
        "cursor" | "cursor-agent" => Some("Cursor Agent"),
        "pi" => Some("Pi"),
        "omp" => Some("Omp"),
        "copilot" => Some("GitHub Copilot"),
        "droid" => Some("Droid"),
        "amp" => Some("Amp"),
        "kimi" => Some("Kimi"),
        "kiro" | "kiro-cli" => Some("Kiro"),
        "qwen" => Some("Qwen Code"),
        "devin" => Some("Devin"),
        "cline" => Some("Cline"),
        "grok" => Some("Grok"),
        "hermes" => Some("Hermes"),
        "kilo" => Some("Kilo"),
        "qodercli" => Some("Qoder"),
        "letta" => Some("Letta"),
        "mastracode" => Some("Mastra Code"),
        "maki" => Some("Maki"),
        "muse" => Some("Muse"),
        _ => None,
    }
}

/// Build a Profile backed by a Herdr runtime template (`terminal.run` accepts template name in `argv[0]`).
pub fn herdr_template_profile(template_name: &str) -> Profile {
    Profile {
        id: format!("herdr-{template_name}"),
        name: Some(format!("Herdr · {template_name}")),
        enabled: true,
        identity: false,
        preset: Some(format!("herdr:{template_name}")),
        model: None,
        effort: None,
        role_instruction: String::new(),
        argv: Some(vec![template_name.to_owned(), "{prompt}".into()]),
        resume_argv: None,
        exit: Some(ExitAction::Manual),
        session_instruction: None,
    }
}

/// Look up catalog metadata (`model_args`, `effort_args`, `supported_efforts`, `default_models`)
/// for a profile's preset or launch binary.
pub fn catalog_entry_for_profile(profile: &Profile) -> Option<CatalogEntry> {
    let list = builtin_catalog();
    if let Some(preset) = profile.preset.as_deref() {
        let key = preset.strip_prefix("herdr:").unwrap_or(preset);
        if let Some(hit) = list.iter().find(|e| e.id == key || e.binary == key) {
            return Some(hit.clone());
        }
    }
    if !profile.is_identity()
        && let Some(hit) = list.iter().find(|e| e.id == profile.id)
    {
        return Some(hit.clone());
    }
    let bin = profile.command_name();
    list.into_iter().find(|e| e.binary == bin || e.id == bin)
}

/// Identify a running interactive Agent CLI from Herdr push metadata (`foreground` and `status`).
///
/// Relies strictly on data pushed in Herdr `Terminal` snapshots and `foreground` / `status` events.
/// Returns a canonical catalog ID (`"codex"`, `"claude"`, `"agy"`, etc.) when a known Agent CLI is
/// active in the foreground, or `None` when the terminal is finished, at an interactive shell
/// prompt, or running a non-Agent command.
pub fn identify_herdr_agent(value: &serde_json::Value) -> Option<&'static str> {
    if value["state"] == "finished" {
        return None;
    }
    let fg = value.get("foreground").filter(|v| v.is_object());
    let fg_name = fg.and_then(|v| v["name"].as_str());
    let fg_argv: Vec<&str> = fg
        .and_then(|v| v["argv"].as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    let status_app = value
        .get("status")
        .filter(|v| v.is_object())
        .and_then(|v| v["app"].as_str());
    identify_agent_process(status_app, fg_name, &fg_argv)
}

/// Pure classifier for a terminal's active foreground process and optional OSC 7501 `status.app`.
pub fn identify_agent_process(
    status_app: Option<&str>,
    fg_name: Option<&str>,
    fg_argv: &[&str],
) -> Option<&'static str> {
    if let Some(id) = match_foreground_argv(fg_argv) {
        return Some(id);
    }
    if let Some(name) = fg_name
        && let Some(id) = match_agent_binary_token(name)
    {
        return Some(id);
    }
    // Interactive shells retain the last OSC 7501 `status` emitted by an exited child until the
    // next OSC 7501 event. Never surface a stale `status.app` once foreground returns to a shell
    // or is clearly running an unrelated non-wrapper binary.
    let fg_head = fg_argv
        .first()
        .copied()
        .or(fg_name)
        .map(clean_program_basename);
    if let Some(head) = fg_head.as_deref()
        && is_interactive_shell(head)
        && !shell_has_inline_command(fg_argv)
    {
        return None;
    }
    if let Some(app) = status_app {
        let can_trust_status = match fg_head.as_deref() {
            None => true,
            Some(head) => is_runtime_or_wrapper(head) || match_agent_binary_token(head).is_some(),
        };
        if can_trust_status && let Some(id) = match_status_app(app) {
            return Some(id);
        }
    }
    None
}

fn match_foreground_argv(argv: &[&str]) -> Option<&'static str> {
    let mut idx = 0;
    while idx < argv.len() {
        let token = argv[idx].trim();
        if token.is_empty() {
            idx += 1;
            continue;
        }
        let base = clean_program_basename(token);
        if is_env_launcher(&base) {
            idx += 1;
            while idx < argv.len() {
                let arg = argv[idx].trim();
                if arg == "--" {
                    idx += 1;
                    break;
                }
                if arg == "-S"
                    || arg == "--split-string"
                    || arg == "-u"
                    || arg == "--unset"
                    || arg == "-C"
                    || arg == "--chdir"
                {
                    idx += 2;
                    continue;
                }
                if arg.starts_with('-') || arg.contains('=') {
                    idx += 1;
                    continue;
                }
                break;
            }
            continue;
        }
        if is_package_exec_launcher(&base) {
            idx += 1;
            if matches!(base.as_str(), "npm" | "pnpm" | "yarn")
                && idx < argv.len()
                && matches!(argv[idx].trim(), "exec" | "dlx")
            {
                idx += 1;
            } else if base == "uv"
                && idx + 1 < argv.len()
                && argv[idx].trim() == "tool"
                && argv[idx + 1].trim() == "run"
            {
                idx += 2;
            } else if base == "deno" && idx < argv.len() && argv[idx].trim() == "run" {
                idx += 1;
            }
            while idx < argv.len() {
                let arg = argv[idx].trim();
                if arg == "--" {
                    return argv.get(idx + 1).and_then(|target| {
                        let t = target.trim();
                        match_agent_path_or_package(t)
                            .or_else(|| match_agent_binary_token(&clean_program_basename(t)))
                    });
                }
                if matches!(arg, "-p" | "--package") {
                    if let Some(pkg) = argv.get(idx + 1)
                        && let Some(id) = match_agent_path_or_package(pkg)
                    {
                        return Some(id);
                    }
                    idx += 2;
                    continue;
                }
                if let Some(pkg) = arg
                    .strip_prefix("--package=")
                    .or_else(|| arg.strip_prefix("-p="))
                    && let Some(id) = match_agent_path_or_package(pkg)
                {
                    return Some(id);
                }
                if arg.starts_with('-') {
                    idx += 1;
                    continue;
                }
                return match_agent_path_or_package(arg)
                    .or_else(|| match_agent_binary_token(&clean_program_basename(arg)));
            }
            return None;
        }
        if is_script_interpreter(&base) {
            idx += 1;
            while idx < argv.len() {
                let arg = argv[idx].trim();
                if arg == "--" {
                    return argv.get(idx + 1).and_then(|target| {
                        let t = target.trim();
                        match_agent_path_or_package(t)
                            .or_else(|| match_agent_binary_token(&clean_program_basename(t)))
                    });
                }
                if matches!(arg, "-c" | "-e" | "--eval" | "-p" | "--print") {
                    return None;
                }
                if arg == "-m" {
                    return argv
                        .get(idx + 1)
                        .and_then(|m| match_python_module(m.trim()));
                }
                if interpreter_flag_takes_value(&base, arg) {
                    idx += 2;
                    continue;
                }
                if arg.starts_with('-') {
                    idx += 1;
                    continue;
                }
                return match_agent_path_or_package(arg)
                    .or_else(|| match_agent_binary_token(&clean_program_basename(arg)));
            }
            return None;
        }
        if is_interactive_shell(&base) {
            for (pos, raw) in argv.iter().enumerate().skip(idx + 1) {
                let arg = raw.trim();
                if (arg == "-c"
                    || (arg.starts_with('-') && !arg.starts_with("--") && arg.ends_with('c')))
                    && let Some(cmd) = argv.get(pos + 1)
                {
                    let parts: Vec<&str> = cmd.split_whitespace().collect();
                    return match_foreground_argv(&parts);
                }
            }
            return None;
        }
        return match_agent_binary_token(&base).or_else(|| match_agent_path_or_package(token));
    }
    None
}

fn clean_program_basename(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let file = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    let no_login = file.strip_prefix('-').unwrap_or(file);
    let lower = no_login.to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".mjs", ".cjs", ".js", ".py"] {
        if let Some(stem) = lower.strip_suffix(suffix)
            && !stem.is_empty()
        {
            return stem.to_owned();
        }
    }
    lower
}

fn strip_arch_or_version_suffix(name: &str) -> &str {
    // Handle packaged Mach-O / release binaries such as `grok-1.0.4-macos-aarch64` or `claude-2.1.0`.
    if let Some((head, tail)) = name.split_once('-')
        && tail
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || matches!(tail, "macos" | "darwin" | "linux"))
    {
        return head;
    }
    name
}

fn match_agent_binary_token(raw: &str) -> Option<&'static str> {
    let base = clean_program_basename(raw);
    let candidates = [base.as_str(), strip_arch_or_version_suffix(&base)];
    for candidate in candidates {
        let hit = match candidate {
            "codex" => Some("codex"),
            "claude" | "claude-code" => Some("claude"),
            "agy" | "antigravity" | "antigravity-cli" => Some("agy"),
            "gemini" | "gemini-cli" => Some("gemini"),
            "opencode" | "opencode-ai" => Some("opencode"),
            "cursor-agent" => Some("cursor"),
            "pi" | "pi-coding-agent" => Some("pi"),
            "omp" => Some("omp"),
            "copilot" | "github-copilot-cli" => Some("copilot"),
            "droid" => Some("droid"),
            "amp" => Some("amp"),
            "kimi" | "kimi-cli" => Some("kimi"),
            "kiro" | "kiro-cli" | "kiro-cli-chat" => Some("kiro"),
            "qwen" | "qwen-code" => Some("qwen"),
            "devin" | "devin-cli" => Some("devin"),
            "cline" | "cline-cli" => Some("cline"),
            "grok" | "grok-cli" => Some("grok"),
            "hermes" | "hermes-agent" => Some("hermes"),
            "kilo" | "kilocode" => Some("kilo"),
            "qodercli" | "qoder" => Some("qodercli"),
            "letta" | "letta-code" => Some("letta"),
            "mastracode" => Some("mastracode"),
            "maki" => Some("maki"),
            "muse" => Some("muse"),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

fn match_status_app(raw: &str) -> Option<&'static str> {
    let norm = raw.trim().to_ascii_lowercase();
    let norm = norm.strip_prefix("herdr:").unwrap_or(&norm);
    match norm {
        "cursor" | "cursor-agent" => Some("cursor"),
        "mastra" | "mastracode" | "mastra code" => Some("mastracode"),
        "claude code" => Some("claude"),
        "gemini cli" => Some("gemini"),
        "qwen code" => Some("qwen"),
        "github copilot" => Some("copilot"),
        other => match_agent_binary_token(other),
    }
}

fn match_python_module(module: &str) -> Option<&'static str> {
    let root = module
        .split('.')
        .next()
        .unwrap_or(module)
        .to_ascii_lowercase();
    match root.as_str() {
        "kimi_cli" | "kimi" => Some("kimi"),
        "aider" => None,
        "letta" | "letta_code" => Some("letta"),
        "hermes" | "hermes_agent" => Some("hermes"),
        other => match_agent_binary_token(other),
    }
}

fn match_agent_path_or_package(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let norm = trimmed.replace('\\', "/").to_ascii_lowercase();
    // Scoped package / Homebrew Cellar / Caskroom / virtualenv script path patterns.
    let without_ver = strip_package_version(&norm);
    for needle in [without_ver, norm.as_str()] {
        for (pattern, id) in [
            ("@openai/codex", "codex"),
            ("@anthropic-ai/claude-code", "claude"),
            ("@google/gemini-cli", "gemini"),
            ("@qwen-code/qwen-code", "qwen"),
            ("@mariozechner/pi-coding-agent", "pi"),
            ("@sourcegraph/amp", "amp"),
            ("@github/copilot", "copilot"),
            ("@vibe-kit/grok-cli", "grok"),
            ("@kilocode/cli", "kilo"),
            ("@letta-ai/letta-code", "letta"),
            ("opencode-ai", "opencode"),
            ("antigravity-cli", "agy"),
            ("kimi-cli", "kimi"),
            ("kiro-cli", "kiro"),
            ("qwen-code", "qwen"),
            ("claude-code", "claude"),
            ("gemini-cli", "gemini"),
            ("cursor-agent", "cursor"),
            ("qodercli", "qodercli"),
            ("mastracode", "mastracode"),
        ] {
            if contains_path_segment_or_pkg(needle, pattern) {
                return Some(id);
            }
        }
    }
    let base = clean_program_basename(trimmed);
    if !matches!(
        base.as_str(),
        "index" | "cli" | "main" | "bin" | "run" | "bundle" | "dist"
    ) {
        return match_agent_binary_token(&base);
    }
    None
}

fn strip_package_version(spec: &str) -> &str {
    if let Some(rest) = spec.strip_prefix('@') {
        if let Some(at) = rest.find('@') {
            return &spec[..at + 1];
        }
        spec
    } else if let Some((pkg, _)) = spec.split_once('@') {
        pkg
    } else {
        spec
    }
}

fn contains_path_segment_or_pkg(haystack: &str, pattern: &str) -> bool {
    if haystack == pattern {
        return true;
    }
    if let Some(idx) = haystack.find(pattern) {
        let before_ok = idx == 0 || matches!(haystack.as_bytes()[idx - 1], b'/' | b':');
        let end = idx + pattern.len();
        let after_ok =
            end == haystack.len() || matches!(haystack.as_bytes()[end], b'/' | b'@' | b':');
        return before_ok && after_ok;
    }
    false
}

fn is_env_launcher(base: &str) -> bool {
    matches!(base, "env" | "nohup" | "caffeinate" | "nice" | "arch")
}

fn is_package_exec_launcher(base: &str) -> bool {
    matches!(
        base,
        "npx" | "bunx" | "pnpx" | "uvx" | "npm" | "pnpm" | "yarn" | "uv"
    )
}

fn is_script_interpreter(base: &str) -> bool {
    matches!(base, "node" | "nodejs" | "bun" | "deno")
        || base == "python"
        || base.strip_prefix("python").is_some_and(|rest| {
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

fn interpreter_flag_takes_value(base: &str, flag: &str) -> bool {
    if flag.contains('=') {
        return false;
    }
    match base {
        "node" | "nodejs" | "bun" => matches!(
            flag,
            "-r" | "--require"
                | "--import"
                | "--loader"
                | "--experimental-loader"
                | "--conditions"
                | "-C"
                | "--input-type"
                | "--watch-path"
                | "--env-file"
                | "--config"
        ),
        "deno" => matches!(flag, "-c" | "--config" | "--import-map" | "--lock"),
        _ => matches!(flag, "-W" | "-X"),
    }
}

fn is_runtime_or_wrapper(base: &str) -> bool {
    is_env_launcher(base) || is_package_exec_launcher(base) || is_script_interpreter(base)
}

fn is_interactive_shell(base: &str) -> bool {
    matches!(
        base,
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "dash"
            | "ksh"
            | "mksh"
            | "tcsh"
            | "csh"
            | "nu"
            | "nushell"
            | "pwsh"
            | "powershell"
            | "elvish"
            | "xonsh"
            | "login"
    )
}

fn shell_has_inline_command(argv: &[&str]) -> bool {
    argv.iter().skip(1).any(|raw| {
        let arg = raw.trim();
        arg == "-c" || (arg.starts_with('-') && !arg.starts_with("--") && arg.ends_with('c'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identifies_direct_and_packaged_agent_binaries() {
        assert_eq!(
            identify_agent_process(None, Some("agy"), &["/opt/homebrew/bin/agy"]),
            Some("agy")
        );
        assert_eq!(
            identify_agent_process(
                None,
                Some("codex"),
                &[
                    "/opt/homebrew/Caskroom/codex/0.167.2/bin/codex",
                    "--no-daemon"
                ]
            ),
            Some("codex")
        );
        assert_eq!(
            identify_agent_process(None, Some("claude"), &["claude", "--resume", "abc"]),
            Some("claude")
        );
        assert_eq!(
            identify_agent_process(
                None,
                Some("grok-1.0.4-macos-aarch64"),
                &["/opt/homebrew/Caskroom/grok-build/1.0.4/grok-1.0.4-macos-aarch64"]
            ),
            Some("grok")
        );
        assert_eq!(
            identify_agent_process(None, Some("cursor-agent"), &["cursor-agent"]),
            Some("cursor")
        );
        assert_eq!(
            identify_agent_process(None, Some("kiro-cli"), &["kiro-cli"]),
            Some("kiro")
        );
    }

    #[test]
    fn identifies_node_python_and_env_shebang_wrappers() {
        assert_eq!(
            identify_agent_process(
                None,
                Some("node"),
                &[
                    "/opt/homebrew/opt/node/bin/node",
                    "/opt/homebrew/Cellar/qwen-code/0.17.1/libexec/lib/node_modules/@qwen-code/qwen-code/cli.js"
                ]
            ),
            Some("qwen")
        );
        assert_eq!(
            identify_agent_process(
                None,
                Some("python3.12"),
                &[
                    "/opt/homebrew/Cellar/kimi-cli/1.46.0/libexec/bin/python",
                    "/opt/homebrew/Cellar/kimi-cli/1.46.0/libexec/bin/kimi"
                ]
            ),
            Some("kimi")
        );
        assert_eq!(
            identify_agent_process(
                None,
                Some("env"),
                &[
                    "/usr/bin/env",
                    "node",
                    "--no-warnings",
                    "/Users/dev/.nvm/lib/node_modules/@anthropic-ai/claude-code/cli.js"
                ]
            ),
            Some("claude")
        );
        assert_eq!(
            identify_agent_process(
                None,
                Some("npm"),
                &["npx", "-y", "@google/gemini-cli@latest"]
            ),
            Some("gemini")
        );
        assert_eq!(
            identify_agent_process(None, Some("npm"), &["npm", "exec", "--", "@openai/codex"]),
            Some("codex")
        );
    }

    #[test]
    fn clears_stale_osc_status_after_returning_to_shell_and_ignores_ordinary_commands() {
        // Stale OSC 7501 status retained on PTY after child exits back to zsh must NOT show an icon.
        assert_eq!(
            identify_agent_process(Some("codex"), Some("zsh"), &["/bin/zsh", "-l"]),
            None
        );
        assert_eq!(
            identify_agent_process(Some("claude"), Some("-zsh"), &["-zsh"]),
            None
        );
        // Active OSC 7501 status while running under node/python without identifiable script path works.
        assert_eq!(
            identify_agent_process(Some("codex"), Some("node"), &["node", "/tmp/bundle.js"]),
            Some("codex")
        );
        // Unrelated foreground tools never match, even if stale OSC 7501 status exists or editor `cursor` runs.
        assert_eq!(
            identify_agent_process(Some("codex"), Some("git"), &["git", "status"]),
            None
        );
        assert_eq!(
            identify_agent_process(None, Some("cursor"), &["cursor", "."]),
            None
        );
        assert_eq!(
            identify_agent_process(None, Some("vim"), &["vim", "README.md"]),
            None
        );
        assert_eq!(
            identify_agent_process(None, Some("python3"), &["python3", "scripts/build.py"]),
            None
        );
        // Finished Herdr terminal never reports an active agent.
        assert_eq!(
            identify_herdr_agent(&json!({
                "state": "finished",
                "status": {"app": "agy", "state": "working"},
                "foreground": {"name": "agy", "argv": ["agy"]}
            })),
            None
        );
    }
}
