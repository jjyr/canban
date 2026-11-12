use crate::presenter::*;
pub fn diagnostics() -> String {
    format!(
        "Canban {}\nCommit: {}\nPlatform: {} {}",
        canban_updater::Build::current().version,
        canban_updater::Build::current().commit,
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
        "https://github.com/jjyr/canban/issues/new?template=bug_report.yml&environment={}",
        encode(&diagnostics())
    )
}
pub fn open(ui: Ui) {
    crate::github::open_url(ui, url());
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_opens_bug_form_and_only_prefills_public_environment() {
        let u = url();
        assert!(u.starts_with(
            "https://github.com/jjyr/canban/issues/new?template=bug_report.yml&environment="
        ));
        assert!(!u.contains('\n'));
        assert!(u.contains(&format!(
            "Canban%20{}%0ACommit%3A%20",
            canban_updater::Build::current().version
        )));
        assert!(u.contains("%0APlatform%3A%20"));
        assert!(u.ends_with(&format!(
            "{}%20{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
        assert!(!u.contains("&body="));
        assert!(!u.contains("&title="));
        assert!(!u.contains("token"));
        assert!(u.len() < 1024);
    }
}
