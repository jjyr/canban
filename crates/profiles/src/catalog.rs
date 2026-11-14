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
