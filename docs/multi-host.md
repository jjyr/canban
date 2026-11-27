# Multi-host Canban daemons, Tailscale/SSH connections and Diagnostics

Status: **User-confirmed design and active implementation (updated 2026-11-27)**.

This contract defines how one native `Canban.app` client connects simultaneously to its local `AppServer` (`canband`, invoked as `canban serve`) and one or more remote `canband` instances (such as an always-on home Linux server over Tailscale or an SSH-reachable host). See [Multi-machine execution](multi-machine.md) for the earlier single-server remote-Herdr layer.

---

## 1. Product goals and core mental model

1. **Offline-resilient autonomous progression:** Each host runs a self-contained `canband` + local `herdr-core` (`hc`) pair owning its own SQLite WAL database, Projects, Cards, Workflow Engine, Scheduler, and Git worktrees. When the user's MacBook sleeps or goes offline, remote `canband` hosts continue advancing Cards, collecting Agent reports, running Queue Merge, and polling linked GitHub repositories without interruption.
2. **Simultaneous multi-host client (zero context-switching friction):** `Canban.app` connects to `This Mac` (`local`) and configured remote hosts concurrently—mirroring Herdr's multi-Core mental model. Local Projects remain live when offline; remote Projects seamlessly stream updates and surface `Needs you` / `Updates` items in the global **Activity & Notifications** (`Command+I`) popover as soon as connectivity returns.
3. **One machine concept in the UI:** A Project lives on the `canband` host where it was created. Users no longer need to juggle separate "AppServer host" versus "Herdr execution machine" concepts in normal workflows: adding a machine connects (and optionally bootstraps) `canband` + `hc` on that host.
4. **Tailscale-first direct data plane with SSH discovery/bootstrap/fallback:**
   - Users select machines directly from discovered **Tailscale peers** (`tailscale status --json`) or **SSH hosts** (`~/.ssh/config` or `user@host`).
   - Whenever a host is reachable over TCP (Tailscale, LAN, or another private/public route), JSON-RPC calls, `app.subscribe` push streams, and multiple concurrent `herdr.connect` live Terminal byte streams share a **single persistent TCP connection encrypted end-to-end with `Noise_XX_25519_ChaChaPoly_BLAKE2s`**, multiplexed with `yamux/1` (`transport.mux`), and authenticated by **Noise X25519 client public keys** (authorized via single-use `pair_...` codes, SSH pairing, or Tailscale `WhoIs` auto-auth)—avoiding SSH tunnel overhead and recovering immediately on laptop wake.
   - SSH is used both for **one-click remote bootstrap / pairing / daemon upgrades** (especially on Linux hosts) and as a **managed Unix-socket tunnel fallback** for non-Tailscale SSH hosts.
5. **First-class Linux daemon support & built-in Diagnostics:** Remote `canband` runs headlessly on Linux (`x86_64` and `aarch64`) as well as macOS, with `systemd --user` (+ linger) service installation and an integrated **Settings → Diagnostics** page for end-to-end transport, version, runtime, toolchain, and log inspection.

---

## 2. System architecture

```text
                    Native Canban.app (macOS Client)
     +------------------------------------------------------------+
     |  Ui State + Merged Snapshot (Projects/Cards/Terminals)     |
     |  HostRegistry (~/.canban/config/hosts.json + disk caches)  |
     +-------+-----------------------------+----------------------+
             |                             |
   Local Unix Socket             Direct Tailscale TCP (+ Auth Token)
  (~/.canban/app.sock)           or Managed SSH Socket Forward
             |                             |
             v                             v
  +---------------------+       +---------------------------------+
  | This Mac: canband   |       | Remote Linux/macOS: canband     |
  | + SQLite WAL        |       | + SQLite WAL                    |
  | + Workflow/Scheduler|       | + Workflow/Scheduler (24/7)     |
  | + local herdr-core  |       | + local herdr-core (PTYs)       |
  +---------------------+       +---------------------------------+
```

### 2.1 Daemon network listener, security and pairing (`canban serve`)

- **Local Unix socket preserved:** `canban serve` always binds `<home>/app.sock` (`0600`) for local CLI and local Agent PTY callbacks without requiring a token.
- **Persistent listener configuration (`<home>/config/server.json`):**
  ```json
  {
    "tailscale": true,
    "port": 7680,
    "listen": null,
    "trust_tailscale_self": true
  }
  ```
  `canban serve --tailscale [--port 7680]` or `canban serve --listen <ip>:<port>` persists or overrides the TCP listener configuration so service restarts (`systemd --user` / `launchd`) retain network reachability. `trust_tailscale_self` defaults to `true` and is active whenever TCP listening is enabled (`trust_tailscale_self_enabled()`).
- **Mandatory Noise_XX E2EE & bind policy (`canban-transport::noise`):**
  - Every TCP connection starts with the 4-byte wire preface `CBN1` followed by a `Noise_XX_25519_ChaChaPoly_BLAKE2s` mutual handshake (`snow`, pure Rust). Legacy plaintext JSON-RPC (`{...}`) clients receive a JSON-RPC `-32002` upgrade error and are closed immediately (zero silent plaintext downgrade).
  - Each `canband` daemon generates and persists a static X25519 server keypair in `<home>/noise.key` (`0600`), and each client generates a single static X25519 keypair in `<data_dir>/client_noise.key` (`0600`, or via `CredentialStore`).
  - Because all TCP traffic is end-to-end encrypted and mutually authenticated at the transport layer, `--allow-insecure-bind` is removed: any explicit `--listen` address is accepted, logging a startup warning when binding unspecified `0.0.0.0` / `::`.
  - **Strict host-key pinning (Option B):** Non-Tailscale TCP endpoints **must** include a valid 32-byte server public key (`?pk=<base64url>`) in the pairing URI / `SavedHost`. The client verifies the server's static public key after Noise Message 2 and **before** sending Message 3, aborting on mismatch (`HostKeyMismatchError` / `HostConnectionState::KeyMismatch`, halting retries with zero dials) without revealing the client identity or consuming a single-use pairing code. Tailscale addresses allow bare-address first-time `WhoIs` TOFU (`server_pubkey: None`) and pin the learned `server_pubkey` into `hosts.json` on first connect.
- **Pure public-key authentication (`authorized_devices.json`, `pair_...`, and Tailscale WhoIs):**
  - Bearer tokens (`dev_...`, legacy `server.token`, and ephemeral `ts_sess_...`) are removed. After the Noise_XX handshake establishes the peer's verified X25519 static public key `client_pubkey`, the server authorizes the encrypted `transport.mux` handshake via one of three paths:
    1. **Authorized device key (`TcpAuthMode::Key`):** `client_pubkey` is present in `<home>/authorized_devices.json` (`0600`).
    2. **Single-use pairing code (`TcpAuthMode::PairingCode("pair_...")`):** A valid, unexpired 5-minute code from `<home>/pairing_codes.json` (`0600`) is consumed once and atomically registers `client_pubkey` into `authorized_devices.json`.
    3. **Tailscale WhoIs same-account auto-auth (`TcpAuthMode::Tailscale`, default enabled):** `SystemTailscaleVerifier` verifies the source IP is in Tailscale CGNAT/ULA ranges (`100.64.0.0/10` / `fd7a:115c:a1e0::/48`), checks matching personal `UserID` from `tailscale status --json` and `tailscale whois --json <peer_addr>`, and strictly rejects tagged nodes, unauthorized machines, and sharee nodes. `TailscaleSessionRegistry` tracks live WhoIs sessions in memory as `ts:<StableID>` (`auth_kind: "tailscale"`) alongside persistent `authorized_devices.json` entries in `device.list`.
  - **Dedicated transport crate (`crates/transport`, `canban-transport`):** Noise_XX handshakes & `NoiseStream` framing (with injectable `NoiseRngFactory` / `DeterministicNoiseRng` for DST), public-key / pairing / Tailscale WhoIs authorization, live device revocation broadcast (`RevocationBus`), Unix/TCP accept loops, client pooling/relay bridging, and single-connection `yamux/1` multiplexing (`transport.mux`) are isolated in `crates/transport`.
  - **Single-connection `yamux/1` multiplexing (`transport.mux`):** Connected TCP clients run `yamux/1` inside `NoiseStream`, carrying concurrent JSON-RPC requests, `app.subscribe` push streams, and `herdr.connect` terminal relays over a single encrypted socket. Calling `device.revoke` (`canban device revoke <id>` or `Settings → Hosts → Revoke`) immediately kicks that device's active session.
  - **On-demand Tailnet discovery (`host.discover_canban`):** Once connected to at least one `canband` host, clients can invoke `host.discover_canban` (never background-polled) to probe linked hosts and online same-`UserID` Tailscale peers (up to 16 workers, 1.5s timeout).
- **CLI pairing, device management & service commands:**
  - `canban pair [--port 7680] [--client-pubkey <b64>] [--device-name <name>] [--json]`: Ensures listener config is saved and prints either a 5-minute single-use `canban://<addr>:<port>?pk=<server_pubkey>#pair_...` pairing URI (default) or authorizes `--client-pubkey` directly (used by automated SSH pairing).
  - `canban device {list | invite | authorize --pubkey <b64> --name <name> | revoke <id>}`: Lists authorized device keys (with fingerprints), creates one-time invites, authorizes a client key, or revokes devices live.
  - `canban service install [--tailscale] [--port 7680]` / `canban service status`: Installs and starts `~/.config/systemd/user/canban.service` on Linux (attempting `loginctl enable-linger`) or `~/Library/LaunchAgents/com.canban.daemon.plist` on macOS.

### 2.2 Client `HostRegistry`, routing and offline snapshot cache

Connection events retain their host identity in a client-side `HostEvent`
envelope; this does not change AppServer RPC payloads. A status transition such
as Connecting, Offline, Updating or ApiMismatch updates host status only. The
same observed state emits no additional event. Only a transition from not
connected to Connected emits that host's `resync`; it must not invalidate
another host's Git reads. Subsequent probes of reachable incompatible daemons wait
30 seconds unless the user explicitly reconnects. Git applies host,
Project and known worktree invalidation as described in [Git workbench](git-workbench.md).

`canban-ui` replaces the single `OnceLock<Arc<AppClient>>` assumption with a multi-host `HostRegistry` persisted in `<local-home>/config/hosts.json`:

- **Host entries:**
  - `local`: Built-in `This Mac` connection over `<local-home>/app.sock`.
  - Remote hosts (`id`, `name`, `transport`, `ssh_target`, `enabled`, `server_id`):
    - `Transport::Tcp { addr, token }` — Direct Tailscale / private TCP connection pool and direct TCP `herdr.connect` Terminal byte relay.
    - `Transport::Ssh { target, remote_home }` — Client-managed background `ssh -L` Unix-socket forwarder into a mode-`0600` local runtime socket directory, giving both JSON-RPC and `herdr.connect` transparent stream access without requiring Tailscale on pure-SSH servers.
- **Loopback / duplicate-authority guard:** Connecting a remote host checks `server_id` from `host.probe` / `app.ping` against `local` and all existing hosts, preventing accidental self-connections (e.g., selecting the current MacBook from the Tailscale peer list) or cloned-home identity collisions.
- **Connection state machine & offline cache:**
  - States per host: `Connecting`, `Connected { rtt_ms, api_version }`, `Updating { version }`, `Reconnecting { attempt, last_error }`, `ApiMismatch { server_api, app_api }`, `AuthFailed`, `Offline { last_error }`.
  - Triggering `update.daemon` immediately transitions the host into `Updating`, guards concurrent duplicate update calls and background polling from clobbering status, extends client RPC read timeout to 20 minutes for slow archive downloads, and polls bounded post-restart reconnection (`systemctl --user restart` / `launchctl kickstart`) so status progresses seamlessly from `ApiMismatch` → `Updating` → `Connected`.
  - Each successful remote `Snapshot` is cached atomically under `<local-home>/cache/hosts/<host_id>.json`. When the laptop starts offline or loses connectivity, `HostRegistry` serves the cached snapshot with editable offline outbox projection and a 64 MiB per-host LRU read cache for Card timelines and read-only Git Diffs ([Offline editing and synchronization](offline-sync.md)), syncing queued edits automatically on reconnect.
- **Host-tagged entities and O(1) operation routing:**
  - `Project`, `Card`, `TerminalEntry`, `WorkflowRun`, `WorkflowSource`, `WorkflowRevision`, `ManagedWorktree`, `ConfigurationSession`, and `MergeBatch` carry `#[serde(default)] pub host_id: String` (`"local"` by default; stamped with the owning `host.id` when merged from a remote `AppClient`). Raw entity IDs are preserved unchanged so nested payloads (`timeline.list`, `review.preview`, `execution.output`, `workflow.sources`) require no brittle ID translation.
  - Presenter `dispatch` routes operations automatically by explicit `host_id` -> `card_id` / `project_id` / `terminal_id` / `worktree_id` ownership -> active Project's `host_id` -> `local`.
- **Git & GitHub scope:**
  - **Git (`Diff & Review`):** Naturally follows the currently selected Project/Card and executes `git.*` / `review.*` RPCs on that Project's owning `canband` without cross-host aggregation complexity.
  - **GitHub Workspace (`Icon::GitHub`):** Remains bound to `local` (single-account browsing without multi-host list duplication). When linking a remote Project to a GitHub repository, the client can optionally sync the local GitHub App credentials to that remote `canband` so 24/7 Issue/PR intake continues while the laptop is offline.

---

## 3. User interface and interaction design

### 3.1 Top-chrome `Hosts` status pill and quick popover

Adjacent to the top-right `Activity & Notifications` pill (`⚡ N · ● running`) in the 42-point window chrome:

- **Status pill:** Shows `🟢 N Hosts` when remote hosts are configured (or a subtle single-host/add-host icon button when only `This Mac` is active; turns `🟡` on reconnect/API mismatch and `⚪` when a remote host is offline).
- **Quick popover:**
  - Lists `This Mac` and every configured remote host with live state, transport (`Tailscale · 11ms` or `SSH`), active Agent usage (`3/8`), and Project count.
  - Inline recovery actions per host: `Reconnect`, `Update daemon` (on API mismatch), `Open Terminal`, or `Configure…`.
  - Footer actions: `+ Connect machine…` and `Diagnostics →`.

### 3.2 `Connect a Machine` discovery and bootstrap wizard

Accessible from the `Hosts` popover, `Settings → Hosts`, `+ New Project` host picker, and Command Palette (`>Connect a Machine…`):

1. **Discovery tabs:**
   - **Tailscale (Recommended):** Queries local `tailscale status --json` and lists Tailnet peers with OS badge, Tailscale IP, MagicDNS name, and online status. Excludes `Self`.
   - **SSH Host:** Lists concrete aliases from `~/.ssh/config` plus free-form `user@host` input.
   - **Manual URL / Pairing URI:** Accepts `canban://100.x.y.z:7680#<token>` or `host:port` + token directly.
2. **Probe & guided connection:**
   - Selecting a Tailscale peer checks `<tailscale-ip>:7680` reachability via `probe_tcp_reachable`:
     - **Running & same personal Tailscale account (`trust_tailscale_self` enabled):** Attempts **`ts_auto` Tailscale WhoIs auto-auth** directly over `transport.mux` with zero SSH login or pairing code required (and omits storing `ssh_target` on automatic WhoIs connections). If WhoIs fails (cross-account peer, tagged server, or `trust_tailscale_self: false`), it falls back to **Pair automatically via SSH** (`canban pair --json`) or **Paste pairing token**.
     - **Not running / not installed:** Offers **Install & Start via SSH** (detects remote OS/arch, installs `hc` + Linux/macOS `canban` binary under `~/.local/bin`, enables `systemd --user` / `launchd` with `--tailscale`, pairs token or authenticates via Tailscale WhoIs, and connects over Tailscale TCP) alongside a collapsible manual one-liner command. Interactive SSH password/host-key prompts open in a live `canban/terminal` setup session when non-interactive SSH cannot complete automatically.

### 3.3 Board, Activity & Notifications, Quick Open and Terminals

- **Board & Project Switcher:** Mixes Projects across all hosts. Remote Projects show a quiet host suffix/badge (`· home-linux` or offline badge when disconnected). When viewing an offline remote Project's Board, a calm top banner (`<host> is offline (...) · showing cached data · edits sync when it reconnects`, plus pending/conflict counts and a `Sync…` button) keeps Card creation, title/body editing, comments, stage moves, and resolves available via the client outbox while online-only actions (gates, delivery, live Terminals) stay disabled ([Offline editing and synchronization](offline-sync.md)). `+ New Project` includes a **Host** selector (`This Mac` vs connected remote hosts) and browses directories on that selected `canband`.
- **Activity & Notifications (`Command+I`):** Aggregates `Needs you`, `Updates`, and `Running` across all connected hosts, displaying the host name alongside Project context on remote items and routing `Card & Terminal →` / `Review →` / `✓ Read` to the owning host.
- **Terminals:** Lists Agent PTYs and manual Shells across all connected hosts, grouped/badged by host. `+` inherits the selected Terminal's host and live cwd; `+ ▾` creates a new shell on any connected host.

### 3.4 Split `Settings` (`App` vs `Host Settings`) and `Diagnostics`

`Settings` separates local client preferences from per-daemon configuration:

- **APP (THIS MAC):**
  - `Theme & support` (palette, shortcuts)
  - `Terminal` (local font family & size)
  - `Hosts` (add, pair, reconnect, update remote `canband` daemons via `update.daemon` when in `ApiMismatch`, or remove remote `canband` connections; replaces the legacy `Settings → Machines` surface)
  - `Diagnostics` (end-to-end health checks & live logs across all hosts)
  - `Updates` (desktop app updates + remote host backend version status)
- **HOST SETTINGS (`[ Target: This Mac ▾ ]` selector):**
  - `Agent scheduling` (`max_agents`, `idle_minutes` on the selected `canband`)
  - `Agents & Identities` (`agents.json` Profiles on the selected `canband`, plus **Copy profiles from This Mac** when configuring a remote host)
  - `Runtime & Storage` (`hc` path/session and `<home>` path on the selected `canband`)
- **SYSTEM:**
  - `Acknowledgements`

### 3.5 `Settings → Diagnostics`

Dedicated diagnostic workspace for verifying and repairing multi-host setups:

1. **Host selector strip:** Switch between `This Mac` and any configured remote host, showing overall health badges (`Healthy`, `Warning`, `Offline`).
2. **Structured check groups:**
   - **Transport & Latency:** Socket/Tailscale TCP/SSH tunnel status, RTT (`ms`), reconnect count, last sync timestamp.
   - **Canban Service & Storage:** Daemon PID, platform, version & `DESKTOP_API` match, service manager state (`systemd --user` + linger / `launchd`), SQLite DB/WAL sizes.
   - **Herdr Runtime:** Resolved `hc` binary, version, session, Core ID, active PTY count.
   - **Execution Toolchain & Profiles:** `git`, `python3`, `tailscale`, and resolved executable status for each enabled Agent Profile (`@developer`, `codex`, `claude`, etc.).
3. **Quick actions & Live Logs:**
   - `Run Checks`, `Reconnect`, `Open Host Shell →`, `Bootstrap / Repair via SSH…`, and `Copy Diagnostic Report` (redacted JSON/Markdown summary).
   - Built-in log viewer switching between `server.log` and `updater.log` on the selected host via `diagnostics.logs`.

---

## 4. Implementation milestones

1. **Milestone 1 — Daemon TCP/Tailscale transport, auth, Linux service & Diagnostics RPCs (`canban-app-server`, `canban-herdr`, `canban-cli`, `release`):**
   - Bump `DESKTOP_API` in `crates/updater/src/lib.rs`.
   - Add `config/server.json`, `server.token`, `host.probe`, authenticated TCP JSON-RPC + `herdr.connect` relay in `canban-app-server` and TCP relay support in `canban-herdr` / `AppClient`.
   - Add `diagnostics.status` and `diagnostics.logs` RPCs, plus Tailscale/SSH local discovery helpers.
   - Add CLI `canban serve --tailscale / --listen`, `canban pair`, and `canban service install / status` (Linux `systemd --user` + macOS `launchd`), and extend `.github/workflows/release.yml` + `scripts/release.py` to publish headless Linux release archives (`canban-cli-linux-x86_64.tar.gz`, `canban-cli-linux-aarch64.tar.gz`).
2. **Milestone 2 — Multi-Host client core (`HostRegistry`, merged snapshots, offline cache & routing):**
   - Introduce `host_id` / `host_name` metadata on snapshot entities (`canban-types`).
   - Implement `HostRegistry` (`config/hosts.json`, per-host connection state machine, SSH tunnel manager, disk snapshot cache, `server_id` duplicate guard, and multi-host event aggregation).
   - Route UI presenter mutations, Project creation/directory browsing, Card views, Reminders, and `canban/terminal` bindings to their owning `AppClient`.
3. **Milestone 3 — Connection UX (`Hosts` chrome pill/popover & `Connect a Machine` wizard):**
   - Build the top-chrome `Hosts` status pill & popover.
   - Build the `Connect a Machine` modal with Tailscale peer discovery, SSH config discovery, `host.probe` auto-detection, SSH auto-pairing / remote Linux bootstrap, and manual token pairing.
   - Surface quiet host labels and offline read-only banners on Board, Activity & Notifications, Quick Open, and Terminals.
4. **Milestone 4 — Per-Host `Settings`, `Diagnostics` workspace, Command Palette parity & Gallery/Integration verification:**
   - Split `Settings` into `APP` vs `HOST SETTINGS` (with host selector + `Copy profiles from This Mac`) and add `Diagnostics` (`SectionId::Hosts`, `SectionId::Diagnostics`).
   - Register all new host/diagnostic actions in `crates/ui/src/commands.rs`, add deterministic Gallery scenes in `examples/gallery`, and run multi-host automated & native acceptance checks.

---

## 5. Multi-host Environment Variables & Global Secrets Replication

1. **Per-host Local Layers vs Replicated Global Layer:**
   - Agent/`@Identity` and Project environment variables & secrets live per-host inside that host's SQLite `env_entries` and `vault_entries` (encrypted with that host's `<CANBAN_HOME>/vault.key`). Neither `config/agents.json` nor `Project` JSON carries env/secret maps, so `Copy profiles from This Mac` never overwrites a remote host's Agent env/secrets.
   - Only **Global Environment & Secrets** (`Settings -> App -> Environment & Secrets`) replicates across remote `canband` hosts, triggered explicitly via the **Cross-host sync** bar (`Sync to hosts`).
2. **End-to-End Transport Confidentiality (`Noise_XX`):**
   - Because all TCP connections are wrapped in `Noise_XX_25519_ChaChaPoly_BLAKE2s` AEAD encryption, `env.save` and `env.global.sync` transmit directly inside the encrypted Noise stream without the redundant `enc:v1:` application-layer sealing wrapper. Receiving hosts still encrypt secrets at rest in SQLite `vault_entries` under their local `<CANBAN_HOME>/vault.key` (XChaCha20-Poly1305).
3. **Explicit Pointer-Only Unified Outbox Replication (`MutationKind::SyncGlobalEnv`):**
   - Saving Global Env & Secrets on `local` or pairing a new remote host (`add_host`) does **not** automatically push entries to remotes. Instead, each remote host tracks a local `cache/hosts/<host_id>/global-env-sync.json` state fingerprint against the local DB's deterministic global-env fingerprint (covering active entries, secret vault ciphertexts, and `deleted=1` tombstones), and the **Cross-host sync** bar reports any newly paired or out-of-date hosts until you click **Sync to hosts**.
   - Clicking **Sync to hosts** enqueues a pointer-only `MutationKind::SyncGlobalEnv` into every enabled remote host's Outbox (clearing any prior failed entry, coalescing queued syncs to one op per host, staying excluded from Card pending counts), triggers background drain on connected hosts, and leaves offline hosts queued for reconnect.
   - **Zero secrets hit `.outbox.json` or disk caches:** During Outbox drain, the sender reads active global rows, tombstones, and the matching state fingerprint in a single SQLite read transaction, seals secrets with the target host's TCP transport token, calls `env.global.sync`, and persists the synced fingerprint only on success. Recipients resolve conflicts via Last-Write-Wins (`updated_at`) and remove entries marked by newer tombstones.

