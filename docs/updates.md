# Internal releases and desktop updates

Status: implemented on macOS arm64, updated 2026-11-11. Local fixture and native acceptance are recorded below; GitHub publication has not been exercised.

## Product boundary

A push to GitHub `main` (including a merged PR) triggers the release workflow. Releases do not become Cards or workflow stages. The installed AppServer checks for published internal releases once per rolling 24 hours and on explicit request. Automatic checks persist their start time before networking, including failed attempts, so restarting does not trigger another check within the interval. First launch or an overdue restart checks immediately. Manual checks bypass the interval; interrupted checks remain inspectable and may be retried manually. The compact navigation rail reserves its bottom for a highlighted Update button when a newer compatible release exists. Clicking it downloads and immediately installs/restarts; it never waits for automatic Agent tasks to complete. Download/progress/error/retry share the same operation path. Settings offers a manual check and current version details.

```text
PR merge / push main -> GitHub Actions -> versioned GitHub Prerelease
                                             |
                          automatic / manual version check
                                             |
                              bottom navigation Update button
                                             |
                                download -> verify -> updater
                                             |
                         stop old UI/service -> install -> restart
                                             |
                             healthy -> finish and flush reports
                             failed  -> down migrations + old App
```

## Release identity and publication

Display/tag versions are `YYYYMMDD-N`, using Asia/Shanghai and a numeric daily sequence. Compare the date and numeric sequence, not lexical strings. Reserve tags atomically; publication retries reuse their reservation, and failed reservations may leave gaps. Build the triggering commit, never a moving main checkout. Drafts remain invisible until every artifact and manifest is uploaded. Internal releases are prereleases; enumerate published releases instead of relying on GitHub's stable-only latest endpoint.

Reservation/retry and publication look up releases through the authenticated, paginated release list, which includes drafts. GitHub's release-by-tag endpoint resolves published releases and can return 404 for the reserved draft. All draft asset replacement and publication then address the found release ID; a missing reserved release fails without creating another during publication. Published releases remain immutable.

The manifest binds version, commit, API/schema, platform/architecture, asset name, byte size and SHA-256. UI, Agent CLI and standalone updater ship together in one App bundle. Display versions are separate from Cargo semver and the numeric macOS bundle version. A checksum detects incomplete/mismatched assets; HTTPS and the configured release repository define the initial distribution trust boundary. The source/release repository is private: reuse Canban GitHub sign-in, and require that its App installation has access to `jjyr/canban`. Private assets use the authenticated GitHub API octet-stream route; follow signed storage redirects without the GitHub credential. No Keychain or second token store is introduced. Public Developer ID signing/notarization and other architectures are separate distribution work.

## Upgrade and failure recovery

On macOS, after SHA-256 verification and safe extraction, inspect the candidate App recursively and remove only existing `com.apple.quarantine` attributes before its first `build-info` execution. Repeat that scoped preparation on the final same-filesystem candidate copy before creating the install journal or stopping the old App. Other extended attributes, permissions, the installed App, application data and unrelated paths are untouched. Preparation never follows symlinks and rejects links/special files. A failed inspection/removal aborts preparation, surfaces the error and leaves the old UI/AppServer running. An absent quarantine attribute is a no-op; repeated preparation is safe. This uses native extended-attribute APIs (equivalent to targeted `xattr -d`), not `xattr -cr`. It does not replace Developer ID signing/notarization or grant TCC/filesystem permissions. Browser-downloaded first installation remains a separate user-open/distribution concern.

Download/validation finishes before stopping the current App. A detached updater owns a durable journal and exclusive upgrade lock. Service startup checks that lock before opening/migrating the database. The updater stops only the identified UI and owning AppServer, waits for service-lock release and switches the entire staged App bundle on the same filesystem. Herdr Core, existing PTYs, provider sessions and Git worktrees stay alive. No automatic-task drain is required.

Candidate startup must report the exact version/commit and UI readiness. Until that handshake commits the upgrade, business writes, new scheduling and GitHub effects are held. A failed start returns the database to its pre-upgrade schema using the new version's down migrations, restores the old bundle and restarts it. This is automatic recovery only for the installation/startup transaction, not arbitrary crashes after the new version has accepted writes. CLI report/reply operations with stable idempotency keys persist during handoff and retry after success or recovery. Arbitrary Git/user mutations are never blindly replayed.

If down/restore fails, preserve the current database, bundle copies, pending reports and journal/log; stop with a manual-recovery error. Do not open it with an incompatible old server or repeatedly retry migration. Release installation does not make an automatic database backup or whole-home snapshot. The local backend activation described below takes a database snapshot only when a schema upgrade is needed. In-progress Agent-created files and worktrees are not rolled back.

## Migration authoring rules

Schema 8 is the legacy baseline. Each new migration requires a stable, ordered version and paired up/down functions. All pending up steps and their schema-version updates run in one SQLite transaction; a failed up is rolled back by SQLite, not by invoking down on a partially executed step. All down steps for an upgrade run in reverse order in one transaction. A failed down rolls back that attempt and leaves the last committed schema for manual repair.

Migration code is database-only, deterministic and transactional. Do not perform Git/network/file operations, change journal mode, run VACUUM or commit its own transaction inside a migration. Maintain parent/child data integrity; validate schema and foreign keys before committing. Preserve fields/data where practical. A down function can recreate a dropped column/table but cannot recreate discarded values: prefer additive changes and record any data-loss implications explicitly. Never claim schema reversal is a data backup.

Add meaningful tests for up/down round trips with representative records, failure midway through a multi-step up, failure during down, and rejection of unsupported/newer schemas. A new release's rollback implementation must retain every down step required to reach the supported old baseline. Existing schema 0–8 initialization remains compatible; automatic down never crosses that baseline.

The implementation lives in `crates/storage/src/migrations.rs`. Append a `Migration { version, up, down }` to `STEPS` and advance `CURRENT` together; versions must be consecutive after 8. The registry rejects a mismatched ceiling. Each function receives the framework's `rusqlite::Transaction`; do not open another connection or transaction. `Store::open` applies pending up steps. The candidate CLI exposes `database --target N` for the installer, with the same startup fence and exclusive service ownership. It does not start product or Herdr services. The registry is `CURRENT = 12`: schema 9 moves legacy GitHub mirrors to disposable cache storage and adds PR source uniqueness; schema 10 adds durable editor drafts. A separate cache-export preflight runs before the SQL-only migration transaction, from both Store startup and the installer CLI. Export failure prevents cleanup; migration failure leaves the export available. The user explicitly approved export-before-cleanup. Downgrade reverses schema changes but cannot recreate scrubbed remote prose. Schema 11 adds the durable event journal and Workflow execution/subscription tables; schema 12 adds original YAML, SQL source references/templates and configuration sessions. Down refuses to discard authored sources or active configuration sessions; local backend activation uses its pre-migration snapshot on startup failure instead. See [Workflow implementation](product/workflow-engine.md). The schema-10 compatibility marker records whether a legacy schema-9 database already had editor drafts and/or the GitHub source index, preserving its original shape on rollback.

## Implemented ownership and interfaces

```text
GitHub Actions / scripts/release.py
  reserve tag -> test -> build-release -> package -> publish draft
                                     |
canban-github -> authenticated release/asset requests
                                     |
AppServer update.* -> canban-updater library -> detached updater binary
       ^                   |                         |
       |              feed + staging          journal + bundle switch
UI rail / Settings         |                         |
       |             SHA-256 / manifest        candidate CLI database
       +--- session save / ready ack --------> server + UI health check
                                                   |
                                     failure -> down + old bundle
                                                   |
                                     failed recovery -> Recovery UI
```

`canban-updater` is a separate crate/binary. It owns discovery, bounded download/extraction, update state, locking, the durable Agent outbox and bundle replacement. AppServer owns the shared GitHub account client and the user operation boundary. Storage owns paired migrations; UI owns rail/Settings presentation and session capture. The Recovery screen is independent of AppServer and SQLite.

- `update.state {}` returns build, discovery/progress/error state and the current install token/phase.
- `update.check {}` starts a background check. Concurrent check/install operations share an operation lock.
- `update.install { version, ui_pid, app_id }` requires the exact currently offered version and the owning packaged UI process. Agent-bound requests cannot install. The manifest/candidate API is allowed to advance; the new App's matching UI and CLI are verified together.
- `canban build-info` returns `{ version, commit, schema, api, platform }`. The current desktop API is 10 and schema is 12. Local unversioned builds identify themselves as `development` / `unknown`.

Update state is under `~/.canban/updates/`: `state.json`, `install.json`, `updater.log`, `operation.lock`, `upgrade.lock`, `jobs/<token>/canban-updater`, `outbox/` and temporary staging/session files. Successful replacement retains the previous whole App bundle beside the installed App. There is no automatic cleanup of previous bundles in this first version.

Local reports/replies carry their original request key through the file outbox. Remote reporting uses its existing durable machine spool; the bridge leaves report/reply requests unanswered during handoff so the restored service can consume them. The remote bridge also rejects update installation. Delivery resumes only after the install commits or rollback succeeds; permanent application rejection preserves a `.pending-manual` payload and `.rejected` explanation. Other operations are rejected during handoff instead of queued. The initiating UI captures its route, Card drafts (including pasted attachment bytes), Terminal tabs and Archive query before shutdown. Interrupted uploads become explicitly retryable. UI readiness requires the first applied snapshot and a rendered frame; this prevents a restored Card route from opening before its data exists. Other concurrently open UI clients are not automatically restarted by the initiating client's update.

## Build, bootstrap and recovery

The in-app updater remains the release installation path. A newer App installed
externally can also activate its bundled backend through the explicit startup
action below. Opening an App never silently replaces an answering backend.

`mise run build` packages UI, CLI and updater together. `mise run build-release` produces optimized, ad-hoc-signed binaries; it does not publish. `.github/workflows/release.yml` builds the triggering SHA on a macOS arm64 runner, uploads `Canban-macos-aarch64.tar.zst` plus `manifest-macos-aarch64.json`, and publishes the completed draft as a prerelease. Packaging includes existing third-party notices. Developer ID signing/notarization is not implemented.

Development uses `mise run dev`, which derives an isolated checkout/branch home
and disables channel updates; it does not replace the installed `~/.canban`
service. For an ordinary installed App, startup compares the backend and App:

- **Older backend:** show **Update backend** before entering the workspace.
- **Equal version:** enter normally.
- **Newer backend:** enter normally with **Please use a newer Canban App**. Never downgrade the backend.

Published `YYYYMMDD-N` versions are compared numerically when both are available;
legacy/unversioned builds fall back to desktop API level (missing legacy API is
zero). Equal-API unversioned builds do not establish source-version ordering.
This is a version rule, not a negotiated protocol compatibility range.
An explicit `CANBAN_ENDPOINT` does not offer local replacement; update the backend
on its owning host. Bound Dev Apps are restarted through `mise run dev`.

**Update backend** uses the CLI/updater beside the currently running App. It
rechecks PID/process identity, home and version, then uses the existing updater
journal, service/upgrade locks, startup fences, outbox and exact server/UI
readiness. It quits the startup prompt, stops only the identified backend and
starts the installed matching pair. It performs no release check, asset download
or App-bundle replacement. Herdr Core and existing PTYs stay alive.

The worker reads the actual database schema after service shutdown. Equal schema
skips the migration CLI and snapshot entirely, even if the old service binary
was left behind after a newer App was installed. A pending schema upgrade first
captures a SQLite snapshot including committed WAL data, then runs the installed
CLI's database-only migration. Pre-schema-8 initialization uses the existing
legacy Store path to the current schema; a newer database is refused. A failed
migration or candidate boot restores that snapshot and stops at manual Recovery;
the saved worker's explicit `--recover` retries activation after the cause is
fixed. A failure before database work reopens the startup prompt. It never
restores or downloads an older App bundle.

Subsequent installed releases use the sidebar action. Access to the private release repository requires existing Canban GitHub sign-in and installation access to that repository; check failures are inspectable in Settings. No credentials are included in artifacts or diagnostics.

An interrupted install is recovered using the saved detached worker, with a single recovery owner. A `manual_recovery` journal is never automatically retried. The standalone Recovery window displays the error/log location and opens the update directory without connecting to AppServer or opening SQLite. After fixing the reported cause, use the token from `install.json` to retry the saved worker:

```sh
"$HOME/.canban/updates/jobs/<token>/canban-updater" --home "$HOME/.canban" --recover
```

Do not delete the journal, pending reports or bundle copies to bypass a failed down. Manual repair must first establish the actual schema and a compatible binary. A corrupted journal also fails closed. A normal crash after a committed update does not trigger database downgrade.

## Verification boundary

Use isolated homes, bundle copies, HTTP release fixtures and Herdr Cores. Cover normal update, corrupt/wrong-architecture assets, concurrent checks/install, upgrade-lock startup gating, exact candidate identity/readiness, forward migration failure, candidate startup failure with down, down failure requiring manual repair, and running-Agent report/reply delivery across the restart. Native Gallery exercises bottom-rail pending/progress/error states; packaged-App acceptance exercises the actual install/restart. CI/public release publication must be reported separately from local fixture acceptance.

[Local backend activation and daily checks acceptance](backend-update-acceptance-20261111.md)
records the 2026-11-11 matrix, real startup-button handoff, newer-backend entry
and isolated development regression checks.

### Executed acceptance (2026-11-10)

Historical pre-merge evidence: these fixtures used schema 8/API 7. See [combined integration acceptance](merge-integration-20261111.md) for the current schema-10/API-8 release.

All application/Core/bundle fixtures were disposable; the user's live home/Core were not used. Fixture versions (including `20261110-4`) were compiled from the uncommitted working tree based on `2827f8a254617e86b172444e41afe288c8d0a425`; their commit field identifies that base, not a published source revision. No real release tag was reserved.

| Check | Result and evidence |
| --- | --- |
| Workspace | 181 Rust tests passed; workspace/all-target Clippy with warnings denied, formatting and diff checks passed. Includes migration roundtrip/failure, journal/startup fencing, queue idempotency, remote report deferral/replay and authenticated asset redirect credential stripping. |
| Release logic/package | Three offline release tests passed (Shanghai sequence/retry identity, atomic collision and published immutability). Optimized `20261110-4` fixture packaged with version/commit/API/schema, manifest size/SHA-256 and ad-hoc signature. No GitHub upload/publication was performed. |
| Six-case matrix | `mise run acceptance-updates -- --baseline BASELINE_APP --candidate CANDIDATE_APP`; `/private/tmp/cb-updates-rsi9cf6e`, log `/private/tmp/canban-updates-acceptance-complete.log`: normal, corrupt, failed up, failed candidate boot, schema rollback and failed down all passed. Rejects Agent-triggered install, stale target and double install; checks report delivered once and exact live terminal/Core/worktree identity. |
| Migration evidence boundary | Synthetic unit steps exercise actual paired transactional schema 8→9→10→8. The installer matrix wraps the candidate CLI to simulate a future schema 9 and down failure against real scratch SQLite. That pre-merge fixture itself remained schema 8. |
| Native packaged App | `/private/tmp/cb-updates-7s057ul3/normal`, `native-ui-evidence.json` and `result.json`: clicked the bottom Update action while the fixture Agent was Working; installed `20261110-4`, automatically restored the Card and unsent comment, showed the version/Up to date in Settings, and preserved the Agent/report identity through restart. |
| Recovery UI | `/private/tmp/cb-update-recovery-czt3inge`: actual packaged Recovery window opened and quit with a deliberately invalid SQLite-file payload unchanged. Production Gallery pending/progress and `CANBAN_GALLERY_SCENE=update-recovery` layout were inspected natively. Gallery recovery Quit returns to its fixture shell; packaged recovery Quit exits. |
| Development handoff | `/private/tmp/cb-dev-start-npf7kzjq/results.json`: replaced current/legacy services, retained preferences and live PTY identity, rejected explicit endpoint replacement; script expectations now derive API from the packaged build instead of hardcoding a historical level. |

The active-Agent fixture is a deterministic interactive Python Agent on a real isolated Herdr PTY, not a new paid Codex/Claude run. Local HTTP/native acceptance establishes install/restart/recovery behavior; the actual GitHub Actions run, private authenticated release discovery end to end, an actual remote-machine upgrade run and public distribution signing remain unverified. Remote bridge deferral is verified at its production dispatch boundary in a regression test. Chinese IME was excluded at the user's request.

### Scoped quarantine preparation acceptance (2026-11-10)

`crates/updater/src/quarantine.rs` uses native macOS extended-attribute APIs through the existing lockfile's `xattr` dependency. `feed::validate_candidate` invokes it before `build-info`; `worker::prepare_locked` invokes it again on the final candidate copy before the install journal/handoff. Non-macOS preparation is a no-op. No shell command or system-wide setting is involved.

Workspace tests passed (185), including four new regressions using real attributes on temporary files: recursive selective removal/idempotence while preserving unrelated attributes, contents and executable permissions; refusal to follow a symlink outside the candidate; a real permission-denied removal and successful retry; and candidate-execution ordering, proving failed preparation does not execute the candidate or create an install journal. Build/packaging, workspace/all-target Clippy with warnings denied, formatting and diff checks passed. Logs: `/private/tmp/canban-quarantine-tests.log`, `/private/tmp/canban-quarantine-build.log` and `/private/tmp/canban-quarantine-clippy.log`.

The complete six-case updater matrix passed again with the new preparation path: `/private/tmp/cb-updates-tzxkxe6j`, log `/private/tmp/canban-quarantine-acceptance.log`. Native packaged-App acceptance `/private/tmp/cb-updates-60prst__/normal` exercised the real sidebar install/restart and restored the open Card/unsent comment; Settings showed `20261110-4` / Up to date. The initiating debug bundle contained the new preparation code; the versioned destination was the previously accepted release fixture. Tagged-file/error handling was verified by the new filesystem regressions, rather than claiming the normal HTTP fixture naturally acquired browser quarantine. Tests used isolated homes/Core/bundles and did not change the user's installed App attributes or system security settings.
