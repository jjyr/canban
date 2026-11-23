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
#[cfg(target_os = "macos")]
pub mod apple;
pub mod input;
pub mod model;
#[cfg(target_os = "macos")]
pub use apple::macos as native;
pub mod portable;
pub mod remote;
pub mod session;
pub mod view;
pub mod view_state;

pub use remote::RemotePtyTransport;
pub use view::{
    FocusRequest, FontSpec, LiveBinding, TerminalView, decode_file_uri, parse_font_value,
};

/// Installed font family catalog discovered from the host OS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledFonts {
    pub monospaced: Vec<String>,
    pub others: Vec<String>,
}

/// Evicts any cached live PTY attachment session for the given `(core_id, terminal)` pair.
pub fn evict_session(core_id: &str, terminal: &str) {
    session::evict_session(core_id, terminal);
}

/// Returns the most recently measured live terminal viewport `(cols, rows)`, if any.
pub fn last_viewport_size() -> Option<(u16, u16)> {
    session::last_viewport_size()
}

/// Returns the most recently measured terminal cell `(width_milli_pt, height_milli_pt)`, if any.
pub fn last_cell_metrics_milli() -> Option<(u32, u32)> {
    session::last_cell_metrics_milli()
}

/// Formats local filesystem paths as a POSIX shell-escaped paste payload with a trailing space.
///
/// Safe ASCII paths (`[A-Za-z0-9_./:@+-]`) remain unquoted; paths containing spaces, quotes,
/// non-ASCII characters, or shell metacharacters are single-quoted with embedded `'` escaped
/// as `'\''`. Multiple paths are joined by single spaces and terminated with a trailing space
/// without appending a newline.
pub fn shell_escape_paths<I, S>(paths: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    for raw in paths {
        let path = raw.as_ref();
        if path.is_empty() {
            continue;
        }
        shell_escape_into(&mut out, path);
        out.push(' ');
    }
    out
}

/// Escapes a single argument for POSIX shells (`zsh` / `bash` / `sh` / `fish`).
pub fn shell_escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    shell_escape_into(&mut out, path);
    out
}

fn is_shell_safe_unquoted_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b':' | b'@' | b'+' | b'-')
}

fn shell_escape_into(out: &mut String, path: &str) {
    if path.is_empty() {
        out.push_str("''");
        return;
    }
    if path.bytes().all(is_shell_safe_unquoted_byte) {
        out.push_str(path);
        return;
    }
    if !path.contains('\'') && !path.chars().any(char::is_control) {
        out.push('\'');
        out.push_str(path);
        out.push('\'');
        return;
    }
    let mut in_single = false;
    for ch in path.chars() {
        if ch == '\'' {
            if in_single {
                out.push('\'');
                in_single = false;
            }
            out.push_str("\\'");
        } else if ch.is_control() {
            if in_single {
                out.push('\'');
                in_single = false;
            }
            // ANSI-C quoting $'\n' / $'\t' / $'\xNN' for control chars in filenames
            match ch {
                '\n' => out.push_str("$'\\n'"),
                '\r' => out.push_str("$'\\r'"),
                '\t' => out.push_str("$'\\t'"),
                _ => {
                    let mut buf = [0u8; 4];
                    out.push_str("$'");
                    for b in ch.encode_utf8(&mut buf).as_bytes() {
                        use std::fmt::Write;
                        let _ = write!(out, "\\x{b:02x}");
                    }
                    out.push('\'');
                }
            }
        } else {
            if !in_single {
                out.push('\'');
                in_single = true;
            }
            out.push(ch);
        }
    }
    if in_single {
        out.push('\'');
    }
}

/// Returns cached installed font families without blocking, or `None` if not yet scanned.
pub fn try_installed_fonts() -> Option<InstalledFonts> {
    #[cfg(target_os = "macos")]
    {
        native::try_installed_fonts()
    }
    #[cfg(target_os = "linux")]
    {
        portable::try_installed_fonts()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Returns the installed font families on the current host OS, separated into
/// verified ASCII-monospaced families and other families.
pub fn installed_fonts() -> InstalledFonts {
    #[cfg(target_os = "macos")]
    {
        native::installed_fonts()
    }
    #[cfg(target_os = "linux")]
    {
        portable::installed_fonts()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        InstalledFonts::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_paths_preserves_safe_ascii_and_quotes_special_or_unicode_paths() {
        assert_eq!(
            shell_escape_path("/Users/jjy/Workspace/canban/Cargo.toml"),
            "/Users/jjy/Workspace/canban/Cargo.toml"
        );
        assert_eq!(
            shell_escape_path("/Users/jjy/My Folder/file (1).png"),
            "'/Users/jjy/My Folder/file (1).png'"
        );
        assert_eq!(
            shell_escape_path("/tmp/$HOME & `uname` \"quoted\".txt"),
            "'/tmp/$HOME & `uname` \"quoted\".txt'"
        );
        assert_eq!(
            shell_escape_path("/Users/jjy/文档/测试截图.png"),
            "'/Users/jjy/文档/测试截图.png'"
        );
        assert_eq!(
            shell_escape_path("/Users/jjy/it's a 'quote'"),
            "'/Users/jjy/it'\\''s a '\\''quote'\\'"
        );
        assert_eq!(shell_escape_path("'leading"), "\\''leading'");
        assert_eq!(shell_escape_path("line1\nline2"), "'line1'$'\\n''line2'");

        assert_eq!(
            shell_escape_paths([
                "/Users/jjy/foo/bar.rs",
                "/Users/jjy/My Docs/it's 截图 (1).png",
            ]),
            "/Users/jjy/foo/bar.rs '/Users/jjy/My Docs/it'\\''s 截图 (1).png' "
        );
        assert_eq!(shell_escape_paths::<[&str; 0], &str>([]), "");
    }
}
