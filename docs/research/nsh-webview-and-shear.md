# NSH, WebView, and future Shear reuse

Status: source-backed feasibility assessment and proposed UI boundary. No native prototype or performance benchmark was run. The NSH snapshot is a locally available pinned revision, not a claim about latest upstream. See [sources](sources.md).

## What NSH provides

At revision `b7a69186d3c20a59690f2ac834dc527bee7f8438`, NSH supports Rust or TSX applications, native platform backends, native component embedding, and a host communication channel. TSX runs in NSH's runtime; it is not browser DOM code. Its WebView is a distinct browser surface. The macOS backend uses AppKit and WKWebView.

NSH already has Editor, DiffView, and RichEditor facilities. Its documentation describes code-editor IME/input, a rich-document transaction model, and unified/split patch presentation. These are candidate building blocks; they do not prove that Canban's mention workflow or Shear's custom review behavior is already available. [NSH overview](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/README.md), [editor](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/editor.md), [rich editor](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/rich-editor.md), [diff](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/diff.md).

The WebView primitive exposes `src`/`html`, injected JavaScript, `onMessage`, navigation/load events, and a `postMessage` interface. Separately, NSH's host channel provides application host calls/events. Canban must connect these to one service rather than create competing execution controllers in native and web code. [Elements](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/elements.md), [embedding](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/embedding.md).

## Selected UI arrangement: native Rust/NSH

The user subsequently chose Rust/NSH for the main Board, card detail, Timeline, and Notes, with WebView embedded only as needed. This supersedes the earlier recommendation to put the whole work surface in a WebView.

```text
Canban desktop process
  Rust/NSH: Project picker / Board / Card / Timeline / Notes
       |
       | typed asynchronous Rust operation/state boundary
       v
  NSH-independent AppServer
       +-- product state / workflow / Scheduler / timeline
       +-- local IPC entry for CLI; optional later MCP adapter
       +-- storage / directory / core adapter
       |
       v
  herdr-core (separate process)

Optional WebView: a specific resource/specialized component only
```

The pinned NSH [Rust gallery](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/examples/gallery-rs/README.md) and [native entry](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/examples/gallery-rs/native/src/main.rs) demonstrate a Rust-only application path through AppSource::Rust without a TSX/QuickJS requirement. This is a framework capability/source finding, not proof that Canban's complete interaction set already works.

Keep state ownership in AppServer while UI retains navigation/focus/draft presentation. Database/Git/core calls run outside render/input paths, and NSH applies projection updates in its supported execution context. See the [current architecture](../architecture.md) for same-process co-hosting and the future daemon boundary.

Prototype native card drag/autoscroll, keyboard movement, Timeline virtualization, Notes edit/preview and IME, and state updates under real output. A screenshot or the presence of an Editor/RichEditor does not establish those behaviors. Any WebView component also needs its own focus/selection/bridge acceptance.

## Terminal viewer is a real client feature

herdr-core supplies terminal bytes and execution APIs, not a ready-to-embed terminal widget. For interactive parity with Herdr, Canban must implement a terminal emulator integration, resize/input, attach/detach, flow control, clipboard and selection, scrollback, focus, reconnection, and explicit permission for taking control.

A web terminal emulator is a plausible MVP candidate, not a selected dependency. Distinguish a read-only log/`terminal.read` preview from a faithful live terminal. If initial delivery only offers a preview and opens an external terminal for takeover, label it a reduced interaction milestone rather than claiming Herdr UI parity. The [integration guide](herdr-core-integration.md) explains transport obligations.

## What can be copied from Shear

The available checkout at `/Users/jjy/Workspace/shear` is the `jjyr/shear-tool` repository at `e782772145cbeb2fde02657100c0e2373bf0e3ab`, with a clean tracked working tree during inspection. It separates `shear-core` from `shear-app`.

| Shear capability | Current owner | Reuse path for Canban |
| --- | --- | --- |
| Git/local source loading and patch authority | `shear-core/src/workspace.rs`, `source.rs`, `engine.rs` | Extract or vendor Rust code behind a repository/review adapter |
| Immutable change model and semantic projection | `shear-core/src/model.rs`, `syntax.rs`, `projection.rs` | Reuse model/algorithms; expose bounded file and hunk queries |
| Local comments and anchors | `shear-core/src/comments.rs` | Reuse anchoring rules with Canban snapshot identity/storage migration |
| GitHub PR loading and discussion | Core GitHub modules plus app session/UI code | Optional remote-source adapter; unnecessary for a local-first MVP |
| Map, Diff, density rail, navigation, inline interaction | `shear-app/src/ui.rs`, `treemap.rs`, `density.rs`, `comments.rs` | Port presentation behavior to NSH or web; not drop-in code |
| Custom Diff projection and scroll APIs | `vendor/gpui-component` | Reimplement equivalent capabilities or adopt another component after testing |

The core crate's manifest has no GPUI dependency; the app uses GPUI Kit 0.9.0 with GPUI-specific entities and `DiffState`. Shear's local vendored Diff adds shared-scroll, overview-row, visibility, context expansion, and bounded change-navigation APIs. Copying the app source does not convert these entities into AppKit or DOM widgets. [Architecture](https://github.com/jjyr/shear-tool/blob/e782772145cbeb2fde02657100c0e2373bf0e3ab/docs/architecture.md), [core manifest](https://github.com/jjyr/shear-tool/blob/e782772145cbeb2fde02657100c0e2373bf0e3ab/crates/shear-core/Cargo.toml), [UI state](https://github.com/jjyr/shear-tool/blob/e782772145cbeb2fde02657100c0e2373bf0e3ab/crates/shear-app/src/ui.rs), [vendored extension](https://github.com/jjyr/shear-tool/blob/e782772145cbeb2fde02657100c0e2373bf0e3ab/vendor/README.md).

There is a third option: NSH's GPUI backend can host native GPUI components. That introduces a different shell/backend choice and dependency alignment work; its pinned documentation uses `gpui-fast`, whereas current Shear uses `gpui-pre`. It should not be counted as direct reuse within the recommended macOS AppKit shell. An external Shear app can remain a temporary review tool while Canban owns the card and review handoff.

## Reuse the review contract first

```text
Card + accepted intent + ContextSnapshot
                  |
            Run / worktree
                  |
         ChangeArtifact (base, head, files, checks)
                  |
     +------------+------------------+
     |                               |
 external Shear now          future embedded review
                                     |
                          AI explanation + human decision
                                     |
                         verified local merge or optional PR
```

Define a review source by machine, repository, base SHA, head SHA or immutable working-tree snapshot, and artifact ID. A path alone is not a stable review identity. Shear already distinguishes local patch authority from GitHub's canonical PR patch; preserve that separation if PR import is added later.

Shear's `--inspect` prints a loaded `Change` as JSON. It is useful for a bounded model experiment, not evidence of a stable streaming review service. Large sources and refreshes need file/hunk loading APIs rather than repeatedly transferring an entire repository through the UI bridge. [Entrypoint](https://github.com/jjyr/shear-tool/blob/e782772145cbeb2fde02657100c0e2373bf0e3ab/crates/shear-app/src/main.rs), [review workflow](https://github.com/jjyr/shear-tool/blob/e782772145cbeb2fde02657100c0e2373bf0e3ab/docs/review-workflow.md).

## MVP and later work

For MVP, capture artifact references, checks, summary, and a human review handoff inside the card. Opening a basic diff or an external review tool is sufficient for the initial loop. Full semantic Map, density rail, advanced inline review, AI PR explanation, and GitHub write integration are later modules.

When review is added, reuse analysis and anchor behavior before porting the entire workbench. Validate binary/renamed/deleted files, large diffs, stale anchors, changed heads, source context outside patch hunks, and local versus remote comment authority. Review approval must remain bound to the exact artifact being merged.

Before copying code, preserve the relevant license and third-party notices: Shear declares MIT at the workspace level, while its vendored Diff extension is Apache-2.0. This is a source integration task to perform when code is actually imported; no code was copied in this research.
