# UI assets

## Notification sound

`notification.wav` is an original synthesized 240ms chime (mono PCM, 22,050 Hz,
880/1320 Hz tones with an attack and exponential decay). It is embedded by the
UI and played through NSH, without a system-sound dependency or network access.

## App icon

`app-icon.svg` is the original Canban app icon: an Agent's robot hand handing
the viewer a three-column Kanban board, with the just-finished Review card
highlighted. It is a flat geometric 1024×1024 vector on the macOS icon grid
(824-point rounded square, transparent margin).

`scripts/package.py` renders this SVG with `rsvg-convert` (Homebrew `librsvg`),
then uses macOS `iconutil` to generate `Contents/Resources/AppIcon.icns` with
16, 32, 128, 256 and 512-point images at 1× and 2×. `CFBundleIconFile` declares
the icon for Finder, the Dock and the application switcher. Normal, development,
Gallery and release bundles share this source; release signing happens after
the icon is installed, and the updater archive includes the whole bundle.
Generated PNGs and ICNS files are build artifacts, not additional source assets.

Install the build dependency with `brew install librsvg`, then run
`mise run build` (or `mise run build-release`). GitHub release CI installs the
same dependency. Editing the SVG and rebuilding is sufficient to update the
packaged icon; relaunch an already running app to load its new bundle resources.

Verified 2026-11-11: all ten image representations round-tripped through
`iconutil` with the expected RGBA dimensions in normal, development and Gallery
bundles. Finder displayed the new icons, and the packaged development UI started
with an isolated home/Core. `mise run build`, `mise run test` (207 tests),
`mise run acceptance` and `mise run build-release` passed. The release bundle
passed strict/deep signature verification, and an archive produced by
`canban-updater pack` contained the exact icon and plist. A missing renderer
failed with installation guidance without changing the existing signed bundle.
GitHub Actions execution and release publication were not performed for this
change.

Copyright 2026 JJy <jjyruby@gmail.com>. Apache-2.0, like Canban's original code.
