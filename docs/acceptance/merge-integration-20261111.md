# GitHub, editor and updater integration acceptance

Status: integrated locally on main, 2026-11-11. No push or real release publication was performed.

## Source and conflict decisions

- Updater implementation: `b092870`, including release automation, paired migration framework, native update/recovery and scoped quarantine preparation.
- `feat/github-workspace`: `a18669b`, merged as `9f4c58a`.
- `worktree/optimize-editor`: `c230b8b`, the local branch referred to as optimize-editor. Its native editor/draft changes are preserved in the final merge.
- The two source worktrees were clean and were not modified. Their local branches remain intact.

Both feature branches independently used schema 9/API 7. The combined contract is desktop API 8/schema 10: migration 9 adds GitHub PR source uniqueness and normalizes old remote mirrors; migration 10 adds editor drafts. A compatibility marker records the prior schema-9 table/index shape so startup rollback works against either older branch, including retaining preexisting editor drafts. Production registry up/down steps use the updater's transaction framework rather than separate hand-coded commits.

The user explicitly approved the GitHub branch's existing export-before-cleanup transition. A cache-export preflight preserves legacy remote mirrors at `cache/github/legacy/v8.json` before database-only normalization. Local Card identity/instructions, user comments, execution history and Agent reports stay intact. A schema down does not recreate scrubbed remote prose. No whole-database backup was added.

UI resolution preserves all three New Card kinds (Local, GitHub Issue and GitHub PR), the common Markdown editor, source reference panel, durable drafts, GitHub workspace and global update/recovery. Upgrade session capture carries transient upload bytes separately from SQLite drafts, decodes older nested upload sessions and restores the New Card draft identity. The candidate may read `draft.list` during handoff before announcing UI readiness; writes stay gated. Release checks explicitly revalidate the GitHub response cache, so its event-based browsing cache cannot freeze update discovery. Editor reference queries use the common App/gh transport selection.

## Automated checks

- `mise run test`: 207 Rust tests passed. Includes both legacy schema-9 shapes and real up/down compatibility, the approved GitHub export/sanitization cases, stable draft revisions/tombstones and old/new upgrade session upload serialization.
- Strict workspace/all-target Clippy, formatting, cached/working diff checks and conflict-marker scan passed.
- `mise run acceptance`: `/private/tmp/cb-amjkls1c/home` (initial combined run); the final repeated run is recorded in `/private/tmp/canban-merge-acceptance-final.log`.
- `mise run acceptance-github-workspace`: final `/private/tmp/cb-github-workspace-ffwq9as2`, including Issue/PR intake, notifications/pagination/cache, versioned Diff, write receipts, stale-head rejection and source refresh without Agent dispatch.
- `mise run acceptance-dev`: `/private/tmp/cb-dev-start-3z_98rn6`, retaining a real PTY and preferences through current/legacy AppServer handoff with API 8.
- Three offline release tests passed. Optimized package and ad-hoc signature verification passed; artifact generation did not upload or publish.
- Six-case schema-8-baseline updater acceptance: `/private/tmp/cb-updates-dw2r9cif`, log `/private/tmp/canban-merge-update-acceptance.log`. Normal update exercises real paired migration 8→10; startup failure returns to 8. Corrupt package, failed up, failed boot, synthetic future schema and failed down/manual recovery passed. The future-schema harness derives its version from the candidate instead of hardcoding schema 9.

The versioned fixture was `20261110-5`, built from base `9f4c58a` plus the uncommitted merge resolutions. Its commit field identifies that fixture base, not a published final source revision. The final source merge was committed only after checks. Build/test logs are under `/private/tmp/canban-merge-*.log`.

## Native checks

The production Gallery editor scene showed Local/Issue/PR selection with the shared Markdown editor and PR-local instructions. No duplicate form was used.

The isolated packaged GitHub app at `/private/tmp/cb-github-workspace-okwkm7bg` showed Board/GitHub/Drafts navigation, Markdown headings/formatting, separate source-type controls and a retained local draft after switching to PR and back. AppServer `draft.list` confirmed the exact title/body were durable; `merge-native-evidence.json` records those checks. This initial native check preceded legacy cleanup integration, which was subsequently covered by the automated migration tests.

The final native updater run at `/private/tmp/cb-updates-pmxe24ky/normal` upgraded an actual schema-8 App to the combined schema-10/API-8 fixture while its deterministic interactive Agent was working. The Card, unsent comment and original Agent binding were restored; Settings showed `20261110-5` / Up to date and GitHub/Drafts navigation was present. SQLite was independently checked at version 10. Evidence: `native-ui-evidence.json`, `result.json` and `/private/tmp/canban-merge-update-native.log`.

## Limits

All application homes, Core sessions, HTTP/GitHub fixtures and copied bundles were disposable. The user's live database/Core was not migrated. GitHub writes used the deterministic gh subprocess fixture, and Agent continuity used a deterministic interactive provider on a real PTY. This is not new paid Codex/Claude, live OAuth/remote-PR publication, Chinese IME, devpc upgrade or GitHub Actions publication acceptance. No branch worktree was removed and no remote refs were updated.
