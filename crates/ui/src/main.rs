fn main() {
    if let Err(error) = canban_ui::run() {
        eprintln!("Canban: {error:#}");
        std::process::exit(1);
    }
}
