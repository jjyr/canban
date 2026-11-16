# GitHub synchronization acceptance — 2026-11-16

Card 96, branch `canban-0afd/96`, based on `f34f5ec9`. Desktop API 61.
The current behavior is specified in [GitHub workspace](github-workspace.md).

## Implemented

- Icon-only Board synchronization action with queued, animated running, failed,
  paused and successful states; detailed accessible labels/tooltips.
- Controlled intake toggles and shared anchored searchable scope pickers. Turning
  intake off/on retains the selected draft scope. Save remains explicit.
- Background local-remote discovery and separate fork/parent metadata discovery.
  Choosing a source changes the draft, not Git remotes.
- Same-repository rule saves avoid network verification. Saves queue a run; two
  event-woken workers coalesce requests by project/generation independently of
  browsing refresh. Old generations cannot overwrite newly saved rules.
- Manual synchronization works while automatic synchronization is paused. Issue,
  PR and observation errors are aggregated and retained rather than hiding PR
  failures behind an earlier successful Issue phase.
- Existing synchronous `github.sync` callers remain supported and serialize with
  queued work. Snapshot status merges across connected hosts.

## Automated validation

- `mise run build`: passed, packaged `.local/Canban.app`.
- Affected AppServer/Kanban/UI/UI Kit/types suites: 360 passed with `-j 4`.
- After the final picker and event-wakeup changes,
  `mise run test -- -j 4 -E 'test(github) | test(repository_binding)'`:
  25 passed, 525 excluded by the filter.
- `mise run acceptance-github-native`: headless portion passed using real
  AppServer/CLI/gh-subprocess boundaries against a deterministic remote and an
  isolated Core. Home: `/private/tmp/cb-github-workspace-ff8qepq2/home`.
  Same-repository Save returned in 0.0186 seconds with a two-second Issue response
  delay. Checks include automatic intake after Save, paused manual sync, PR failure
  visibility/retry, source uniqueness, write receipts, cache restart and no remote
  bodies in business storage.
- `mise run acceptance`: passed; isolated home `/private/tmp/cb-sm3zu_li/home`.
- `git diff --check`: passed.

An intermediate parallel run timed out waiting five seconds for the existing
background Git delivery test while builds were active. The complete bounded-
concurrency rerun passed, without changing that test. A new UI test initially
omitted the required Workflow fixture; correcting that fixture passed the full
suite and final targeted run.

## Native interaction

Operated the packaged fixture app through Cua Driver, separately from the live
Canban application. Verified Board/settings navigation; PR scope selection to
Repository collaborators; toggle off/on retaining that selection; Save; Draft PR
toggle and persisted RPC readback; in-progress synchronization with Save already
enabled; Board refresh without opening settings; injected PR HTTP 403 represented
by the warning icon and stage-specific accessible text; retry restoring success.
The Board retained all three fixture Cards and no Agent was launched.

`CANBAN_GALLERY_SCENE=github-sync-settings mise run gallery` rendered the production
settings/toggles. Additional deterministic scenes cover `github-sync-queued`,
`github-sync-issues`, `github-sync-prs`, `github-sync-failed`,
`github-sync-success` and `github-sync-paused`. The fixture app, server/Core and
Gallery were stopped after inspection. Cua Driver required coordinate fallback for
some invalidated accessibility elements; those rejected actions were not counted
as successful interactions.

These are fixture UI/network checks, not live OAuth or protected-repository write
acceptance. No GitHub write API was used against a real repository. The development
bundle was not installed over `/Applications/Canban.app`.

## Live Fiber source correction

The installed application's project had selected `jjyr/fiber` via `origin`;
GitHub returned empty open Issue and PR lists for that fork. Through the installed
Canban CLI, changed only its GitHub binding to `nervosnetwork/fiber`, retaining all
existing intake/filter/delivery settings and Git remotes. Subsequent CLI reads
confirmed 79 Issue Cards and 19 PR Cards, all 98 in Backlog, and no source error.
The explicit legacy synchronous CLI call exceeded its client read wait, but later
state reads verified completion. This correction used the existing installed
backend; it does not claim deployment of the new UI/worker implementation.
