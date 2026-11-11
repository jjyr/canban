# Desktop utilities and cross-Project Card creation

Implemented and verified on macOS, 2026-11-09. The source comparison remains in [Herdr desktop utilities](research/herdr-desktop-utilities.md); this document describes the shipped implementation and its acceptance limits.

## Layout and interaction

A shared 32-point utility bar sits below the workspace. Its top divider starts **after** the 56-point navigation rail; the rail remains visually continuous to the window bottom. Terminal retains its independently split window chrome.

```text
+------+-----------------------------------------------------------+
| Nav  | Board / Overview / Terminal workspace                      |
|      |                                                           |
|      +-----------------------------------------------------------+
|      | [Codex] 4% · 7d    [Coffee][Theme][Keys][Bug][Settings]     |
+------+-----------------------------------------------------------+
```

The compact provider summary selects the highest used percentage across that provider's accounts/windows and displays the reset countdown from that **same window**. Days round up (`7d`, `5d`); below one day the display uses hours/minutes, and an elapsed reset shows `due` until refreshed. The tooltip identifies the window and absolute reset time; the panel lists each account/window separately. Unknown reset times are omitted. The countdown updates once per minute; it does not cause a provider HTTP request every tick. Codex window names use the returned duration, because a primary window can be Weekly.

Only signed-in providers appear. With neither Codex nor Claude signed in, the Usage group disappears. A transient fetch/access error preserves a known reading marked stale; a confirmed logout removes it. Usage follows the selected Project's machine or active Terminal's machine, with an explicit pin available in the panel. Caffeine always targets the AppServer host, regardless of the Usage selection.

The right-side actions are always reachable. Only one utility dialog is open at once. Escape/outside click dismisses it; Terminal input is gated while it is open and focus returns after dismissal. `⌘/` opens searchable Shortcuts; `⌘N` opens New Card even when the native Terminal owns focus. Terminal `⌘T`, `⌘W` and tab cycling share the command catalog.

New Card uses a **Create** button for both Local and GitHub Issue Cards. `⌘Enter` submits through the same form intent and validation as the button; its hint appears in the button tooltip/accessibility label and Shortcuts catalog. This binding is active only on New Card without another popup. Empty titles, missing Issue selection/Project and pending submissions do not create anything. Normal Enter still belongs to the editor, and successful creation still enters Backlog.

Card comments use **Send** and focused-editor `⌘Enter`, while Enter inserts a newline. The same command catalog lists this contextual action; empty/pending/archived submissions and active utility/search popups are guarded. See [Card conversation](card-conversation.md).

New Card initially targets the current Project. Its searchable Project selector changes only the composer destination, preserving title/body. Changing Project clears a selected GitHub Issue, replaces source-specific results and invalidates outstanding searches. Successful creation opens the Card in its owning Project. Note-to-Card creation is removed while [Notes are deferred](notes-deferred.md).

Canban always uses dark native controls and defaults to **Canban**. There is no System/Light/Dark mode or grouping. The named picker offers Canban plus 16 classic Terminal palettes: Dracula, Nord, Gruvbox, Tokyo Night, Catppuccin Mocha, One Dark, Monokai, Solarized, Tomorrow Night, Oceanic Next, Material, Ayu, Everforest, Rosé Pine, Kanagawa and Ayu Mirage. Each choice has source-color swatches, a saved-selection check and native button focus. Hover temporarily previews the theme across App chrome and Terminal; moving off the choice restores the saved theme. Click (or keyboard activation) confirms and persists the choice. Escape, outside click, Close, popup switching and navigation discard any unconfirmed preview. The check continues to identify the saved theme during preview. The picker scrolls within the window and dismisses with Escape or an outside click.

App chrome derives semantic colors from the selection; the Terminal keeps its exact source foreground/background/cursor and ANSI 16 values. Previewing or confirming a theme updates both without recreating the PTY or clearing content. Preview state belongs only to the window presenter; it is never sent to AppServer or written to disk. Background preference refreshes retain the active preview, and a failed confirmation restores the saved theme. Preferences are saved as `{"theme":"dracula"}` in `config/desktop.json`. The previous appearance/palette schema is read safely: the retired Forest/Plum palettes and unknown names fall back to Canban, and appearance is ignored. Loading does not rewrite the old file; the next explicit selection saves the new schema atomically. AppServer rejects invalid choices before modifying the file. Desktop startup requires the current [API level](development-startup.md#api-compatibility), including the named-theme contract introduced at level 2.

The desktop preference also supports optional `last_project`, a stable Project ID used to restore the startup Board. `utilities.configure` accepts `theme`, `last_project`, or both; omitted fields retain their saved value and explicit null clears the Project preference. Validate theme/Project values before writing, and skip unchanged writes. The normal UI saves selections asynchronously in order. See [startup restoration](quick-open-and-navigation.md#board-title-bar-project-activity-2026-11-10). This requires API level 5; old theme-only configs remain readable.

Settings now occupies the last bottom utility button, beside the other global utilities; it is removed from the navigation rail. The rail/bar divider remains after the 56-point rail.

Caffeine holds a system-idle sleep assertion while allowing display sleep; its dedicated AppServer worker owns the guard, so closing the UI does not release it. It is off after AppServer restart.

The later real-use failure was an older persistent AppServer returning `Unknown operation: utilities.state` to a new UI. Desktop startup checks the API level before opening the workspace and now shows startup failures in a native window. `mise run dev` (also `mise run run`) replaces only its selected isolated development instance; it does not repair an older installed-home service. See [development startup and upgrade acceptance](development-startup.md). Earlier fresh-service tests did not cover this upgrade scenario.

Report issue opens the repository's **Bug report** Issue Form directly, with public version/OS/architecture metadata prefilled in its editable Environment field. It does not submit an issue or require the Canban GitHub connection. Settings also offers Copy diagnostics with the same public metadata. Tokens, Terminal output and logs are not appended to the URL.

## Repository Issue Forms

The App and GitHub entry points share the same form instead of maintaining separate Markdown bodies:

```text
App Report issue ----> Bug report + prefilled Environment
                              |
GitHub New issue -----+-------+----> user completes form --> Submit
                      |
                      +-- Feature request ----------------> Submit
```

`.github/ISSUE_TEMPLATE/bug_report.yml` requires **What happened?** and **Steps or triggering context**. Expected behavior, Environment and screenshots/additional context are optional. Intermittent problems can describe their triggering context rather than requiring reliable reproduction. GitHub supplies the normal Issue title field.

`.github/ISSUE_TEMPLATE/feature_request.yml` requires the problem the feature would solve, with optional desired behavior and additional context. Neither form inserts a title prefix, assignee or submission checklist. GitHub's blank-Issue option remains available.

The App URL uses `template=bug_report.yml&environment=<URL-encoded diagnostics>`. `environment` is the Bug form field ID; preserve this ID and filename when editing the form, or update `ui/utilities/report.rs` in the same change. Other form contents remain owned by the repository template. This follows GitHub's documented [Issue URL query parameters](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue#creating-an-issue-from-a-url-query) and [field ID prefill contract](https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-githubs-form-schema#keys).

Both form schemas and required fields were checked locally, including the URL-to-template/field mapping. The report-link regression test, application build and UI/all-target Clippy passed. The form files take effect on GitHub after they reach the default branch.

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

`canban-themes` owns the pure built-in catalog shared by AppServer validation and UI Kit, with no NSH or product dependencies. Pinned palette sources and licenses are listed in [the catalog](../crates/themes/README.md). `canban-ui-kit` owns reactive dark theme tokens, the reusable swatch choice, utility glyphs and the shared button; it has no provider/domain dependency. `canban-ui` owns routing, panel composition, the command catalog and response-generation fences. `canban-usage` owns provider probing/cache policy and depends on the existing `canban-herdr::host` transport. The standalone Gallery renders the production window frame with deterministic provider fixtures; it does not read credentials or start a Core/AppServer.

| AppServer method | Parameters | Result |
| --- | --- | --- |
| `utilities.state` | none | Preferences, awake state/scope/host and public platform/version |
| `utilities.configure` | `theme`: a built-in theme ID, e.g. `canban`, `dracula`, `tokyo-night` | Updated state after validated atomic persistence |
| `utilities.awake` | `enabled`: boolean | Updated state after the power worker acknowledges |
| `usage.status` | Optional `machine`, default local | Cached readings, busy/error, last update; starts a due refresh |
| `usage.refresh` | Optional `machine`, default local | Same snapshot; requests a refresh subject to minimum spacing |

These desktop operations reject execution-scoped Agent calls. Usage refreshes normally every ten minutes, backs off authentication/rate failures to thirty minutes and host errors to five minutes, and spaces manual probes by ten seconds. Credential-access failures suppress automatic retries until explicit refresh. Credentials stay on the selected host; AppServer/UI receive public summaries only. Cache state is memory-only.

Codex consumes existing Codex/OpenCode OAuth. Claude consumes its existing credential service/file, respects explicit config directories and custom endpoints, and optionally reads `cswap list --json`. Canban creates no additional Claude sign-in or token refresh store. The provider contracts were adapted from Herdr at `a6db99b4edab37b2038ac1016bd0af82ed0a5ebc`; attribution is retained in the source and third-party notices.

## Earlier acceptance (before the named-theme update)

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

## Minimal Usage summary (2026-11-09)

The bottom bar shows provider icons, percentages and reset countdowns directly. It no longer prefixes them with **Usage: AppServer host** or a machine name. Machine/account selection remains in the Usage details popup. Signed-out provider hiding and the existing usage/reset behavior are preserved.

## Named-theme acceptance (2026-11-09)

- `mise run test`: 130 Rust tests passed, including unique/dark catalog and semantic contrast checks, legacy preference loading, invalid-save preservation and saved-theme restart. Workspace/all-target Clippy passed with warnings denied; production and standalone Gallery builds succeeded.
- `mise run acceptance` and `mise run acceptance-dev` passed using isolated homes/Core. The development restart test checks the named-theme schema, API level 2, older-service replacement and an unchanged live PTY.
- Native Gallery: default Canban, Dracula selection, source swatches, selected checks, Escape dismissal, both provider usage summaries at the constrained 720-point viewport and the bottom Settings route were checked. The navigation rail has no Settings duplicate. A real 740 × 600 Gallery window exposed and then verified the fix for overflowing popup contents: the header stays fixed, the list clips/scrolls, Ayu Mirage can be reached and selected, and an outside click dismisses the popup without activating the underlying navigation.
- Isolated production app: switching a live fixture Agent Terminal from Canban to Dracula preserved `THEME_BEFORE_SWITCH` and accepted `THEME_AFTER_SWITCH`. After UI restart, Dracula remained selected; the same Terminal/Core IDs and output survived, and `THEME_AFTER_RESTART` was accepted. The bottom Settings button opened the Settings page and showed its selected state.
- These theme checks use a deterministic interactive fixture over real Herdr, not a paid provider. The initial CUA connection to the new test bundle stalled before recovering; that automation delay is separate from the accepted in-App operations. Chinese IME and system appearance changes were not exercised.

## Hover preview acceptance (2026-11-09)

- The production Gallery picker was checked at 740 × 600: pointer entry temporarily applied Dracula while Canban remained checked; pointer exit restored Canban without confirming a choice.
- A disposable production App/Core with a real Shell Terminal retained `HOVER_TERMINAL_OK` through Dracula preview. App chrome, Terminal background and ANSI colors changed together. Across the normal background refresh interval, the preference file retained its exact bytes and modification time, and AppServer continued to report Canban.
- Escape restored Canban and Terminal input accepted `PREVIEW_CANCELLED_OK`. Clicking Dracula changed the saved preference and check. A subsequent Nord preview followed by an outside click left Dracula saved. After UI restart, Dracula remained selected and the same Terminal/Core binding was retained.
- Workspace tests (130 Rust tests), strict workspace/all-target Clippy and production/Gallery builds passed. No paid Agent was launched and the user's normal home/Core was not touched.
