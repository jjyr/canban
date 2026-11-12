# UI development examples

## Native component gallery

```sh
mise run gallery
```

This builds the `gallery` Cargo example at `examples/gallery/main.rs`, packages it as `.local/CanbanGallery.app`, and opens its own native window. Build without launching with `mise run build-gallery`. The ordinary `mise run build` packages the production UI, CLI and updater, without the Gallery.

Gallery imports UI Kit and the production components from the `canban-ui` library. Its fixtures live in `examples/gallery/fixtures.rs`; it does not call the desktop host, open a Canban home/database, start an AppServer, schedule Agents or connect to Herdr. It needs no configured Agent or CLI beside its executable. The production app has no Gallery route, sidebar entry or `--gallery` mode.

The Scheduling scene renders the production global usage/limit editor, including input validation. Workspace scenes include Card branch/base/path metadata, copy actions and human-gate fixtures.

Current scenes include the production Board/Card workspace, focused Review and retained Card drafts. Notes are deferred. Full/narrow viewport controls exercise the responsive composition. The Controls scene covers semantic typography/surfaces, button variants and disabled state, the Markdown source editor/preview, Timeline and Agent profile registry, Review Diff and a read-only native VT preview. Appearance switches and editable drafts exercise the same production controls. The Terminal scene supplies static `content` to the production renderer, without a live PTY binding.

Bring the Gallery window to the front when checking embedded native views. Background automation captures can defer their presentation; a blank background capture alone is not a successful render check.

For component work:

1. Edit the owning production component in `crates/ui-kit`, `crates/ui` or `crates/terminal`.
2. Add/update a deterministic scene and its meaningful states here, using that component directly.
3. Run `mise run gallery` and check the affected appearance, focus, input, overflow/scroll and resize interactions. Chinese IME checks can be performed when requested.
4. Fix shared defects in their owning component; keep the reproduction scene here.

Use this standalone example as the UI-component check surface. Agent/workflow/storage behavior still belongs to the relevant integration acceptance, rather than simulated business operations in Gallery. Do not copy component implementations into examples or add development-only fixtures back to production screens.

The Cargo target is registered by `crates/ui/Cargo.toml`; direct development builds can also use `mise exec -- cargo run -p canban-ui --example gallery` after `mise run terminal-deps`.

The **Overview** scene uses the production global list/inspector with deterministic fixtures for all six groups, multiple Projects, request-less recovery, Closed/history disclosure and narrow layout. It does not open an AppServer or Core.

The **Quick Open** scene supplies cross-Project duplicate Card names, Unicode titles, Agent lifecycle states and a long candidate list. It uses the production icon rail and search popup; Command+O works in every scene, and the compact Project picker reuses the same popup.

Navigation checks: use the production Board Project-name control to switch Projects or open New Project; use **Long Project** with **Narrow viewport** for heading truncation, and **Empty Board** for first-Project onboarding without task controls or a duplicate creation button. **Controls** includes the shared Lucide icons with selection and attention styling. The Gallery composes the production screen router, including Project forms and Settings; it has no AppServer, so save/execute actions remain previews.

**Settings** renders all production groups inside the same screen scroll. **Narrow viewport** now uses a 720-point workspace. The Controls scene includes the clickable entry card and editable directory/native chooser; cancellation does not modify the field. Overview fixtures show every nonempty status group together, with no filter toolbar and independently expandable Closed history.

**Terminals** renders the production Project-grouped Agent/Terminal inventory and selected native viewer with static VT data; it never attaches the Gallery to a real Core. Fixtures include multiple profiles, queued, closed and archived conversations. Check folder disclosure, Closed history, sidebar resize/visibility, Open Card / Close Card return, and narrow and wide dark-theme layouts. `mise run acceptance-terminals` prepares an isolated real-PTY app for the live navigation/resume/input checks in [Terminal workspace acceptance](../docs/terminal-workspace.md).

To inspect the exact production split chrome without a Core, build the Gallery and run `CANBAN_GALLERY_SCENE=terminals .local/CanbanGallery.app/Contents/MacOS/gallery`. Add `CANBAN_GALLERY_WIDTH=740` for a narrow window; all scenes use dark native controls. This scene reuses `ui/frame.rs`, two-line Agent entries and independently numbered Terminal entries, rename, tab close/reopen and native VT preview. It never opens the user's home or connects to Herdr.

The Terminal fixture includes a long mixed Chinese/English Card title and both Develop/Codex and Review/Claude Profiles. Check stage/Profile readability, provider mark placement, one-line tabs, independent Terminal names without type icons, middle-ellipsized branch text with full hover, custom-name clearing and close/actions reachability at narrow and wide sizes.


Workspace scenes reuse the complete production window frame and utility overlays. **Usage fixtures** supplies Codex/Claude windows with relative reset times (5d/7d) and a stale reading; **No signed-in providers** checks group hiding. At the 720-point viewport, inspect the rail/bar boundary, provider summaries, action reachability, theme selection and popup dismissal. Caffeine and Theme changes in Gallery are fixture-only; real power/credential verification belongs to isolated AppServer acceptance.

**New Project** opens the production directory-first form. Check automatic basename suggestions, retained manual names, collapsed Execution machine, remote summary and directory clearing after a machine change. Use **Narrow viewport** to check path/Choose and footer reachability. Remote directory contents still require an isolated live AppServer; Gallery does not make host calls.

The **Card conversation** scene uses the production compact reader with a rich description, comments, Agent messages and compact chronological runtime events. **Append reply** tests new-activity and bottom-follow behavior; use full/narrow viewport controls while checking header truncation and composer space.

**Inactive Terminals** composes the production list with live idle Agents, normal Shells and attention/closed history. Show/Hide inactive changes only live idle visibility; closed/archived entries never return to the list. Global Back/Forward chrome is the same production component.

**Themes** uses the production bottom-bar picker in every workspace scene. Check Canban as default, the 16 named classic choices, swatch/selected states, keyboard focus, Escape/outside dismissal and unchanged Terminal contents. For hover regression: enter Dracula without clicking (colors change, Canban stays checked), move into a gap (Canban returns), click Dracula (Dracula becomes checked), then preview Nord and dismiss/reopen (Dracula remains checked). Check mouse transitions between adjacent tiles and while scrolling; an old leave must not clear the next tile's preview. There are no Light/Dark or system-mode controls. Use `CANBAN_GALLERY_WIDTH=740 CANBAN_GALLERY_HEIGHT=600` for a narrow, short window and check the last theme remains reachable by scrolling and the bottom Settings button remains visible with both Usage fixtures.

The Overview fixture also includes **Recover Agent that exited before reporting**: the list and inspector show exit code 1, the assignment is Failed under Closed without an automatic task gate, Read saved output replaces live preview, and Open terminal is absent. It deliberately exercises the unavailable-Resume variant; isolated native acceptance covers an actual resumable provider history. From the Card, Needs attention must select this conversation-level row rather than a nonexistent Card-only row.

**Overflowing Board** contains 24 Cards in each of Backlog, Develop and Review. Run `CANBAN_GALLERY_SCENE=board-overflow mise run gallery` to open the production frame directly. Check independent column scrolling, fixed stage headers, rounded muted overlay thumbs without opaque tracks or layout shifts, thumb dragging, idle fade, Card open/close scroll retention, and theme preview/selection. Use `CANBAN_GALLERY_WIDTH=740 CANBAN_GALLERY_HEIGHT=600` for the narrow/short case; horizontal and vertical indicators share the same UI Kit theme. Underfilled columns must not show an indicator.

The Terminal scene also checks **inline session metadata**: Agent rows show title/stage, Provider/Profile/status and machine name; Shell rows show name/status and machine. Select the long mixed-language Agent title to wrap it in full, switch to the differently named Claude reviewer, and inspect the remote custom Shell after scrolling. Check Local/Build machine, Working/Queued/Disconnected, no current directory in list rows, no row/Provider tooltip, and stable selected fill on hover. Top chrome and context menus retain their own detailed information.

**Profile-only Cards** (2026-11-10): use **New Card**, **Card states** and **Card conversation** with the production controls. Creation and Task Edit expose title/body without a Routing tags field; Board tiles have no tag badges. Check full/narrow form spacing, retained text when changing Local/GitHub type, keyboard Create/Save and stage/default Profile tooltip wording. In Task Edit, continuously type into both title and description: inputs must stay mounted/focused and retain the entire text until Save, including after unrelated activity refresh. Actual default/stage/@Profile selection is verified through isolated AppServer/Herdr acceptance, not Gallery fixtures.


**Whole-Card placement** (2026-11-10): use **Overflowing Board** or `CANBAN_GALLERY_SCENE=board-overflow mise run gallery`. Drag from a title/body, insert between peers or into another column, and verify placeholder/peer motion before release. The Grip is decorative. Reorder within a column, cancel outside/Escape, test vertical/horizontal edge scrolling and hover under several named themes. The Gallery installs a local handler for the production `board::Drop` intent; it never starts Agents. `CANBAN_BOARD_TRACE=1` logs gesture/slot transitions, deferred refreshes and Rust hit-test counters without per-frame log traffic; these counters exclude native rendering/input latency.

**Project activity** (2026-11-10): use the named scene or `CANBAN_GALLERY_SCENE=project-activity mise run gallery` for the production Board/frame with twelve other Projects. All are eligible even without Cards/events. Check subtly filled Project buttons, numeric attention badges (including two distinct Cards in Pot), absent zero/working/queued/event summaries, visit/name ordering, direct switching and current-Project exclusion. Wide windows must fit more than three entries; `CANBAN_GALLERY_WIDTH=720 CANBAN_GALLERY_HEIGHT=600` exercises width-driven overflow. Long Unicode names truncate; the existing search list retains complete names, matching badges and duplicate-name directory disambiguation. Escape/outside closes it; Command+O returns to global search. Board-with-Card retains the switcher in wide mode; Overview, Review, Settings and Terminals do not display it. Fixture data remains Gallery-only.

**Notes deferred** (2026-11-10): no Notes workspace, editor, Note-to-Card action, search result or Card context selector is available. Legacy fixture records intentionally remain in some snapshots to verify that they do not create destinations or project activity. Attachments, Card editing and Timeline remain production components.

For Card reading/editing regressions, use **Card conversation** for rich Markdown and a long shared history. **Reply in 5s** adds a real fixture event while you continue typing; check focus, selection, undo and reading position. **Append reply** checks explicit jump-to-latest behavior while reading earlier content. The production composer provides Write/Preview, caret @ completion, native multi-file picking and upload error/retry (Gallery has no AppServer, so saving an attachment intentionally exposes the failure state). Verify persisted submissions/uploads separately in the packaged App with an isolated home/Core.

**Update available** / **Update failed**, or `CANBAN_GALLERY_SCENE=updates mise run gallery`, exercise the production bottom-rail update indicator and progress/retry composition. `CANBAN_GALLERY_SCENE=update-recovery mise run gallery` renders the production standalone Recovery screen without AppServer/SQLite; its Quit action returns to the Gallery shell. These are fixtures, not an installation. Verify actual bundle replacement, session restoration and migration recovery through the isolated packaged-App acceptance described in [updates](../docs/updates.md).

**Bottom-Card lift regression**: `CANBAN_GALLERY_SCENE=board-bottom-drag mise run gallery` opens four uneven-height Cards with empty destination columns. Drag Card #4 a small distance by its body/grip, then reorder, cross columns and cancel outside the Board. The lift must preserve the grab point without jumping upward. Repeat after scrolling to the bottom in `board-overflow`; see [Board regression](../docs/board-interaction.md#bottom-card-lift-regression--2026-11-12).
