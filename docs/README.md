# Canban product design and runtime research

Canban is the product; Human Context names the proposed approach. Current directions center on native Rust/NSH UI, directory-backed Projects, Cards and a unified Timeline. The accepted next architecture uses three Board columns, one workflow engine, durable event subscriptions, Agent-authored/manual workflow definitions and a generic workflow viewer. AppServer is an independent service shared by UI clients and local/remote executions. Documentation is English; accepted directions, implementation and open design are identified separately.

The [Unified workflow engine, Studio and implementation](product/workflow-engine.md) records the 7 core workflow design principles, Project Workflows, SQLite-only original YAML/version history, graph-derived non-destructive Studio automation options, Queue Merge with upstream sync control, per-step Inspectors, optional YAML-declared Board columns, Edit with agent guidance, shared Column trigger hints and durable event subscriptions. Manual edits, Studio option toggles and Agent submissions use one transactional publication path and one interpreter. Onboarding/default Profile setup remains the next product discussion.

The repository now has a runnable macOS MVP. [Real Agent acceptance, 2026-11-09](acceptance/real-agent-acceptance-20261109.md) covers the current Codex workflow, native post-Archive Resume repair and Claude provider-authentication boundary. [Latest acceptance](acceptance/mvp-acceptance.md) covers the command-defined profile update, real Codex human-input/rework tasks, explicit Review commit/merge, and Terminal recovery checks. Start with [implementation, development and acceptance](implementation.md) for actual behavior, commands and validation limits. Dated pre-implementation statements in the design/research documents describe their original scope; they are not the current implementation status.

[Real Agent workflow and cross-Board acceptance](acceptance/real-agent-acceptance.md) records the two-Board, four-Codex-Session Develop/Review/Archive run and the exact concurrency/isolation limits.

## Reading and maintaining the current contract

Documentation has three roles:

- **Current contract:** feature documents describe the accepted behavior and ownership. Start with [implementation](implementation.md), then follow the feature references below. [Development conventions](development-conventions.md) and [UI architecture](ui-architecture.md) describe cross-cutting rules.
- **Decision history and proposals:** [product-log](product/product-log.md), dated research and explicitly marked proposals explain alternatives and rationale. They do not override the current feature contract. When a decision changes, rewrite the affected current section and retain the old rationale as clearly dated history.
- **Acceptance evidence:** dated verification reports in `docs/acceptance/` record the build, environment, exercised actions and limits. Keep earlier results intact and mark superseded behavior; historical evidence is not a fresh verification of today's code.

The latest explicit user decision takes precedence over earlier discussion. Before changing an existing feature, check its current contract and any later superseding decision. If code and documents disagree, record the discrepancy rather than declaring a proposal implemented. Use the current code/shared types for exact RPC payloads. Documentation-only reconciliation does not constitute new runtime acceptance.

## Acceptance reports

- [Combined GitHub/editor/updater acceptance (2026-11-11)](acceptance/merge-integration-20261111.md)
- [Backend activation acceptance (2026-11-11)](acceptance/backend-update-acceptance-20261111.md)
- [Workflow/event-system merge acceptance (2026-11-11)](acceptance/event-system-merge-acceptance-20261111.md)
- [Workflow UI refinement acceptance (2026-11-11)](acceptance/workflow-ui-acceptance-20261111.md)
- [Card conversation refinement acceptance (2026-11-11)](acceptance/card-conversation-acceptance-20261111.md)
- [Live Review and delivery exercise (2026-11-11)](acceptance/review-delivery-dogfood-20261111.md)
- [Column transition dispatch acceptance (2026-11-17)](acceptance/column-transition-acceptance-20261117.md)
- [Large GitHub Board performance acceptance (2026-11-18)](acceptance/board-performance-acceptance-20261118.md)
- [Agent RPC/CLI acceptance (2026-11-17)](acceptance/agent-interface-acceptance-20261117.md)

## Current product documents

| Document | Purpose |
| --- | --- |
| [Vision and scope](product/vision.md) | Current user requirements and MVP boundary |
| [Multi-host daemons and Diagnostics](multi-host.md) | Simultaneous local + remote `canband` connections, Tailscale/SSH discovery, offline snapshot cache, per-host Settings and Diagnostics |
| [Unified client architecture](unified-client.md) | Accepted direction: single `canban-client` layer for Desktop, Mobile, and CLI with `HostConnector`, Cargo feature pruning, and runtime `ClientCapabilities` |
| [Deterministic Simulation Testing (DST)](dst.md) | Turmoil-backed multi-host DST covering all 8 data/client domains (`mise run sim`) and continuous random-seed fuzzing (`mise run random-sim`) |
| [Mobile interaction design](mobile-interaction.md) | Shared mobile UI (iOS/Android), NSH mapping, Tailscale WhoIs auto-auth & single-address pairing, on-demand Tailnet discovery, Terminals and Diff views |
| [Offline editing and synchronization](offline-sync.md) | Proposal: cross-platform offline Card creation/editing/moves/archives, standalone `canban-sync` replay and field-level conflict merge |
| [Cross-platform Terminal portability](terminal-portability.md) | Proposal: shared `canban-terminal` core, A+B rendering split across macOS/Linux/iOS/Android and remote `PtyTransport` |
| [Product discussion and decisions](product/product-log.md) | Rationale, superseded proposals, and open questions |
| [Unified workflow engine, Studio and implementation](product/workflow-engine.md) | Canonical Workflow contract: 7 design principles, Project/Board ownership, default Queue Merge flow, graph-derived Studio options, YAML schema, CLI/RPC operations and workspaces |
| [Board dragging and placement](board-interaction.md) | Whole-Card dragging, animated placeholders/peers, durable ordering and workflow boundary |
| [Kanban card workflow](product/kanban-workflow.md) | Numbers, resources, stages/run/outcome separation, comments, mentions, and archive |
| [GitHub workspace and cache](github-workspace.md) | GitHub page, Issue/PR sources, Board filters, remote Review, pagination and event-refreshed disposable cache |
| [GitHub integration implementation](github-integration.md) | Registered App, provider/Kanban boundaries, Issue intake, native creation, PR delivery, API and explicit live-acceptance limits |
| [GitHub Cards and unified creation](product/github-cards.md) | Product decisions, automatic Issue intake, local removal and collaborator API research |
| [GitHub App and device authorization](product/github-authentication.md) | Canban-owned App, Device Flow, installation access and secret-free token refresh |
| [Workspace and scheduling implementation](workspace-and-scheduling.md) | Named branches, remote/base synchronization, global Agent limit, priority and human approval gates |
| [Scheduler](product/scheduler.md) | Stage determines work; profiles select agents; admission determines timing |
| [Agent profile configuration](product/agent-profiles.md) | Implemented registry, role instructions, stage references, routing and JSON examples |
| [Agent CLI and extensibility](product/agent-extensibility.md) | CLI capabilities and integration boundaries; use Agent profile configuration for the current command/preset schema |
| [Agent interaction](product/agent-interaction.md) | Interactive terminal execution, completion/exit, resume and acceptance limits |
| [One Session model](product/sessions.md) | Card/profile conversations, request/attempt lifetimes and live replies through one execution model |
| [Unified timeline](product/timeline.md) | Card history presentation, comment revisions, artifacts and ordering; shared event contract links |
| [Agent-callable interface](product/agent-interface.md) | Application operations shared with the desktop |
| [Agent RPC and CLI responses](agent-response-contract.md) | Agent-first defaults, explicit fields/full views, reliable receipts and pagination |
| [Review and code delivery](product/review-delivery.md) | Basic Diff, return-to-Develop, explicit commit/merge, conflict and receipt behavior |
| [Git workbench](git-workbench.md) | Retained branch selectors, checkout/range separation, asynchronous cached reads and Git sync feedback |
| [Event-driven runtime observation](runtime-observation.md) | Subscription, connection recovery, bounded discovery, Git caches and exit classification |
| [Memory profiling and synchronization](memory-profiling.md) | Conditional state synchronization, SQL history reads, UI indexes and Instruments capture |
| [Agent lifecycle implementation](agent-overview-and-lifecycle.md) | Status-aware global budget, idle reclamation, global Overview and isolated/native validation |
| [Terminal interaction and navigation](terminal-interaction.md) | Delayed idle exit, live inactive filtering, input pipeline, global Back/Forward and current acceptance |
| [Dedicated Terminal workspace](terminal-workspace.md) | Independent Terminals, Managed Branch Worktrees, live directory context, Project grouping, split chrome, Agent resume and isolated PTY acceptance |
| [Project entry, grouped Overview and Settings](workspace-entry-and-settings.md) | Clickable empty Board, native directory chooser, historical Overview grouping and unified Settings groups |
| [Compact navigation and Quick Open](quick-open-and-navigation.md) | Unified 42pt chrome, 56pt icon rail, Command+O fuzzy search, full Command Palette (`>`) parity and native acceptance |
| [Archive browser](archive.md) | Global Project/conversation hierarchy, debounced fuzzy search and recovery |
| [Card creation and editing](card-editor.md) | Shared right-pane/expanded Markdown editor, durable drafts, references and keyboard acceptance |
| [Card conversation workspace](card-conversation.md) | Side-by-side Card + Live Terminal workspace, compact Card header, scrolling task/Timeline, explicit recipients and comment shortcuts |
| [Native UI motion](ui-motion.md) | Shared motion policy, retained Card reveal, editor resizing, native modal fades and regression scenes |
| [Desktop utilities](desktop-utilities.md) | Implemented Usage/reset countdown, Caffeine, Theme, Shortcuts, target-Project creation and acceptance |
| [Insights](insights.md) | Daily comparisons, Project / Agent contributions, annual cadence and timing samples |
| [Releases and updates](updates.md) | Manual release dispatch (`mise run release`), push-to-main CI & `ci-gate`, daily release checks, local backend activation, paired migrations and recovery |
| [Development startup](development-startup.md) | Branch-scoped dev/dev-stop, bound homes/bundles, ownership checks, native startup errors and acceptance |
| [Instance isolation and remaining gaps](instance-isolation.md) | Implemented checkout/branch isolation and explicit installed-backend activation and remaining generic compatibility boundaries |
| [Herdr UI layout and product references](research/herdr-ui-product-and-layout.md) | Source-backed recommendations for workspace chrome, navigation, Quick Open, contextual feedback and Review |
| [Global Activity & Notifications](product/overview.md) | Persistent top-right chrome pill & popover (`Command+I`), inline read/merge, 1-step Card+Terminal jump, superseding standalone Overview |
| [Attention handling](product/needs-attention.md) | Task/runtime naming, contextual handling inside Overview, Card/Terminal navigation and resolution rules |
| [Unified Card reminders](reminders.md) | Per-Card unread/pending items, Project counts, durable visible-read positions and local sound delivery |
| [Events, subscriptions and notifications](product/events-and-notifications.md) | Accepted next design: durable facts/delivery, Card/GitHub selectors, recovery, source provenance and notifications |

Keep Backlog / Develop / Review as the default Board columns; planning is a workflow step while the Card remains in Backlog, not a required Design column. Projects that want 4 or more columns (such as Design or QA) can declare `columns` directly in their primary Workflow YAML, which synchronizes the Project's Board stages on publication. Archive retains completed work outside the active Board. Earlier Space organization and required Plan/Process/Ready/In Progress/Done columns are superseded.

## Architecture and integration

Start with [MVP interface contracts](interfaces/mvp-contract.md) for the proposed concrete reporting/call mechanism, then use the architecture/source references below for ownership and feasibility.

1. [Canban architecture](architecture.md): native UI clients, independent AppServer, storage, callbacks and local/remote runtime bindings.
   [crates and timers](architecture-crates-and-timers.md) records the user-requested crate split, AppServer/Workflow Engine/Scheduler names, and card timer placement.
   [Data architecture](data-architecture.md) records ~/.canban/, SQLite with WAL, file-backed execution context/reports, and zstd/lz4-compressed Card archives; detailed schema and archive mechanisms remain proposals.
   [UI architecture and design system](ui-architecture.md) records the approved NSH reuse, separate UI Kit, component contracts, native gallery, and regression workflow.
   [NSH integration notes](nsh-notes.md) tracks version-scoped NSH runtime quirks and Canban workarounds to audit on each NSH upgrade.
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

Preserve Agent-reported completion, no residual-process coordination, worktree handling at Archive, ordinary Review reports and existing execution identities. The 2026-11-11 direction now specifies durable events and a unified workflow engine with Agent/manual authoring and generic display. Native macOS notifications, advanced schedules, a graph editor and Shear UI remain separate/deferred. Project authoring and Edit with agent are selected and implemented; onboarding remains open.

Source facts are tied to inspected revisions. Research examples and proposed API/configuration schemas are not implementation evidence; documents explicitly marked implemented describe the current feature contract. Canban integrates with compatible Herdr Core through its protocol, owning task/context/history and scheduling while Core owns PTYs and observed runtime state.
