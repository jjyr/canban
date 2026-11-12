# Integrating Canban with herdr-core

Status: **recommendation for discussion**, 2026-11-05. This is a proposed architecture, not an implemented or approved implementation decision. Source baseline: [core architecture](herdr-core-architecture.md), core `c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a` (0.1.18), Herdr app `e77f112ac2ea136b84fe510efa42d41163bfd776`. [Product concepts](herdr-core-concepts.md) explains why Space belongs to the app layer.

Current product scope is governed by the user's later [MVP simplifications](../mvp-plan.md): trust agent completion, ignore exceptional residual old execution, retain worktrees until Archive, use ordinary Review reports and JSON templates, reuse existing runnable agents, and start with internal enum/channel/topic events. Source limitations below remain valid; earlier hardening ideas must not become extra MVP progression gates. Detailed event architecture and macOS notification delivery are deferred.

## Recommendation

Build Canban as an **independent native Rust/NSH application and a client of the herdr-core protocol**, with WebView embedded only where needed. Its logical AppServer is co-hosted in the desktop process for MVP and can be hosted separately later. Canban should own human context, notes, cards, workflow decisions, dispatch rules, run records, and its durable timeline. Core should own terminals, processes, terminal status, and remote runtime connections.

For the MVP, prefer connecting to a compatible user-installed local core, following current Herdr app's client approach. Sharing the default core enables Canban to index existing agents and coexist with Herdr without requiring Herdr's UI to be open. Provide an explicit connection target and display its core identity. An isolated Canban runtime session is a later operational option; it uses the same protocol adapter and does not change the domain model.

This recommendation selects a **boundary**, not an obligation to inherit Herdr layout documents, GPUI, or every existing UI feature. It also does not assume Canban dispatch already exists inside core.

## Route comparison

| Route | Reuse | Additional ownership/cost | Assessment |
| --- | --- | --- | --- |
| Link core's Rust library into Canban's UI/backend process | Internal Rust types/functions | Adapting daemon lifecycle, Tokio setup, holder re-exec modes, process supervision, updates; dependence on runtime internals | Technically possible with substantial work; poor default seam. |
| Ship/start an isolated core as an app-managed background service | Public protocol and existing holder behavior | Binary packaging, signing, compatible updates, session/config/state selection, clear process lifetime | Viable later for a self-contained distribution; do not make it die with the WebView. |
| Connect to an installed core service | Public protocol; user agents/machines; Herdr coexistence | Discovery, compatibility checks, reconnect, own app database, terminal viewer | **Recommended MVP route.** Lowest upstream coupling with useful interoperability. |
| Implement Canban as a Herdr app mod | Host UI, shell, terminal surfaces, app commands | GPUI/mod API and host lifecycle constraints; does not use the requested independent NSH shell | Useful reference/prototype route, not the proposed product host. |
| Drive only Herdr app commands | Existing Space/workspace/terminal UI | Requires Herdr app availability; UI topology becomes an execution dependency | Optional “open in Herdr” integration, not the primary runtime contract. |

**Verified nuance:** core has a public [`lib.rs`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/lib.rs), so “core cannot be embedded because no library exists” would be incorrect. However, no stable embeddable-runtime façade is documented. [`server::serve`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/server.rs#L19) builds its own Tokio runtime, and [`client::spawn_detached`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/client.rs#L177) normally re-executes the current executable for `serve`/`hold` modes. Linking the crate into an unrelated executable does not automatically retain the standalone binary's behavior.

**Verified precedent:** current Herdr app [`src/core_bin.rs`](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/core_bin.rs#L1) explicitly discovers an installed core or installs core's own release; the app does not carry one. [`src/hc.rs:277`](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/hc.rs#L277) opens framed `hc connect`; generation/method/version validation is a separate step in that module. Reuse this design knowledge rather than copy its GPUI host assumptions.

## Proposed module and authority boundaries

```text
+------------------------------------------------------------------+
| Rust/NSH native UI                                               |
|      | typed Rust commands/queries                               |
|      v                                                           |
| AppServer <--------------- local IPC adapter <--- CLI            |
|   +-- Kanban / Context / Timeline / Notifications crates         |
|   +-- Workflow Engine / Scheduler crates                         |
|   +-- small event enum and memory subscriptions                  |
|   +-- Storage / Workspaces / Herdr adapter crates                |
| Optional embedded WebView for a particular surface               |
+------------------------------------------------------------------+
                         | hc connect / core protocol
                         v
                 herdr-core service
                         |
                 holders / terminals
                         ^
                         | same runtime, other client
                   Herdr app (optional)
```

AppServer is one business-operation/state owner and is independent of NSH types. Native UI calls typed Rust operations; it does not need to serialize RPC to its own service. The IPC/CLI adapter and a possible later MCP adapter call the same handlers. No view remount should create a second dispatch loop. The [architecture](../architecture.md) shows co-hosting now and optional daemon extraction later, including lifecycle costs. A local store such as SQLite plus artifact files remains a proposal.

| Data or operation | Authority |
| --- | --- |
| Human intent, note text, context versions, card state, acceptance criteria | Canban |
| Tag aliases, command/profile definitions, rule versions, scheduling/limits | Canban |
| Queue admission, dispatch intent, immutable attempt ID, retry decision | Canban |
| Terminal ID, PID, process state, reported agent status, raw terminal output | Core |
| Whether the user accepted work or authorized a merge | Canban |
| Git object content and actual branch/ref state | Git; observed by Canban |
| Herdr Space/workspace/tab/pane layout | Herdr app, if used |

## Association model

A card can have many execution attempts. Canban uses one generic Session model, with the Run identity serving as session identity in the proposed MVP. Submitted @mentions and stage entries schedule fresh conversations through the same core adapter; only the dispatch trigger/task/context/callback binding differs. This Session is not HC_SESSION. See [Session model](../product/sessions.md). An attempt may have a primary terminal and later child terminals. A core terminal may also be unrelated to any card; display those in Agents without inventing tasks for them.

```text
Canban Project -> directory + Board -> Card #42
       |                  |
       +-> repositories <-+-- selected repository for a coding attempt
       |                  |
       +-> Notes -------> ContextSnapshot
                          |
                          +-> DispatchRequest -> RunAttempt -> core terminal
                          |                       (many over card lifetime)
                          +-> Review/decision/artifact links

External Herdr Space -- optional explicit navigation link -- Canban Project
```

The current product model is directory-backed Project -> Board -> stages/cards, as clarified in [Projects/templates](../product/workflow-engine.md). Use stable Canban IDs for `project_id`, `card_id`, `run_id`, and `context_snapshot_id`, plus project-local card numbers. Store a `RuntimeConnection` plus the core identity and terminal address; on remote runs also retain the selected machine identity and its observed alias. A displayed alias like `box/t7` is routing information, not a globally permanent ID. Do not use CWD, process name, a card title, or a terminal title as the primary join key.

Recommended terminal labels, all **Canban conventions** interpreted by Canban:

Identity minimization: the [focused ID research](herdr-identifiers.md) recommends (dispatch_request_id, attempt_number) for the logical execution and the existing (owning core.id, local terminal.address) for runtime identity. A run_id/canban.run value can serialize that logical key rather than require another UUID. Mutable labels aid lookup; they do not create identity authority or exactly-once launch semantics.

```json
{
  "canban.app": "canban",
  "canban.project": "project_123",
  "canban.card": "card_456",
  "canban.run": "run_789",
  "canban.profile": "opus"
}
```

Core stores/filters labels but does not enforce their meaning or immutability. Canban must keep its own mapping and handle changed/removed labels. Canban card tags such as `claude` select a configured routing profile; `@opus` can name a preset in directed interactions. These are Canban semantics, not native core agent identities. The stage selects the workflow independently of agent routing. A profile resolves to command arguments or a core template plus environment/machine/context policy; record that resolved configuration and rule version per attempt.

Herdr app can be another client of the same core. Canban should not rewrite the app's `herdr-app` document or copy its Space IDs into the Canban domain as if they were core concepts. Optional import/linking should be explicit and preserve both authorities.

## Scheduler protocol and the uncertain-result window

Proposed launch flow:

```text
card enters Develop / Review / another executable stage
  -> commit stage entry + context/workflow/routing snapshot + automatic intent
  -> dispatcher claims queue entry and reserves capacity
  -> reconcile existing attempt/terminal evidence
  -> run recorded workflow preparation callbacks; resolve directory/artifacts
  -> terminal.run with immutable run label and deliberate CWD/argv
  -> persist returned terminal address and first_message outcome
  -> consume status/output; collect artifacts and verification result
  -> apply template outcome; default development result auto-enters Review
  -> atomically link result, stage event, and configured review intent
```

Every agent task needs a recorded context snapshot, not a pointer that silently changes as the note is edited. Preserve the prompt, referenced note versions, repository/base reference, profile, and acceptance criteria. Editing later context affects a new dispatch or an explicit steer event.

**Verified:** `terminal.run` has no durable request-id/idempotency parameter in [`RunParams`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L198). A `name` is 1–32 restricted characters and unique only among live terminals; [`Registry::create`](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L235) and name resolution permit reuse after finish. JSON-RPC message IDs correlate replies; they do not make launching exactly once.

If the connection dies after launch but before the reply, a retry can run the task twice. Persist the intent before IO. On recovery, list **all retained terminals**, including finished ones, by the immutable run label, and reconcile against Canban's prior observations. A deterministic short terminal name can prevent some concurrent live duplicates, but cannot prove exactly-once execution after rapid completion, pruning, or state loss. An unresolved result must remain `unknown`/`reconciliation required`, not silently relaunch. A later attempt should receive a new attempt ID after an explicit retry policy or human decision.

This gap is a reason to keep a run ledger, not a reason to fork core in the MVP. Stronger automatic retry guarantees would require a verified upstream deduplication contract or a controlled execution wrapper with durable receipts; neither was established here.

## Status, output, and completion policy

Keep three axes independently visible:

```text
workflow:    Backlog / Develop / Review / custom stage
execution:   Queued / Starting / Active / Finished / Unknown
reported:    working / blocked / idle / done / error / unavailable
```

The workflow row follows the user's current default template; archive and task outcome are separate. Execution categories are proposals, not new core enum values. A blocked agent should create attention on the card while preserving the running attempt. `done` should lead to validation/review according to the card's policy, not automatically mark the card accepted or merged. A process exit must record its actual reason/code. `keep` producing a fresh shell must not be shown as a resumed implementation attempt.

Core status requires OSC 7501. A profile should document whether its installed agent reports it; otherwise display status unavailable alongside actual process/output observations. Canban's product progression follows the agent's explicit completed declaration through the shared reporting/callback interface, with no independent verification or process-exit gate. This preserves the user's decision to reuse existing runnable agents without adding a provider-specific task evaluator. Do not port legacy screen heuristics into core.

For a full Agents surface, a WebView terminal emulator is a plausible implementation, but compatibility is unverified. Validate terminal modes, IME, large output, backpressure, resize ownership, clipboard/selection, and reconnect on real macOS before committing to parity. An output/read-only detail pane plus open/attach-in-terminal is a smaller staged capability; it must be presented as such if chosen.

## Reconnect and event persistence

1. Suspend new dispatches for the unavailable connection; mark stale observations visibly.
2. Reconnect with backoff; perform `hello`, generation/method/feature checks, and identify the core.
3. Create a new connection epoch. Never compare its sequence numbers to the previous core process as if they formed one log.
4. Subscribe and install the returned snapshot before applying later deltas. On `missed`, replace the affected projection and refetch watched documents.
5. Reconcile pending launch intents and recorded run bindings; do not infer success or failure from missing live terminals alone.
6. Reattach wanted terminal views and negotiate stream features again. Reconnect does not rerun tasks.

Persist Canban domain events and meaningful observed execution milestones under Canban IDs. Core events supply a live projection and observations, not a complete historical transcript. If Canban was offline while transitions occurred, record the reconciliation gap honestly. Store final summaries, verification records, diff/base/head references, and relevant artifacts independently before core retention removes them.

## Runtime lifecycle and updates

| Mode | Startup | Closing Canban | Upgrade/control policy |
| --- | --- | --- | --- |
| Shared installed core, recommended initially | Discover configured binary, validate identity/version; connect/autostart using documented behavior | Close only client channels; running tasks continue | Do not stop/restart/update the shared service merely because Canban exits or refreshes. Offer an explicit compatible upgrade path when necessary. |
| Canban-specific `HC_SESSION` | Deliberately launch/connect that namespace | Preserve tasks by default; app and runtime lifetime remain separate | Canban may manage its selected runtime, with an explicit migration/upgrade contract. |
| App-managed packaged core | Bundle/discover trusted binary; start detached service and holders correctly | Same durable behavior; avoid a child process killed with the UI | Packaging, signing, notices, storage, and backward compatibility become Canban release responsibilities. |

`HC_SESSION` separates core/terminal state, while config/templates remain shared unless config home is isolated too. `HC_SOCKET` alone does not isolate state. A private session is not an account/security sandbox. A machine reboot is not a core hot restart. None of these modes should silently switch the existing user's core identity or wipe state to repair a connection.

Current core `hello` supplies generation, version, methods, and features. Require the methods Canban actually uses; negotiate attach features rather than assume them from a version string. The upstream schema has additive compatibility tests against the released schema, but remains preview. Pin the investigated/released baseline for development and validate the actual installed service at connection time. No local client's current version was verified in this research.

## Scheduler and scheduled jobs remain Canban services

Core templates give commands and opening instructions. Core parent/child links and waits support agent cooperation. Neither replaces tag resolution, queue admission, per-profile concurrency, repository/worktree ownership, resource budgets, schedule deduplication, or human-review gates.

The requested familiarity with Herdr's Agents and scheduled-work UI is a UX reference, not evidence that these product services already exist in current core. Older `herdr-gpui` orchestration/automation modules and current `herdr-app` are different codebases; any later reuse of the older UI or scheduler needs its own source and dependency assessment. The MVP should not depend on an assumed core scheduler API.

A scheduler should create the same dispatch intent as a board transition, with a distinct source and scheduled occurrence ID. Do not run one scheduler in the WebView and another in Rust. Scheduled jobs can run only while the coordinator is alive; show missed-run policy explicitly. The current direction co-hosts the logical coordinator/AppServer in the Canban application process. It may remain resident independently of one window, but quitting the process stops admission/report handling. A separately managed daemon is a later extraction option if application-quit survival is explicitly needed. Persistent core processes do not make an absent Canban scheduler continue ticking.

Source uncertainty still matters for an initial launch with no reply, which can remain unknown rather than blindly retried. The user separately chose to ignore exceptional old executions after logical completion in MVP: do not retain writer/process reservations as a progression barrier. Capacity is a logical admission control, not an enforcement guarantee over every live process or descendant.

## Focused validation before implementation commitment

These are proposed acceptance experiments, not tests already executed:

- Connect the Rust adapter to an isolated core, validate `hello`, subscribe, launch a harmless command, and associate it with a card/run label.
- Kill only the scratch client connection around launch; verify reconciliation avoids a duplicate and exposes an unresolved outcome when evidence is insufficient.
- Restart a scratch core while a harmless process runs; verify terminal continuity and new snapshot/reattach behavior. Separately verify `keep` loss yields a new shell without rerunning the task.
- Exercise an OSC-reporting fixture and a non-reporting fixture; verify blocked, done, exit, and unknown status remain distinct from human acceptance.
- Test full WebView terminal attachment with resize, IME, focus, selection, output pressure, and automatic replies before promising Herdr-like terminal parity.
- Restart the Canban coordinator with a queued dispatch and a scheduled occurrence; prove the run intent is deduplicated and disconnected reservations reconcile.

Use scratch `HC_SESSION` plus scratch XDG paths throughout. Upstream explicitly forbids interfering with the user's running core/agents. These checks need no paid agent to validate the integration mechanics.
