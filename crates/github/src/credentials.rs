//! Credentials live outside configuration, Card data and execution context.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const FILE: &str = "github.json";

pub fn read(home: &Path) -> Result<Option<Vec<u8>>> {
    let directory = home.join("credentials");
    if !directory.exists() {
        return Ok(None);
    }
    private_directory(&directory)?;
    let path = directory.join(FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "GitHub credential path must be a regular file"
    );
    private_permissions(&path, 0o600)?;
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "GitHub credential file is too large");
    Ok(Some(bytes))
}

pub fn store(home: &Path, bytes: Option<&[u8]>) -> Result<()> {
    let directory = home.join("credentials");
    if bytes.is_none() && !directory.exists() {
        return Ok(());
    }
    private_directory(&directory)?;
    let path = directory.join(FILE);
    if let Some(bytes) = bytes {
        let mut pending = tempfile::NamedTempFile::new_in(&directory)?;
        private_permissions(pending.path(), 0o600)?;
        pending.write_all(bytes)?;
        pending.as_file().sync_all()?;
        pending
            .persist(&path)
            .context("Could not replace GitHub credential file")?;
    } else if let Err(e) = fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(e.into());
    }
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "GitHub credential directory must not be a symlink"
    );
    private_permissions(path, 0o700)
}

fn private_permissions(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_survive_restart_rotate_privately_and_disconnect() {
        let home = tempfile::tempdir().unwrap();
        assert!(read(home.path()).unwrap().is_none());
        store(home.path(), Some(b"first")).unwrap();
        assert_eq!(read(home.path()).unwrap().unwrap(), b"first");
        store(home.path(), Some(b"rotated")).unwrap();
        assert_eq!(read(home.path()).unwrap().unwrap(), b"rotated");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.path().join("credentials"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(home.path().join("credentials/github.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        store(home.path(), None).unwrap();
        assert!(read(home.path()).unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_linked_credential_paths() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        symlink(other.path(), home.path().join("credentials")).unwrap();
        assert!(store(home.path(), Some(b"private")).is_err());
        assert!(read(home.path()).is_err());
        fs::remove_file(home.path().join("credentials")).unwrap();
        fs::create_dir(home.path().join("credentials")).unwrap();
        fs::write(other.path().join("target"), b"other").unwrap();
        symlink(
            other.path().join("target"),
            home.path().join("credentials/github.json"),
        )
        .unwrap();
        assert!(read(home.path()).is_err());
        store(home.path(), Some(b"new")).unwrap();
        assert_eq!(fs::read(other.path().join("target")).unwrap(), b"other");
        assert_eq!(read(home.path()).unwrap().unwrap(), b"new");
    }
}
