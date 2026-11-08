# Multi-machine execution through Herdr Core

Research snapshot: 2026-11-08. This document retains the source inspection and discussion history. The approved design is now implemented; use [current multi-machine behavior](../multi-machine.md) and [acceptance](../multi-machine-acceptance.md) for the runtime contract. Earlier proposal and not-implemented statements below describe the inspection baseline.

## Inspection boundary

- Installed `hc --version`: **0.1.18**.
- Core source: `c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a`, package 0.1.18,
  inspected in `/private/tmp/canban-research-herdr-core`.
- Herdr GPUI source: `a6db99b4edab37b2038ac1016bd0af82ed0a5ebc`.
- Canban baseline: `59ea696`.

This pass read local source/documentation and the installed version only. It did
not query the user's running Core, connect to hosts, install remote software or
perform a real multi-machine acceptance test. These versions are inspected
snapshots, not an assertion of latest upstream releases.

## What Core already provides

```text
Canban UI + AppServer + product database
                  |
             local hc/Core
              /    |     \
      local PTYs   SSH     SSH
                   |       |
               Core A    Core B
                PTYs      PTYs
                files     files
```

Core maintains named connections to other Cores. The destination owns the
programs, files and terminal history; the local Core aggregates Terminal lists
and events. `terminal.run` accepts `on`, and existing Terminal operations route
qualified addresses such as `box/t7`. Attach streams can therefore use the same
Canban native renderer rather than a second remote renderer.

| Capability | Existing Core support |
| --- | --- |
| Connection setup | `hc machine add`, named SSH targets, optional destination session |
| Installation/update | Interactive approval when needed; explicit installation option for scripts |
| Other transports | `--via` commands carrying unmodified stdin/stdout without a PTY |
| Remote interactive work | `hc run --on`, qualified read/send/attach/control operations |
| Remote short commands | `machine.exec` / `hc exec --on`, bounded duration/output and process scope |
| Files | Remote read/write/edit and local-file upload through `hc put` |
| Reconnection | Core retries; observed states include connecting, connected, needs_login, needs_update and unreachable |
| Identity | Machine record contains the destination Core's permanent ID; reconnect checks it |

Losing an SSH connection does not itself end remote terminal programs. Removing
the local connection does not remove remote state or stop remote programs.
Remote host shutdown or sandbox expiry is different from a lost connection.
Short `machine.exec` calls are not the lifetime owner for interactive Agents.

A Machine name is a local route alias. Stable binding must distinguish the
connected gateway Core, destination owner Core, destination-local Terminal ID
and current route. Alias rename/reuse must not silently rebind an execution.
Core sessions isolate runtime state; they are not separate account permissions.

## What remains in Canban

Current Canban launches without `on`, performs Git/filesystem work locally and
injects local CLI/socket/run-directory paths into Agent invocations. Merely
passing a remote Terminal address does not complete remote workflow support.

1. **Machine-aware execution locations and paths.** Project is a Kanban/context
   organization object and must not belong to one machine. Match concrete
   working directories by `(machine identity, directory)` and associate those
   locations with product context separately. Equal paths on two machines do
   not identify one checkout; different checkouts can serve the same Project.
   Local canonicalization and local Git cannot resolve remote cwd/worktrees.
2. **Remote workspace operations.** Prepare worktrees, read Diff, commit and
   merge on the selected checkout's machine, using the same product workflow rules.
3. **Context and artifacts.** Stage the captured prompt/context/attachments on
   the destination and retrieve outcomes/reports into Canban's durable history.
4. **Agent-callable Canban interface.** Remote `canban` needs a reachable
   AppServer endpoint and a working CLI/proxy. The current local Unix socket,
   binary path and `CANBAN_RUN_DIR` cannot be copied verbatim. An SSH-forwarded
   endpoint or a small remote proxy are candidates, not selected designs.
   Core's parent-Terminal callbacks do not provide arbitrary Canban RPC.
5. **Recovery and scheduling.** Preserve execution identity across reconnects;
   an unreachable host is not evidence that its Agent stopped. Keep product
   completion reports distinct from runtime observations.

Herdr GPUI also implements saved devices, SSH hosts, discovery and cloud
providers. Some of that UI uses its older Herdr daemon transport. Its current
Core client represents remote `on` but the generic backend spawn path leaves
it unset. Reuse Core's public machine protocol deliberately rather than treating
all GPUI device features as already integrated Core capabilities.

## Suggested first scope, pending discussion

Keep one Canban AppServer/database as the workflow authority and reuse local
Core's remote connections. Start with remote Terminal viewing/input/reconnect.
The user rejected binding a Project to one machine: Projects organize Kanban
and context. Execution placement belongs to concrete working directories and
execution records. Keep existing Profile/workflow semantics while discussing
how placement is selected. Do not introduce multiple Card databases or
cross-computer synchronization merely to execute remotely.

Multiple computers independently editing the same Canban data is a separate
product architecture. Core's machine feature does not synchronize Canban SQLite,
Notes, Cards or their Timeline. If the Mac hosting AppServer stops, remote PTYs
may continue but Canban scheduling/report handling is absent until it returns.

## Composition options under discussion

These are alternatives, not approved schema changes or implementation scope.

```text
Project -> Board / Cards / Notes / Workflow
                     |
                   Request
                     |
             resolved execution context
             + Profile: which Agent/configuration
             + Machine: where it executes
             + Checkout?: which concrete files/version
                     |
                  Attempt -> Herdr Terminal
```

A checkout/worktree is a concrete machine-local working copy, not a replacement
for Project or a mandatory user-created Space. Non-code tasks can run without
a Git checkout, and independent Terminals need no Card or Project. The current
MVP's `Project.directory` combines a product object with its default local work
location; separating those concerns later should preserve existing directories
and bindings rather than move them automatically.

| Placement composition | Useful case | Tradeoff |
| --- | --- | --- |
| Execution explicitly selects machine and directory | Ad hoc remote Agent or independent Terminal | Simple, but automatic dispatch needs a default |
| Profile supplies an optional preferred target | A GPU/Linux-specific Agent setup | Convenient; avoid coupling every shared Profile to one machine |
| Workflow stage selects a target | Develop on Mac, test on Linux | Explicit handoff; code/version delivery between checkouts is required |
| Scheduler selects from eligible machines | Several interchangeable workers | Requires capability, availability and placement rules; later scope |

All can resolve to the same execution record; they need not become separate
Session types or separate execution engines. A live invocation pins its actual
destination, checkout and provider session. Editing defaults does not move it.
Resume returns to its original destination/context unless an explicit migration
is designed. A Git branch with the same name on two machines does not prove
equal content; uncommitted edits require explicit transfer/snapshots before
cross-machine review or tests can claim to inspect the same work.

Application deployment is a separate axis:

| Workflow authority | Benefit | Cost |
| --- | --- | --- |
| One desktop-hosted AppServer | Matches the current application; remote Agents through Core | Workflow orchestration stops when that app stops |
| One independently hosted AppServer, one or more native clients | Scheduling survives closing a client; shared product state | AppServer transport, remote context access and service lifecycle |
| Independent AppServers/databases with synchronization | Offline work on several computers | Conflict handling for Card state, timeline order, scheduling and reports |

An optional remote execution helper could prepare checkouts, stage files or
proxy Agent CLI calls. Such a helper would not own another Board/database or
decide workflow progression; Core's exec/file APIs may cover the first version
without one. Neither a remote helper nor a machine pool is selected yet.

## AppServer lifetime: selected direction

The user selected an AppServer lifetime independent of UI. One authoritative
AppServer owns product data, workflow progression, scheduling and the global
budget; UI instances are clients. Closing a UI must not stop that service.
The existing CLI can already host AppServer with `canban serve`, but the desktop
currently co-hosts it rather than attaching to an existing server. Service
lifecycle and desktop-client transport changes remain unimplemented.

The default execution destination is server configuration, not the computer
where a user happened to open a UI. Project remains independent of placement.

## Proposed machine-selection rules, pending user agreement

Separate queue ordering from placement. Preserve current depth-first stage
priority, FIFO within a depth, human gates and the default global budget of
eight Canban-managed Agents. Independent/manual Terminals do not consume it.

1. **Preserve existing execution context.** Continuing a live conversation or
   resuming a provider session returns to its original destination and checkout.
   An existing Card checkout normally determines where subsequent code work
   runs. A conflicting explicit target requires an intentional new checkout/
   handoff, not treating an equal path on another machine as the same files.
2. **For an unplaced invocation, resolve requested placement.** A per-invocation
   or stage target can specify a destination; a Profile can optionally restrict
   eligible destinations. Without such a rule, use the configured default
   execution destination. Exact configuration syntax is not selected yet.
3. **Filter eligible destinations.** The destination must be enabled for
   automatic work, connected, able to run the Profile, and able to provide the
   requested directory/code context. A fixed target is a requirement, not a
   hint permitting silent fallback. Herdr reports connectivity but does not
   define Canban Profile availability or repository readiness.
4. **Choose among genuinely interchangeable destinations.** Count active
   Canban-managed Agent slots on each, select the least occupied, and break ties
   in a fixed configuration order. Reuse current starting/working/unknown slot
   semantics; verified blocked/idle/done/error states release execution slots.
   Existing invocations do not migrate to rebalance. No default per-machine cap
   or hardware-weight calculation is proposed for the first version.
5. **Reserve and record before launch.** Persist the selected destination,
   checkout and a short placement reason on the execution path before starting
   the Terminal. Existing uncertain-launch/idempotency rules still apply.

If a candidate cannot run now, requests waiting for it consume no execution
slot. Choose the deepest/FIFO request that is currently dispatchable, so a
Review waiting on an offline host does not stall unrelated work on another
host. No valid candidate is a visible waiting/configuration reason rather than
a new failed Agent invocation. Login/configuration problems can surface through
attention notifications. Reconnecting a host does not approve a Card's human
gate. An already-started disconnected Agent retains an uncertain execution
reservation and is not automatically relaunched elsewhere.

Cross-machine code delivery and the Agent CLI reporting channel remain separate
design requirements. Selecting a destination alone does not implement them.

## Sources

- [Core Machines guide](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/docs/machines.md).
- [Machine identity](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/machines.rs#L12).
- [Reconnect and route handling](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/remote.rs#L238).
- [Protocol routing](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/remote.rs#L760).
- [Exec lifetime](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/exec.rs).
- [GPUI Core backend](https://github.com/herdrdev/herdr-app/blob/a6db99b4edab37b2038ac1016bd0af82ed0a5ebc/crates/herdr-core-client/src/backend.rs#L71).
- Canban: `crates/herdr/src/lib.rs`, `crates/app-server/src/runtime.rs`,
  `crates/workspaces/src/context.rs`, `crates/workspaces/src/lib.rs`.
