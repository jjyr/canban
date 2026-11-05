# Herdr identifiers and Canban execution identity

Research date: 2026-11-05. Source-only findings; no core, agent, or app process was started or modified.

Inspected clean snapshots:

- herdr-core 0.1.18: [c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a](https://github.com/herdrdev/herdr-core/tree/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a).
- herdr-app 0.1.13: [e77f112ac2ea136b84fe510efa42d41163bfd776](https://github.com/herdrdev/herdr-app/tree/e77f112ac2ea136b84fe510efa42d41163bfd776).

## Finding

**Herdr already supplies the runtime identity Canban needs: the owning core ID plus the terminal address. It does not supply a universal agent-conversation or durable pre-launch execution ID.**

Reuse those runtime identifiers rather than inventing another terminal ID. For the application ledger, Canban can reuse its existing dispatch request identity plus an attempt number. A separate Session UUID, Run UUID, and execution UUID are not all required.

This is consistent with the [one Session model](../product/sessions.md): mention, stage-entry, and future scheduled requests differ in scheduling inputs/callbacks, not in execution type.

## Which identifiers exist?

| Identifier | Verified meaning and scope | Suitable use |
| --- | --- | --- |
| hello.core.id | Random identity persisted in core.json for one core state namespace | Identify the owning runtime; pair with the local terminal address |
| Terminal.address, such as t7 | Public terminal identity allocated from the registry counter on that core | Read/attach/control and runtime association |
| Remote address, such as box/t7 | Terminal address rewritten with the connected core's local machine alias | Current route/display address; do not treat the alias as permanent identity |
| Machine.id | The remote core's permanent ID | Resolve the owning core behind a remote route |
| Terminal.name | Optional human name, unique only among live terminals | Human convenience; insufficient as a permanent execution key |
| HC_TERMINAL_ID | Local terminal address injected into the launched program's environment | Let an agent identify its runtime terminal when calling back |
| HC_SESSION | Name selecting isolated core state/socket namespace | Runtime configuration, not an agent conversation ID |
| Terminal.parent | Parent terminal address, optionally remote-qualified | Delegation relationship, not a Session/Card ID |
| Terminal.labels | Client-defined mutable string metadata | Store Canban correlation keys for lookup/reconciliation |
| Terminal.turn | Counter increasing when root status leaves working | Attention/wait progress; not a successful-task or conversation identity |
| OSC status record id | Program-provided key for a status record within a terminal | Track reported sub-status; no guaranteed provider-conversation meaning |
| Event seq | Ordered sequence within a core process | Live event reconciliation, not durable history identity |
| JSON-RPC id / attach channel | Reply correlation / connection stream routing | Transport only |
| Provider conversation/session ID | No dedicated field found in the inspected public Terminal/Status/API schema | Optional future provider-specific metadata, not assumed to exist |

Sources: [core information](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L12-L18), [terminal record](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L13-L95), [machine identity versus alias](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/machines.rs#L12-L33), [terminal environment](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/run.rs#L211-L239), [core namespace paths](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/paths.rs#L8-L35), [status model and counters](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs#L12-L136).

## Runtime identity to reuse

```text
Canban connects to local core A
  hello.core.id = A
  terminal t7
      canonical runtime identity: (A, t7)

Canban sees box/t7 through core A
  machine alias box -> Machine.id = B
  local terminal on B = t7
      canonical runtime identity: (B, t7)
      current route through A: box/t7
```

For local-only MVP, storing the connected core ID and returned local address is enough to identify the terminal. If remote execution is added, retain the owner core ID/local address plus the connection route. Do not pair local core A with a remote bare t7 and accidentally identify A's own t7.

Remote aliases can be renamed. The core's [rename implementation](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/remote.rs#L424-L441) rewrites mirrored addresses; a formerly visible box/t7 can appear under a new prefix while referring to the same remote terminal.

## Stability and lifetime

| Situation | Core ID | Terminal identity | Meaning for Canban |
| --- | --- | --- | --- |
| Client reconnects | Preserved | Preserved if the record exists | Rebind/read/attach; do not create a new execution |
| Planned core restart with holders preserving programs | Preserved | Preserved through saved registry/adoption | Same running agent can continue; refresh snapshots and streams |
| Agent program exits normally | Preserved | Finished record retained subject to policy | Capture result/history separately |
| keep recovery after program loss | Preserved | Same terminal address retained | A fresh shell can replace the agent; same terminal does not prove same conversation |
| Terminal record pruned or explicitly removed | Preserved | No longer queryable | Keep the saved identity as historical reference; no inference that task history should disappear |
| State wiped/recreated | New identity under normal fresh initialization | Counter can start over | A new core/t7 is not the old core/t7 |
| State disk detected as copied to another machine | Core creates a new identity and forgets old runtime life | Old terminal registry is discarded | Treat it as a different runtime, even if paths/names look the same |
| Remote alias renamed | Owner ID unchanged | Owner-local address unchanged | Refresh routing alias; do not rewrite logical execution identity |

Core loads the saved registry at startup. Its next_id increments on creation and is not reduced when records are pruned. Under intact persisted state, pruning does not by itself recycle terminal IDs. A retained (core ID, local terminal address) remains a useful historical key even after it stops being queryable. Do not confuse missing current state with invalid historical identity.

The source's CoreInfo comment describes the ID as reset when state is wiped, but [server.rs:107–135](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/server.rs#L107-L135) also handles detected disk clones by assigning a new ID and deleting old runtime state. Hostname changes alone do not trigger this.

Evidence: [registry allocation](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L209-L259), [saved-registry startup](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/runtime.rs#L163-L185), [finished-record retention](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L372-L401), [recovery and stream sequence semantics](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md#L148-L177).

The default finished retention is approximately one day, with live-parent observation rules, and at most 500 finished records. Removal can happen sooner with explicit rm/remove. This is not Canban's product-history retention.

## Why the terminal ID cannot be the only execution key

A terminal key can be the canonical identifier for a successfully created runtime terminal. If Canban only displayed existing terminals, it could directly use that key as Herdr app does.

Canban also needs to represent:

- Queued work before any terminal exists.
- Preparation/validation failures before terminal creation.
- Failed starts where core removes the terminal record.
- A launch whose reply is lost, so the terminal may exist but its returned address is unknown.
- Multiple resolved attempts under one original request.
- A terminal surviving while its original agent is replaced by a shell.
- Historical results after core's record retention expires.

These do not require an elaborate conversation subsystem. They do require a durable application intent/attempt reference independent of receiving terminal.run's reply.

RunParams contains no application idempotency key. Core allocates/records its terminal during start, before the result reply. An initial spawn timeout can leave spawn_pending, and a failed/non-kept launch can remove the record. A JSON-RPC message ID or a terminal name does not bridge this window. See [run parameters](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L198-L285), [launch and failure path](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/run.rs#L133-L206).

## Recommended minimal scheme

Proposal:

```text
Card / trigger event
       |
dispatch_request_id             created before external IO
       |
attempt_number                  1, 2, ... only for actual new attempts
       |
logical execution key           (dispatch_request_id, attempt_number)
       |
optional runtime binding        (owner_core_id, local_terminal_address)
       |
optional provider metadata      actual provider conversation ID, if observed
```

Use the existing request/attempt key as the Session/Run identity. It can be serialized into one run_id string if the UI/API needs a scalar. Do not also require separate Session, Run, and execution UUIDs for the same attempt.

Before calling terminal.run, persist the attempt and launch configuration. Pass a namespaced correlation label such as canban.run derived from that key, plus project/card labels where useful. Store the returned runtime binding without generating another synthetic terminal ID.

If the reply is lost, list all retained terminals including finished ones by the correlation label and reconcile with the existing launch ledger. Labels are mutable and may be removed; they aid correlation, not authorization or an exactly-once guarantee. If evidence is insufficient, retain an unknown state rather than automatically starting another attempt.

Recommend keep=false for Canban's fresh task executions unless a deliberate product requirement needs shell recovery. This is a proposal, not a source requirement. It avoids treating a replacement shell as the old agent. Holder-preserved continuity across core/viewer restart is independent of keep; copying Herdr app's keep=true default is unnecessary for that purpose.

For MVP, this leaves one logical execution identity plus Herdr's own runtime identity. No separate session types are introduced.

## Provider and internal IDs not to rely on

The inspected public schema has no dedicated Claude/Codex conversation identifier. Status.app, status.title, and status.msg are program reports; none guarantees such an ID. A caller could choose to report or label one, but that would be a provider/client convention requiring validation.

program_started_ms, holder, and scope are private bookkeeping removed from Public Terminal and its schema. Although startup updates program_started_ms, Canban must not use it as a supported incarnation/session ID. PID is public but reusable and changes with a replacement program. OSC child-record IDs are supplied by the program, and turn can advance on blocked/clear/exit, so neither is a successful-execution counter.

See [private-field filtering](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L405-L450) and [status/turn behavior](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs#L76-L136).

## Existing Herdr app precedent

Herdr app's [core launcher](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/hc.rs#L393-L406) calls terminal.run and returns its address. Its [tab/pane model](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/model.rs#L102-L105) has app-owned IDs while storing a terminal address as the runtime reference. It does not obtain a generic provider-conversation ID from core.

This is a useful precedent for reusing runtime addresses while keeping product identities independent. It is not proof that Herdr app already supplies Canban's dispatch ledger or durable card history.

## Remaining validation

The recommendation is based on source inspection. A later isolated fixture test should cover lost replies, reconnect, core restart, terminal pruning, keep recovery, and owner identity/alias changes. No such runtime test or installed-version compatibility check was performed here. No machine inventory or user runtime state was changed.
