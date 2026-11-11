# Workflow sources, execution and subscriptions

Status: implemented in this worktree, 2026-11-11. Schema 12, desktop API 10. This is the concrete interface and UX reference for [Workflow rules](product/workflow-engine.md) and [events](product/events-and-notifications.md). The SQLite-only source decision supersedes the earlier proposal for managed YAML files.

## Project authoring and storage

A Project owns multiple named Workflow sources. The Board remains Backlog / Develop / Review. Open **Workflows** in the Project header, or **Project Workflows** in Settings. The page lists sources, preserves original YAML in a native editor, and places **New workflow** below the source list and keeps only the Agent-icon **Edit with agent** action at the top right. The editor has icon actions for **History** and, while modified, **Discard changes and reload**, plus **Save**. There is no permanent Refresh button or active-version label. Card detail retains execution history, reports and approval controls, without a full configuration/graph panel.

```text
Project / Workflows
  development.yaml        | Source name   [History]       [Edit with agent]
  issue-planning.yaml  •  | YAML editor (syntax colors, line numbers,
  pr-review.yaml          | two-space indentation and soft wrapping)
  [+ New workflow]        |                                   [Save]
```

Names ending in `.yaml` identify documents, not paths under the Canban home. SQLite is the only source of truth:

```text
canban.sqlite
  workflow_templates       bootstrap for new Projects
  workflow_sources         Project + source name + active revision/hash
  workflow_revisions       immutable original YAML + compiled definition
  workflow_runs            captured revision + execution checkpoints
  event_subscriptions      selectors + target + durable cursor
  event_deliveries          captured definition revision + receipt/progress
  configuration_sessions   Project + configuration purpose + Profile/history
```

No Workflow YAML, source mirror or revision copy is generated under `~/.canban`. An old `config/workflows/default.json`, if present before upgrade, is imported once into the SQL bootstrap template; subsequent reads use SQL. The old input is not monitored or rewritten. Existing Project templates and JSON definitions become visible SQL YAML sources. Existing live/queued assignments and decisions retain their captured identity and semantics.

**Save** (also Command+S) validates the entire submitted collection and commits original YAML, immutable revisions, active references and subscriptions in one SQLite transaction. Invalid YAML/graphs/Profiles/columns or any database failure leaves all previously active configuration intact. The editor retains its draft. Successful publication quietly clears the unsaved marker and updates stored revisions and Column descriptions; it produces no success toast or publication banner.

The collection revision returned by `workflow.sources` is an optimistic concurrency token. A competing Agent/editor publication causes `revision_conflict`. The native page retains the user's buffer and shows highlighted latest YAML for the selected source alongside the retained editor; reloading/discarding is explicit. Drafts survive navigation within the current window. Unsaved editor drafts are not another published SQL revision and are not visible to a separately launched Agent.

The native YAML editor uses NSH built-in syntax highlighting with the `highlight` feature enabled, two-space indentation, soft wrapping and an explicit unsaved marker in the source list. Agent and History panels use native modal layers; Escape closes them from a focused editor and restores the source caret. Configuration-launch errors remain visible inside the Agent panel.

History reads immutable SQL revisions. **Load as draft** puts historical YAML into the editor. Applying it creates a new revision; it never rewrites history. Original comments, multiline instructions and formatting are retained byte for byte on ordinary YAML publication. JSON compatibility import necessarily generates YAML.

## YAML language

See [the complete example](../examples/workflows/plan-and-develop.yaml). YAML 1.2 parsing uses `serde_yaml_ng`; duplicate mapping keys, unknown fields, unknown steps/Profiles/columns and automatic cycles fail validation. This is GitHub Actions-inspired authoring syntax for the existing Canban interpreter, not GitHub Actions compatibility.

```yaml
schema: 1
id: plan-only
name: Read and plan
on:
  manual:
    event: manual
    start_at: plan
  new_issue:
    event: card.created
    match:
      source_kind: github.issue
      reason: automatic_intake
    start_at: plan
steps:
  - id: plan
    name: Read and design
    uses: agent
    with:
      profile: codex
      instruction: |
        Read the Card and current source, then report a concrete plan.
  - id: approve
    uses: approval
    with:
      title: Review the plan
    on_changes_requested: plan
```

`schema` versions syntax independently of publication revisions. `id` is stable and must be unique in the current engine registry; `name` is presentation. Named `on` rules have distinct IDs, so several rules can subscribe to the same event. Each provides `event`, optional `match`, `start_at`, and optional `enabled` (default true). Project scope is implicit; `match.types` is rejected because `event` supplies the type. Top-level `enabled: false` disables future starts for that source without destroying history.

`steps` is a sequence. Every step has a stable `id`, optional authoring `name`, `uses`, and `with` arguments. Ordinary steps default to the next sequence item; `next` overrides that successor. `on_changes_requested` supplies an approval rework target. The last ordinary step ends the run. `complete` explicitly ends a path. Supported operations:

| `uses` | `with` arguments |
| --- | --- |
| `agent` | optional `profile`, required `instruction` |
| `card.move` | `column` |
| `approval` | `title` |
| `condition` | JSON `pointer`, exact `equals`, `then`, `otherwise` step IDs |
| `notify` | `message` |
| `complete` | none |

Conditions read `{inputs, outputs, card}`. Outputs reference exact reports/paths and selected messages. Definition-level `workspace` controls checkout preparation; `max_steps` (1–1000, default 100) bounds visits including explicit human rework. Parallel/join/subflow/script execution and per-step filesystem isolation are not implemented.

## API and file submission

All clients use the same AppServer validation/publication operations. `canban call` accepts inline JSON or `@/absolute/params.json` containing the complete parameter object.

| Operation | Parameters / result |
| --- | --- |
| `workflow.schema` | YAML syntax, built-ins and supported operation descriptions |
| `workflow.sources` | `project_id`; returns collection `revision`, `sources` and active SQL references |
| `workflow.check` | `project_id`, collection `revision`, complete `sources`; validates without publishing |
| `workflow.apply` | same inputs; atomically publishes source and enabled triggers |
| `workflow.list` | optional `project_id`, `workflow_id`; immutable revisions, including original `source_yaml` |
| `workflow.get` | `workflow_id`, optional historical `revision` |
| `workflow.prompt` | `project_id`; complete Project-wide configuration Prompt |
| `workflow.template` | read the SQL bootstrap template for Project creation compatibility |
| `workflow.start` | `card_id`, `workflow_id`, enabled manual `entry` |
| `workflow.runs` / `workflow.run` | optional `card_id` / exact `run_id` |
| `workflow.decide` | `run_id`, current `visit`, Card `revision`, `decision: approve/rework`; human caller |
| `workflow.stop` | `run_id`; stop future progression, without killing the live PTY |

Example `workflow.apply` parameters:

```json
{
  "project_id": "PROJECT_ID",
  "revision": "COLLECTION_TOKEN_FROM_WORKFLOW_SOURCES",
  "sources": [
    { "filename": "planning.yaml", "source": "schema: 1\nid: planning\n..." },
    { "filename": "development.yaml", "source": "schema: 1\nid: development\n..." }
  ]
}
```

`filename` is the source document name, not a server filesystem path. Omit a source to retire its active reference and future subscriptions; its immutable revisions and existing runs remain. Disabling via YAML is usually more convenient. An unchanged source does not create another revision.

Files are an optional submission/export transport, under a path chosen by the user or Agent:

```sh
CANBAN=/absolute/path/to/canban
"$CANBAN" --home /chosen/home call workflow.sources '{"project_id":"PROJECT_ID"}'
"$CANBAN" --home /chosen/home workflow submit \
  --project-id PROJECT_ID --revision COLLECTION_TOKEN --file /tmp/planning.yaml --check
"$CANBAN" --home /chosen/home workflow submit \
  --project-id PROJECT_ID --revision COLLECTION_TOKEN --file /tmp/planning.yaml
"$CANBAN" --home /chosen/home workflow export \
  --project-id PROJECT_ID --name planning.yaml --output /chosen/new-path.yaml
```

Submit merges that named source into the current collection, requires the supplied collection token and validates/publishes through the same API. Export creates a new output file and refuses to overwrite an existing one. Local file changes have no effect until submitted. Neither command creates an internal Workflow directory.

Legacy `workflow.validate`/`workflow.publish` accept JSON definitions; publication imports into the same SQL YAML source model. `workflow.bind` translates old default-entry selection into visible YAML trigger rules. These are compatibility inputs to one interpreter, not independently editable runtime configurations.

## Edit with agent

The Project header has one **Edit with agent** entry. There is no separate Copy/View Prompt action outside it. Its panel contains:

- Complete selectable, scrollable Project-wide Prompt, independent of selected source.
- **Select all** and explicit **Copy prompt**; opening never modifies the clipboard. The preview stays visible after copying. Copy failure appears inline with manual selection/copy instructions.
- Profile selection from the existing registry, including Codex/Claude/custom commands.
- **Start and open Terminal**, **Continue in Terminal**, or **Resume and open Terminal**, based on the selected Project/Profile's conversation. **New conversation** is explicit; end a live conversation before replacing it.

The exact Prompt generated by `AppServer::workflow_prompt` is used for both copy and initial launch. It includes the actual Project identity/directory, CLI, home and endpoint; instructs the Agent to read schema/Profiles/all SQL YAML sources and active revisions; use ASCII to explain triggers, Agents, column effects, approvals/rework and overlap; and wait for the user's requested changes. It then explains expected-revision check/apply, preserving unrelated YAML/comments and existing instances, and forbids direct SQLite editing. Local files are described only as optional submission/export inputs. Codex's actual history binding is part of this visible Prompt.

A configuration conversation is owned by **Project + workflow-configuration purpose + Profile**. It creates neither a business Card nor a Request/Attempt that could advance a Card. It uses existing Profile argv expansion, global Scheduler admission, Core/PTY transport, native Terminal renderer and lifecycle controls. Configuration Agents run on the AppServer host, where the configuration service is available, independently of a Project's business execution machine.

```text
Project
  Configuration
    Workflows · Codex
          |
     existing native Terminal
          |
     [Back to Workflows]
```

Continue only focuses the retained Terminal; it does not resend the Prompt. Resume uses the exact saved provider history; unknown launches are not blindly replayed. Provider installation/authentication and actual model behavior remain provider requirements. Switching Profile is explicit. Navigation receipts are fenced to the originating page so late launches do not redirect users elsewhere. Closing a queued configuration terminal cancels admission.

## Events, overlap and Column hints

`canban-workflow-engine` remains the only interpreter. AppServer supplies transactional IO and an enqueue adapter to existing business Conversation/Request/Attempt/Scheduler execution. Legacy callback fields are compatibility snapshots, not another executor. Planning runs in Backlog; Workflow-owned movement continues the same run.

All matching subscriptions receive durable receipts. One main Workflow runs on a Card at a time; another matching event waits visibly in `subscription.deliveries` with its captured definition revision. Waiting for an active run or human gate does not consume failure retries. Other delivery failures use bounded backoff and inspectable failure/retry. New manual starts conflict visibly. Publication applies immediately to future receipts/runs; captured receipts, active runs, queued assignments and human decisions retain their old versions. `workflow.stop` does not kill a PTY.

Column headers show the stage name without a Card count and retain equal height whether or not automation exists. Enabled column-related triggers show a **workflow-node** icon. Hover does not open a tooltip. Clicking opens an opaque native popover above Card text, with bounded scrolling, rule details grouped by Workflow and a **View workflows** link. Escape, the close icon and pressing outside dismiss it. Flow effects are labelled separately. Rules with no clear column association stay in the Project source page. The pure `source::column_details` library derives text from active definitions/rules/subscriptions in the client; AppServer has no hover-prose generation RPC. Unsaved drafts never affect these hints.

Column rules default to local-origin events; Workflow-owned moves do not start a duplicate developer through another default column rule. Explicit origin filters can opt into other events; causal ancestry still suppresses workflow loops. Initial GitHub baselines/restoration do not trigger automation without explicit subscription policy.

`event.list` pages by durable offset with selectors. `subscription.save/list/deliveries/ack/retry` provide all-event or selected Project/Card/type/column/source subscriptions. Consumer receipts are independent of Workflow targets. GitHub metadata observation covers Issue/PR changes and discussions without retaining remote prose in durable history. GitHub integration currently polls approximately every 120 seconds; it is not a lossless webhook audit stream.

## Migration and verification

[Workflow UI refinement acceptance](workflow-ui-acceptance-20261111.md) records
the current click-only popover, simplified editor, syntax colors, native modal
focus/dismissal and conflicting-publication checks. The earlier evidence below
retains its original UI labels and layout.

Schema 12 adds SQL source references, the bootstrap template and Project configuration sessions. Original YAML is stored on immutable Workflow revisions. The migration itself is database-only; startup converts legacy Project/template/definition input without relaunching active work. Downgrade refuses to discard authored YAML sources or live configuration sessions. An older binary cannot interpret these sources: preserve/migrate them deliberately before downgrading.

Required checks are `mise run test`, `mise run build`, `mise run acceptance`, `mise run acceptance-workflows`, workspace Clippy and `git diff --check`. Native Gallery scenes `workflows`, `workflow-agent`, `workflows-conflict` (an 800-point viewport and long names) and `column-automations` reuse production editors/panels/popovers. `scripts/acceptance-workflows.py --native` runs a scratch packaged App, isolated home/Core and deterministic Profile programs over real PTYs; it never replaces the live home/Core.

Fresh evidence from 2026-11-11:

| Check | Result / isolated artifact |
| --- | --- |
| Workspace tests | Passed; includes 104 AppServer tests and async editor draft-retention regression |
| Build, Clippy and diff checks | Packaged schema 12/API 10; warnings-as-errors and `git diff --check` passed |
| Main acceptance | `/private/tmp/cb-__81kjz8/home`; Develop/Review, mentions, gates, archive/restore, restart and cross-Board budget passed |
| Workflow acceptance | `/private/tmp/cb-workflows-y08f4pic/evidence.json`; SQL YAML/version conflicts, explicit file transport, retained instances, Prompt identity and configuration history reuse/resume passed |
| Native App | `/private/tmp/cb-workflows-7zkg0kuh/native-ui-evidence.json`; YAML edit/apply, quiet v1→v2, historical v1 draft→v3, copy/Continue/Terminal return and concurrent publication conflict with retained native draft |
| Native Gallery | `mise run gallery` with the production Workflow editor and Edit with agent panel; source/Prompt/Profiles and Prompt focus checked |

These temporary paths are local acceptance artifacts, not permanent runtime storage. Deterministic real-PTY checks do not establish live Codex/Claude authentication or generated-plan quality. Onboarding/default Profile setup remains a separate UX topic.
