# Project entry, grouped Overview and Settings

Implemented 2026-11-08. This refines the [compact navigation](quick-open-and-navigation.md) contract without changing workflow, scheduler admission or Agent lifecycle policy.

## Project entry

Without a valid selected Project, the Board shows only its Project selector and a **Create a Project** entry card. It does not render New Card or workflow columns. The entire entry card is one accessible NSH Button, with shared hover/pressed/focus styling; there is no second creation button below it. Selecting/creating a Project restores the Project/work-location/Add Card header and stage columns. Board has no Execution slots summary or task filter toolbar.

```text
No selected Project                       Selected Project
Board                                     Board
[Choose Project v]                        [Project name v]    [+ New Card]
[ + Create a Project                    ]
                                          Backlog / Develop / Review
```

The Project-name selector still offers existing Projects and New Project in its footer. Clicking the entry opens the same creation form. Cancel returns to Board without creating anything.

### Simplified New Project form (2026-11-09, updated 2026-11-16)

The creation form shows Host (`This Mac` or any connected remote `canband` host, when remote hosts are configured), Directory, Name, optional Workflow / Project Agents disclosures, and Cancel / Create Project. Legacy Core `Execution machine` pickers and `Settings -> Machines` have been removed in favor of unified `Hosts` routing (see [multi-host daemons](multi-host.md)). The page subtitle, Project details section heading and routine field descriptions are removed. Fields retain accessible labels, shared focus/disabled styling and necessary error feedback.

```text
New Project

Host
[This Mac ▾]                                [+ Connect Machine…]

Directory
[/absolute/path/to/canban                          ]
┌──────────────────────────────────────────────────┐
│ 📁 canban                 /absolute/path/to      │
│ 📁 canban-tools           /absolute/path/to      │
└──────────────────────────────────────────────────┘

Name
[canban                                            ]

▸ Workflow · Default
▸ Project Agents · Global defaults

                         [Cancel] [Create Project]
```

The default host is `This Mac` (`local`). Switching hosts clears the previous directory, resets browser results, probes the target host's Agent catalog, and invalidates pending responses; a custom Project name is retained.

Name follows the directory basename, including typed/pasted paths and candidate selection. It keeps following directory changes until the user explicitly edits Name; later directory changes preserve that custom text. Trailing directory separators and bare `~` / `/` roots are handled cleanly. Empty names/directories disable Create Project. New paths are created by the server operation (`mkdir -p`) on save, including `~` / `~/` expansion on the target machine.

Directory uses a single inline-filtering path combobox without an external **Choose…** button or native folder sheet, uniformly across local and remote SSH machines through `directory.list`. Work location settings reuse this exact path combobox while retaining their execution-change explanation.

## Source directory

The shared `ui-kit::path_combobox` (with standalone local helper `ui-kit::directory_field`) provides one editable text input (`Role::Combobox`) paired with an anchored subdirectory filter overlay:

- Focusing or typing splits the query at the last `/` into `(base_dir, segment_filter)` (expanding `~` / `~/` against the host's home directory) and ranks matching subdirectories in real time (`canban_ui_kit::search::score_field`). Dot-prefixed directories stay hidden unless `segment_filter` starts with `.`.
- `↑` / `↓` move the highlighted candidate while keeping keyboard focus and IME composition inside the text input. `Tab` drills into the highlighted directory by appending `/` and loading its subdirectories without closing the dropdown.
- `Enter` or clicking a candidate row finalizes the directory selection (stripping trailing `/`), closes the dropdown, and emits `on_choose`. `Escape` or outside click closes the dropdown while preserving typed text.
- Typing a path that does not yet exist offers a `Use "<path>" · New directory` candidate so new directories can be confirmed and created seamlessly on submit.
- Choosing or confirming a path does not create a Project immediately. The explicit Create Project / Save location action remains the commit point.

The UI Kit owns path query parsing, filtering, keyboard navigation and overlay presentation; AppServer `directory.list` fetching, Project-name policy and `project.create` / `project.update` stay in product UI. The backend expands `~` / `~/`, creates missing directories and canonicalizes the saved path.

## Overview

Overview now renders compact Needs you, Updates, Running and Queued groups, with Idle and Closed collapsed by default. No filter toolbar is required. Rows directly open Card, Review, Terminal or saved history; the previous context inspector has been removed. Selection, scroll, expansion and row keyboard focus survive Back. See [current Overview](product/overview.md) and [2026-11-11 refinement acceptance](overview-terminal-acceptance-20261111.md). Dated acceptance below records the earlier inspector implementation.

## Settings composition

Settings pairs a section jump column (208 points wide on desktop viewports, collapsing into a horizontal top jump strip when `narrow < 950` points without unmounting the content scroll) with one screen-owned vertical scroll (`max_w` 880 points). Clicking a jump entry scrolls directly to that section while pinning its active highlight across smooth-scroll animation and clamped bottom offsets until the user scrolls away; manual scrolling updates the active item via scroll-spy. Shared `settings_section`, `settings_field`, `settings_nav_heading` and `settings_nav_item` components provide consistent group headings, surfaces, borders, spacing and focus/hover states. Numeric fields are bounded to 160 points; descriptions wrap in narrow layouts.

| Nav group | Section | Capabilities exposed |
| --- | --- | --- |
| Interface | Theme and support | Notification sound toggle, theme picker, shortcuts palette, issue reporting and diagnostics copy |
| Interface | Terminal | Installed monospaced font family picker, point size controls (9–32 pt) and reset |
| App | Hosts | `This Mac` and connected remote `canband` hosts (Tailscale/SSH), pairing, reconnect, `Update Daemon`, and removal |
| App | Diagnostics | Per-host connectivity, API compatibility, Herdr Core, toolchains, and live daemon logs |
| App | Updates | Current build version, channel check and install/restart |
| Host | Agent scheduling | Execution summary, global concurrency, idle timeout and Save scheduling settings; validation stays inside the group |
| Host | Terminal Environment | **Inherit User Terminal Environment** toggle (`Inherit` default vs. `Minimal` fallback) plus **Refresh** snapshot control; applies to automated `Agent`/`Script` steps on `local` / `collaborator` trust while `external_pr` and untrusted external cardless events remain scoped to minimal/`PATH`-only env unless explicitly overridden in workflow/step `env:` (interactive Card Terminals and code-controlled AppServer actions remain unrestricted) |
| Host | Agents & Identities | Enabled Herdr CLI Agents, `@identities`, default interactive Agent, Setup wizard and configuration reload |
| Host | Herdr | Runtime executable/session and explicit Check connection |
| Host | Storage | Application-home location and path copy |
| *(Bottom divider)* | Acknowledgements & Licenses | Canban Apache-2.0 attribution, core/runtime credits (`Herdr`, `sheprd` / formerly `herdr-gpui`, `NSH`, `Ghostty`), asset/upstream credits (`CodexBar`, `iTerm2-Color-Schemes`, `Lucide`, `Lobe Icons`, `Primer Octicons`) with license badges and repository links, plus a collapsible bundled-notices and full license viewer |

Save scheduling settings persists both fields; concurrency must be positive, idle timeout nonnegative, and 0 disables idle reclamation.

## Components and Gallery

| Owner | Contract |
| --- | --- |
| `ui-kit/entry_card.rs` | Generic icon, title, supporting text and press intent; one NSH Button |
| `ui-kit/directory_field.rs` | Controlled `path_combobox` / `directory_field`: single editable path input, inline subdirectory filtering, `Tab` drill-down, `Enter`/click selection, `New directory` fallback, and path-selected intent (no external `Choose…` button) |
| `ui-kit/settings.rs` | Section/field visual composition plus `settings_nav_heading` / `settings_nav_item` jump controls; parent owns scrolling and persistence |
| `ui/settings.rs` | Settings middle jump rail, narrow horizontal jump strip, scroll-spy/pin model, and `Acknowledgements & Licenses` section |
| `ui/board.rs`, `ui/screens.rs` | Project availability, form/autoname policy and application operations |
| `ui/overview.rs` | Compact global rows and direct destinations; no filtering |
| `ui/components/scheduling.rs`, `ui/profiles.rs` | Shared settings section and content-only profile projection |

The production Gallery includes Empty Board, Settings, existing all-state Overview fixtures, and shared entry/directory controls. Narrow viewport now constrains the workspace to 720 points, crossing the native narrow-layout breakpoint. These scenarios reuse production components and require no application data or Agent.

## Validation

Native acceptance used the isolated packaged app and configuration at `/private/tmp/canban-settings-1rlm3roy`. The task-owned home/Core endpoint is separate from the user's live Canban/Herdr. No Agent was launched.

Verified through actual native UI interactions:

- Empty Board has just the Project selector and one clickable entry; it has no task filters, summary or New Card.
- Clicking the entry opens the form. Blank fields disable submit. Choose opens the macOS folder sheet; selection fills the directory and blank name, cancellation retains both, and a second folder selection preserves a custom name.
- Creating the Project from the selected folder returns to its Board and restores task controls.
- Invalid scheduling concurrency shows inline validation; saving concurrency 6 and idle timeout 0 updates the live summary and persists both values after leaving/reopening Settings. The single Settings scroll moves scheduling out of view and reaches Workflow, Herdr and Storage.
- Gallery Overview simultaneously exposes Needs you, Ready for review, Working, Queued and Idle; Closed starts collapsed and expands independently. Selecting a permission row opens its correct context; narrow layouts return from the inspector to the complete grouped list.
- Settings group styling, bounded inputs, wrapping descriptions and action reachability were checked in light/dark appearance at 720-point constrained width.

The scratch `native-checks.json` records the checked interactions; `final-snapshot.json` retains one Project and zero Cards, requests or attempts, and `agents.json` retains concurrency 6 / idle timeout 0. The task-owned acceptance app and Gallery were closed after verification.

All 62 Rust tests, all-target Clippy with `-D warnings`, application/Gallery builds, formatting and whitespace checks passed. Chinese IME remains excluded at the user's request. Provider execution, new workflow behavior and macOS notifications are outside this UI refinement's acceptance scope.


## Simplified form acceptance (2026-11-09)

An independent `Canban Auto Acceptance` app used the disposable home `/private/tmp/cb-utilities-zl_w4pdq/home`; the user's running Canban window and default data were left alone. No Agent was launched.

- Native form showed only New Project, Directory/Choose, Name, collapsed Execution machine and Cancel/Create. Typed directory changes updated the generated name, including a trailing slash; explicit name edits survived subsequent path changes.
- Native Choose opened the macOS folder sheet. Cancel preserved path/name. Selecting `directory-picked` generated that name; Create opened its empty Board. AppServer snapshot confirmed the saved name, directory and local machine.
- A second launch with an explicit isolated AppServer endpoint used the same Choose action to open the server browser. Navigating into `directory-picked` and selecting it updated both path and generated name. This exercises the server browser path, not live SSH-machine acceptance.
- Gallery's 720-point New Project scene retained a manual name while switching to Build machine, cleared the old directory, disabled creation until a new path was entered and kept Build machine visible after collapse. No duplicate Browse action remained.
- Application/Gallery builds, 113 workspace Rust tests, UI/all-target Clippy with warnings denied, and whitespace checks passed.
