# GitHub workspace acceptance — 2026-11-10

Scope: `feat/github-workspace`, based on `2827f8a`, in `.worktrees/github-workspace`. This report covers the GitHub workspace and disposable-cache implementation, not unrelated changes in the main checkout. The current feature contract is [GitHub workspace](github-workspace.md).

## Automated validation

- `mise run test`: **180 passed**, no failures. Coverage includes source identity/ownership, schema migration, request coalescing, ETag handling, account/cache generations, pagination-family invalidation, and late later-page read/refresh rejection.
- `mise exec -- cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `mise exec -- cargo fmt --all -- --check`: passed.
- `mise run build`: packaged the native application successfully, also run as the dependency of both acceptance tasks.
- `mise run build-gallery`: packaged the standalone Gallery successfully.
- `mise run acceptance-github-workspace`: passed with a real AppServer, isolated home/Core and the deterministic executable gh fixture. Final evidence: `/private/tmp/cb-github-workspace-mj3tx5hv/results.json`.
- `mise run acceptance`: passed Develop → Review, blocked mention/notice, Archive/Restore/WAL, restart, and three-Board global concurrency. Evidence home: `/private/tmp/cb-5owdv2cf/home`.

The GitHub integration run checks automatic Issue/PR Backlog intake without dispatch, source deduplication, Local references, notification pagination/cache, versioned diff, comment/reply/Review receipt replay, rejected and uncertain publication outcomes, stale-head rejection, source refresh, explicit notification read, restart/cache restore, and cache-only change propagation with unchanged Cards. It verifies remote source/discussion prose is absent from the checked business tables.

The original acceptance bootstrap briefly started the default Core before switching its identity on the same isolated socket. The harness now configures its intended isolated Core before first startup and keeps that identity across profile restart; the complete acceptance passed afterward. GitHub fixture profiles use echo-only commands and do not invoke models.

Worktree-local logs are `.local/github-tests.log`, `.local/github-clippy.log`, `.local/github-fmt.log`, `.local/github-gallery-build.log`, `.local/github-workspace-acceptance.log` and `.local/github-baseline-acceptance.log`.

## Native interaction

The packaged production components ran in an owned application bundle with fixture home `/private/tmp/cb-github-workspace-u_v8hsgv/home`, private Core socket/configuration and deterministic gh transport. Interactive checks observed:

- GitHub rail entry above Archive; GitHub configuration absent from Settings.
- Board repository configuration, Local/GitHub filtering and visible/total counts.
- PR details and diff; line-comment draft and successful Review submission to the fixture.
- Collapsed comment/Review forms preserving diff reading space.
- Cache clearing rebuilding the visible list.
- A cache-only notification refresh changing the visible row without navigation.
- Changing the relation filter on notification page 2 returning to page 1.
- A pending line Review surviving a changed PR head, with the stale-version notice and disabled submission.

The standalone Gallery displayed the production PR diff with Open/Review requested filters and the offline cached-content scene. Native check metadata is `/private/tmp/cb-github-workspace-u_v8hsgv/native-checks.json`. The owned native apps, fixture AppServer and private Core were closed after validation. The live user Core/home was not used.

## Live-read boundary

The new transport also read the existing gh account's real notification inbox: 50 returned items, a next page, and a cached second read. No real remote mutations were performed. Evidence: `/private/tmp/cb-github-live-read-su98xn0l/results.json`.

Comment, reply and Review publication paths are exercised by automated/native fixtures, not a fresh live GitHub permission or protected-branch run. Existing GitHub App permissions remain relevant: the originally registered App did not request Issues write permission. Historical App/PR/merge acceptance in [GitHub integration](github-integration.md) does not establish fresh authorization for these new actions.
