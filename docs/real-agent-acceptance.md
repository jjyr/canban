# Real Agent workflow and cross-Board acceptance

Verified 2026-11-06 against the first MVP runtime (commit 5b60d90), using the existing Codex executable/authentication and a separate Canban home and Herdr core. The earlier complete-loop acceptance used a deterministic fixture Agent; the earlier real Codex check covered context/reporting only. This run extends the evidence to actual development, review and archive across two Boards.

## Scenario and result

Two disposable Git Projects each started with a failing implementation and fixed test file. Each Project owns one Board and a Card numbered #1. The harness moved both Cards from Backlog into Develop before waiting for completion. Canban selected Codex and started separate real Agent processes through Herdr.

| Board | Actual development task | Development / Review terminals | Independent validation |
| --- | --- | --- | --- |
| Alpha | Implement exact greeting formatting with Unicode names | t1 / t3 | 2 tests passed |
| Beta | Implement bounded clamp and reject inverted bounds | t2 / t4 | 4 tests passed |

Both development Agents submitted completed through the actual bundled Canban CLI with Markdown report files. Each declaration automatically moved its Card to Review and scheduled a fresh real review Session. Both reviewers read the captured context and earlier development report, inspected the worktree and independently ran validation, then submitted their own completed reports.

The acceptance harness then checked the work and reports and invoked Archive for each Card. It verified the zstd packages existed, expanded report copies had been compacted, and report bodies read back identically from compressed storage. This is an explicit acceptance-harness archive action, not an automatic human-approval or merge feature.

Observed cross-Board overlap occurred during both development and review. All four Sessions had different owning-core terminal addresses, and each Card reused only its own worktree between stages. The two worktree paths were distinct. Source repositories and supplied tests remained unchanged; implementations were changed only in the task worktrees. Dirty worktrees were retained at Archive, following the current cleanup policy.

Captured manifests contained only the corresponding Project/Card and that Board's Note. Distinct Board marker strings appeared in their own development/review reports and did not appear in the other Board's reports. The shared database kept request, attempt, report and Timeline associations scoped by stable IDs. These checks demonstrate normal workflow/data isolation for this run; they do not establish a security boundary between same-user Agent processes.

Run result: passed, approximately 78 seconds. Core ID: 682677fc8d52411ea90c3e20bf6a086b. Detailed observations, worktree paths, validation output and archive references were retained at /private/tmp/cb-real-310tki54/evidence.json; that temporary location may be removed later. The durable evidence summary is this document, and the harness is reproducible from source.

## Reproduce

```sh
mise run acceptance-real
```

This opt-in task runs four actual Codex Sessions using the installed provider configuration. It creates isolated temporary Git Projects, a separate Canban AppServer/home and HC_SESSION/HC_SOCKET/state/config. It stops only its own service/core and retains the generated evidence and task worktrees for inspection. Default mise run acceptance remains the deterministic, model-free check. Do not run the real-model task as an implicit prerequisite for routine unit tests.

## Concurrency at the time of this run

Historical configuration for the recorded real-model run: all Boards shared one Scheduler. The then-default global capacity was 2 and each built-in profile's capacity was 1. This run changed only its isolated configuration to global capacity 2 and Codex profile capacity 2, so two independent Codex Sessions could overlap. The user's normal configuration was not changed.

- Two Boards using default Codex with the default Codex capacity 1 normally queue behind the same profile limit.
- Raising the Codex profile capacity to 2 allows separate Codex Sessions on both Boards concurrently, within the global limit.
- Routing different Cards to different profiles can also overlap within their respective profile/global limits. Cross-provider live acceptance, such as Codex plus Claude, was not performed in this run.
- Profile selection comes from tags/default configuration. There is no automatic requirement to assign different brands/models to different Boards and no opportunistic switch to another provider just because it is idle.

For Git Projects with a commit, each Card receives a managed worktree; stages of the same Card reuse that worktree. Non-Git/unborn Projects use their configured directory directly. If two Projects are configured to use the same non-isolated directory, their file writes can conflict. Shared ports, databases, repositories' administrative operations, user credentials and provider quotas also remain shared external resources. The current runtime does not promise exclusive leases or tenant/security isolation.

The recorded implementation counted logical work until the Agent's explicit outcome, without draining residual processes. Per-Project limits and automatic provider load balancing were not implemented. Capacity controls have since been removed by user direction; the historical real-model run remains evidence of independent Sessions, worktrees, reports and workflow progression.


## Subsequent simplification: no application concurrency limits

The user removed concurrency limits on 2026-11-06. Current Scheduler does not count running Sessions or gate new work by global/profile capacity. Config and saved-profile capacity fields are ignored for backwards compatibility. The real-model harness now uses normal unrestricted defaults; the evidence above predates that change and was not rerun as a paid-model check.

The model-free `mise run acceptance` regression deliberately supplies obsolete global/profile capacity=1, starts three same-profile Agents across three Boards, and holds them at a filesystem barrier. All three must reach the barrier with distinct terminal bindings before any can report completion. This verifies that the old limits no longer serialize Agents without invoking a model provider.


This regression passed on 2026-11-06 using `hc 0.1.18`, scratch home `/private/tmp/cb-59ulgvij/home`, and core `8b23d93c62532a118c1dd96bd45c63b2`. The same run passed Develop/Review, mentions/blocked notice, Archive/Restore and restart checks. Workspace tests passed (12 total). This follow-up used deterministic fixture Agents rather than additional real-model Sessions.

## 2026-11-07 interactive lifecycle acceptance

Run mise run acceptance-real-interactive explicitly for real model usage. Latest passing evidence is /private/tmp/cb-real-interactive-xf_mcgnu/evidence.json, provider thread 01a1b1d8-e587-7c92-bc99-b36e6ddcabcd. The harness created its own Note marker and Git repository, trusted only that owned disposable directory, bound an interactive Codex invocation, delivered a live @developer Question, verified its separate CLI reply, completed actual Unicode greeting code/tests, launched an independent Reviewer conversation, observed normal exits, resumed the exact Developer thread in a different terminal and archived readable replies. Source files outside the managed worktree remained unchanged.

The deterministic interactive harness additionally verifies interrupt/continue, repeated-open coalescing, idempotent terminal control intents, restart and archive/restore without using a model. The production native UI acceptance used /private/tmp/cb-terminal-ui-t4ppye3_ with a separate core. It verified real Codex TUI rendering/input, Unicode, selection/copy/paste, host restart/reattach and explicit completion/normal exit. Fixture-only UI operations also verified Timeline replies and exact UUID reuse after resume. These temporary local evidence paths are reproducibility aids, not checked-in datasets or portable provider-history backups.

Claude Code 1.0.108 smoke check: the production adapter launched its interactive TUI in the isolated native app and accepted the owned test-folder trust prompt. The model request returned Invalid API key / Please run /login. This is a concrete local authentication blocker: actual Claude report/Stop/resume acceptance is not claimed. Existing credentials/settings were not changed.
