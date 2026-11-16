//! Platform-specific desktop host setup, window launch, external URL/path opening,
//! and local machine copy helpers shared across macOS (`nsh-platform-macos`) and
//! Linux (`nsh-platform-linux`).
use anyhow::{Context, Result, ensure};

/// Human-readable display label for the local machine (`"This Mac"` on macOS,
/// `"This Linux"` on Linux).
#[cfg(target_os = "macos")]
pub const LOCAL_HOST_NAME: &str = "This Mac";

#[cfg(not(target_os = "macos"))]
pub const LOCAL_HOST_NAME: &str = "This Linux";

/// Conversational phrase for `"on this Mac"` / `"on this machine"`.
#[cfg(target_os = "macos")]
pub const ON_THIS_MACHINE: &str = "on this Mac";

#[cfg(not(target_os = "macos"))]
pub const ON_THIS_MACHINE: &str = "on this machine";

/// Registers Canban's native UI sets and platform-specific native components
/// before launching the main application window.
pub fn init_host(ui_sets: Vec<nsh_core::element::Registry>) {
    #[cfg(target_os = "macos")]
    {
        nsh_platform_macos::set_components(canban_terminal::native::register);
        nsh_platform_macos::set_ui_sets(ui_sets);
    }
    #[cfg(target_os = "linux")]
    {
        nsh_platform_linux::set_ui_sets(ui_sets);
    }
}

/// Runs a root NSH component in a standalone desktop window until closed.
/// Returns `true` when the native event loop exited cleanly (`SUCCESS`).
pub fn run_root<C>(app_id: String, title: impl Into<String>, size: (i32, i32)) -> bool
where
    C: nsh_core::element::Component,
    C::Props: Default,
{
    let title = title.into();
    #[cfg(target_os = "macos")]
    {
        let code = nsh_platform_macos::run_app(
            nsh_platform_macos::AppSource::Rust(
                nsh_platform_macos::Root::of::<C>().app_id(app_id.clone()),
            ),
            nsh_platform_macos::AppOptions {
                title,
                app_id,
                size,
                appearance: nsh_platform_macos::Appearance::Dark,
                ..Default::default()
            },
        );
        code == std::process::ExitCode::SUCCESS
    }
    #[cfg(target_os = "linux")]
    {
        canban_ui_kit::linux_gtk::sync_theme(
            canban_ui_kit::tokens::current_theme().chrome(),
        );
        let code = nsh_platform_linux::run_app(
            nsh_platform_linux::AppSource::Rust(
                nsh_platform_linux::Root::of::<C>().app_id(app_id.clone()),
            ),
            nsh_platform_linux::AppOptions {
                title,
                app_id,
                size,
                appearance: nsh_platform_linux::Appearance::Dark,
                ..Default::default()
            },
        );
        code == nsh_platform_linux::gtk4::glib::ExitCode::SUCCESS
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (app_id, title, size);
        false
    }
}

/// Opens an external URL or local filesystem path with the desktop's default handler.
///
/// - macOS: `/usr/bin/open <target>`
/// - Linux: `xdg-open <target>`, falling back to `gio open <target>` if `xdg-open`
///   is unavailable or returns non-zero.
pub fn open_external(target: &str) -> Result<()> {
    ensure!(!target.trim().is_empty(), "External target is empty");
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/open")
            .arg(target)
            .status()
            .context("Could not run /usr/bin/open")?;
        ensure!(status.success(), "macOS could not open {target}");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::process::{Command, Stdio};
        let xdg = Command::new("xdg-open")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if matches!(xdg, Ok(status) if status.success()) {
            return Ok(());
        }
        let gio = Command::new("gio")
            .args(["open", target])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Neither `xdg-open` nor `gio open` is available to open external targets")?;
        ensure!(gio.success(), "Desktop handler could not open {target}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_external_rejects_blank_targets() {
        assert!(open_external("").is_err());
        assert!(open_external("   ").is_err());
    }

    #[test]
    fn local_host_copy_matches_target_platform() {
        #[cfg(target_os = "macos")]
        {
            assert_eq!(LOCAL_HOST_NAME, "This Mac");
            assert_eq!(ON_THIS_MACHINE, "on this Mac");
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(LOCAL_HOST_NAME, "This Linux");
            assert_eq!(ON_THIS_MACHINE, "on this machine");
        }
    }
}
