# Workflow/event-system integration acceptance

Executed on macOS arm64, 2026-11-11, while merging
`worktree/kanban-event-system` at `8ae1590` into `main` at `3c762b4`.
The latter preserves the previously accepted local backend activation and daily
release checks. The combined build uses schema 12 / desktop API 10.

## Integration decisions

- Keep the SQL-backed YAML sources, immutable Workflow revisions, configuration
  conversations and durable event subscriptions from the feature branch.
- Keep checkout/branch-scoped development, explicit installed-backend activation
  and persisted rolling-24-hour release checks from main.
- Resolve the shared AppServer exports/modules and retain API 10 with the daily
  interval. Reconcile current documentation with schema 12 and isolated dev
  startup; historical acceptance remains tied to its original schema/API.
- Derive backend acceptance's target schema from `build-info`, rather than
  hardcoding schema 10. Add an explicit schema-10 activation case alongside the
  current-schema, schema-8 and legacy-7 cases.

## Executed verification

All runtime fixtures used disposable homes and isolated Herdr Cores. No installed
App or live `~/.canban` service was changed.

| Check | Result / evidence |
| --- | --- |
| Build and workspace | `mise run build`, 231 Rust tests, workspace/all-target Clippy with warnings denied passed. Logs: `/private/tmp/canban-event-merge-build.log`, `/private/tmp/canban-event-merge-tests.log`, `/private/tmp/canban-event-merge-clippy.log`. |
| Standard application | `mise run acceptance` passed at `/private/tmp/cb-p0fvoj7u/home`; Develop/Review, mentions/gates, archive/restore, restart and cross-Board budget. Log: `/private/tmp/canban-event-merge-acceptance.log`. |
| Workflow execution | `mise run acceptance-workflows` passed at `/private/tmp/cb-workflows-87214ws0/evidence.json`; frozen run revisions, explicit approval, durable restart, SQL YAML publication/conflicts, file transport and configuration-session history reuse/resume. Log: `/private/tmp/canban-event-merge-workflows.log`. |
| Runtime observation | `mise run acceptance-events` passed at `/private/tmp/cb-events-irym95z5`; idle scan avoidance, shared cwd, missed snapshot/failed exit and reconnect behavior. Log: `/private/tmp/canban-event-merge-events.log`. |
| Backend activation | `mise run acceptance-backend` passed at `/private/tmp/cbb-bdwrw4yu`; same schema skips migration/snapshot; 10/8/7→12 each migrate once; injected migration/boot failures restore the original database and recover explicitly; no downloads, bundle changes or PTY replacement. Daily checks remain throttled across restart and failure. Log: `/private/tmp/canban-event-merge-backend.log`. |
| Branch isolation | `mise run acceptance-dev` passed at `/private/tmp/cbd-lzx65ijy`; independent branch/checkout identities, ownership, targeted stop and retained Core PTY. Log: `/private/tmp/canban-event-merge-dev.log`. |
| Native Workflow App | `/private/tmp/cb-workflows-htzrwku8/merge-native-evidence.json`: opened Project Workflows, edited original YAML with a comment, clicked Save and apply, observed Active v2 and disabled clean-save controls, and checked exact persisted text/revision. Edit with agent continued the existing configuration Terminal; Back to Workflows retained v2. Log: `/private/tmp/canban-event-merge-native.log`. |
| Native backend App | `/private/tmp/cbb-by74zhmy/native/result.json`: clicked the real API-10 App's Update backend prompt against the legacy service, observed pending then normal Board entry. Zero download/migration for the already-current schema, unchanged bundle and retained PTY. Log: `/private/tmp/canban-event-merge-backend-native.log`. |
| Production Gallery | `CANBAN_GALLERY_SCENE=workflows mise run gallery`; verified source/editor/clean-save composition and opened Edit with agent with its Prompt/Profile controls. Task-owned Gallery closed. Log: `/private/tmp/canban-event-merge-gallery.log`. |

One initial parallel acceptance invocation collided while both tasks packaged
`.local/Canban.app`; codesign saw the other packager's temporary executable.
The backend task was rerun serially and passed. This was a test orchestration
collision, not a passing result or a product fix.

The final process audit found script-based fixture UIs whose Python exec had
changed their recorded process identity. Backend-harness cleanup now also matches
only its disposable bundle's exact UI command, including earlier recovery
attempts. The complete matrix passed again at `/private/tmp/cbb-gi84a85m`, log
`/private/tmp/canban-event-merge-backend-cleanup.log`; the final process audit
found no remaining task-owned App, AppServer or Core processes.

Formatting, Python syntax and `git diff --check` also passed. Native configuration
Agents use deterministic programs over real PTYs; no paid Codex/Claude run or
provider authentication is claimed. The download/bundle-swap release matrix,
GitHub publication and multi-client upgrade coordination were not rerun here.
Schema-12 downgrade retains the feature branch's refusal to discard YAML sources
or active configuration sessions; local activation failure uses its snapshot
recovery. See [updates](updates.md) and [Workflow implementation](workflow-implementation.md).
