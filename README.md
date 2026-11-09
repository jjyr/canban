# Canban

Canban is a native Rust/NSH desktop workbench for **Human Context**: preserving intent, decisions, working context, and review history across Agent execution. Kanban is the management center; Scheduler turns stage changes and mentions into Agent runs through herdr-core.

The first macOS MVP implements local Projects/Boards/Cards, JSON workflows and routing, Card/profile Agent conversations with exact-ID resume, native libghostty terminals, explicit CLI outcome reporting, database Notes with revisions/links/Timeline, Board/Card and focused work layouts, context selection, resources/image paste, Timeline, attention notices, and compressed archives. See [implementation and acceptance](docs/implementation.md) for the exact supported behavior and remaining limits.

## Run

Prerequisites: macOS, Xcode command-line tools, mise (pinned Rust and Zig), and an installed compatible hc. Agent profiles use your existing executables and authentication.

```sh
mise run build
mise run run
```

The local development bundle is .local/Canban.app. Configuration and managed data default to ~/.canban/. Set CANBAN_HOME to an absolute alternate directory for an isolated instance. Only one AppServer may own a home at a time.

```sh
mise run gallery
mise run test
mise run acceptance
mise run acceptance-interactive
```

The opt-in acceptance task uses a disposable directory and isolated Herdr core with a deterministic fixture Agent; it does not use a paid model or modify the user's existing core. Real Codex reporting was also validated separately. The [development guide](docs/implementation.md) covers configuration, CLI operations, runtime lifetime, and reproducible verification.

For an explicit real-model check, mise run acceptance-real runs actual Codex development and review on two disposable Git Boards, verifies overlapping execution and isolation, then archives both Cards. See [real acceptance evidence](docs/real-agent-acceptance.md). This uses your installed provider configuration; it is not part of routine unit/fixture checks.

Start with the [documentation guide](docs/README.md), particularly the [herdr-core integration recommendation](docs/research/herdr-core-integration.md) and the separate [product concepts explanation](docs/research/herdr-core-concepts.md).

The MVP uses separate crates for Kanban, Workflow Engine, Scheduler, storage/context/history, Herdr, AppServer, UI Kit and product UI. AppServer runs independently of UI windows; the desktop starts or attaches to it, and `canban serve` can host it directly. Project execution can use local or remote Core machines, preserving original Resume and Review workspaces. Main surfaces are native; advanced Shear review integration is deferred. See [multi-machine behavior and acceptance](docs/multi-machine.md).

Cargo package names use canban-xxx; crate directories and source filenames omit that prefix. See [development conventions](docs/development-conventions.md).

Copyright 2026 JJy <jjyruby@gmail.com>.

Licensed under the [Apache License, Version 2.0](LICENSE).

For interactive real-Codex acceptance, run mise run acceptance-real-interactive. It verifies active @ replies, separate Developer/Reviewer histories, normal exit, exact-ID resume and archived replies. Folder trust/permission prompts still require normal interactive handling. Ghostty source is checksum-pinned by the terminal-deps task; bundled third-party notices are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

UI component development uses the standalone [Gallery example](examples/README.md) (`mise run gallery`); it is separate from the production app.

## Feedback

Use **Report issue** in the App to open the Bug report form with your environment filled in. You can also [report a bug](https://github.com/jjyr/canban/issues/new?template=bug_report.yml) or [suggest a feature](https://github.com/jjyr/canban/issues/new?template=feature_request.yml) directly on GitHub. Review the form and submit it there.
