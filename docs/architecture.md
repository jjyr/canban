# Canban architecture: native UI and a co-hosted application service

Status: current direction discussed with the user, 2026-11-05. Rust/NSH main UI and optional WebView embedding are user-selected. The AppServer is logically independent; MVP co-hosts it with the desktop rather than requiring a separately deployed Canban daemon. Naming and crate boundaries were refined on 2026-11-06: AppServer, Workflow Engine, and Scheduler. Detailed runtime/thread wiring and agent adapter selection remain implementation proposals.

Implementation update, 2026-11-06: a native MVP now exists with same-process AppServer, a CLI/headless host, installed-hc JSON-RPC integration and SQLite/file persistence. See [current implementation and acceptance](implementation.md); broader proposals below retain their original design scope.

## Overall assessment

The product responsibility boundaries are clear enough for focused MVP implementation planning: Canban owns Project/Board/card/context/history/workflow; herdr-core owns terminal execution. The architecture should now converge on one application state owner, one shared operation surface, and explicit UI/service/runtime boundaries.

Earlier recommendations that made a WebView the primary work surface or a separate Canban service process the default are superseded by this direction.

## MVP process and module model

```text
+--------------------------------------------------------------------+
| Rust + NSH UI: Board / Card / Timeline / Notes                     |
|        | typed Rust commands/queries                               |
|        v                                                           |
| AppServer <---------------- local IPC adapter <--- CLI             |
| shared operations + composition + lifecycle                        |
|        |                                                           |
|        +-- Kanban / Context / Timeline / Notifications crates      |
|        +-- Workflow Engine / Scheduler crates                      |
|        +-- small event enum + in-memory subscriptions              |
|        +-- Storage / Workspaces / Herdr adapter crates             |
|                                                                    |
| Optional embedded WebView for a particular surface                 |
| UI-local focus / navigation / selection / draft presentation       |
+--------------------------------------------------------------------+
                         |
                         | hc connect / core protocol
                         v
                herdr-core service (separate)
                         |
                 holder -> PTY -> agent
```

The user requested separate Rust crates for the major components. The diagram still represents one Canban process for MVP; crate boundaries do not require separate actors, threads, or processes. See [crates and timers](architecture-crates-and-timers.md) for the proposed workspace and acyclic dependency direction. The CLI is a short-lived caller; it need not be another scheduler or long-running Canban service. Herdr core and its holders remain separate processes supplied by Herdr.

Native UI invokes the same typed Rust application operations used by the IPC adapter; it does not need a loopback RPC connection to reach its own service. CLI, direct RPC, and a possible MCP adapter are entry adapters over those operations, not separate product engines. See [interface research](research/agent-interface-options.md).

## What AppServer means

AppServer is the Rust application entry/composition component hosting the feature crates. The corresponding crates own their domain decisions/state; AppServer wires them and routes shared operations. It has no dependency on NSH views, component handles, AppKit, WebView, or a particular CLI/MCP host.

| Responsibility | Owner |
| --- | --- |
| Shared command/query/report entry, crate composition and lifecycle | AppServer |
| Projects/Boards/cards/numbers/stage/attention/timing attributes | Kanban |
| Notes/resources/attachments/captured input | Context |
| JSON template interpretation and workflow progression | Workflow Engine |
| Routing, queue/time readiness, generic execution lifecycle | Scheduler |
| Durable card history/comments/reports | Timeline |
| Internal notifications and later native delivery | Notifications |
| Focus/navigation/selection/transient edit presentation | NSH UI |
| Runtime terminal process/output/status | herdr-core through the Herdr adapter |
| Directory/Git/worktree preparation and Archive handling | Workspaces |
| Shared local IO boundary | Storage |
| Agent-facing syntax or protocol envelope | Thin CLI/IPC/MCP adapters |

The UI sends intent and displays projections. It does not directly update the database, independently choose an agent, create worktrees, or start a dispatch loop whenever a view mounts. A view can own a transient draft without becoming another authority for the saved card or execution state.

Use dedicated crates for the main AppServer, Kanban, Workflow Engine, Scheduler, and UI responsibilities, with supporting context/history/notification/storage/runtime crates as proposed in the [crate plan](architecture-crates-and-timers.md). AppServer wires these components; business ownership stays in its respective crate. No public extension framework or separate process per crate is required.

## Shared operations and minimal events

The [internal architecture comparison](architecture-internal-options.md) contrasts an explicit coordinator, modular event-driven reactions, and component actors. The recommended logical structure is module ownership with lightweight event-driven reactions; this remains a proposal for discussion, not a requirement to implement an actor/event framework before MVP.

```text
UI intent / CLI command / agent report
                  |
           shared application operation
                  |
       product update + recorded history
                  |
           minimal application event
           /        |          \
     UI projection  workflow   notification
                    Scheduler
```

The service owns mutation policy and storage, while interested components receive relevant enum events over an in-memory channel/topics. A subscription is not permission for every consumer to independently mutate the same card or create the same dispatch. Use a clear owner for each effect.

This records module ownership, not a completed event framework. Exact enum variants, channel crate, topic taxonomy, subscription lifecycle, and delivery/replay guarantees remain deferred as requested. The [timeline draft](product/timeline.md) describes persisted history; it is not a mandatory second event-bus schema.

## Rust/NSH UI and background work

The selected main surfaces—Board, card details, Timeline, and Notes—are written with Rust/NSH native components. WebView is an optional embedded capability for a resource or specialized surface, not the default host of the workbench.

The pinned NSH source has a Rust-only gallery mounted through AppSource::Rust with no TSX/QuickJS requirement. Its macOS backend runs an engine thread and applies native view changes on the main thread. Rust components can use asynchronous tasks, background, and blocking helpers. [Rust gallery](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/examples/gallery-rs/README.md), [native entry](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/examples/gallery-rs/native/src/main.rs), [macOS backend](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/macos.md), [background helper](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/crates/nsh-core/src/element/ambient.rs#L1013-L1024).

Recommended application rule: filesystem, database, Git, core transport, and other potentially blocking work run outside NSH render/input paths. The service returns state/results through a controlled asynchronous Rust boundary. NSH-local code applies projection updates in its supported context; do not mutate view-owned signals arbitrarily from a worker.

Native platform/NSH thread constraints are supplied by the framework. The exact AppHandle/subscription wiring must be validated in the native spike; no working Rust/NSH prototype was built in this research.

Optional WebViews receive a narrow resource/component bridge to the same service. Do not grant preview content direct core/socket/database authority. Native drag/drop, IME, selection, Markdown/rich editing, list virtualization, and terminal presentation need real interaction acceptance; Rust/native does not itself prove those product behaviors.

The user-approved [UI architecture](ui-architecture.md) separates canban-ui-kit for tokens/styled reusable controls over NSH from reusable product components and presenters in canban-ui, with a native gallery and component-level regression workflow. The first MVP implements these boundaries; [implementation and acceptance](implementation.md) records the verified subset and remaining limits.

## Storage and identity

Use a Canban-owned SQLite store with WAL and managed content files, with a service-owned write path. Recommend one shared database across Projects. Keep card history independently of bounded core terminal retention. Core's opaque documents can carry small optional metadata, but do not replace Canban's product model or overwrite the herdr-app layout document.

The user selected ~/.canban/ as the configuration and managed-data home, SQLite with WAL, file-backed execution content, and zstd/lz4 compression of archived Cards including related Agent execution data/results. The [data architecture](data-architecture.md) proposes keeping archived Card lookup records in SQLite while moving detailed history/content into compressed packages. File publication precedes committing references; state/history writes commit before emitting in-memory events. Detailed layout, schema, and archive mechanisms remain proposals.

The revised file layout groups execution content under projects/<project-id>/cards/<card-id>/runs/<request-id>/<attempt-number>/, making Card ownership visible alongside the explicit database associations. Card archive packaging combines this subtree with exported Card records and required shared resources.

```text
Project -> directory + Board -> Card #42
                                |
                         context + timeline
                                |
                        dispatch_request_id
                                |
                         attempt_number
                                |
                  one logical Session / execution
                                |
           runtime binding: (owning core.id, terminal.address)
```

The [ID report](research/herdr-identifiers.md) recommends using the existing request plus attempt as logical execution identity, optionally serialized as run_id; no separate Session/Run/execution UUIDs are needed for the same attempt. Runtime labels aid association; the core tuple is a binding, not a universal provider conversation ID.

A new submitted @mention or executable-stage entry creates a fresh conversation through the same Session model. Reconnect refers to the previous execution. Context continuity comes from the card/Notes/results, not implicit reuse of an agent conversation.

## Timer placement

Persist card timing attributes through the Kanban crate. Put due-time waiting/trigger readiness in the Scheduler crate, initially a timers module. Workflow Engine applies configured workflow actions when the timer fires; a reminder can instead produce an internal notification. Timer/date/countdown widgets belong to native UI and edit configuration through AppServer. See [crates and timers](architecture-crates-and-timers.md); this placement does not add recurrence/cron or always-on scheduling to MVP.

## Product execution policy

1. A stage entry or submitted @mention reaches the shared application operation and produces the relevant product event/request.
2. Scheduler selects the workflow/profile/context and launches eligible work without concurrency caps.
3. Preparation creates/reuses the directory/worktree when applicable, retained across Develop/Review/rework until Archive.
4. The runtime adapter launches the existing runnable agent setup through core, with report association/instructions.
5. The agent explicitly declares completed, blocked, or other supported outcome through the shared report entry.
6. Completed invokes that request's configured callback. Default development completion enters Review and runs the ordinary report task.
7. Blockage marks the card needs attention and produces an internal notification. Native macOS delivery is later.

The user deliberately excludes independent quality/test/process-exit gates and exceptional old-execution coordination from MVP. One generic Session implementation serves every trigger. JSON templates, ordinary reports, and basic operation/replay association are enough; no writer-drain or elaborate resource-lease framework is required.

## Extracting a daemon later

```text
MVP:
  NSH UI -- typed Rust handle --> AppServer
                                same desktop process
  CLI -------- local IPC ------> same operations

Later, if needed:
  NSH UI ------- IPC ----------> Canban daemon
  CLI ---------- IPC ----------> AppServer + same product modules
  MCP adapter -- IPC ----------> optional equivalent entry
                                      |
                                   herdr-core
```

The service library can be reused in a daemon host because its operations/state do not depend on NSH. Replace the UI's local handle with a remote adapter to the same logical contract. Do not make the native UI use wire RPC now solely to anticipate this future.

Extraction is not zero work: service startup/ownership, shutdown, endpoint discovery, subscriptions/snapshots, error/reconnect behavior, and data migration/version compatibility need process-boundary acceptance. It should reuse business logic rather than rewrite workflow decisions.

## Window close versus application quit

Co-hosting does not require destroying AppServer whenever one window closes. The application process may remain resident; the exact close/quit policy must be stated when implementing lifecycle.

If Canban quits, its in-memory subscriptions, report endpoint, and new workflow admission stop unless a daemon exists. Herdr-owned terminals can still run. Their survival does not guarantee that Canban callbacks or reports are processed while Canban is absent.

Do not infer an always-on promise from either the core's holder design or the logical server component. A separate daemon becomes justified by an explicit requirement to keep the Canban coordinator available after application quit.

## What remains to prove

The architecture boundaries are now sufficiently clear for a small vertical slice: native card interaction -> shared operation/history -> automatic core launch -> agent report -> normal Review/report -> attention/notification.

Verify native UI/state-update integration, existing-agent startup and reporting, persistence/restart, optional terminal takeover, and the chosen application lifetime. The [MVP interface contract](interfaces/mvp-contract.md) and [CLI/RPC/MCP research](research/agent-interface-options.md) specify proposed boundaries; neither is implemented acceptance evidence. No app implementation or runtime validation was performed in this documentation work.
