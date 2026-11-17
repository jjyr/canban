# Agent RPC/CLI acceptance, 2026-11-17

Implemented on the Card 111 branch based on
`c0470ad4730da82e1e2f51e854d548103976b660`. Desktop API changes from 64 to 65;
SQLite schema remains 20. The [response contract](agent-response-contract.md)
defines the new defaults and explicit field/full requests. This is a response
contract change, not an old-default compatibility layer.

## Verification

- `mise run build`: packaged `.local/Canban.app` successfully.
- `mise run test -- --test-threads 4`: 601 tests passed, one ignored test skipped.
  The first unrestricted run hit the existing background Git-push-start deadline
  while other builds/fixtures were active. The complete bounded-concurrency run
  passed, including that test. Its deadline was not relaxed.
- `mise run acceptance`: passed the real isolated Herdr/PTy/CLI workflow,
  mentions/blockage, archive/restore/restart and global-concurrency checks.
- Response regression cases cover default/full/selected views, pre-write field
  errors, old operations-record replay, presentation-independent request keys,
  revision conflicts, Card cursor scope/staleness, empty/multiple filtered pages,
  Timeline progress and unchanged stored comment fields.
- Native acceptance used a copy of the packaged app with home
  `/private/tmp/cb-native-yiqn0baf/home` and its own Core/socket/config/state.
  Through actual native controls, created Card 14, observed its full description,
  posted one comment, edited it, and moved Backlog to Develop. The edited comment
  and stage appeared in the UI and matched a subsequent explicit-endpoint RPC
  query. The disposable app/Core were then stopped. No live installed home/Core
  was used for those writes.
- `canban skill` matched both the repository skill and the new isolated home's
  materialized skill. This does not assert that a separately installed global
  skill copy or the installed application has already been upgraded.

The native harness was updated to seed isolation before its first daemon launch
and to consume the current Terminal/Agent snapshot shape. Gallery was not needed:
no component geometry/style contract changed. Native acceptance exercised the
affected desktop consumers instead. Live paid-provider acceptance was not run.

## Synthetic operation comparison

`scripts/acceptance-agent-interface.py` starts disposable services and uses a Unix
socket proxy to capture actual requests/replies for actual CLI invocations. The
fixture contains 20 local Cards, 14 review comments and two stage moves. A
manual-only workflow and disabled external runtime prevent Agent scheduling.

The baseline is the installed development CLI reporting API 64, with binary
SHA-256 `c8879901504c6394b5b87df8d546436335e4b659a66135df973512fb374a3a3d`.
It reports commit `unknown`, so this evidence does not attribute it to an exact
source commit. The compared implementation reports API 65. Both build identities
and binary hashes are retained with the measurements.

| Measurement | API 64, same 62 operations | API 65, same 62 operations | API 65, revised skill strategy |
| --- | ---: | ---: | ---: |
| CLI invocations / RPC calls | 62 | 62 | 34 |
| Serialized business parameter bytes | 38,729 | 38,729 | 38,315 |
| RPC request bytes, including envelope/newline | 42,833 | 42,833 | 40,499 |
| RPC response bytes, including envelope/newline | 424,890 | 49,763 | 7,524 |
| CLI stdout bytes, including formatting/newline | 438,572 | 47,655 | 6,368 |
| Locally selected candidate context bytes | 40,480 | 40,480 | 3,650 |

With the same operations, RPC response bytes decreased 88.3% and CLI stdout
bytes decreased 89.1%. The revised strategy eliminates 14 unnecessary Card reads
and 14 full comment readbacks, retaining revision reads for the two moves. Its
initial discussion checks filter comment kinds and save incremental progress.

These are synthetic measured UTF-8 bytes, not a recreation of the historical
private Card bodies or a claim to reproduce the original 479,662-byte result.
No tokenizer/model API was invoked. Candidate context files are the deterministic
local selection a caller *could* submit; they were not all sent to this Agent.
The actual analysis context received aggregate metrics, build identities and
selected validation results. The unchanged 40,480-byte candidate column in the
same-call comparison illustrates why raw RPC savings alone do not establish
model-token savings when a caller already filters locally. Total model-token
consumption for a real provider task remains unmeasured.

Per-call parameters, raw RPC replies, CLI stdout, candidate-context payloads,
metrics JSONL and totals are retained outside Git under:

`/Users/jjy/.canban/projects/0afddd43/cards/111/agents/codex/interface-acceptance/`

Native proof is retained beside that directory as `native-interface.png` and
`native-interface-result.json`.

Reproduce the current-contract checks with
`mise run acceptance-agent-interface`. Add
`-- --baseline-cli /absolute/path/to/a/pre-change/canban` for a before/after
comparison. Do not point the fixture at an installed application home or live
Core.

## Limits and migration

Fields currently select top-level domain properties. Card pagination scans
matching IDs/revisions to validate its generation; it avoids decoding all bodies
but is not constant-time in project size. Concurrent changes invalidate a cursor
instead of maintaining server-side historical snapshots. Unknown/unprojected
methods retain their method-specific result, and full desktop state/durable
event consumers retain their existing payloads.

Owned acceptance callers that inspect whole objects now explicitly request full
views. External callers and remote daemons must migrate with the release; this
branch does not install or publish the new app automatically.
