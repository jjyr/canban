# Canban product design and runtime research

Canban is the product; Human Context names the proposed approach. Current user directions center on native Rust/NSH UI, directory-backed Projects, local Boards/cards, stage-triggered workflows, tag/default automatic scheduling, and a unified card timeline. AppServer is a logical component co-hosted in the desktop process for MVP; WebView is embedded only as needed. Documentation is English; proposals and unresolved decisions are identified.

The repository now has a runnable macOS MVP. Start with [implementation, development and acceptance](implementation.md) for actual behavior, commands and validation limits. Dated pre-implementation statements in the design/research documents describe their original scope; they are not the current implementation status.

## Current product documents

| Document | Purpose |
| --- | --- |
| [Vision and scope](product/vision.md) | Current user requirements and MVP boundary |
| [Product discussion and decisions](product/product-log.md) | Rationale, superseded proposals, and open questions |
| [Projects and workflow templates](product/projects-and-workflows.md) | Directory/Board ownership, required Backlog, callbacks, customization, and template migration |
| [Kanban card workflow](product/kanban-workflow.md) | Numbers, resources, stages/run/outcome separation, comments, mentions, and archive |
| [Scheduler](product/scheduler.md) | Stage determines work; tags select agents; admission determines timing |
| [One Session model](product/sessions.md) | Fresh conversations from different triggers; Canban schedules through core with one execution model |
| [Unified timeline](product/timeline.md) | Proposed event envelope, event catalog, comment revisions, attachments, ordering, and recovery |
| [Agent-callable interface](product/agent-interface.md) | Proposed application operations shared with the desktop |
| [MVP sequence](mvp-plan.md) | Scope, implementation proposals, decision gates, and acceptance matrix |
| [MVP scope review](product/mvp-scope-review.md) | User resolutions and the simplified first-release boundary |
| [Events and notifications](product/events-and-notifications.md) | Rust enum/channel/topic direction, needs-attention state, internal notifications; detailed architecture deferred |

Current default template: Backlog / Develop / Review. Only Backlog is mandatory. Archive retains completed work outside the active Board. Plan/Process are earlier generic planning/execution terms, not additional required columns. Earlier Space-based organization and Ready/In Progress/Done column recommendations have been superseded.

## Architecture and integration

Start with [MVP interface contracts](interfaces/mvp-contract.md) for the proposed concrete reporting/call mechanism, then use the architecture/source references below for ownership and feasibility.

1. [Proposed Canban architecture](architecture.md): native UI, logical/co-hosted AppServer, storage, callbacks, runtime binding, and future daemon extraction.
   [Internal architecture comparison](architecture-internal-options.md) compares module/call organizations; [crates and timers](architecture-crates-and-timers.md) records the user-requested crate split, AppServer/Workflow Engine/Scheduler names, and card timer placement.
   [Data architecture](data-architecture.md) records ~/.canban/, SQLite with WAL, file-backed execution context/reports, and zstd/lz4-compressed Card archives; detailed schema and archive mechanisms remain proposals.
   [UI architecture and design system](ui-architecture.md) records the approved NSH reuse, separate UI Kit, component contracts, native gallery, and regression workflow.
2. [herdr-core integration](research/herdr-core-integration.md): installed service versus app-managed service, embedding, and Herdr app linkage.
3. [Herdr product concepts](research/herdr-core-concepts.md): existing Space/Tab/Pane versus core Terminal/Session, separate from Canban's Project model.
4. [herdr-core architecture](research/herdr-core-architecture.md): source modules, holders/processes, protocol, status, persistence, and recovery.
5. [Herdr identifiers](research/herdr-identifiers.md): reusable runtime identity, its lifetime limits, and a minimal logical execution key.
6. [CLI, RPC, and MCP](research/agent-interface-options.md): Herdr's actual pattern, agent-facing adapter comparison, and the single shared operation boundary.

Core executes terminals; it has no Canban Project, Board, Card, workflow, or scheduler. Space belongs to herdr-app. Canban's Project is its own directory-backed product object and need not synchronize with a Herdr Space.

## Supporting research

Project-wide [development conventions](development-conventions.md) specify prefixed Cargo package names, unprefixed crate directories/source filenames, and Apache-2.0 licensing.

| Document | Purpose |
| --- | --- |
| [NSH, WebView, and Shear](research/nsh-webview-and-shear.md) | UI composition, terminal-client obligations, reusable analysis, review-port cost |
| [Related products](research/product-landscape.md) | Official-documentation workflow comparison and hypotheses |
| [Sources and verification](research/sources.md) | Pinned source revisions and research verification boundary |

The user's latest MVP decisions are authoritative: agent-reported completion, no residual-process coordination, worktree handling at Archive, ordinary Review reports, JSON templates, existing runnable agent setup, and internal attention/notification events. Detailed event architecture, native macOS notifications, template GUI, and Shear UI come later.

Source facts are tied to inspected revisions. Product examples and API/configuration schemas are design drafts, not implemented features. The runtime integration recommendation remains a protocol client of compatible installed core, with Canban owning task/context/history and scheduling.
