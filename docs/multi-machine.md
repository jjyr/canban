# Multi-machine execution

> [!NOTE]
> **2026-11-15 architecture update:** User-facing multi-machine orchestration now centers on [Multi-host Canban daemons, Tailscale/SSH connections and Diagnostics](multi-host.md), where `Canban.app` connects simultaneously to local and remote `canband` services (each managing its own SQLite database, Workflow Engine, Scheduler and local `herdr-core` so workflows keep advancing while the client laptop is offline). The single-server remote-Herdr `WorkLocation` protocol documented below remains supported for compatibility.

Implementation: 2026-11-09. See [acceptance](multi-machine-acceptance.md) for
executed checks and remaining boundaries. The earlier [research](research/multi-machine.md)
records source inspection and the alternatives discussed before implementation.

## Authority and runtime

One AppServer owns the database, workflow decisions and global Scheduler. Native
UI windows and the CLI are clients. Closing a UI does not stop scheduling.
`canban serve` is the service host; desktop startup attaches to it or starts it
in a separate process. A home lock prevents two serving instances. Explicit
`CANBAN_ENDPOINT` attaches only; an unavailable explicit endpoint never creates
a replacement authority. `app.shutdown` explicitly stops the service.

```text
 Native UI A       Native UI B       local canban CLI
      \                |                  /
       +--------- AppServer RPC ---------+
                         |
              One Canban AppServer
              + SQLite WAL + Card files
              + Workflow Engine + Scheduler (global 8 by default)
                         |
              AppServer host's Herdr Core
                 /                   \
           local PTYs         named Core connections
                                     |
                              remote owning Core
                              + PTYs + Git + files
```

UI Terminal bytes also pass through AppServer. Its transparent `herdr.connect`
relay carries Core's native framed attach protocol. The renderer remains
libghostty; a UI needs no SSH/Core configuration for individual execution hosts.
The AppServer relay observes actual human activity for idle reclamation. Viewer attachment alone does not indefinitely protect an Agent; see [Terminal interaction](terminal-interaction.md).
The initial product-state client polls snapshots; a network push subscription
and service-manager installation are separate refinements.

A remote UI can use an SSH-forwarded Unix socket as `CANBAN_ENDPOINT`. There is
no unauthenticated TCP server, multi-authority database sync or machine pool.
"AppServer host" always identifies the service host, regardless of UI location.

## Project location and immutable executions

A Project has one current `WorkLocation { machine, directory }`. `machine` is
`local` or the destination Core's permanent ID. `directory` remains a compatibility
mirror for existing records and clients; `Project::work_location()` is the
canonical accessor. An alias is a display/route name, not execution identity.

```text
Project
  + Board / Notes / Cards / workflow
  + current location --------------------> machine B /repo/new
         |
         + new/ordinary queued work uses this at admission

Card
  + current WorkspaceBinding ------------> machine A /repo/original
  |     + actual worktree / branch / base commit / merge target
  + retained workspace history
  + Requests
       + queued: placement still follows Project
       + starting: location saved in the admission transaction
       + Attempt
           + immutable location / cwd / workspace / runtime home
           + gateway Core ID + owner Core ID + qualified Terminal address
           + provider session ID
```

Saving a location never moves code or ends an existing invocation. New execution
admission reads the Project and saves its location in the same transaction that
marks the Request starting. Resume/retained @profile questions pin the original
Attempt and restore its machine, cwd and provider session. Start fresh uses the
current Project location. Profiles retain portable command names; resolution
happens on the execution host. Explicit absolute paths remain explicit.

Review, commit/merge and cleanup use the Card's saved workspace source directory
and machine, even if Project settings have changed. Old workspaces remain in
history when a delivered Card starts work in another environment. A stage may
not silently review undelivered work from an unrelated checkout: it pauses for
a human handoff/acceptance. Archive checks all retained bindings, protects live
terminals, retains dirty worktrees and preserves commits before clean removal.

Registered but offline locations can be saved without verification. New work
waits for connection/preflight; another machine's runnable work can still be
admitted. An unavailable explicit Terminal target does not fall back. The global
Agent budget, workflow-depth priority and human gates apply across machines;
independent shells do not consume Agent slots.

## Settings and directory selection

- Settings → Machines shows the AppServer host and Core's connection catalog.
- Add takes a display name, SSH target and optional remote Core session. It opens
  an AppServer-host setup Terminal running `hc machine add`, preserving Herdr's
  login, host-key and installation prompts.
- Disconnect requires confirmation when references exist; remote programs,
  execution records and files remain. Existing Core `--via` connections also
  appear and can be used without inventing a second connection catalog.
- New Project and Board → Work location share the path/Choose component.
  New Project defaults to local with its machine picker collapsed; choosing a
  remote machine keeps that name in the collapsed summary. Changing machine
  clears the path. The single Choose action operates on the selected host.
  Native folder selection is used only for a directly launched local UI sharing
  the service filesystem; forwarded/explicit endpoints use server browsing.
- Default Terminal machine is configurable in Settings/`machine.default`.

## Terminals

`+` and Command+T inherit the selected Terminal's machine and observed live cwd.
With no selection they use the configured default machine and its home.
`+ ▾` explicitly selects a machine and opens that host's login shell in its home.
No Project or name is required. Manual titles remain `Terminal N`; machine names
are visible inline in the session list. Current directories are omitted there;
full context stays in the selected chrome/details. Directory association uses machine identity
as well as cwd. Known Card worktrees/branches can provide a presentation hint;
a manual shell never becomes a scheduled Card invocation.

Agent list entries show Card/stage, Provider/Profile with state, then machine.
Selected titles expand and unselected titles truncate, without a row metadata
popover. Full paths and execution provenance remain available in details and
Timeline. Stored remote addresses use `owner-Core-ID/terminal-ID`, so renaming
a route does not redirect existing terminal input to a different machine.

## Remote context and Agent CLI

Remote Unix execution requires Python 3 and the profile's programs. Git worktree
and Review actions require Git on that host. Long-running programs use Core
PTYs; bounded `machine.exec` is used only for short process/filesystem operations.
Truncated/timed-out command output is rejected rather than trusted as Git data.

```text
remote Agent -> thin canban CLI
                   |
             private RPC spool
             request key + JSON
                   |
         Core file/exec connection
                   |
          AppServer operation handler
             + DB/files/Timeline
                   |
          atomic reply via same Core
```

A per-authority `server-id` namespaces remote files under
`~/.canban/executors/<server-id>/`. Prompt manifests, previous submitted reports
and attachments are staged with target-correct paths. A small Python CLI reads
remote files/stdin and implements call, snapshot/status, report, bind-session,
reply and Note commands. It never hosts a product database. Calls travel through
a private file spool over the existing Core connection, without SSH reverse
forwarding or a public port. AppServer polls one pending call per cycle on each
machine. Request keys survive reconnect/publication retries; replies publish
atomically. UI disconnect does not interrupt this reporting path.

Remote CLI requests currently have a 4 MiB JSON limit and a 180-second reply
wait. A timeout explicitly reports that effects may already be recorded; inspect
before retrying and retain the request key. Large staged files transfer in
bounded chunks. Only submitted report/reply/Note content becomes authoritative
Canban history; arbitrary provider-local files are not automatically synchronized.

Product completion still comes from `canban report`. Runtime idle, blocked and
done remain distinct. Completion does not immediately exit the remote process;
verified idle/done and the configured inactivity deadline govern normal exit,
using AppServer-observed activity. Old provider turn metadata is not a substitute
for a current outcome or an idle deadline. The same lifecycle applies on local
and remote machines.

## Operations

| Method | Purpose |
| --- | --- |
| `app.state` | Snapshot, effective configuration and service home |
| `app.shutdown` | Explicit service shutdown |
| `machine.list` | Core catalog plus AppServer host |
| `machine.add` | Interactive setup Terminal on AppServer host |
| `machine.remove` | Disconnect a catalog connection, with reference confirmation |
| `machine.default` | Save default independent-Terminal machine |
| `directory.list` | Browse a selected machine's directory |
| `project.create` | Accept `location: {machine, directory}`; legacy directory means local |
| `project.location` | Edit future work location; optional expected-location conflict check |
| `terminal.create` | Explicit machine or inherited machine/live cwd |
| `herdr.connect` | Upgrade the local service connection to a Core byte relay |

Machine-specific connection instructions and deployment evidence belong in the
Machines inventory, not this repository.
