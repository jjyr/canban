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
| G2: Finder startup errors are invisible | Addressed: native startup error window, including the older-backend update prompt before the workspace opens. |
| G3: a new home still shares Core | Accepted boundary for ordinary development; acceptance keeps its own Core. |
| G4: shared application identity/bundle destination | Addressed for generated instances: unique bundle destinations and IDs, instance/home controller locks, UI process lock. |
| G5: generic connection lacks expected-instance/protocol negotiation | Partially addressed: managed dev startup checks PID/home/build/policy, bound UI/CLI enforce their home. Startup now orders release/API versions; expected generic identity and negotiated compatibility remain open. |
| G6: update controls are dropped by the runner | Addressed: generated instances enforce disabled channel updates at AppServer; update fixture overrides are preserved instead of accidentally filtered. |
| G7: upgrade coordinates only its initiating UI | Still open for shared-service clients. |
| G8: external installation leaves an old service alive | Addressed by explicit Update backend for ordinary local Apps; reuses installed binaries, migrates only pending schema, and never downloads or downgrades. Unattended takeover remains out of scope. |

## G5: generic attached-client compatibility

[AppClient](../crates/app-server/src/client.rs) compares published release versions,
falling back to desktop API for legacy/unversioned builds. An older backend
blocks entry with an update prompt; a newer backend allows entry with a newer-App
notice. It does not negotiate an explicit compatibility range or bind an expected
server identity during generic attachment. Local build metadata can still report
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

## G8: explicit activation after external installation

Finder/Homebrew replacement can leave the old AppServer answering. Ordinary
local Apps now detect an older backend and offer **Update backend**, using the
installed App's binaries through the existing updater locks/journal/readiness.
The actual database schema decides whether migration is necessary; equal schema
skips it. No download or bundle replacement occurs. A newer backend continues
normally with a newer-App notice, without downgrade. See [updates](updates.md)
for activation/recovery and the daily release-check contract.

This 2026-11-11 decision supersedes the earlier deferral of local activation.
No unattended takeover, new runtime manager or versioned-runtime store is added.
Explicit remote endpoints require updating on the owning host. Development
commands remain isolated and do not repair an installed instance.

## Verification boundary

See [development verification](development-startup.md#verification) for executed
checks. The new multi-instance suite uses disposable checkouts, homes and Core,
and verifies that targeted stop/restart leaves other service PIDs and a real PTY
unchanged. The [backend acceptance](backend-update-acceptance-20261111.md) separately verifies
local activation with disposable installed-style bundles and homes. These runs
do not establish multi-client update coordination. The user's installed
AppServer is not restarted by these checks.
