---
name: canban
description: "Operate Canban Cards, workflows, comments, reviews, and execution outcomes through the injected CANBAN_CLI."
---

# Canban CLI Skill

Canban injects `CANBAN_CLI` and task environment variables when launching an Agent session on a Card or Project. Use `"$CANBAN_CLI"` for all Canban mutations and queries.

## Guardrails

1. **Check environment first**: Before running Card-scoped commands, verify Canban injected a live session:
   ```sh
   test -n "${CANBAN_CLI:-}" && test -n "${CANBAN_ENDPOINT:-}"
   ```
   If unset, you are outside a Canban-launched session (or pass `--home` / `--endpoint` explicitly when the user asks).
2. **Always use current environment variables**: Use `"$CANBAN_CLI"`, `"$CANBAN_ENDPOINT"`, `"$CANBAN_CARD"`, `"$CANBAN_PROJECT"`, and `"$CANBAN_RUN_DIR"` from the current process environment. Never copy stale `CANBAN_CARD` or `CANBAN_PROJECT` values from earlier conversation turns.
3. **Never touch storage directly**: Never read or edit `canban.sqlite` directly, and do not archive Cards unless explicitly asked.
4. **Keep scratch and Markdown payloads outside Git**: Write report, reply, and Card body files under `"${CANBAN_RUN_DIR:-/tmp}"` (for example `"$CANBAN_RUN_DIR/report.md"`), never inside the repository worktree.

## Injected Environment

| Variable | Purpose |
| --- | --- |
| `CANBAN_CLI` | Absolute path to the matching `canban` executable (or remote bridge wrapper). |
| `CANBAN_ENDPOINT` | AppServer Unix socket path (or `spool:...` on remote execution hosts). |
| `CANBAN_HOME` | Active Canban application home directory. |
| `CANBAN_CARD` | Short Card number (for example `42`), injected into both scheduled Agent runs and manual Card Terminals. |
| `CARD_ID` | Internal Card ID, injected alongside `CANBAN_CARD` in both scheduled Agent runs and manual Card Terminals. |
| `CANBAN_PROJECT` | Active Project ID (CLI commands also infer the Project from a git repository or Card worktree CWD when omitted). |
| `CANBAN_RUN_DIR` | Scratch directory outside the repository worktree for Markdown payloads. |
| `CANBAN_PROFILE_ID` | Active `@identity` or Agent profile ID (when applicable). |
| `CANBAN_SCHEDULE_RUN_ID` | Scheduled run ID (`scheduled:<project_id>:<short_id>`), injected into Cardless scheduled Terminal Agent runs. |
| `CANBAN_SCHEDULE_ID` | Parent schedule state ID (`schedule:<project_id>:<workflow_id>:<rule_id>`), injected into Cardless scheduled runs. |

## Session Behavior

- **Discussion vs. implementation**: In interactive sessions, `@mention` runs, and manual Card terminals, edit repository files only when asked to implement. If the Card is not yet in `develop`, move it to `develop` in this session first.
- **Card updates**: Update the Card title or body only with user-confirmed design points or when asked.
- **Reporting**: In interactive sessions, reply in the terminal during discussion and run `report` only when a requested task finishes. In dedicated `@mention` assignments, publish and finish in one `report` call (no separate `reply`).

## Core Execution Commands

### Report task or comment outcome

```sh
"$CANBAN_CLI" report \
  --state <completed|backlog|blocked|failed|working> \
  --message 'Concise one-line summary' \
  [--file "$CANBAN_RUN_DIR/report.md"]
```

- `completed`: Deliverable work or dedicated `@mention` comment task is finished. For code tasks, stage and `git commit` deliverable changes on the Card branch before reporting `completed` (do not `git push` unless instructed).
- `blocked`: Waiting on human clarification, credentials, or a design decision.
- `backlog`: Card is already implemented, obsolete, or not actionable right now; returns the Card to Backlog with your explanation.
- `failed`: Unrecoverable execution error.
- `working`: Optional interim progress update without finishing the assignment.

### Report batch merge queue outcome (`batch report`)

When running in a project `_merge_queue` batch `@merger` session (`CANBAN_BATCH_ID` is injected):

```sh
"$CANBAN_CLI" batch report \
  --state <completed|blocked|failed|working> \
  --message 'Concise summary of batch resolution' \
  [--evict <CARD_NUMBER> ...] \
  [--file "$CANBAN_RUN_DIR/report.md"]
```

- Do not `git push` or modify the main checkout directly; Canban fast-forwards and pushes once on `completed`.
- Use repeatable `--evict <CARD_NUMBER>` only if you rebuilt `_merge_queue` cleanly from `refs/canban/batch/<short_id>/target` without those broken Card(s) and verified tests on the remaining Cards.

### Answer a live follow-up question (`reply`)

When a human posts a follow-up question with a Question ID against an already running non-comment task, answer that question without ending your main task:

```sh
"$CANBAN_CLI" reply --to <QUESTION_ID> --file "$CANBAN_RUN_DIR/reply.md"
```

### Inspect or update the current Card (`card show` / `card update`)

Read the current Card summary (uses `CANBAN_CARD` + `CANBAN_PROJECT` automatically, or pass `--card <NUMBER>`):

```sh
"$CANBAN_CLI" card show
# Request task text explicitly when it is not already in the session context.
"$CANBAN_CLI" card show --fields number,title,body,stage,revision
```

Update the Card title or Markdown body:

```sh
"$CANBAN_CLI" card update [--title 'Updated title'] --file "$CANBAN_RUN_DIR/card.md"
```

### Adopt a workflow stage (`card adopt`)

Move the current Card to a stage (default: `develop`) and bind its workflow to the current session without spawning a duplicate scheduled run:

```sh
"$CANBAN_CLI" card adopt [--stage develop]
```

### Stage local diff review comments (`review draft`)

When reviewing a Card or GitHub PR, write line-specific findings and an overall summary as a **local unpublished review draft** (never publish directly to GitHub) so a human can inspect, edit, or publish them from the local Diff UI:

```sh
"$CANBAN_CLI" review draft --file "$CANBAN_RUN_DIR/review_draft.json"
```

Expected JSON schema for `review_draft.json`:

```json
{
  "summary": "Overall review summary...",
  "verdict": "comment",
  "comments": [
    {
      "path": "crates/scheduler/src/lib.rs",
      "line": 86,
      "side": "RIGHT",
      "snippet": "let _ = self.reconcile_slots();",
      "body": "Explain the issue and suggested fix..."
    }
  ]
}
```

- `verdict`: `"comment"` (default), `"approve"`, or `"request_changes"`.
- `side`: `"RIGHT"` (added or context line on the new file) or `"LEFT"` (deleted line on the old file).
- Use `"$CANBAN_CLI" review clear` to discard the local review draft for the current Card if needed.

## Efficient RPC and CLI use

- RPC and CLI default to compact Agent-facing summaries and mutation receipts.
  `--human` pretty-prints the same JSON; it does not expand content.
- Use `--fields number,title,body,stage,revision` (RPC `fields` array) when task
  text is needed. Use `--full` (RPC `view:"full"`) only for a complete business
  view. Fields/full are mutually exclusive; fields are explicit top-level names.
- Reuse injected task text and existing query data. If the needed fields are
  already known, request them together rather than summary then detail.
- Inventory with `card.list` and a stage filter. Read `items`, follow every
  `next_cursor` to null, and handle `cursor_stale` by restarting and deduplicating
  IDs. The default limit is 50; the maximum is 500. Do not routinely send full
  project lists or snapshots into model context.
- Check discussions with `timeline.list`, `kinds:["comment.created","comment.updated"]`
  and `after_sequence`. Follow next_cursor, then save last_sequence even after
  an empty filtered result. Comment read text appears once as `body`.
- `event.list` instead filters after paging: continue its global cursor to
  highwater even after an empty page. Do not confuse the two cursor contracts.
- Accepted write receipts normally suffice; do not read back full objects just
  to confirm the write. Verify business outcomes or ambiguous/queued results
  specifically. Preserve revision checks on concurrent writes.
- Write long payloads through `call METHOD @params.json`, explicitly set comment
  `mentions:[]` unless dispatch is intended, and reuse a stable request_key when
  retrying. Changing only fields/full does not change the write's identity.
- `canban skill` is the current binary's bundled skill; refresh installed copies
  when upgrading. Old RPC defaults are not a compatibility contract.

## Inspecting State & Calling AppServer RPCs

Inspect overall state or runtime health:

```sh
"$CANBAN_CLI" snapshot
"$CANBAN_CLI" status
"$CANBAN_CLI" --help
```

Call any public AppServer JSON-RPC method via `"$CANBAN_CLI" call <METHOD> '<JSON>'` (or `@/path/to/params.json`):

```sh
# List Projects or Cards
"$CANBAN_CLI" call project.list '{}'
"$CANBAN_CLI" call card.list '{"project_id":"<PROJECT_ID>","stage":"backlog","limit":50}'

# Read a Card, its Timeline events, or Diff preview
"$CANBAN_CLI" call card.get '{"card":<CARD_NUMBER>,"project_id":"<PROJECT_ID>","fields":["number","title","body","stage","revision"]}'
"$CANBAN_CLI" call timeline.list '{"card":<CARD_NUMBER>,"project_id":"<PROJECT_ID>","kinds":["comment.created","comment.updated"],"after_sequence":0,"limit":50}'
"$CANBAN_CLI" call review.preview '{"card":<CARD_NUMBER>,"project_id":"<PROJECT_ID>"}'

# Post a Card comment (supports @profile mentions to schedule another role)
"$CANBAN_CLI" call comment.add '{"card":<CARD_NUMBER>,"project_id":"<PROJECT_ID>","body":"Ready for @reviewer","mentions":["reviewer"],"request_key":"review-request-unique"}'

# Create a follow-up Card in Backlog
"$CANBAN_CLI" call card.create '{"project_id":"<PROJECT_ID>","title":"Follow-up task","body":"Details..."}'

# Inspect the Card's Agent terminals or captured terminal output
"$CANBAN_CLI" call execution.get '{"card":<CARD_NUMBER>,"project_id":"<PROJECT_ID>"}'
"$CANBAN_CLI" call execution.output '{"terminal_id":"<TERMINAL_ID>"}'
```

## Authoring Workflows (CLI)

When asked to inspect or update a Project's or Global (`__global`) Workflow YAML (`workflow.sources` is stored in SQLite, not Git files):

```sh
# Project-scoped workflow collection
"$CANBAN_CLI" workflow sources --project-id <PROJECT_ID>
"$CANBAN_CLI" workflow export --project-id <PROJECT_ID> --name development.yaml --output "$CANBAN_RUN_DIR/development.yaml"
"$CANBAN_CLI" workflow submit --project-id <PROJECT_ID> --revision <REVISION> --file "$CANBAN_RUN_DIR/development.yaml" --check
"$CANBAN_CLI" workflow submit --project-id <PROJECT_ID> --revision <REVISION> --file "$CANBAN_RUN_DIR/development.yaml"

# Global host-level workflow collection (`scope: global`, no Board columns or Cards)
"$CANBAN_CLI" workflow sources --global
"$CANBAN_CLI" workflow export --global --name cleanup.yaml --output "$CANBAN_RUN_DIR/cleanup.yaml"
"$CANBAN_CLI" workflow submit --global --revision <REVISION> --file "$CANBAN_RUN_DIR/cleanup.yaml"
```

## Scheduled Workflows (Cron Tasks)

Workflows support GitHub Actions-style cron triggers (`schema: 9`) across two scopes:
- **`scope: global`** (`project_id: __global`): Host-level schedules without Board columns or `card:` creation rules (ideal for maintenance, cleanup, cross-project checks).
- **`scope: project`** (default): Project-scoped schedules running in either **Cardless Script/Agent mode** or **Card-creating mode** (`card:`).

Missed ticks while Canban was offline or asleep are skipped rather than replayed on wakeup.

### 1. Script-First + On-Failure Agent Escalation (Lazy Admission)

Scheduled workflows starting with `uses: script` (or top-level `run:` shorthand) run with **zero PTY and zero `scheduler.max_agents` slot overhead** when they succeed (`exit 0`). Only when `on_failure` transitions into an `uses: agent` step does Canban allocate a `TerminalEntry` and an Agent slot, handing off the failed step's exit code and stderr/stdout tail in the prompt.

If that recovery Agent calls `"$CANBAN_CLI" report --state blocked --message "..."`, its **live PTY stays open** (never auto-reclaimed) and surfaces in **Activity & Notifications (`⌘I`) → Needs you** so a human can inspect or reply directly in the terminal. Reporting `completed` or `failed` reclaims the PTY after the turn settles.

```yaml
schema: 9
scope: global
id: global:worktree_gc
name: Worktree GC (7d)
on:
  schedule:
    - cron: "0 3 * * *"
      timezone: Local
      overlap: skip          # skip (default) | queue | allow
steps:
  - id: gc
    run: |
      "$CANBAN_CLI" worktree gc --merged-before 7d --fail-on-dirty --env-dirty DIRTY_WORKTREES
    on_failure: ask_agent
    next: finish
  - id: ask_agent
    uses: agent
    with:
      profile: developer
      instruction: |
        Worktree cleanup found dirty or active worktrees ($DIRTY_WORKTREES).
        Inspect the affected paths and call `$CANBAN_CLI report --state blocked --message "..."`
        if human input is required, or clean safe items and report `--state completed`.
    next: finish
  - id: finish
    uses: complete
```

Inside a Cardless scheduled Agent (`CANBAN_SCHEDULE_RUN_ID` is injected, `CANBAN_CARD` is unset; Global runs inject `CANBAN_SCOPE=global`), report your outcome with:

```sh
"$CANBAN_CLI" report \
  --state <completed|blocked|failed|working> \
  --message 'Concise summary of scheduled run' \
  [--file "$CANBAN_RUN_DIR/report.md"]
```

### 2. Card-Creating Mode (`scope: project` with `card:`)

Creates a Card on each trigger with `schedule_source` provenance (`Scheduled · <Workflow> #<run_number>` badge on Board and Card Detail) and runs the workflow's step graph against that Card so execution logs, reports, and diffs attach to its Timeline:

```yaml
schema: 9
id: nightly_qa:example
name: Nightly QA
on:
  nightly:
    cron: "30 3 * * 1-5"
    timezone: UTC
    overlap: skip
    start_at: run_qa
    card:
      column: develop
      title: "Nightly QA ({{date}} #{{run_number}})"
      body: "Automated scheduled check for {{workflow}} ({{datetime}})."
steps:
  - id: run_qa
    uses: agent
    with:
      profile: developer
      instruction: Run full integration suite and fix or report regressions.
    next: finish
  - id: finish
    uses: complete
```

Supported placeholders in `card.title` / `card.body`: `{{date}}`, `{{datetime}}`, `{{run_number}}`, `{{workflow}}`, `{{rule}}`. Supported `timezone` values: `Local` (default), `UTC`, fixed offsets (`+08:00`), or standard IANA names (`Asia/Shanghai`, `America/New_York`, `Europe/London`, `Asia/Tokyo`, etc.).

### 3. Operating Schedules & Worktree GC via CLI

```sh
# List schedules and recent runs (all scopes, --global, or --project-id)
"$CANBAN_CLI" schedule list
"$CANBAN_CLI" schedule list --global
"$CANBAN_CLI" schedule runs --global [--schedule-id <SCHEDULE_ID>] [--limit 20]

# Create a Global or Project scheduled workflow directly
"$CANBAN_CLI" schedule create --global --name "Worktree GC" --cron "0 3 * * *" \
  --mode script_agent --script '"$CANBAN_CLI" worktree gc --merged-before 7d --fail-on-dirty --env-dirty DIRTY_WORKTREES' \
  --profile-id developer --instruction "Inspect dirty worktrees and ask the user with --state blocked if needed."

# Manually trigger ("Run now"), toggle, or delete a schedule
"$CANBAN_CLI" schedule trigger --schedule-id <SCHEDULE_ID>
"$CANBAN_CLI" schedule toggle --schedule-id <SCHEDULE_ID> --off
"$CANBAN_CLI" schedule toggle --schedule-id <SCHEDULE_ID> --on
"$CANBAN_CLI" schedule delete --schedule-id <SCHEDULE_ID>

# Read a finished Cardless run's script log or report
"$CANBAN_CLI" schedule artifact --schedule-run-id <RUN_ID> --path <REPORT_OR_OUTPUT_PATH>

# Garbage-collect merged clean worktrees older than a retention window
"$CANBAN_CLI" worktree gc --merged-before 7d [--project-id <PROJECT_ID>] [--dry-run] [--fail-on-dirty] [--env-dirty DIRTY_WORKTREES]
```
