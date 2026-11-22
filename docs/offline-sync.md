# Offline editing and synchronization (all platforms)

Status: Implemented for desktop (2026-11-22); mobile pending.

## Overview

When a desktop or mobile client loses connectivity to a remote `canband` daemon, cached Boards and Cards must remain editable rather than dropping into read-only mode. Offline mutations queue in a persistent client-side outbox and synchronize automatically when the host reconnects.

- **Scope:** Applies identically to **all platforms** (desktop and mobile) for any remote host managed by `HostRegistry`. The `local` host over Unix socket (`app.sock`) is normally always reachable and does not require offline queuing.
- **Supersedes read-only fallback note:** Extends the read-only `Offline · Cached` snapshot behavior introduced in [Multi-host daemons](multi-host.md#22-client-hostregistry-routing-and-offline-snapshot-cache).

## Operation matrix

| Operation | Offline behavior | Reconnect / sync behavior |
| --- | --- | --- |
| Create Card | Allowed; client chooses validated ID (`id`) and projects `number: 0` (`Local`) with `Not synced` badge | Replayed in order; client-chosen ID preserved and server assigns real Card `number` |
| Edit Card `title` / `body` | Allowed; updates local projection and queues pre-edit `base` (`title`/`body`) | Replayed in order; field-level 3-way auto-merge if disjoint; conflict view if same field changed |
| Add comment (`@agent` or plain) | Allowed; queued with validated client-chosen comment `id` (`pending: true`, synthetic seq `u64::MAX` excluded from timeline highwater) | Replayed in order against Card ID; server event replaces synthetic comment |
| Move Card to target stage | Allowed; Card moves optimistically in local Board cache | **Automatically applied once toward the recorded target stage without extra confirmation** |
| Resolve / Archive Card | Allowed; `card.resolve.preview` falls back to cached Card state, and Card resolves optimistically | **Automatically applied once on reconnect**; failed server rejection surfaces in Sync errors |
| Human approval / rejection gates | **Disabled online-only** (requires fresh gate/workflow state) | Available as soon as host transitions to `Connected` |
| Delivery (`Deliver…` / Queue Merge / commit) | **Disabled online-only** | Available when `Connected` |
| Interactive Terminal input | **Disabled online-only**; last screen snapshot stays visible read-only | Live PTY reattaches on `Connected` |
| Live Git Diff | **Read-only cached patch** (`timeline.list`, `git.compare`, `review.preview`, `git.file_lines`); delivery stays online-only | Prefetches active Cards' first `timeline.list` page on `Connected` |

## Client outbox and server idempotency

### Client outbox

A standalone shared synchronization crate — **`crates/sync` (`canban-sync`)** — owns the offline cache projection and persistent mutation outbox for both desktop and mobile clients.

- Each remote host entry owns an ordered durable outbox persisted atomically under `<local-home>/cache/hosts/<host_id>.outbox.json` (hard cap **500** queued operations, with crash-safe `InFlight` recovery on restart).
- Every queued mutation records:
  - `id` (`request_key`): client-generated UUID / unique token for AppServer idempotency (`operations` table).
  - `kind`: `CreateCard` (`card.create`), `UpdateCard` (`card.update` — `title`/`body` only), `AddComment` (`comment.add`), `MoveCard` (`card.move`), or `ResolveCard` (`card.resolve`).
  - `card_id`: client-chosen ID for newly created Cards (`is_valid_client_id`, rendered as `Local` while `number == 0`) or the existing remote Card ID.
  - `base` and `params`: pre-edit `title`/`body` snapshot and RPC parameters (with create+update coalescing and duplicate `id` deduplication).
- **Optimistic projection and entity ownership:** `HostRegistry::merged_snapshot()` applies pending outbox projections onto a cloned snapshot (leaving persisted `<host_id>.json` clean), updates `pending_sync` marks (`Pending` / `Conflict` / `Failed`) and `HostSummary` counts (`pending_mutations`, `sync_conflicts`), and registers remote entities in `entity_hosts` during `HostRegistry::new`, `add_host`, `apply_established_slot`, `poll_remotes_once`, and `sync_host_now` so routed mutations target the remote host immediately even before a full snapshot merge.
- **Replay order and receipts:** Mutations for a host replay sequentially (FIFO, while blocking only Cards with an unresolved `Conflict` or `Failed` item) using split lock-free `prepare_next_step`/`finish_step` draining on `Connected` transitions. Offline mutations return synthetic receipts (`"offline_queued": true`) so `Ui::dispatch_complete` surfaces `"Saved offline · will sync when <host> reconnects"` without fake failures.

### Server idempotency and compatibility

- `AppServer` already persists RPC idempotency keys in SQLite `operations` (`request_key`); offline replay reuses `request_key` (setting `retry_same_params` when a connection drops mid-call) and accepts optional validated client-chosen `id` parameters on `card.create` (`kanban::create_with_id`) and `comment.add` so no ID translation map is needed across `outbox` replay or `HostRegistry`.
- Updating Card/comment `id` and `DESKTOP_API` capabilities increments `DESKTOP_API` from `76` to **`77`** in `crates/updater/src/lib.rs`.

## UI states and conflict resolution

### Global and Card indicators

- **Global banner:** Shown at the top of Inbox / Board / workspace when a host is offline or has queued work:
  - `Offline · Cached · N pending`
  - `Syncing N…` (on reconnect)
  - `N conflicts` (when user input is needed)
- **Card badge:** Cards with un-acked local mutations show an unsynced badge (`☁̸ Local` / pending dot) on `card_tile` and in the Card header.

### Sync screen and conflict handling

```text
Sync queue                              Field conflict (Card #142)
┌──────────────────────────────┐        ┌──────────────────────────────┐
│ Sync                         │        │ Description changed on server│
│ ✓ Synced 5                   │        │                              │
│ ⏳ Pending 2  New onboarding…│        │ Server version:              │
│ ⚠ Conflict 1  Fix login bug ›│ ─────▶ │ …handle session expiry…      │
└──────────────────────────────┘        │                              │
                                        │ Your offline edit:           │
                                        │ …handle session expiry + log…│
                                        │                              │
                                        │ [Use server] [Mine] [Merge]  │
                                        └──────────────────────────────┘
```

- **Disjoint fields auto-merge:** If the server Card `revision` advanced while offline, but server edits modified different fields than the offline mutation (for example, server updated `stage` or status while the client edited `description` or appended a comment), the client automatically rebases onto the new `revision` and retries.
- **Same-field conflicts:** When both server and offline edits changed the same field (such as `title` or `description`), the Sync screen opens a side-by-side / stacked comparison with `[Use server]`, `[Use mine]`, and `[Edit merged]`, reusing the existing explicit revision-adoption pattern in [Card creation and editing](card-editor.md#persistence-and-failure-handling).
- **Rejected moves / archives:** If a queued stage move or resolve/archive cannot execute on reconnect (e.g., Card was deleted remotely, target column changed on workflow publication, or a workflow rule rejects the transition), the item moves to the Sync error/conflict list with clear server reason plus `[Retry]` and `[Discard]` actions.

## Offline cache bounds

- **Default scope:** Cache snapshots for **all paired Projects** together with `timeline.list`, `git.compare`, `review.preview`, and `git.file_lines` responses by default under `<local-home>/cache/hosts/<host_id>/reads/`.
- Editable offline Board projections extend `<local-home>/cache/hosts/<host_id>.json` without overwriting persisted base snapshots with synthetic outbox entries ([Multi-host daemons](multi-host.md#22-client-hostregistry-routing-and-offline-snapshot-cache)).
- Per-Card `Timeline` history and recently viewed Diff patches are **never** embedded in `Snapshot` (per [UI architecture](ui-architecture.md)); `ReadCache` enforces a **64 MiB LRU byte budget** per host (`DEFAULT_READ_CACHE_BUDGET_BYTES`) and background-prefetches the first `timeline.list` page for active Cards on `Connected`.

## Decisions (2026-11-22)

- **Crate boundary:** Extract a standalone `crates/sync` (`canban-sync`) crate shared by desktop and mobile clients.
- **Cache scope:** Cache all paired Projects' snapshots and Card timelines by default (within bounded pages and 64 MiB LRU byte budgets per host).
- **Offline Diff cache:** Cache recently viewed Git Diff patches for read-only offline viewing within the same 64 MiB LRU cache; delivery actions remain online-only.

### Acceptance (2026-11-22)

- **Workspace checks and tests:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo nextest run --workspace --locked` passed (`806 passed, 2 skipped` where the 2 skipped tests are pre-existing `#[ignore]` targets in `crates/profiles/src/launch.rs` and `crates/ui/src/board/performance.rs`).
- **Offline sync & UI coverage:** End-to-end tests in `crates/sync`, `crates/kanban`, `crates/app-server` (`multi_host::host_registry_merges_multi_host_routes_mutations_caches_offline_and_recovers`, `multi_host::host_registry_detects_same_field_conflict_and_resolves_via_use_mine`, `representations::client_chosen_card_and_comment_ids_validate_reject_duplicates_and_replay_idempotently`), and `crates/ui` (`presenter::tests::pending_synthetic_events_do_not_advance_timeline_highwater`, `components::card_tile::tests::card_tile_renders_local_number_and_sync_chips`, `hosts::tests::trigger_pill_and_offline_banner_reflect_multi_host_state`, `hosts::tests::sync_modal_renders_failed_and_conflict_actions`) verified offline queueing, read cache fallback, entity-ownership registration, reconnect FIFO drain, 3-way conflict detection/resolution (`UseMine`), UI chips (`Local`, `Not synced`, `Sync issue`), and the `Sync` modal.
- **Gallery & isolated dev-app limits:** `mise run build-gallery` succeeded and `.local/CanbanGallery.app/Contents/MacOS/gallery` launched cleanly without panics; CUA `get_window_state` on `CanbanGallery` returned `elements=0` with no AX tree or screenshot captured in this environment. `mise run dev` compiled `canban-ui`, `canban-cli`, and `canban-updater`, then exited at `scripts/dev.py`'s safety guard (`CANBAN_ENDPOINT is attach-only; development commands will not manage that service`) because the session environment exports `CANBAN_ENDPOINT`; interactive dev GUI offline-reconnect driving was therefore not executed against the live window in this session.
