use anyhow::{Result, ensure};
use canban_types::{Artifact, Card, Project, Request, Snapshot, id};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
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
    let notes: Vec<_> = snapshot
        .notes
        .iter()
        .filter(|n| {
            n.project_id == project.id
                && card
                    .context_notes
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&n.id))
        })
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
    let manifest = serde_json::json!({"schema_version":1,"project":project,"card":card,"request":req,"attempt":attempt,"timeline":timeline,"notes":notes,"reports":reports,"artifacts":artifacts,"questions":questions});
    let manifest_path = dir.join("context.json");
    atomic(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    let prompt = format!(
        "# Canban Card #{}: {}\n\n{}\n\n## Agent role: @{}\n{}\n\n## Assignment\n{}\n\n## Captured Human Context\nRead the exact context manifest at {}. It contains notes, comments, earlier report references, and attachment paths (relative to {}). Read relevant earlier reports. Notes are database documents. Use `canban note read NOTE_ID --json` to read current content/revision, and `canban note write NOTE_ID --revision N --file /path/to/content.md` (or stdin) to update them. The CLI uses the injected execution binding to record your changes on the Note and Card Timelines. Markdown links use [label](note:NOTE_ID); keep IDs intact when renaming. Never edit canban.sqlite directly. Captured Notes remain historical snapshots.\n\n## Outcome reporting\nYou must explicitly report your own outcome by running the provided CLI. Do not move or archive the card.\nCANBAN_ENDPOINT={} CANBAN_REQUEST_ID={} CANBAN_ATTEMPT={} {} report --state completed --message 'Summary' --file /absolute/path/to/your-report.md\nUse --state blocked when you need human input; working for progress, failed on failure. --file is optional. CANBAN_CLI, CANBAN_ENDPOINT, CANBAN_REQUEST_ID and CANBAN_ATTEMPT are also in your environment. CLI success means the outcome was recorded; Canban applies the workflow.\n",
        card.number,
        card.title,
        card.body,
        req.profile.id,
        req.profile.role_instruction,
        req.instruction,
        execution_home
            .join(relative(home, &manifest_path)?)
            .display(),
        execution_home.display(),
        shell_quote(endpoint),
        shell_quote(&req.id),
        attempt,
        shell_quote(&cli.to_string_lossy())
    );
    let mut prompt = prompt;
    prompt.push_str("\nSave reports and question replies under $CANBAN_RUN_DIR, outside the source checkout. Keep only deliverable source/tests/docs in the Card worktree. Canban review acceptance commits all reviewed checkout changes.\n");
    prompt.push_str("\n## Conversation binding and follow-up questions\nThis invocation's Canban request/attempt binding supersedes older commands in conversation history.\n");
    if let Some(instruction) = &req.profile.session_instruction {
        prompt.push_str(instruction);
        prompt.push('\n');
    }
    for q in questions {
        prompt.push_str(&format!("\nQuestion ID: {}\n{}\nPublish the answer with \"$CANBAN_CLI\" reply --to {} --file /absolute/path/to/answer.md. A reply does not complete the task.\n",q.id,q.body,shell_quote(&q.id)));
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
