# Multi-machine execution

> [!NOTE]
> **Superseded architecture (legacy remote-Herdr removed):** Multi-machine orchestration now uses [Multi-host Canban daemons, Tailscale/SSH connections and Diagnostics](multi-host.md), where the desktop UI connects simultaneously to local and remote `canband` services (`HostRegistry` / `host_id` routing), each managing its own SQLite database, Workflow Engine, Scheduler, and local `herdr-core`.
>
> The legacy single-server remote-Herdr `WorkLocation` layer (`machine.*` RPCs, `remote_cli.py`, `owner_core_id`, `runtime_home`, `default_machine`, and `project.location`) has been removed (SQLite schema v22 / `DESKTOP_API` 78). Project directory and setting updates now use `project.update`.

See [Multi-host Canban](multi-host.md) for the current multi-machine protocol and [acceptance](multi-machine-acceptance.md) / [research](research/multi-machine.md) for historical records of the earlier design.

## Current authority and Project directory boundaries

Each `canband` / AppServer owns its own local SQLite database, workflow decisions, global Scheduler, and local `herdr-core` runtime:

- `canban serve` hosts the local service; a home lock prevents two serving instances.
- Explicit `CANBAN_ENDPOINT` attaches only; `app.shutdown` explicitly stops the service.
- Each Project stores a local `directory` (updated via `project.update`, with optional `expected_directory` conflict check, `name`, and `target_branch`).
- Updating a Project's `directory` affects newly admitted queued work and does not interrupt running Terminals/Attempts or retarget retained `WorkspaceBinding` checkouts (`directory` on `ManagedWorktree`, `WorkspaceBinding`, and `AgentRun`).
- `directory.list` browses directories on the connected `canband` host, and `terminal.create` runs on that host (optionally using `setup_argv` for controlled flows such as SSH `canband` bootstrapping or `gh auth login`).
