use super::*;
use anyhow::ensure;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Component, PathBuf},
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u64);
impl Version {
    pub fn parse(value: &str) -> Result<Self> {
        let (date, n) = value
            .split_once('-')
            .context("Use YYYYMMDD-N release versions")?;
        ensure!(
            date.len() == 8
                && date.bytes().all(|b| b.is_ascii_digit())
                && !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && !n.starts_with('0'),
            "Invalid release version"
        );
        let date = date.parse::<u32>()?;
        let year = date / 10000;
        let month = (date / 100) % 100;
        let day = date % 100;
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        };
        ensure!(
            year >= 2000 && day > 0 && day <= days,
            "Invalid release date"
        );
        let n = n.parse()?;
        ensure!(n > 0, "Invalid release sequence");
        Ok(Self(date, n))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub commit: String,
    pub platform: String,
    pub api: u32,
    pub schema: u32,
    #[serde(default)]
    pub rollback_floor: u32,
    pub asset: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Available {
    pub manifest: Manifest,
    pub url: String,
    pub notes: String,
}
fn api() -> Result<String> {
    let value =
        std::env::var("CANBAN_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".into());
    ensure!(
        value == "https://api.github.com" || loopback(&value),
        "Update API override must be loopback HTTP"
    );
    Ok(value.trim_end_matches('/').into())
}
fn loopback(url: &str) -> bool {
    url.strip_prefix("http://")
        .is_some_and(|s| s.starts_with("127.0.0.1:") || s.starts_with("localhost:"))
}
fn validate_url(url: &str) -> Result<()> {
    ensure!(
        url.starts_with("https://") || loopback(url),
        "Update assets require HTTPS"
    );
    Ok(())
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .max_redirects(5)
        .build()
        .into()
}
fn bytes(url: &str, limit: u64, client: Option<&canban_github::Client>) -> Result<Vec<u8>> {
    validate_url(url)?;
    let mut r = if let Some(c) = client.filter(|_| url.starts_with("https://api.github.com/repos/"))
    {
        c.release_response(url)?
    } else {
        agent()
            .get(url)
            .header("User-Agent", "Canban updater")
            .call()?
    };
    let mut b = Vec::new();
    r.body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut b)?;
    ensure!(b.len() as u64 <= limit, "Update metadata exceeds limit");
    Ok(b)
}
pub fn discover(
    current: &Build,
    client: Option<&canban_github::Client>,
) -> Result<Option<Available>> {
    discover_version(current, None, client)
}
pub fn discover_version(
    current: &Build,
    target_version: Option<&str>,
    client: Option<&canban_github::Client>,
) -> Result<Option<Available>> {
    // Explicit/daily release checks must revalidate GitHub's disposable cache.
    let fresh = client.cloned().map(canban_github::Client::refreshing);
    let client = fresh.as_ref();
    let api = api()?;
    let repo = std::env::var("CANBAN_UPDATE_REPOSITORY").unwrap_or_else(|_| "jjyr/canban".into());
    ensure!(
        repo.split('/').count() == 2
            && repo.split('/').all(|p| !p.is_empty()
                && p != ".."
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "Invalid release repository"
    );
    let mut candidates = Vec::new();
    for page in 1..=5 {
        let route = format!("/repos/{repo}/releases?per_page=100&page={page}");
        let list: Vec<Value> = if let Some(c) = client.filter(|_| api == "https://api.github.com") {
            c.get(&route)?
        } else {
            serde_json::from_slice(&bytes(&format!("{api}{route}"), 4 * 1024 * 1024, None)?)?
        };
        let count = list.len();
        for r in list {
            if r["draft"] == true {
                continue;
            }
            let Some(tag) = r["tag_name"].as_str() else {
                continue;
            };
            let Ok(version) = Version::parse(tag) else {
                continue;
            };
            if let Some(target) = target_version.map(str::trim).filter(|s| !s.is_empty()) {
                if tag != target {
                    continue;
                }
            } else if Version::parse(&current.version)
                .ok()
                .is_some_and(|v| version <= v)
            {
                continue;
            }
            candidates.push((version, r));
        }
        if count < 100 {
            break;
        }
    }
    candidates.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, release) in candidates {
        let asset = format!("Canban-{}.tar.zst", current.platform);
        let Some(assets) = release["assets"].as_array() else {
            continue;
        };
        let Some(binary) = assets.iter().find(|a| a["name"] == asset) else {
            continue;
        };
        let Some(meta) = assets
            .iter()
            .find(|a| a["name"] == format!("manifest-{}.json", current.platform))
        else {
            continue;
        };
        let manifest: Manifest = serde_json::from_slice(&bytes(
            meta[if client.is_some() && api == "https://api.github.com" {
                "url"
            } else {
                "browser_download_url"
            }]
            .as_str()
            .context("Manifest URL missing")?,
            128 * 1024,
            client,
        )?)?;
        ensure!(
            manifest.version == release["tag_name"]
                && manifest.platform == current.platform
                && manifest.asset == asset,
            "Release manifest identity mismatch"
        );
        ensure!(
            manifest.commit.len() == 40 && manifest.commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid release commit"
        );
        ensure!(
            manifest.schema >= current.schema
                && manifest.rollback_floor <= current.schema
                && manifest.rollback_floor >= canban_storage::migrations::BASELINE,
            "Release does not support this database upgrade/rollback"
        );
        ensure!(
            manifest.size > 0
                && manifest.size <= 512 * 1024 * 1024
                && binary["size"].as_u64() == Some(manifest.size),
            "Invalid update asset size"
        );
        ensure!(
            manifest.sha256.len() == 64 && manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid update checksum"
        );
        let url = binary[if client.is_some() && api == "https://api.github.com" {
            "url"
        } else {
            "browser_download_url"
        }]
        .as_str()
        .context("Asset URL missing")?
        .to_owned();
        validate_url(&url)?;
        return Ok(Some(Available {
            manifest,
            url,
            notes: release["body"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(16000)
                .collect(),
        }));
    }
    Ok(None)
}
pub fn check(home: &Path, client: Option<canban_github::Client>) -> Result<()> {
    check_inner(home, client, false)
}
pub fn check_automatically(home: &Path, client: Option<canban_github::Client>) -> Result<()> {
    check_inner(home, client, true)
}
fn check_inner(home: &Path, client: Option<canban_github::Client>, automatic: bool) -> Result<()> {
    let _lock = lock(home, "operation.lock")?;
    ensure!(!handoff::active(home), "Upgrade is in progress");
    let mut state = load_state(home);
    if automatic && state.automatic_delay(now()) > 0 {
        if state.phase == "checking" {
            state.phase = "error".into();
            state.error = Some("The previous update check was interrupted. Check again manually or wait for the next daily check.".into());
            save_state(home, &state)?;
        }
        return Ok(());
    }
    state.last_check_started_at = now();
    state.phase = "checking".into();
    state.error = None;
    save_state(home, &state)?;
    let result = discover(&Build::current(), client.as_ref());
    state.checked_at = now();
    match result {
        Ok(available) => {
            state.available = available;
            state.phase = if state.available.is_some() {
                "available"
            } else {
                "current"
            }
            .into();
        }
        Err(e) => {
            state.phase = "error".into();
            state.error = Some(format!("{e:#}"));
        }
    }
    save_state(home, &state)
}
/// Stream and verify before extraction; never overwrite the installed bundle.
pub fn stage(
    home: &Path,
    release: &Available,
    client: Option<&canban_github::Client>,
) -> Result<PathBuf> {
    validate_url(&release.url)?;
    let job = home.join("updates/staging").join(canban_types::id());
    std::fs::create_dir_all(&job)?;
    let archive = job.join("download.tar.zst");
    let mut output = File::create(&archive)?;
    let mut response = if let Some(c) =
        client.filter(|_| release.url.starts_with("https://api.github.com/repos/"))
    {
        c.release_response(&release.url)?
    } else {
        agent()
            .get(&release.url)
            .header("User-Agent", "Canban updater")
            .call()?
    };
    let mut reader = response.body_mut().as_reader();
    let mut total = 0u64;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut state = load_state(home);
    state.phase = "downloading".into();
    state.error = None;
    state.progress = 0;
    save_state(home, &state)?;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(
            total <= release.manifest.size,
            "Downloaded update exceeds declared size"
        );
        digest.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
        let progress = (total * 100 / release.manifest.size) as u8;
        if progress != state.progress {
            state.progress = progress;
            save_state(home, &state)?;
        }
    }
    output.sync_all()?;
    ensure!(
        total == release.manifest.size
            && format!("{:x}", digest.finalize()) == release.manifest.sha256.to_lowercase(),
        "Update size/checksum mismatch"
    );
    let bundle = extract(&archive, &job)?;
    validate_candidate(&bundle, &release.manifest)?;
    Ok(bundle)
}
pub(crate) fn verify_bundle_tree(path: &Path) -> Result<()> {
    let kind = std::fs::symlink_metadata(path)
        .with_context(|| format!("Could not inspect candidate {}", path.display()))?
        .file_type();
    ensure!(
        kind.is_file() || kind.is_dir(),
        "Candidate contains a link or special file: {}",
        path.display()
    );
    if kind.is_dir() {
        for entry in std::fs::read_dir(path)? {
            verify_bundle_tree(&entry?.path())?;
        }
    }
    Ok(())
}
/// Pack a desktop bundle (`Canban.app` on macOS or `canban/` directory on Linux)
/// into a zstd-compressed tarball after verifying it contains no links or special files.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn pack(source: &Path, output: &Path) -> Result<()> {
    ensure!(
        std::fs::symlink_metadata(source)
            .map(|m| m.is_dir())
            .unwrap_or(false),
        "Desktop bundle directory missing: {}",
        source.display()
    );
    verify_bundle_tree(source)?;
    let root_name = if crate::is_app_bundle(source) {
        "Canban.app"
    } else {
        "canban"
    };
    let encoder = zstd::Encoder::new(File::create(output)?, 9)?;
    let mut tar = tar::Builder::new(encoder);
    tar.follow_symlinks(false);
    tar.append_dir_all(root_name, source)?;
    let encoder = tar.into_inner()?;
    encoder.finish()?.sync_all()?;
    Ok(())
}
pub(crate) fn validate_candidate(bundle: &Path, manifest: &Manifest) -> Result<()> {
    if manifest.platform.starts_with("macos-") {
        ensure!(
            crate::is_app_bundle(bundle),
            "macOS update archive must contain Canban.app"
        );
    } else if manifest.platform.starts_with("linux-") {
        ensure!(
            !crate::is_app_bundle(bundle),
            "Linux update archive must contain canban/ directory bundle"
        );
    }
    if crate::is_app_bundle(bundle) {
        for binary in ["canban", "canban-ui", "canban-updater"] {
            ensure!(
                bundle.join("Contents/MacOS").join(binary).is_file(),
                "Incomplete App bundle: {binary}"
            );
        }
        crate::quarantine::prepare(bundle)?;
    } else {
        verify_bundle_tree(bundle)?;
        ensure!(
            !bundle.join("share/dev-instance.json").exists(),
            "Candidate Linux bundle cannot include a development instance binding"
        );
        let meta_path = bundle.join("bundle.json");
        ensure!(
            meta_path.is_file(),
            "Incomplete Linux bundle: bundle.json missing"
        );
        let meta: Value = serde_json::from_slice(&std::fs::read(&meta_path)?)
            .context("Invalid bundle.json in candidate Linux bundle")?;
        ensure!(
            meta["app_id"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()),
            "Linux bundle.json is missing app_id"
        );
        for binary in ["canban", "canban-ui", "canban-updater"] {
            let path = bundle.join("bin").join(binary);
            ensure!(path.is_file(), "Incomplete App bundle: {binary}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    let output = std::process::Command::new(crate::bundle_cli(bundle))
        .arg("build-info")
        .output()?;
    ensure!(
        output.status.success(),
        "Candidate build identity command failed"
    );
    let build: Build = serde_json::from_slice(&output.stdout)?;
    ensure!(
        build.version == manifest.version
            && build.commit == manifest.commit
            && build.platform == manifest.platform
            && build.schema == manifest.schema
            && build.api == manifest.api,
        "Candidate binary does not match manifest"
    );
    Ok(())
}
fn extract(archive: &Path, job: &Path) -> Result<PathBuf> {
    let decoder = zstd::Decoder::new(File::open(archive)?)?;
    let mut tar = tar::Archive::new(decoder);
    let mut expanded = 0u64;
    let mut archive_root: Option<std::ffi::OsString> = None;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.components().all(|c| matches!(c, Component::Normal(_))),
            "Unsafe update archive path"
        );
        let Some(Component::Normal(first)) = path.components().next() else {
            anyhow::bail!("Unsafe update archive path");
        };
        ensure!(
            first == "Canban.app" || first == "canban",
            "Unsafe update archive path"
        );
        if let Some(expected_root) = &archive_root {
            ensure!(
                expected_root == first,
                "Update archive mixes multiple top-level bundle directories"
            );
        } else {
            archive_root = Some(first.to_os_string());
        }
        ensure!(
            entry.header().entry_type().is_file() || entry.header().entry_type().is_dir(),
            "Update archive contains a link or special file"
        );
        expanded = expanded
            .checked_add(entry.size())
            .context("Archive size overflow")?;
        ensure!(
            expanded <= 2 * 1024 * 1024 * 1024,
            "Expanded App exceeds limit"
        );
        ensure!(entry.unpack_in(job)?, "Archive entry escaped staging");
    }
    let root = archive_root.context("Empty update archive")?;
    Ok(job.join(root))
}

/// Discover the newest published headless CLI release archive (`canban-cli-<platform>.tar.gz` +
/// `manifest-cli-<platform>.json`) newer than `current` (or matching `target_version` when set).
pub fn discover_cli(
    current: &Build,
    target_version: Option<&str>,
    client: Option<&canban_github::Client>,
) -> Result<Option<Available>> {
    let fresh = client.cloned().map(canban_github::Client::refreshing);
    let client = fresh.as_ref();
    let api = api()?;
    let repo = std::env::var("CANBAN_UPDATE_REPOSITORY").unwrap_or_else(|_| "jjyr/canban".into());
    ensure!(
        repo.split('/').count() == 2
            && repo.split('/').all(|p| !p.is_empty()
                && p != ".."
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "Invalid release repository"
    );
    let mut candidates = Vec::new();
    for page in 1..=5 {
        let route = format!("/repos/{repo}/releases?per_page=100&page={page}");
        let list: Vec<Value> = if let Some(c) = client.filter(|_| api == "https://api.github.com") {
            c.get(&route)?
        } else {
            serde_json::from_slice(&bytes(&format!("{api}{route}"), 4 * 1024 * 1024, None)?)?
        };
        let count = list.len();
        for r in list {
            if r["draft"] == true {
                continue;
            }
            let Some(tag) = r["tag_name"].as_str() else {
                continue;
            };
            let Ok(version) = Version::parse(tag) else {
                continue;
            };
            if let Some(target) = target_version.map(str::trim).filter(|s| !s.is_empty()) {
                if tag != target {
                    continue;
                }
            } else if Version::parse(&current.version)
                .ok()
                .is_some_and(|v| version <= v)
            {
                continue;
            }
            candidates.push((version, r));
        }
        if count < 100 {
            break;
        }
    }
    candidates.sort_by_key(|a| std::cmp::Reverse(a.0));
    let asset = format!("canban-cli-{}.tar.gz", current.platform);
    let manifest_name = format!("manifest-cli-{}.json", current.platform);
    for (_, release) in candidates {
        let Some(assets) = release["assets"].as_array() else {
            continue;
        };
        let Some(binary) = assets.iter().find(|a| a["name"] == asset) else {
            continue;
        };
        let Some(meta) = assets.iter().find(|a| a["name"] == manifest_name) else {
            continue;
        };
        let manifest: Manifest = serde_json::from_slice(&bytes(
            meta[if client.is_some() && api == "https://api.github.com" {
                "url"
            } else {
                "browser_download_url"
            }]
            .as_str()
            .context("CLI manifest URL missing")?,
            128 * 1024,
            client,
        )?)?;
        ensure!(
            manifest.version == release["tag_name"]
                && manifest.platform == current.platform
                && manifest.asset == asset,
            "CLI release manifest identity mismatch"
        );
        ensure!(
            manifest.commit.len() == 40 && manifest.commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid release commit"
        );
        ensure!(
            manifest.schema >= current.schema,
            "Release schema {} is older than current database schema {}",
            manifest.schema,
            current.schema
        );
        ensure!(
            manifest.size > 0
                && manifest.size <= 256 * 1024 * 1024
                && binary["size"].as_u64() == Some(manifest.size),
            "Invalid CLI update asset size"
        );
        ensure!(
            manifest.sha256.len() == 64 && manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid CLI update checksum"
        );
        let url = binary[if client.is_some() && api == "https://api.github.com" {
            "url"
        } else {
            "browser_download_url"
        }]
        .as_str()
        .context("CLI asset URL missing")?
        .to_owned();
        validate_url(&url)?;
        return Ok(Some(Available {
            manifest,
            url,
            notes: release["body"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(16000)
                .collect(),
        }));
    }
    Ok(None)
}

/// Download, SHA256-verify, and extract a headless CLI archive (`canban-<platform>.tar.gz`),
/// returning the staged executable path after verifying `canban build-info`.
pub fn stage_cli(
    home: &Path,
    release: &Available,
    client: Option<&canban_github::Client>,
) -> Result<PathBuf> {
    validate_url(&release.url)?;
    let job = home.join("updates/staging").join(canban_types::id());
    std::fs::create_dir_all(&job)?;
    let archive = job.join("canban.tar.gz");
    let mut output = File::create(&archive)?;
    let mut response = if let Some(c) =
        client.filter(|_| release.url.starts_with("https://api.github.com/repos/"))
    {
        c.release_response(&release.url)?
    } else {
        agent()
            .get(&release.url)
            .header("User-Agent", "Canban updater")
            .call()?
    };
    let mut reader = response.body_mut().as_reader();
    let mut total = 0u64;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(
            total <= release.manifest.size,
            "Downloaded CLI update exceeds declared size"
        );
        digest.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
    }
    output.sync_all()?;
    ensure!(
        total == release.manifest.size
            && format!("{:x}", digest.finalize()) == release.manifest.sha256.to_lowercase(),
        "CLI update size/checksum mismatch"
    );
    let decoder = flate2::read::GzDecoder::new(File::open(&archive)?);
    let mut tar = tar::Archive::new(decoder);
    let mut expanded = 0u64;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.components().count() == 1
                && path
                    .components()
                    .next()
                    .is_some_and(|c| c.as_os_str() == "canban"),
            "Unsafe CLI update archive path"
        );
        ensure!(
            entry.header().entry_type().is_file(),
            "CLI update archive contains a non-file entry"
        );
        expanded = expanded
            .checked_add(entry.size())
            .context("CLI archive size overflow")?;
        ensure!(
            expanded <= 512 * 1024 * 1024,
            "Expanded CLI binary exceeds limit"
        );
        ensure!(entry.unpack_in(&job)?, "CLI archive entry escaped staging");
    }
    let staged_bin = job.join("canban");
    ensure!(staged_bin.is_file(), "CLI archive did not contain `canban`");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged_bin, std::fs::Permissions::from_mode(0o755))?;
    }
    let out = std::process::Command::new(&staged_bin)
        .arg("build-info")
        .output()
        .context("Failed to execute staged CLI `build-info`")?;
    ensure!(out.status.success(), "Staged CLI `build-info` failed");
    let build: Build = serde_json::from_slice(&out.stdout)?;
    ensure!(
        build.version == release.manifest.version
            && build.commit == release.manifest.commit
            && build.platform == release.manifest.platform
            && build.schema == release.manifest.schema
            && build.api == release.manifest.api,
        "Staged CLI binary does not match manifest"
    );
    Ok(staged_bin)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_order_numerically_and_validate_dates() {
        assert!(Version::parse("20261110-10").unwrap() > Version::parse("20261110-9").unwrap());
        for s in [
            "20260229-1",
            "20261131-1",
            "20261110-0",
            "20261110-01",
            "../bad",
        ] {
            assert!(Version::parse(s).is_err());
        }
        assert!(Version::parse("20240229-1").is_ok());
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn quarantine_failure_prevents_candidate_execution_and_install_journal() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Canban.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let build = Build::current();
        let manifest = Manifest {
            version: build.version.clone(),
            commit: build.commit.clone(),
            platform: build.platform.clone(),
            api: build.api,
            schema: build.schema,
            rollback_floor: 8,
            asset: String::new(),
            size: 1,
            sha256: String::new(),
        };
        let cli = macos.join("canban");
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\n: > \"${{0}}.executed\"\nprintf '%s\\n' '{}'\n",
                serde_json::to_string(&build).unwrap()
            ),
        )
        .unwrap();
        for name in ["canban-ui", "canban-updater"] {
            std::fs::write(macos.join(name), b"fixture").unwrap();
        }
        xattr::set(&cli, "com.apple.quarantine", b"0081;fixture;Canban;").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o444)).unwrap();
        let result = validate_candidate(&bundle, &manifest);
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Could not remove candidate quarantine")
        );
        assert!(!macos.join("canban.executed").exists());
        assert!(!dir.path().join("updates/install.json").exists());
        validate_candidate(&bundle, &manifest).unwrap();
        assert!(macos.join("canban.executed").exists());
        assert!(xattr::get(&cli, "com.apple.quarantine").unwrap().is_none());
    }
    #[test]
    fn unsafe_paths_and_links_never_escape_staging() {
        for (name, kind) in [
            ("../escape", tar::EntryType::Regular),
            ("Canban.app/link", tar::EntryType::Symlink),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("unsafe.zst");
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_entry_type(kind);
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            if kind.is_symlink() {
                header.set_link_name("../../escape").unwrap();
            }
            header.set_cksum();
            let mut raw = header.as_bytes().to_vec();
            raw.extend_from_slice(&[0; 1024]);
            let compressed = zstd::encode_all(raw.as_slice(), 1).unwrap();
            std::fs::write(&archive, compressed).unwrap();
            let output = dir.path().join("staging");
            std::fs::create_dir(&output).unwrap();
            assert!(extract(&archive, &output).is_err());
            assert!(!dir.path().join("escape").exists());
        }
    }

    #[test]
    fn stage_cli_verifies_sha256_and_build_info_and_rejects_tampered_archives() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();

        let mut candidate = Build::current();
        candidate.version = "20261116-9".into();
        candidate.commit = "0123456789abcdef0123456789abcdef01234567".into();
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"build-info\" ]; then printf '%s\\n' '{}'; exit 0; fi\nexit 1\n",
            serde_json::to_string(&candidate).unwrap()
        );

        let mut tar_buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut tar_buf, flate2::Compression::default());
            let mut archive = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(script.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            archive
                .append_data(&mut header, "canban", script.as_bytes())
                .unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        let sha = format!("{:x}", Sha256::digest(&tar_buf));
        let size = tar_buf.len() as u64;

        let served_bytes = tar_buf.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = server.accept() {
                    let mut req = [0u8; 1024];
                    let _ = stream.read(&mut req);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                        served_bytes.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&served_bytes);
                }
            }
        });

        let avail = Available {
            manifest: Manifest {
                version: candidate.version.clone(),
                commit: candidate.commit.clone(),
                platform: candidate.platform.clone(),
                api: candidate.api,
                schema: candidate.schema,
                rollback_floor: 0,
                asset: format!("canban-cli-{}.tar.gz", candidate.platform),
                size,
                sha256: sha,
            },
            url: format!("http://{addr}/canban-cli.tar.gz"),
            notes: String::new(),
        };

        let staged = stage_cli(dir.path(), &avail, None).unwrap();
        assert!(staged.is_file());

        let mut bad = avail;
        bad.manifest.sha256 = "0".repeat(64);
        assert!(stage_cli(dir.path(), &bad, None).is_err());

        let _ = handle.join();
    }

    #[cfg(unix)]
    #[test]
    fn linux_desktop_bundle_packs_stages_and_validates_all_binaries_and_metadata() {
        use std::{io::Write, os::unix::fs::PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("canban-src");
        std::fs::create_dir_all(source.join("bin")).unwrap();
        std::fs::create_dir_all(source.join("share/applications")).unwrap();
        std::fs::create_dir_all(source.join("share/icons/hicolor/scalable/apps")).unwrap();

        let candidate = Build {
            version: "20261116-12".into(),
            commit: "abcdef0123456789abcdef0123456789abcdef01".into(),
            schema: canban_storage::migrations::CURRENT,
            api: DESKTOP_API,
            platform: "linux-x86_64".into(),
        };
        std::fs::write(
            source.join("bundle.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "app_id": "com.canban.desktop",
                "name": "Canban",
                "executable": "canban-ui",
                "version": candidate.version,
                "commit": candidate.commit,
                "sequence": "1",
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            source.join("share/applications/com.canban.desktop.desktop"),
            b"[Desktop Entry]\nName=Canban\nExec=canban-ui\n",
        )
        .unwrap();
        let cli_script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"build-info\" ]; then printf '%s\\n' '{}'; exit 0; fi\nexit 1\n",
            serde_json::to_string(&candidate).unwrap()
        );
        std::fs::write(source.join("bin/canban"), cli_script.as_bytes()).unwrap();
        std::fs::write(source.join("bin/canban-ui"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(source.join("bin/canban-updater"), b"#!/bin/sh\nexit 0\n").unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(
                source.join("bin").join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let archive = dir.path().join("Canban-linux-x86_64.tar.zst");
        pack(&source, &archive).unwrap();
        let archive_bytes = std::fs::read(&archive).unwrap();
        let sha = format!("{:x}", Sha256::digest(&archive_bytes));
        let size = archive_bytes.len() as u64;

        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let served = archive_bytes.clone();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = server.accept() {
                let mut req = [0u8; 1024];
                let _ = stream.read(&mut req);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    served.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&served);
            }
        });

        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let avail = Available {
            manifest: Manifest {
                version: candidate.version.clone(),
                commit: candidate.commit.clone(),
                platform: candidate.platform.clone(),
                api: candidate.api,
                schema: candidate.schema,
                rollback_floor: 8,
                asset: "Canban-linux-x86_64.tar.zst".into(),
                size,
                sha256: sha,
            },
            url: format!("http://{addr}/Canban-linux-x86_64.tar.zst"),
            notes: "Linux desktop release".into(),
        };

        let staged_bundle = stage(&home, &avail, None).unwrap();
        assert_eq!(
            staged_bundle.file_name().and_then(|s| s.to_str()),
            Some("canban")
        );
        assert!(staged_bundle.join("bundle.json").is_file());
        assert!(staged_bundle.join("bin/canban").is_file());
        assert!(staged_bundle.join("bin/canban-ui").is_file());
        assert!(staged_bundle.join("bin/canban-updater").is_file());
        let _ = handle.join();

        // Missing bundle.json or dev-instance.json in a Linux candidate is rejected.
        std::fs::write(staged_bundle.join("share/dev-instance.json"), b"{}").unwrap();
        assert!(validate_candidate(&staged_bundle, &avail.manifest).is_err());
        std::fs::remove_file(staged_bundle.join("share/dev-instance.json")).unwrap();
        std::fs::remove_file(staged_bundle.join("bundle.json")).unwrap();
        assert!(validate_candidate(&staged_bundle, &avail.manifest).is_err());

        // Platform mismatch (macos manifest against Linux directory bundle) is rejected.
        let mut mac_manifest = avail.manifest.clone();
        mac_manifest.platform = "macos-aarch64".into();
        assert!(validate_candidate(&source, &mac_manifest).is_err());
    }
}
