use anyhow::{Result, ensure};
use canban_types::{Artifact, Card, Project, Request, RequestTrigger, Snapshot, id};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const REPORTING_TASK_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/reporting_task.txt"
));
const REPORTING_COMMENT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/context/reporting_comment.txt"
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

pub fn materialize_skill(home: &Path) -> Result<PathBuf> {
    let path = skill_path(home);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SKILL_MD) {
        atomic(&path, SKILL_MD.as_bytes())?;
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
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("{}.tmp", id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub fn card_dir(home: &Path, project: &str, card: &str) -> PathBuf {
    home.join("projects").join(project).join("cards").join(card)
}
pub fn run_dir(home: &Path, req: &Request, attempt: u32) -> PathBuf {
    card_dir(home, &req.project_id, &req.card_id)
        .join("runs")
        .join(&req.id)
        .join(attempt.to_string())
}
#[allow(
    clippy::too_many_arguments,
    reason = "Explicit immutable input and reporting boundaries for a captured execution"
)]
pub fn capture(
    home: &Path,
    project: &Project,
    card: &Card,
    req: &Request,
    attempt: u32,
    snapshot: &Snapshot,
    endpoint: &str,
    cli: &Path,
) -> Result<String> {
    capture_for(
        home, project, card, req, attempt, snapshot, endpoint, cli, home,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn capture_for(
    home: &Path,
    project: &Project,
    card: &Card,
    req: &Request,
    attempt: u32,
    snapshot: &Snapshot,
    endpoint: &str,
    cli: &Path,
    execution_home: &Path,
) -> Result<String> {
    let mut context_card = card.clone();
    if let Some(source) = context_card.github_issue() {
        context_card.title = format!("{}#{}", source.repository, source.number);
    }
    let card = &context_card;
    let dir = run_dir(home, req, attempt);
    let timeline: Vec<_> = snapshot
        .events
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
        .filter(|q| q.request_id == req.id && q.state != "replied")
        .collect();
    let github_source = card.github_issue();
    // Notes are deferred. Keep the manifest field empty for old readers, without
    // injecting saved legacy documents or changing already captured executions.
    let manifest = serde_json::json!({"schema_version":1,"github_source":github_source,"project":project,"card":card,"request":req,"attempt":attempt,"timeline":timeline,"notes":[],"reports":reports,"artifacts":artifacts,"questions":questions});
    let manifest_path = dir.join("context.json");
    atomic(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;

    let mut prompt = format!("# Card #{}: {}\n", card.number, card.title);
    if !card.body.trim().is_empty() {
        prompt.push('\n');
        prompt.push_str(card.body.trim());
        prompt.push('\n');
    }
    if req.profile.role_instruction.trim().is_empty() {
        prompt.push_str(&format!("\n## Role: @{}\n", req.profile.id));
    } else {
        prompt.push_str(&format!(
            "\n## Role (@{})\n{}\n",
            req.profile.id,
            req.profile.role_instruction.trim()
        ));
    }
    prompt.push_str(&format!("\n## Task\n{}\n", req.instruction.trim()));

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
            let role = snapshot
                .requests
                .iter()
                .find(|rq| rq.id == r.request_id)
                .map(|rq| rq.profile.id.as_str())
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
                    execution_home.join(rel).display()
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
                execution_home.join(&a.path).display()
            ));
        }
    }

    let cli_quoted = shell_quote(&cli.to_string_lossy());
    if let Some(source) = github_source {
        let number = source.number.to_string();
        let params = shell_quote(&serde_json::json!({"card_id": card.id}).to_string());
        prompt.push('\n');
        prompt.push_str(&render_template(
            GITHUB_SOURCE_TEMPLATE,
            &[
                ("repository", &source.repository),
                ("number", &number),
                ("url", &source.url),
                ("cli", &cli_quoted),
                ("params", &params),
            ],
        ));
    }

    let reporting_template = match &req.trigger {
        RequestTrigger::Comment { .. } => REPORTING_COMMENT_TEMPLATE,
        RequestTrigger::Interactive => REPORTING_INTERACTIVE_TEMPLATE,
        RequestTrigger::Workflow | RequestTrigger::StageEntry | RequestTrigger::Custom(_) => {
            REPORTING_TASK_TEMPLATE
        }
    };
    let endpoint_quoted = shell_quote(endpoint);
    let req_quoted = shell_quote(&req.id);
    let attempt_str = attempt.to_string();
    prompt.push('\n');
    prompt.push_str(&render_template(
        reporting_template,
        &[
            ("endpoint", &endpoint_quoted),
            ("request_id", &req_quoted),
            ("attempt", &attempt_str),
            ("cli", &cli_quoted),
        ],
    ));

    if let Some(instruction) = req
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
    if !req.trigger.is_comment() && !questions.is_empty() {
        prompt.push_str("\n## Pending questions\n");
        for q in questions {
            let qid_quoted = shell_quote(&q.id);
            prompt.push_str(&render_template(
                PENDING_QUESTION_ITEM_TEMPLATE,
                &[
                    ("question_id", &q.id),
                    ("question_body", q.body.trim()),
                    ("cli", &cli_quoted),
                    ("question_id_quoted", &qid_quoted),
                ],
            ));
        }
    }
    atomic(&dir.join("prompt.md"), prompt.as_bytes())?;
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
    ensure!(bytes.len() <= 32 * 1024 * 1024, "Attachment exceeds 32 MiB");
    let artifact_id = id();
    let path = card_dir(home, &card.project_id, &card.id)
        .join("resources")
        .join(format!("{artifact_id}{}", suffix(name)));
    atomic(&path, bytes)?;
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
pub fn pack(home: &Path, card: &Card, snapshot: &Snapshot) -> Result<String> {
    let dir = home.join("archives").join(&card.project_id).join(&card.id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.tar.zst", id()));
    let tmp = path.with_extension("tmp");
    let file = std::fs::File::create(&tmp)?;
    let mut encoder = zstd::Encoder::new(file, 3)?;
    encoder.include_checksum(true)?;
    let mut archive = tar::Builder::new(encoder);
    archive.follow_symlinks(false);
    let manifest = serde_json::to_vec_pretty(
        &serde_json::json!({"format_version":1,"codec":"zstd","project_id":card.project_id,"card_id":card.id,"created_at":canban_types::now()}),
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
    let files = card_dir(home, &card.project_id, &card.id);
    if files.exists() {
        archive.append_dir_all("files", files)?;
    }
    let encoder = archive.into_inner()?;
    let file = encoder.finish()?;
    file.sync_all()?;
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
    std::fs::File::open(&dir)?.sync_all()?;
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
    let decoder = zstd::Decoder::new(std::fs::File::open(home.join(path))?)?;
    let mut archive = tar::Archive::new(decoder);
    let dest = card_dir(home, &card.project_id, &card.id);
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
                    atomic(&target, &bytes)?;
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
    let card_path = card_dir(home, &card.project_id, &card.id);
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
    let root = card_dir(home, &card.project_id, &card.id);
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
}
