//! Verified `*.canban.tar.zst` project bundle container for cross-host migration and backup.

use crate::{FsDurability, atomic_with_durability, digest};
use anyhow::{Context, Result, bail, ensure};
use canban_types::id;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub const PROJECT_BUNDLE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardBranchHint {
    pub card_number: u64,
    pub card_id: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeHint {
    pub branch: String,
    pub base_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectGitHints {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remotes: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub card_branches: Vec<CardBranchHint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_worktrees: Vec<WorktreeHint>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBundleFileMeta {
    pub path: String,
    pub bytes: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBundleManifest {
    pub format_version: u32,
    pub codec: String,
    pub sqlite_schema: u32,
    pub include_archived: bool,
    pub created_at: u64,
    pub source_server_id: String,
    pub project_id: String,
    pub project_name: String,
    pub source_directory: String,
    pub exported_cards: usize,
    pub exported_archived_cards: usize,
    pub exported_tombstones: usize,
    pub skipped_archived_cards: usize,
    pub skipped_archived_github_cards: usize,
    #[serde(default)]
    pub required_profiles: Vec<String>,
    #[serde(default)]
    pub secret_keys: Vec<String>,
    #[serde(default)]
    pub git_hints: ProjectGitHints,
    #[serde(default)]
    pub files: Vec<ProjectBundleFileMeta>,
}

#[derive(Clone, Debug)]
pub struct LoadedProjectBundle {
    pub manifest: ProjectBundleManifest,
    pub records_json: Vec<u8>,
}

/// Validates that a relative path inside the bundle contains only normal components.
pub fn validate_relative_bundle_path(rel: &str) -> Result<PathBuf> {
    ensure!(!rel.is_empty(), "Bundle path must not be empty");
    ensure!(
        !rel.starts_with('/') && !rel.starts_with('\\'),
        "Bundle path must be relative: {rel}"
    );
    let path = Path::new(rel);
    let mut normalized = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                bail!("Unsafe path component in bundle: {rel}");
            }
        }
    }
    ensure!(
        !normalized.as_os_str().is_empty(),
        "Bundle path must not be empty"
    );
    Ok(normalized)
}

fn ensure_no_symlink_ancestors(root: &Path, rel: &Path) -> Result<()> {
    let mut cur = root.to_path_buf();
    let comps: Vec<_> = rel.components().collect();
    for (idx, comp) in comps.iter().enumerate() {
        cur.push(comp.as_os_str());
        match fs::symlink_metadata(&cur) {
            Ok(meta) => {
                ensure!(
                    !meta.file_type().is_symlink(),
                    "Refusing symlink path component in destination: {}",
                    cur.display()
                );
                if idx + 1 < comps.len() {
                    ensure!(
                        meta.is_dir(),
                        "Expected directory component in destination: {}",
                        cur.display()
                    );
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn append_bytes<W: Write>(
    archive: &mut tar::Builder<W>,
    archive_path: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, archive_path, bytes)?;
    Ok(())
}

/// Writes and verifies a self-contained `*.canban.tar.zst` project bundle atomically.
pub fn write_project_bundle(
    output_path: &Path,
    overwrite: bool,
    manifest: &ProjectBundleManifest,
    records_json: &[u8],
    files: &[(String, PathBuf)],
) -> Result<u64> {
    write_project_bundle_with_durability(
        output_path,
        overwrite,
        manifest,
        records_json,
        files,
        FsDurability::Full,
    )
}

/// Writes and verifies a self-contained `*.canban.tar.zst` project bundle atomically,
/// applying the requested [`FsDurability`] policy to disk barriers.
pub fn write_project_bundle_with_durability(
    output_path: &Path,
    overwrite: bool,
    manifest: &ProjectBundleManifest,
    records_json: &[u8],
    files: &[(String, PathBuf)],
    durability: FsDurability,
) -> Result<u64> {
    ensure!(
        output_path.is_absolute(),
        "Output path must be absolute: {}",
        output_path.display()
    );
    ensure!(
        manifest.format_version == PROJECT_BUNDLE_FORMAT_VERSION,
        "Unsupported project bundle format version {}",
        manifest.format_version
    );
    ensure!(
        manifest.codec == "zstd",
        "Unsupported project bundle codec {}",
        manifest.codec
    );
    if !overwrite && output_path.exists() {
        bail!(
            "Output file already exists ({}); choose a new path or pass --overwrite",
            output_path.display()
        );
    }

    let parent = output_path
        .parent()
        .context("Output path must have a parent directory")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".canban-project-export-{}.tmp", id()));

    struct TempGuard<'a>(&'a Path, bool);
    impl Drop for TempGuard<'_> {
        fn drop(&mut self) {
            if self.1 {
                let _ = fs::remove_file(self.0);
            }
        }
    }
    let mut guard = TempGuard(&tmp, true);

    {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        let file = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let mut encoder = zstd::Encoder::new(file, 3)?;
        encoder.include_checksum(true)?;
        let mut archive = tar::Builder::new(encoder);
        archive.follow_symlinks(false);

        let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
        append_bytes(&mut archive, "manifest.json", &manifest_bytes)?;
        append_bytes(&mut archive, "records.json", records_json)?;

        for (rel, abs_src) in files {
            let clean_rel = validate_relative_bundle_path(rel)?;
            let meta = fs::symlink_metadata(abs_src)
                .with_context(|| format!("Missing managed file {}", abs_src.display()))?;
            ensure!(
                meta.is_file(),
                "Managed file must be a regular file (not a symlink): {}",
                abs_src.display()
            );
            let bytes = fs::read(abs_src)
                .with_context(|| format!("Failed reading managed file {}", abs_src.display()))?;
            let tar_path = Path::new("files").join(&clean_rel);
            let tar_path_str = tar_path.to_str().context("Non-UTF-8 bundle path")?;
            append_bytes(&mut archive, tar_path_str, &bytes)?;
        }

        let encoder = archive.into_inner()?;
        let file = encoder.finish()?;
        if durability == FsDurability::Full {
            file.sync_all()?;
        }
    }

    // Read the entire compressed stream before publication to verify frame integrity.
    {
        let decoder = zstd::Decoder::new(File::open(&tmp)?)?;
        let mut verify = tar::Archive::new(decoder);
        for entry in verify.entries()? {
            let mut entry = entry?;
            io::copy(&mut entry, &mut io::sink())?;
        }
        let mut decoder = verify.into_inner();
        io::copy(&mut decoder, &mut io::sink())?;
    }

    if overwrite {
        fs::rename(&tmp, output_path)?;
        guard.1 = false;
    } else {
        match fs::hard_link(&tmp, output_path) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                guard.1 = false;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                bail!(
                    "Output file already exists ({}); choose a new path or pass --overwrite",
                    output_path.display()
                );
            }
            Err(_) => {
                ensure!(
                    !output_path.exists(),
                    "Output file already exists ({}); choose a new path or pass --overwrite",
                    output_path.display()
                );
                fs::rename(&tmp, output_path)?;
                guard.1 = false;
            }
        }
    }
    if durability == FsDurability::Full {
        File::open(parent)?.sync_all()?;
    }
    Ok(fs::metadata(output_path)?.len())
}

/// Reads only `manifest.json` from a `*.canban.tar.zst` project bundle without
/// reading `records.json` or managed files.
pub fn read_project_bundle_manifest(archive_path: &Path) -> Result<ProjectBundleManifest> {
    let meta = fs::symlink_metadata(archive_path)
        .with_context(|| format!("Bundle not found: {}", archive_path.display()))?;
    ensure!(
        meta.is_file(),
        "Bundle path must be a regular file: {}",
        archive_path.display()
    );

    let decoder = zstd::Decoder::new(File::open(archive_path)?)?;
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        ensure!(
            !entry_type.is_symlink() && !entry_type.is_hard_link(),
            "Bundle contains unsupported link entry"
        );
        let raw_path = entry.path()?.into_owned();
        if raw_path == Path::new("manifest.json") {
            ensure!(entry_type.is_file(), "manifest.json must be a regular file");
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            let manifest: ProjectBundleManifest =
                serde_json::from_slice(&buf).context("Invalid manifest.json in project bundle")?;
            ensure!(
                manifest.format_version == PROJECT_BUNDLE_FORMAT_VERSION,
                "Unsupported project bundle format version {} (expected {})",
                manifest.format_version,
                PROJECT_BUNDLE_FORMAT_VERSION
            );
            ensure!(
                manifest.codec == "zstd",
                "Unsupported project bundle codec {}",
                manifest.codec
            );
            return Ok(manifest);
        }
    }

    bail!("Project bundle is missing manifest.json")
}

/// Reads and cryptographically verifies a `*.canban.tar.zst` project bundle without modifying disk state.
pub fn read_project_bundle(archive_path: &Path) -> Result<LoadedProjectBundle> {
    let meta = fs::symlink_metadata(archive_path)
        .with_context(|| format!("Bundle not found: {}", archive_path.display()))?;
    ensure!(
        meta.is_file(),
        "Bundle path must be a regular file: {}",
        archive_path.display()
    );

    let decoder = zstd::Decoder::new(File::open(archive_path)?)?;
    let mut archive = tar::Archive::new(decoder);

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut records_json: Option<Vec<u8>> = None;
    let mut observed_files: BTreeMap<String, (u64, String)> = BTreeMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        ensure!(
            !entry_type.is_symlink() && !entry_type.is_hard_link(),
            "Bundle contains unsupported link entry"
        );
        let raw_path = entry.path()?.into_owned();
        if raw_path == Path::new("manifest.json") {
            ensure!(entry_type.is_file(), "manifest.json must be a regular file");
            ensure!(
                manifest_bytes.is_none(),
                "Duplicate manifest.json in bundle"
            );
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            manifest_bytes = Some(buf);
        } else if raw_path == Path::new("records.json") {
            ensure!(entry_type.is_file(), "records.json must be a regular file");
            ensure!(records_json.is_none(), "Duplicate records.json in bundle");
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            records_json = Some(buf);
        } else if let Ok(rel) = raw_path.strip_prefix("files") {
            if entry_type.is_dir() {
                continue;
            }
            ensure!(
                entry_type.is_file(),
                "Bundle entry must be a regular file: {}",
                raw_path.display()
            );
            let rel_str = rel
                .to_str()
                .context("Bundle contains non-UTF-8 file path")?
                .replace('\\', "/");
            let _ = validate_relative_bundle_path(&rel_str)?;
            ensure!(
                !observed_files.contains_key(&rel_str),
                "Duplicate file entry in bundle: {rel_str}"
            );
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 8192];
            let mut total = 0u64;
            loop {
                let n = entry.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                hasher.update(&buf[..n]);
            }
            observed_files.insert(rel_str, (total, format!("{:x}", hasher.finalize())));
        } else {
            bail!("Unexpected entry in project bundle: {}", raw_path.display());
        }
    }

    let mut decoder = archive.into_inner();
    io::copy(&mut decoder, &mut io::sink())?;

    let manifest: ProjectBundleManifest =
        serde_json::from_slice(&manifest_bytes.context("Project bundle is missing manifest.json")?)
            .context("Invalid manifest.json in project bundle")?;
    ensure!(
        manifest.format_version == PROJECT_BUNDLE_FORMAT_VERSION,
        "Unsupported project bundle format version {} (expected {})",
        manifest.format_version,
        PROJECT_BUNDLE_FORMAT_VERSION
    );
    ensure!(
        manifest.codec == "zstd",
        "Unsupported project bundle codec {}",
        manifest.codec
    );

    let records_json = records_json.context("Project bundle is missing records.json")?;

    ensure!(
        observed_files.len() == manifest.files.len(),
        "Bundle file count mismatch (manifest lists {}, archive has {})",
        manifest.files.len(),
        observed_files.len()
    );
    for expected in &manifest.files {
        let rel_str = expected.path.replace('\\', "/");
        let _ = validate_relative_bundle_path(&rel_str)?;
        let (actual_bytes, actual_digest) = observed_files
            .get(&rel_str)
            .with_context(|| format!("Bundle is missing file listed in manifest: {rel_str}"))?;
        ensure!(
            *actual_bytes == expected.bytes && *actual_digest == expected.digest,
            "Bundle file checksum mismatch for {rel_str}"
        );
    }

    Ok(LoadedProjectBundle {
        manifest,
        records_json,
    })
}

/// Extracts verified `files/<rel>` entries matching `allowed_prefixes` into `dest_home`.
/// On error, removes any files created during this call before returning.
pub fn extract_project_bundle_files(
    archive_path: &Path,
    dest_home: &Path,
    allowed_prefixes: &[String],
) -> Result<Vec<PathBuf>> {
    extract_project_bundle_files_with_durability(
        archive_path,
        dest_home,
        allowed_prefixes,
        FsDurability::Full,
    )
}

/// Extracts verified `files/<rel>` entries matching `allowed_prefixes` into `dest_home`,
/// applying the requested [`FsDurability`] policy to extracted file writes.
/// On error, removes any files created during this call before returning.
pub fn extract_project_bundle_files_with_durability(
    archive_path: &Path,
    dest_home: &Path,
    allowed_prefixes: &[String],
    durability: FsDurability,
) -> Result<Vec<PathBuf>> {
    let decoder = zstd::Decoder::new(File::open(archive_path)?)?;
    let mut archive = tar::Archive::new(decoder);
    let mut created: Vec<PathBuf> = Vec::new();

    let res: Result<()> = (|| {
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_type = entry.header().entry_type();
            ensure!(
                !entry_type.is_symlink() && !entry_type.is_hard_link(),
                "Bundle contains unsupported link entry"
            );
            let raw_path = entry.path()?.into_owned();
            if let Ok(rel) = raw_path.strip_prefix("files") {
                if entry_type.is_dir() {
                    continue;
                }
                ensure!(
                    entry_type.is_file(),
                    "Bundle entry must be a regular file: {}",
                    raw_path.display()
                );
                let rel_str = rel
                    .to_str()
                    .context("Bundle contains non-UTF-8 file path")?
                    .replace('\\', "/");
                let clean_rel = validate_relative_bundle_path(&rel_str)?;
                ensure!(
                    allowed_prefixes
                        .iter()
                        .any(|prefix| rel_str.starts_with(prefix)),
                    "Unexpected file outside project scope: {rel_str}"
                );
                ensure_no_symlink_ancestors(dest_home, &clean_rel)?;
                let target = dest_home.join(&clean_rel);
                ensure!(
                    matches!(
                        fs::symlink_metadata(&target),
                        Err(ref e) if e.kind() == io::ErrorKind::NotFound
                    ),
                    "Destination file already exists: {}",
                    target.display()
                );
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                atomic_with_durability(&target, &bytes, durability)?;
                created.push(target);
            } else {
                io::copy(&mut entry, &mut io::sink())?;
            }
        }
        let mut decoder = archive.into_inner();
        io::copy(&mut decoder, &mut io::sink())?;
        Ok(())
    })();

    if let Err(err) = res {
        cleanup_extracted_files(&created);
        return Err(err);
    }
    let _ = digest(&[]); // keep digest helper linkage explicit if unused elsewhere
    Ok(created)
}

/// Removes files created during an aborted extraction.
pub fn cleanup_extracted_files(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest(
        project_id: &str,
        files: Vec<ProjectBundleFileMeta>,
    ) -> ProjectBundleManifest {
        ProjectBundleManifest {
            format_version: PROJECT_BUNDLE_FORMAT_VERSION,
            codec: "zstd".into(),
            sqlite_schema: 24,
            include_archived: false,
            created_at: 1_700_000_000_000,
            source_server_id: "srv-1".into(),
            project_id: project_id.into(),
            project_name: "Sample".into(),
            source_directory: "/tmp/sample".into(),
            exported_cards: 1,
            exported_archived_cards: 0,
            exported_tombstones: 0,
            skipped_archived_cards: 0,
            skipped_archived_github_cards: 0,
            required_profiles: vec!["developer".into()],
            secret_keys: vec!["API_TOKEN".into()],
            git_hints: ProjectGitHints::default(),
            files,
        }
    }

    #[test]
    fn rejects_corrupted_truncated_and_traversal_bundles() {
        let dir = std::env::temp_dir().join(format!("canban-bundle-bad-{}", id()));
        fs::create_dir_all(&dir).unwrap();

        // 1. Truncated / corrupted archive
        let bad_zst = dir.join("bad.canban.tar.zst");
        fs::write(&bad_zst, b"not a valid zstd stream").unwrap();
        assert!(read_project_bundle(&bad_zst).is_err());
        assert!(read_project_bundle_manifest(&bad_zst).is_err());
        assert!(read_project_bundle_manifest(&dir.join("missing.canban.tar.zst")).is_err());
        assert!(read_project_bundle_manifest(&dir).is_err());

        // 2. Crafted tar with `../` traversal inside files/
        let traversal_tar = dir.join("traversal.canban.tar.zst");
        {
            let file = File::create(&traversal_tar).unwrap();
            let mut enc = zstd::Encoder::new(file, 3).unwrap();
            enc.include_checksum(true).unwrap();
            let manifest_bytes = serde_json::to_vec(&sample_manifest("p1", Vec::new())).unwrap();
            let mut archive = tar::Builder::new(enc);
            append_bytes(&mut archive, "manifest.json", &manifest_bytes).unwrap();
            append_bytes(&mut archive, "records.json", b"{}").unwrap();
            // Build raw header manually to inject `..` path bypassing tar::Builder validation.
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o600);
            let name_bytes = b"files/../escape.txt";
            h.as_gnu_mut().unwrap().name[..name_bytes.len()].copy_from_slice(name_bytes);
            h.set_cksum();
            archive.append(&h, &b"bad"[..]).unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        assert!(read_project_bundle(&traversal_tar).is_err());

        // 3. Crafted tar with symlink entry
        let symlink_tar = dir.join("symlink.canban.tar.zst");
        {
            let file = File::create(&symlink_tar).unwrap();
            let mut enc = zstd::Encoder::new(file, 3).unwrap();
            enc.include_checksum(true).unwrap();
            let manifest_bytes = serde_json::to_vec(&sample_manifest("p1", Vec::new())).unwrap();
            let mut archive = tar::Builder::new(enc);
            append_bytes(&mut archive, "manifest.json", &manifest_bytes).unwrap();
            append_bytes(&mut archive, "records.json", b"{}").unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            archive
                .append_link(&mut h, "files/projects/p1/evil", "/etc/passwd")
                .unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        assert!(read_project_bundle(&symlink_tar).is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
