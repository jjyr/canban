# Agent profiles and workflow references

Status: implemented 2026-11-07, including Card/profile conversation binding, interactive launch/resume and native terminals. Codex has real-model acceptance; Claude has adapter regression coverage, pending live authentication/model acceptance.

## Ownership

Profiles are application-wide reusable configurations in ~/.canban/config/agents.json. Canban supplies developer/reviewer starter profiles on a fresh home, along with codex/claude for direct mentions. Users customize them through JSON. A profile has a stable id, fixed role_instruction, launch argv or an existing Herdr template, and optional routing tags. Model/CLI options live in that argv/template. A profile ID identifies a role/configuration, not a provider brand or terminal.

Workflow JSON contains stage tasks/transitions and references profile IDs. Two profiles can execute the same Codex binary with different instructions. They remain distinct roles in requests and history. Project workflows are saved snapshots; editing the default workflow only affects new Projects.

```text
agents.json                       workflow default.json
  developer + role instruction <-- Develop.profile
  reviewer  + role instruction <-- Review.profile
                                         |
                                    Project snapshot
                                         |
                              request profile/task snapshot
                                         |
                               captured prompt + context
```

## Actual JSON fields

A runnable interactive configuration example:

```json
{
  "hc": "hc",
  "hc_session": "default",
  "hc_env": {},
  "default_agent": "developer",
  "profiles": [
    {
      "id": "developer",
      "role_instruction": "Implement the agreed Card task and explain validation.",
      "argv": ["codex", "--no-daemon", "--sandbox", "workspace-write", "--add-dir", "{home}", "{prompt}"],
      "tags": ["developer"]
    },
    {
      "id": "reviewer",
      "role_instruction": "Review actual changes critically. Report findings without implementing unrelated changes.",
      "argv": ["codex", "--no-daemon", "--sandbox", "workspace-write", "--add-dir", "{home}", "{prompt}"],
      "tags": ["reviewer"]
    }
  ]
}
```

Preserve other profiles, custom options and hc settings. provider is optional: codex/claude are inferred from the executable basename; explicitly set it for wrappers. codex receives an invocation-local notify command and exact-ID resume arguments. claude receives per-invocation Stop settings, --session-id for creation and --resume for reuse. custom requires resume_argv with {session_id}; command is an explicit one-shot compatibility mode. Opaque Herdr templates are supported in command mode, because their provider lifecycle cannot be inferred safely. Existing JSON is not rewritten; legacy exec/-p arguments are adapted in memory for recognized interactive providers.

Matching workflow example for config/workflows/default.json:

```json
{
  "name": "Development",
  "stages": [
    {
      "id": "backlog", "name": "Backlog", "execute": false,
      "profile": null, "instruction": "", "on_completed": null
    },
    {
      "id": "develop", "name": "Develop", "execute": true,
      "profile": "developer", "instruction": "Implement this Card.",
      "on_completed": "review"
    },
    {
      "id": "review", "name": "Review", "execute": true,
      "profile": "reviewer", "instruction": "Produce an ordinary Markdown review report.",
      "on_completed": null
    }
  ]
}
```

role_instruction defaults to empty for old profiles; profile defaults to null for old stages. The fixed role and the stage/comment task are separate sections in the captured prompt. The context manifest also retains the exact queued profile snapshot, so later configuration edits do not rewrite an earlier execution's role.

## Selection and validation

Selection order:

1. Explicit @profile: select that exact registry entry; its response has no stage-transition callback and does not change Card tags.
2. Automatic stage.profile: select that exact entry. Card tags do not override this named role.
3. No stage reference: use Card routing tags; unmatched/ambiguous tags request attention.
4. No reference or tags: use default_agent.

Startup, Project creation and configuration reload validate profile references. A reference must resolve to a registered profile; blank references or profile references on non-executing stages are invalid. Default workflow and saved Project references are checked before activating a reloaded registry. Removal of an in-use profile fails with an error and preserves the previous active configuration. Already queued requests keep their own snapshots.

## Existing data and UI

Existing agents.json, default workflow JSON, saved Projects and queued requests are not rewritten. If an older/custom registry lacks starter roles and its default workflow file is absent, the generated workflow omits those references and preserves tag/default routing instead of injecting profiles. An existing workflow file without profile fields continues its previous behavior.

Settings shows the global registry and role instructions with JSON paths/reload. Board column headers show the named stage profile or tag/default fallback. Card comments offer Ask @profile buttons backed by the configured registry. These actions resolve the Card/profile conversation, append to a live invocation or resume an exited one. Profile edits are loaded for later invocations; running/queued snapshots remain immutable.

## Validation

- Workspace tests cover stage/mention role and callback selection, profile-removal rejection, missing-reference rejection before directory creation, immutable queued role/task captures, and old registry compatibility.
- Model-free acceptance through real hc 0.1.18 passed using separate developer/reviewer fixture profiles and verified their actual prompt roles, terminal bindings, Develop -> Review, mentions/attention, Archive/Restore, restart and unrestricted three-Board execution. Evidence home: /private/tmp/cb-684xomr_/home; core: 9563f908e2e5e1932efb79376e8289cf.
- Native acceptance used a separate test bundle/home at /private/tmp/cb-profiles-ui-vucz0i89. Board profile labels, inserting @reviewer from the production picker, submitting the question through the actual composer, a completed Reviewer fixture report and unchanged Backlog stage were verified. Settings exposed the same global registry/roles. Normal ~/.canban data was not changed.

These checks used deterministic fixture Agents for new routing coverage. Earlier real-Codex acceptance remains separate evidence; it was not repeated for this configuration change.
