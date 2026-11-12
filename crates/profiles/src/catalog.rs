//! Built-in Herdr interactive CLI Agent catalog.
//!
//! Covers all 24 official interactive CLI agents recognized by Herdr's terminal
//! detector and resume planner (`codex`, `claude`, `agy`, `gemini`, `opencode`,
//! `cursor`, `pi`, `omp`, `copilot`, `droid`, `amp`, `kimi`, `kiro`, `qwen`,
//! `devin`, `cline`, `grok`, `hermes`, `kilo`, `qodercli`, `letta`, `mastracode`,
//! `maki`, `muse`), plus helper construction for dynamic Herdr templates.

use canban_types::{ExitAction, Profile};
use serde::{Deserialize, Serialize};

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
}

impl CatalogEntry {
    pub fn to_preset_profile(&self) -> Profile {
        Profile {
            id: self.id.clone(),
            name: Some(self.display_name.clone()),
            enabled: true,
            identity: false,
            preset: None,
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

fn bind_session_instruction(var_hint: Option<&str>) -> String {
    match var_hint {
        Some(env_var) => format!(
            "Before other work, run \"$CANBAN_CLI\" bind-session --session-id \"${env_var}\" to bind this invocation to its actual provider history. Do not reuse a thread ID quoted in earlier conversation history."
        ),
        None => "The provider session ID is allocated and bound by Canban through your launch command. Use the current invocation's Canban reporting commands.".into(),
    }
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
            session_instruction: Some(bind_session_instruction(Some("CODEX_THREAD_ID"))),
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
            session_instruction: Some(bind_session_instruction(None)),
        },
        CatalogEntry {
            id: "agy".into(),
            display_name: "Antigravity".into(),
            binary: "agy".into(),
            resumable: true,
            argv: s(&["agy", "--add-dir", "{home}", "-i", "{prompt}"]),
            resume_argv: s(&[
                "agy",
                "--add-dir",
                "{home}",
                "--conversation",
                "{session_id}",
                "-i",
                "{prompt}",
            ]),
            exit: ExitAction::Manual,
            session_instruction: None,
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
            session_instruction: Some(bind_session_instruction(None)),
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
        role_instruction: String::new(),
        argv: Some(vec![template_name.to_owned(), "{prompt}".into()]),
        resume_argv: None,
        exit: Some(ExitAction::Manual),
        session_instruction: None,
    }
}
