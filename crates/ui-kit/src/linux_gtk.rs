//! Linux GTK4 host stylesheet synchronization for Canban's dark themes and
//! uncomposited X11 display environments (TigerVNC, Openbox, i3, dwm, bspwm, X2Go).
//!
//! Why both layers are needed on Linux:
//! 1. **Uncomposited X11 (`gdk::Display::is_composited() == false`)**:
//!    GTK4 gives `GtkPopover` / `GtkPopoverMenu` and `GtkTooltip` an ARGB visual
//!    (`is_rgba() == true` on Xorg/Xvnc) and expands the underlying X11 override-redirect
//!    surface to accommodate CSS `box-shadow` blur extents and `border-radius` corners.
//!    Without a running X11 compositor (`picom`/`xcompmgr`/Mutter/KWin), X11 paints
//!    alpha=0 pixels in that ARGB popup window as solid black (`#000000`), producing a
//!    thick black rectangular halo around every popup menu, context menu, and native
//!    tooltip. Similarly, GTK4 automatically adds `.solid-csd` to client-side decorated
//!    windows on uncomposited X11 with a 5px neutral-gray (`#1b1b1b`/`#2d2d2d`) double
//!    bezel (`padding: 4px; box-shadow: inset ...`). Stripping popup shadows/radii and
//!    collapsing `.solid-csd` to a clean 1px themed border when `!display.is_composited()`
//!    eliminates every black bounding box on uncomposited X11 while preserving rounded
//!    corners and drop shadows on Wayland and composited X11 desktops, and dynamically
//!    updates via `connect_composited_notify` if a compositor starts or stops live.
//! 2. **GTK4 native widget & menu polish (composited and uncomposited)**:
//!    - Replaces `.nsh-dark`'s hardcoded `#0a0a0a` root backdrop with the active Canban
//!      theme's `BACKGROUND` color so initial window map/resize never flashes near-black.
//!    - Fixes `popover.menu separator`'s negative horizontal margin (`margin: 4px -4px`
//!      in `nsh-platform-linux`), which triggers `Gtk-WARNING: GtkSeparator reported min
//!      width -7, but sizes must be >= 0` on GTK 4.22+.
//!    - Styles `windowcontrols` hover/active states and native `popover` / `tooltip`
//!      surfaces (such as `GtkText` / `GtkTextView` right-click edit menus and
//!      `GtkWindowControls` tooltips) in Canban's active theme tokens.

/// Builds the Linux GTK4 stylesheet for `chrome` (`canban_themes::Theme::chrome()`)
/// and whether the display currently has a running compositor (`composited`).
pub fn gtk_css_for_chrome(chrome: &[u32; 16], composited: bool) -> String {
    let background = hex_rgb(chrome[0]);
    let surface = hex_rgb(chrome[1]);
    let inset = hex_rgb(chrome[2]);
    let text = hex_rgb(chrome[3]);
    let muted = hex_rgb(chrome[4]);
    let border = hex_rgb(chrome[5]);
    let selected = hex_rgb(chrome[11]);

    let mut css = format!(
        "\
.nsh-dark {{ background-color: {background}; color: {text}; }}
popover.background > contents {{
  background-color: {surface};
  color: {text};
  border: 1px solid {border};
  background-image: none;
}}
popover.background > arrow {{
  background-color: {surface};
  border-color: {border};
}}
popover modelbutton {{
  color: {text};
  background-image: none;
}}
popover modelbutton:hover, popover modelbutton:selected {{
  background-color: {selected};
  color: {text};
}}
popover modelbutton:disabled, popover modelbutton accelerator {{
  color: {muted};
}}
popover separator, popover.menu separator {{
  min-height: 1px;
  min-width: 1px;
  margin: 4px 0;
  background-color: {border};
}}
tooltip, tooltip.background {{
  background-color: {surface};
  color: {text};
  border: 1px solid {border};
  background-image: none;
}}
windowcontrols button > image {{
  background-color: transparent;
  background-image: none;
  color: {muted};
}}
windowcontrols button:hover > image {{
  background-color: {inset};
  color: {text};
}}
windowcontrols button:active > image {{
  background-color: {selected};
  color: {text};
}}
"
    );

    if !composited {
        css.push_str(&format!(
            "\
popover, popover.background, popover.menu {{
  margin: 0;
  padding: 0;
  border-radius: 0;
  box-shadow: none;
  background: none;
}}
popover > arrow, popover.background > arrow {{
  min-width: 0;
  min-height: 0;
  margin: 0;
  padding: 0;
  border: none;
  background: none;
  box-shadow: none;
}}
popover > contents, popover.background > contents, popover.menu > contents {{
  margin: 0;
  border-radius: 0;
  box-shadow: none;
}}
tooltip, tooltip.background, tooltip.csd {{
  margin: 0;
  border-radius: 0;
  box-shadow: none;
}}
window.csd, window.solid-csd {{
  margin: 0;
  padding: 0;
  border-radius: 0;
  box-shadow: none;
  border: 1px solid {border};
}}
window.csd.maximized, window.solid-csd.maximized,
window.csd.fullscreen, window.solid-csd.fullscreen,
window.csd.tiled, window.solid-csd.tiled {{
  border: none;
}}
"
        ));
    }

    css
}

fn hex_rgb(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0xffffff)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::gtk_css_for_chrome;
    use nsh_platform_linux::gtk4::{self as gtk, gdk, glib, prelude::*};
    use std::{
        cell::RefCell,
        sync::{Mutex, OnceLock},
    };

    /// Higher than `nsh-platform-linux`'s `STYLE_PROVIDER_PRIORITY_APPLICATION` (600)
    /// and input providers (`+ 1` / `+ 2`), while leaving `PRIORITY_USER` (800) free
    /// for user `~/.config/gtk-4.0/gtk.css` overrides.
    const PRIORITY_CANBAN_LINUX: u32 = gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 20;

    static LATEST_CHROME: OnceLock<Mutex<[u32; 16]>> = OnceLock::new();

    fn latest_chrome_mutex() -> &'static Mutex<[u32; 16]> {
        LATEST_CHROME.get_or_init(|| {
            Mutex::new(canban_themes::resolve(canban_themes::DEFAULT).chrome())
        })
    }

    struct GtkThreadState {
        provider: Option<gtk::CssProvider>,
        watching_display: bool,
        last_css: String,
    }

    thread_local! {
        static GTK_STATE: RefCell<GtkThreadState> = const {
            RefCell::new(GtkThreadState {
                provider: None,
                watching_display: false,
                last_css: String::new(),
            })
        };
    }

    pub fn sync(chrome: [u32; 16]) {
        *latest_chrome_mutex()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = chrome;

        // `install_theme()` and `set_theme()` execute on NSH's engine thread inside
        // component code (or on test worker threads during headless unit tests).
        // Schedule application on GLib's default main context, where `run_app` runs
        // GTK's main loop; if GTK has not initialized on that thread, it is a no-op.
        glib::MainContext::default().invoke(|| {
            apply_on_gtk_main();
        });
    }

    fn apply_on_gtk_main() {
        if !gtk::is_initialized_main_thread() {
            return;
        }
        let Some(display) = gdk::Display::default() else {
            return;
        };

        let chrome = *latest_chrome_mutex()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let css = gtk_css_for_chrome(&chrome, display.is_composited());

        let need_watch = GTK_STATE.with(|cell| {
            let mut st = cell.borrow_mut();
            let provider = st
                .provider
                .get_or_insert_with(|| {
                    let p = gtk::CssProvider::new();
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &p,
                        PRIORITY_CANBAN_LINUX,
                    );
                    p
                })
                .clone();
            if st.last_css != css {
                provider.load_from_string(&css);
                st.last_css = css;
            }
            !std::mem::replace(&mut st.watching_display, true)
        });

        if need_watch {
            display.connect_composited_notify(|_| {
                apply_on_gtk_main();
            });
        }
    }
}

#[cfg(target_os = "linux")]
pub fn sync_theme(chrome: [u32; 16]) {
    imp::sync(chrome);
}

#[cfg(not(target_os = "linux"))]
pub fn sync_theme(_chrome: [u32; 16]) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncomposited_css_strips_popup_shadows_radii_and_solid_csd_bezel() {
        let chrome = canban_themes::resolve("canban").chrome();
        let uncomposited = gtk_css_for_chrome(&chrome, false);
        assert!(uncomposited.contains(".nsh-dark { background-color: #101318;"));
        assert!(uncomposited.contains("popover, popover.background, popover.menu"));
        assert!(uncomposited.contains("popover > contents, popover.background > contents, popover.menu > contents"));
        assert!(uncomposited.contains("border-radius: 0;"));
        assert!(uncomposited.contains("box-shadow: none;"));
        assert!(uncomposited.contains("window.csd, window.solid-csd"));
        assert!(uncomposited.contains("border: 1px solid #303946;"));
        assert!(uncomposited.contains("popover separator, popover.menu separator"));
        assert!(uncomposited.contains("margin: 4px 0;"));
    }

    #[test]
    fn composited_css_preserves_rounded_popups_and_shadows_while_fixing_separators_and_theme() {
        let chrome = canban_themes::resolve("dracula").chrome();
        let composited = gtk_css_for_chrome(&chrome, true);
        assert!(composited.contains(&format!(
            ".nsh-dark {{ background-color: #{:06x};",
            chrome[0]
        )));
        assert!(composited.contains("popover separator, popover.menu separator"));
        assert!(composited.contains("margin: 4px 0;"));
        assert!(!composited.contains("window.solid-csd"));
        assert!(!composited.contains("popover > contents, popover.background > contents, popover.menu > contents"));
    }
}
