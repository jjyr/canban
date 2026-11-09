# Desktop utilities and cross-Project Card creation

Implemented and verified on macOS, 2026-11-09. The source comparison remains in [Herdr desktop utilities](research/herdr-desktop-utilities.md); this document describes the shipped implementation and its acceptance limits.

## Layout and interaction

A shared 32-point utility bar sits below the workspace. Its top divider starts **after** the 56-point navigation rail; the rail remains visually continuous to the window bottom. Terminal retains its independently split window chrome.

```text
+------+-------------------------+-------------------------------+
| Nav  | Board / Notes / Overview / Terminal workspace            |
|      |                         |                               |
| Gear +-------------------------+-------------------------------+
|      | Usage: host [Codex] 4% · 7d    [Coffee][Theme][Keys][Bug] |
+------+---------------------------------------------------------+
```

The compact provider summary selects the highest used percentage across that provider's accounts/windows and displays the reset countdown from that **same window**. Days round up (`7d`, `5d`); below one day the display uses hours/minutes, and an elapsed reset shows `due` until refreshed. The tooltip identifies the window and absolute reset time; the panel lists each account/window separately. Unknown reset times are omitted. The countdown updates once per minute; it does not cause a provider HTTP request every tick. Codex window names use the returned duration, because a primary window can be Weekly.

Only signed-in providers appear. With neither Codex nor Claude signed in, the Usage group and its machine label disappear. A transient fetch/access error preserves a known reading marked stale; a confirmed logout removes it. Usage follows the selected Project's machine or active Terminal's machine, with an explicit pin available in the panel. Caffeine always targets the AppServer host, regardless of the Usage selection.

The right-side actions are always reachable. Only one utility dialog is open at once. Escape/outside click dismisses it; Terminal input is gated while it is open and focus returns after dismissal. `⌘/` opens searchable Shortcuts; `⌘N` opens New Card even when the native Terminal owns focus. Terminal `⌘T`, `⌘W` and tab cycling share the command catalog.

New Card initially targets the current Project. Its searchable Project selector changes only the composer destination, preserving title/body/tags. Changing Project clears a selected GitHub Issue, replaces source-specific results and invalidates outstanding searches. A Note's copied text survives a target change; its context link is submitted only within its original Project. Successful creation opens the Card in its owning Project.

Theme offers System/Light/Dark and Canban/Forest/Plum. UI Kit semantic tokens and native Terminal foreground/background/ANSI/cursor colors update together, preserving terminal identity, content and geometry. Appearance and palette are saved in `config/desktop.json`. Caffeine holds a system-idle sleep assertion while allowing display sleep; its dedicated AppServer worker owns the guard, so closing the UI does not release it. It is off after AppServer restart.

Report issue opens an editable `jjyr/canban` GitHub issue draft with English reproduction sections and public version/OS/architecture metadata. It does not submit an issue or require the Canban GitHub connection. Settings also offers Copy diagnostics with the same public metadata. Tokens, Terminal output and logs are not appended to the URL.

## Ownership and API

```text
canban-ui/commands + utilities + Card composer
          | user intents / public readings
          v
       AppServer
          +-- utilities -> atomic desktop preferences
          |             -> dedicated keepawake worker
          +-- canban-usage -> machine-scoped cache/backoff
                                   |
                              existing Host API
                              /              \
                     local Python       Core machine.exec
                              \              /
                              host-side probe
                        existing CLI auth -> quota API
```

`canban-ui-kit` owns reactive theme tokens, utility glyphs and the shared button; it has no provider/domain dependency. `canban-ui` owns routing, panel composition, the command catalog and response-generation fences. `canban-usage` owns provider probing/cache policy and depends on the existing `canban-herdr::host` transport. The standalone Gallery renders the production window frame with deterministic provider fixtures; it does not read credentials or start a Core/AppServer.

| AppServer method | Parameters | Result |
| --- | --- | --- |
| `utilities.state` | none | Preferences, awake state/scope/host and public platform/version |
| `utilities.configure` | `appearance`: system/light/dark; `palette`: canban/forest/plum | Updated state after validated atomic persistence |
| `utilities.awake` | `enabled`: boolean | Updated state after the power worker acknowledges |
| `usage.status` | Optional `machine`, default local | Cached readings, busy/error, last update; starts a due refresh |
| `usage.refresh` | Optional `machine`, default local | Same snapshot; requests a refresh subject to minimum spacing |

These desktop operations reject execution-scoped Agent calls. Usage refreshes normally every ten minutes, backs off authentication/rate failures to thirty minutes and host errors to five minutes, and spaces manual probes by ten seconds. Credential-access failures suppress automatic retries until explicit refresh. Credentials stay on the selected host; AppServer/UI receive public summaries only. Cache state is memory-only.

Codex consumes existing Codex/OpenCode OAuth. Claude consumes its existing credential service/file, respects explicit config directories and custom endpoints, and optionally reads `cswap list --json`. Canban creates no additional Claude sign-in or token refresh store. The provider contracts were adapted from Herdr at `a6db99b4edab37b2038ac1016bd0af82ed0a5ebc`; attribution is retained in the source and third-party notices.

## Acceptance

All application/runtime tests used a disposable home and isolated Core; the user's default data and live Core were not modified.

- `mise run build` and `mise run build-gallery` succeeded. Workspace tests passed (113 Rust tests), four Python probe tests passed, and workspace/all-target Clippy passed with warnings denied.
- Native real Codex read succeeded: Weekly usage with a reset seven days away. The main bar showed `4% · 7d`, Claude was absent without a detected login, and Refresh/outside dismissal worked.
- Native `⌘N` opened New Card with Alpha selected; choosing Beta retained the exact title/body, and creation was verified in Beta through AppServer. `⌘N` and `⌘/` also worked with native Terminal focus; dismissal restored Terminal focus.
- Native Light/Dark and Forest/Plum selection applied. After UI and AppServer restart, Light/Plum preferences were retained. The same Core/terminal binding and `TERMINAL_THEME_OK` output survived; theme changes did not replace the PTY.
- Caffeine on/off produced/removed the actual `Canban background execution` macOS assertion. After closing the UI it remained active in AppServer, then was explicitly released during cleanup.
- Closing the test Terminal tab removed it and Core reported the exact PTY finished with reason `killed`.
- Gallery used the production frame at 720-point width: both provider icons/percentages/countdowns, stale data and all utility actions remained visible. Empty-provider hiding, dark Forest appearance and Escape dismissal were checked.
- Report issue opened a New issue browser tab. URL/template behavior has a test; inspecting the full browser draft was stopped by automatic approval review because the active browser contained unrelated private pages. No issue was submitted.

Live Claude quota and remote-machine quota probing were not accepted in this run; fixtures cover Claude's response/file/cswap contracts. Theme-file import, font customization, native notifications and a full shortcut rebinding editor remain outside this change.
