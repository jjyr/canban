# Canban

Canban is a proposed desktop workbench for **Human Context**: preserving a person's intent, decisions, working context, and review history across agent execution. Kanban is the management center; Scheduler turns explicit requests and configured workflow rules into agent runs through herdr-core.

The current repository contains product and architecture research. There is no application implementation yet. The current product model is Project -> directory + Board -> cards, with automatic stage workflows and tag/default agent routing. Comments, resources, state changes, and executions share each card's timeline.

Start with the [documentation guide](docs/README.md), particularly the [herdr-core integration recommendation](docs/research/herdr-core-integration.md) and the separate [product concepts explanation](docs/research/herdr-core-concepts.md).

The proposed MVP centers on modular Kanban and Scheduler, using Rust/NSH for the main native UI and embedding WebView only where needed. The logical AppServer is co-hosted in the desktop process for MVP, with a boundary that can support a daemon host later. Advanced Shear review integration is deferred.

Cargo package names use canban-xxx; crate directories and source filenames omit that prefix. See [development conventions](docs/development-conventions.md).

Copyright 2026 JJy <jjyruby@gmail.com>.

Licensed under the [Apache License, Version 2.0](LICENSE).
