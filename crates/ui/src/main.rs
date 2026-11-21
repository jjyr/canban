#[cfg(feature = "local-host")]
fn main() {
    if let Err(error) = canban_ui::run() {
        eprintln!("Canban: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "local-host"))]
fn main() {
    eprintln!("Canban desktop binary requires the `local-host` Cargo feature.");
    std::process::exit(1);
}
