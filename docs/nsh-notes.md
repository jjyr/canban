# NSH integration notes and version-scoped workarounds

Current pinned NSH dependency in root [Cargo.toml](../Cargo.toml):

- **Crate version:** `0.7.2` (`nsh-core` with `svg` feature, `nsh-base`, `nsh-platform-macos`, `nsh-platform-linux`, `nsh-platform-ios`, `nsh-platform-android`)
- **Git revision:** `ff30a1509149c3d31f11723c46ff68e931806dc3`
- **Last reviewed:** 2026-11-28

Use this document to track NSH-specific runtime quirks, rendering/input constraints, and Canban workarounds. When upgrading the NSH pin in `Cargo.toml`, audit each section below against the new revision and remove any workaround that upstream NSH has resolved.

## 1. Reactive tracking during theme installation (`0.6.0` -> `0.7.2`)

- **Symptom:** Calling `install_theme()` or `sync_nsh_tokens()` inside a reactive `dynamic()` subtree (such as `workspace_root()`) can cause non-default desktop themes to trigger an infinite remount loop before `ui-ready`.
- **Workaround:** Call `install_theme()` at the top-level shell (`Canban()` / `CanbanMobile()` / `Bootstrap()`) and wrap `sync_nsh_tokens()` in `untracked(...)` so theme installation and switches do not register reactive dependencies on enclosing `dynamic()` regions.
- **References:** `crates/ui-kit/src/tokens.rs` (`theme_install_and_switch_do_not_track_enclosing_dynamic_region`), commit `6c9d3c2`.

## 2. macOS CoreText system monospace CJK-Latin spacing (`0.6.0` -> `0.7.2`)

- **Symptom:** macOS `.AppleSystemUIFontMonospaced` adds synthetic CJK-Latin inter-script spacing in CoreText, which causes per-grapheme caret X measurement to drift from rendered mixed-text lines in `Editor`.
- **Workaround:** Keep `MONO_FONT` in `crates/ui-kit/src/tokens.rs` as an explicit named fallback cascade (`"SF Mono, Menlo, PingFang SC"` on macOS/iOS; `"JetBrains Mono, DejaVu Sans Mono, Liberation Mono, Noto Sans Mono, Noto Sans CJK SC, monospace"` on Linux/Android) rather than system UI monospace, and keep the CoreText caret-drift regression test in `crates/ui/src/editor.rs`.
- **References:** `crates/ui-kit/src/tokens.rs`, `crates/ui/src/editor.rs`, commits `bb6e051`, `4954ba9`.

## 3. Native terminal IME buffers and synchronized output (`0.6.0` -> `0.7.2`)

- **Symptom:** Chinese IME composition (preedit/commit), Kitty keyboard protocol input, and synchronized terminal output can drift the cursor or lose focus if IME buffers are treated as immutable or responder focus is dropped across redraws.
- **Workaround:** Preserve mutable IME text buffers, restore responder focus explicitly, and stabilize cursor updates during preedit, commit, and synchronized output frames in Canban's native terminal view.
- **References:** `crates/terminal`, commits `74854fe`, `0364fdf`.

## 4. NSH `0.7.2` API migrations (`0.7.2` @ `ff30a150`)

- **`KeyOptions.over_focused`:** Changed from `bool` to `nsh_core::keymap::OverFocused` (`Never`, `Always`, `PointerFocus`). `crates/ui-kit/src/markdown.rs` uses `OverFocused::Always` for `mod+enter` submission over focused Markdown editors.
- **`nsh_core::mobile` scope narrowing:** System form controls (`segmented_control`, `SegmentedControlItem`, `switch`) were removed from `nsh_core::mobile` in `0.7.2` (leaving `TabBar` / `TabBarItem` / `open_app_settings`). `crates/ui/src/mobile/chrome.rs` uses Canban's unified tactile `mobile_segmented_control` and `canban_ui_kit::toggle` (`mobile_switch`) across iOS, Android, and Gallery.

## 5. Mobile horizontal gesture primitives (`0.6.0` -> `0.7.2`)

- **Capability:** NSH provides native UI-thread horizontal gesture arbitration (`PanActivation::horizontal` / `PanDirection` + `pan_values` + `bind_translate_x` + `on_pan_end`) as well as native-thread horizontal snapping pagers (`nsh_base::carousel`).
- **Usage:** Canban Mobile composes a reusable `swipe_row` primitive in `crates/ui/src/mobile/chrome.rs` for Inbox left/right triage (Done / Defer) alongside `nsh_base::carousel` for Board stage switching and Card `Conversation | Terminal | Diff` paging, keeping at most one horizontal gesture owner per screen to avoid nested gesture conflicts.
- **NSH `0.7.2` mobile fixes included:** pull-to-refresh spinner retention on dismiss (`7568e6de6`), iOS offscreen GPU/shader view culling (`10d6d105d`, `6427aaa20`), `UISegmentedControl` selection transition stability (`075b14958`), iOS `context_menu` dismiss-overlap guard (`e6baacae0`), iOS standalone backdrop sync (`6cf51dcf8`), and Android predictive-back progress delivery (`33d119ce5`).
