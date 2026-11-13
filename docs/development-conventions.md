# Development conventions

Status: user-selected conventions, updated 2026-11-10. Applied by the Rust workspace and macOS MVP; documentation/evidence rules below consolidate the subsequent product discussions.

## Scope and application operations

- Read the current feature contract and later superseding decisions before extending existing behavior. Follow the [documentation roles](README.md#reading-and-maintaining-the-current-contract); keep product documentation in English.
- Implement the confirmed use case with the existing boundaries first. Add abstractions, entities and configuration layers only when a concrete need warrants them. Preserve execution identity, history and explicit human approval while simplifying implementation.
- Buttons, shortcuts, menus and Command Palette entries for one operation dispatch the same intent with the same eligibility/pending checks. **Command catalog parity:** every user-executable UI operation across global chrome, Board, Card, Diff & Review, Terminals, Managed Worktrees, GitHub and Settings must be registered in the searchable Command Palette (`Command+/`, plain `/` outside text editors, or `Command+O` with a `>` prefix) so keyboard search can discover and invoke any action available via pointer controls. AppServer owns durable effects, runtime resolution and receipts; UI controls do not independently choose Agent/Shell process-control paths. Terminal close is the reference example.
- Navigation, list selection and Back/Forward restore context without replaying launch, Resume, retry, merge or approval. An explicitly labelled Start/Resume action may invoke execution; merely inspecting history does not.
- Follow the accepted next [unified workflow](product/workflow-engine.md) and [durable event/subscription](product/events-and-notifications.md) contracts when implementing this extension. Separate column from run/step progress, use one engine, preserve active work during migration, and share Agent/manual definition validation with generic display. These contracts are not current runtime acceptance.
- Keep workflow stage, assignment outcome, runtime status, connection health and human decision state distinct. A process exit or Herdr `done` cannot substitute for an Agent report, and a reply/read acknowledgement cannot approve a human gate. See the [state model](product/sessions.md#independent-state-dimensions).
- Use existing events and shared projections/caches before adding background polling or per-row queries. Define cache invalidation, disconnect recovery and query bounds. The [runtime observation contract](runtime-observation.md) specifies the current subscription, deadlines and on-demand Git checks; it does not prohibit unrelated mechanisms such as GitHub synchronization.

## Cargo package names and filesystem names

Use canban-xxx for Cargo package names. Use the unprefixed xxx for crate directories and ordinary source/module filenames.

```text
crates/
  app-server/        package: canban-app-server
  kanban/            package: canban-kanban
  workflow-engine/   package: canban-workflow-engine
  scheduler/         package: canban-scheduler
  profiles/          package: canban-profiles
  terminal/          package: canban-terminal
  context/           package: canban-context
  timeline/          package: canban-timeline
  notifications/    package: canban-notifications
  herdr/             package: canban-herdr
  workspaces/        package: canban-workspaces
  storage/           package: canban-storage
  types/             package: canban-types
  ui-kit/            package: canban-ui-kit
  ui/                package: canban-ui
  cli/               package: canban-cli
```

For example: crates/scheduler/Cargo.toml declares name = "canban-scheduler". Its source files can be src/lib.rs, src/queue.rs, and src/timers.rs; do not add canban- to those filesystem names. Normal Rust imports use the package-derived underscore form, such as canban_scheduler.

Workspace dependencies use package names as keys and unprefixed paths. Illustrative fragments for the future workspace:

```toml
# Root Cargo.toml
[workspace.package]
license = "Apache-2.0"

[workspace.dependencies]
canban-scheduler = { path = "crates/scheduler" }
```

```toml
# crates/scheduler/Cargo.toml
[package]
name = "canban-scheduler"
license.workspace = true
```

These fragments illustrate conventions now used by the workspace manifests. The Agent CLI binary is canban; the desktop binary is canban-ui.

## UI architecture and component development

Follow the user-approved [UI architecture and design system](ui-architecture.md).

- Keep generic tokens/styled controls and shared interaction conventions in canban-ui-kit. Keep product components, presenters and screen composition in canban-ui. UI Kit has no AppServer, persistence, Herdr or product-domain dependency.
- Reuse suitable NSH controls/behavior before implementing custom interaction. Give independently reusable components their own file/module and a concise typed input/event, state, layout and interaction contract; do not make a crate per widget or copy shared controls into screens.
- Maintain the standalone native Rust/NSH Gallery in `examples/gallery`, importing production components with deterministic fixtures and meaningful composition states. Use `mise run gallery` for UI component checks; keep its entry/fixture data out of the production app.
- Reuse shared tokens and explicit component variants. Extend the shared design contract when a new semantic need arises rather than applying inconsistent screen-specific overrides.
- Fix defects in their owning layer, retain useful reproductions/regression cases and verify affected consumers. Compilation or screenshots alone do not establish native focus, IME, drag or scroll acceptance. Use verification appropriate to the change.
- Build the system through a useful Board -> Card detail -> comment/mention -> Timeline slice, then extend it with subsequent product surfaces. Do not build an exhaustive speculative component library first.

The implementation now includes UI Kit, a native Gallery and an opt-in real-Herdr acceptance harness. See [component inventory](ui-components.md) and [implementation/acceptance](implementation.md) for coverage; further components and tooling remain refinements.

## Verification and documentation updates

For implementation changes, use `mise run build`, `mise run test` and the affected acceptance tasks. UI component changes use `mise run gallery`; application-flow changes need a real packaged native app with an isolated home/Core for acceptance. Use `mise run dev` / `mise run dev-stop` for branch-scoped development; `mise run run` is the same isolated start entry. Explicit homes override branch selection. The manager refuses installed `~/.canban`, foreign process records and unrecorded services. Socket, lock and data follow the home; changing only a socket or bundle ID is insufficient. Ordinary development may share Herdr Core, but acceptance must isolate it. `CANBAN_ENDPOINT` attaches to existing service data. See [development startup](development-startup.md) and [remaining isolation gaps](instance-isolation.md).

Record what was exercised, the environment/build and material omissions. Distinguish deterministic providers over real PTYs from real Codex/Claude runs, Gallery checks from actual App integration, and transport timing from end-to-end input latency. A passing build or a screenshot alone proves neither interaction correctness nor provider readiness. Do not silently count an unavailable/failed check as passed or rerun unrelated suites for a documentation-only correction.

Update current feature prose when behavior changes; do not leave contradictory rules under a newer disclaimer. Keep dated acceptance results and decision rationale identifiable as history, with links to their replacement. Do not rewrite an earlier test count or imply new runtime verification when only documentation changed. Chinese IME checks remain excluded from the current acceptance scope at the user's request.

## License

Canban is licensed under Apache-2.0. The root [LICENSE](../LICENSE) contains the standard Apache License, Version 2.0 text. Use the SPDX identifier Apache-2.0 in Cargo metadata; future workspace packages inherit it with license.workspace = true.

Original Canban copyright attribution: Copyright 2026 JJy <jjyruby@gmail.com>. The project notice appears above the complete, unchanged Apache-2.0 license text; the standard appendix remains part of that text.

Retain existing licenses, copyright notices, and applicable third-party notices when importing external code/assets. Selecting Canban's project license does not replace the original notices of imported code.

## Architecture references

The [crate plan](architecture-crates-and-timers.md) defines proposed responsibility/dependency boundaries, and the [overall architecture](architecture.md) defines the co-hosted AppServer/native UI/runtime relationship. The workspace now implements the core boundaries; timer placement and other deferred capabilities remain design proposals.

## Database, workflow migrations and release updates

Follow [the update/migration contract](updates.md). Schema 8 is the legacy baseline; new migrations require paired up/down steps. Run each complete upgrade or downgrade batch in one transaction, update the schema version in that transaction, and keep migrations database-only. Prefer additive changes; down cannot recover discarded values. Test representative round trips and mid-batch failures. Release installation does not back up the database: a failed up uses transaction rollback, and a failed candidate boot uses reverse-order down. If down fails, stop for manual repair rather than starting an incompatible old version. Explicit or automatic activation of an already installed App has a separate local path: skip equal-schema migration, snapshot before a pending upgrade (including legacy schemas), and restore that snapshot on activation failure; see the update contract.

Workflow YAML documents evolve through their own linear migration registry in `crates/workflow-engine/src/migrations.rs` (`BASELINE..=CURRENT`, stored in `Document.schema`) rather than accumulating legacy compatibility branches inside `source.rs`, `stages.rs`, `definition.rs`, or Studio UI:
- Authoring, compilation, and Studio code always target `migrations::CURRENT` only. Whenever workflow defaults, field names, or structural invariants change in a way that affects stored YAML, increment `migrations::CURRENT` and append a sequential `Migration` step in `crates/workflow-engine/src/migrations.rs`.
- `source::parse` upgrades any supported older `schema` document in memory, `workflow.apply` / `project.create` canonicalize older submitted YAML to `CURRENT`, and `AppServer` startup (`authoring::initialize`) automatically migrates active `WorkflowSource` rows on older schemas by publishing a new `WorkflowRevision`—leaving prior immutable `WorkflowRevision` entries intact for history/audit and preserving the captured `definition_revision` on in-flight `WorkflowRun`s.
- Built-in Agent step prompts are omitted from stored YAML (`with.instruction` is written only when customized by the user) and resolved at compile time, so refinements to built-in default instructions take effect without rewriting user YAML.

Increment `DESKTOP_API` in `crates/updater/src/lib.rs` whenever a change alters the UI ↔ `AppServer` runtime contract — including new or changed RPC methods/parameters, `Snapshot` or event payload shapes, or compiled Workflow YAML capabilities (such as new `uses:` step types, workflow `migrations::CURRENT` bumps, or validation rules). Local unversioned builds (`version: "development"`) compare `DESKTOP_API` when connecting to `~/.canban`; advancing `DESKTOP_API` ensures a newly installed `/Applications/Canban.app` (`mise run install-app`) detects an older running local `AppServer` and automatically upgrades it via `canban_updater::worker::local` rather than silently attaching to a stale daemon.
