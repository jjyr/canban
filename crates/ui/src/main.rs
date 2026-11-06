use anyhow::Result;
use std::sync::Arc;
mod components;
mod presenter;
mod profiles;
mod screens;
mod sessions;
use canban_ui_kit::tokens::*;
use nsh_core::prelude::*;
use presenter::*;
static APP: std::sync::OnceLock<Arc<canban_app_server::AppServer>> = std::sync::OnceLock::new();
static GALLERY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
#[native_module("canban/native")]
fn native(_: &mut Registry) {}
#[component]
pub fn Canban() -> impl IntoElement {
    let ui = Ui::new();
    if *GALLERY.get().unwrap_or(&false) {
        ui.page.set("gallery".into());
    }
    refresh(ui);
    let events = stored(crate::APP.get().unwrap().subscribe());
    let dirty = signal(false);
    set_interval(std::time::Duration::from_millis(250), move || {
        if events.with(|rx| rx.try_iter().count() > 0) {
            dirty.set(true);
        }
        if dirty.get_untracked() && !ui.refreshing.get_untracked() {
            dirty.set(false);
            refresh(ui);
        }
    });
    view()
        .flex_1()
        .bg(BACKGROUND)
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(screens::sidebar(ui))
                .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
                    let page = ui.page.get();
                    let _ = ui.card.get();
                    let _ = ui.project.get();
                    untracked(|| screens::screen(ui, &page))
                }))),
        )
        .child(
            view()
                .h(px(28.0))
                .px_4()
                .justify_center()
                .border_t_1()
                .border_color(BORDER)
                .child(canban_ui_kit::muted(move || ui.status.get())),
        )
}
fn main() {
    if let Err(e) = run() {
        eprintln!("Canban: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let home = canban_app_server::default_home()?;
    let cli = std::env::current_exe()?.with_file_name("canban");
    anyhow::ensure!(
        cli.exists(),
        "Build the Agent CLI first: mise exec -- cargo build -p canban-cli"
    );
    let app = canban_app_server::AppServer::open(home, cli)?;
    let _runtime = app.start()?;
    APP.set(app).ok();
    GALLERY.set(std::env::args().any(|s| s == "--gallery")).ok();
    nsh_platform_macos::set_components(canban_terminal::native::register);
    nsh_platform_macos::set_ui_sets(vec![nsh_base::registry(), registry()]);
    let result = nsh_platform_macos::run_app(
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<Canban>().app_id("com.canban.desktop"),
        ),
        nsh_platform_macos::AppOptions {
            title: "Canban — Human Context".into(),
            app_id: "com.canban.desktop".into(),
            size: (1360, 900),
            ..Default::default()
        },
    );
    if result != std::process::ExitCode::SUCCESS {
        anyhow::bail!("Native application exited with an error");
    }
    Ok(())
}
