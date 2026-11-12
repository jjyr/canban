# Canban architecture

Current implementation: 2026-11-09. Rust/NSH owns the main interface, libghostty
renders native Terminals, and WebView remains available for future embedded
editors. AppServer is an independent service; earlier MVP co-hosting is superseded.

```text
Rust / NSH UI       Agent CLI        other UI clients
     |                  |                  |
     +----------- AppServer RPC -----------+
                         |
              +----------+-----------+
              |      AppServer       |
              | command/query/report |
              | transactions/events  |
              +----------+-----------+
                         |
        +----------------+------------------+
        |                |                  |
     Kanban       Workflow Engine       Scheduler
     Cards        template progression  admission/priority
     Projects     human review gates    global Agent budget
        |                |                  |
        +----------------+------------------+
                         |
       Context / Timeline / Notifications
       Workspaces / Review / Profiles / Storage
                         |
                 Herdr protocol adapter
                         |
           gateway Core -> local/remote owning Cores
                         |
                 terminal programs
```

The AppServer crate composes domain modules and owns their transactional and
runtime boundaries. It has no dependency on NSH, AppKit or UI component handles.
The accepted next [unified engine](product/workflow-engine.md) interprets entries,
steps and decisions independently of Board columns, replacing the current stage
callback progression. It is implemented in the initial slice; see [workflow/event implementation](product/workflow-engine.md) for exact coverage.
Scheduler determines when eligible work starts, using workflow depth and a
configurable global budget. Herdr owns the programs and their runtime status;
Canban owns durable context, task outcomes, human decisions and history.

## Process and persistence boundaries

Desktop startup connects to `canban serve` or starts it as a separate process.
Closing the UI preserves scheduling. A single-owner lock protects the service
home. UI windows poll state through `AppClient`; their native Terminal streams
upgrade a connection to a transparent Core relay. Explicit `CANBAN_ENDPOINT`
can point to an SSH-forwarded socket without putting execution-host credentials
in the UI. See [multi-machine execution](multi-machine.md).

```text
~/.canban/                       remote execution host
  server-id                      ~/.canban/executors/<server-id>/
  app.sock                         bin/canban (thin CLI)
  server.lock / server.log          rpc/ (private request/reply spool)
  config/                          worktrees/<project>/<card>/
  canban.sqlite (WAL)               projects/<project>/cards/<card>/
  projects/<project>/cards/<card>/    runs/<request>/<attempt>/...
    resources/                     provider state remains provider-owned
    runs/<request>/<attempt>/
  archives/                       no remote product database
  worktrees/ (local execution)
```

SQLite keeps structured product records and ordered Timeline metadata. Managed
files keep large captures, reports, attachments and terminal output. Files publish
before database references. Card archives include exported records and managed
files in verified zstd packages. Archive remains separate from code delivery.
Git worktree/branch operations execute where the saved workspace lives.

## Execution identity

A Project has one mutable current machine/directory. Starting admission snapshots
that location; an Attempt also saves its cwd, workspace and runtime/provider
identity. Existing Resume and Review use their saved location. A changed Project
setting affects future work and never silently moves an old result. Machine
aliases are presentation/routes; stored remote Terminal addresses use owner IDs.

```text
Project.current_location -> new queued execution admission
Card + Profile           -> Conversation (provider session)
Request                  -> trigger + frozen profile/task/callback
Attempt                  -> location + workspace + Core/Terminal binding
Report                   -> declared outcome + durable content
human gate               -> explicit disposition before workflow continues
```

A runtime permission wait and a task's reported blocked outcome are different
signals. Both reach global Overview. Neither terminal disappearance nor idle
means task completion. A later runtime done allows configured normal exit only
after a report; remote completion uses root turns instead of comparing host clocks.

## Crates and UI

Main crates remain `canban-app-server`, `canban-kanban`,
`canban-workflow-engine`, `canban-scheduler` and `canban-ui`, with supporting
crates for context, persistence, profiles, workspace/Git, Terminal transport,
Timeline and notifications. Directory names omit the package prefix.

```text
screens / presenters
        |
reusable product components (Machine picker, work location, Card, Timeline...)
        |
canban-ui-kit (tokens and styled controls)
        |
nsh-base -> nsh-core -> macOS platform
```

UI Kit has no domain/server dependency. Native Gallery imports production
components with deterministic fixtures; application acceptance uses an isolated
real Core. Compilation and screenshots alone do not establish interaction
correctness. See [UI architecture](ui-architecture.md) and
[implementation/acceptance](implementation.md).

## Events and timers

Current AppServer topic-change channels invalidate client snapshots, while
Timeline persists local history. The accepted next [event/subscription design](product/events-and-notifications.md)
adds canonical durable facts, scoped selectors, delivery/checkpoint progress and
recovery; channels remain advisory wake-ups. State changes and facts commit
together; consumers persist effects/intents before acknowledgement. No separate
broker or full event-sourced application is required.

Manual/event/column entries use the same Workflow Engine and existing Scheduler.
Workflow-owned moves carry run/step causality so they do not start duplicate
Agents. Agent/manual definitions share validation and a generic definition/run
viewer. See the implementation contract for exact delivered coverage and runtime evidence. Detailed
onboarding/settings Agent conversations and viewer interaction remain open.

Card timing belongs to Kanban; readiness/time waiting belongs to Scheduler;
Workflow Engine applies actions. Timer placement does not imply recurring jobs
or a timer UI are implemented.
