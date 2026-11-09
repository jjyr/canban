use crate::presenter::*;
use nsh_core::prelude::*;
pub fn diagnostics() -> String {
    format!(
        "Canban {}\nPlatform: {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn url() -> String {
    format!(
        "https://github.com/jjyr/canban/issues/new?body={}",
        encode(&format!(
            "## Expected behavior\n\n\n## Actual behavior\n\n\n## Steps to reproduce\n\n1. \n\n## Environment\n\n{}\n",
            diagnostics()
        ))
    )
}
pub fn open(ui: Ui) {
    crate::github::open_url(ui, url());
}
pub fn external(ui: Ui, url: &'static str) {
    ui.spawn(async move {
        let result = blocking(move || {
            std::process::Command::new("/usr/bin/open")
                .arg(url)
                .status()
        })
        .await;
        if !matches!(result,Ok(Ok(s)) if s.success()) {
            ui.operation_error.set("Could not open usage page".into());
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_only_prefills_public_metadata_and_never_submits() {
        let u = url();
        assert!(u.starts_with("https://github.com/jjyr/canban/issues/new?body="));
        assert!(!u.contains('\n'));
        assert!(u.contains("Expected%20behavior"));
        assert!(!u.contains("token"));
    }
}
