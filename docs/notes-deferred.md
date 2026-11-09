# Notes deferred

Decision: 2026-11-10. Notes need further product design and are removed from the current product scope. Board/Card task descriptions, comments, reports and attachments remain the supported Human Context surfaces.

## Current behavior

- No Notes destination, editor, Note-to-Card flow, Card Note selector or Open Note action is shipped in the desktop. Card details retain an **Attachments** section.
- Quick Open has Project, Card, Agent and Terminal destinations. Saved legacy Notes are not searchable destinations.
- Project activity summaries ignore Note events, including legacy Note-related events in Card Timelines. Historical Timeline text remains readable without a Note navigation action.
- New Agent captures do not include saved Notes, even when a legacy Card has `context_notes`. Their schema-1 manifests keep an empty `notes` array for compatible readers; prompts do not advertise Note operations.
- Gallery uses production components without a Notes scene. Some fixtures deliberately retain legacy Note records to check that they do not create a UI entry or affect summaries.

## Compatibility and data retention

This is a product withdrawal, not a data deletion/migration. Existing Note, revision and event records, Card associations, captured input files and archives remain intact. Existing captured executions are not rewritten.

Legacy Note RPCs, `card.context.update` and the hidden `canban note` command remain compatibility paths for already captured/running Agents and old clients. The command is omitted from normal CLI help; new prompts and current interface documentation do not present it as a supported product capability. These paths are not a commitment to the final Note design.

The earlier [Notes/workspace proposal](product/notes-and-workspace.md) and [implementation evidence](notes-and-workspace-implementation.md) are historical references. They do not define the current scope or establish acceptance of a future design. A future Note implementation requires a new product decision; no replacement knowledge-base feature is introduced now.

## Verification

Regression coverage checks that legacy Notes remain stored/readable while new captures contain no Note documents/instructions; ordinary Card context remains intact, earlier capture bytes remain unchanged, Quick Open excludes legacy records, and Note activity cannot create/reorder a Project summary. General isolated acceptance verifies the unchanged Develop/Review flow with retained legacy data and an undiscoverable compatibility command. Native acceptance is recorded after the implementation checks below.

Executed 2026-11-10: application/Gallery builds, all 146 workspace Rust tests, strict workspace/all-target Clippy and `mise run acceptance` passed. Acceptance home: `/private/tmp/cb-3kzhfcrk/home`. Provider acceptance scripts now put their isolation markers in Card content instead of requiring Notes; paid/live model runs were not repeated for this withdrawal.

The production-component Gallery verified the rail and global search without Notes, including no results for an existing fixture Note title. An isolated production app with two retained Notes, legacy Card associations and three real-Herdr deterministic fixture executions verified no Note navigation/search/context selector, ordinary Card creation through Command+N/Command+Enter, Task Edit/Save, comment submission through Command+Enter and adding/reading back a real file attachment. A Project with only Note events remained **No recent activity**. Saved Note records/events and the original Card associations were unchanged; all new fixture execution manifests had empty `notes` arrays and no Note CLI instructions. Local temporary evidence: `/private/tmp/cb-defer-notes-qnf3x1wf/` (`before.json`, `after-native.json`, `checks.json`, `evidence.json`). The Gallery, test app and isolated Core were stopped after verification. Chinese IME remains excluded.
