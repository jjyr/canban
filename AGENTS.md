# Project guide

- Read [docs/README.md](docs/README.md) for current product and architecture directions.
- Keep project documentation in English.
- Follow [development conventions](docs/development-conventions.md).
- Cargo package names use canban-xxx; crate directories use crates/xxx and source/module filenames omit the canban- prefix.
- Canban uses Apache-2.0. Future Cargo workspace packages inherit that license. Preserve applicable third-party licenses/notices when importing code or assets.
- Original Canban copyright attribution is JJy <jjyruby@gmail.com>, starting in 2026. Preserve third-party attribution separately.
- Keep the main components in separate crates: AppServer, Kanban, Workflow Engine, Scheduler, UI, and the supporting boundaries documented in docs.
- This is currently a documentation/design repository. Do not describe planned interfaces or crates as implemented.
- Canonical machine inventory is ~/Workspace/Machines. Keep machine-specific connection, environment, and deployment knowledge there.
