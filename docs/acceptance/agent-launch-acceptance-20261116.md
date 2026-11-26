# Agent launch adaptation acceptance — 2026-11-16

Scope: the launch adapter in `canban-profiles`, integrated with Card/comment,
Terminal, schedule, merge-batch, configuration, and Review Agent launches.
The implementation branch started from main at `32e1ad44d8831f17dc3a4050099171c56a563a38`.

## Selected behavior

Only profiles explicitly declaring `preset: "codex"` receive per-invocation
`shell_environment_policy.set` CLI overrides in addition to process env.
Other providers and plain shells pass through unchanged. Existing user
inheritance/exclusion/include policies are preserved. Omitted variables are not
cleared from a daemon's inherited environment; that behavior is intentionally
outside this change. No prompts, global provider configuration, or live daemon
were modified.

## Checks

- `mise run test -- -E 'package(canban-profiles) | package(canban-app-server)'`:
  202 tests passed. The opt-in real-Codex test was skipped here and run separately.
- `mise run acceptance-agent-launch`: passed using Codex CLI 0.169.0 on macOS
  arm64. The test sends production-generated argv to the real Codex config
  parser, then exercises concurrent Card/cardless threads and resumes a persisted
  thread after restarting an isolated app-server. It verifies Unicode, quotes,
  backslashes, newlines, DEL, updated context, unchanged legacy exclusion rules,
  preserved user-set variables, and the agreed omitted-key inheritance behavior.
  No model requests or live user Core/daemon are involved.
- `mise run build`: passed; the branch-specific `.local/Canban.app` was packaged.
- `mise run acceptance`: passed with isolated Canban home and Herdr Core
  (`/private/tmp/cb-rv7mi6yn/home`, Core `3194467786f35a9b55485ef7fbd0f1ab`).
  This covered Develop/Review, comment mention dispatch, archive/restore,
  restart recovery, and concurrent scheduling across three Boards.
- `git diff --check`: passed.

The linker emitted the existing large `__eh_frame` warning; builds completed.

## Limits

The real-Codex check uses app-server protocol shell commands rather than paid
model generation or an interactive TUI. It verifies the real parser and thread
execution boundary, not a full native-UI-to-Codex TUI interaction. No remote SSH
host, provider MCP/hook environment propagation, or other Codex release was
exercised. No installed application or user profile was updated.

See the [launch contract](product/agent-profiles.md#per-invocation-launch-adaptation)
and [Codex configuration reference](https://developers.openai.com/codex/config-reference/).
