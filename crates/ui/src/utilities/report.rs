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
fn body() -> String {
    let template = include_str!("../../../../.github/ISSUE_TEMPLATE/bug_report.md");
    let (_, body) = template
        .strip_prefix("---\n")
        .expect("issue template front matter")
        .split_once("\n---\n")
        .expect("issue template body");
    body.trim_start().replace(
        "<!-- Required: enter the Canban version you are using (Settings -> Copy diagnostics). -->",
        &diagnostics(),
    )
}
pub fn url() -> String {
    format!(
        "https://github.com/jjyr/canban/issues/new?template=bug_report.md&body={}",
        encode(&body())
    )
}
pub fn open(ui: Ui) {
    crate::github::open_url(ui, url());
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_opens_markdown_template_and_prefills_public_version() {
        let u = url();
        assert!(
            u.starts_with("https://github.com/jjyr/canban/issues/new?template=bug_report.md&body=")
        );
        assert!(!u.contains('\n'));
        assert!(u.contains(&format!(
            "Canban%20{}%0ACommit%3A%20",
            canban_updater::Build::current().version
        )));
        assert!(u.contains("%0APlatform%3A%20"));
        assert!(body().contains("## Canban version\n\nCanban "));
        for heading in ["What happened?", "Steps to reproduce", "Additional context"] {
            assert!(body().contains(&format!("## {heading}")));
        }
        assert!(!body().contains("Required: enter"));
        assert!(!body().starts_with("---"));
        assert!(!u.contains("&title="));
        assert!(!u.contains("token"));
        assert!(u.len() < 2048);
    }
}
