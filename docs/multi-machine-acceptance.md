# Multi-machine implementation acceptance

Date: 2026-11-09. Baseline: `59ea696`, with implementation changes still
uncommitted. Tests used disposable Canban homes and isolated Core sockets,
sessions, state and configuration. No user's active Canban database or default
Core was used. Machine-specific connection/setup notes live in the Machines
inventory.

## Executed checks

| Check | Result |
| --- | --- |
| `mise run test` | 88 Rust tests passed |
| Workspace Clippy, all targets, warnings denied | Passed |
| `mise run build` | Native app and CLI packaged |
| `mise run build-gallery` | Packaged; native work-location picker checked at narrow width with connected/offline fixtures |
| `mise run acceptance` | Real local Core workflow, blocked outcome, archive/restore/WAL, restart and cross-Board global budget passed |
| `scripts/acceptance-machines.py` | Real remote Core PTY workflow passed; result markers below |
| Native UI via CUA | Settings, work location, remote browse, path reset, remote Agent input and independent remote shell verified |
| Desktop starts its own AppServer | Fresh isolated desktop created the service and connected |
| Quit UI while an Agent waits at a barrier | Service still answered; releasing the barrier completed Develop and scheduled/completed Review with no UI |
| `git diff --check` | Passed |

Both local and remote Core binaries were version 0.1.18. The Linux x86_64 binary
came from the official vendor release and was verified against its published
SHA-256 before use. Remote interactive Agents were deterministic Python
fixtures with real PTYs, runtime status, persisted provider-history files and
actual Canban CLI calls. This establishes transport/workflow behavior, not a
paid-model quality or authentication result.

The remote acceptance emitted:

```text
REMOTE_SHELL_INHERIT_OK
REMOTE_WORKFLOW_BLOCK_NOTE_REVIEW_MERGE_OK
REMOTE_ORIGINAL_ENV_RESUME_AND_ROUTE_RENAME_OK
REMOTE_PTY_AND_CLI_RESTART_OK
REMOTE_DISCONNECT_RECONNECT_IDENTITY_OK
REMOTE_ARCHIVE_OK
MULTI_MACHINE_ACCEPTANCE_PASSED
```

It created a remote source repository and separate Card branch/worktree, sent
runtime permission and product-blocked signals, updated a database Note through
the remote CLI, saved deliverable source changes, generated a review report,
and committed/merged the actual remote worktree. Changing the Project to a local
repository did not redirect Review/merge. Exact Resume restored the original
remote cwd and provider session. Renaming and removing/re-adding the Core route
preserved the saved owner-qualified Terminal address. Restarting AppServer
preserved that PTY and delivered a remote CLI write queued across restart.
Archive retained its compressed, readable product package.

Native checks selected a remote machine in the Terminal menu and opened a shell
in its home without a Project requirement. Typing through libghostty produced
`NATIVE_REMOTE_SHELL_OK` and the remote `pwd`. Another native attachment accepted
`hello-native-remote` in the resumed Agent. The work-location browser listed a
real remote repository; switching to AppServer host cleared its former path and
disabled Save until a new directory was selected.

## Defects found and repaired

- Profile loading originally resolved command names into local absolute paths.
  Remote execution now resolves the original command on its actual host.
- Automatic exit originally compared remote Core timestamps with AppServer's
  clock. Remote reporting now samples the same Terminal's current root turn
  before acknowledging completion and waits for a later done turn.
- A stale Terminal observation could overwrite a new launch receipt with an old
  starting row. Persistence now checks that the observed runtime is still current.
- Per-terminal directory matching issued many remote filesystem/Git operations.
  Candidate paths are now probed in one remote operation and matched using the
  same Rust algorithm as local context.
- Provider session uniqueness now includes its owning machine. Two providers on
  different hosts may legitimately use the same opaque session ID.

Rust regressions cover location changes before admission, original-location
Resume, handoff gates for undelivered work, one AppServer owner, explicit
shutdown, remote clock skew, offline work not starving local Boards, no silent
Terminal fallback, same-path host isolation and provider session ownership.

## Reproduction

Build the app/CLI and prepare a disposable AppServer home with an isolated Core.
Connect that Core to an isolated remote Core using the normal Herdr catalog.
Then run:

```sh
python3 scripts/acceptance-machines.py \
  --home /absolute/path/to/disposable/home \
  --machine OWNER_CORE_ID_OR_CATALOG_NAME \
  --fixture-root /absolute/remote/disposable/directory
```

The harness modifies the scratch profile configuration, creates test repositories,
restarts the scratch AppServer and renames/removes/reconnects its machine record.
It requires Python 3 and Git on the target. It preserves result metadata under
the Canban scratch root. Stop the test AppServer and both test Cores when done;
remote Core shutdown must be explicit because disconnect alone preserves PTYs.

## Boundaries

- Real Codex/Claude provider authentication, paid-model task quality and
  provider-specific history recovery were not rerun by this remote fixture test.
- Native keyboard/Terminal interaction was checked; Chinese IME was skipped as
  requested. Long-output/focus behavior otherwise retains its existing acceptance.
- The native UI used local AppServer sockets, including explicit client attachment.
  A second physical UI computer and SSH-forwarded UI socket were not separately
  exercised. Native opening/materialization of files on such a UI still needs
  acceptance before claiming a complete remote-desktop client experience.
- First remote workspace implementation targets Unix hosts with Python 3/Git.
  Core's Windows support does not establish Canban Windows workspace support.
- Remote CLI JSON requests are limited to 4 MiB, with a 180-second reply wait.
  No arbitrary provider-home synchronization, machine pool, multiple simultaneous
  Project locations, public TCP deployment or service-manager installation is claimed.
- Test artifacts/logs were retained for inspection; task-owned services and PTYs
  were stopped after acceptance. Dirty or uncertain worktrees are retained rather
  than forcibly deleted.
