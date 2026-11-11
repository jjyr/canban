# Project guide

- Read [docs/README.md](docs/README.md) for current product and architecture directions.
- Keep project documentation in English.
- Follow [development conventions](docs/development-conventions.md).
- Cargo package names use canban-xxx; crate directories use crates/xxx and source/module filenames omit the canban- prefix.
- Canban uses Apache-2.0. Future Cargo workspace packages inherit that license. Preserve applicable third-party licenses/notices when importing code or assets.
- Original Canban copyright attribution is JJy <jjyruby@gmail.com>, starting in 2026. Preserve third-party attribution separately.
- Keep the main components in separate crates: AppServer, Kanban, Workflow Engine, Scheduler, UI, and the supporting boundaries documented in docs.
- Follow the selected [UI architecture](docs/ui-architecture.md): separate canban-ui-kit from product UI, reuse suitable NSH behavior, maintain component contracts and a native gallery using production components, and fix shared defects in their owning layer with appropriate regression scenarios.
- The repository now contains a macOS MVP. Read [implementation and acceptance](docs/implementation.md) for implemented behavior versus remaining proposals; historical research/specification entries are not runtime acceptance evidence.
- Use mise run build/test/acceptance for implementation checks. Use `mise run gallery` (the standalone `examples/gallery` app) to check UI component changes, adding scenes/regression states there. Validate application-flow changes with the running .local/Canban.app; compilation alone does not establish interaction acceptance. Herdr tests must use an isolated home/core rather than mutating the user's live core.
- Use `mise run dev` / `mise run dev-stop` for development. They select an isolated home, bundle and process records by canonical checkout + Git branch; `mise run run` is the same start entry. `dev-stop -- --branch NAME` targets a previous branch and `dev-stop -- --all` is limited to this checkout. Explicit `CANBAN_HOME` / `--home` overrides branch selection; development commands refuse the installed `~/.canban` and unrecorded services. Socket, lock and data follow the selected home. `CANBAN_ENDPOINT` is attach-only, not isolation. Shared Herdr Core is acceptable for ordinary development for now; acceptance must still use an isolated Core. See [development instances](docs/development-startup.md) and [remaining isolation gaps](docs/instance-isolation.md).
- Canonical machine inventory is ~/Workspace/Machines. Keep machine-specific connection, environment, and deployment knowledge there.
