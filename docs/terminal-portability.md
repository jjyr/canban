# Cross-platform Terminal architecture (macOS, Linux, iOS, Android)

Status: Phase 0, Phase 1, and Phase 2 implemented 2026-11-23; Android NDK cross-compile and Phase 3/4 pending.

## Goal

Share the `libghostty-vt` screen model, PTY session pool, view timing state, transport layer, and input/selection logic across macOS, Linux, iOS, and Android while preserving macOS's tuned native renderer.

## Current `crates/terminal` layout

| Module / file | Responsibility | Cross-platform status |
| --- | --- | --- |
| `src/model.rs` | `libghostty-vt` screen, cell, cursor, image, link, and input adapter | ✅ Shared across all targets |
| `src/session.rs` | Transport-agnostic `PtyTransport` / `LiveSession` attachment pool (`HerdrLocalTransport`, `WakeHub`) | ✅ Shared (`herdr-local` Cargo feature isolates local Herdr) |
| `src/input.rs` | Platform-neutral input, paste/path escape, theme, resize, selection, and link-matching helpers | ✅ Shared across Apple and portable viewers |
| `src/remote.rs` | `RemotePtyTransport` speaking Generation-1 binary framing over `canban-transport` (`herdr.connect`) | ✅ Shared across all targets (no `canban-herdr` / `canban-exec`) |
| `src/view_state.rs` | Cursor blink, IME commit cursor hold, scrollbar fade, link hover spans, dirty-row plan | ✅ Shared across Apple and portable viewers |
| `src/portable.rs` | `GhosttyEmulator` adapting `model`/`session` to `nsh_base::Terminal` | ✅ Shared across Linux, iOS, Android |
| `src/view.rs` | Unified `TerminalView` UI entry point (`native("canban/terminal")` on macOS, `portable` elsewhere) | ✅ Shared entry point for `canban-ui` & Gallery |
| `src/apple/apple_render.h` / `.m` | CoreGraphics + CoreText cell, underline, box-drawing, and image renderer | 🟡 Apple-shared (header explicitly forbids AppKit and UIKit) |
| `src/apple/macos.rs` / `src/apple/macos_view.m` | macOS `NSView` host, `NSTextInputClient` IME, mouse/key translation, FFI callbacks | ❌ macOS-only (`#[cfg(target_os = "macos")]`, `nsh-platform-macos`) |

## Approved architecture: "A + B, iOS first via B"

```text
                      canban-terminal (shared core)
   model (libghostty-vt) · session (PtyTransport pool) · view_state
   portable input/key mapping · selection & link logic · mobile key-bar synthesis
   Transports: HerdrLocalTransport (desktop local) | RemoteTcpTransport (herdr.connect)
                                │
         ┌──────────────────────┴──────────────────────┐
   Renderer A: Apple native                     Renderer B: Portable
   apple_render (CoreGraphics + CoreText)       nsh_base::Terminal + GhosttyEmulator
    ├ macOS: NSView host (retained)              ├ Linux (existing)
    └ iOS:   UIView host (Phase 4)               ├ Android
                                                 └ iOS initial release (Phase 2/3)
```

### Design tradeoffs and code sharing

- **Why keep Renderer A on macOS:** Retaining `NSView` + `apple_render` preserves Canban's verified macOS CJK `NSTextInputClient` IME behavior, Kitty graphics placement, and synchronized-output frame stability ([NSH integration notes §3](nsh-notes.md#3-native-terminal-ime-buffers-and-synchronized-output-060--5e3b0f78)).
- **Why start iOS on Renderer B:** `nsh_base::Terminal` already runs on `nsh-platform-ios` and `nsh-platform-android` (lines of text rendered via the platform text engine), letting iOS and Android share one portable rendering integration (`portable.rs`) first, then optionally upgrade iOS to a thin `UIView` shell over `apple_render` without touching `session`, transports, or UI callers.
- **Estimated reuse:** ~5,700 lines of core (`model` + `session` + `view_state` + portable input/selection), plus ~1,400 lines of `portable.rs` for Linux/Android/initial iOS, and ~2,150 lines of `apple_render` shared across macOS/iOS — roughly **65–70% shared code**.

## Implementation phases

### Phase 0 — Cross-compile spike (`libghostty-vt` for iOS and Android)

- **iOS (implemented 2026-11-22):**
  - Extended `zig_target` in [crates/terminal/build.rs](../crates/terminal/build.rs#L10-L22) with `aarch64-apple-ios` -> `aarch64-ios` and `aarch64-apple-ios-sim` -> `aarch64-ios-simulator`.
  - Generalized `macos_sdk` into `apple_sdk` (selecting `iphoneos` or `iphonesimulator` via `xcrun --sdk`, tracking `SDKROOT` env changes), updating `cache_key`, and passed `--target=arm64-apple-ios[-simulator]` + `-isysroot{sdk}` to bindgen on iOS. No extra `cargo:rustc-link-lib` entries were needed on iOS beyond Apple system frameworks (`CoreFoundation`, `libiconv.2`, `libSystem.B`).
  - Added pure-Rust smoke example [vt_smoke.rs](../crates/terminal/examples/vt_smoke.rs) (`Model` parses VT + Unicode without `herdr-local` or macOS UI dependencies) and the `mise run check-ios` task in [mise.toml](../mise.toml#L27-L35) (`IPHONEOS_DEPLOYMENT_TARGET = "14.0"`, building `canban-terminal --no-default-features --locked --example vt_smoke` for `aarch64-apple-ios-sim` and `aarch64-apple-ios`).
  - **Verification:** `cargo run -p canban-terminal --example vt_smoke` printed `vt_smoke ok: hello ios 世界` on host; `mise run check-ios` succeeded for both iOS targets; `otool -l` reported `LC_BUILD_VERSION platform 7` (`minos 14.0`, `sdk 27.0`) for `aarch64-apple-ios-sim` and `LC_BUILD_VERSION platform 2` (`minos 14.0`, `sdk 27.0`) for `aarch64-apple-ios`; `nm -gU` confirmed statically linked `_ghostty_*` symbols in both `libghostty-vt.a` archives and final Mach-O binaries.
  - **Simulator runtime verification (2026-11-23):** Booted `iPhone 18 Pro` (`iOS 27.0 / 24A434`, UDID `80199FEC-F004-46AE-A1FD-68DDA238D6B5`), ran `xcrun simctl spawn` on `target/aarch64-apple-ios-sim/debug/examples/vt_smoke` (exit 0 without ad-hoc re-signing), observed `vt_smoke ok: hello ios 世界`, and shut down the simulator.
  - **Mobile dependency chain iOS probe (2026-11-23):** `cargo check --locked` with `IPHONEOS_DEPLOYMENT_TARGET=14.0` succeeded (`rc=0`) on both `aarch64-apple-ios-sim` and `aarch64-apple-ios` for `canban-sync`, `canban-transport`, and `canban-ui --no-default-features` (later cleaned to 0 warnings in Phase 1).
- **Android (deferred):** `aarch64-linux-android` cross-compilation (plus NDK sysroot/API-level/libc++ linkage verification) was deferred because no Android NDK is installed on this host (`ANDROID_NDK_HOME` is unset and `~/Library/Android/sdk/ndk` does not exist).

### Phase 1 — Module reorganization and unified UI entry (implemented 2026-11-23)

- Renamed `src/linux.rs` -> `src/portable.rs` and moved macOS-specific files under `src/apple/` (`apple/macos.rs`, `apple/macos_view.m`, `apple/apple_render.h`, `apple/apple_render.m`), keeping `pub use apple::macos as native;` gated by `#[cfg(target_os = "macos")]`.
- Extracted platform-neutral input, paste/path escape, theme, resize, selection, and link-matching helpers from macOS FFI callbacks and `portable.rs` into [src/input.rs](../crates/terminal/src/input.rs).
- Added [src/view.rs](../crates/terminal/src/view.rs) (`TerminalView`, `LiveBinding`, `FocusRequest`, `FontSpec`), extending `LiveBinding` with `transport: Option<Arc<dyn PtyTransport>>` and replacing the `#[cfg(target_os = "macos")]` branches in [crates/ui/src/card/terminal.rs](../crates/ui/src/card/terminal.rs), [crates/ui/src/terminals/viewer.rs](../crates/ui/src/terminals/viewer.rs), and [examples/gallery/main.rs](../examples/gallery/main.rs), plus gating desktop-only `WindowLaunch`/`WindowHost` in [crates/ui/src/platform.rs](../crates/ui/src/platform.rs).
- Desktop runtime behavior remains unchanged (`native("canban/terminal")` preserved on macOS), and `canban-ui --no-default-features` compiles with zero warnings on both iOS targets.

### Phase 2 — Remote `PtyTransport` (`herdr.connect` over `canban-transport`, implemented 2026-11-23)

- Implemented `RemotePtyTransport` in [src/remote.rs](../crates/terminal/src/remote.rs) on `canban-transport` (`ClientTransport::open_async_stream`, `yamux/1` multiplexing, and direct TCP per-device bearer auth) + Tokio async reader/writer tasks speaking Generation-1 Herdr binary framing (`herdr.connect` -> `hello` -> `terminal.attach` -> 9-byte frames for `Data`/`Resize`/`Closed`, input, `automatic_input`, `consumed`, `focus`, `facts`, and clean `Detach` on drop).
- Mobile targets compile with `--no-default-features` (omitting `herdr-local` so `canban-herdr` and `canban-exec` are not linked), with `UiBackend::terminal_transport` providing the transport injection hook for pure-remote clients.

### Phase 3 — Mobile interaction components

- Shared **accessory key bar** above the software keyboard (`Esc`, `Tab`, `Ctrl` sticky toggle, `↑`, `↓`, `←`, `→`, `^C`, `⤢` landscape fullscreen) emitting standard `KeyInput` / VT bytes into `LiveSession`.
- Pinch-to-zoom font size scaling and touch long-press text selection on `portable.rs`.

### Phase 4 — Optional iOS `UIView` host over `apple_render`

- Build a thin `UIView` + `UITextInput` shell backed by the existing AppKit/UIKit-free `apple_render.m` renderer when iOS requires Kitty graphics or higher-density cell throughput than `nsh_base::Terminal`.
