# Agent profiles and workflow references

Current behavior update, 2026-11-09: [Terminal interaction and navigation](../terminal-interaction.md) supersedes immediate completion exit, indefinite viewer protection and ended-session pages. Completed Agents stay interactive until their configurable idle timeout; actual user input renews activity, exited sessions leave the Terminal list/tabs, and exact Resume lives on the Card.

Multi-machine update, 2026-11-09: see [current execution/location and service contract](../multi-machine.md). Profile commands resolve on their execution host; Resume and Review retain their original workspace, while new work uses the Project's current location.


Status: implemented 2026-11-07; current contract reconciled 2026-11-10. This is the current schema. Earlier provider/Herdr-template/hook proposals are superseded. See [MVP acceptance](../mvp-acceptance.md) for evidence and limits.

## Ownership

Profiles are application-wide configurations in `~/.canban/config/agents.json`. Workflow templates reference their IDs and define stage tasks/transitions. A Card retains one provider conversation for each profile ID. Developer and Reviewer may both use Codex but have independent histories. Neither profile definitions nor workflow templates are live Sessions.

```text
config/agents.json                 workflow JSON
  developer + commands + role <--- Develop.profile
  reviewer  + commands + role <--- Review.profile
                                      |
                                Project snapshot
                                      |
                             Request profile/task snapshot
                                      |
                       generic invocation -> Herdr terminal
```

`canban-profiles` resolves JSON presets and whole-field overrides. Scheduler/Workflow Engine/AppServer invoke a common interface; runtime code does not select Codex/Claude branches. Herdr supplies status, turn tracking, PTY input, attachment and interruption. Canban owns explicit reports/replies and the opaque provider-history binding.

## Profile schema

| Field | Meaning |
| --- | --- |
| `id` | Required identity for mentions, workflow references and Card conversation reuse |
| `preset` | Optional preset name; omitted fields inherit its defaults |
| `role_instruction` | Role text included separately from the current task; empty by default |
| `argv` | First-launch argument array; must resolve to a nonempty array |
| `resume_argv` | Exact-history command array; absent inherits, `[]` explicitly disables Resume |
| `exit` | `{"type":"manual"}` or `{"type":"input","text":"/exit"}`; no preset defaults to manual |
| `session_instruction` | Optional prompt recipe for binding history; `""` explicitly disables inherited text |

`argv` and `resume_argv` replace the corresponding preset array in full. They are individual arguments, with no implicit shell evaluation/appending. Supported placeholders are `{prompt}`, `{home}`, `{cli}` and `{session_id}`. A fresh launch referencing `{session_id}` gets a preallocated UUID; otherwise the Agent binds its actual ID. Saved IDs are opaque; Canban never substitutes a Herdr terminal address or a latest-history search.

Presets are embedded JSON in `crates/profiles/presets`. `config/presets/<name>.json` supplies or replaces a preset of that name. Presets do not inherit other presets. Reload applies changes to subsequently queued work; already queued/running Request snapshots remain unchanged. Editing commands does not reset an existing conversation. An incompatible Resume fails visibly and offers explicit fresh context after the failed invocation exits.

```json
{
  "hc": "hc",
  "hc_session": "default",
  "hc_env": {},
  "default_agent": "developer",
  "profiles": [
    {
      "id": "developer", "preset": "codex",
      "role_instruction": "Implement the agreed Card task and explain validation."
    },
    {
      "id": "reviewer", "preset": "codex",
      "role_instruction": "Review actual code and tests; publish an ordinary report."
    },
    {
      "id": "my-agent", "preset": null,
      "role_instruction": "Publish explicit Canban outcomes.",
      "argv": ["/absolute/path/to/wrapper", "start", "{prompt}"],
      "resume_argv": ["/absolute/path/to/wrapper", "resume", "{session_id}", "{prompt}"],
      "session_instruction": "Bind your actual history using canban bind-session.",
      "exit": {"type":"manual"}
    }
  ]
}
```

A user overriding Codex's executable/options can set both complete command arrays while retaining `preset: "codex"`. An explicit manual exit or empty resume array also overrides the preset. `profile.presets` exposes available defaults; `config.get` exposes resolved configuration. Preserve the source JSON when editing overrides: serializing the resolved configuration back would turn inherited defaults into explicit overrides.

## Shipped recipes

Codex starts interactively with `--no-daemon --sandbox workspace-write --add-dir {home}` and resumes the exact ID. Its prompt recipe asks the Agent to call `"$CANBAN_CLI" bind-session --session-id "$CODEX_THREAD_ID"` before other work. Claude starts with `--session-id {session_id}` and resumes with `--resume {session_id}`. Both configure normal `/exit` input. Neither installs notify/Stop hooks or changes global provider settings. Claude still needs successful local authentication for real acceptance.

The `exit` recipe supplies a normal provider exit command; it is not a completion hook. An explicit completed report records the outcome and applies its callback or human gate without immediately closing the Terminal. Idle reclamation later requires verified root idle/done, a resumable binding, no protected pending work and the configured timeout. Actual human input renews activity; merely viewing the Terminal does not indefinitely protect it. Providers without usable status or Resume/normal-exit support retain manual control. A sent normal exit that does not finish within 30 seconds remains visible for inspection and is not repeatedly sent. See [Terminal interaction](../terminal-interaction.md) for the current lifecycle; the earlier completion-plus-later-done immediate-exit rule is superseded.

## Workflow and routing

The default JSON workflow has non-executing Backlog, Develop using `developer` with `on_completed: "review"`, and Review using `reviewer` with no automatic next stage. Review means an ordinary report, followed by human disposition.

Selection order is explicit `@profile`, stage `profile`, then global `default_agent`. Routing tags were removed on 2026-11-10: neither Cards nor Profiles expose a `tags` field. Old JSON fields are ignored on decode without a dedicated migration; raw configuration files and historical records are not proactively rewritten. New `card.create`/`card.update` calls reject `tags`, including empty arrays. Desktop API level 3 rejects an older AppServer that could still apply tag-based routing; `mise run run` starts a matched UI/service build. Explicit unknown Profile IDs fail instead of falling back. Direct mentions do not change the workflow callback. Startup, Project creation and reload validate profile references before activating the new registry.

## Migration

Recognized legacy provider/batch configuration is converted once. Canban first saves the exact original bytes as `config/agents.legacy-<uuid>.json`, then atomically publishes validated JSON. Known provider launch/resume defaults are supplied through presets. Historical Request snapshots and conversations are not rewritten. A legacy opaque Herdr `template` is rejected with instructions to supply explicit commands; the original config stays intact. Herdr launch-template mode is no longer a Profile execution path. Kanban workflow templates are unaffected.

## UI

Settings shows resolved profiles and reload controls; Board headers show stage names/counts with automatic roles in their tooltip; Card comments offer the same configured mentions. Clicking a live Card Agent attaches to its terminal. Clicking an exited Agent resumes its exact conversation in a new terminal. Missing history is visible, with a separate **Start with retained Card context** action. Leaving the page/hiding the viewer only detaches it. X, Command+W and Close terminal instead invoke the unified AppServer close-and-terminate lifecycle. Exited entries leave the Terminal list/tabs; Card and Overview retain history and explicit Resume.

## Profile-only selection acceptance (2026-11-10)

Routing-tag fields, aliases, matching, Board badges and search terms are removed. Regression coverage checks explicit → stage → default selection, unknown explicit references failing without fallback, old Card/Profile JSON loading without the removed field, and rejected tag-bearing Card mutations leaving saved state unchanged. Existing template-role/callback tests still cover independent Developer/Reviewer assignments. Final workspace tests passed (137), including the API-level regression; strict workspace/all-target Clippy and App/Gallery packaging passed. Isolated `mise run acceptance` and `mise run acceptance-dev` passed. The latter verified API level 3, older-service replacement and retained PTY identity at `/private/tmp/cb-dev-start-v08xq969`. The general fixture run is `/private/tmp/cb-mffu3frp/home`.

Native production App checks used disposable data and real Herdr PTYs with deterministic provider commands. Command+Enter created one Card without tags; Task Edit saved the complete title/body. An unassigned Develop stage selected global `codex`, Review selected stage `reviewer`, and an explicit `@claude` selected that Profile with no stage callback, preserving the pending human Review gate. These names identify fixture Profiles, not live model/provider-authentication acceptance. Initial creation evidence is `/private/tmp/cb-routing-native-45x6k312/native-checks.json`; final keyboard-edit/workflow evidence is `/private/tmp/cb-routing-native-0t9ic0ku/native-checks.json`.

Acceptance reproduced a pre-existing Task Edit focus failure: subscribing the structural editor directly to the entire Draft rebuilt inputs during typing and saved only a prefix. `ui/card/task.rs` now selects only editing/expansion mode for that structural owner; text changes update mounted inputs. Final App and 850-point Gallery checks retained full continuous keyboard input and focus. Gallery also verified tag-free creation/editing and Local/GitHub type switching with title/body retained. Test apps and their isolated Cores were stopped; no user data/Core or provider credentials were changed. Chinese IME was excluded.
