# NSH integration notes and version-scoped workarounds

Current pinned NSH dependency in root [Cargo.toml](../Cargo.toml):

- **Crate version:** `0.5.0` (`nsh-core`, `nsh-base`, `nsh-platform-macos`, `nsh-platform-linux`)
- **Git revision:** `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`
- **Last reviewed:** 2026-11-19

Use this document to track NSH-specific runtime quirks, rendering/input constraints, and Canban workarounds. When upgrading the NSH pin in `Cargo.toml`, audit each section below against the new revision and remove any workaround that upstream NSH has resolved.

## 1. Reactive tracking during theme installation (`0.5.0` @ `f4d68f7`)

- **Symptom:** Calling `install_theme()` or `sync_nsh_tokens()` inside a reactive `dynamic()` subtree (such as `workspace_root()`) can cause non-default desktop themes to trigger an infinite remount loop before `ui-ready`.
- **Workaround:** Call `install_theme()` at the top-level shell (`Canban()` / `Bootstrap()`) and wrap `sync_nsh_tokens()` in `untracked(...)` so theme installation and switches do not register reactive dependencies on enclosing `dynamic()` regions.
- **References:** `crates/ui-kit/src/tokens.rs` (`theme_install_and_switch_do_not_track_enclosing_dynamic_region`), commit `6c9d3c2`.

## 2. macOS `Editor` font cascade and `CanvasFont` caret measurement (`0.5.0` @ `f4d68f7`)

- **Symptom:** NSH `Editor` draws text with `FontFamily::new(MONO_FONT)` while measuring per-grapheme caret X offsets via `CanvasFont::parse("13px \"<MONO_FONT>\"")`, which splits on `,` and keeps only the first font family. Using `SF Mono` (hidden from `CTFontDescriptor` by family name) falls back to proportional `.AppleSystemUIFont`, and `.AppleSystemUIFontMonospaced` adds synthetic CJK-Latin inter-script spacing — both causing CJK/mixed-text cursor drift.
- **Workaround:** Keep `MONO_FONT` in `crates/ui-kit/src/tokens.rs` set to `"Menlo, PingFang SC"` on macOS and `"monospace"` on Linux, and guard CoreText caret measurement against drift.
- **References:** `crates/ui-kit/src/tokens.rs` (`mono_font_uses_named_latin_and_cjk_families_without_system_ui_monospace`), commits `bb6e051`, `4954ba9`.

## 3. Native terminal IME buffers and synchronized output (`0.5.0` @ `f4d68f7`)

- **Symptom:** Chinese IME composition (preedit/commit), Kitty keyboard protocol input, and synchronized terminal output can drift the cursor or lose focus if IME buffers are treated as immutable or responder focus is dropped across redraws.
- **Workaround:** Preserve mutable IME text buffers, restore responder focus explicitly, and stabilize cursor updates during preedit, commit, and synchronized output frames.
- **References:** `crates/terminal`, commits `74854fe`, `0364fdf`.

## 4. Uncomposited X11 GTK4 surfaces and Linux popups (`0.5.0` @ `f4d68f7`)

- **Symptom:** On Linux X11 sessions without a compositor, transparent GTK4 popups and overlays can render with solid black corners/boxes or unthemed chrome.
- **Workaround:** Sync GTK chrome theme tokens in `crates/ui-kit/src/linux_gtk.rs` and avoid unguarded transparent popup surfaces on uncomposited X11.
- **References:** `crates/ui-kit/src/linux_gtk.rs`, commit `4f78d20`.

## 5. Scroll culling with animated / absolutely positioned rows (`0.5.0` @ `f4d68f7`)

- **Symptom:** In tall scrollable containers (such as deep Board columns), positioning rows at `y = 0` and offsetting them purely via transforms can cause NSH scroll culling to hide visible rows or overlap tiles during scrolling.
- **Workaround:** Position rows at their resting layout `top` so NSH's viewport culling sees accurate bounds, and apply relative motion offsets around that resting position.
- **References:** `crates/ui/src/board.rs`, commit `b813c8c`.

## 6. Hover cursor stability inside popovers (`0.5.0` @ `f4d68f7`)

- **Symptom:** Rapidly crossing nested hit targets inside floating popovers (e.g., top-right notification/activity links) can cause delayed or flickering pointer cursors.
- **Workaround:** Attach pointer cursor styling to the outer stable interactive target (`press_target` / link container) rather than toggling cursor styles on inner text/icon leaves.
- **References:** commits `0308135`, `8ca486f`.

## 7. Theme-reactive scrollbars (`0.5.0` @ `f4d68f7`)

- **Behavior:** Platform overlay scrollers do not automatically match Canban's dark/named themes.
- **Workaround:** Replace platform overlay scrollers with Canban's custom theme-reactive scrollbars installed via `canban_ui_kit::scrollbar::install()` during `install_theme()`.
- **References:** `crates/ui-kit/src/scrollbar.rs`, commit `7a148d4`.
