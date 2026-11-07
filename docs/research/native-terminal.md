# Native libghostty terminal architecture

Status: source-backed research followed by implementation on 2026-11-07. crates/terminal now hosts pinned libghostty-vt in an NSH/AppKit native view; canban-herdr supplies the persistent framed stream. See implementation.md for accepted behavior and remaining limits.

## Scope and ownership

The concrete initial reuse target is libghostty-vt, as used by the inspected Herdr app. It parses terminal output, maintains terminal state and supplies input encoding. It is not a ready-made NSH widget, does not implement provider conversation resume and must not create a second PTY beside Herdr.

```text
Canban native UI
  Card / Agent list / AgentSessionPanel / native action toolbar
                             |
                    canban-terminal
                 +-----------+-------------------+
                 |                               |
         NSH native macOS host             libghostty-vt
         rendering / focus / IME      screen / modes / input encoding
                 |                               |
                 +------ bytes / resize / focus -+
                             |
                 UI terminal controller
                             |
                      canban-herdr
              persistent framed hc connect
                             |
                   herdr-core / holder / PTY
                             |
                  interactive Codex / Claude
```

Herdr owns program creation, PTY lifetime, terminal records, attach streams and runtime observations. Provider CLI owns model conversation/history. Canban owns the Card/profile conversation association, execution/report/workflow state and launch/resume/normal-exit policy. Terminal rendering belongs to the client, independently of those product objects.

## Minimal boundaries

| Owner | Responsibility |
| --- | --- |
| AppServer | Resolve Card/profile conversation, bind provider ID, open/ask/end operations, durable reports and callbacks |
| Scheduler | Select profile and prepare fresh/resume intent through the same queue; no concurrency caps |
| Small provider adapter modules | Initial/resume argv, reliable provider-ID binding and supported graceful quit behavior; begin with Codex/Claude |
| canban-herdr | Provider-neutral public core protocol, persistent framed client, attach/send/interrupt and runtime events |
| canban-terminal (crates/terminal) | FFI/VT model, NSH native terminal host, rendering and input; no Card, workflow, provider or database dependency |
| canban-ui | Card Agent entries, reusable panel, UI controller connecting a terminal instance to its selected stream |
| UI Kit | Shared buttons, badges, status tokens and focus conventions; no terminal business logic |

Use one terminal crate for the substantial native dependency and renderer, with ordinary ffi/model/input/macos modules. A separate crate for every helper or a new provider-plugin framework is unnecessary. The crate name follows canban-terminal while its directory is terminal. AppServer remains NSH-independent and supports the existing headless host.

## Two paths through the system

```text
Control / lifecycle
  click Agent or @profile
    -> AppServer resolves (Card, profile)
    -> live: focus/attach or send input
    -> exited: provider resume argv -> Herdr terminal.run
    -> first use: provider start argv -> Herdr terminal.run
    -> report -> Workflow Engine -> next stage + request normal exit

Terminal data
  core attach DATA -> libghostty-vt -> native screen drawing
  native keyboard/IME/paste -> VT encoding -> attach DATA -> PTY
  size/focus/display changes -> attach control frames
```

Normal keystrokes do not call a business RPC or create Timeline records. Structured launch/resume/report/interrupt/close actions do. Resolve authority to the selected invocation before attaching; later Card navigation must not redirect an existing terminal's input.

A terminal instance has one lifecycle and one selected stream. Close/detach drops the viewer, not the Agent. Stream loss triggers reattach to the surviving runtime after resynchronization, not a provider resume or duplicate launch. Keep data buffering bounded and return stream credit after terminal-model consumption. Honor negotiated automatic-input handling for terminal-generated replies.

## Source reuse and build boundary

Inspected Herdr app revision: e77f112ac2ea136b84fe510efa42d41163bfd776. It uses:

- vendor/ghostty submodule 291fd050b9e9c293c8adb6a140742649b7e42d3a.
- crates/ghostty-sys: C FFI bindings generated with bindgen; build.rs statically builds libghostty-vt through Zig and states Zig 0.16 is required for that snapshot.
- src/vt.rs: terminal state/input wrapper, with dependencies on Herdr theme/settings, sprites, links and platform key helpers.
- src/terminal.rs: GPUI-specific rendering, focus, IME, selection and scrolling; it cannot be registered unchanged as an NSH AppKit component.
- src/hc.rs: persistent framed transport and per-terminal channel routing, useful as a reference separate from rendering.

NSH at b7a69186d3c20a59690f2ac834dc527bee7f8438 exposes AppView::register_component and a native macOS component interface. Host a terminal NSView through that seam. Its view registration does not automatically supply terminal text shaping, IME candidate positioning or accessibility. Prefer a native terminal view over treating terminal input as an ordinary multiline text field.

Pin compatible source/API/compiler versions when implementing. Check library and third-party licensing, preserve attribution and license files, and keep the project's Apache-2.0 conventions. The research does not select a floating upstream revision or install Zig.

## Acceptance boundary

A successful library build only proves linkage. Accept real interactive Agent TUIs with Unicode/wide characters, alternate screen, cursor updates, selection, scrolling, copy/paste, bracketed paste, Chinese IME composition/candidate positioning, keyboard shortcuts, input focus, resizing and terminal reattachment. Exercise output pressure so it does not starve input or other UI work. Include readable light/dark themes and reuse the exact production component in a native Gallery fixture.

The first slice need not reproduce every Herdr graphic or terminal feature, but cannot be a read-only log presented as a terminal. Document unsupported features rather than assuming the underlying VT library supplies their presentation.

## Primary references

- Herdr [FFI build](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/crates/ghostty-sys/build.rs), [VT wrapper](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/vt.rs), [GPUI terminal view](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/terminal.rs), [framed client](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/hc.rs), and [Ghostty submodule declaration](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/.gitmodules).
- Herdr core [stream protocol](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md) and [public methods](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api.rs).
- NSH [native component integration](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/embedding.md) and [macOS native host](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/crates/nsh-platform-macos/src/native.rs).

## Build and presentation now implemented

The 2026-11-07 dependency update pins upstream Ghostty `5813c9461d5af78b6307a1254d00b1bc190eaccd` (libghostty-vt 1.1.0-dev, upstream main when checked) and Zig 0.17.1. scripts/terminal-deps.py fetches and verifies that immutable Git commit, with a sparse checkout excluding unused GUI assets; build.rs builds libghostty-vt and generates FFI bindings from the same source. This replaces the earlier archive fetch, which repeatedly stalled on large GUI resources. THIRD_PARTY_NOTICES.md and third_party retain upstream licenses. The native component supports text/cursor/styles, scrollback, selection/copy, bracketed paste, key encoding, marked text/IME caret positioning, resize/focus and reattach. This is a Canban AppKit renderer, not Ghostty's complete platform renderer. Mouse-reporting, kitty graphics and link activation remain outside this initial slice. Exhaustive IME and output-pressure acceptance remain pending.

### Upgrade compatibility and verification

The adapter now uses the renamed viewport/key-encoder API, `GhosttyString` for borrowed key text, `GhosttyCodepointBuffer` for grapheme extraction, and the fixed-layout style flags. Key text is cleared after encoding so an event cannot retain a caller's temporary string. A small C header lets Clang resolve the upstream typed style macros for bindgen. Keeping the old raw grapheme buffer call against the new ABI caused a native trap during regression testing; the typed buffer fixes that mismatch. Zig's immutable dependency/compiler cache is shared under `.local/cache/zig`, while per-invocation build/install paths remain separate.

Verification passed: 38 workspace tests, Clippy for all targets with warnings denied, formatting/diff checks, and packaged main/Gallery builds. Model tests cover Unicode/redraw/alternate screen, bracketed paste, cursor visibility, resize, 12,000-line scrollback, Enter/Ctrl-C/application cursor encoding, and bold/underline/inverse/hidden styles. A sparse checkout against a local object source also verified the required CMake template and exclusion of unused font assets.

The upgraded packaged main app was operated with deterministic interactive providers and a real isolated Herdr Core: native input and response rendering, 12,000-line output, upward/downward scrollback, subsequent input, Interrupt, Develop completion into Review, normal exit and native reopening of the exact provider session with subsequent input. Evidence is retained in `/private/tmp/cb-native-oi2tmeww/ghostty-evidence.json`. This is terminal/transport fixture acceptance, not new live Codex/Claude acceptance. Chinese IME was deliberately skipped. The Gallery was rebuilt; its appearance matrix was not re-accepted during this dependency-only update.

A same-instance app restart again produced `cgWindowNotFound` in computer-use automation while the new app process remained running. Fresh-instance acceptance succeeded. The root cause and repeated-restart window visibility remain unresolved; this update does not claim to fix that separate issue.
