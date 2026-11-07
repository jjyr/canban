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
