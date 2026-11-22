//! Cross-target smoke test: libghostty-vt links and parses VT output.
fn main() {
    let mut model = canban_terminal::model::Model::new(40, 8);
    model.write("hello \x1b[1mios\x1b[0m 世界\r\n".as_bytes());
    let text = model
        .frame()
        .rows
        .into_iter()
        .map(|r| r.into_iter().map(|c| c.text).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("hello ios 世界"),
        "unexpected screen: {text:?}"
    );
    println!(
        "vt_smoke ok: {}",
        text.lines().next().unwrap_or_default().trim_end()
    );
}
