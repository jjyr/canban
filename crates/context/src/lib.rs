pub mod project_bundle;
pub use project_bundle::{
    CardBranchHint, LoadedProjectBundle, PROJECT_BUNDLE_FORMAT_VERSION, ProjectBundleFileMeta,
    ProjectBundleManifest, ProjectGitHints, WorktreeHint, cleanup_extracted_files,
    extract_project_bundle_files, read_project_bundle, read_project_bundle_manifest,
    validate_relative_bundle_path, write_project_bundle,
};

use anyhow::{Result, ensure};
use canban_types::{Artifact, Card, Event, ExecutionTrigger, Project, Snapshot, TerminalEntry, id};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const REPORTING_TASK_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/reporting_task.txt"
));
const REPORTING_MENTION_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/reporting_mention.txt"
));
const REPORTING_INTERACTIVE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/reporting_interactive.txt"
));
const PENDING_QUESTION_ITEM_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/pending_question_item.txt"
));
const GITHUB_SOURCE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/github_source.txt"
));
pub const SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../skills/canban/SKILL.md"
));

pub fn skill_path(home: &Path) -> PathBuf {
    home.join("skills").join("canban").join("SKILL.md")
}

/// Durability policy for atomic temp-file + rename persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FsDurability {
    /// Flush both temp file and parent directory via `sync_all()` (production default).
    #[default]
    Full,
    /// Skip `sync_all()` disk barriers while preserving atomic temp-file + rename semantics (for DST).
    NoSync,
}

pub fn materialize_skill(home: &Path) -> Result<PathBuf> {
    materialize_skill_with_durability(home, FsDurability::Full)
}

pub fn materialize_skill_with_durability(home: &Path, durability: FsDurability) -> Result<PathBuf> {
    let path = skill_path(home);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SKILL_MD) {
        atomic_with_durability(&path, SKILL_MD.as_bytes(), durability)?;
    }
    Ok(path)
}

pub fn skill_install_command(home: &Path) -> String {
    skill_install_command_with_user_home(home, std::env::var_os("HOME").as_deref().map(Path::new))
}

pub fn skill_install_command_with_user_home(home: &Path, user_home: Option<&Path>) -> String {
    let skills_dir = home.join("skills");
    let raw = skills_dir.to_string_lossy();
    let arg = if let Some(uh) = user_home
        && let Ok(rel) = skills_dir.strip_prefix(uh)
        && !rel.as_os_str().is_empty()
        && rel
            .to_str()
            .is_some_and(|s| s.bytes().all(is_shell_safe_byte))
    {
        format!("~/{}", rel.to_string_lossy())
    } else if raw.bytes().all(is_shell_safe_byte) {
        raw.into_owned()
    } else {
        shell_quote(&raw)
    };
    format!("npx skills add {arg} --skill canban -g")
}

fn is_shell_safe_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-')
}

pub fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let key = &after_open[..end];
            if let Some((_, val)) = vars.iter().find(|(k, _)| *k == key) {
                out.push_str(val);
            } else {
                out.push_str("{{");
                out.push_str(key);
                out.push_str("}}");
            }
            rest = &after_open[end + 2..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn relative(home: &Path, path: &Path) -> Result<String> {
    Ok(path.strip_prefix(home)?.to_string_lossy().into_owned())
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_with_durability(path, bytes, FsDurability::Full)
}

pub fn atomic_with_durability(path: &Path, bytes: &[u8], durability: FsDurability) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("{}.tmp", id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    f.write_all(bytes)?;
    if durability == FsDurability::Full {
        f.sync_all()?;
    }
    drop(f);
    std::fs::rename(&tmp, path)?;
    if durability == FsDurability::Full {
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    }
    Ok(())
}
pub fn card_dir(home: &Path, project: &str, card_number: u64) -> PathBuf {
    home.join("projects")
        .join(project)
        .join("cards")
        .join(card_number.to_string())
}
pub fn scratch_dir(home: &Path, project: &str, card_number: u64) -> PathBuf {
    card_dir(home, project, card_number).join("scratch")
}
pub fn run_dir(home: &Path, entry: &TerminalEntry) -> PathBuf {
    if let Some(run) = &entry.agent {
        if run.card_number > 0 {
            card_dir(home, &run.project_id, run.card_number)
                .join("agents")
                .join(&run.profile.id)
        } else {
            home.join("projects")
                .join(&run.project_id)
                .join("cards")
                .join(&run.card_id)
                .join("agents")
                .join(&run.profile.id)
        }
    } else {
        home.join("terminals").join(&entry.id)
    }
}
pub fn capture(
    home: &Path,
    project: &Project,
    card: &Card,
    entry: &TerminalEntry,
    snapshot: &Snapshot,
    card_events: &[Event],
) -> Result<String> {
    capture_with_durability(
        home,
        project,
        card,
        entry,
        snapshot,
        card_events,
        FsDurability::Full,
    )
}

pub fn capture_with_durability(
    home: &Path,
    project: &Project,
    card: &Card,
    entry: &TerminalEntry,
    snapshot: &Snapshot,
    card_events: &[Event],
    durability: FsDurability,
) -> Result<String> {
    let run = entry
        .agent
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Terminal has no Agent execution state"))?;
    let mut context_card = card.clone();
    if let Some(source) = context_card.github_issue() {
        context_card.title = format!("{}#{}", source.repository, source.number);
    }
    let card = &context_card;
    let dir = if run.card_number == card.number {
        run_dir(home, entry)
    } else {
        card_dir(home, &run.project_id, card.number)
            .join("agents")
            .join(&run.profile.id)
    };
    let timeline: Vec<_> = card_events
        .iter()
        .filter(|e| e.card_id == card.id)
        .collect();
    let reports: Vec<_> = snapshot
        .reports
        .iter()
        .filter(|r| r.card_id == card.id)
        .collect();
    let artifacts: Vec<_> = snapshot
        .artifacts
        .iter()
        .filter(|a| a.card_id == card.id)
        .collect();
    let questions: Vec<_> = snapshot
        .questions
        .iter()
        .filter(|q| {
            q.state != "replied"
                && (q.terminal_id.as_deref() == Some(entry.id.as_str())
                    || q.card_id == card.id && q.profile_id == run.profile.id)
        })
        .collect();
    let github_source = card.github_issue();
    // Notes are deferred. Keep the manifest field empty for old readers, without
    // injecting saved legacy documents or changing already captured executions.
    let manifest = serde_json::json!({"schema_version":2,"github_source":github_source,"project":project,"card":card,"terminal_id":entry.id,"agent":run,"timeline":timeline,"notes":[],"reports":reports,"artifacts":artifacts,"questions":questions});
    let manifest_path = dir.join("context.json");
    atomic_with_durability(
        &manifest_path,
        &serde_json::to_vec_pretty(&manifest)?,
        durability,
    )?;

    let mut prompt = format!("# Card #{}: {}\n", card.number, card.title);
    if !card.body.trim().is_empty() {
        prompt.push('\n');
        prompt.push_str(card.body.trim());
        prompt.push('\n');
    }
    if run.profile.role_instruction.trim().is_empty() {
        prompt.push_str(&format!("\n## Role: @{}\n", run.profile.id));
    } else {
        prompt.push_str(&format!(
            "\n## Role (@{})\n{}\n",
            run.profile.id,
            run.profile.role_instruction.trim()
        ));
    }
    prompt.push_str(&format!("\n## Task\n{}\n", run.instruction.trim()));

    let mut comments: Vec<_> = timeline
        .iter()
        .copied()
        .filter(|e| {
            matches!(e.kind.as_str(), "comment.created" | "comment.replied")
                && !e.message.trim().is_empty()
        })
        .collect();
    comments.sort_by_key(|e| e.sequence);
    if comments.len() > 20 {
        comments = comments.split_off(comments.len() - 20);
    }
    if !comments.is_empty() {
        prompt.push_str("\n## Card discussion\n");
        for c in comments {
            let actor = c
                .actor
                .strip_prefix("agent:")
                .map(|a| format!("@{a}"))
                .unwrap_or_else(|| c.actor.clone());
            prompt.push_str(&format!("- **{actor}**: {}\n", c.message.trim()));
        }
    }

    let mut prior_reports: Vec<_> = reports
        .iter()
        .copied()
        .filter(|r| r.state != "working" && (!r.message.trim().is_empty() || r.path.is_some()))
        .collect();
    prior_reports.sort_by_key(|r| r.created_at);
    if prior_reports.len() > 10 {
        prior_reports = prior_reports.split_off(prior_reports.len() - 10);
    }
    if !prior_reports.is_empty() {
        prompt.push_str("\n## Earlier reports\n");
        for r in prior_reports {
            let role = r
                .actor
                .strip_prefix("agent:")
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    r.terminal_id.as_deref().and_then(|tid| {
                        snapshot
                            .terminals
                            .iter()
                            .find(|t| t.id == tid)
                            .and_then(|t| t.agent.as_ref().map(|a| a.profile.id.as_str()))
                    })
                })
                .unwrap_or("agent");
            let summary = if r.message.trim().is_empty() {
                "see report file"
            } else {
                r.message.trim()
            };
            if let Some(rel) = &r.path {
                prompt.push_str(&format!(
                    "- @{role} ({}): {summary} — file: {}\n",
                    r.state,
                    home.join(rel).display()
                ));
            } else {
                prompt.push_str(&format!("- @{role} ({}): {summary}\n", r.state));
            }
        }
    }

    if !artifacts.is_empty() {
        prompt.push_str("\n## Attachments\n");
        for a in &artifacts {
            prompt.push_str(&format!(
                "- {} ({}): {}\n",
                a.name,
                a.media_type,
                home.join(&a.path).display()
            ));
        }
    }

    if let Some(source) = github_source {
        let number = source.number.to_string();
        let params = shell_quote(
            &serde_json::json!({"project_id": project.id, "card": card.number}).to_string(),
        );
        prompt.push('\n');
        prompt.push_str(&render_template(
            GITHUB_SOURCE_TEMPLATE,
            &[
                ("repository", &source.repository),
                ("number", &number),
                ("url", &source.url),
                ("params", &params),
            ],
        ));
    }

    let reporting_template = match &run.trigger {
        ExecutionTrigger::Comment { .. } => REPORTING_MENTION_TEMPLATE,
        ExecutionTrigger::Interactive => REPORTING_INTERACTIVE_TEMPLATE,
        ExecutionTrigger::Workflow | ExecutionTrigger::StageEntry | ExecutionTrigger::Custom(_) => {
            REPORTING_TASK_TEMPLATE
        }
    };
    prompt.push('\n');
    prompt.push_str(reporting_template);

    if let Some(instruction) = run
        .profile
        .session_instruction
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        prompt.push('\n');
        prompt.push_str(instruction);
        prompt.push('\n');
    }
    // Comment-triggered requests exist solely to answer the comment via a single `report`
    // call; only non-comment assignments render separate `canban reply` items.
    if !run.trigger.is_comment() && !questions.is_empty() {
        prompt.push_str("\n## Pending questions\n");
        for q in questions {
            let qid_quoted = shell_quote(&q.id);
            prompt.push_str(&render_template(
                PENDING_QUESTION_ITEM_TEMPLATE,
                &[
                    ("question_id", &q.id),
                    ("question_body", q.body.trim()),
                    ("question_id_quoted", &qid_quoted),
                ],
            ));
        }
    }
    atomic_with_durability(&dir.join("prompt.md"), prompt.as_bytes(), durability)?;
    relative(home, &dir.join("prompt.md"))
}
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
pub fn suffix(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| {
            !e.is_empty() && e.len() <= 12 && e.chars().all(|ch| ch.is_ascii_alphanumeric())
        })
        .map(|e| format!(".{e}"))
        .unwrap_or_default()
}
pub fn attachment(
    home: &Path,
    card: &Card,
    name: &str,
    media_type: &str,
    bytes: &[u8],
) -> Result<Artifact> {
    attachment_with_durability(home, card, name, media_type, bytes, FsDurability::Full)
}

pub fn attachment_with_durability(
    home: &Path,
    card: &Card,
    name: &str,
    media_type: &str,
    bytes: &[u8],
    durability: FsDurability,
) -> Result<Artifact> {
    ensure!(bytes.len() <= 32 * 1024 * 1024, "Attachment exceeds 32 MiB");
    let artifact_id = id();
    let path = card_dir(home, &card.project_id, card.number)
        .join("resources")
        .join(format!("{artifact_id}{}", suffix(name)));
    atomic_with_durability(&path, bytes, durability)?;
    Ok(Artifact {
        id: artifact_id,
        project_id: card.project_id.clone(),
        card_id: card.id.clone(),
        name: name.into(),
        media_type: media_type.into(),
        path: relative(home, &path)?,
        digest: digest(bytes),
        bytes: bytes.len() as u64,
    })
}

pub fn pack_at(
    home: &Path,
    card: &Card,
    snapshot: &Snapshot,
    card_events: &[Event],
    now_ms: u64,
) -> Result<String> {
    pack_at_with_durability(
        home,
        card,
        snapshot,
        card_events,
        now_ms,
        FsDurability::Full,
    )
}

pub fn pack_at_with_durability(
    home: &Path,
    card: &Card,
    snapshot: &Snapshot,
    card_events: &[Event],
    now_ms: u64,
    durability: FsDurability,
) -> Result<String> {
    let dir = home
        .join("archives")
        .join(&card.project_id)
        .join(card.number.to_string());
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.tar.zst", id()));
    let tmp = path.with_extension("tmp");
    let file = std::fs::File::create(&tmp)?;
    let mut encoder = zstd::Encoder::new(file, 3)?;
    encoder.include_checksum(true)?;
    let mut archive = tar::Builder::new(encoder);
    archive.follow_symlinks(false);
    let manifest = serde_json::to_vec_pretty(
        &serde_json::json!({"format_version":1,"codec":"zstd","project_id":card.project_id,"card_id":card.id,"created_at":now_ms}),
    )?;
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_size(manifest.len() as u64);
    manifest_header.set_mode(0o600);
    manifest_header.set_cksum();
    archive.append_data(&mut manifest_header, "manifest.json", &manifest[..])?;
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, "records.json", &bytes[..])?;
    let events_bytes = serde_json::to_vec_pretty(card_events)?;
    let mut events_header = tar::Header::new_gnu();
    events_header.set_size(events_bytes.len() as u64);
    events_header.set_mode(0o600);
    events_header.set_cksum();
    archive.append_data(&mut events_header, "events.json", &events_bytes[..])?;
    let files = card_dir(home, &card.project_id, card.number);
    if files.exists() {
        archive.append_dir_all("files", files)?;
    }
    let encoder = archive.into_inner()?;
    let file = encoder.finish()?;
    if durability == FsDurability::Full {
        file.sync_all()?;
    }
    // Read the entire compressed stream before publication to detect an incomplete frame.
    let decoder = zstd::Decoder::new(std::fs::File::open(&tmp)?)?;
    let mut verify = tar::Archive::new(decoder);
    for entry in verify.entries()? {
        let mut entry = entry?;
        std::io::copy(&mut entry, &mut std::io::sink())?;
    }
    let mut decoder = verify.into_inner();
    std::io::copy(&mut decoder, &mut std::io::sink())?;
    std::fs::rename(&tmp, &path)?;
    if durability == FsDurability::Full {
        std::fs::File::open(&dir)?.sync_all()?;
    }
    relative(home, &path)
}
pub fn read_archive(home: &Path, path: &str) -> Result<Snapshot> {
    let decoder = zstd::Decoder::new(std::fs::File::open(home.join(path))?)?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new("records.json") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(serde_json::from_slice(&bytes)?);
        }
    }
    anyhow::bail!("Archive records.json is missing")
}
pub fn restore_files(home: &Path, card: &Card, path: &str) -> Result<()> {
    restore_files_with_durability(home, card, path, FsDurability::Full)
}

pub fn restore_files_with_durability(
    home: &Path,
    card: &Card,
    path: &str,
    durability: FsDurability,
) -> Result<()> {
    let decoder = zstd::Decoder::new(std::fs::File::open(home.join(path))?)?;
    let mut archive = tar::Archive::new(decoder);
    let dest = card_dir(home, &card.project_id, card.number);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let p = entry.path()?.into_owned();
        if let Ok(rel) = p.strip_prefix("files") {
            ensure!(
                rel.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "Invalid archive path"
            );
            if entry.header().entry_type().is_file() {
                let target = dest.join(rel);
                if !target.exists() {
                    let mut bytes = Vec::new();
                    entry.read_to_end(&mut bytes)?;
                    atomic_with_durability(&target, &bytes, durability)?;
                }
            }
        }
    }
    Ok(())
}
pub fn archived_file(home: &Path, card: &Card, relative_path: &str) -> Result<Vec<u8>> {
    let archive_path = card
        .archive
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Resource file is missing"))?;
    let card_path = card_dir(home, &card.project_id, card.number);
    let full = home.join(relative_path);
    let rel = full.strip_prefix(card_path)?;
    let wanted = Path::new("files").join(rel);
    let mut archive = tar::Archive::new(zstd::Decoder::new(std::fs::File::open(
        home.join(archive_path),
    )?)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == wanted {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    anyhow::bail!("File is not present in the Card archive")
}
pub fn compact_files(home: &Path, card: &Card, path: &str) -> Result<()> {
    let mut archive = tar::Archive::new(zstd::Decoder::new(std::fs::File::open(home.join(path))?)?);
    let root = card_dir(home, &card.project_id, card.number);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let p = entry.path()?.into_owned();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if let Ok(rel) = p.strip_prefix("files") {
            ensure!(
                rel.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "Invalid archive path"
            );
            let target = root.join(rel);
            if let Ok(current) = std::fs::read(&target) {
                let mut saved = Vec::new();
                entry.read_to_end(&mut saved)?;
                if digest(&current) == digest(&saved) {
                    std::fs::remove_file(target)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_frontmatter_is_valid_and_within_agent_skills_limits() {
        let rest = SKILL_MD
            .strip_prefix("---\n")
            .expect("SKILL.md starts with YAML frontmatter");
        let (frontmatter, body) = rest
            .split_once("\n---\n")
            .expect("SKILL.md closes YAML frontmatter");
        assert!(frontmatter.lines().any(|l| l.trim() == "name: canban"));
        let desc_line = frontmatter
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .expect("SKILL.md declares description");
        let unquoted = desc_line
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .expect("description is double-quoted YAML scalar");
        assert!(!unquoted.is_empty());
        assert!(unquoted.chars().count() <= 1024);
        assert!(!unquoted.contains('"'));
        assert!(body.contains("CANBAN_CLI"));
        assert!(body.contains("CANBAN_RUN_DIR"));
    }

    #[test]
    fn skill_materializes_idempotently_and_formats_install_command() {
        let dir = std::env::temp_dir().join(format!("canban-skill-test-{}", id()));
        let path = materialize_skill(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL_MD);
        // Second call succeeds without rewriting identical bytes.
        let path2 = materialize_skill(&dir).unwrap();
        assert_eq!(path, path2);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            skill_install_command_with_user_home(
                Path::new("/Users/alice/.canban"),
                Some(Path::new("/Users/alice"))
            ),
            "npx skills add ~/.canban/skills --skill canban -g"
        );
        assert_eq!(
            skill_install_command_with_user_home(
                Path::new("/preview/.canban"),
                Some(Path::new("/Users/alice"))
            ),
            "npx skills add /preview/.canban/skills --skill canban -g"
        );
        assert_eq!(
            skill_install_command_with_user_home(
                Path::new("/Users/alice/My Home/.canban"),
                Some(Path::new("/Users/alice"))
            ),
            "npx skills add '/Users/alice/My Home/.canban/skills' --skill canban -g"
        );
    }

    #[test]
    fn context_prompt_templates_use_injected_canban_cli_without_inline_env_overrides() {
        for (name, tpl) in [
            ("reporting_task.txt", REPORTING_TASK_TEMPLATE),
            ("reporting_mention.txt", REPORTING_MENTION_TEMPLATE),
            ("reporting_interactive.txt", REPORTING_INTERACTIVE_TEMPLATE),
            ("pending_question_item.txt", PENDING_QUESTION_ITEM_TEMPLATE),
            ("github_source.txt", GITHUB_SOURCE_TEMPLATE),
        ] {
            assert!(
                tpl.contains("\"$CANBAN_CLI\""),
                "{name} should invoke CLI through \"$CANBAN_CLI\""
            );
            assert!(
                !tpl.contains("CANBAN_ENDPOINT=")
                    && !tpl.contains("CANBAN_PROJECT=")
                    && !tpl.contains("CANBAN_CARD=")
                    && !tpl.contains("{{cli}}"),
                "{name} should not inline environment variables or expand raw CLI binary paths"
            );
        }

        for (name, tpl) in [
            ("reporting_task.txt", REPORTING_TASK_TEMPLATE),
            ("reporting_mention.txt", REPORTING_MENTION_TEMPLATE),
            ("reporting_interactive.txt", REPORTING_INTERACTIVE_TEMPLATE),
        ] {
            assert!(
                tpl.contains("## Behavior") && tpl.contains("## Commands"),
                "{name} should separate ## Behavior and ## Commands"
            );
            let (behavior, _) = tpl
                .split_once("## Commands")
                .expect("template has ## Commands section");
            assert!(
                !behavior.to_ascii_lowercase().contains("adopt"),
                "{name} ## Behavior section should not leak `adopt` CLI terminology"
            );
        }
    }

    #[test]
    fn atomic_with_durability_nosync_creates_and_replaces_atomically() {
        let dir = std::env::temp_dir().join(format!("canban-nosync-test-{}", id()));
        let target = dir.join("nested/state.json");
        atomic_with_durability(&target, br#"{"v":1}"#, FsDurability::NoSync).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), br#"{"v":1}"#);

        atomic_with_durability(&target, br#"{"v":2}"#, FsDurability::NoSync).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), br#"{"v":2}"#);

        // No stray temp files remain in the target directory.
        let entries: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["state.json".to_owned()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
