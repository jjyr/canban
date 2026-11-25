# Deterministic Simulation Testing (DST)

Canban's `canban-sim` crate (`crates/sim/`) runs full-stack, multi-host deterministic simulations of the data layer, workflow engine, scheduler, GitHub sync, Herdr observation, notifications, idle reclamation, and unified client layer (`canban-client::MultiHostClient`) inside a single-threaded [`turmoil`](https://github.com/tokio-rs/turmoil) simulation stepped deterministically via `sim.step()`.

## Architecture and Boundary

The simulation boundary pushes all the way up to `canban-client::MultiHostClient` (the single shared client used by Desktop, Mobile, and CLI) on top of 1..N real `AppServer` instances backed by real SQLite databases:

- **Deterministic Network & Execution (`turmoil`)**:
  - `turmoil` is strictly confined to `crates/sim` (zero leakage into desktop, mobile, or CLI binaries).
  - Each simulated `AppServer` runs inside a Turmoil host with `ServerRuntimeEnv { inline_workers: true, .. }` so RPC handling, delivery workers, runtime observation, and GitHub sync execute deterministically inside Turmoil's per-host `current_thread` Tokio runtime without OS background thread pools racing the virtual clock.
  - `MultiHostClient` runs inside the `"ui-client"` Turmoil host via `MultiHostClient::from_options_with_runtime`, connecting to simulated servers through `TurmoilConnector` (`HostConnector`) and `TransportStream::Boxed` over `turmoil::net::TcpStream` / `TcpListener`.
  - Network faults (`partition`, `partition_oneway`, `hold`/`release`, `set_fail_rate`) and process faults (`crash_server`, `bounce_server`) are injected directly through Turmoil.
- **Hybrid Scoped Clock**:
  - Turmoil natively drives all `tokio::time` timers (`sleep`, `interval`, `timeout`, `tokio::time::Instant`).
  - Wall-clock epoch timestamps (`created_at`, `updated_at`, `completed_at`, LWW env timestamps, viewer presence leases, idle reclaim deadlines, and `AgentRun::runtime_status_at`) use an instance-scoped `Arc<dyn canban_types::Clock>` (`TurmoilClock`), with optional per-host clock skew and zero global mutable state.
- **Deterministic External World Seams**:
  - `SimHerdr` (`canban_herdr::HerdrBackend`): in-memory Herdr Core simulator with full terminal lifecycle, status/turn events, `blocked` waits, stale/duplicate `seq` injection, `missed` snapshot resync, subscriber disconnects, Core identity restarts, `status_mismatch` races, and recorded `sent_inputs` / `killed_terminals`.
  - `SimGitHub` (`canban_github::HttpTransport`): in-memory GitHub REST API simulator with repos, issues, PRs, CI check runs, ETag / HTTP `304 Not Modified`, exact HTTP request counters, and injected `403`/`500`/stale response faults.

## Scenarios and Universal Invariant Oracle

`InvariantOracle` (`crates/sim/src/oracle.rs`) checks cross-cutting invariants both mid-fault (`check_step_invariants`) and after healing (`check_quiescent_invariants`):

- Monotonic `Card.revision` and unique entity IDs across every server and `MultiHostClient::merged_snapshot()`.
- Zero duplicate notices or reminders, and zero resurrected read/archived notifications across reconnects and server restarts.
- Zero plaintext secret leaks in `.outbox.json` on disk.
- `merged_runtime_state()` active-agent count parity across local and remote hosts.
- Immediate Project placement on `terminal.create` (pure UI `canban_ui::terminals::entries` + `worktrees::entry_scope` never transiently place a newly created project terminal under `"Other"`), and strict 4-tier close/exit fallback priority (`same worktree -> same project -> same host -> cross host`) via `canban_ui::terminals::fallback_after_close`.
- Full outbox drain and client-to-server convergence at quiescence.

All 8 product domains plus an interleaved chaos workload and a determinism self-test are covered:

1. **`card_automation`**: Workflow YAML publication, manual & `card.column_entered` triggers, idempotent `request_key`, orthogonal Herdr `done` vs explicit `execution.report`, blocked -> `card.continue` resume, approval gate, and automatic stage progression.
2. **`remote_faults`**: Partitioned & one-way partitioned offline mutations, FIFO outbox queueing & replay without duplication or reordering, concurrent server edit conflict detection & resolution, and server crash/bounce recovery.
3. **`github_sync`**: Initial Issue/PR intake, idle sync idempotency (zero Card revision churn, zero duplicate events, ETag `304` reuse, bounded HTTP volume), rapid `github.sync.request` coalescing, HTTP `403` rate-limit recovery, and in-place PR updates.
4. **`client_pages`**: Multi-host Desktop & Mobile `MultiHostClient` routing, qualified IDs, `timeline.list`, `card.list`, `merged_snapshot()`, `merged_runtime_state()`, and disconnected cache fallback.
5. **`env_sync`**: Desktop Global Env propagation to partitioned/skewed remote hosts, per-host LWW conflict convergence, scoped Project/Profile env isolation, and zero plaintext secret leaks in `.outbox.json`.
6. **`notifications`**: Multi-host reminder deduplication, durable `notifications.read` / `notifications.read_all`, archived Card stale-reminder suppression, and non-resurrection across partitions and server restarts.
7. **`herdr_lifecycle`**: `working` -> `idle` -> `done` -> `blocked` wait notice lifecycle, duplicate & stale `seq` rejection, `missed` snapshot resync, subscriber disconnect recovery, Core restart (`replaced`), `lost`/`finished` exit classification, runtime count parity, immediate project placement on `terminal.create`, and multi-host 4-tier close/exit fallback priority.
8. **`reclaim`**: Automatic idle reclamation safety under active `viewer.presence` (`paused = true`, zero sends/kills), clean rollback on Herdr `status_mismatch` race without stuck `closing` or false error attention, and liveness once unviewed and idle (`/exit` -> `idle_closed`).
9. **`chaos`**: Weighted random interleaving of all 8 subsystems combined with Turmoil network/process faults and continuous oracle checks, plus a same-seed determinism digest self-test.

## Commands

```bash
# Run one complete deterministic simulation round (all 8 scenarios + chaos)
mise run sim

# Replay a specific seed / scenario deterministically
mise run sim -- --seed 42 --scenario chaos
mise run sim -- --seed 100 --scenario remote_faults --seeds 5

# Continuously run random-seed DST in release mode until Ctrl-C
# (prints every failing seed + replay command and keeps going)
mise run random-sim

# Stop immediately on the first failing seed (`--fail-fast` / `-x`)
mise run random-sim -- --fail-fast

# Parallel / bounded random-seed runs (e.g. 4 workers, 50 seeds, or 5 minutes)
mise run random-sim -- --jobs 4
mise run random-sim -- --seeds 50 --jobs 4 --fail-fast
mise run random-sim -- --duration 300 --jobs 4 --scenario all

# Run the curated fast DST suite via nextest
mise run test -- -E 'package(canban-sim)'
```

When `mise run random-sim` hits an invariant violation, it immediately prints the failing `seed` and exact `mise run sim -- --seed <N> --scenario <name>` replay command, saves the failure log to `.local/sim-failures/seed-<N>.log` (and appends to `.local/sim-failures/failed-seeds.txt`), and keeps exploring new seeds until Ctrl-C (unless `--fail-fast` / `-x` is set). When stopped, it prints a full summary of any failed seeds and exits non-zero if any failed.
