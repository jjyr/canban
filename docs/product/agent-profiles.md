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
| `tags` | Routing aliases; empty by default |

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
      "role_instruction": "Implement the agreed Card task and explain validation.",
      "tags": ["developer"]
    },
    {
      "id": "reviewer", "preset": "codex",
      "role_instruction": "Review actual code and tests; publish an ordinary report.",
      "tags": ["reviewer"]
    },
    {
      "id": "my-agent", "preset": null,
      "role_instruction": "Publish explicit Canban outcomes.",
      "argv": ["/absolute/path/to/wrapper", "start", "{prompt}"],
      "resume_argv": ["/absolute/path/to/wrapper", "resume", "{session_id}", "{prompt}"],
      "session_instruction": "Bind your actual history using canban bind-session.",
      "exit": {"type":"manual"},
      "tags": ["custom"]
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

Selection order is explicit `@profile`, stage `profile`, Card tags, then `default_agent`. Unmatched/ambiguous tags request attention. Automatic named stage profiles are not overridden by Card tags. Direct mentions do not retag the Card or change its workflow callback. Startup, Project creation and reload validate profile references before activating the new registry.

## Migration

Recognized legacy provider/batch configuration is converted once. Canban first saves the exact original bytes as `config/agents.legacy-<uuid>.json`, then atomically publishes validated JSON. Known provider launch/resume defaults are supplied through presets. Historical Request snapshots and conversations are not rewritten. A legacy opaque Herdr `template` is rejected with instructions to supply explicit commands; the original config stays intact. Herdr launch-template mode is no longer a Profile execution path. Kanban workflow templates are unaffected.

## UI

Settings shows resolved profiles and reload controls; Board headers show stage names/counts with automatic roles in their tooltip; Card comments offer the same configured mentions. Clicking a live Card Agent attaches to its terminal. Clicking an exited Agent resumes its exact conversation in a new terminal. Missing history is visible, with a separate **Start with retained Card context** action. Leaving the page/hiding the viewer only detaches it. X, Command+W and Close terminal instead invoke the unified AppServer close-and-terminate lifecycle. Exited entries leave the Terminal list/tabs; Card and Overview retain history and explicit Resume.
