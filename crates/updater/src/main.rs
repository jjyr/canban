fn main() {
    if let Err(e) = run() {
        eprintln!("Canban updater: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if command.as_deref() == Some("pack") {
        let source = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("App bundle missing"))?;
        let output = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("Archive path missing"))?;
        return canban_updater::feed::pack(
            std::path::Path::new(&source),
            std::path::Path::new(&output),
        );
    }
    anyhow::ensure!(
        command.as_deref() == Some("--home"),
        "Use --home HOME [--recover]"
    );
    let home =
        std::path::PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("Home missing"))?);
    let recover = args.next().as_deref() == Some("--recover");
    canban_updater::worker::run(&home, recover)
}
