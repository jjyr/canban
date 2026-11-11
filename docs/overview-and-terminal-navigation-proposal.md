# Overview and Terminal navigation refinement

Status: selected by the user and implemented, 2026-11-11. The findings below record the pre-change design audit. Current behavior is in [Overview](product/overview.md) and [Terminal navigation](terminal-workspace.md#unified-terminal-navigation); executed checks and limits are in [refinement acceptance](overview-terminal-acceptance-20261111.md).

## Findings

Overview currently gives every conversation a bordered, three-to-four-line block. Profile precedes task identity; runtime summary and group/reason often repeat each other; missing branches occupy a permanent line. Selecting a row opens an inspector (replacing the list on narrow windows); destination actions are inside it. Unread Cards also have their own section and can repeat the same Card's conversation rows. See `crates/ui/src/overview.rs` (`agent_row`, `unread_updates`, `row_actions`).

The projection preserves useful distinctions: Card/profile conversations, independent task/runtime reasons and saved human-review gates (`crates/notifications/src/overview.rs`). Keep those identities while changing presentation. Current age falls back to request/Card creation; it should not be labelled time in current execution state without corresponding evidence.

Terminal navigation is partially shared, but target resolution and execution intent differ across callers. `Ui::open_terminal` redirects an unlisted entry to its Card; `terminals::open_card` chooses an entry by ended state without asking which profile; `open_request` converts a historical request into its Card/profile key. This can obscure the distinction between a historical invocation and its current conversation. `sessions::open` issues `agent.open`, whereas ordinary Terminal selection is navigation only.

## Selected Overview

Answer three questions: what needs me, what changed, and what is running across Projects. Keep it global and filter-free by default. Remove the default inspector and read-only Terminal preview; put handling in the existing Card, Review or Terminal workspace. Optional diagnostics remain in an overflow action.

```text
Overview                              Running 4 / 8  ·  Queued 2

Needs you  3
! #42 Search permissions             canban · @developer    4m
  Permission required                                      [Terminal]
! #17 Choose retry policy            fiber · @developer     8m
  A decision is needed on retry limits                     [Open Card]
◇ #28 Export results                 tools · @reviewer      2m
  Result ready for human review                            [Review]

Updates  2
• #31 Fix reconnect                  fiber · @developer
  Latest unread reply excerpt…                             [Read reply]

Running  4
● #36 Add search                     canban · @developer    [Terminal]
  Reported activity, when available

Queued  2
  #37 Import settings                tools · @developer    [Open Card]
  Waiting for an execution slot

▸ Idle  3                         ▸ Closed conversations  12
```

Example content and counts above are illustrative.

- Combine human-review and other actionable rows into Needs you, retaining distinct reason labels and destinations. Preserve each independently resolvable reason; a row with both task and runtime waits exposes both Card and Terminal actions.
- Show task title first, Project/profile second, then one useful reason or reported activity line. Do not substitute generated guesses for missing activity; use a concise factual state. Branch, provider and machine are secondary; show machine inline when remote/unavailable context matters. Full values remain available through tooltips/overflow.
- Use compact aligned rows and section spacing instead of a border around every item. Reserve action width, truncate titles, and use restrained color plus text/icons. Highlight the reason rather than tinting every actionable row strongly.
- Needs you and Running are expanded. Updates and Queued appear only when nonempty. Idle and Closed are collapsed by default with retained expansion state. Closed remains discoverable for recovery.
- Counts name their unit: actionable items/conversations, unread Cards and Scheduler execution slots are different. Keep the rail's existing unique-Card reminder count; do not imply it equals the sum of section counts.
- If a Card is already in Needs you, attach its unread indicator there and omit a duplicate Updates item. An Updates Card can still have Running conversations: the former represents information to read, the latter runtime activity. Avoid repeating the same excerpt in both places.
- Row activation invokes its visible primary destination. Explicit secondary actions go directly to their targets. Keyboard selection/focus alone does not navigate; Enter activates. No new intermediate page. Closed rows open saved Card history; Resume remains an explicitly labelled action.
- Waiting age comes from the relevant reason episode; execution age requires a real state-start timestamp. Omit misleading age when unavailable. Stable identity and ordering keep background updates from moving focused rows; ties use stable IDs.
- Visiting Overview does not mark replies read or resolve gates. Read reply should open the corresponding Card Timeline item; only actual visible reading updates unread state. Preserve origin, scroll, expansion and focus on Back.

Scope: retain managed Card conversations and recovery items. Include Project configuration Agents in current execution visibility so Workflow editing is not invisible; this requires extending the current Card-only projection with a Project-owned item. Do not include all manual Shells as task activity; the Terminal workspace remains their inventory. Host resource dashboards are outside this proposal.

## Terminal entry inventory and selected behavior

| Surface | Current entry/source | Proposed behavior |
| --- | --- | --- |
| Overview | Inspector Open terminal / Resume, `overview.rs` | Inline exact Terminal action; explicit Resume for closed conversations |
| Board Card tile | Card opening, `components/card_tile.rs`; no direct Terminal entry found | Keep title opening Card; add a compact active-Agent Terminal action and context-menu equivalent |
| Card header | Terminal icon and profile chips, `card/header.rs` | Icon resolves Card context; profile chip targets that profile; status-specific action label |
| Card attention | Needs attention jumps back to Overview, `card/header.rs` | Runtime wait opens exact Terminal; task gate stays on Card; review opens Review; multiple reasons use a small action menu |
| Card Timeline | Agent link and actor link, `components/timeline_item.rs` | Preserve request/attempt provenance; active matching invocation opens Terminal; historical execution opens its saved context with a separate current-conversation link |
| Run panel | Open / resume Agent, `components.rs` | Split label and behavior into Open Terminal, Resume Agent or Start Agent, according to authoritative eligibility |
| Review | Open Card Terminals, `review.rs` | Use shared Card target selection; retain Review draft and scroll on return |
| Quick Open | Agent and Terminal candidates, `quick_open/model.rs` and `quick_open.rs` | Live targets attach directly; closed candidates clearly say View history and do not silently resume |
| Project Workflows | Edit with agent, `workflows.rs` | Explicit Start/Resume and open Terminal; existing live conversation uses Open Terminal; retain Project configuration identity |
| Machine connection | Connect machine, `machines.rs` | Creation receipt opens its exact connection Terminal through the same navigation path |
| Terminal workspace | Tree, tabs, cycling, new tab and Agent menu, `terminals.rs`, `terminals/tree.rs`, `terminals/tabs.rs` | Reuse existing tab by stable identity; separate New Terminal and Start/Resume Agent from selection |
| Rail / global commands | Terminals destination and Quick Open | Rail restores last valid Terminal; context-specific Open Terminal uses the shared resolver. Choose any new shortcut only after checking the current command catalog |
| Archive / ended execution | Historical Card/conversation context | View history; explicit Restore before eligible execution; never revive through navigation |

## One interaction contract

Use one product navigation intent with typed target context: exact Terminal, conversation, invocation, or Card. Do not turn all targets into a conversation string before resolving provenance. UI owns navigation and focus; AppServer remains authoritative for runtime binding, eligibility and execution effects. Existing server operations can remain; a new RPC is not inherently required.

For a Card-level request, reuse its last valid explicitly selected Agent when available; otherwise directly open the sole live Agent. Multiple live Agents show an anchored compact picker with profile/provider, state and machine. Never choose an arbitrary profile by iteration order. With no live Agent, the same picker shows labelled Resume/Start choices and queue/history context; it does not navigate to an empty Terminal workspace. Exact profile/invocation links skip this picker.

| Target state | Action and result |
| --- | --- |
| Live, connected | Open Terminal: select/reuse exact tab, reveal tree group, focus PTY |
| Live, waiting for input | Open Terminal with the same binding and visible reason |
| Starting / queued | Show request context and factual state; no second Start or capacity bypass; attach when a real binding exists |
| Disconnected / unknown | Open existing target with connection feedback; reconnect same binding; do not infer exit or launch a replacement |
| Closed, resumable | Resume Agent is explicit, uses exact provider history and Scheduler admission; show pending/queued feedback |
| Closed, missing history | View history and explain unavailable resume; any fresh-context start is separately labelled |
| Archived | View saved context; Restore is explicit before execution |
| Target removed during navigation | Explain unavailability at source and offer Card/history; no silent switch to an unrelated Agent |

All successful direct opens converge on the existing full Terminal workspace. No Terminal preview popover, embedded second renderer or confirmation for ordinary attach. Back/Forward restore source context without executing Start/Resume. Late launch receipts update state but must not steal navigation after the user leaves. Double activation is pending-guarded; reuse tab/binding and retain existing operation idempotency.

## Delivery and acceptance

1. Implement shared target resolution and action labels, including exact historical provenance, multiple profiles, unavailable states and safe async receipts. Migrate current callers first.
2. Replace Overview inspector navigation with compact production rows and direct actions. Add useful source-derived reason/activity summaries and the Project configuration projection; keep unsupported fields explicitly absent.
3. Add Board and contextual attention shortcuts using the same resolver. Update current feature contracts only when the direction is selected and implemented.

Gallery scenarios: long titles, narrow width, multi-reason rows, no reported activity, unread deduplication, disconnected machine and keyboard focus. Isolated native acceptance: every entry above, two live profiles, historical request versus current invocation, queue without PTY, exact resume, disappearance during navigation, repeated click, late receipt, Back to Review/Overview with drafts/scroll intact, and unchanged Terminal attachment during metadata updates. Use the project build/test/acceptance tasks when implementation changes; this document adds no runtime validation claim.
