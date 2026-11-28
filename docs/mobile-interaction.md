# Mobile interaction design (iOS and Android)

Status: Shared mobile UI (`crates/ui/src/mobile/`), Daily Briefing Inbox with native-thread `swipe_row` triage, pushed GitHub PR/Issue Markdown reader, portable Terminal controls, iOS/Android shells, Noise_XX E2EE, Tailscale WhoIs auto-auth, and on-demand Tailnet discovery implemented (reconciled 2026-11-28 on NSH `0.7.2`). Public app-store distribution and physical-device validation tracked separately; see [implementation.md](implementation.md).

## Scope

Full mobile client connected to remote `canband` daemons over Noise_XX end-to-end encrypted TCP (Tailscale or pinned private TCP):

- **Inbox (Daily Briefing)** powered by the shared 4-tier `DailyInbox` projection: 4-column clickable KPI decision filters (`Waiting`, `Merge`, `Agents`, `Shipped`), Needs Action (Card approvals/deliveries, Blocked Terminals, Failed Schedules with one-tap Retry, direct GitHub blockers), swipe-to-triage rows (`Done` / `Defer` + auto-dismiss Undo toast), project-grouped Active Work with structured Agent telemetry, direct GitHub notifications with a pushed native Markdown PR/Issue reader, collapsed secondary summaries (`Scheduled Runs`, `Shipped Today`, `Watching`, `Earlier`), and top-right `Today / Deferred / Done` drawer switcher.
- **Board** with pinned stage pills, horizontally swipeable stage pager (`Carousel`), flat hairline Card rows aligned with desktop `CardTile` hierarchy, Card creation with Agent Assignee selection, and searchable Project switching.
- **Card workspace** with horizontally swipeable `Conversation | Terminal | Diff` (`Carousel` synced with top `SegmentedControl`), Edit Card sheet, Workflow Retry controls, `@agent` quick-mention composer, live Terminal, and unified Git Diff with `Return for rework` / `Deliver…`.
- **Terminals** tab with hairline `Card Agents` / `Worktrees` / `Shells` rows, Agent brand icons, plain-shell command status (`●` / `✓` / `exit N`), `Interrupt (^C)` & confirmed `Close Terminal` controls, and lightweight Shell creation.
- **Settings** with flat hairline `Hosts`, `Preferences` (remote-synced `Theme` and `Daily Cutoff`), and `Sync` queue/conflict resolution + Diagnostics.
- **Offline Board and Card viewing/editing** across all stages; see [Offline editing and synchronization](offline-sync.md).
- **Single-address pairing** with Noise_XX E2EE, pinned host public keys (`?pk=<base64url>`), Tailscale WhoIs auto-auth, and on-demand Tailnet discovery.

## Visual style and gesture hierarchy

Canban Mobile uses a **Linear Mobile × Desktop Editorial** hybrid native style aligned with [UI architecture](ui-architecture.md):

- **Flat hierarchy, no nested borders:** Lists sit directly on `BACKGROUND` separated by `1px` `BORDER` hairlines; focused decision cards, active alerts, and code/diff blocks use borderless `SURFACE` containers with subtle left accent bars where urgency matters.
- **No explanatory UI copy:** Show state, data, and actions directly without instructional captions or static gesture manuals.
- **System outer chrome** via `nsh_core::mobile` and `nsh_base`:
  - Bottom 4-tab `TabBar` (`Inbox`, `Board`, `Terminals`, `Settings`) with native `Inbox` badge strictly mirroring `inbox.actionable_count`.
  - `nav_stack` / `nav_page` providing system navigation headers, iOS edge swipe-back, and Android predictive back.
  - `Sheet` with `SheetDetent` stops (`Medium`, `Large`) and `keyboard_lift`.
  - Native `AlertDialog` and unified Canban tactile `mobile_segmented_control` / `toggle` controls.
- **Single horizontal gesture owner per screen** (zero competing horizontal gestures):
  - **Root navigation:** Bottom 4-tab bar (tap to switch; no outer horizontal tab pager).
  - **Inbox:** Row-level horizontal swipe (`swipe_row` built on NSH `PanActivation::horizontal` + `pan_values` + `bind_translate_x`) for left = `Done` and right = `Defer…`. `Today / Deferred / Done` lives in the top-right header menu.
  - **Board:** Full-page horizontal `carousel` switching between Project stages.
  - **Card workspace:** Full-page horizontal `carousel` switching between `Conversation | Terminal | Diff`, synced with the top `SegmentedControl`.

## NSH component mapping

| Desktop concept | Mobile component / behavior | Notes |
| --- | --- | --- |
| Left-rail `Inbox` (Daily Briefing) | `Inbox` root tab (`TabBar` + badge `== actionable_count`) | Shared `DailyInbox` projection & `inbox::state` |
| Inbox inline `Done` / `Defer` buttons | Native-thread `swipe_row` + long-press `context_menu` + `Defer` `Sheet` | `PanActivation::horizontal` + `pan_values` + `bind_translate_x` |
| Inbox inline Markdown PR reader | Pushed `MobileRoute::GitHubItem` `nav_page` | Full-width Markdown, CI status, linked Card, bottom reply bar |
| Side-by-side Card + Live Terminal | `SegmentedControl` + `carousel` inside pushed Card `nav_page` | Switches `Conversation \| Terminal \| Diff` |
| Multi-column Board | Scrollable stage pills + hairline Card rows in `carousel` | Horizontal swipe switches stage column |
| Right-click context menu | `on_long_press` + `haptic` + `context_menu` | No hover states on touch |
| Manual/auto list reload | `pull_refresh_scroll` (`UIRefreshControl` on iOS) | Pull-to-refresh on Inbox, Board, Terminals |
| Drag Card across columns | Long-press `context_menu` -> `Move to…` | Reliable on narrow viewports |
| `@agent` mention in Card composer | `@` button toggling quick `@profile` pill bar above composer | Zero-typing agent mentions |
| Live Terminal | `canban-terminal` portable renderer (`nsh_base::term` + `GhosttyEmulator`) | 1-RTT fast-attach + `zstd-s2c` stream compression; accessory bar + pinch zoom |
| Platform dialogs and toggles | `nsh_base::alert_dialog` + Canban tactile `mobile_segmented_control` / `toggle` | Consistent across iOS, Android, and Gallery (`nsh_core::mobile` in 0.7.2 provides `TabBar` only) |

## Single-address pairing & transport security

Desktop, CLI (`canban pair`), and mobile share a unified single-address pairing URI while also accepting bare Tailscale addresses:

```text
canban://<host:port>?pk=<server_pubkey>[&sid=<server_id>&name=<hostname>]#pair_<code...>
```

- **Mandatory `Noise_XX_25519_ChaChaPoly_BLAKE2s` E2EE:** All JSON-RPC calls, secrets, `app.subscribe` streams, and `herdr.connect` Terminal streams travel inside Noise AEAD transport ciphertexts (`CBN1` wire preface). Legacy `🔒` / `⚠ Not Tailscale` transport warning badges are removed.
- **Host-key pinning:** Non-Tailscale TCP URIs require `?pk=<base64url>` and abort before Noise Message 3 on mismatch (`HostConnectionState::KeyMismatch`). Tailscale endpoints accept bare addresses for first-time `WhoIs` TOFU and pin `server_pubkey` in `hosts.json`.
- **Zero-QR Tailscale same-account sign-in & on-demand Tailnet discovery:** Bare Tailscale addresses (`host.ts.net`, `100.x.y.z`, `canban://host.ts.net`) authenticate via `tailscale whois --json` (`trust_tailscale_self: true`). After first-host pairing—and on demand from **Settings → Discover on Tailnet**—`host.discover_canban` surfaces reachable Tailnet Canban hosts for one-tap **Join** / **Join All**.

## Gallery regression scenes

Phone-viewport (`390×844` and `412×915`) regression scenes in [examples/gallery/mobile.rs](../examples/gallery/mobile.rs) cover production `canban-ui` mobile screens:

1. `mobile-inbox`: Daily Briefing Inbox with 4-column KPI bar, Needs Action, Active Work grouped by Project, GitHub notifications, and collapsed secondary summaries.
2. `mobile-github-pr`: Pushed GitHub PR/Issue Markdown reader with author avatar, CI summary, linked Card shortcut, and bottom reply capsule.
3. `mobile-board`: Stage pills, hairline Card rows with Assignee/PR/status metadata, and horizontal `carousel` pager.
4. `mobile-card-conversation`: Pushed Card `nav_page`, `SegmentedControl` + `carousel`, Workflow Retry / approval banner, Edit action, Timeline, and `@agent` capsule composer.
5. `mobile-terminal`: Portable `Terminal` view with accessory key bar (`Esc`, `Tab`, `Ctrl`, arrows, `^C`, `⤢`) and header Interrupt/Fullscreen controls.
6. `mobile-terminals-tab`: Hairline `Card Agents` / `Worktrees` / `Shells` list with Agent brand icons and plain-shell command status.
7. `mobile-diff`: Borderless single-column unified diff with file selector pills, line-wrap toggle, and sticky `Return for Rework` / `Deliver…` bar.
8. `mobile-settings`: Hairline `Hosts`, `Preferences` (`Theme`, `Daily Cutoff`), and `Sync` sections.
9. `mobile-pairing`: Single-address pairing confirmation with Noise host-key fingerprint and live reachability probe.
10. `mobile-sync-conflicts`: Offline sync queue and field-level conflict resolution sheet.
11. `mobile-offline-states`: Offline/queued banner and pending sync indicators.
