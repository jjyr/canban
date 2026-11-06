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
