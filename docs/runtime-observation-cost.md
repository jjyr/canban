# Runtime observation cost assessment

Historical baseline and prototype; see [implemented event observation](runtime-observation.md) for the replacement. The benchmark below intentionally reproduces the previous polling shape.

Measured locally on 2026-11-09 against isolated Herdr 0.1.18. This is an assessment and prototype, not a production polling change. Existing AppServer polling is not yet light enough to approve unchanged.

## Current call path

`AppServer::start` runs `tick()` then waits 500 ms; RPC and other work extend the actual period. `observe_terminals` reads status, lists all Terminals, resolves machine names and checks status again. Agent observation then reads status again and calls `terminal.get` for every tracked invocation. Every `Client::call` creates a fresh `hc connect` child, performs a handshake and creates a reply-reader thread.

For eight tracked invocations this duplicates the inventory through 13 RPC connections per round; for 32 it uses 37. Terminal results include launch arguments and other fields, so these are not small status-only packets. `terminal.list(all=true)` also transfers finished history. Independently, directory-context association runs Git/filesystem probes per Terminal, including repository roots and Card checkouts. Those probes are not included in the measurements below.

## Reproduction and measurements

Run `/usr/bin/python3 scripts/benchmark-runtime-poll.py`. It creates only its own `/private/tmp/cb-poll-cost-*` Core/socket/state/config and up to 32 synthetic sleeping Python PTYs, each with a 16 KiB argument representing a task prompt. It kills the test Terminals and stops its Core in cleanup. It does not inspect credentials or invoke Claude/Codex. It requires installed `hc`.

The benchmark mirrors the current status/inventory/per-invocation RPC sequence through `hc connect`; it does not run an AppServer, database, directory/Git context discovery, native rendering or a remote machine. Each case takes 20 samples. Client CPU includes the Python harness and waited-for `hc` children, and excludes Core/holder CPU.

| Case | Current median / P95 | Prototype median / P95 | Current / prototype hc children per round |
| --- | --- | --- | --- |
| 8 live Terminals | 122.82 / 163.66 ms | 10.04 / 10.65 ms | 13 / 1 |
| 32 live Terminals | 397.38 / 426.67 ms | 16.46 / 18.34 ms | 37 / 1 |
| 32 finished history entries, no live Terminal | 37.60 / 42.15 ms | 11.66 / 12.16 ms | 4 / 1 |

For eight live Terminals the measured client CPU per round was 111.56 ms versus 8.29 ms; for 32 it was 333.27 ms versus 13.61 ms. The empty-Core case is diagnostic only: a truly empty Canban home skips observation, whereas the benchmark deliberately issues the inventory sequence.

The prototype combines two changes: one connection for status/inventory/machines/status, and using that inventory instead of repeating individual `terminal.get` calls. It still lists all history and starts one connection child per sample. It is not a complete implementation and has not passed AppServer correctness or remote acceptance. It must not be used as proof of full application CPU, timing or memory behavior.

Raw evidence from this run: `/private/tmp/cb-poll-cost-6b8w635b/results.json`.

## Recommended production boundary

Keep periodic AppServer reconciliation, independent of UI list rendering and user clicks, but optimize its observation boundary before implementing the pending failure-classification update:

1. Fetch one runtime inventory per round and share it between Agent, independent-Terminal and Overview projections. Avoid reopening an RPC connection for every Agent.
2. Poll tracked active/uncertain bindings; stop routine history scans once exit and output capture are persisted. A Terminal disappearing from a live-only inventory needs an authoritative follow-up query before classification.
3. Keep status observation separate from Git/directory context discovery. Cache directory associations and refresh on directory changes, explicit refresh or a slower bounded interval for branch changes.
4. Use a simple approximately one-second normal cadence initially. On failure, back off per unavailable execution environment, retaining Unknown/Disconnected instead of fabricating an exit. Do not add SSH processes or direct provider probes to each status check.
5. Recheck the original machine/Core/Terminal identity and distinguish explicit missing-record replies from transport errors. Only actual state changes should produce events/notices or state writes; status freshness still needs bounded updates.
6. Accept the production implementation with active/finished/missing/replaced/unavailable scenarios, the actual full AppServer workload, and remote measurements before claiming those paths are light.

No user trust preferences, live Core, normal application data or Agent authentication were changed by this assessment.
