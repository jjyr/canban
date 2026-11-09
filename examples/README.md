# UI development examples

## Native component gallery

```sh
mise run gallery
```

This builds the `gallery` Cargo example at `examples/gallery/main.rs`, packages it as `.local/CanbanGallery.app`, and opens its own native window. Build without launching with `mise run build-gallery`. The ordinary `mise run build` packages only the production application and CLI.

Gallery imports UI Kit and the production components from the `canban-ui` library. Its fixtures live in `examples/gallery/fixtures.rs`; it does not call the desktop host, open a Canban home/database, start an AppServer, schedule Agents or connect to Herdr. It needs no configured Agent or CLI beside its executable. The production app has no Gallery route, sidebar entry or `--gallery` mode.

The Scheduling scene renders the production global usage/limit editor, including input validation. Workspace scenes include Card branch/base/path metadata, copy actions and human-gate fixtures.

Current scenes include the production Board/Card workspace, focused Review and Notes editor/related pane with links/backlinks and retained drafts. Full/narrow viewport controls exercise the responsive composition. The Controls scene covers semantic typography/surfaces, button variants and disabled state, controlled multiline input/Markdown preview, Timeline, Agent mention picker/registry, Review Diff and a read-only native VT preview. Appearance switches and editable drafts exercise the same production controls. The Terminal scene supplies static `content` to the production renderer, without a live PTY binding.

Bring the Gallery window to the front when checking embedded native views. Background automation captures can defer their presentation; a blank background capture alone is not a successful render check.

For component work:

1. Edit the owning production component in `crates/ui-kit`, `crates/ui` or `crates/terminal`.
2. Add/update a deterministic scene and its meaningful states here, using that component directly.
3. Run `mise run gallery` and check the affected appearance, focus, input, overflow/scroll and resize interactions. Chinese IME checks can be performed when requested.
4. Fix shared defects in their owning component; keep the reproduction scene here.

Use this standalone example as the UI-component check surface. Agent/workflow/storage behavior still belongs to the relevant integration acceptance, rather than simulated business operations in Gallery. Do not copy component implementations into examples or add development-only fixtures back to production screens.

The Cargo target is registered by `crates/ui/Cargo.toml`; direct development builds can also use `mise exec -- cargo run -p canban-ui --example gallery` after `mise run terminal-deps`.

The **Overview** scene uses the production global list/inspector with deterministic fixtures for all six groups, multiple Projects, request-less recovery, Closed/history disclosure and narrow layout. It does not open an AppServer or Core.

The **Quick Open** scene supplies cross-Project duplicate Card/Note names, Unicode titles, Agent lifecycle states and a long candidate list. It uses the production icon rail and search popup; Command+O works in every scene, and the compact Project picker reuses the same popup.

Navigation checks: use the production Board Project-name control to switch Projects or open New Project; use **Long Project** with **Narrow viewport** for heading truncation, and **Empty Board** for first-Project onboarding without task controls or a duplicate creation button. **Controls** includes the shared Lucide icons with selection and attention styling. The Gallery composes the production screen router, including Project forms and Settings; it has no AppServer, so save/execute actions remain previews.

**Settings** renders all production groups inside the same screen scroll. **Narrow viewport** now uses a 720-point workspace. The Controls scene includes the clickable entry card and editable directory/native chooser; cancellation does not modify the field. Overview fixtures show every nonempty status group together, with no filter toolbar and independently expandable Closed history.

**Terminals** renders the production Project-grouped Agent/Terminal inventory and selected native viewer with static VT data; it never attaches the Gallery to a real Core. Fixtures include multiple profiles, queued, closed and archived conversations. Check folder disclosure, Closed history, sidebar resize/visibility, Open Card / Close Card return, and narrow/light/dark layouts. `mise run acceptance-terminals` prepares an isolated real-PTY app for the live navigation/resume/input checks in [Terminal workspace acceptance](../docs/terminal-workspace.md).

To inspect the exact production split chrome without a Core, build the Gallery and run `CANBAN_GALLERY_SCENE=terminals .local/CanbanGallery.app/Contents/MacOS/gallery`. Add `CANBAN_GALLERY_WIDTH=740` for a narrow window and `CANBAN_GALLERY_APPEARANCE=dark` (or `light`) for deterministic appearance. This scene reuses `ui/frame.rs`, two-line Agent entries and independently numbered Terminal entries, rename, tab close/reopen and native VT preview. It never opens the user's home or connects to Herdr.

The Terminal fixture includes a long mixed Chinese/English Card title and both Develop/Codex and Review/Claude Profiles. Check stage/Profile readability, provider mark placement, one-line tabs, independent Terminal names without type icons, middle-ellipsized branch text with full hover, custom-name clearing and close/actions reachability at narrow and wide sizes.


Workspace scenes reuse the complete production window frame and utility overlays. **Usage fixtures** supplies Codex/Claude windows with relative reset times (5d/7d) and a stale reading; **No signed-in providers** checks group hiding. At the 720-point viewport, inspect the rail/bar boundary, provider summaries, action reachability, theme selection and popup dismissal. Caffeine and Theme changes in Gallery are fixture-only; real power/credential verification belongs to isolated AppServer acceptance.

**New Project** opens the production directory-first form. Check automatic basename suggestions, retained manual names, collapsed Execution machine, remote summary and directory clearing after a machine change. Use **Narrow viewport** to check path/Choose and footer reachability. Remote directory contents still require an isolated live AppServer; Gallery does not make host calls.

The **Card conversation** scene uses the production compact reader with a long task, comments, Agent messages and collapsed runtime events. **Append reply** tests new-activity and bottom-follow behavior; use full/narrow viewport controls while checking header truncation and composer space.
