# Mobile interaction design (iOS and Android)

Status: Shared mobile UI (`crates/ui/src/mobile/`), portable Terminal controls, iOS/Android shells, Tailscale WhoIs auto-auth, and on-demand Tailnet discovery implemented (2026-11-23–2026-11-24). Public app-store distribution and Android/physical-device validation tracked separately; see [implementation.md](implementation.md).

## Scope

Full mobile client connected to remote `canband` daemons over Tailscale or direct private TCP:

- **Needs-you inbox** for cross-Project interventions, inline approvals, agent questions, updates, and running work.
- **Board** for browsing Project stages, horizontal stage switching, creating Cards, and queuing moves/edits.
- **Card workspace** with top segmented switching across **Conversation**, live **Terminal**, and **Git Diff**.
- **Terminals** tab for browsing `Card Agents`, `Shells`, and `Worktrees`, opening live sessions, and launching new Shells or Managed Branch Worktrees.
- **Git Diff and delivery** with file list, unified per-file diff, `Return for rework`, and `Deliver…` (Queue Merge allowed on mobile).
- **Offline Board and Card viewing/editing** across all stages and drafts; see [Offline editing and synchronization](offline-sync.md).
- **QR pairing using the single address chosen on desktop**, with explicit `TransportSecurity` badges (`🔒` for Tailscale / SSH, `⚠` for other TCP) and zero implicit cross-network fallback.

## Visual style and platform adaptation

Canban uses a **hybrid native style**:

- **System outer chrome** via `nsh_core::mobile` and `nsh_base`:
  - Bottom `TabBar` with platform icons (SF Symbols on iOS, drawables on Android) and native badges (`TabBarItem::badge`).
  - `nav_stack` / `nav_page` providing system navigation headers, iOS edge swipe-back, and Android predictive back.
  - `Sheet` with `SheetDetent` stops (`Medium`, `Large`, `Height`, `Fraction`) and `keyboard_lift` adjustment.
  - System `SegmentedControl`, `Switch`, and `DateTimePicker` where native platform feel is expected.
- **Canban content surfaces** via `canban-ui-kit` semantic tokens, named themes, and shared product components (`card_tile`, `timeline_item`, mention/comment composer, status badges).
- **Tablet and wide viewports (≥ 600 points):** `DockArea` keeps desktop split layouts on wide viewports and collapses only below 600 points.

## Top-level navigation

Bottom `TabBar` has four tabs:

1. **Needs you** (badge shows count of actionable items)
2. **Board**
3. **Terminals**
4. **Settings** (including paired Hosts and Diagnostics)

The navigation-bar title acts as a switcher menu for selecting the active Host and Project (or triggering Quick Open / search).

## NSH component mapping

| Desktop concept | Mobile component / behavior | Notes |
| --- | --- | --- |
| Top-right `Activity & Notifications` popover (`Command+I`) | `Needs you` root tab (`nsh_core::mobile::TabBar` + badge) | First-class mobile entry |
| Side-by-side Card + Live Terminal | `nsh_core::mobile::SegmentedControl` inside pushed `nav_page` | Switches `Conversation \| Terminal \| Diff` |
| Multi-column Board | Scrollable top stage tabs + vertical `card_tile` list | Horizontal swipe switches stage column |
| Right-click context menu | `on_long_press` + `haptic` + `context_menu` | No hover states on touch |
| Manual/auto list reload | `on_refresh` / `refreshing` on scroll container | Pull-to-refresh on Inbox, Board, Terminals |
| Drag Card across columns | Long-press `context_menu` -> `Move to…` -> `Sheet` | Reliable on narrow viewports |
| `@agent` mention / New Shell pickers | `nsh_base::Sheet` with `keyboard_lift` | Slides above software keyboard |
| Git Diff | `nsh_base::diff` | File list -> pushed unified single-column view |
| Live Terminal | `canban-terminal` portable renderer (`nsh_base::term` + `GhosttyEmulator`) | iOS later upgrades to `UIView` on `apple_render`; see [Terminal portability](terminal-portability.md) |
| Platform toggles and pickers | `nsh_core::mobile::{SegmentedControl, Switch, DateTimePicker}` | Native HIG / Material 3 controls |

**Known NSH gap:** `nsh-core` / `nsh-base` do not currently provide horizontal swipe-to-reveal row actions (such as iOS swipe-left to approve or delete). Use inline row action buttons (`[Reject] [Approve]`) plus `on_long_press` + `haptic` + `context_menu` until row swipe gestures exist upstream.

## Wireframes

### 1. QR pairing (Desktop `Settings -> Hosts` -> Phone)

```text
Desktop Settings -> Hosts                 Phone (first launch)
┌──────────────────────────────┐          ┌────────────────────┐
│ Pair a phone                 │          │       Canban       │
│  Address: [home-mac.ts.net ▾]│          │                    │
│  ┌────────┐  home-mac        │  ──QR──▶ │  Scan QR code in   │
│  │ ▓▓ ▓ ▓ │  🔒 Tailscale    │  system  │  desktop Settings  │
│  │ ▓ ▓▓ ▓ │  expires 04:59   │  camera  │  -> Hosts          │
│  └────────┘                  │  opens   │                    │
│ Paired devices               │  link    │ [Open Camera]      │
│  iPhone 17 · 2m ago [Revoke] │          │ [Paste URI]        │
└──────────────────────────────┘          └────────────────────┘
                                                    │
                                                    ▼
                                          ┌────────────────────────────────────┐
                                          │ Connect home-mac?                  │
                                          │ macOS · v20261120                  │
                                          │                                    │
                                          │ 🔒 home-mac.ts.net:7680            │
                                          │    Tailscale · 12ms                │
                                          │                                    │
                                          │ 🔒 Tailscale / SSH = encrypted     │
                                          │ ⚠  Other TCP = depends on network  │
                                          │                                    │
                                          │ [Cancel]                 [Connect] │
                                          └────────────────────────────────────┘
```

### 2. Bottom navigation and `Needs you` inbox

```text
┌──────────────────────────────┐
│ Needs you         home-mac ▾ │ <- System nav bar; tap host/Project to switch
├──────────────────────────────┤
│ ⚠ Offline · Cached · 3 sync  │ <- Shown only when offline, syncing, or conflicted
├──────────────────────────────┤
│ Needs you                    │
│ ● Fix login bug   Approval   │
│   claude · 2m [Reject][Approve] <- Inline actions (no row swipe in NSH)
│ ● Add OAuth       Question   │
│   "Use PKCE flow?"   [Reply] │
│ Updates                      │
│ ✓ Refactor API   Review done │
│ Running (2)                  │
│ ◐ Migrate DB     claude · 4m │
├──────────────────────────────┤
│[Needs ③][Board][Term][Set]   │ <- System TabBar with native badge
└──────────────────────────────┘
Pull to refresh · Long-press row -> haptic + context_menu (Open Terminal / View Diff / Mute)
```

### 3. Board (top stage tabs + horizontal swipe between columns)

```text
┌──────────────────────────────┐
│ canban ▾                  ＋ │ <- Tap title to switch Project; + creates Card
├──────────────────────────────┤
│ Todo 4│Doing 2│Review 1│Done │ <- Scrollable stage strip; swipe body left/right to switch
├──────────────────────────────┤
│ ┌──────────────────────────┐ │
│ │ Fix login bug          ◐ │ │ <- Shared card_tile
│ │ #142 · claude · 4m       │ │
│ └──────────────────────────┘ │
│ ┌──────────────────────────┐ │
│ │ New onboarding   ☁̸ Local │ │ <- Created offline; queued for sync
│ └──────────────────────────┘ │
└──────────────────────────────┘
Long-press Card -> haptic + context_menu: Move to… / Resolve & Archive / Open Terminal / Copy Link
"Move to…" -> Sheet listing Project stages (offline moves queue and auto-apply on reconnect)
```

### 4. Card workspace (pushed `nav_page`)

```text
┌──────────────────────────────┐
│ ‹ Board   Fix login bug   ⋯  │ <- ⋯ opens Card actions (Move, Archive, Sync status)
├──────────────────────────────┤
│ [ Conversation│Terminal│Diff]│ <- System SegmentedControl
├──────────────────────────────┤
│ Doing · claude · main→fix-142│
│ ▸ Description & context      │ <- Collapsible section
│ ── Timeline ──               │
│ 🤖 Located session expiry…   │ <- Shared timeline_item
│ 👤 @claude add regression    │
│ ┌ Approval: run db migrate ┐ │
│ │      [Reject]  [Approve] │ │ <- Online-only gate
│ └──────────────────────────┘ │
├──────────────────────────────┤
│ [@] Add a comment…      [↑]  │ <- Lifts with keyboard; @ opens Agent Sheet
└──────────────────────────────┘
```

### 5. Live Terminal (Card `Terminal` segment or pushed from `Terminals` tab)

```text
┌──────────────────────────────┐
│ ‹ Board   Fix login bug   ⋯  │
│ [ Conversation│Terminal│Diff]│
├──────────────────────────────┤
│ $ cargo test                 │
│ running 42 tests             │
│ test auth::expire ... ok     │
│ ▊                            │ <- Pinch to zoom font; long-press starts selection
├──────────────────────────────┤
│ Esc Tab Ctrl ↑ ↓ ← → ^C  ⤢   │ <- Accessory key bar above keyboard; ⤢ = landscape fullscreen
├──────────────────────────────┤
│      (Software keyboard)     │
└──────────────────────────────┘
Offline: shows last captured screen snapshot read-only + "Terminal needs a connection"
```

### 6. Mobile `Terminals` tab

```text
┌──────────────────────────────┐
│ Terminals         home-mac ▾ │ <- Pull to refresh; + opens creation Sheet
│                          ＋  │
├──────────────────────────────┤
│ Card Agents                  │
│ ● #142 Fix login bug  claude │
│   canban/142 · active · 4m   │
│ ○ #138 OAuth flow     codex  │
│   canban/138 · idle          │
│ Shells                       │
│ ● zsh · canban               │
│   ~/Workspace/canban · 12m   │
│ Worktrees                    │
│ ◐ feat/mobile-sync    clean  │
├──────────────────────────────┤
│[Needs ③][Board][Term][Set]   │
└──────────────────────────────┘
Tap + -> Sheet:
  - New Shell (select Project / worktree)
  - New Branch Worktree… (enter branch name + base)
Tap row -> pushes full-screen Terminal page with accessory key bar
Long-press row -> haptic + context_menu: Open Card / Promote to Card / Close Terminal
```

### 7. Git Diff (`Diff` segment)

```text
File list                        Single-file diff (pushed nav_page)
┌──────────────────────────┐    ┌──────────────────────────┐
│ main…fix-142  +48 −12    │    │ ‹  src/auth.rs  ‹ 2/5 ›  │
│ M src/auth.rs    +30 −8  │ ─▶ │ @@ -40,6 +40,9 @@        │
│ A tests/exp.rs   +18     │    │  fn refresh() {          │
│ M Cargo.toml      +0 −4  │    │ -  let t = old();        │
├──────────────────────────┤    │ +  let t = renew()?;     │
│ [Return for rework]      │    │ [Wrap ⇄]   Swipe ‹file›  │
│ [Deliver…]               │    └──────────────────────────┘
└──────────────────────────┘
Single-column unified diff by default; horizontal scroll for long lines with a line-wrap toggle.
`Deliver…` triggers Queue Merge / explicit delivery on the owning canband.
```

### 8. Sync status and conflict resolution

See [Offline editing and synchronization](offline-sync.md#ui-states-and-conflict-resolution) for the Sync screen and field-level conflict wireframes shared across desktop and mobile.

## Single-address pairing

### Single-address pairing URI

Desktop, CLI (`canban pair`), and mobile share a unified single-address pairing URI while also accepting bare `canban://<addr>#<secret>` strings:

```text
canban://<host:port>[?sid=<server_id>&name=<hostname>]#<secret>
```

- **Explicit desktop selection (zero silent cross-network fallback):** On desktop (`Settings -> Hosts -> Pair a phone`) or CLI (`canban pair`), the user selects the single endpoint address they intend to expose (typically Tailscale MagicDNS `*.ts.net` or Tailscale IPv4 `100.x.y.z`, or an explicit LAN/custom VPN address).
- **Single-address confirmation & probe:** The client parses the single address, optionally checks reachability/RTT with an unauthenticated TCP connect (`probe_addr`), displays the transport security badge before confirmation, and saves **only that chosen address**—never storing hidden fallback addresses or silently failing over between Tailscale and LAN routes. Switching addresses later is an explicit edit in `Settings -> Hosts`.
- **Transport security icons:** Pairing confirmation cards and `Settings -> Hosts` rows display explicit link security badges (`TransportSecurity` in `canban-client`):
  - **Local socket / Tailscale / SSH** (`Local`, `*.ts.net` / `100.64.0.0/10` / `fd7a:115c:a1e0::/48`, or `HostTransport::Ssh` encrypted tunnels): display a lock icon (`🔒`) with hint `"Tailscale — encrypted by WireGuard"` or `"SSH — encrypted tunnel"`.
  - **All other TCP addresses** (LAN IPs, custom hostnames): display a caution icon (`⚠`) with hint `"Not Tailscale — may be unencrypted unless your own VPN protects it"`.
  - Hints are purely informational and **never block selection or connection**; the user decides because Canban cannot inspect whether a non-Tailscale TCP route runs inside the user's own VPN.
- **Unreachable tailnet guidance:** If a Tailscale endpoint does not respond, show `"Tailscale is not connected on this phone"` with a shortcut button to open the Tailscale app alongside a `Retry` action.
- **Camera capture:** Because NSH has no camera or barcode scanner component, desktop renders the pairing URI as a QR code and the phone uses the OS camera to open the registered `canban://` deep link; an in-app `Paste URI` button serves as fallback.

### Transport security

Link confidentiality is provided by the user's chosen network path (Tailscale WireGuard, a user-managed VPN, or the SSH tunnel fallback) rather than Canban-managed transport encryption:

- `canband`'s TCP listener carries no application-layer encryption: JSON-RPC requests, Terminal byte streams, and the bearer token travel in plaintext over that TCP socket, so users should only expose `--listen` addresses that sit on an encrypted or private network.
- Desktop already supports encrypted SSH tunnels via `HostTransport::Ssh`; mobile presently connects over TCP (Tailscale or another VPN) because iOS sandboxing forbids spawning `ssh` subprocesses, and any future pure-Rust SSH tunnel transport on mobile will share the same `🔒` `TransportSecurity::Ssh` classification.
- Application-layer TLS (e.g., TLS 1.3 with QR-pinned certificates) and mandatory Tailscale-only enforcement were evaluated and declined on 2026-11-22 so users remain free to route traffic through their own VPN setups.

### Pairing credentials & Tailscale WhoIs auto-auth

- **Zero-QR Tailscale same-account sign-in (`ts_auto`):** Both first-run onboarding and the Add Host / Pairing sheet accept bare Tailscale addresses without `#secret` (`host.ts.net`, `100.x.y.z`, `canban://host.ts.net`, or `tcp://100.x.y.z:7680`). During `transport.mux`, `canband` verifies the peer via local `tailscale whois --json` (requiring the same personal Tailscale `UserID` and rejecting tagged or shared-in peers unless `"trust_tailscale_self": false` is configured) and returns an ephemeral per-connection `session_token` for RPCs and `enc:v1:` secret sealing. Confirmation cards and `Settings` host detail rows show `Tailscale — automatic same-account sign-in (managed in Tailscale Admin)`.
- **On-demand Tailnet host discovery (`host.discover_canban`):** After pairing with a first host—and manually via **Settings → Hosts → Discover on Tailnet** (never background-polled)—`canban-client` invokes `host.discover_canban` on the connected host, which probes its enabled saved TCP entries in `config/hosts.json` (`linked_on_host: true`) and online same-user non-tagged desktop/server Tailscale peers (`16` parallel workers, `1.5s` timeout). Already-paired hosts are filtered out by `server_id` and normalized address; reachable peers appear in the `AVAILABLE ON TAILNET` sheet with one-tap **Join** / **Join All (N)** for Tailscale endpoints and **URI…** (`canban://<addr>#` prefill) for linked non-Tailscale TCP hosts.
- **One-time pairing code -> per-device revocable token:** For cross-account peers, tagged servers, non-Tailscale VPNs, or hosts with `"trust_tailscale_self": false`, the QR / pairing URI carries a short-lived (5-minute) single-use `pair_...` pairing code rather than a static shared secret. On first connection, the client exchanges that pairing code for a **per-device revocable bearer token** (`dev_...`) persisted on `canband` (and stored locally in the client's `CredentialStore`).
- **Paired devices management:** Desktop `Settings -> Hosts` provides a `Paired devices` list showing persistent `dev_...` tokens (with `[Revoke]` to invalidate an individual device live) alongside active Tailscale WhoIs sessions (`Tailscale` badge and `Managed in Tailscale Admin`).
- **Backward compatibility:** Bare single-address URIs without query parameters (`canban://<host>:<port>#<token>` or `<host>:<port>#<token>`) remain accepted by the parser.

## Planned Gallery scenes

Per [UI architecture](ui-architecture.md), add phone-viewport (`390×844` and `412×915`) regression scenes to `examples/gallery` using production `canban-ui-kit` and `canban-ui` components before shipping mobile views:

1. `mobile-needs-you`: inbox with offline/sync banner, inline Approve/Reject, question reply, updates, running items, and `TabBar` badge.
2. `mobile-board`: stage tabs, `card_tile` list, offline-created unsynced badge, and `Move to…` `Sheet`.
3. `mobile-card-conversation`: pushed `nav_page`, `SegmentedControl`, collapsible context, `timeline_item` stream, and bottom composer.
4. `mobile-terminal`: portable `Terminal` view with accessory key bar (`Esc`, `Tab`, `Ctrl`, arrows, `^C`, `⤢`) and offline read-only snapshot state.
5. `mobile-terminals-tab`: grouped `Card Agents` / `Shells` / `Worktrees` inventory and `+` creation `Sheet`.
6. `mobile-diff`: file list and single-column unified diff with line-wrap toggle and `Return for rework` / `Deliver…` actions.
7. `mobile-pairing`: QR confirmation sheet showing the single chosen address, RTT reachability, lock (`🔒` Tailscale / SSH) vs caution (`⚠` other TCP) security icons, hint texts, and unreachable state.
8. `mobile-sync-conflicts`: Sync queue summary (synced/pending/conflicts) and field-level merge conflict screen.
9. `mobile-offline-states`: offline banner variants (`Offline · Cached`, `N pending`, `N conflicts`).
