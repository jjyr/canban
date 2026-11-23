//! Standalone iOS application entrypoint hosted on `nsh-platform-ios`.
use std::path::PathBuf;

/// Resolves the persistent application support directory for paired host configs,
/// offline outbox queues, read caches, and device credentials.
pub fn resolve_data_dir(platform_dir: Option<PathBuf>, home_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = platform_dir.filter(|p| !p.as_os_str().is_empty()) {
        return dir;
    }
    if let Some(home) = home_dir.filter(|p| !p.as_os_str().is_empty()) {
        return home
            .join("Library")
            .join("Application Support")
            .join("dev.canban.mobile");
    }
    std::env::temp_dir().join("canban-ios")
}

#[cfg(target_os = "ios")]
mod ios_app {
    use super::resolve_data_dir;
    use canban_client::{FileCredentialStore, RemoteClient, RemoteClientConfig};
    use nsh_core::prelude::*;
    use std::{path::PathBuf, sync::Arc};

    #[component]
    pub fn CanbanIosApp() -> impl IntoElement {
        if !canban_ui::backend::is_connected() {
            let dir = resolve_data_dir(data_dir(), std::env::var_os("HOME").map(PathBuf::from));
            let _ = std::fs::create_dir_all(&dir);
            // FileCredentialStore avoids `-34018` missing-entitlement Keychain failures on
            // ad-hoc signed Simulator builds while conforming to the `CredentialStore` trait.
            let creds = Arc::new(FileCredentialStore::open(&dir));
            let mut cfg = RemoteClientConfig::new(dir);
            cfg.device_name = "Canban iOS".to_owned();
            if let Ok(client) = RemoteClient::new(cfg, creds) {
                let backend: Arc<dyn canban_ui::backend::UiBackend> =
                    Arc::new(canban_ui::backend::RemoteBackend::new(client));
                let _ = canban_ui::backend::set(backend);
            }
        }
        canban_ui::mobile::CanbanMobile::element(())
    }

    pub fn launch() {
        nsh_platform_ios::set_ui_sets(vec![nsh_base::registry(), canban_ui::registry()]);
        nsh_platform_ios::run_app(
            nsh_platform_ios::AppSource::Rust(
                nsh_platform_ios::Root::of::<CanbanIosApp>()
                    .app_id("dev.canban.mobile")
                    .url_schemes(["canban"]),
            ),
            nsh_platform_ios::AppOptions::default(),
        );
    }
}

#[cfg(target_os = "ios")]
fn main() {
    ios_app::launch();
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!(
        "canban-ios is intended for iOS targets (aarch64-apple-ios-sim / aarch64-apple-ios). Run `mise run ios-sim`."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_data_dir_prefers_platform_dir_then_app_support_then_temp() {
        assert_eq!(
            resolve_data_dir(
                Some(PathBuf::from("/var/mobile/Data")),
                Some(PathBuf::from("/var/mobile"))
            ),
            PathBuf::from("/var/mobile/Data")
        );
        assert_eq!(
            resolve_data_dir(None, Some(PathBuf::from("/Users/sim"))),
            PathBuf::from("/Users/sim/Library/Application Support/dev.canban.mobile")
        );
        assert_eq!(
            resolve_data_dir(Some(PathBuf::new()), Some(PathBuf::from("/Users/sim"))),
            PathBuf::from("/Users/sim/Library/Application Support/dev.canban.mobile")
        );
        assert_eq!(
            resolve_data_dir(None, None),
            std::env::temp_dir().join("canban-ios")
        );
    }
}
