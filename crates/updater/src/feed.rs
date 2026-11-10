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
            if Version::parse(&current.version)
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
    let _lock = lock(home, "operation.lock")?;
    ensure!(!handoff::active(home), "Upgrade is in progress");
    let mut state = load_state(home);
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
    extract(&archive, &job)?;
    let bundle = job.join("Canban.app");
    validate_candidate(&bundle, &release.manifest)?;
    Ok(bundle)
}
fn validate_candidate(bundle: &Path, manifest: &Manifest) -> Result<()> {
    for binary in ["canban", "canban-ui", "canban-updater"] {
        ensure!(
            bundle.join("Contents/MacOS").join(binary).is_file(),
            "Incomplete App bundle: {binary}"
        );
    }
    crate::quarantine::prepare(bundle)?;
    let output = std::process::Command::new(bundle.join("Contents/MacOS/canban"))
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
fn extract(archive: &Path, job: &Path) -> Result<()> {
    let decoder = zstd::Decoder::new(File::open(archive)?)?;
    let mut tar = tar::Archive::new(decoder);
    let mut expanded = 0u64;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.components().all(|c| matches!(c, Component::Normal(_)))
                && path
                    .components()
                    .next()
                    .is_some_and(|c| c.as_os_str() == "Canban.app"),
            "Unsafe update archive path"
        );
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
    Ok(())
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
}
