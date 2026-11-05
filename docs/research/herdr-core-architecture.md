# herdr-core architecture

Status: source research, 2026-11-05. No core was started, changed, or benchmarked for this document.

This document describes the runtime's **code and process architecture**. [Herdr product concepts](herdr-core-concepts.md) separately explains Space, workspace, terminal, and their ownership. [Integration](herdr-core-integration.md) proposes the Canban boundary.

## Revision and scope

The inspected upstream is `herdrdev/herdr-core` at [`c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a`](https://github.com/herdrdev/herdr-core/tree/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a), whose package version is **0.1.18**. The protocol is preview, generation **1**. Statements marked **Verified** are source/documentation findings at this revision; they are not live acceptance results. Current Herdr app reference: [`e77f112ac2ea136b84fe510efa42d41163bfd776`](https://github.com/herdrdev/herdr-app/tree/e77f112ac2ea136b84fe510efa42d41163bfd776).

## Runtime responsibility

**Verified:** core runs processes in durable terminals, parses their output, serves raw terminal streams and state, and supports parent/child agent communication. It draws no UI. Its own [project rule](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/AGENTS.md#L23) says it is a runtime, not a multiplexer: layout/workspace features belong outside core.

```text
Apps / CLI / agents
        |
        | JSON-RPC control; framed terminal streams
        v
+--------------------- core process ----------------------+
| server.rs / api.rs     request handling, method schema   |
| runtime.rs             registry + live runtime state     |
| events.rs / status.rs  facts, subscriptions, OSC 7501    |
| ghostty-vt             parsed screen/history/query state |
| viewers.rs / wire.rs   attach, flow control, input       |
| remote.rs              connections to other cores       |
| persistence            terminal records + saved screens |
+------------------------+-------------------------------+
                         | private holder protocol
                         v
+-------------------- holder process ---------------------+
| owns PTYs + program processes; reads output continuously |
| spawn / input / resize / signal / reap / bounded tail    |
| supervises core recovery; no board or agent-brand policy |
+------------------------+-------------------------------+
                         |
             +-----------+------------+
             v                        v
         PTY / agent A            PTY / shell B
```

The holder is a crucial boundary. Closing a viewer does not kill the program; planned core restart can preserve programs through holders. This is stronger than an app spawning an agent directly and keeping its process handle in a UI object. It is not a guarantee of surviving OS reboot, process death, or every crash without lost output.

## Code map

| Source | Responsibility | Consequence for Canban |
| --- | --- | --- |
| [`api.rs:15`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api.rs#L15), `api/types.rs` | Protocol generation, features, one method table, typed arguments/results | Build the adapter against actual methods and negotiated features. |
| [`server.rs:19`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/server.rs#L19) | Session lock, endpoint, current-thread Tokio runtime, connections, recovery tasks | Core is already an independently managed service process. |
| [`runtime.rs:23`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/runtime.rs#L23) | `Core`, `State`, screens, viewers, holders, persistence, machines | Runtime state is larger than a subprocess runner. Avoid recreating it in Canban. |
| [`holder.rs:1`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/holder.rs#L1), `holder_proto.rs` | Durable PTY/process ownership; approximately 1 MiB recovery tail per terminal | A core reconnect is not a request to relaunch the original task. |
| [`status.rs:98`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs#L98) | Applies reported status; increments a turn when root leaves `working` | Runtime progress is evidence for a card, not a business completion verdict. |
| [`events.rs:1`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/events.rs#L1) | Ordered changes and fresh snapshots for slow subscribers | Maintain a live projection, with explicit reconciliation. |
| [`templates.rs:1`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/templates.rs#L1), `input.rs`, `notes.rs`, `wait.rs` | Launch roles, readiness, serialized input, parent notifications, turn waits | Reuse execution mechanics; implement Canban tags and dispatch decisions separately. |
| [`remote.rs:204`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/remote.rs#L204), `machines.rs` | Keep connections to machines, mirror terminals, route calls | Preserve machine identity when associating a card with a run. |
| [`api/doc.rs:9`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/doc.rs#L9) | Compare-and-set opaque JSON documents | Suitable for small client metadata, not automatically a Kanban database. |

## Control, events, and interactive output are different channels

**Verified:** [API transport documentation](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md) specifies local Unix sockets or Windows named pipes. `hc connect` carries the protocol over stdin/stdout and can run through SSH. A connection begins with newline-delimited JSON-RPC. `hello {"framed":true}` switches after its reply to framing.

```text
Canban Rust adapter -- hc connect -- local core
                                     |
                                     +-- channel 0: JSON-RPC request/reply
                                     +-- events channel: JSON event messages
                                     +-- attach channel A: raw terminal bytes
                                     +-- attach channel B: raw terminal bytes
```

A frame is `[channel:u32 LE][kind:u8][length:u32 LE][payload]`, maximum payload 16 MiB. Stream kinds include DATA, CREDIT, CLOSE, RESIZE, FOCUS, FACTS, and AUTOMATIC_INPUT. Streams start with 256 KiB credit; a viewer more than 4 MiB behind receives a repaint rather than an unlimited output backlog. These are current implementation limits, not a performance benchmark.

Interactive attach is **not** an HTML view or a rendered screen API. The client needs a terminal emulator plus keyboard encoding, selection/copy, focus, sizing, display facts, and backpressure. Initial attach supplies a terminal paint and optional history; subsequent output is raw bytes. An observer must honor negotiated resize ordering. Automatic replies from a terminal emulator should use AUTOMATIC_INPUT only when both `hello.features` and the attach result confirm support; otherwise use the documented DATA fallback. This distinction prevents automatic replies from stealing activity/size ownership on capable paths.

## Agent state is reported, not inferred

**Verified:** status comes exclusively from OSC 7501. There is no core screen-text agent detector, vendor registry, or automatic recognition of Claude/Codex. Foreground process facts are separate observations, not agent-state inference.

```text
program emits OSC 7501
          |
          v
Ghostty parser -> status.rs -> terminal.status / records / turn
                                      |
                                      v
                              ordered status event
                                      |
                                      v
                          Canban runtime projection
                                      |
                           Canban completion policy
```

A program without status reporting has **unknown status**, even when the process exists and output is arriving. `terminal.wait` treats a never-reporting program as working until it exits. `blocked`, `idle`, `done`, and `error` can all end a wait; a timeout is a check-in and leaves the process running. Therefore “wait ended”, “agent says done”, “process exited 0”, “human accepted”, and “merged” must remain separate facts.

Templates are Markdown command/role definitions loaded from core's config directory. They neither install agents nor configure status reporting. Default readiness waits for a status report, while `ready: quiet` explicitly substitutes output settling; quiet does not prove readiness. `terminal.run` reports whether the first message was sent. At this revision `templates.rs:127` defaults readiness timeout to **60,000 ms**; the templates prose still says 30 seconds. Code is authoritative, and Canban should handle the result rather than rely on either fixed delay.

## Persistence and recovery

**Verified:** terminal registry, screens/history, machine connections, peer records, and opaque documents reside in core state. Session selection uses `HC_SESSION`; config/templates share the config home unless XDG config is separately isolated. `HC_SOCKET` changes the endpoint without moving state.

```text
core restart / update
      |
      +-> save screens + hand over -> holders retain programs
      +-> new core adopts holders -> reconnect + snapshot + attach

lost program, keep=true
      |
      +-> lost event -> retry recovery -> NEW SHELL -> restarted event
                                                 (original task NOT rerun)
```

`keep` is a terminal-continuity feature, not a job retry policy. During recovery, attach/input can return unavailable. Explicit kill and normal process exit remain final. An uncertain initial spawn can retain a `spawn_pending` claim because a program may already be starting; a client must not interpret a timeout as proof that nothing ran.

Core terminal retention is finite: [`terminals.rs:371`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L371) expires finished records after its observation/one-day retention rules and caps them at 500; `rm` can remove a terminal sooner. Screen history is bounded too. A complete Canban task timeline and review evidence need Canban-owned durable records and artifacts.

Events have process-local sequence numbers. Reconnect starts from a new snapshot; a slow subscriber gets `missed` plus a replacement snapshot. **There is no durable event replay log for reconstructing an entire task history.** Documents are not included in terminal snapshots: subscribe first, then refetch watched documents after reconnect, `missed`, and remote reconnection.

Documents use CAS versions, a 1 MiB serialized JSON limit per name, and atomic `docs.json` replacement. Null deletes the value while retaining its version. A malformed document file is quarantined and core can start with empty documents. These are useful client-storage primitives, but insufficient justification for making them Canban's sole store for notes, cards, schedules, and an append-only timeline.

## Existing API and deliberate omissions

**Verified from the method table:** core exposes server lifecycle; terminal run/list/get/kill/remove/keep/label/read/attach/send/wait/keys/interrupt/put; template listing; machine management/execution; file read/write/edit; documents; subscription; discovery.

Core includes operational limits such as bounded streams and limited concurrent `machine.exec` calls. It does **not** provide Canban's card model, tag resolution, board-transition triggers, cron policy, per-model budget, fair queue, workspace lock, approval ledger, PR review model, or merge orchestration. Parent/child execution and notification are runtime facilities; they are not a complete task scheduler.

`machine.exec` runs bounded commands without a PTY and ends their process scope on completion, timeout, or caller disconnect. Long-running interactive agents belong in `terminal.run`, not in a disconnected exec call.

## Trust boundary

**Verified:** core is not a sandbox or permission broker. Any API client gets the core account's command, filesystem, and terminal access. Unix protection is local socket/private directories; no per-client API password or capability scope is added. Sessions separate runtime state, not security identities. Remote SSH supplies authentication/encryption; custom transports must supply their own guarantees.

For Canban this means the Rust AppServer should own the core connection and expose shared typed operations to native NSH UI and narrow adapters to any optionally embedded WebView. Untrusted preview HTML and arbitrary note content must not gain the raw core API. This is an integration design requirement, not a new guarantee supplied by core.
