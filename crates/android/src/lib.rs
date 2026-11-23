//! Standalone Android application library hosted on `nsh-platform-android`.
use std::path::PathBuf;

/// Resolves the persistent application data directory for paired host configs,
/// offline outbox queues, read caches, and device credentials.
pub fn resolve_data_dir(platform_dir: Option<PathBuf>, home_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = platform_dir.filter(|p| !p.as_os_str().is_empty()) {
        return dir;
    }
    if let Some(home) = home_dir.filter(|p| !p.as_os_str().is_empty()) {
        return home.join("dev.canban.mobile");
    }
    std::env::temp_dir().join("canban-android")
}

#[cfg(target_os = "android")]
mod android_app {
    use super::resolve_data_dir;
    use canban_client::{FileCredentialStore, RemoteClient, RemoteClientConfig};
    use nsh_core::prelude::*;
    use std::{path::PathBuf, sync::Arc};

    #[component]
    pub fn CanbanAndroidApp() -> impl IntoElement {
        if !canban_ui::backend::is_connected() {
            let dir = resolve_data_dir(data_dir(), std::env::var_os("HOME").map(PathBuf::from));
            let _ = std::fs::create_dir_all(&dir);
            let creds = Arc::new(FileCredentialStore::open(&dir));
            let mut cfg = RemoteClientConfig::new(dir);
            cfg.device_name = "Canban Android".to_owned();
            if let Ok(client) = RemoteClient::new(cfg, creds) {
                let backend: Arc<dyn canban_ui::backend::UiBackend> =
                    Arc::new(canban_ui::backend::RemoteBackend::new(client));
                let _ = canban_ui::backend::set(backend);
            }
        }
        canban_ui::mobile::CanbanMobile::element(())
    }

    nsh::ui_sets!(vec![nsh_base::registry(), canban_ui::registry()]);
    nsh::main!(
        nsh::Root::of::<CanbanAndroidApp>()
            .app_id("dev.canban.mobile")
            .url_schemes(["canban"])
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_data_dir_prefers_platform_dir_then_home_then_temp() {
        assert_eq!(
            resolve_data_dir(
                Some(PathBuf::from(
                    "/data/user/0/dev.canban.mobile/files/nsh/dev.canban.mobile"
                )),
                Some(PathBuf::from("/data/data"))
            ),
            PathBuf::from("/data/user/0/dev.canban.mobile/files/nsh/dev.canban.mobile")
        );
        assert_eq!(
            resolve_data_dir(None, Some(PathBuf::from("/data/user/0"))),
            PathBuf::from("/data/user/0/dev.canban.mobile")
        );
        assert_eq!(
            resolve_data_dir(Some(PathBuf::new()), Some(PathBuf::from("/data/user/0"))),
            PathBuf::from("/data/user/0/dev.canban.mobile")
        );
        assert_eq!(
            resolve_data_dir(None, None),
            std::env::temp_dir().join("canban-android")
        );
    }
}
