# Unified client architecture (`canban-client`)

Status: **Implemented (2026-11-25) — Desktop UI, Mobile UI (`canban-ios`, `canban-android`), and CLI all run on the single `canban-client` layer; the legacy `HostRegistry` multi-host engine in `canban-app-server` and duplicate `LocalBackend` in `canban-ui` have been removed.**

## Problem

Client responsibilities are currently split across three separate implementations:

1. **Mobile (`crates/client::RemoteClient`, ~3.7k LOC):** pure-remote async multi-host client, push subscriptions, offline sync (`canban-sync`), and pairing.
2. **Desktop (`crates/app-server::HostRegistry`, ~4.3k LOC):** multi-host registry living inside the server crate, duplicating handshake, push/reconnect supervision, snapshot/runtime merging, and offline outbox replay while adding `"local"` Unix socket management and SSH tunnels.
3. **CLI (`crates/app-server::client::AppClient`):** standalone single-host RPC and startup handshake client.

Because protocol handshakes, authentication, and multi-host synchronization are implemented in multiple places, fixes drift across platforms (for example, desktop migrated to `app.ping` while mobile still called `host.probe`, and mobile accepted bare Tailscale addresses while desktop's URI tab still required `#<token>`). Meanwhile, desktop UI code in `crates/ui/src/hosts.rs` scatters `#[cfg(feature = "local-host")]` branches across user-facing interaction callbacks.

## Target layering

All clients—Desktop UI (macOS/Linux), Mobile UI (iOS/Android), and CLI (`canban`)—consume a single **`canban-client`** layer built directly on top of **`canban-transport`** and **`canban-sync`**:

```text
  Desktop UI (macOS/Linux)       Mobile UI (iOS/Android)       CLI (`canban`)
              \                             |                        /
               ▼                            ▼                       ▼
                         canban-client (single client core)
        ┌──────────────────────────────────────────────────────────────┐
        │  SingleHostSession   (lightweight handshake + RPC for CLI)   │
        │  MultiHostClient     (routing, push, merge, outbox, pairing) │
        └──────────────────────────────────────────────────────────────┘
                                            │
                       HostConnector + HostStore + Capabilities
                                            │
                                            ▼
                         canban-transport (pure wire layer)
        Noise_XX E2EE · pubkey / pair_... / Tailscale WhoIs auth · yamux/1 mux · PTY relay
```

### 1. Transport vs Connector

- **`canban-transport`** remains the pure wire layer (zero subprocesses, zero SQLite, zero platform-specific daemon management): `Noise_XX_25519_ChaChaPoly_BLAKE2s` E2EE framing, X25519 public-key / pairing / Tailscale WhoIs authorization, revocation enforcement, and `yamux/1` stream multiplexing.
- **`HostConnector`** abstracts turning a `HostTransport` specification (`Tcp`, `Local`, `Ssh`) into a ready `ClientTransport` plus an optional lifecycle `Lease`:
  - **`TcpConnector` (built-in on all platforms):** dials TCP, completes `Noise_XX` with pinned host key verification (`?pk=`) or Tailscale TOFU, negotiates `yamux/1`, and authenticates via static client public key / `pair_...` / Tailscale WhoIs.
  - **`LocalUnixConnector` (desktop):** connects to `<home>/app.sock`, optionally spawns or activates the local `canban serve` daemon, and performs local version/API checks.
  - **`SshConnector` (desktop):** opens an SSH tunnel via `canban-exec`, exposes the forwarded endpoint as a `ClientTransport`, and returns a `Lease` whose `Drop` cleans up the tunnel process.

Once a `ClientTransport` is established, every higher-level behavior—`app.ping` + `app.state` handshake, `server_id` identity verification, `app.subscribe` push + polling fallback, `Snapshot`/`RuntimeState` merging, entity-host routing, `RemotePtyTransport`, and `canban-sync` offline outbox/read-cache/3-way conflict resolution—runs through one shared implementation.

### 2. Compile-time features vs runtime capabilities

Keep compile-time Cargo features and runtime capabilities strictly separated:

- **Cargo features (dependency & sandbox pruning only):**
  - **Always built-in (no feature gate):** `TcpConnector`, `Noise_XX` E2EE, `yamux/1`, Tailscale same-account protocol auth, bare Tailscale address & `canban://` (`?pk=`) URI parsing, `host.discover_canban`, and `canban-sync`.
  - **Optional features (desktop / CLI only):**
    - `local-daemon`: `LocalUnixConnector`, local daemon spawn/handoff, `local` host registration.
    - `ssh`: `SshConnector`, non-interactive SSH auto-pairing, and remote host bootstrap (via `canban-exec`).
    - `local-discovery`: local `tailscale status` CLI peer enumeration and `~/.ssh/config` parsing.
    - `updater`: desktop self-update and remote daemon upgrade checks (via `canban-updater`).
  - Mobile builds (`canban-ios`, `canban-android`) compile `canban-client` with `--no-default-features` and remain completely free of `canban-app-server`, `rusqlite`, `canban-exec`, and `canban-updater`.

- **Runtime `ClientCapabilities` (UI & orchestration decisions):**
  - Instead of scattering `#[cfg(feature = "local-host")]` across UI widgets, `canban-client` exposes a small `Capabilities` descriptor and optional capability handles (e.g. `ssh()`, `local_discovery()`, `updater()`):
    - `local_host: bool`
    - `ssh: bool`
    - `local_tailscale_discovery: bool`
    - `self_update: bool`
  - UI screens query capabilities at runtime to decide whether to render the SSH tab, local peer discovery, or updater actions, and tests can exercise any capability combination with in-memory mocks.

### 3. CLI fast path

Short-lived CLI commands (such as Agent calls `canban card`, `canban reply`, `canban report`, `canban status`) must remain millisecond-fast and must not spawn background multi-host supervisors or hydrate full offline caches. `canban-client` therefore exposes two entry points sharing the same protocol, auth, and error-handling primitives:

- **`SingleHostSession`:** lightweight on-demand connection for CLI commands and one-shot probes.
- **`MultiHostClient`:** long-lived supervised client for Desktop and Mobile UIs.

### 4. Persistence boundaries

`canban-client` depends on narrow storage traits rather than hard-coding desktop paths:

- **`HostStore` / `CredentialStore`:**
  - Desktop adapter persists host entries in `<home>/config/hosts.json` and caches under `<home>/cache/`.
  - Mobile adapters persist hosts and sealed tokens via iOS Keychain or Android private app storage (`FileCredentialStore`).
  - Tests use `MemoryCredentialStore` and temporary directories.

## Staged migration plan

Execute incrementally without breaking running desktop, mobile, or CLI flows:

1. **Stage 1 — Extract shared single-host protocol & `SingleHostSession` into `canban-client`:**
   - Move single-host handshake (`app.ping`, `server_id` verification, token upgrade) and endpoint connection logic out of `crates/app-server/src/client.rs` into `canban-client`.
   - Point CLI commands and `RemoteClient` at `SingleHostSession` so all platforms share one RPC/auth handshake.
2. **Stage 2 — Introduce `HostConnector`, `HostStore`, and `ClientCapabilities` in `canban-client`:**
   - Generalize `RemoteClient` into `MultiHostClient` backed by pluggable `HostConnector` (`TcpConnector` built-in) and `HostStore`.
   - Define `ClientCapabilities` and optional desktop capability traits (`ssh`, `local-discovery`, `updater`) behind Cargo features.
3. **Stage 3 — Implement desktop connectors & replace `HostRegistry`:**
   - Implement `LocalUnixConnector` (managing `"local"` daemon lifecycle) and `SshConnector` (tunnel `Lease` + SSH pairing) in the desktop adapter.
   - Switch desktop `canban-ui` (`LocalBackend`) to `MultiHostClient`, remove `crates/app-server/src/hosts.rs` duplication, and keep `canban-app-server` focused solely on server runtime, storage, workflows, and RPC handlers.
4. **Stage 4 — Simplify `crates/ui/src/backend.rs` and UI capability checks:**
   - Collapse duplicate `LocalBackend` / `RemoteBackend` glue in `crates/ui/src/backend.rs`.
   - Replace scattered `#[cfg(feature = "local-host")]` blocks in `crates/ui/src/hosts.rs` with `ClientCapabilities` queries and add cross-capability unit tests.
