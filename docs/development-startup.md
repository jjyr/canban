# Development startup and service compatibility

Implemented and verified on macOS, 2026-11-09.

## Run the current development build

```sh
mise run run
```

The user selected the existing `~/.canban` home for development. Override it with an absolute `CANBAN_HOME` for disposable data. This command builds UI/CLI/updater, packages `.local/CanbanDev.app` (`com.canban.dev`), gracefully stops the selected home's AppServer through `app.shutdown`, waits for its service lock, starts the newly packaged CLI as AppServer, checks readiness/API/desktop utilities, then opens the development UI.

```text
mise run run
  -> build current UI + CLI
  -> close only the previously tracked development UI
  -> app.shutdown -> wait for the service lock
  -> start current canban serve -> verify its PID/API/utilities
  -> open current CanbanDev UI
              |
       same ~/.canban records
              |
        same Herdr Core / Agent PTYs
```

The ordinary `.local/Canban.app` retains its own bundle identity, so macOS does not redirect a development launch into an older ordinary window. The runner records a development UI PID, start time and exact command in a home-keyed file under `.local/dev`; another run closes that process only if its identity still matches. The ordinary app is not force-closed. UI drafts/tab selection remain process-local, as before.

Existing Core/terminal/provider bindings are recovered by the normal AppServer reconciliation. This does not migrate running worktrees or create replacement Agent sessions. Starting/uncertain launches retain the existing recovery policy. Closing or interrupting the development UI leaves AppServer running. Repeating the command uses the next build and replaces the managed development window/service again. Service effects already in progress are not forcibly killed; a shutdown/lock timeout fails the launch without deleting a socket or starting a second authority.

`CANBAN_ENDPOINT` remains attach-only. The runner refuses to replace an explicitly selected endpoint; update that service on its owning host, or unset the endpoint to manage a local home. The ordinary UI/CLI can still connect explicitly.

## API compatibility

Desktop API level 8 combines GitHub workspace/source Cards, durable editor drafts and internal release discovery/install with build identity. The three development branches had independently selected level 7; the merged contract uses level 8. It includes durable reminders (level 6), saved last-Project/partial preferences (level 5), Board ordering (level 4), Profile-only selection (level 3) and named themes (level 2). `app.ping` reports `api_version`, process `pid`, package `version`, `home` and release `build` identity. UI startup requires its desktop API before opening a workspace. An older service, even with the same `0.1.0` package version, produces a startup error pointing to the development launch command. The runner derives its required API from the just-packaged CLI's `build-info`, avoiding a second hardcoded API constant.

The development runner always replaces the service, even when its API/package version matches. PID verification establishes that it reached the process it just launched from the newly packaged executable. API level is a compatibility contract, not a source/build fingerprint; increase it when new desktop operations become required.

## Acceptance

```sh
mise run acceptance-dev
```

The regression suite creates a temporary home and isolated Core, opens one real shell PTY, changes Theme, and replaces the service. It checks that preferences, saved Core/terminal binding and the PTY process survive. It then replaces a simulated older ping/shutdown-only service with the current one and verifies desktop utilities work. An explicit endpoint is rejected without changing its running PID. No paid Agent is launched. The suite stops only its scratch AppServer/Core.

Executed evidence: `/private/tmp/cb-dev-start-b9m0ezgg/results.json` and `acceptance.log`. Workspace tests passed (116), including rejection of an old service with a matching package version, numeric branch/path collision handling and exact restoration of a legacy UUID worktree/branch. Workspace/all-target Clippy passed with warnings denied.

After the user selected reuse of existing data, `mise run run` was also executed twice against `~/.canban`. The second run closed the tracked development UI and started a new AppServer PID. Native Theme changed to Dark/Forest and remained selected after that repeated launch; System/Canban was restored afterward. Existing execution Core/terminal identities matched the pre-restart snapshot. Evidence: `/private/tmp/canban-before-dev-run.json` and `/private/tmp/canban-live-dev-results.json`. The development UI/service remain available for use.

## Readable workspaces

New Card #42 in directory `canban`, with Project ID beginning `4e5r`, uses branch `canban-4e5r/42` and `~/.canban/worktrees/canban-4e5r/42`. The Project display name and Card title do not affect these names. Normalization supports multilingual directory names. Conflicting branch/path names choose matching suffixes such as `42-2`; namespace conflicts choose `canban-4e5r-2/42`. Existing saved branches and UUID/numeric/readable directories remain unchanged, including Archive/Resume restoration. See [workspace naming and lifecycle](workspace-and-scheduling.md#readable-workspace-names).


## Update-aware startup verification (2026-11-10)

`mise run acceptance-dev` passed with desktop API 7: `/private/tmp/cb-dev-start-npf7kzjq/results.json`. It preserved Theme and the exact live PTY through current/legacy service replacement and rejected explicit endpoint takeover. Service shutdown is established by release of `server.lock`; a process can exit before its accept thread removes the stale socket path. New startup holds the service lock before opening SQLite. Internal upgrade fencing/recovery additionally runs before database access; see [updates](updates.md). This verification used an isolated home/Core, not the user's existing data.
