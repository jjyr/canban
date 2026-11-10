# Canban product design and runtime research

Canban is the product; Human Context names the proposed approach. Current user directions center on native Rust/NSH UI, directory-backed Projects, local Boards/cards, stage-triggered workflows, stage-profile/default automatic scheduling, and a unified card timeline. AppServer is an independent service shared by UI clients and local/remote executions; WebView is embedded only as needed. Documentation is English; proposals and unresolved decisions are identified.

The repository now has a runnable macOS MVP. [Real Agent acceptance, 2026-11-09](real-agent-acceptance-20261109.md) covers the current Codex workflow, native post-Archive Resume repair and Claude provider-authentication boundary. [Latest acceptance](mvp-acceptance.md) covers the command-defined profile update, real Codex human-input/rework tasks, explicit Review commit/merge, and Terminal recovery checks. Start with [implementation, development and acceptance](implementation.md) for actual behavior, commands and validation limits. Dated pre-implementation statements in the design/research documents describe their original scope; they are not the current implementation status.

[Real Agent workflow and cross-Board acceptance](real-agent-acceptance.md) records the two-Board, four-Codex-Session Develop/Review/Archive run and the exact concurrency/isolation limits.

## Reading and maintaining the current contract

Documentation has three roles:

- **Current contract:** feature documents describe the accepted behavior and ownership. Start with [implementation](implementation.md), then follow the feature references below. [Development conventions](development-conventions.md) and [UI architecture](ui-architecture.md) describe cross-cutting rules.
- **Decision history and proposals:** [product-log](product/product-log.md), dated research and explicitly marked proposals explain alternatives and rationale. They do not override the current feature contract. When a decision changes, rewrite the affected current section and retain the old rationale as clearly dated history.
- **Acceptance evidence:** dated verification sections/reports record the build, environment, exercised actions and limits. Keep earlier results intact and mark superseded behavior; historical evidence is not a fresh verification of today's code.

The latest explicit user decision takes precedence over earlier discussion. Before changing an existing feature, check its current contract and any later superseding decision. If code and documents disagree, record the discrepancy rather than declaring a proposal implemented. Use the current code/shared types for exact RPC payloads. Documentation-only reconciliation does not constitute new runtime acceptance.

## Current product documents

| Document | Purpose |
| --- | --- |
| [Vision and scope](product/vision.md) | Current user requirements and MVP boundary |
| [Multi-machine execution](multi-machine.md) | Independent AppServer, mutable Project work location, remote Agent CLI and native Terminal routing |
| [Multi-machine acceptance](multi-machine-acceptance.md) | Isolated local/remote verification, limits and reproduction |
| [Product discussion and decisions](product/product-log.md) | Rationale, superseded proposals, and open questions |
| [Projects and workflow templates](product/projects-and-workflows.md) | Directory/Board ownership, required Backlog, callbacks, customization, and template migration |
| [Board dragging and placement](board-interaction.md) | Whole-Card dragging, animated placeholders/peers, durable ordering and workflow boundary |
| [Kanban card workflow](product/kanban-workflow.md) | Numbers, resources, stages/run/outcome separation, comments, mentions, and archive |
| [GitHub integration implementation](github-integration.md) | Registered App, provider/Kanban boundaries, Issue intake, native creation, PR delivery, API and explicit live-acceptance limits |
| [GitHub Cards and unified creation](product/github-cards.md) | Product decisions, automatic Issue intake, local removal and collaborator API research |
| [GitHub App and device authorization](product/github-authentication.md) | Canban-owned App, Device Flow, installation access and secret-free token refresh |
| [Notes deferred](notes-deferred.md) | Notes removed from current product scope; retained data, historical evidence and compatibility boundaries |
| [Workspace and scheduling implementation](workspace-and-scheduling.md) | Named branches, remote/base synchronization, global Agent limit, priority and human approval gates |
| [Scheduler](product/scheduler.md) | Stage determines work; profiles select agents; admission determines timing |
| [Agent profile configuration](product/agent-profiles.md) | Implemented registry, role instructions, stage references, routing and JSON examples |
| [Agent CLI and extensibility](product/agent-extensibility.md) | CLI capabilities and integration boundaries; use Agent profile configuration for the current command/preset schema |
| [Agent interaction](product/agent-interaction.md) | Interactive terminal execution, completion/exit, resume and acceptance limits |
| [One Session model](product/sessions.md) | Card/profile conversations, request/attempt lifetimes and live replies through one execution model |
| [Unified timeline](product/timeline.md) | Proposed event envelope, event catalog, comment revisions, attachments, ordering, and recovery |
| [Agent-callable interface](product/agent-interface.md) | Proposed application operations shared with the desktop |
| [MVP sequence](mvp-plan.md) | Scope, implementation proposals, decision gates, and acceptance matrix |
| [MVP scope review](product/mvp-scope-review.md) | User resolutions and the simplified first-release boundary |
| [Review and code delivery](product/review-delivery.md) | Basic Diff, return-to-Develop, explicit commit/merge, conflict and receipt behavior |
| [MVP progress and next steps](product/mvp-next-steps.md) | Current acceptance boundaries and recommended daily-use, review and code-delivery priorities |
| [Event-driven runtime observation](runtime-observation.md) | Subscription, connection recovery, bounded discovery, Git caches and exit classification |
| [Runtime observation cost](runtime-observation-cost.md) | Historical polling baseline and prototype; superseded by event-driven runtime observation |
| [Agent lifecycle implementation](agent-overview-and-lifecycle.md) | Status-aware global budget, idle reclamation, global Overview and isolated/native validation |
| [Terminal interaction and navigation](terminal-interaction.md) | Delayed idle exit, live inactive filtering, input pipeline, global Back/Forward and current acceptance |
| [Dedicated Terminal workspace](terminal-workspace.md) | Independent Terminals, live directory context, Project grouping, split chrome, Agent resume and isolated PTY acceptance |
| [Project entry, grouped Overview and Settings](workspace-entry-and-settings.md) | Clickable empty Board, native directory chooser, filter-free Overview and unified Settings groups |
| [Compact navigation and Quick Open](quick-open-and-navigation.md) | Implemented icon rail, Command+O fuzzy search, candidate navigation and native acceptance |
| [Card conversation workspace](card-conversation.md) | Compact Card header, scrolling task/Timeline, explicit recipients, comment shortcuts and Terminal boundary |
| [Native UI motion](ui-motion.md) | Shared motion policy, retained Card reveal, editor resizing, native modal fades and regression scenes |
| [Desktop utilities](desktop-utilities.md) | Implemented Usage/reset countdown, Caffeine, Theme, Shortcuts, target-Project creation and acceptance |
| [Development startup](development-startup.md) | Matched UI/AppServer development launches, API compatibility checks, existing-data reuse and live-PTY acceptance |
| [Herdr UI layout and product references](research/herdr-ui-product-and-layout.md) | Source-backed recommendations for workspace chrome, navigation, Quick Open, contextual feedback and Review |
| [Global Agent Overview](product/overview.md) | Herdr UI reference, all-Agent grouped overview, attention highlight, queued/idle/closed states and context inspector |
| [Attention handling](product/needs-attention.md) | Task/runtime naming, contextual handling inside Overview, Card/Terminal navigation and resolution rules |
| [Events and notifications](product/events-and-notifications.md) | Rust enum/channel/topic direction, needs-attention state, internal notifications; detailed architecture deferred |

Current default template: Backlog / Develop / Review. Only Backlog is mandatory. Archive retains completed work outside the active Board. Plan/Process are earlier generic planning/execution terms, not additional required columns. Earlier Space-based organization and Ready/In Progress/Done column recommendations have been superseded.

## Architecture and integration

Start with [MVP interface contracts](interfaces/mvp-contract.md) for the proposed concrete reporting/call mechanism, then use the architecture/source references below for ownership and feasibility.

1. [Canban architecture](architecture.md): native UI clients, independent AppServer, storage, callbacks and local/remote runtime bindings.
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
10. [Multi-machine execution](research/multi-machine.md): verified Core machine routing/recovery, current Canban gaps and a proposed first remote scope.

Core executes terminals; it has no Canban Project, Board, Card, workflow, or scheduler. Space belongs to herdr-app. Canban's Project is its own directory-backed product object and need not synchronize with a Herdr Space.

## Supporting research

Project-wide [development conventions](development-conventions.md) specify prefixed Cargo package names, unprefixed crate directories/source filenames, and Apache-2.0 licensing.

| Document | Purpose |
| --- | --- |
| [NSH, WebView, and Shear](research/nsh-webview-and-shear.md) | UI composition, terminal-client obligations, reusable analysis, review-port cost |
| [Desktop utilities and cross-Project creation](research/herdr-desktop-utilities.md) | Source-backed Usage, Caffeine, Theme, Shortcuts and New Card target-Project source research |
| [Related products](research/product-landscape.md) | Official-documentation workflow comparison and hypotheses |
| [Sources and verification](research/sources.md) | Pinned source revisions and research verification boundary |

The user's latest MVP decisions are authoritative: agent-reported completion, no residual-process coordination, worktree handling at Archive, ordinary Review reports, JSON templates, existing runnable agent setup, and internal attention/notification events. Detailed event architecture, native macOS notifications, template GUI, and Shear UI come later.

Source facts are tied to inspected revisions. Research examples and proposed API/configuration schemas are not implementation evidence; documents explicitly marked implemented describe the current feature contract. Canban integrates with compatible Herdr Core through its protocol, owning task/context/history and scheduling while Core owns PTYs and observed runtime state.
