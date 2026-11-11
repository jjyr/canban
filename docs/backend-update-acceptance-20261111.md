# Local backend activation and daily checks acceptance

Executed 2026-11-11 on macOS arm64, from an uncommitted working tree based on
`c89e906` (schema 10 / desktop API 8). These are disposable installed-style
bundles and homes with isolated Herdr Cores; the user's installed App, live
`~/.canban` and Core were not changed. The current contract is [updates](updates.md).

## Executed checks

| Check | Result and evidence |
| --- | --- |
| Build and workspace | `mise run build`, 211 Rust tests, workspace/all-target Clippy with warnings denied passed. Logs: `/private/tmp/canban-backend-update-tests.log`, `/private/tmp/canban-backend-update-clippy.log`. One earlier concurrent test run had three transient service-lock acquisition failures on immediate reopen; the complete rerun passed. |
| Local activation matrix | `mise run acceptance-backend`; `/private/tmp/cbb-5ct2evv4`, log `/private/tmp/canban-backend-update-acceptance.log`. Same schema: zero migration CLI calls and no snapshot. Schema 8 and legacy 7: one migration each. Injected migration and candidate-UI failures: original schema/data restored, startup fenced, explicit recovery succeeded. Every case checked zero HTTP requests/downloads, unchanged App-bundle hashes and the same real Core PTY PID. |
| Daily scheduling | Same matrix: initial automatic check, no second check after restart within 24 hours, interrupted persisted check becomes inspectable without a request, manual checks bypass the interval, failed attempts remain throttled, and an overdue restart makes exactly one request. |
| Actual Update backend button | `mise run acceptance-backend -- --native`; `/private/tmp/cbb-wb8ppm8k/native/result.json`, log `/private/tmp/canban-backend-update-native.log`. The packaged UI connected to a fixture API-4 service holding the service lock. Clicked **Update backend**, observed its disabled pending state, then automatic restart into the normal Board workspace. The journal reached complete; zero download/migration, no snapshot, bundle unchanged and real PTY preserved. This exercises production preparation and the detached worker, rather than constructing only a journal. |
| Newer backend entry | `mise run acceptance-backend -- --newer-native`; `/private/tmp/cbb-cy6lr_gd/newer/result.json`, log `/private/tmp/canban-backend-update-newer-native.log`. A socket proxy changed only the real service's ping API to 9. The actual packaged UI entered Board normally, displayed the newer-App notice above utilities, and created no handoff journal. |
| Production Gallery | `CANBAN_GALLERY_SCENE=backend-older mise run gallery`: clicked fixture Update backend, observed the error and enabled retry button. `backend-newer`: normal Board composition with the production newer-App notice. Both task-owned Gallery processes were closed. |
| Existing application/dev flows | `mise run acceptance` passed at `/private/tmp/cb-1jok_0p8`; `mise run acceptance-dev` passed at `/private/tmp/cbd-2det0vh_`. Logs: `/private/tmp/canban-backend-update-standard.log`, `/private/tmp/canban-backend-update-dev.log`. |

## Reproduction and limits

```sh
mise run acceptance-backend
mise run acceptance-backend -- --native
mise run acceptance-backend -- --newer-native
```

Native modes print their fixture root and wait for inspection. In the first,
click **Update backend** and inspect the restarted workspace; in the second,
inspect normal entry and the newer-App notice. Create `<printed-root>/native-done`
after inspection so the harness checks its assertions and cleans up the fixture.

The old-service fixture implements the legacy ping/shutdown boundary, rather
than shipping a historical binary. The newer-service proxy verifies the current
client's entry policy against a compatible payload; it cannot prove compatibility
with unknown future protocol changes. No paid Agent/provider run, actual
Homebrew installation, GitHub publication, multi-client coordination or new
Developer ID/notarization acceptance is claimed. The existing six-case release
download/bundle-swap matrix was not rerun for this change; its earlier evidence
remains in [updates](updates.md) and [combined integration](merge-integration-20261111.md).
