//! Platform-specific desktop host setup, window launch, external URL/path opening,
//! and local machine copy helpers shared across macOS (`nsh-platform-macos`) and
//! Linux (`nsh-platform-linux`).
#[cfg(feature = "local-host")]
use anyhow::{Context, Result, ensure};
#[cfg(not(feature = "local-host"))]
use anyhow::{Result, ensure};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{marker::PhantomData, sync::Mutex};

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Clone)]
struct WindowLaunch {
    title: String,
    size: (i32, i32),
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
static WINDOW_LAUNCH: Mutex<Option<WindowLaunch>> = Mutex::new(None);

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct WindowHost<C>(PhantomData<C>);

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl<C> nsh_core::element::Component for WindowHost<C>
where
    C: nsh_core::element::Component,
    C::Props: Default,
{
    type Props = ();
    type Event = nsh_core::element::NoEvent;

    fn view(
        _props: Self::Props,
        _cx: nsh_core::element::Scope<Self::Event>,
    ) -> impl nsh_core::element::IntoElement {
        let launch = WINDOW_LAUNCH
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| WindowLaunch {
                title: "Canban".to_owned(),
                size: (1360, 900),
            });
        nsh_core::element::desktop::window()
            .id("main")
            .title(launch.title)
            .default_size(nsh_core::geometry::size(
                nsh_core::geometry::px(launch.size.0 as f32),
                nsh_core::geometry::px(launch.size.1 as f32),
            ))
            .on_open_change(|change| {
                if !change.open {
                    nsh_core::element::desktop::quit();
                }
            })
            .content(|| C::element(C::Props::default()))
    }
}

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
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = ui_sets;
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
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        *WINDOW_LAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(WindowLaunch {
            title: title.clone(),
            size,
        });
    }
    #[cfg(target_os = "macos")]
    {
        let code = nsh_platform_macos::run_app(
            nsh_platform_macos::AppSource::Rust(
                nsh_platform_macos::Root::of::<WindowHost<C>>().app_id(app_id.clone()),
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
        let code = nsh_platform_linux::run_app(
            nsh_platform_linux::AppSource::Rust(
                nsh_platform_linux::Root::of::<WindowHost<C>>().app_id(app_id.clone()),
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
    #[cfg(not(feature = "local-host"))]
    {
        anyhow::bail!("Opening external targets requires the local-host feature")
    }
    #[cfg(all(feature = "local-host", target_os = "macos"))]
    {
        let status = canban_exec::raw_command("/usr/bin/open")
            .arg(target)
            .status()
            .context("Could not run /usr/bin/open")?;
        ensure!(status.success(), "macOS could not open {target}");
        Ok(())
    }
    #[cfg(all(feature = "local-host", not(target_os = "macos")))]
    {
        use std::process::Stdio;
        let xdg = canban_exec::command("xdg-open")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if matches!(xdg, Ok(status) if status.success()) {
            return Ok(());
        }
        let gio = canban_exec::command("gio")
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
