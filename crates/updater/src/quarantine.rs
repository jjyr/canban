//! Remove only macOS download quarantine from a verified candidate App.
//! Called after archive verification, before any candidate execution/shutdown.
use anyhow::Result;
use std::path::Path;

pub(crate) fn prepare(bundle: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use anyhow::{Context, ensure};
        const ATTRIBUTE: &str = "com.apple.quarantine";
        ensure!(
            bundle.extension().is_some_and(|s| s == "app")
                && std::fs::symlink_metadata(bundle)?.is_dir(),
            "Quarantine preparation requires a candidate App directory"
        );
        fn visit(path: &Path) -> Result<()> {
            let kind = std::fs::symlink_metadata(path)
                .with_context(|| format!("Could not inspect candidate {}", path.display()))?
                .file_type();
            ensure!(
                kind.is_file() || kind.is_dir(),
                "Candidate contains a link or special file: {}",
                path.display()
            );
            if xattr::get(path, ATTRIBUTE)
                .with_context(|| {
                    format!("Could not inspect candidate quarantine: {}", path.display())
                })?
                .is_some()
            {
                xattr::remove(path, ATTRIBUTE).with_context(|| {
                    format!("Could not remove candidate quarantine: {}", path.display())
                })?;
                ensure!(
                    xattr::get(path, ATTRIBUTE)?.is_none(),
                    "Candidate quarantine remains: {}",
                    path.display()
                );
            }
            if kind.is_dir() {
                for entry in std::fs::read_dir(path)? {
                    visit(&entry?.path())?;
                }
            }
            Ok(())
        }
        visit(bundle)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = bundle;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    const QUARANTINE: &str = "com.apple.quarantine";
    const RETAINED: &str = "com.canban.test.retained";

    #[test]
    fn removes_only_candidate_quarantine_recursively_and_is_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Canban.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let executable = macos.join("canban");
        std::fs::write(&executable, b"candidate").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let outside = dir.path().join("old-app");
        std::fs::write(&outside, b"retain").unwrap();
        for path in [&bundle, &macos, &executable, &outside] {
            xattr::set(path, QUARANTINE, b"0081;fixture;Canban;").unwrap();
            xattr::set(path, RETAINED, b"preserved").unwrap();
        }
        prepare(&bundle).unwrap();
        prepare(&bundle).unwrap();
        for path in [&bundle, &macos, &executable] {
            assert!(xattr::get(path, QUARANTINE).unwrap().is_none());
            assert_eq!(xattr::get(path, RETAINED).unwrap().unwrap(), b"preserved");
        }
        assert!(xattr::get(&outside, QUARANTINE).unwrap().is_some());
        assert_eq!(std::fs::read(&executable).unwrap(), b"candidate");
        assert_eq!(
            std::fs::metadata(executable).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn refuses_symlinks_without_touching_their_targets() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"retain").unwrap();
        xattr::set(&outside, QUARANTINE, b"0081;fixture;Canban;").unwrap();
        let bundle = dir.path().join("Canban.app");
        std::fs::create_dir(&bundle).unwrap();
        symlink(&outside, bundle.join("link")).unwrap();
        assert!(
            prepare(&bundle)
                .unwrap_err()
                .to_string()
                .contains("link or special file")
        );
        assert!(xattr::get(&outside, QUARANTINE).unwrap().is_some());
    }

    #[test]
    fn failed_removal_aborts_preparation_and_can_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Canban.app");
        std::fs::create_dir(&bundle).unwrap();
        let file = bundle.join("read-only");
        std::fs::write(&file, b"candidate").unwrap();
        xattr::set(&file, QUARANTINE, b"0081;fixture;Canban;").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        let result = prepare(&bundle);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Could not remove candidate quarantine")
        );
        assert!(xattr::get(&file, QUARANTINE).unwrap().is_some());
        prepare(&bundle).unwrap();
        assert!(xattr::get(&file, QUARANTINE).unwrap().is_none());
    }
}
