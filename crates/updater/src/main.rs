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
        let encoder = zstd::Encoder::new(std::fs::File::create(output)?, 9)?;
        let mut tar = tar::Builder::new(encoder);
        tar.append_dir_all("Canban.app", source)?;
        let encoder = tar.into_inner()?;
        encoder.finish()?.sync_all()?;
        return Ok(());
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
