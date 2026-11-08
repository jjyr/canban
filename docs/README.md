# Canban product design and runtime research

Canban is the product; Human Context names the proposed approach. Current user directions center on native Rust/NSH UI, directory-backed Projects, local Boards/cards, stage-triggered workflows, tag/default automatic scheduling, and a unified card timeline. AppServer is a logical component co-hosted in the desktop process for MVP; WebView is embedded only as needed. Documentation is English; proposals and unresolved decisions are identified.

The repository now has a runnable macOS MVP. [Latest acceptance](mvp-acceptance.md) covers the command-defined profile update, real Codex human-input/rework tasks, explicit Review commit/merge, and Terminal recovery checks. Start with [implementation, development and acceptance](implementation.md) for actual behavior, commands and validation limits. Dated pre-implementation statements in the design/research documents describe their original scope; they are not the current implementation status.

[Real Agent workflow and cross-Board acceptance](real-agent-acceptance.md) records the two-Board, four-Codex-Session Develop/Review/Archive run and the exact concurrency/isolation limits.

## Current product documents

| Document | Purpose |
| --- | --- |
| [Vision and scope](product/vision.md) | Current user requirements and MVP boundary |
| [Product discussion and decisions](product/product-log.md) | Rationale, superseded proposals, and open questions |
| [Projects and workflow templates](product/projects-and-workflows.md) | Directory/Board ownership, required Backlog, callbacks, customization, and template migration |
| [Kanban card workflow](product/kanban-workflow.md) | Numbers, resources, stages/run/outcome separation, comments, mentions, and archive |
| [Notes and the Kanban workspace](product/notes-and-workspace.md) | SQLite-backed file-like Notes, related-event Timeline, bidirectional links, Agent read/write and ASCII workspace layouts |
| [Notes/workspace implementation](notes-and-workspace-implementation.md) | Implemented document operations, Agent CLI, UI composition and acceptance limits |
| [Workspace and scheduling implementation](workspace-and-scheduling.md) | Named branches, remote/base synchronization, global Agent limit, priority and human approval gates |
| [Scheduler](product/scheduler.md) | Stage determines work; tags select agents; admission determines timing |
| [Agent profile configuration](product/agent-profiles.md) | Implemented registry, role instructions, stage references, routing and JSON examples |
| [Agent CLI and extensibility](product/agent-extensibility.md) | Current CLI surface, multi-Agent ownership and proposed command-defined lifecycle presets |
| [Agent interaction](product/agent-interaction.md) | Interactive terminal execution, completion/exit, resume and acceptance limits |
| [One Session model](product/sessions.md) | Card/profile conversations, request/attempt lifetimes and live replies through one execution model |
| [Unified timeline](product/timeline.md) | Proposed event envelope, event catalog, comment revisions, attachments, ordering, and recovery |
| [Agent-callable interface](product/agent-interface.md) | Proposed application operations shared with the desktop |
| [MVP sequence](mvp-plan.md) | Scope, implementation proposals, decision gates, and acceptance matrix |
| [MVP scope review](product/mvp-scope-review.md) | User resolutions and the simplified first-release boundary |
| [Review and code delivery](product/review-delivery.md) | Basic Diff, return-to-Develop, explicit commit/merge, conflict and receipt behavior |
| [MVP progress and next steps](product/mvp-next-steps.md) | Current acceptance boundaries and recommended daily-use, review and code-delivery priorities |
| [Agent lifecycle implementation](agent-overview-and-lifecycle.md) | Status-aware global budget, idle reclamation, global Overview and isolated/native validation |
| [Dedicated Terminal workspace](terminal-workspace.md) | Independent Terminals, live directory context, Project grouping, split chrome, Agent resume and isolated PTY acceptance |
| [Project entry, grouped Overview and Settings](workspace-entry-and-settings.md) | Clickable empty Board, native directory chooser, filter-free Overview and unified Settings groups |
| [Compact navigation and Quick Open](quick-open-and-navigation.md) | Implemented icon rail, Command+O fuzzy search, candidate navigation and native acceptance |
| [Herdr UI layout and product references](research/herdr-ui-product-and-layout.md) | Source-backed recommendations for workspace chrome, navigation, Quick Open, contextual feedback and Review |
| [Global Agent Overview](product/overview.md) | Herdr UI reference, all-Agent grouped overview, attention highlight, queued/idle/closed states and context inspector |
| [Attention handling](product/needs-attention.md) | Task/runtime naming, contextual handling inside Overview, Card/Terminal navigation and resolution rules |
| [Events and notifications](product/events-and-notifications.md) | Rust enum/channel/topic direction, needs-attention state, internal notifications; detailed architecture deferred |

Current default template: Backlog / Develop / Review. Only Backlog is mandatory. Archive retains completed work outside the active Board. Plan/Process are earlier generic planning/execution terms, not additional required columns. Earlier Space-based organization and Ready/In Progress/Done column recommendations have been superseded.

## Architecture and integration

Start with [MVP interface contracts](interfaces/mvp-contract.md) for the proposed concrete reporting/call mechanism, then use the architecture/source references below for ownership and feasibility.

1. [Proposed Canban architecture](architecture.md): native UI, logical/co-hosted AppServer, storage, callbacks, runtime binding, and future daemon extraction.
   [Internal architecture comparison](architecture-internal-options.md) compares module/call organizations; [crates and timers](architecture-crates-and-timers.md) records the user-requested crate split, AppServer/Workflow Engine/Scheduler names, and card timer placement.
   [Data architecture](data-architecture.md) records ~/.canban/, SQLite with WAL, file-backed execution context/reports, and zstd/lz4-compressed Card archives; detailed schema and archive mechanisms remain proposals.
   [UI architecture and design system](ui-architecture.md) records the approved NSH reuse, separate UI Kit, component contracts, native gallery, and regression workflow.
   [App UI review and design directions](ui-direction-review.md) audits the current native UI and proposes visual styles, workspace layouts and component priorities for discussion.
2. [herdr-core integration](research/herdr-core-integration.md): installed service versus app-managed service, embedding, and Herdr app linkage.
3. [Herdr product concepts](research/herdr-core-concepts.md): existing Space/Tab/Pane versus core Terminal/Session, separate from Canban's Project model.
4. [herdr-core architecture](research/herdr-core-architecture.md): source modules, holders/processes, protocol, status, persistence, and recovery.
5. [Herdr identifiers](research/herdr-identifiers.md): reusable runtime identity, its lifetime limits, and a minimal logical execution key.
6. [CLI, RPC, and MCP](research/agent-interface-options.md): Herdr's actual pattern, agent-facing adapter comparison, and the single shared operation boundary.
7. [Interactive Agent lifecycle](research/interactive-agent-lifecycle.md): provider conversation IDs, real terminal attachment, graceful exit/resume, renderer options and acceptance gates.
8. [Native libghostty terminal](research/native-terminal.md): VT library versus native renderer, NSH host, Herdr stream ownership and source reuse.
9. [Active @question replies](research/agent-reply-options.md): explicit CLI replies versus Herdr terminal observation, association limits and the proposed combined boundary.

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
