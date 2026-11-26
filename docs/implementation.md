[Dedicated Terminal workspace](terminal-workspace.md) (2026-11-08) is the current Terminal UI contract: its separate icon/tree, Card/Overview/Quick Open convergence and isolated multi-PTY acceptance supersede the older Card-plus-Terminal focused split. Review retains the Card-focused delivery layout.

Current runtime update (2026-11-09): [Event-driven runtime observation](runtime-observation.md) replaces periodic Terminal/Agent scans, adds bounded context discovery and changes automatic unreported-exit gates into Failed history.

# MVP implementation and development

Workflow/event contract (reconciled 2026-11-12): [Unified workflow engine, Studio and implementation](product/workflow-engine.md) consolidates the design principles, YAML schema, Visual Studio graph-derived options, Queue Merge upstream sync, operations and durable subscriptions. Projects own named YAML sources and immutable versions in SQLite; no Workflow files or revision mirrors are generated in the Canban home. Project Workflows supports Visual Studio, YAML editing/history and Edit with agent; Column trigger details are derived client-side. Configuration conversations have Project ownership and use the existing Terminal. Onboarding remains a separate topic; deterministic fixture acceptance does not imply live provider authentication.

[Unified Card reminders](reminders.md) (2026-11-10) defines the current unread /
pending counts, persistent visible-read positions, Overview updates and local
notification sounds. Current builds use SQLite schema 15 and desktop API version 36 (earlier Workflow/GitHub milestones used schema 12/API 10 and schema 10/API 8). Upgrade
the installed AppServer together with its UI. `mise run dev` (also `mise run run`)
now manages a separate checkout/branch instance; it does not upgrade installed
`~/.canban`. Existing history is baselined quietly during migration.

[Compact navigation and Quick Open](quick-open-and-navigation.md) covers the implemented icon rail, Command+O fuzzy destination picker and native interaction checks.

[Agent Overview and lifecycle](agent-overview-and-lifecycle.md) documents the implemented status-aware execution budget, configurable resumable idle exit and global attention/Agent workspace.

Status: native macOS (AppKit/Metal) and Linux (GTK4) desktop workbench with simultaneous multi-host (`canband`) connections and multi-machine execution; current contracts reconciled 2026-11-16 (desktop API version 58). See [Multi-host `canband` architecture](multi-host.md) for simultaneous local + remote Tailscale/SSH `canband` operation, offline snapshot caching, Linux desktop & CLI release archives, split App vs Host Settings, and the Diagnostics inspector. The desktop updater and manual `Release` workflow have isolated/native acceptance recorded in [updates](updates.md); older documentation-only reconciliation added no acceptance. This is the current implementation reference; earlier design documents contain broader proposals and dated pre-implementation statements.

[Desktop utilities](desktop-utilities.md) covers the shared bottom bar, subscription Usage, Caffeine, persisted appearance, command catalog and cross-Project Card creation.

## Tailscale WhoIs Auto-Auth & On-Demand Tailnet Discovery (2026-11-24)

Commit `42782a4` advances `DESKTOP_API` to **86** and adds zero-code same-account Tailscale authentication plus on-demand Tailnet discovery across `canban-types`, `canban-transport`, `canban-app-server`, `canban-client`, desktop `Settings → Hosts` / `Connect a Machine`, and the mobile onboarding / `Settings` UI:

- **Tailscale WhoIs same-account auto-auth (`config/server.json` `trust_tailscale_self: true` default):** Clients send `TAILSCALE_AUTH_SENTINEL` (`ts_auto`) during the `transport.mux` (`yamux/1`) handshake. `SystemTailscaleVerifier` verifies the source IP is in Tailscale CGNAT/ULA ranges (`100.64.0.0/10` / `fd7a:115c:a1e0::/48`), caches the local personal `UserID` from `tailscale status --json` (60s TTL), and validates `tailscale whois --json <peer_addr>`, strictly rejecting mismatched `UserID`s, tagged nodes (`Tags` non-empty on host or peer), `MachineAuthorized == false`, and shared-in external nodes (`IsShareeNode == true`). On success, `transport.mux` returns `auth_mode: "tailscale"` and a per-connection ephemeral `session_token` (`generate_ephemeral_session_token()`) used for `enc:v1:` Global Secrets transport sealing (never deriving keys from static `ts_auto`). Non-mux daemons reject `ts_auto` and prompt upgrading or pairing via `canban://` URI.
- **In-memory session registry & desktop UX:** `TailscaleSessionRegistry` tracks live WhoIs peers as `ts:<StableID>` (`auth_kind: "tailscale"`) without writing to `device_tokens.json`; `device.list` merges them with persistent `dev_...` tokens and exposes `trust_tailscale_self`. Desktop **Settings → Hosts (Paired Devices)** labels them with a `Tailscale` badge and `Managed in Tailscale Admin`, while **Connect a Machine → Tailscale** probes `<ip>:7680` reachability, attempts WhoIs auto-auth first (omitting `ssh_target` when connected via WhoIs), and falls back to non-interactive SSH pairing or manual pairing URIs when needed. Mobile onboarding/pairing accepts bare Tailscale addresses (`host.ts.net`, `100.x.y.z`, `canban://host.ts.net`, `tcp://100.x.y.z:7680` without `#secret`).
- **On-demand Tailnet discovery (`host.discover_canban`):** Queries an already-connected `canband` host once (never background-polled) to merge enabled non-self TCP entries from `config/hosts.json` (`linked_on_host: true`) and online same-`UserID` non-tagged non-sharee non-mobile/TV (`iOS`/`Android`/`tvOS` excluded) Tailscale peers from `tailscale status --json`, runs a bounded parallel TCP probe (up to `16` workers, `1.5s` timeout), and returns sorted reachable hosts (`linked_on_host`, `rtt_ms`, `name`). Mobile runs it automatically after first-host pairing and exposes **Settings → Discover on Tailnet**, filtering already-paired hosts by `server_id`/address and offering one-tap **Join** / **Join All (N)** or **URI…** prefill. See [Multi-host daemons](multi-host.md) and [Mobile interaction](mobile-interaction.md).

## iOS Mobile Shell, Shared Mobile UI & Terminal Phase 3 (2026-11-23)

`crates/terminal` (Phase 3) adds `src/mobile_bar.rs` (floating/docked `Esc`/`Tab`/`Ctrl` sticky/`↑`/`↓`/`←`/`→`/`^C`/`Paste`/`⤢` accessory bar and `9–28pt` pinch-to-zoom) and extends `PortableOptions` / `TerminalView` (`src/view.rs`, `src/portable.rs`) with `.mobile_controls(...)`, `.on_fullscreen_toggle(...)`, `.offline_notice(...)`, offline cached-snapshot banners, long-press touch line copy, and keyboard/safe-area bottom avoidance. `crates/ui` adds shared hybrid mobile screens under `src/mobile/` (`Needs you` with actionable badge, `Board`, `Terminals`, `Settings`, `canban://` single-address pairing confirmation with `TransportSecurity` badges & TCP reachability probe, pushed `Card` `Conversation | Terminal | Diff`, pushed `Terminal`, sync conflict/diagnostics sheets, and 0-host onboarding), wires non-local `open_url` in `src/links.rs` and `on_foreground`/`remote_client` hooks on `UiBackend`, and registers 9 phone-viewport (`390×844` / `412×915`) mobile scenes in `examples/gallery/mobile.rs` (`examples/gallery/main.rs`). `crates/ui-kit/src/pull_refresh.rs` uses native `UIRefreshControl` when compiled for iOS (`cfg!(target_os = "ios")`). `crates/ios` (`canban-ios`), `scripts/ios.py`, and `mise.toml` (`check-ios`, `build-ios-sim`, `ios-sim` with `IPHONEOS_DEPLOYMENT_TARGET=15.0`) package and launch `.local/ios/Canban.app` (`dev.canban.mobile`, `canban://` URL scheme); cross-compilation passes on `aarch64-apple-ios-sim` and `aarch64-apple-ios`, and live `iPhone 18 Pro Max` (`iOS 27.0`) Simulator E2E verification confirmed clean onboarding, `canban://` deep-link handling, `⚠ Not Tailscale` / live TCP probing, single-use `canban pair` authentication against isolated `canban serve`, and the connected 4-tab `UITabBar` + `Settings` screens.

## Shared pure-remote client & `UiBackend::RemoteBackend` (2026-11-23)

`crates/client` (`canban-client`) implements a pure-remote client without `canban-app-server`, SQLite, or SSH subprocesses, combining native Tokio `app.subscribe` push streams and `call_async`/`connect_tcp_async` over `canban-transport`, single-address QR pairing (`canban://<addr>[?sid=...&name=...]#<secret>`) with zero implicit fallback addresses, `TransportSecurity` (`Local`, `Tailscale`, `Ssh`, `UnverifiedTcp`), pluggable `CredentialStore` (`FileCredentialStore` / `MemoryCredentialStore`), TCP `enc:v1:` secret sealing, cached `RemotePtyTransport`, and `canban-sync` offline outbox/read-cache/3-way conflict resolution. `canban-ui` exposes `RemoteBackend` unconditionally (even with `--no-default-features`).

## Terminal cross-platform core & remote PTY transport (Phases 0–2, 2026-11-23)

`crates/terminal` now separates macOS `NSView` (`src/apple/`) from portable `nsh_base::Terminal` (`src/portable.rs`), extracts shared input/selection/resize/theme logic into `src/input.rs`, exposes a unified `TerminalView` UI entry point (`src/view.rs`), and implements `RemotePtyTransport` (`src/remote.rs`) speaking Generation-1 Herdr binary framing (`herdr.connect` -> `hello` -> `terminal.attach`) directly over `canban-transport` (`yamux/1` or direct TCP) + Tokio. `canban-terminal --no-default-features` and `canban-ui --no-default-features` cross-compile cleanly with zero warnings on `aarch64-apple-ios-sim` (`platform 7`) and `aarch64-apple-ios` (`platform 2`), verified alongside live iOS 27.0 Simulator `vt_smoke` execution; Android NDK verification remains pending. See [Cross-platform Terminal architecture](terminal-portability.md).

## Offline editing and synchronization (2026-11-22)

Remote `canband` Projects remain editable when offline through `crates/sync` (`canban-sync`) and desktop `HostRegistry` integration (DESKTOP_API **77**). `card.create` and `comment.add` accept validated client-chosen IDs (`Local` badge while `number == 0`), offline queueable mutations persist up to 500 ops per host with optimistic snapshot overlays and synthetic receipts (`offline_queued: true`), read-cacheable timelines/diffs use a 64 MiB per-host LRU cache, and reconnect replays FIFO with 3-way `title`/`body` merge plus the desktop `Sync` modal (`Retry`, `Discard`, `Use mine`, `Use server`, `Edit merged`). See [Offline editing and synchronization](offline-sync.md).

## Timeline-first Insights redesign & activity tracking (2026-11-21)

Insights uses a **timeline-first** layout with `1D` hourly swimlanes (`Me · Card`, `Me · Terminal`, `Me · Other`, plus `Profile` or `Model` Agent lanes and `Merges` markers), `7D`–`90D` daily stacked bars, a 3-panel Overview row (`Output`, `Automation funnel`, `Projects`), a `[Profile] / [Model]` Agents panel, a `[Resolved] / [My time] / [Agent time]` 365-day heatmap, and an always-expanded `Timings` panel. SQLite schema advances to **21** (`activity_segments` with 400-day retention) and desktop API version advances to **76** for `activity.record`, `insights.activity`, and `insights.activity_buckets`. Commit counts and token usage metrics remain intentionally omitted. See [Insights](insights.md).

## Agent RPC and CLI responses (2026-11-17)

RPC and CLI now default to Agent-oriented Card summaries, normalized paginated
Timeline reads and mutation receipts. Content is selected in AppServer through
`fields` or `view: "full"`; CLI maps `--fields`/`--full` and defaults to compact
JSON, with `--human` changing formatting only. This change advances desktop API
to 65 (schema 20 is unchanged). Upgrade affected clients together; old defaults
are not preserved. See the [response contract](agent-response-contract.md) and
[isolated/native acceptance and byte comparison](agent-interface-acceptance-20261117.md).

## Adopting an existing Agent terminal (2026-11-18)

`canban card adopt` automatically includes the inherited `HC_TERMINAL_ID` and,
when available, `HC_SESSION` as optional runtime context. Its command, flags and
receipt remain unchanged. AppServer verifies the original Core, machine, live
PTY, Profile and Card workspace before transferring a known independent terminal
to the canonical Card/Profile Agent entry. It preserves the existing process,
terminal name, provider conversation and live tab selection; it does not launch
another Agent. An already-adopted workflow can acquire this binding on a later
adopt call without restarting the workflow.

Missing, unavailable, ambiguous or conflicting runtime context only produces a
server log message. Workflow adoption continues with its existing validation and
response contract. Successful takeover uses the ordinary Agent report, runtime
observation, idle reclamation and exact Resume paths, without a merge-specific
exit condition. The optional RPC context advances desktop API to **66**; the
database schema is unchanged. Remote CLI requests carry the same optional context
and are scoped to their authenticated reporting bridge's machine.

Full adoption, restart, idle exit, and resume coverage runs in DST (`canban-sim`
`sessions_and_adoption` and `chaos`), while `mise run acceptance` includes a real-PTY
`adopt` + `interrupt` smoke check over an isolated Core without paid provider APIs or
the user's installed Canban home/Core.

Executed acceptance (2026-11-18): baseline `mise run acceptance` passed with
`/private/tmp/cb-l6_m56ma/home`. The adoption scenario passed at
`/private/tmp/cb-adoption-t202ok3y/evidence.json`. Native acceptance passed at
`/private/tmp/cb-adoption-zl8514d6`, with `evidence.json` recording process/session
continuity, restart, completed report, `idle_closed` and exact Resume;
`native-ui-checks.json` records the native Card tab and Terminal workspace checks.
Keyboard input after adoption reached the original provider process, and the
original terminal name remained selected without a duplicate live row. Remote
bridge support is implemented but was not exercised against a live remote host.

## GitHub workflow integration, unified cardless `WorkflowRun`, and trust-scoped host env (2026-11-19)

Card #150 extends Canban's GitHub, Workflow, and Host Environment architecture across four coordinated boundaries:

1. **Unified Event System & Repository-Scoped Cardless Matching:** GitHub PR/Issue observations use one canonical event taxonomy with `actor = "github:<login>"` and body-free lightweight payloads. Repository-level polling (`/issues?since=...` filtering PRs, `/issues/comments?since=...`) runs independently of `intake: off` whenever repo-scoped subscriptions are active, using durable baseline watermarks in `source_observations` (`created_at >= baseline`) to distinguish `opened` from `discovered`. Cardless Project rules match via `selector.repository` without stamping `event.project_id`, and cross-GitHub feedback loops are suppressed via `exclude_self` plus receipt-tracked remote IDs (`comment_id` / `review_id`) in `source_observations`. See [Events and subscriptions](product/events-and-notifications.md) and [GitHub workspace](github-workspace.md).
2. **Unified `WorkflowRun` & Thin `ScheduledRun`:** `WorkflowRun` supports optional `card_id: Option<String>` and `trigger_event_id`, serving as the single execution engine for Card-bound runs, cardless GitHub event runs (`script -> condition -> agent` with `$CANBAN_OUTPUT`), and cardless scheduled runs. `ScheduledRun` remains strictly the cron firing, overlap, and 50-row history ledger pointing to `workflow_run_id`. See [Unified workflow engine](product/workflow-engine.md) and [Data architecture](data-architecture.md).
3. **Decoupled GitHub PR Review Auto-Publish:** `@reviewer` continues saving a local `review_draft.json`, while an optional toggleable (`enabled: true/false`) post-review `script` step calls `canban review publish` (server-verified workflow-step authorization queued through `github_action_gate`, clearing `review-draft:{cid}` on publish).
4. **Trust-Scoped Host Terminal Environment Policy:** Host Settings exposes **Inherit User Terminal Environment** (`Inherit` default, `Minimal` fallback, and **Refresh**) for automated `Agent`/`Script` steps on `local` / `collaborator` trust (forwarding inherited env safely without dumping host secrets onto Codex CLI `argv`). Unattended `Agent`/`Script` steps on `external_pr` or untrusted external cardless events default to minimal/`PATH`-only host env (overridable via explicit workflow/step `env:`), while interactive Card Terminals and AppServer code-controlled actions (`review publish`, `deliver`, `github.pr.publish`) remain unrestricted. See [Settings composition](workspace-entry-and-settings.md#settings-composition).

## GitHub workspace update

The [GitHub workspace and cache](github-workspace.md) extends the integration with a dedicated rail page, paginated notifications/Issues/PRs, PR source Cards, local GitHub references, Board source filters, repository rules and explicit remote comments/Review. Account controls moved out of Settings. Remote data uses disposable event-refreshed file caches instead of permanent business-table mirrors; schema 9 preserves bindings and local work. The GitHub/editor integration used schema 10/API 8, extended to schema 12/API 10 by Workflows; [combined acceptance](merge-integration-20261111.md) records migration and native verification.

## Build and launch

Use the pinned mise Rust toolchain. NSH 0.6.0 is pinned to `7620f0145d3a83b0283ca7c397460b87497e039d` in Cargo.toml and Cargo.lock (`nsh-core` with `svg` feature; `nsh-platform-macos` on macOS; `nsh-platform-linux` with GTK4/Pango/GStreamer and without QuickJS/WebKitGTK on Linux; `nsh-platform-ios` on iOS; `nsh-platform-android` on Android). See [NSH integration notes](nsh-notes.md) and the historical [0.5.0 upgrade record](nsh-050-upgrade-20261111.md). On macOS, install Xcode command-line tools and librsvg (`brew install librsvg`); on Linux, install `pkg-config`, GTK4, GStreamer (`gstreamer-1.0` + `plugins-base`), `librsvg` (`rsvg-convert`) and `zstd`, plus a compatible `hc`.

```sh
mise run dev
mise run dev-stop
```

`mise run build` creates `target/debug/{canban,canban-ui,canban-updater}` and an ordinary installable desktop bundle (`.local/Canban.app` on macOS, `.local/canban` on Linux). `mise run dev` builds the binaries and starts a checkout/branch-scoped Dev bundle under `.local/dev/<label>-<hash>/` (`CanbanDev.app` on macOS, `CanbanDev` on Linux), with its own home, native ID, bundle binding and process records. `mise run dev-stop` gracefully stops only that instance; `mise run run` is the same isolated start entry. Explicit homes override branch selection, and installed `~/.canban` is refused by the development manager. See [development startup](development-startup.md) and [remaining gaps](instance-isolation.md). On macOS all bundles are ad-hoc signed after assembly and pass strict/deep verification; on Linux `mise run install-app` and `scripts/install.sh` atomically install `~/.local/opt/canban`, `~/.local/bin/{canban,canban-ui}` symlinks, and XDG `.desktop` / hicolor icons (while `install.sh --headless` installs standalone `~/.local/bin/canban` on headless servers). Developer ID signing/notarization remains separate distribution work. The standalone `mise run gallery` packages `.local/CanbanGallery.app` on macOS or `.local/canban-gallery` on Linux and uses fixtures without AppServer/data. Several compatible UI clients may still attach to one service; a second AppServer cannot own the same home.

All bundle variants generate the macOS app icon from [the original SVG](../crates/ui/assets/app-icon.svg), including standard and Retina sizes. Packaging installs `Contents/Resources/AppIcon.icns` and declares `CFBundleIconFile` before bundle signing. See [asset provenance and generation](../crates/ui/assets/README.md#app-icon). The release workflow installs librsvg; distributed apps need no SVG renderer at runtime.

`mise run test-package` exercises actual Mach-O fixtures and macOS signature checks: clean ordinary/development/Gallery packages, Release-to-Debug repackaging, stale resource seal repair and relocation into a temporary Applications directory. The release workflow runs it before building the distribution bundle.

Packaging acceptance (2026-11-11): the new regressions failed against the prior packager (all three local bundle variants plus both repackaging cases), then all three tests passed after the fix. `mise run build`, `mise run build-release`, `mise run test` (207 tests), `mise run acceptance` and `mise run acceptance-dev` passed. The newly built Debug `.local/Canban.app` and the repaired installed `/Applications/Canban.app` both opened native windows through LaunchServices with an isolated home/Core; the installed app also opened Settings. Logs are `/private/tmp/canban-package-before.log`, `/private/tmp/canban-package-after.log` and `/private/tmp/canban-signing-*.log`; native fixture metadata is under `/private/tmp/canban-signed-launch-2i7wmcy8`. This establishes local signing/launch behavior, not Developer ID notarization. Live application data was not used.

CANBAN_HOME selects an alternate absolute application-data directory. Its default is ~/.canban/. Native UI connects through AppClient RPC to a separate, persistent AppServer. A same-user, mode-0600 Unix socket exposes JSON-RPC to the CLI. The home directory is mode 0700.

```text
canban-ui -> components/presenters -> AppClient -- RPC --> AppServer
     |                                         |
canban-ui-kit -> nsh-base                 owner crates / storage
                                               |
                         +---------------------+--------------------+
                         |                     |                    |
                    SQLite WAL             managed files      hc connect
                                                              JSON-RPC
                                                                  |
                                                             herdr-core
```


Current service lifecycle (2026-11-09): the desktop attaches to or starts an independent AppServer. Closing a window leaves scheduling active. Multiple UI clients share one authority; explicit CANBAN_ENDPOINT attaches only. Remote execution, directory browsing, saved workspace/Resume bindings and Terminal byte relay are implemented. See [multi-machine execution](multi-machine.md) and [acceptance](multi-machine-acceptance.md). Earlier same-process lifecycle descriptions are superseded.

## Configuration

First startup creates `config/agents.json` and stores the default Workflow bootstrap in SQLite (`workflow_sources` + `workflow_revisions`). No external Workflow JSON file is read or written. The local `hc` path resolves when available. Profile commands remain portable and resolve on the selected execution host at launch. Edit JSON and choose Reload in Settings (`Agents & Identities`), or call `config.reload`. Invalid configuration leaves the previous in-memory configuration active. Stage profile IDs are validated at startup, Project creation and configuration reload against the global registry; removing a profile referenced by the default/saved Project workflow is rejected. Requests already queued keep their profile/instruction/callback snapshot.

Fresh homes receive developer and reviewer starter profiles (both initially using the existing Codex launch setup), plus codex/claude profiles for direct mentions. Existing registries/workflow files and saved Project workflows are preserved without injecting or rewriting roles. When creating the SQL bootstrap under an older registry, generated stages omit references to unavailable starter profiles and use the global default.

The default workflow is Backlog -> Develop -> Review. Joining Backlog does not analyze or automatically move Cards; entering Develop runs `@developer` and moves the Card to Review; entering Review runs Queue Merge (`deliver`), which merges into the target branch and syncs upstream (`sync_upstream: true` by default), while requiring human approval first for untrusted external PRs (`source_trust == "external_pr"`). No `@reviewer` Agent step is included by default, though users can add one via YAML or Studio. New Projects receive a visible `development.yaml` source document in Project Workflows (or accept `workflow_yaml` directly in `project.create`). Source document names do not imply managed files.

Agent configuration has hc, hc_session, hc_env, default_agent and profiles. Profiles have id, optional preset, role_instruction, optional argv/resume_argv, exit and session_instruction. canban-profiles resolves editable JSON defaults and explicit whole-field overrides. Commands expand {prompt}, {home}, {cli} and {session_id} as individual arguments. The runtime has no provider-specific invocation branches or Herdr launch-template mode. See [profile schema and migration](product/agent-profiles.md).

Codex and Claude presets configure interactive launch, exact-ID Resume and normal /exit input. Neither injects notify/Stop hooks. Codex binds its actual tool-environment ID through bind-session; Claude's fresh command receives a preallocated UUID. Business completion requires an explicit Canban report. Completion keeps the interactive process available. A configured inactivity timeout later uses verified idle/done plus actual human-input timestamps to request normal exit; unfinished automatic work and pending input remain protected. Finished entries leave Terminals, while explicit Card Resume retains exact conversation history. See [current Terminal lifecycle/navigation](terminal-interaction.md). A provider without usable status keeps manual terminal control. Legacy config conversion writes an exact-byte backup before publication; historical Requests/conversations are preserved. Opaque legacy launch templates require explicit command conversion.
Routing precedence is explicit @profile -> stage.profile -> default_agent. Routing tags and Profile aliases have been removed; stages without a profile use the global default. Old stored tag fields are ignored when decoding; new Card mutations reject the removed field rather than silently choosing another Agent. Queued/running Requests retain their captured Profile. A submitted @profile mention immediately appends a Question to the live Card/profile invocation, or starts/resumes it if absent/exited. Stage-entry execution queues a request using that same provider conversation and a new callback binding. Retry is explicit and uses a new attempt; reconnect does not create one.

Boards share one Scheduler with a configurable global Agent limit (default eight). Starting/working/closing and uncertain invocations reserve slots; verified root blocked/idle/done/error states release capacity while retaining their Terminals. A completed report alone does not release a working runtime. Queued work waits without occupying a slot, and independent Shells are outside the Agent budget. Later template stages run first, with FIFO ties. Automatic requests and progression pause on persisted human gates until an explicit user decision. Settings and JSON share the same configuration. See [workspace/scheduling implementation and acceptance](workspace-and-scheduling.md); earlier unlimited-concurrency evidence describes the previous policy.

See [Card conversation workspace](card-conversation.md) for the current compact detail layout, explicit desktop recipients, comment shortcuts and native verification.

## Product behavior

- Create a Project in an existing or new absolute directory; Git is optional. Each Project has a Board and increasing local Card numbers.
- Create/edit Cards, move with stage buttons, the Card header stage switcher or Board card dragging, and inspect current execution/attention status.
- Notes are deferred pending product design. Their UI/editor/search/Card-selection flows are removed; new executions use Card content, Timeline, reports and attachments without automatically injecting legacy Notes. Existing Note records, associations and capture files remain intact. See [current scope and compatibility](notes-deferred.md).
- Comment and mention configured Agents. Reports and state changes join the same ordered Timeline. Pasted images in Card/comment editors save bytes as Card resources and insert an attachment reference into the draft; save/post the draft to retain that text reference. Resources also support file-path import and native opening.
- Card creation and task editing share a native Markdown editor in the Board right pane with retained expanded mode. Drafts persist independently in schema v9 and are restored through Drafts. `@` offers Profile references; `#` offers local Cards and connected GitHub Issues/PRs. Enter/Shift+Enter always insert newlines; Tab selects candidates and Command+Enter confirms. See [Card editor](card-editor.md) for current behavior and acceptance limits.
- AppServer returns structured errors for stale Card revisions. A failed save remains visible in the status area. In-flight UI operations do not block render/input paths; stale resource-load results cannot replace a newly selected Card's output. Incoming state updates do not rebuild active Card editor drafts.
- Card Agent links and Overview open the dedicated Terminal workspace. Users type/paste, interrupt, explicitly close or resume exact provider history; leaving the view only detaches it. X, Command+W and Close terminal share AppServer termination and receipt handling. Exited entries leave the list/tabs while Card/Overview retain saved history. `libghostty-vt` owns VT state, incremental render-state snapshots, DEC mode 2026 synchronized-output render holds, selection gestures, text formatting, paste encoding, mouse encoding and key encoding (both macOS Option keys act as Alt, while Option-drag/Option-scroll override mouse reporting for local rectangular selection and scrollback). On macOS, AppKit + CoreText render directly from the binary snapshot through an NSH native component, with CJK/IME preedit caret and optimistic commit-cursor hold, Finder file/text drag-and-drop (`NSDraggingDestination`) and `⌘V` file-URL path pasting with POSIX shell escaping and trailing-space separation via bracketed paste, procedural Box Drawing/Block/Braille glyphs, SGR underline styles/colors, Command-click hyperlinks (OSC 8 and HTTP/HTTPS URLs), OSC 52 clipboard writes, and reactive Terminal font family/size settings persisted in `config/desktop.json` (selected from a searchable picker of installed system/user monospaced fonts). Herdr owns the PTY, and runtime state never implies business completion.
- The Agent's explicit working/completed/blocked/failed declaration is recorded through the CLI. Completed invokes only its originating request's callback. Blocked/failed creates a durable attention notice. Completion receipts and commands with request keys have duplicate/conflict handling.
- For a Git repository with HEAD, task execution creates a named managed worktree and reuses it across stages. New branches use `<directory-name>-<project-id-first-4>/<card-number>`; managed paths mirror that name under `worktrees/`. Matching numeric suffixes resolve branch/path conflicts. Existing saved UUID/numeric/readable paths and branches remain unchanged. See [workspace naming](workspace-and-scheduling.md#readable-workspace-names). Non-Git repositories can use their Project directory with worktree/base synchronization disabled. Uncommitted source-directory edits are not silently copied into a new worktree. Archive retains dirty worktrees with a notice; clean worktrees are removed after preserving their HEAD under refs/canban/cards/<card-id>.
- Review offers a native basic Diff and explicit commit/merge into a clean target checkout. Stale previews and active/queued Card work block acceptance. Conflicts remain in the Card checkout, with durable failure/attention receipts. Accept does not archive. See [Review delivery](product/review-delivery.md).
- Archive creates a checksummed zstd-compressed tar package with manifest.json, records.json and Card-owned files. It verifies the package before publishing its index and removing matching expanded files. Report/attachment/output access can resolve compressed content on demand. Restore expands files and retains identity without silently scheduling work.

Structured database records remain online in this MVP for lookup, history, references and report deduplication, even though a complete Card record export is also compressed in the archive. This is an intentional refinement of the earlier proposed SQL-detail pruning. Large file bytes are compacted; SQL cold-storage pruning/space reclamation and lz4 encoding are later work. Late accepted reports remain durable outside an existing sealed package.

## Agent interface

While the UI or headless host is running:

```sh
target/debug/canban status
target/debug/canban snapshot
target/debug/canban call project.create '{"name":"Example","directory":"/absolute/path/to/source"}'
target/debug/canban call card.create '{"project_id":"PROJECT_ID","title":"Task","body":"Intent and context"}'
target/debug/canban call card.move '{"card_id":"CARD_ID","revision":1,"stage":"develop"}'
```

See [Agent profile configuration](product/agent-profiles.md) for loadable JSON examples, schema, routing precedence and backward compatibility. Settings displays the shared registry and roles, Board header tooltips display stage profile references, and the Card composer offers Ask @profile actions.

Public operations include update.state/check/install, app.ping/snapshot, project.create/list, card.create/get/list/update/adopt/move/archive/restore/attention.clear/continue/workspace, comment.add, timeline.list, execution.get/report/retry/output, attachment.add, resource.read/materialize, config.get/reload, scheduler.status/configure, herdr.status/check/install/templates, profile.presets, review.preview/merge, terminal.create/rename/context/interrupt/end/close and agent.open/bind/reply/interrupt/end. Note RPCs, card.context.update and the hidden canban note command remain legacy compatibility paths, outside the current product scope. agent.turn_ended is a deprecated ignored compatibility endpoint. For timeline.list supply card_id, optional kinds, after_sequence and limit; follow next_cursor and persist last_sequence when the scan ends. Card lists return items/next_cursor and require full pagination. Card details and mutation responses use the explicit views in the [response contract](agent-response-contract.md). Current code and shared types are authoritative for exact payloads; the [interface design](interfaces/mvp-contract.md) remains a broader contract reference.

The launcher injects CANBAN_CLI, CANBAN_HOME, CANBAN_ENDPOINT, CANBAN_PROJECT, CANBAN_CARD, CARD_ID, CANBAN_SESSION_ID, CANBAN_PROFILE_ID and CANBAN_RUN_DIR across scheduled Agent terminals and Card terminals, and supplies reporting instructions in the actual prompt.

```sh
"$CANBAN_CLI" card show --fields number,title,body,stage,revision
"$CANBAN_CLI" card update --file "$CANBAN_RUN_DIR/card.md"
"$CANBAN_CLI" card adopt
"$CANBAN_CLI" report --state working --message "Investigating"
"$CANBAN_CLI" report --state completed --message "Finished" --file "$CANBAN_RUN_DIR/report.md"
"$CANBAN_CLI" report --state blocked --message "Need a human decision"
```

The CLI sends file contents, not just a temporary path. Completed gets a stable default request key for receipt retry; changing the payload under that key is a conflict. Pass --request-key for an explicit operation identity where needed. Terminal addresses bind to owning core.id; Conversation stores the distinct exact provider session ID. Runtime observations omit argv/env copies of potentially long prompts.

## Verification performed

```sh
mise run test
mise run lint
mise run sim
mise run test-integration
# Explicitly opt in to real-model usage:
mise run acceptance-real-interactive
mise run acceptance-real-workflows
```

The acceptance script creates a fresh temporary Canban home and isolated HC_SESSION/HC_SOCKET/state/config. It starts a deterministic fixture program through the real installed Herdr core and PTY, and checks queued launch, work product, captured profile role instructions, distinct template-selected Developer/Reviewer executions, explicit CLI outcomes, automatic Develop -> Review, fresh mention, blocked notice, WAL, report compression/read/restore and restart without duplicate scheduling. It stops only that scratch core. It does not call a paid model.

Separately, an existing Codex 0.166.0 Agent was launched through the isolated core with normal workspace-write permissions. It read the captured context and invoked the bundled CLI with a Markdown report; Canban recorded completed. This establishes the actual provider/report path, beyond fixture-only success. It does not establish live acceptance for every installed Agent or template.

Subsequent live acceptance also completed two real coding tasks concurrently on separate Boards, automatically launched two fresh Codex review Sessions, and archived both Cards after independent validation. Four real Sessions and their report/context/worktree associations were checked; reports remained readable after compression. Run mise run acceptance-real explicitly to reproduce. The [evidence summary](real-agent-acceptance.md) distinguishes this from fixture acceptance and from cross-provider/security isolation claims.

Earlier native Canban.app interaction checks (before the named-theme update) covered Card creation, multiline Unicode paste/edit/save and preview, comment mention, cross-stage drag, Notes save/preview, light/dark Gallery rendering, keyboard Tab/Space activation and skipping disabled controls, copying an image in Preview then pasting its bytes into a Card, opening the retained attachment in Preview, per-Card Note checkbox selection, and closing/relaunching the standalone window. Image references/resources persisted and active editor text survived the resulting state refresh. These were real native interactions, not a browser mockup or screenshots alone.

Rust regression coverage includes state/history/request consistency and duplicate operations/reports, stale revisions, fresh mention callback isolation, blocking notices, routing failures, compressed resource restore, path isolation, workflow cycle rejection, large Unix-socket JSON frames, legacy Note retention/exclusion from new captures, and managed-worktree reuse/dirty retention/commit preservation. The large-frame case caught and prevents a macOS nonblocking accepted-socket truncation bug found during live integration.

### Deterministic Simulation Testing (DST)

Full-stack multi-host simulation coverage lives in `crates/sim` (`canban-sim`), running real `AppServer` instances, SQLite WAL databases with `StorageFaultInjector` (`SQLITE_BUSY` / `SQLITE_IOERR`), per-instance `Failpoints` covering all 5 dual-write crash windows (`herdr.after_terminal_run_before_db_commit`, `herdr.after_send_before_db_commit`, `github.after_pr_write_before_db_commit`, `git.after_push_before_batch_finalize`, `outbox.mid_write_truncate`) and deterministic DST Vault RNG (`XChaCha20Poly1305` keys/nonces derived from `IdGen` in DST, `OsRng` in production), real `canban-transport` + `MultiHostClient` auth/pairing/revocation/Tailscale/API-handshake paths plus **framed local transport** (`AppServer::serve_local_stream` over `turmoil::net`), real `canban-workspaces` worktree, inline + Herdr PTY script, review merge, GitHub PR delivery, and `merge_batch` delivery flows, real `project.export`/`project.import`/`project.archive`/`project.restore` bundles, and `canban-client::MultiHostClient` instances inside deterministic single-threaded `turmoil` worlds with `TurmoilClock`, fail-closed in-process `SimExec`, `SimTailscaleVerifier`, `SimHerdr`, and `SimGitHub`. It covers **17 focused scripted scenarios** (`card_automation`, `remote_faults`, `github_sync`, `client_pages`, `env_sync`, `notifications`, `herdr_lifecycle`, `reclaim`, `schedules_and_budget`, `workflow_faults`, `multi_client_sync`, `transport_auth`, `workspaces_delivery`, `project_import_export`, `sessions_and_adoption`, `review_and_events`, `scheduler_cross_contention`) plus a **6-sub-profile Swarm-Testing `chaos` generator** (**18 scenarios total**) with deterministic prefix+ddmin shrinking (`canban-sim shrink` / `--shrink` / `--auto-shrink`), permanent regression corpus replay (`canban-sim corpus`), Armed-vs-Fired fault telemetry (zero vacuous armed faults), Cold Properties Top 10, `never!` negative coverage gate (`exec.blocked_command == 0`), Read-Your-Writes / At-Most-Once / AuthFailed zero-dials / committed `MergeBatch` oracle models, and per-seed watchdog timeouts (`--seed-timeout`). CI runs both release-mode gates on macOS and Linux: **Gate A** (`canban-sim run --seeds 1 --assert-coverage --seed-timeout 60`, 18/18 scenarios & 79/79 properties) and **Gate B** (`canban-sim random --scenario chaos --seeds 12 --jobs 4 --master-seed 2026 --seed-timeout 60 --assert-coverage`, 12 chaos seeds & 34/34 base + chaos properties; local >=24-seed deep-fuzz runs additionally enforce all 6 Swarm sub-profiles at 40/40 properties). See [Deterministic Simulation Testing](dst.md) for the OS-boundary architecture, dual-write failpoints, universal invariant oracle, local deep-fuzz workflow, and explicit DST vs native/integration verification boundaries.

## Remaining boundaries

This is the macOS MVP with interactive Card/profile conversations, multi-machine execution and the [implemented GitHub Issue/PR integration](github-integration.md). Shear review, native notifications, template designer/migration UI, advanced timers and multi-authority data sync remain deferred. Release automation (`Release` GitHub Actions workflow running unit, packaging and headless integration tests before reserving/publishing a GitHub Release) and desktop install/recovery are implemented with local fixture/native acceptance; public Developer ID signed distribution remains unverified. Claude's preset is implemented and configuration-tested, but live Claude acceptance has not been established. Provider startup/trust dialogs remain interactive; Canban does not automatically authorize them. Notes are deferred; legacy data remains stored without an editor or automatic capture injection. Terminal presentation supports SGR/pixel mouse-reporting, alternate-screen scroll, clickable OSC 8 / HTTP(S) links, procedural Box/Block/Braille rendering and live font configuration, while kitty graphics and Windows/Linux native terminal views remain deferred. Bounded 12,000-line output, native scroll/zoom, Core outage and live-host restart have acceptance in the latest update. Saved output is bounded terminal observation; explicit reports are authoritative outcomes.

## Earlier interactive acceptance (before the profile refactor)

Real Codex 0.166.0 completed a Unicode greeting task in a managed worktree, replied to an active @developer question from the selected Note, ran its tests, and triggered a separate Reviewer conversation. Both exited normally after their final turn. A subsequent @developer resumed the exact original provider ID in a new terminal, recalled the Note marker and completed without replaying Develop's callback. Archive retained readable replies. Evidence: /private/tmp/cb-real-interactive-xf_mcgnu/evidence.json (temporary local artifact).

The deterministic interactive harness uses real Herdr/PTYS and a fixture provider to exercise active replies without task completion, interrupt/continue, duplicate opens, independent roles, retained completed conversations, explicit/idle exit, exact-ID resume, restart and archive/restore. Earlier native UI acceptance also exercised Unicode input, Ctrl+C/continue, Card comment -> separate Timeline reply, detach/reattach, explicit completion -> NORMAL_EXIT, and the same session UUID on resumed terminal. Fixture evidence is not real-model evidence.

Historical integration at that point: schema version 2 added conversations/questions. Attempt records retain provider ID, conversation ID, close state, last turn boundary and final-response path. The old CLI helpers bind-session, reply --to QUESTION_ID --file PATH and agent-notify used the current invocation's environment. Codex notify supplies thread-id and last-assistant-message; Claude accepts only its root Stop callback. Physical control operations commit their request-key intent before sending, so retrying an uncertain action does not replay it.

A final real-Codex acceptance rerun passed after isolating inherited CODEX_THREAD_ID. The production native view also rendered the actual Codex TUI, accepted Unicode input, reattached after the desktop host restarted, bound its exact thread and completed/closed normally. Native acceptance caught and fixed a hidden-cursor NSNull crash and drawing outside terminal bounds. Selection/copy and bracketed paste were exercised; an automation clipboard conflict was observed and cleared before submission. This does not establish a full IME matrix.

Claude Code 1.0.108 smoke check: the production adapter launched its interactive TUI in the isolated native app and accepted the owned test-folder trust prompt. The model request returned Invalid API key / Please run /login. This is a concrete local authentication blocker: actual Claude report/normal-exit/resume acceptance is not claimed. The later refactor removed the Stop-hook design. Existing credentials/settings were not changed.

Final native Gallery acceptance verified the production hidden-cursor fixture after scrolling into view without clicking it. NSH's layer-transform scroll can precede its AppKit bounds commit; the terminal tracks both geometries and schedules redraw across that transition. Final workspace validation: 19 AppServer tests, 2 terminal tests and 1 worktree test passed; workspace Clippy with warnings denied passed; both deterministic acceptance tasks passed. The final development bundle is .local/Canban.app. Scratch hosts/cores were stopped after validation.

## NSH upgrade (2026-11-07)

Upgraded the three direct NSH dependencies together from 0.3.0 at b7a69186d3c20a59690f2ac834dc527bee7f8438 to 0.4.0 at aca7b8f92a33af9d83c3693a516f649f7ea5c5e5, the upstream main HEAD observed during this upgrade. Cargo.lock pins the resolved graph. No Canban compatibility code changes were required. Workspace check, all 22 tests, Clippy with warnings denied, acceptance and acceptance-interactive passed against this revision. The packaged native app launched with an isolated fixture home and displayed the Board, Card editor and Agent controls. The user then requested stopping further checks and continuing the design discussion; further native interaction/IME/Terminal acceptance was not performed for this dependency upgrade. The scratch app was stopped.

## Latest profile/delivery/recovery acceptance

See [MVP acceptance](mvp-acceptance.md) for the executed post-refactor matrix and exact temporary evidence. Real Codex completed human-input and Review/rework tasks; both actual work products were explicitly committed/merged, tested in their target repositories, archived and restored. Native Review was operated against real Git changes, including stale-preview rejection. Terminal checks found/fixed scroll direction and attachment-layout races, and verified same-binding reconnect, input discard while disconnected and native-host restart. History deletion and explicit fresh-context recovery passed in real-Herdr fixture acceptance.

Use `mise exec -- python3 scripts/acceptance-native.py` to prepare an isolated production-native app for CUA/manual checks. Metadata is written under `.local/native-acceptance.json` and the temporary base. Creating a `restart` marker there before quitting reloads the host while keeping its scratch Core alive; normal exit stops only that test Core. `CANBAN_APP_ID` allows isolated development bundles to coexist with the regular app. This harness does not establish acceptance without performing and observing the interactions.


## Notes and Kanban workspace update (2026-11-07)

Historical update: the Note portion below was withdrawn from product scope on 2026-11-10; see [Notes deferred](notes-deferred.md). The dated acceptance remains historical.

The approved calm native composition is implemented: Board with a selected-Card pane, focused Card/Terminal or Review, explicit return/expand controls and retained navigation drafts/Board scroll. Notes remain database-backed documents with file-like Agent CLI operations, immutable revisions, stable links/backlinks and transactionally associated history. The [implementation and acceptance report](notes-and-workspace-implementation.md) records contracts, real Herdr fixture integration, native interaction checks and remaining limits.

Navigation refinement (2026-11-08): the rail contains only Quick Open, Board, Overview, Notes, Archive and Settings. Project switching/creation lives in the Board header; icons share vendored Lucide SVGs rendered by NSH. [Navigation behavior and native regression acceptance](quick-open-and-navigation.md#navigation-refinement-acceptance-2026-11-08) records the isolated two-Project run, modal Escape fix and long-title layout regression.

[Project entry, grouped Overview and Settings](workspace-entry-and-settings.md) (2026-11-08) documents the clickable empty Board, native directory chooser, Overview without filters and a single scrolling Settings page, including isolated native acceptance.

## Terminal workspace chrome and manual Shells (2026-11-08)

The [Terminal workspace](terminal-workspace.md) now shares a Project/Card inventory, numbered tabs and a single native renderer across Scheduler Agents and manually opened Shells. NSH's custom window titlebar is split at the same resizable boundary as the content; Card and branch stay in chrome, with secondary controls in native menus. New tabs open immediately, rename happens afterwards, and closing a tab terminates its associated live PTY before removing the tab. The technical status footer is removed; failures remain dismissible notices. SQLite schema v4 preserves entry names/numbers and manual Core/PTy bindings. Manual Shells do not consume Scheduler slots, create workflow Requests or inherit Agent idle reclamation.

### Independent Terminal context update (2026-11-08)

Terminal creation no longer requires Project: `terminal.create {}` starts at home; optional `cwd`, `project_id`, `card_id` or `inherit_terminal_id` selects the launch directory. Project/Card arguments do not persist ownership. `terminal.context` returns Herdr-observed cwd, optional Project/Card hints and branch; unavailable runtime returns null hints with an error. Schema v5 removes TerminalSource and stored Shell cwd, retaining optional Conversation association and independent runtime binding. In-memory snapshot contexts drive Project/Other grouping and chrome. See [Terminal contract](terminal-workspace.md) for migration, inference and acceptance details.

### Agent labels and independent Terminal sequence

Schema v6 adds a separate durable manual Terminal sequence. Agent inventory entries show two lines (Card/title + execution stage; provider/Profile + status), with one-line bounded tabs. Independent entries display Terminal N without a type icon. Custom names override the title portion and clearing them restores the generated title. Codex/Claude vector marks are shared UI Kit assets with preserved Herdr notices. Branch/directory labels shorten visually while full values remain in hover/actions. See [Terminal workspace](terminal-workspace.md) for contracts and current validation.


## Release and desktop updater (updated 2026-11-14)

[Releases and updates](updates.md) defines the implementation, migration authoring rules and acceptance. `.github/workflows/ci.yml` runs unit (`NEXTEST_PROFILE=ci` with retries and `fail-fast = false`), packaging, deterministic headless integration (`mise run test-integration` via `scripts/run-acceptance.py`), installer and baseline upgrade tests on every push to `main` and on pull requests, surfacing retried flaky tests as Step Summary warnings. The manually dispatched `Release` workflow runs `scripts/release.py ci-gate` to require green CI on the exact commit SHA (waiting while CI is pending) before reserving `YYYYMMDD-N`, building the macOS and Linux desktop/CLI targets, validating the packaged archive upgrade gate, and publishing only complete official GitHub Releases (`prerelease: false`); transient failures recover via `gh run rerun --failed <run>` with idempotent `reserve` and `publish`. Persisted rolling-24-hour and unrestricted manual checks feed the global bottom-rail Update action. Installation immediately replaces the initiating UI/AppServer without draining Herdr Agents; durable idempotent reports/replies survive the handoff. Paired database-only up/down batches are transactional, with no automatic database backup for release installation. Failed candidate startup rolls back schema/bundle and verifies the restored version; failed down stops with a standalone database-free Recovery window.

The earlier GitHub/editor integration used schema 10/API 8; Workflow sources and subscriptions now extend that to schema 12/API 10. The service lock is acquired before opening SQLite. The earlier integration's six isolated update/failure cases and native sidebar upgrade covered schema 10, including unsent-draft restoration and exact Agent terminal/Core/worktree preservation. The [Workflow/event-system merge acceptance](event-system-merge-acceptance-20261111.md) separately records schema-12 local backend activation and native Workflow checks; it does not rerun the release download/bundle-swap matrix. See the linked evidence for the simulated future-schema boundary and unverified GitHub publication.

### Local backend activation (2026-11-11)

Ordinary local Apps show **Update backend** only when the answering backend is
older. The action uses the installed App through the existing updater and never
downloads or replaces a bundle. Actual equal schema skips migration; pending
schema changes take a database snapshot for failure recovery. Equal/newer
backends permit normal entry; a newer backend shows a newer-App notice and is
never downgraded. Dev instances still restart via `mise run dev`. See
[the current update contract](updates.md) and
[executed backend acceptance](backend-update-acceptance-20261111.md).
