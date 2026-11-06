# Canban

Canban is a native Rust/NSH desktop workbench for **Human Context**: preserving intent, decisions, working context, and review history across Agent execution. Kanban is the management center; Scheduler turns stage changes and mentions into Agent runs through herdr-core.

The first macOS MVP implements local Projects/Boards/Cards, JSON workflows and routing, fresh Agent Sessions, explicit CLI outcome reporting, Notes/context selection, resources/image paste, Timeline, attention notices, and compressed archives. See [implementation and acceptance](docs/implementation.md) for the exact supported behavior and remaining limits.

## Run

Prerequisites: macOS, Xcode command-line tools, mise, and an installed compatible hc. Agent executables/templates use your existing installation and authentication.

```sh
mise run build
mise run run
```

The local development bundle is .local/Canban.app. Configuration and managed data default to ~/.canban/. Set CANBAN_HOME to an absolute alternate directory for an isolated instance. Only one AppServer may own a home at a time.

```sh
mise run gallery
mise run test
mise run acceptance
```

The opt-in acceptance task uses a disposable directory and isolated Herdr core with a deterministic fixture Agent; it does not use a paid model or modify the user's existing core. Real Codex reporting was also validated separately. The [development guide](docs/implementation.md) covers configuration, CLI operations, runtime lifetime, and reproducible verification.

Start with the [documentation guide](docs/README.md), particularly the [herdr-core integration recommendation](docs/research/herdr-core-integration.md) and the separate [product concepts explanation](docs/research/herdr-core-concepts.md).

The MVP uses separate crates for Kanban, Workflow Engine, Scheduler, storage/context/history, Herdr, AppServer, UI Kit and product UI. The logical AppServer is co-hosted in the desktop process; the CLI can also host it without a UI. Main surfaces are native; advanced Shear review integration is deferred.

Cargo package names use canban-xxx; crate directories and source filenames omit that prefix. See [development conventions](docs/development-conventions.md).

Copyright 2026 JJy <jjyruby@gmail.com>.

Licensed under the [Apache License, Version 2.0](LICENSE).
