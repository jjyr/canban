# Board filter acceptance — 2026-11-17

Card #102 unifies the Board toolbar and adds Mine plus durable filters. The
behavioral contract is in [GitHub workspace](github-workspace.md#unified-board-filters-and-persistence).
Desktop API 62 adds the `utilities.configure.board_filter` patch and the
`preferences.board_filters` response field.

## Automated checks

- `mise run check` passed.
- `mise run build` passed. Bullet's isolated executables were copied into the
  conventional `target/debug` paths required by the packager; the shared build
  configuration was not changed.
- `mise run test`: 578 tests passed; one ignored test remained skipped.
- A final targeted run passed the two `github::filters` UI tests and the deep
  Review-column scrolling regression after the identity-notice subscription
  adjustment.
- `mise run acceptance` passed with isolated Canban and Herdr homes at
  `/private/tmp/cb-apzom08a`.
- `mise run acceptance-github-workspace` passed with isolated fixture data at
  `/private/tmp/cb-github-workspace-il41iknl`, including source intake,
  deduplication, Local references, source refresh and restart/cache restoration.
- `git diff --check` passed.

Rust execution used `RUSTUP_AUTO_INSTALL=0` and the `bu exec` wrapper with the
repository's pinned mise toolchain.

The new regressions cover Local/Issue/PR authorship, case-insensitive Mine,
importer and assignee exclusion, unavailable source metadata, signed-out Local
behavior, mutually exclusive author selection, type/state transitions,
account/host-qualified Project persistence, reset, atomic rejection of invalid
patches, and common toolbar controls on Local Projects. The scrolling fixture
now accounts for the toolbar's reduced viewport while retaining its original
bottom-card and snapshot-refresh assertions.

## Native observations and remaining interaction checks

The final `.local/Canban.app` ran against a disposable Canban home and isolated
Herdr Core at `/private/tmp/cb-filters-umqwa0hu`. The fixture contains Alice's
Local Card and PR, plus Bob's Local Card and Issue. After seeding Mine through
the public preferences RPC and restarting the desktop, the native Board
restored `Author: Mine`, displayed only Alice's Local Card and PR, and showed
`2 / 4`. Seeding Reset through the same RPC and restarting restored Anyone,
All cards and all four Cards. This verifies startup restoration and rendered filtering; it does not
establish UI-originated saving or orderly Quit flushing.

The native Gallery `board-filters-mine` scene displayed the own Local Card and
PR (`2 / 5`), excluded Bob's imported PR and unavailable source details, and
explained the missing metadata. The `board-filters-empty` scene at 720 points
displayed the active tag chip, missing-source explanation, filtered-empty
message, Reset action and `0 / 5` count. Both use production Board components.

The macOS session became locked. Cua Driver reported `screen_locked`, exposed
no accessibility tree, and refused input. Screenshots remained available.
Interactive dropdown selection, searchable author replacement, Command Palette
parity, Project switching, UI-originated save/Reset across orderly Quit, and
narrow-window interaction still require an unlocked native session. These are
explicitly pending; compilation, headless tests and seeded startup screenshots
are not full interaction acceptance.
