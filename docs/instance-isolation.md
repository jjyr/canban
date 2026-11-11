# Instance isolation and remaining gaps

Status: branch-scoped development management implemented on macOS, 2026-11-11.
This document supersedes the source-only audit of `e6eb1a8`; the current launch,
stop and configuration contract is [development startup](development-startup.md).

## Ownership rule

One Canban home owns one database, `app.sock`, `server.lock`, `server-id`,
configuration and update journal. Compatible UI clients may share its AppServer;
a second socket or application ID does not permit independent writers to the
same home. Use separate homes for concurrently developed versions.

`mise run dev` now derives a distinct home, bundle and native identity from the
canonical checkout and Git branch. `mise run dev-stop` stops that same instance;
`--branch` and `--all` cover instances left running after a branch switch.
An explicit development home overrides branch selection. Development commands
refuse installed `~/.canban` and unrecorded services rather than taking them over.
Home-scoped controller ownership also prevents different checkouts from managing
the same explicit home.

A generated Dev App carries its home binding in signed bundle resources. Its UI
and CLI resolve that home without needing the launcher's shell environment.
Normal installable `Canban.app` builds retain the existing `CANBAN_HOME` /
`~/.canban` selection. `CANBAN_ENDPOINT` selects an existing authority and its
data; it does not create an isolated instance. Bound Dev Apps reject that
endpoint override.

Sharing Herdr Core between development instances is acceptable for the current
scope. `hc_session` / `hc_env` remain separate configuration. Acceptance must
still use isolated Core fixtures; home isolation is not Core isolation.

## Audit disposition

The earlier audit identified eight gaps. These are their current dispositions:

| Gap | Current disposition |
| --- | --- |
| G1: development defaults to installed data | Addressed: automatic checkout/branch homes; `dev` and `run` share the isolated launcher; installed-home management is refused. |
| G2: Finder startup errors are invisible | Addressed: native startup error window, including API mismatch before the workspace opens. |
| G3: a new home still shares Core | Accepted boundary for ordinary development; acceptance keeps its own Core. |
| G4: shared application identity/bundle destination | Addressed for generated instances: unique bundle destinations and IDs, instance/home controller locks, UI process lock. |
| G5: generic connection lacks expected-instance/protocol negotiation | Partially addressed: managed dev startup checks PID/home/build/policy, bound UI/CLI enforce their home. Generic attached clients still need the remaining work below. |
| G6: update controls are dropped by the runner | Addressed: generated instances enforce disabled channel updates at AppServer; update fixture overrides are preserved instead of accidentally filtered. |
| G7: upgrade coordinates only its initiating UI | Still open for shared-service clients. |
| G8: external installation leaves an old service alive | Deferred by user decision, 2026-11-11: the existing in-app updater is the supported installed-version upgrade path; no automatic external-installation handoff is required now. |

## G5: generic attached-client compatibility

[AppClient](../crates/app-server/src/client.rs) still checks a minimum API level.
It does not negotiate an explicit compatibility range or bind an expected server
identity during generic attachment. Local build metadata can still report
`development` / `unknown`; it is not a unique source fingerprint.

Managed dev startup verifies its specific new PID/executable and bundled build,
which resolves the immediate routing problem. A future generic handshake should
use the existing persisted `server-id`, make expected managed-local identity
explicit, and define protocol compatibility. Remote attachment needs service
identity rather than equality between local and remote filesystem paths.
Compatible UI clients need not have identical builds.

## G7: multiple clients across a service upgrade

The [updater contract](updates.md) saves/restarts its initiating UI. Other clients
are not yet coordinated by that transaction. `AppClient::subscribe` reconnects
through `app.state` without repeating startup compatibility checks.

Remaining work is to prepare/preserve all participating clients, revalidate
identity/compatibility after reconnection, and prevent incompatible clients from
continuing operations. This belongs in the existing update coordinator rather
than independent service replacement by each window. Per-branch development
homes avoid cross-branch interference but do not implement this shared-service
upgrade protocol.

## G8: external installation does not perform the in-app service handoff

`AppClient::connect_or_start` starts its packaged service when ping fails. If an
older installed service still answers, Finder/Homebrew replacement does not by
itself replace that process. An incompatible UI now opens an error window; it
does not automatically take over that authority.

Scope decision, 2026-11-11: use the existing in-app updater for installed-version
upgrades. Its service shutdown, migration, readiness and recovery transaction
already covers that path. Automatic handoff after Finder/Homebrew replacement
or first activation of an older installation is deferred; it is not required
to complete branch isolation. No new runtime manager or versioned-runtime
storage is selected. Development commands deliberately do not serve as an
installed-instance repair command.

## Verification boundary

See [development verification](development-startup.md#verification) for executed
checks. The new multi-instance suite uses disposable checkouts, homes and Core,
and verifies that targeted stop/restart leaves other service PIDs and a real PTY
unchanged. Source auditing and successful isolated launches are not evidence of
an installed-home migration or multi-client update. The user's installed
AppServer is not restarted by these checks.
