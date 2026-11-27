# Project guide

## No new unit tests

- **Do NOT write new unit tests**: Never add new `#[test]` or `#[tokio::test]` unit tests anywhere in the workspace (backend, UI, CLI, or tooling) unless the user explicitly asks for one. Keep existing retained tests passing unless intentionally retired or replaced by DST coverage.

## DST-first & determinism

- **DST-first verification**: Validate all backend, sync, client, scheduler, workflow, protocol, and backend-coupled UI projection behavior through Deterministic Simulation Testing ([docs/dst.md](docs/dst.md), `crates/sim`).
- **Wire new features into DST**: Every new business capability, RPC, state transition, or I/O boundary must be deterministic and simulation-ready (injected `Clock`, `IdGen`, `canban-exec` runner, per-instance `Failpoints` / fault seams, zero process-global mutable statics, zero stray wall-clock calls) and covered in `crates/sim` (scenarios, `chaos` ops, oracles, and coverage gates).
- **Turn bug fixes into DST regressions**: When fixing a bug, reproduce it in DST via a scenario, injectable fault (`Failpoints`, storage/network/fake faults), or oracle invariant, and record any failing chaos seed in `crates/sim/corpus/regressions.txt`.
- **Keep time logic pure**: Express timeouts, backoff, debounce, cooldowns, and grace periods with explicit `now` / `Clock` inputs; never rely on wall-clock sleeps or stray `SystemTime::now()` / `Utc::now()` in core runtime/sync crates ([time-dependent logic](docs/development-conventions.md#time-dependent-logic-and-deterministic-testing)).

## Architecture & compatibility

- Read [docs/README.md](docs/README.md), [development conventions](docs/development-conventions.md), [UI architecture](docs/ui-architecture.md), and [implementation status](docs/implementation.md) (historical specs are not acceptance evidence).
- **English-only UI & docs**: Keep all product UI copy (labels, buttons, statuses, empty states, toasts, tooltips, and Gallery fixtures), project docs, prompts, role/session instructions, `prompts/`, and `skills/` strictly in English; get explicit user approval before modifying any prompt.
- **UI design rules**: **No explanatory UI copy** (show data, state, and actions directly; never explain UI rules or mechanics on screen) and **flat hierarchy, no nested borders** (use 1px hairline dividers on the canvas instead of bordered containers inside bordered containers; see [UI architecture](docs/ui-architecture.md)).
- **Crate boundaries & license**: Use `canban-xxx` in `crates/xxx` (omitting `canban-` in module/file names), separating AppServer, Kanban, Workflow Engine, Scheduler, UI (`canban-ui-kit` vs product UI), and supporting crates. Apache-2.0 (`JJy <jjyruby@gmail.com>, 2026`, `license.workspace = true`; preserve third-party notices).
- **Runtime contracts**:
  - Route all local CLI probing and child process launches through `crates/exec` (`std::process::Command::new` is disallowed by `clippy.toml`).
  - Keep `Snapshot` (`app.state`) bounded (serve timelines/histories via paginated or on-demand RPCs) and canonicalize external polls before diffing.
  - Keep mutation RPCs non-blocking: return a hydratable write receipt (with Card `revision`) and hydrate alongside optimistic UI projections.
- **Compatibility & migrations**:
  - Bump `DESKTOP_API` in `crates/updater/src/lib.rs` whenever UI ↔ `AppServer` compatibility changes (RPCs, `Snapshot`/events, or compiled Workflow YAML capabilities).
  - Evolve Workflow YAML schemas strictly via sequential migrations in `crates/workflow-engine/src/migrations.rs` (`Document.schema` / `CURRENT`); keep runtime/UI targeting `CURRENT` only.

## Verification, dev & release

- **Checks & DST**: Run `mise run check` (fmt + compile), `mise run lint` (Clippy `-D warnings`), and `canban-sim` (`corpus`, `run --seeds 1 --assert-coverage`, `random --scenario chaos --seeds 12 --jobs 4 --master-seed 2026 --assert-coverage`). Use `mise run test` only to ensure existing tests do not regress (`mise run test -- -E 'package(...)'`).
- **UI & app validation**: Verify UI components in `mise run gallery` (`examples/gallery`) and end-to-end app flows in an isolated `mise run dev` / `mise run dev-stop` instance (compilation alone is not acceptance).
- **Strict isolation**: Never touch `~/.canban` or the user's live Herdr core (`hc server stop`/`restart`). Strip inherited `HC_*` and set isolated `HC_SESSION`, `HC_SOCKET`, `XDG_STATE_HOME`, and `XDG_CONFIG_HOME` for any Herdr/DST run ([development startup](docs/development-startup.md), [isolation gaps](docs/instance-isolation.md)).
- **Install, release & machines**: Use `mise run install-app` for `/Applications/Canban.app`, `mise run release` to dispatch a GitHub Release, and keep machine-specific connection/deployment knowledge in `~/Workspace/Machines`.
