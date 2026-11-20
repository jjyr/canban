# NSH integration notes and version-scoped workarounds

Current pinned NSH dependency in root [Cargo.toml](../Cargo.toml):

- **Crate version:** `0.6.0` (`nsh-core` with `svg` feature, `nsh-base`, `nsh-platform-macos`, `nsh-platform-linux`)
- **Git revision:** `5e3b0f78a999b11d0bad688ed08ec8fbab6e20be`
- **Last reviewed:** 2026-11-20

Use this document to track NSH-specific runtime quirks, rendering/input constraints, and Canban workarounds. When upgrading the NSH pin in `Cargo.toml`, audit each section below against the new revision and remove any workaround that upstream NSH has resolved.

## 1. Reactive tracking during theme installation (`0.6.0` @ `5e3b0f78`)

- **Symptom:** Calling `install_theme()` or `sync_nsh_tokens()` inside a reactive `dynamic()` subtree (such as `workspace_root()`) can cause non-default desktop themes to trigger an infinite remount loop before `ui-ready`.
- **Workaround:** Call `install_theme()` at the top-level shell (`Canban()` / `Bootstrap()`) and wrap `sync_nsh_tokens()` in `untracked(...)` so theme installation and switches do not register reactive dependencies on enclosing `dynamic()` regions.
- **References:** `crates/ui-kit/src/tokens.rs` (`theme_install_and_switch_do_not_track_enclosing_dynamic_region`), commit `6c9d3c2`.

## 2. macOS CoreText system monospace CJK-Latin spacing (`0.6.0` @ `5e3b0f78`)

- **Symptom:** macOS `.AppleSystemUIFontMonospaced` adds synthetic CJK-Latin inter-script spacing in CoreText, which causes per-grapheme caret X measurement to drift from rendered mixed-text lines in `Editor`. (Upstream NSH `0.6.0` PR #2169 resolved `CanvasFont` fallback-list truncation, so multi-family stacks like `"SF Mono, Menlo, PingFang SC"` now measure and draw identically.)
- **Workaround:** Keep `MONO_FONT` in `crates/ui-kit/src/tokens.rs` as an explicit named fallback cascade (`"SF Mono, Menlo, PingFang SC"` on macOS; `"JetBrains Mono, DejaVu Sans Mono, Liberation Mono, Noto Sans Mono, Noto Sans CJK SC, monospace"` on Linux) rather than system UI monospace, and keep the CoreText caret-drift regression test in `crates/ui/src/editor.rs`.
- **References:** `crates/ui-kit/src/tokens.rs`, `crates/ui/src/editor.rs`, commits `bb6e051`, `4954ba9`.

## 3. Native terminal IME buffers and synchronized output (`0.6.0` @ `5e3b0f78`)

- **Symptom:** Chinese IME composition (preedit/commit), Kitty keyboard protocol input, and synchronized terminal output can drift the cursor or lose focus if IME buffers are treated as immutable or responder focus is dropped across redraws. (Upstream NSH `0.6.0` PR #2288 also extends app-shortcut composition protection to hosted `NSTextInputClient` views.)
- **Workaround:** Preserve mutable IME text buffers, restore responder focus explicitly, and stabilize cursor updates during preedit, commit, and synchronized output frames in Canban's native terminal view.
- **References:** `crates/terminal`, commits `74854fe`, `0364fdf`.

## 4. Scroll culling with animated / absolutely positioned rows (`0.6.0` @ `5e3b0f78`)

- **Symptom:** In tall scrollable containers (such as deep Board columns), positioning rows at `y = 0` and offsetting them purely via transforms can cause scroll culling to misjudge visibility or overlap tiles during scrolling. (NSH `0.6.0` widens culling bounds for bounded transforms via `Core::reach_of`, and resting-top layout remains cleaner for Board rows.)
- **Practice:** Position rows at their resting layout `top` so viewport culling sees accurate bounds, and apply relative motion offsets around that resting position.
- **References:** `crates/ui/src/board.rs`, commit `b813c8c`.

## 5. Hover cursor stability inside popovers (`0.6.0` @ `5e3b0f78`)

- **Symptom:** Rapidly crossing nested hit targets inside floating popovers (e.g., top-right notification/activity links) can cause delayed or flickering pointer cursors.
- **Practice:** Attach pointer cursor styling to the outer stable interactive target (`press_target` / link container) rather than toggling cursor styles on inner text/icon leaves.
- **References:** commits `0308135`, `8ca486f`.

## 6. Theme-reactive scrollbars (`0.6.0` @ `5e3b0f78`)

- **Behavior:** Platform overlay scrollers do not automatically match Canban's dark/named themes.
- **Workaround:** Replace platform overlay scrollers with Canban's custom theme-reactive scrollbars installed via `canban_ui_kit::scrollbar::install()` during `install_theme()`.
- **References:** `crates/ui-kit/src/scrollbar.rs`, commit `7a148d4`.

## Retired workarounds (resolved in NSH `0.6.0` @ `5e3b0f78`)

- **Uncomposited X11 GTK4 popups, `.solid-csd` bezels, root backdrop, and separator min-width:** Resolved upstream in `nsh-platform-linux` (#2451, #2452, #2460); removed `crates/ui-kit/src/linux_gtk.rs` and `canban-ui-kit`'s `nsh-platform-linux` dependency.
- **`CanvasFont` single-family fallback truncation:** Resolved upstream in `nsh-core` / `nsh-base` / `nsh-platform-macos` (#2169); restored full `MONO_FONT` fallback stacks on macOS and Linux.
- **`Collapsible` closing reveal pointer/a11y hiding:** Resolved upstream in `nsh-base` (#2453); removed duplicate `.accessibility_elements_hidden` / `.pointer_events` wrapper overrides from `crates/ui-kit/src/tree_group.rs`.
