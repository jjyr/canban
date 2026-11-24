# NSH integration notes and version-scoped workarounds

Current pinned NSH dependency in root [Cargo.toml](../Cargo.toml):

- **Crate version:** `0.6.0` (`nsh-core` with `svg` feature, `nsh-base`, `nsh-platform-macos`, `nsh-platform-linux`, `nsh-platform-ios`, `nsh-platform-android`)
- **Git revision:** `7620f0145d3a83b0283ca7c397460b87497e039d`
- **Last reviewed:** 2026-11-24

Use this document to track NSH-specific runtime quirks, rendering/input constraints, and Canban workarounds. When upgrading the NSH pin in `Cargo.toml`, audit each section below against the new revision and remove any workaround that upstream NSH has resolved.

## 1. Reactive tracking during theme installation (`0.6.0` @ `7620f014`)

- **Symptom:** Calling `install_theme()` or `sync_nsh_tokens()` inside a reactive `dynamic()` subtree (such as `workspace_root()`) can cause non-default desktop themes to trigger an infinite remount loop before `ui-ready`.
- **Workaround:** Call `install_theme()` at the top-level shell (`Canban()` / `Bootstrap()`) and wrap `sync_nsh_tokens()` in `untracked(...)` so theme installation and switches do not register reactive dependencies on enclosing `dynamic()` regions.
- **References:** `crates/ui-kit/src/tokens.rs` (`theme_install_and_switch_do_not_track_enclosing_dynamic_region`), commit `6c9d3c2`.

## 2. macOS CoreText system monospace CJK-Latin spacing (`0.6.0` @ `7620f014`)

- **Symptom:** macOS `.AppleSystemUIFontMonospaced` adds synthetic CJK-Latin inter-script spacing in CoreText, which causes per-grapheme caret X measurement to drift from rendered mixed-text lines in `Editor`.
- **Workaround:** Keep `MONO_FONT` in `crates/ui-kit/src/tokens.rs` as an explicit named fallback cascade (`"SF Mono, Menlo, PingFang SC"` on macOS/iOS; `"JetBrains Mono, DejaVu Sans Mono, Liberation Mono, Noto Sans Mono, Noto Sans CJK SC, monospace"` on Linux/Android) rather than system UI monospace, and keep the CoreText caret-drift regression test in `crates/ui/src/editor.rs`.
- **References:** `crates/ui-kit/src/tokens.rs`, `crates/ui/src/editor.rs`, commits `bb6e051`, `4954ba9`.

## 3. Native terminal IME buffers and synchronized output (`0.6.0` @ `7620f014`)

- **Symptom:** Chinese IME composition (preedit/commit), Kitty keyboard protocol input, and synchronized terminal output can drift the cursor or lose focus if IME buffers are treated as immutable or responder focus is dropped across redraws.
- **Workaround:** Preserve mutable IME text buffers, restore responder focus explicitly, and stabilize cursor updates during preedit, commit, and synchronized output frames in Canban's native terminal view.
- **References:** `crates/terminal`, commits `74854fe`, `0364fdf`.
