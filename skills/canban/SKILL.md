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

> **Interactive sessions vs. comment tasks**:
> - In an **interactive Card session**, reply directly in the terminal for initial discussion; only run `report` once the user asks you to complete a concrete task.
> - In a **dedicated `@mention` comment assignment**, publish your answer and finish in a single `report --state completed` call (do not call `reply` separately).

### Answer a live follow-up question (`reply`)

When a human posts a follow-up question with a Question ID against an already running non-comment task, answer that question without ending your main task:

```sh
"$CANBAN_CLI" reply --to <QUESTION_ID> --file "$CANBAN_RUN_DIR/reply.md"
```

### Inspect or update the current Card (`card show` / `card update`)

Read the current Card JSON (uses `CANBAN_CARD` + `CANBAN_PROJECT` automatically, or pass `--card <NUMBER>`):

```sh
"$CANBAN_CLI" card show
```

Update the Card title or Markdown description only with user-confirmed design decisions or when explicitly requested:

```sh
"$CANBAN_CLI" card update [--title 'Updated title'] --file "$CANBAN_RUN_DIR/card.md"
```

### Adopt a workflow stage (`card adopt`)

In an interactive session on a Card not yet in `develop`, once the user authorizes implementation, adopt `develop` first so the current session takes over without spawning a duplicate scheduled run:

```sh
"$CANBAN_CLI" card adopt --stage develop
```

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
"$CANBAN_CLI" call card.list '{"project_id":"<PROJECT_ID>"}'

# Read a Card, its Timeline events, or Diff preview
"$CANBAN_CLI" call card.get '{"card_id":"<CARD_ID>"}'
"$CANBAN_CLI" call timeline.list '{"card_id":"<CARD_ID>","limit":50}'
"$CANBAN_CLI" call review.preview '{"card_id":"<CARD_ID>"}'

# Post a Card comment (supports @profile mentions to schedule another role)
"$CANBAN_CLI" call comment.add '{"card_id":"<CARD_ID>","body":"Ready for @reviewer"}'

# Create a follow-up Card in Backlog
"$CANBAN_CLI" call card.create '{"project_id":"<PROJECT_ID>","title":"Follow-up task","body":"Details..."}'

# Inspect the Card's Agent terminals or captured terminal output
"$CANBAN_CLI" call execution.get '{"card":<CARD_NUMBER>,"project_id":"<PROJECT_ID>"}'
"$CANBAN_CLI" call execution.output '{"terminal_id":"<TERMINAL_ID>"}'
```

## Authoring Workflows (CLI)

When asked to inspect or update a Project's Workflow YAML (`workflow.sources` is stored in SQLite, not Git files):

```sh
"$CANBAN_CLI" call workflow.sources '{"project_id":"<PROJECT_ID>"}'
"$CANBAN_CLI" workflow export --project-id <PROJECT_ID> --name development.yaml --output "$CANBAN_RUN_DIR/development.yaml"
"$CANBAN_CLI" workflow submit --project-id <PROJECT_ID> --revision <REVISION> --file "$CANBAN_RUN_DIR/development.yaml" --check
"$CANBAN_CLI" workflow submit --project-id <PROJECT_ID> --revision <REVISION> --file "$CANBAN_RUN_DIR/development.yaml"
```
