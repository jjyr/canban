# Workflow and Column automation UI refinement

Executed on macOS arm64, 2026-11-11, from an uncommitted tree based on
`fe5e5b9` (schema 12 / API 10). The current contract is
[Workflow implementation](workflow-implementation.md).

## Changes and review findings

- Column automation uses a Lucide workflow-node icon and a click-only NSH
  Popover. Both former tooltip layers and the column-local absolute panel were
  removed. Its opaque surface covers Card text, scrolls longer rule lists,
  stays inside the available overlay area and closes with Escape/outside/close.
- Column Card counts were removed. A shared minimum header height keeps column
  content aligned when only some columns have automation controls.
- The Project Workflows header keeps only Agent-icon Edit with agent at the
  right. New workflow follows the source list. History and contextual discard
  use compact labelled icon actions; automatic synchronization replaces the
  permanent Refresh action. Save and Command+S share eligibility/pending checks.
- Active-version labels are absent from the main editor; versions remain in
  History. Unsaved sources get a small marker; long filenames truncate within
  their allocated space and retain their full accessible label.
- NSH's built-in `highlight` feature is enabled explicitly. The main, historical
  and latest-source YAML editors use YAML syntax colors; the source editor also
  uses two-space indentation, guides, line numbers and soft wrapping.
- Agent and History use native modal layers instead of absolute page panels.
  A focused NSH editor originally consumed Escape by blurring without closing
  its modal. The mounted dialog now owns an explicit Escape binding, restoring
  the source caret after closing. Configuration-launch errors render inside
  the Agent panel. Discard is disabled while a save/load is pending.

## Executed acceptance

All AppServer/Core fixtures were disposable and isolated. No live Canban home,
installed App, paid model run or user Core was changed.

| Check | Result and evidence |
| --- | --- |
| Build and workspace | `mise run build`, 231 Rust tests and workspace/all-target Clippy with warnings denied passed. Logs: `/private/tmp/canban-workflow-ui-build-final.log`, `/private/tmp/canban-workflow-ui-tests-final.log`, `/private/tmp/canban-workflow-ui-clippy-final.log`. |
| Standard application | `mise run acceptance` passed at `/private/tmp/cb-n2pau2e_/home`; log `/private/tmp/canban-workflow-ui-standard.log`. |
| Workflow integration and native UI | `mise run acceptance-workflows -- --native`; `/private/tmp/cb-workflows-b7htbt2s/ui-refinement-evidence.json`, log `/private/tmp/canban-workflow-ui-native-final.log`. The harness passed its real-service/PTy/configuration-history assertions before the UI inspection. |
| Native editing/publication | Observed YAML colors, edited a comment and saved with Command+S; unsaved marker cleared. Opened History, loaded v1 as an unsaved draft and clicked Save, producing v3 rather than rewriting history. Created and saved a new source using the action below the list. |
| Native modal/focus | Opened Edit with agent from the focused source editor; Prompt received native focus. One Escape closed the modal and restored the source editor focus. Gallery also verified the modal's opaque surface over the native editor. |
| Native publication conflict | While a local comment remained unsaved, another fixture client published v4 through the normal API. The local text survived and highlighted saved YAML appeared for comparison. Clicking Save produced `revision_conflict`; explicit discard loaded the external text and cleared the marker. Source snapshots are saved beside the evidence file. |
| Native Column details | Clicked Review automations over a real Card; the opaque panel covered the Card text. View workflows closed it and returned to the Project source page. Column headers displayed no numeric Card count. |
| Gallery | `column-automations` exercised production Board text under first/last-column popovers, five rule groups, scrolling, Escape and outside dismissal. `workflows` verified syntax colors and focused-editor modal dismissal. `workflows-conflict` exercised an 800-point viewport, long filename truncation, unsaved marker, highlighted saved-source comparison and reachable Agent/Save actions. Logs: `/private/tmp/canban-workflow-ui-gallery*.log`. |

The native save/history/conflict/modal checks used the final interaction code.
The subsequent equal-height Column header adjustment and History help-text edit
were included in the final build, workspace checks and Gallery build. Native
provider authentication, generated Agent plans, IME and exhaustive theme/window
combinations are outside this verification scope. The 800-point case constrains
the production component in Gallery; an attempted OS window-edge drag did not
resize the real App and is not counted as a successful resize test.
