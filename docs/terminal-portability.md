# Cross-platform Terminal architecture (macOS, Linux, iOS, Android)

Status: Phase 0 (iOS) implemented 2026-11-22; Android pending; later phases proposed.

## Goal

Share the `libghostty-vt` screen model, PTY session pool, view timing state, transport layer, and input/selection logic across macOS, Linux, iOS, and Android while preserving macOS's tuned native renderer.

## Current `crates/terminal` layout

| Module / file | Lines | Responsibility | Cross-platform status |
| --- | --- | --- | --- |
| `src/model.rs` | 3,815 | `libghostty-vt` screen, cell, cursor, image, link, and input adapter | ✅ Shared across all targets |
| `src/session.rs` | 1,027 | Transport-agnostic `PtyTransport` / `LiveSession` attachment pool (`HerdrLocalTransport`, `WakeHub`) | ✅ Shared (`herdr-local` Cargo feature isolates local Herdr) |
| `src/view_state.rs` | 642 | Cursor blink, IME commit cursor hold, scrollbar fade, link hover spans, dirty-row plan | ✅ Shared across Apple and Linux |
| `src/linux.rs` | 1,403 | `GhosttyEmulator` adapting `model`/`session` to `nsh_base::Terminal` | ✅ Platform-neutral in practice (depends only on `nsh_base::term`, `nsh_core`, core `terminal`) |
| `src/apple_render.h` / `.m` | 331 + 1,825 | CoreGraphics + CoreText cell, underline, box-drawing, and image renderer | 🟡 Apple-shared (header explicitly forbids AppKit and UIKit) |
| `src/native.rs` / `src/macos_view.m` | 1,978 + 2,868 | macOS `NSView` host, `NSTextInputClient` IME, mouse/key translation, selection, FFI callbacks | ❌ macOS-only (`nsh-platform-macos`); contains some portable selection/key/link helpers |

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
  - **Mobile dependency chain iOS probe (2026-11-23):** `cargo check --locked` with `IPHONEOS_DEPLOYMENT_TARGET=14.0` succeeded (`rc=0`) on both `aarch64-apple-ios-sim` and `aarch64-apple-ios` for `canban-sync`, `canban-transport`, and `canban-ui --no-default-features` (6 non-fatal unused/dead_code warnings in `canban-ui` under non-macOS targets, including `crates/ui/src/terminals/viewer.rs` and `crates/ui/src/platform.rs`, to be cleaned up during Phase 1).
- **Android (deferred):** `aarch64-linux-android` cross-compilation (plus NDK sysroot/API-level/libc++ linkage verification) was deferred because no Android NDK is installed on this host (`ANDROID_NDK_HOME` is unset and `~/Library/Android/sdk/ndk` does not exist).

### Phase 1 — Module reorganization and unified UI entry

- Rename `src/linux.rs` -> `src/portable.rs`.
- Move `src/native.rs` and `src/macos_view.m` under `apple/macos` (`#[cfg(target_os = "macos")]`).
- Extract platform-neutral selection, link-matching, and key/mouse encoding helpers from `native.rs` / `macos_view.m` into the shared Rust core.
- Replace the `#[cfg(not(target_os = "macos"))]` platform branches in [crates/ui/src/card/terminal.rs](../crates/ui/src/card/terminal.rs#L438-L480) and [crates/ui/src/terminals/viewer.rs](../crates/ui/src/terminals/viewer.rs#L240-L280) with one unified entry point (e.g., `canban_terminal::view(...)`). Desktop runtime behavior remains unchanged.

### Phase 2 — Remote `PtyTransport` (`herdr.connect` over `canban-transport`)

- Implement a `PtyTransport` reusing [crates/transport](../crates/transport/Cargo.toml) (`ClientTransport` over `yamux/1` multiplexing, per-device bearer tokens, and Tokio async streams) for remote `canband` `herdr.connect` PTY sessions, shared by both desktop multi-host viewers and mobile clients instead of maintaining a separate custom TCP relay.
- Mobile targets compile with `default-features = false` (omitting `herdr-local` so `canban-herdr` and `canban-exec` are not linked).

### Phase 3 — Mobile interaction components

- Shared **accessory key bar** above the software keyboard (`Esc`, `Tab`, `Ctrl` sticky toggle, `↑`, `↓`, `←`, `→`, `^C`, `⤢` landscape fullscreen) emitting standard `KeyInput` / VT bytes into `LiveSession`.
- Pinch-to-zoom font size scaling and touch long-press text selection on `portable.rs`.

### Phase 4 — Optional iOS `UIView` host over `apple_render`

- Build a thin `UIView` + `UITextInput` shell backed by the existing AppKit/UIKit-free `apple_render.m` renderer when iOS requires Kitty graphics or higher-density cell throughput than `nsh_base::Terminal`.
