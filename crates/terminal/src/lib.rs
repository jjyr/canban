//! Native terminal rendering over the externally owned Herdr PTY.
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
pub mod model;

#[cfg(target_os = "macos")]
pub mod native;

/// Installed font family catalog discovered from the host OS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledFonts {
    pub monospaced: Vec<String>,
    pub others: Vec<String>,
}

/// Returns the installed font families on the current host OS, separated into
/// verified ASCII-monospaced families and other families.
pub fn installed_fonts() -> InstalledFonts {
    #[cfg(target_os = "macos")]
    {
        native::installed_fonts()
    }
    #[cfg(not(target_os = "macos"))]
    {
        InstalledFonts::default()
    }
}
