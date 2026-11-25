# Deterministic Simulation Testing (DST)

Canban's `canban-sim` crate (`crates/sim/`) runs full-stack, multi-host deterministic simulations of the data layer, workflow engine, scheduler, cron schedules, GitHub sync, Herdr observation, notifications, idle reclamation, environment/secret sync, and unified client layer (`canban-client::MultiHostClient`) inside a single-threaded [`turmoil`](https://github.com/tokio-rs/turmoil) simulation stepped deterministically via `sim.step()`.

## Architecture and OS-Boundary Abstractions

The simulation boundary pushes up to `canban-client::MultiHostClient` (the shared client used by Desktop, Mobile, and CLI) running across 1..N real `AppServer` instances backed by real SQLite databases, while intercepting every external OS boundary behind deterministic, seed-driven seams:

1. **Deterministic Network & Stream Transport (`turmoil` + `TurmoilConnector` + `FaultyStream`)**:
   - `turmoil` is strictly confined to `crates/sim` (zero leakage into desktop, mobile, or CLI binaries).
   - Each simulated `AppServer` runs inside a Turmoil host with `ServerRuntimeEnv { inline_workers: true, .. }` so RPC handling, delivery workers, runtime observation, cron schedule evaluation, and GitHub sync execute deterministically inside Turmoil's per-host `current_thread` Tokio runtime without OS background thread pools racing the virtual clock.
   - `MultiHostClient` instances (`"ui-client"` plus optional `extra_clients`) run in their own Turmoil hosts via `MultiHostClient::from_options_with_runtime`, connecting to simulated servers through `TurmoilConnector` (`HostConnector`) and `FaultyStream` over `turmoil::net::TcpStream` / `TcpListener`.
   - Network faults (`partition`, `partition_oneway`, `hold`/`release`, `set_fail_rate`, per-link latency/fail rates, `StreamFaultPolicy` byte-stream drops/truncation/stalls) and process faults (`crash_server` abrupt SIGKILL, `stop_server_graceful`, `bounce_server`, `crash_client`, `bounce_client`) are injected deterministically.
   - `FsDurability` (`NoSync` / `FullSync`) and optional SQLite `synchronous = OFF` keep simulation I/O fast while preserving real SQLite schema/foreign-key semantics.
2. **Hybrid Instance-Scoped Virtual Clock (`TurmoilClock`)**:
   - Turmoil natively drives all `tokio::time` timers (`sleep`, `interval`, `timeout`, `tokio::time::Instant`).
   - Wall-clock epoch timestamps (`created_at`, `updated_at`, `completed_at`, LWW env timestamps, reminder/recovery windows, viewer presence leases, idle reclaim deadlines, and `AgentRun::runtime_status_at`) use an instance-scoped `Arc<dyn canban_types::Clock>` (`TurmoilClock`) with optional per-host clock skew and zero global mutable state.
   - Deterministic ID generation (`IdGenerator`) ensures repeatable entity creation without host entropy leakage, and `oracle::canonical_state_digest(&world)` produces a UUID-independent state fingerprint across all live hosts.
3. **Fail-Closed Subprocess Seam (`SimExec` / `canban_exec::CommandRunner`)**:
   - All CLI probing and subprocess execution inside simulated servers routes through `SimExec` (`crates/sim/src/exec.rs`), seeded with simulated `config/agents.json` profiles using `/bin/echo`.
   - Scripted `git` repository fixtures (`SimGitRepo`) answer metadata/worktree queries deterministically, while any unexpected OS binary (`gh`, `ssh`, `hc`, `python3`, `bash`, etc.) fails closed and is recorded in `blocked_commands()`.
4. **External Service Simulators (`SimHerdr` & `SimGitHub`)**:
   - **`SimHerdr`** (`crates/sim/src/herdr.rs`, implementing `canban_herdr::HerdrBackend`): full in-memory Herdr Core simulator with terminal lifecycle, `working`/`idle`/`done`/`blocked` status & turn events, `terminal.read` output/screen buffers, stale/duplicate/dropped/reordered `seq` injection, `missed` snapshot resync, subscriber overflow/disconnects, `set_offline(bool)`, Core identity restarts (`restart_core`), `RunFault` launch failures, `hang_exit`/`hang_kill` timeouts, and recorded `sent_inputs` / `killed_terminals`.
   - **`SimGitHub`** (`crates/sim/src/github.rs`, implementing `canban_github::HttpTransport`): full in-memory GitHub REST API simulator with repos, per-login user IDs, collaborator lists, issues, PRs, reviews/comments, CI check runs, real `page`/`per_page` pagination with `Link: rel="next"` headers, ETag / HTTP `304 Not Modified`, primary (`403` reset) and secondary (`429` `Retry-After`) rate limits, `inject_stale_response_once`, per-host offline/fault injection, and exact HTTP request counters.

## Universal Invariant Oracle

`InvariantOracle` (`crates/sim/src/oracle.rs`) verifies cross-cutting safety and liveness properties both after every step (`check_step_invariants`) and at quiescence (`check_quiescent_invariants`):

- **Data & Revision Safety**: monotonic `Card.revision`, unique entity IDs across all servers and `merged_snapshot()`, `entity_hosts` mapping integrity, bounded snapshot projections, and committed-write durability tracking.
- **Notifications & Reminders**: monotonic `CardRead.sequence`, exact parity between `snap.reminders` and `reminders_from_events_at(..., server.now_ms())`, zero stale reminders on archived/removed Cards, unique `sound_event_ids`, and non-resurrection across partitions and server restarts.
- **Security & Secret Hygiene**: recursive plaintext secret scanning across server snapshots, timelines, and client `.outbox.json` files on disk (`oracle.outbox_file_scanned`), asserting offline secret saves were actually scanned.
- **Scheduler, Herdr & Workflow Integrity**: `active_count <= max_agents`, Herdr runtime parity, exclusive terminal bindings, `protected_terminals` safety (zero unsolicited input or kills while protected), single active workflow run per Card, and `waiting_approval => human_gate.is_some()`.
- **UI Terminal Invariants**: immediate Project placement on `terminal.create` (`assert_terminal_in_project`) and 4-tier close/exit fallback priority (`same worktree -> same project -> same host -> cross host`, `assert_close_fallback_priority`).
- **Quiescence & Durability**: zero remaining `Queued`, `InFlight`, or unexpected `Failed` outbox items (`check_outbox_drained`), full client-to-server convergence across connected hosts (projects, Cards, workflows, terminals, card reads, reminders, global env), `PRAGMA foreign_key_check` + `PRAGMA quick_check` on every SQLite database (`oracle.sqlite_integrity_checked`), and post-bounce startup recovery (`zero AgentRun stuck in "starting"`, `zero Question stuck in "delivering"`, `oracle.post_bounce_recovery_checked`).

## Scenarios

`canban-sim` ships 11 focused scripted scenarios plus a Swarm-Testing randomized chaos generator with deterministic shrinking:

1. **`card_automation`**: Sequential v1->v12 workflow schema migration, v12 YAML publication, manual & `card.column_entered` triggers, idempotent `request_key`, orthogonal Herdr `done` vs explicit `execution.report`, blocked -> `card.continue` resume, approval `rework` loop and `approve`, seed-varied mid-workflow partition & server crash/bounce, and `workflow.stop`.
2. **`remote_faults`**: Bidirectional & one-way partitions, FIFO outbox ordering with zero duplicates, true mid-flight drops (`client_spawn` + one-way drop), 3-way disjoint-field auto-merge, same-field conflict resolution across `SyncAction::UseMine`, `Retry`, and `UseEdited`, client crash/bounce recovery reloading `.outbox.json` from disk, reconnect-window & instant-disconnect readability, and concurrent external update/archive.
3. **`github_sync`**: Issue/PR intake, idle sync idempotency (zero Card revision churn, zero duplicate events, ETag `304` reuse, bounded HTTP volume), rapid `github.sync.request` coalescing, primary (`403`) & secondary (`429`) rate-limit recovery, `inject_stale_response_once` resilience, multi-page `Link: rel="next"` pagination, and collaborator vs `external_pr` trust gating.
4. **`client_pages`**: Multi-host Desktop & Mobile `MultiHostClient` routing, immediate write-receipt hydration, colliding raw Card IDs across `remote-a` and `remote-b`, `card.list` cursor pagination + `cursor_stale` detection, `timeline.list` forward pagination, and offline read-cache + pending comment merge.
5. **`env_sync`**: Desktop Global Env propagation under per-host clock skew, real competing LWW writes across skewed hosts (stale push rejection + newer-timestamp convergence), Project-scope `Unset` override verification via `env.preview`, secret rotation, deletion tombstones across crash/bounce, and zero plaintext secret leaks.
6. **`notifications`**: Real blocked/approval/comment reminder generation on BOTH `local` and `remote-a`, raw vs host-qualified ID matching, `CardRead.sequence` advancement via `notifications.read`, `Notice.read` clearance via `card.attention.clear`, `notifications.read_all` across partition & server bounce without resurrection, and archived Card reminder suppression.
7. **`herdr_lifecycle`**: `working`/`idle`/`done`/`blocked` transitions, `terminal.read` / `execution.output` capture, stale `seq` rejection, subscriber drops + `missed` snapshot resync, `herdr.set_offline(true)` outage recovery, `AppServer` `bounce_server` live-terminal reattachment, Herdr Core restart, `max_agents` slot release on `blocked`, and UI terminal-to-project + close-fallback invariants.
8. **`reclaim`**: Active viewer protection (`oracle.protect_terminal`), post-view grace expiry, `bounce_server` during `closing`, stuck `/exit` 30-second timeout escalation to `terminal.kill`, and manual Shell immunity.
9. **`schedules_and_budget`**: Tight `max_agents: 1` stage-priority & FIFO queueing, Herdr-offline queued-terminal livelock guard, virtual-clock cron schedule ticks (`overlap: skip` and `overlap: queue`, card-creating and cardless runs), and `ScheduledRun` persistence across server bounce.
10. **`workflow_faults`**: Mid-flight server crash & partition during active agent execution and `waiting_approval` gates, `condition` branching (`source_trust == "external_pr"` vs trusted local), `notify` steps, `rework` + `approve`, `workflow.stop`, and post-bounce recovery checks.
11. **`multi_client_sync`**: Two concurrent clients (`ui-client` + `ui-client-2` via `SimWorldConfig.extra_clients`) racing via `client_spawn_on`, 2-client 3-way disjoint-field auto-merge, 2-client same-field conflict resolution, dirty `.outbox.json` recovery across `crash_client_named` / `bounce_client_named`, and unassisted `quiesce_natural` reconnect/drain.
12. **`chaos`** (Swarm-Testing DST + Shrinker): Per-seed randomized `ChaosProfile` (Desktop vs Mobile topology, clock skews, latencies, fail rates, `max_agents`, enabled fault families, optional workflow+GitHub co-location, randomized op weights) materializing a deterministic `Vec<ChaosOp>`, explicit expected-error classification (no silent error swallowing, no silent `SyncItemState::Failed` discarding), step-invariant checks after **every** op, and prefix binary search + bounded delta-debugging (`chaos::shrink`) to minimize any failing seed.

## Coverage & Boundaries

| Domain | Covered in `canban-sim` (DST) | Verified Elsewhere (Native / Integration / Gallery) |
| --- | --- | --- |
| Multi-host RPC routing, offline outbox, 3-way merge, conflict actions | Full (`remote_faults`, `multi_client_sync`, `client_pages`, `chaos`) | Live LAN/SSH transport acceptance (`docs/multi-machine-acceptance.md`) |
| Workflow engine (`v1..v12` migration, `agent`, `approval`, `rework`, `condition`, `notify`, `card.move`, `stop`) | Full (`card_automation`, `workflow_faults`, `chaos`) | Real-model Codex/Claude workflow acceptance (`mise run acceptance-real-workflows`) |
| Scheduler budget, stage priority, cron schedules (`skip`/`queue`), idle reclaim & stuck-exit kill | Full (`schedules_and_budget`, `reclaim`, `herdr_lifecycle`, `chaos`) | Real Herdr PTY & shell fixture integration (`mise run acceptance`) |
| GitHub sync (ETag 304, pagination, rate limits, stale reads, collaborator/`external_pr` trust) | Full (`github_sync`, `workflow_faults`, `chaos`) | Live GitHub OAuth / real API smoke checks |
| Global/Project Env sync, LWW under clock skew, `Unset` overrides, secret leak scanning | Full (`env_sync`, `chaos`) | Real Keychain/vault platform integration |
| Notifications, `CardRead.sequence`, `Notice.read`, non-resurrection across crash/bounce | Full (`notifications`, `chaos`) | Desktop audio & native UI notification affordances |
| SQLite WAL integrity, foreign keys, abrupt SIGKILL recovery, client `.outbox.json` reload | Full (universal oracle across all scenarios) | Schema upgrade & rollback bundle tests (`scripts/test-release-upgrade.py`) |
| Native UI layout, NSH/AppKit/GTK4 rendering, `libghostty-vt` IME/clipboard, macOS signing | Pure UI helpers (`entries`, `fallback_after_close`) only | `mise run gallery`, `mise run test-package`, `mise run dev` native checks |
| Real OS subprocesses (`uses: script`, real `git worktree` merge conflicts, real `hc` binary) | Explicitly blocked (`SimExec` fail-closed + `SimGitRepo` fixtures) | `mise run test-integration`, `mise run acceptance-interactive` |

## Commands & CI Gate

```bash
# Run one complete deterministic simulation round (all 11 scripted scenarios + chaos)
mise run sim

# Run with strict property coverage assertions and per-seed watchdog timeout
cargo run --release --locked -p canban-sim --bin canban-sim -- run --seeds 1 --assert-coverage --seed-timeout 60

# Replay a specific seed / scenario deterministically
mise run sim -- --seed 42 --scenario chaos --ops 40
mise run sim -- --seed 100 --scenario multi_client_sync --seeds 5

# Replay all previously recorded failing seeds from a failure log index
cargo run --release --locked -p canban-sim --bin canban-sim -- run --replay-file .local/sim-failures/failed-seeds.txt

# Shrink a failing chaos seed down to a minimal reproducing op sequence
cargo run --release --locked -p canban-sim --bin canban-sim -- shrink --seed 42 --ops 40
# Or auto-shrink automatically on failure during `run`:
cargo run --release --locked -p canban-sim --bin canban-sim -- run --scenario chaos --seed 42 --ops 40 --shrink

# Continuously run random-seed DST in release mode until Ctrl-C
mise run random-sim

# Parallel, reproducible, coverage-checked batch (same command used in GitHub Actions CI)
cargo run --release --locked -p canban-sim --bin canban-sim -- random \
  --scenario all --seeds 12 --jobs 4 --master-seed 2026 --seed-timeout 60 --assert-coverage

# Deep chaos fuzzing batch with coverage assertions
cargo run --release --locked -p canban-sim --bin canban-sim -- random \
  --scenario chaos --seeds 20 --jobs 4 --ops 40 --master-seed 2026 --seed-timeout 60 --assert-coverage

# Run the curated fast DST unit & scenario suite via nextest
mise run test -- -E 'package(canban-sim)'
```

CI (`.github/workflows/ci.yml`) runs a release-mode `canban-sim random --scenario all --seeds 12 --jobs 4 --master-seed 2026 --seed-timeout 60 --assert-coverage` gate on both macOS and Linux runners, uploading `.local/sim-failures/` artifacts whenever a seed fails or times out. Every failure writes a non-colliding trace log (`<scenario>-seed-<N>-ops-<M>.log`), appends an exact replay entry to `.local/sim-failures/failed-seeds.txt`, and prints the exact replay command.
