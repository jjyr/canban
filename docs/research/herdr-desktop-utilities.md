# Desktop utilities and cross-Project Card creation

Implementation update, 2026-11-09: the selected utility bar, provider probes, Theme, Shortcuts, New Card Project selector and Report issue are now implemented. See [current implementation and acceptance](../desktop-utilities.md). Proposal/pre-implementation language below is retained as dated research, not current runtime status.

Status: source-backed design, 2026-11-09. The user accepted the shared utility-bar direction, specified that unsigned-in Codex/Claude providers must remain hidden, and added Report issue. The requested scope also includes Caffeine, Theme, Shortcuts, Command+N and a target Project selector in New Card. Individual implementation recommendations below remain identified as proposals; no implementation of these new utilities is claimed here.

The preceding GitHub/Card/Terminal work is committed as `3589518`. Reference checkout: `~/Workspace/herdr-gpui`, clean at `a6db99b4edab37b2038ac1016bd0af82ed0a5ebc`. This research inspected source only; it did not read provider credentials, call subscription endpoints or change Herdr.

## Reuse boundaries

| Feature | Reference implementation | Reusable part | Canban adaptation |
| --- | --- | --- | --- |
| Usage | `crates/herdr-usage`, `docs/usage-providers.md`, `crates/herdr-gpui/src/usage` | Provider response models/parsers, bounded probing, host-scoped cache, stale/error states, refresh backoff | Replace `herdr-client` host transport and GPUI rendering; start with Codex/Claude rather than every provider |
| Caffeine | `crates/herdr-gpui/src/caffeine.rs`, `caffeine/work.rs` | `keepawake` guard, dedicated worker, pending/on/off transitions, idle-vs-unknown distinction | Define the owning Canban process/machine explicitly; UI only requests a state change |
| Theme | `crates/herdr-gpui/src/config/theme.rs`, `theme_picker.rs`, `settings_window/pages/theme.rs` | Palette parsing, system light/dark pairing, previews/selection patterns | Apply one Canban theme through semantic UI Kit tokens and the native Terminal palette; GPUI views do not transfer |
| Shortcuts | `crates/herdr-commands/src/lib.rs`, `shortcuts/macos.rs`, `crates/herdr-gpui/src/keymap.rs` | One command catalog used for defaults and shortcut presentation | Use NSH bindings and Canban commands; do not import Herdr's worktree/window actions |
| Target Project | Existing Canban New Card presenter | Shared Card creation operation and source-generation checks | Introduce composer-owned target Project; keep it separate from Board navigation |

The reference repository is Apache-2.0. Provider ports also carry CodexBar MIT provenance. Any implementation that copies code/assets must retain the applicable upstream notices. Use pinned/vendored, reviewed code where useful; sibling checkouts must not become build-time path dependencies. Reusing protocol/algorithm code does not imply reusing GPUI widgets.

## Usage means account subscription capacity

Show provider/account usage windows, used percentage and reset times, plus plan/balance fields when reported. This is not per-Card token accounting, a Scheduler concurrency limit or a sum of activity across Agent Profiles. Developer and reviewer using the same account must not become duplicate subscriptions.

Herdr's provider worker is independent of GPUI, but not independent of every Herdr crate: it depends on `herdr-client` host targets, command execution and helpers. Copying the entire crate would also import many unrelated providers and cookie/decryption dependencies. Recommended first slice:

- A provider-focused `canban-usage` boundary with narrow Codex/Claude readers and typed results; AppServer owns polling and cached public readings. UI Kit only supplies generic meters/popovers.
- Identify readings by machine/provider/account. A popover exposes the selected machine and lets the user change it; default to the active Terminal's machine, then the selected Project's execution machine, then AppServer host. An unreadable remote account must never be silently replaced by a local one.
- Keep percentage/reset/account summaries in UI state, not API tokens. Remote credentials stay on their execution machine; adapt probing through Canban's existing host-command boundary rather than inventing another machine registry.
- Begin with Herdr's conservative polling model: about ten minutes for ordinary reads, longer backoff for rate limits/rejected credentials, and spacing for manual refresh. Preserve the last good reading with a stale marker after failure.
- Usage remains informational. Do not automatically block scheduling or move Cards when a quota is low.

Source-specific credential constraints:

- `herdr-usage/src/providers/codex.rs` reads Codex's OAuth sign-in from `$CODEX_HOME/auth.json`, with an OpenCode OAuth fallback, then requests ChatGPT subscription usage. An API key is not a subscription-quota credential. Those endpoints and response shapes were inspected in source, not live-accepted for Canban.
- `herdr-usage/src/providers/claude.rs` first reads Claude Code's existing macOS Keychain login through `/usr/bin/security`, then falls back to its credential file if available. The ordinary `Probe::keychain` path is distinct from the consent-gated `foreign_keychain` path: its comment explicitly describes Claude Code items that `security` may read without prompting. Reading Keychain does not necessarily mean repeated authorization dialogs, though actual access depends on the item's permissions. Do not copy Agent tokens into Canban's GitHub credential file or promise that every Claude installation can work without Keychain.
- Read existing sign-ins when the feature is enabled; do not introduce hidden browser-cookie scans or new Agent account login ownership in the first slice. A protected or missing sign-in yields an explicit unavailable/access-required reading and a provider Usage-page link. Credential-access behavior needs a separate user-facing choice before implementation if Keychain reads are undesirable.

## Caffeine owns a power assertion, not an Agent

Herdr uses `keepawake`, not a Herdr Core RPC. One worker owns the guard; dropping it releases the OS hold, including when its owning process exits. Its GUI-level state is shared across windows. Default scope includes the display; its system-only option allows the display to sleep. Modes are indefinite and until working Agents finish. Idle/blocked/done do not count as working, while unknown connections do not prove that work has finished.

Recommended Canban starting point, still a proposal:

- The button is a shared global utility, with an obvious active appearance and a tooltip naming the affected machine.
- AppServer owns the assertion on **its own host**, so closing a UI window does not release a hold intended to protect background execution. Do not label it "this Mac" when UI and AppServer may be on different machines.
- Start with explicit on/off, system-awake/display-sleep scope. AppServer exit releases the hold. Remote-machine power control and automatic until-Agents-finish mode can follow separately.
- If the desired product is instead "keep the UI laptop/display awake", that is a UI-host utility with a different lifecycle. It should not be conflated with the AppServer-host proposal.

## Theme is one design system

Canban currently has semantic adaptive light/dark colors in `ui-kit/tokens.rs`; the native Terminal has its own libghostty color model. Herdr supports built-in and Ghostty-file palettes, light/dark pairs, search/favorites and previews. A direct copy of its GPUI picker would not solve Canban's native color propagation.

Recommended sequence:

1. Persist System / Light / Dark appearance and expose the same action from Settings and a global utility entry. Use NSH appearance changes for the shared UI.
2. Introduce a palette model behind semantic tokens: background, surfaces, borders, text, muted text, accent, attention and error, plus Terminal foreground/background/cursor/ANSI colors. Cards, dialogs, tabs and menus must read it consistently.
3. Add a small curated preset picker with previews and coordinated Terminal colors. Reuse selected Ghostty parser/contrast logic where helpful. Theme changes must not recreate PTYs, reset scrollback or discard drafts.
4. File import, favorites and a large searchable theme library are later depth, not prerequisites for a coherent first picker.

## Shortcuts have one source of truth

A small UI-owned command catalog should declare command ID, title, default chord and availability/context. Use it for key bindings, button hints, menu labels and a searchable Shortcuts dialog. This avoids a static help list drifting from actual behavior. No separate crate is needed for this catalog initially.

Proposed defaults:

| Shortcut | Action | Availability |
| --- | --- | --- |
| Command+N | Open/reveal New Card | Any Canban page; keep an existing composer draft |
| Command+O | Quick Open | Existing global behavior |
| Command+/ | Show Shortcuts | Any page, including Terminal focus |
| Command+T | New Terminal | Terminal workspace |
| Command+W | Close Terminal through `terminal.close` | Terminal workspace |
| Command+Shift+[ / ] | Previous/next Terminal tab | Terminal workspace |
| Escape | Dismiss active overlay | Existing overlay behavior |

The native Terminal intercepts only declared UI command keys before PTY input. Add N and / to that declaration when binding their application commands; otherwise the native focus can prevent those shortcuts reaching NSH. Keep dialog focus/dismissal behavior and shortcuts available from a click target as well as a key chord. Custom shortcut editing can follow after the catalog/help/bindings agree.

## New Card chooses a destination without switching the Board

```text
Current Board / Notes / Overview / Terminal
                 |
            Add Card / Command+N
                 |
        one New Card composer
        +------------------------------------+
        | Project  [Current Project       v] |
        | [Local] [GitHub Issue]             |
        | title / description or Issue       |
        | local instructions / routing tags  |
        |                                    |
        | Cancel          Create in Backlog  |
        +------------------------------------+
                 |
          existing Card creation operation
                 |
       open result in its owning Project
```

The initial target is the currently selected Project, including when opening the composer from another page. With no selected Project, require a target choice before submission and provide the existing New Project entry if needed. The Project selector can reuse a compact searchable Project picker without reusing its global navigation side effect.

Composer state owns `target_project_id`, separately from `ui.project`. Switching it:

- Retains Local title/body/tags and local instructions. It does not submit, navigate or replace the current Board.
- Reruns GitHub search in the target repository, clears the selected Issue/old results and invalidates outstanding queries. Existing source-generation checks still validate final Add.
- Keeps Local/GitHub type buttons and the same final Backlog submission. A Project without a GitHub source shows the existing connect-source path for that target Project.
- Does not silently attach a source Note from another Project. Existing creation rejects that association. Preserve copied text, but visibly remove the cross-Project Note link when changing the target, or ask to keep the original target if retaining the link is essential. Cross-Project Note linking is not introduced here.

Current code captures `ui.project` at composer construction (`ui/github.rs::new_card`) and `ui.current_project()` for source search; both must resolve through the target state. `Ui::open_card` already selects the result's owning Project after successful creation. A global `Ui::select_project` during composition would reroute/remount views and is the wrong implementation hook.

## Presentation and delivery order

The follow-up layout proposal below places the shared utility group in a thin bottom bar, leaving the rail for major page navigation. This refines the earlier unspecified utility-group placement. Use popovers/dialogs over the existing page and preserve the split Terminal titlebar.

Recommended implementation order: command catalog/Shortcuts + Command+N/target Project, then keep-awake, then coordinated appearance, then subscription Usage with provider/machine acceptance. Each uses production components in Gallery and real app interaction checks. Before calling Usage accepted, verify real account reads and credential/access failures on the intended machine; mocks and source inspection are insufficient.


## Follow-up: exact Claude behavior

Additional source inspection at the same reference revision confirms:

1. Read the host's `CLAUDE_CONFIG_DIR` and settings. If an alternative `ANTHROPIC_BASE_URL` or `apiKeyHelper` means the CLI is not using Anthropic OAuth, avoid querying Anthropic with a leftover OAuth token.
2. On macOS, ask `/usr/bin/security` for `Claude Code-credentials`, preferring the current user's account. An explicit config directory selects the matching hashed service name instead of falling back to another profile's generic account. The hash uses NFC-normalized directory text and the leading SHA-256 bytes.
3. Fall back to `<CLAUDE_CONFIG_DIR>/.credentials.json` when available. Extract `claudeAiOauth.accessToken`, subscription type and rate-limit tier; use CLI metadata for the account email.
4. Query `https://api.anthropic.com/api/oauth/usage` with the existing access token and Claude OAuth headers. Parse session/five-hour, weekly, model-scoped limits and reset times, plus optional additional-usage fields. This provider does not implement a second Claude sign-in or its own OAuth refresh-token rotation.
5. If `cswap` is installed, also read `cswap list --json`. Its CLI owns its credential locks, refresh and usage cache; Herdr consumes active/additional account summaries. It does not install cswap or assume it exists.

References: `crates/herdr-usage/src/providers/claude.rs:77`, `:125`, `crates/herdr-usage/src/probe.rs:420`, `:491`, and `crates/herdr-usage/src/providers/claude/cswap.rs:88`. Generic foreign-Keychain consent persistence is in `crates/herdr-gpui/src/usage/access.rs`; that consent mechanism should not be described as Claude's ordinary read path.

Canban should reuse this ownership model: consume Claude's existing sign-in/optional cswap summaries, expose only public usage data to the UI, and keep credentials on their owning machine. If a protected read fails, show an unavailable reading and an explicit retry action rather than repeatedly generating background prompts. That retry/prompt-suppression behavior is a Canban proposal, not a claim that the ordinary Herdr Claude keychain path persists denials.

## Follow-up: integrated utility bar

Proposed shared bar: 28 points high, below the workspace content and beside the existing 56-point navigation rail. Usage occupies the flexible left side; Caffeine, Theme, Shortcuts and Report issue occupy the right. Settings stays at the foot of the rail. Use the same icon set, semantic colors, hover/focus behavior and popup primitives as the rest of Canban. Preserve generous clickable targets inside the compact bar.

```text
Board / Notes / Overview
+------+-----------------------------------------------------------+
|      | Project / page title                        Add Card       |
| Nav  |                                                           |
|      | Board columns / Card details / Notes / Agent Overview       |
|      |                                                           |
| Gear +-----------------------------------------------------------+
|      | Usage: Local  Codex 22%  Claude 68%    [Cup][Theme][Keys][Bug] |
+------+-----------------------------------------------------------+

Terminals
+------+------------------+----------------------------------------+
| native window controls | tabs                    Card / branch   |
+------+------------------+----------------------------------------+
| Nav  | Terminal list    |                                        |
|      |                  |           Native Terminal              |
|      |                  |                                        |
| Gear +------------------+----------------------------------------+
|      | Usage: devpc  Codex 22%  Claude 68%    [Cup][Theme][Keys][Bug] |
+------+-----------------------------------------------------------+
```

The bottom bar does not change the two-part Terminal titlebar or consume a new Terminal sidebar section. It shows useful account capacity and actions, without technical storage/readiness copy. At narrow widths, shorten Usage to provider icons/percentages and collapse it to one Usage button if needed; keep all data available in its panel. Never squeeze the clickable controls below usable sizes.

### Usage panel

A provider segment opens its anchored panel above the bar. Its header contains **Usage machine** and account selection; its body contains explicitly labeled used percentages, reset times, the last update/stale/error state, Refresh and the provider's Usage-page link. The most constrained reported window can be the compact summary; a tooltip identifies which window it is. Only providers with a detected existing sign-in are displayed. An unsigned-in Provider has no segment, login placeholder or setup prompt; if neither Codex nor Claude is signed in, omit the entire Usage group and its machine selector. A failed refresh of a previously detected sign-in retains the last good reading marked stale/unavailable, never a fabricated zero. Confirmed logout removes the provider and its old displayed reading. A protected credential read or unreachable host must not be misclassified as confirmed logout.

The machine follows the active Terminal on the Terminal page and the selected Project execution environment elsewhere, falling back to AppServer host. An explicit pinned Usage-machine choice can override following context. This changes only the reading, not the selected Project, Terminal or execution destination. The bar's machine selector is specifically a **Usage** selector: it does not retarget Caffeine. Switching machine/account fences outstanding responses so another account's numbers cannot appear beneath the new label.

### Caffeine, Theme and Shortcuts

- Caffeine: one-click explicit on/off, with an active tint and tooltip naming the AppServer host and hold scope under the current ownership proposal. Its target stays explicit even when Usage displays a remote machine. More modes belong in its secondary menu or Settings; they do not fill the bar with text.
- Theme: an anchored picker with System / Light / Dark and preset swatches, with detailed Appearance settings reachable from it. Apply one palette to UI and Terminal without recreating sessions.
- Shortcuts: the keyboard icon or Command+/ opens a searchable centered dialog, grouped by global, Board/Card and Terminal actions, using the command catalog. Command+N opens/reveals the existing New Card composer. A selected non-Terminal page does not change the meaning to New Terminal.
- New Card: show the target Project selector above the existing Local / GitHub Issue segments. It defaults to the current Project; choosing another preserves the composer text and updates the target-specific source choices as described above.

Only one utility popup/dialog should be active. Escape and outside click dismiss it; dismissal restores the previous editor/Terminal focus. Popups suspend Terminal typing and application commands that would accidentally reach the PTY, reusing the existing native focus/input gating. Opening a utility never closes a Terminal or changes a workflow gate.

The shared utility-bar direction and the visibility/feedback requirements below are user-selected. No utility bar, Claude credential read, new shortcut, Project selector or feedback control was implemented by this research update.


## Selected follow-up: hide unsigned-in Usage and add Report issue

The user selected these refinements after reviewing the integrated layout:

| Usage observation | Presentation |
| --- | --- |
| Codex and Claude both have no existing sign-in | No Usage group or machine selector; keep the right-hand utility actions |
| Only one Provider is signed in | Show only that Provider |
| Known sign-in, successful fetch | Show its quota/reset reading |
| Known sign-in, transient refresh failure | Retain the last reading with stale/error detail; do not invent a zero |
| Explicitly observed sign-out | Remove the Provider and its old reading |
| Credential access denied or host unreachable | Not proof of sign-out; preserve a previously known reading as unavailable/stale |

Report issue is a small bug-outline button immediately after Shortcuts at the right end of the bar. It remains available without Usage accounts, a selected Project, running Agents or Canban GitHub App authorization. Hover/accessibility text reads **Report issue**. Its color and interaction states follow the other utility controls, without an alert badge in the normal state.

```text
Signed in:
| Usage: Local  Codex 22%  Claude 68%       [Cup][Theme][Keys][Bug] |

No Codex/Claude sign-in:
|                                         [Cup][Theme][Keys][Bug] |
```

The local `origin` identifies the destination as `jjyr/canban`. Clicking opens the system browser at `https://github.com/jjyr/canban/issues/new`, with an English report body prefilled with Canban version/build when available, OS/architecture, and editable Expected behavior / Actual behavior / Steps to reproduce sections. The user reviews and submits on GitHub. This action does not create an issue through the integration API, modify a Project source repository, or require a Canban GitHub login. Use URL encoding and bound prefilled data to keep the link usable; do not assume a GitHub Issue Form exists before adding one to the repository.

Herdr reference: `crates/herdr-gpui/src/window/report_issue.rs` captures its current versions/error before asynchronously collecting platform data, then builds a GitHub template URL. Its error snippet is bounded to 1,024 characters; URL behavior has tests in `window/tests/report_issue.rs`. Canban can reuse snapshot-before-async, off-UI-thread platform collection and URL-encoding patterns. Initial Canban prefill should include non-secret version/platform metadata. Detailed diagnostic information is an optional copy action, with contents shown before the user attaches them; do not automatically attach Terminal output, provider credentials or complete logs to a browser URL.

The accepted layout and visibility updates are documented here; implementation and runtime acceptance remain subsequent work.
