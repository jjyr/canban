# Development instances and startup

Status: implemented on macOS, 2026-11-11. Branch-scoped development startup
supersedes the earlier default reuse of the installed `~/.canban` home.

## Start and stop

Run these from the desired Git checkout:

```sh
mise run dev
mise run dev-stop
```

`mise run run` uses the same entry point as `dev`. Start builds the UI/CLI/updater
binaries and launches the selected instance in the background. Stop does not
build anything. Repeating start gracefully replaces that instance's UI and
AppServer, retaining its data. Repeating stop succeeds when it is already stopped.
Neither command stops Herdr Core or its Agent/shell PTYs.

By default the instance is selected by the canonical checkout path and current
Git branch. Branch names are sanitized for display and combined with a hash of
the original selection, so `feature/editor` and `feature-editor` do not collide.
Different checkouts/worktrees also get different application identities.

```text
<checkout>/.local/dev/<branch-label>-<hash>/
  instance.json             checkout, branch, home and application binding
  control.lock              serializes start/stop for this instance
  ui.json / server.json     PID, launch time, command and executable records
  ui.lock                   one UI process per instance
  CanbanDev.app/            unique signed bundle and macOS application ID
  home/
    canban.sqlite
    app.sock
    server.lock
    .dev-owner.json         checkout/controller ownership
    .dev-control.lock       coordinates controllers using this home
    config/
    server.log / ui.log
    updates/
```

Application display names include the branch. The generated bundle contains
`Contents/Resources/dev-instance.json`, binding both its UI and CLI to its home
and application identity. Opening that Dev App directly through Finder does not
require shell environment variables. A missing or relocated development binding
is rejected rather than falling back to the installed home; start the instance
again to regenerate the correct bundle. Normal `.local/Canban.app` builds remain
installation artifacts and are not branch-bound Dev Apps.

## Switching branches and selecting another home

Switching Git branches selects another instance; it does not stop the previous
branch. To stop it after switching, or stop all instances of this checkout:

```sh
mise run dev-stop -- --branch feature/editor
mise run dev-stop -- --all
```

`--all` enumerates only this checkout's recorded instances. It leaves other
checkouts and installed applications alone, and reports individual refusals
while attempting the other instances. It ignores the environment's home
selection; an explicit `--home` cannot be combined with `--all`.

For a custom development home, use the same selection for start and stop:

```sh
CANBAN_HOME=/absolute/path/dev-a mise run dev
CANBAN_HOME=/absolute/path/dev-a mise run dev-stop
# Equivalent explicit argument:
mise run dev -- --home /absolute/path/dev-a
mise run dev-stop -- --home /absolute/path/dev-a
```

An explicit home takes precedence over branch selection, so switching branches
while retaining that home selects/restarts the same instance. `--home` takes
precedence over `CANBAN_HOME`; `--branch` cannot be combined with either to avoid
ambiguous stop targets. Custom paths are expanded and normalized. The installed
`~/.canban`, including a symlink resolving to it, is refused by development
commands. They do not adopt an already running, unrecorded AppServer.

Detached HEAD uses `detached-<commit>` as its branch label. Renaming a branch or
moving a checkout changes its automatic instance selection; old data is retained,
never automatically moved/deleted. The macOS Unix socket path must fit its native
length limit; a long checkout path produces an actionable error asking for a
shorter explicit home.

## Ownership and shutdown

```text
select checkout + branch (or explicit home)
  -> acquire instance/home controller locks
  -> validate recorded processes and current socket owner
  -> ask this UI to quit normally and flush pending drafts
  -> app.shutdown(expected_pid) -> wait for server.lock release
  -> package the matching instance bundle
  -> start AppServer -> verify PID/home/build/update policy
  -> start UI -> wait for its rendered-frame readiness record
```

Stop performs the shutdown half only. Before signalling a UI or shutting down a
service, it checks the recorded process's launch time, complete command,
executable path and PID; it also verifies the responding AppServer's home/PID.
A stale/reused PID or an unrecorded service does not authorize termination.
`app.shutdown` checks the expected PID again at the receiver. A timeout reports
failure and does not force-kill a process or unlink a live socket. Successful
stop removes only process records; homes, configuration, databases and bundles
remain available. A Finder-launched bound UI/AppServer writes the same records,
so it is stoppable through the development command too.

The desktop translates SIGTERM/SIGINT into normal AppKit quit, returns from its
native loop and flushes the draft writer before releasing its process record.
AppServer shutdown remains RPC-driven. No global process-name kill is used.

`CANBAN_ENDPOINT` is attach-only: development management commands reject it.
An ordinary UI/CLI may explicitly attach to an existing endpoint, but an
unrelated local home variable does not isolate the remote service's data.
A bound Dev App refuses an endpoint override that would escape its instance.

## Updates, compatibility and startup failures

Generated development bundles disable channel release discovery and installation
in AppServer, including manual check/install RPCs. Settings explains this policy
and disables the update action. Rebuild with `mise run dev` to update the dev
instance. Intentional `CANBAN_UPDATE_*` fixture settings are preserved, while
inherited Agent/session/endpoint bindings are removed from child environments.
Ordinary installed Apps use the [backend activation and daily release-check rules](updates.md).

Managed startup verifies the newly launched server PID, canonical home, complete
build metadata and development update policy. Packaged UI/CLI/updater ship
together. Desktop API compatibility remains a separate contract; the generic
attached client orders published releases or falls back to API levels. It does
not negotiate a compatibility range.

Startup failures now open a standalone native error window with the selected
home and the actual connection/configuration error. This works before AppServer
connection and does not require opening SQLite. The Gallery has a
`CANBAN_GALLERY_SCENE=startup-error` scene using the production surface.

Starting an isolated dev instance does not upgrade or repair an older service
already running for installed `~/.canban`. Ordinary local Apps now offer
**Update backend** for an older service, using their installed binaries without
downloading. A newer service permits entry with a newer-App notice. This action
is unavailable to bound Dev Apps and explicit endpoints. Multi-client upgrade coordination remains tracked in
[instance isolation and remaining gaps](instance-isolation.md).

Herdr Core may be shared by ordinary development instances for now. It is chosen
through `hc_session` and `hc_env` in each home's `config/agents.json`; a new Canban
home does not itself isolate Core. Acceptance still configures a separate Core
before launching fixtures and never touches the user's live runtime.

## Verification

```sh
mise run test-dev
mise run test-package
mise run acceptance-dev
# Optional native inspection; follow the printed continue-marker instructions:
mise run acceptance-dev -- --native
```

Selection tests cover checkout/branch identity, sanitization collisions,
detached HEAD, explicit homes, installed-home refusal, socket path limits and
child environment policy. Packaging tests use real Mach-O fixtures and signature
verification, including branch-bound Dev App assembly.

The acceptance suite creates disposable Git checkouts and isolated homes/Core.
It exercises concurrent branches and checkouts, repeated restart/stop, targeted
branch stop, checkout-scoped `--all`, a home override after switching branches,
refusal of foreign/reused process records and unrecorded services, disabled
updates, and exact PTY/configuration preservation. It starts no paid Agents.
Native mode additionally opens separate real Dev Apps and stops their UI
processes through the same controller.

Executed 2026-11-11 from the uncommitted tree based on `e6eb1a8`: build, 208 Rust
tests, workspace/all-target Clippy with warnings denied, three selection tests,
three packaging tests and standard acceptance passed. The expanded native
matrix passed at `/private/tmp/cbd-h3c52zul/results.json`, including simultaneous
start serialization, foreign-checkout home refusal and Finder reopening after
both UI and AppServer stopped. Earlier native inspection at
`/private/tmp/cbd-8t9i7r62` verified separate main/feature windows, each home in
Settings, disabled channel updates and reopening without a shell home override.
`/private/tmp/cb-startup-9q9nwd5g/results.json` records the real packaged
API-4/API-8 failure window: only ping requests reached the disposable service,
SQLite was never opened, and the Quit control closed the window. Production
Gallery scenes `startup-error` and `updates-disabled` were inspected natively.
Actual `mise run dev -- --no-ui` / `dev-stop` commands also passed against an
explicit disposable home; all fixture services/Core were stopped. Logs are
`/private/tmp/canban-branch-dev-*.log`. No installed-home handoff, paid Agent run
or multi-client channel upgrade was performed.

Historical 2026-11-09/10 single-home handoff acceptance verified explicit live-data
reuse and PTY preservation under the earlier launcher. Those runs are not the
current default, nor proof of current process state. Their recorded evidence
remains `/private/tmp/cb-dev-start-b9m0ezgg`,
`/private/tmp/cb-dev-start-npf7kzjq`, `/private/tmp/canban-before-dev-run.json` and
`/private/tmp/canban-live-dev-results.json`.
