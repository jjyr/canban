# Agent profiles and workflow references

Current behavior update, 2026-11-09: [Terminal interaction and navigation](../terminal-interaction.md) supersedes immediate completion exit, indefinite viewer protection and ended-session pages. Completed Agents stay interactive until their configurable idle timeout; actual user input renews activity, exited sessions leave the Terminal list/tabs, and exact Resume lives on the Card.

Multi-machine update, 2026-11-09: see [current execution/location and service contract](../multi-machine.md). Profile commands resolve on their execution host; Resume and Review retain their original workspace, while new work uses the Project's current location.


Status: implemented 2026-11-07; current contract reconciled 2026-11-10. This is the current schema. Earlier provider/Herdr-template/hook proposals are superseded. See [MVP acceptance](../mvp-acceptance.md) for evidence and limits.

## Ownership

Profiles are application-wide configurations in `~/.canban/config/agents.json`. Workflow templates reference their IDs and define stage tasks/transitions. A Card retains one provider conversation for each profile ID. Developer and Reviewer may both use Codex but have independent histories. Neither profile definitions nor workflow templates are live Sessions.

Current extension (2026-11-11): [unified YAML Workflows](workflow-engine.md) reference Profiles from Agent steps independently of Board columns. Original YAML and immutable versions are stored only in SQLite. Project Workflows provides manual editing/history and Edit with agent. Its Project + configuration purpose + Profile conversation reuses command expansion, Scheduler admission and Terminal transport without a business Card. Onboarding/default Profile setup and provider authentication remain separate. See [the implementation contract](workflow-engine.md).

```text
config/agents.json                 Workflow YAML in SQLite
  developer + commands + role <--- Develop.profile
  reviewer  + commands + role <--- Review.profile
                                      |
                                Project snapshot
                                      |
                             Request profile/task snapshot
                                      |
                       generic invocation -> Herdr terminal
```

`canban-profiles` resolves JSON presets, whole-field overrides, and provider-specific launch adaptation. Scheduler/Workflow Engine/AppServer invoke a common interface; business orchestration and Herdr transport do not select Codex/Claude branches. Herdr supplies status, turn tracking, PTY input, attachment and interruption. Canban owns explicit reports/replies and the opaque provider-history binding.

## Per-invocation launch adaptation

All Agent launch paths (Card/comment, independent Terminal, scheduled task,
merge batch, configuration conversation, and Review) call
`canban_profiles::prepare_launch` after command rendering and execution-host
path resolution. It accepts the resolved Profile, argv, cwd, and explicit
invocation environment, returning a transport-ready `LaunchSpec`.
Plain shell/setup launches use the same boundary without a Profile.

```text
AppServer: business context + command rendering + host path resolution
                              |
                  profiles::prepare_launch
                    /                   \
          preset = codex             other / no Profile
          Codex adapter                 pass-through
          per-thread env config
                    \                   /
                     LaunchSpec { argv, env, cwd }
                              |
                   Herdr local/remote PTY
```

Only an explicitly declared `preset: "codex"` opts into Codex CLI adaptation,
including wrappers that accept Codex CLI arguments. Role IDs and executable
basenames do not select the adapter. Untyped commands, other providers, and
opaque `herdr:*` templates retain their existing argv/env.

The Codex adapter retains process env and appends individual
`-c shell_environment_policy.set.KEY=<TOML string>` overrides. First launch and
Resume both use current invocation values, overriding earlier command-line
values. Overrides precede an explicit `--` terminator. No shell interpolation,
user configuration writes, daemon restarts, or prompt changes are involved.
Only explicit invocation overrides are forwarded, not the AppServer's entire
environment or its authentication variables.

The adapter changes only supplied environment values. It preserves user
inheritance, exclusion, and include-filter policies, including legacy Codex
filter configuration. An include allowlist must permit required context.
An omitted key is not cleared: old daemon environment values can still be
inherited, including Card context in a cardless session. Sanitizing old context
is explicitly outside this change and needs a separate compatibility design.
The adapter rejects environment keys containing dots, equals, or NUL because
Codex CLI config-path splitting cannot represent those keys unambiguously.

The built-in preset still defaults to `--no-daemon`; custom Codex commands can
use a shared daemon with this adaptation. Shell configuration does not claim to
set provider-specific MCP server or hook environments, which require separate
provider contracts.

Run `mise run acceptance-agent-launch` for real Codex config parsing and
concurrent/resumed thread shell checks in isolated temporary homes without model
requests. `CANBAN_CODEX_BIN` selects the Codex executable under test. See [dated validation and limits](../agent-launch-acceptance-20261116.md).

## Profile schema

| Field | Meaning |
| --- | --- |
| `id` | Required identity for mentions, workflow references and Card conversation reuse |
| `name` | Optional human-readable display name (e.g. `Antigravity`, `Gemini CLI`) |
| `enabled` | Boolean (default `true`); disabled Profiles are hidden from `@mention` autocomplete/pickers, ignored in prose `@` scans, and rejected on dispatch while preserving historical records |
| `identity` | Boolean marking semantic role identities (`@planner`, `@developer`, `@reviewer`, `@merger`, or custom identities such as `@qa`) |
| `preset` | Optional preset name (any of the 24 built-in Herdr CLI agents, `herdr:<template>`, or custom `config/presets/<name>.json`); omitted fields inherit its defaults |
| `model` | Optional model identifier (e.g. `gemini-4-carbon`, `gpt-6.1-sol`, `opus`); identities inherit their backing CLI Agent's `model` when omitted |
| `effort` | Optional reasoning effort level (e.g. `low`, `medium`, `high`, `xhigh`, `max`); identities inherit their backing CLI Agent's `effort` when omitted |
| `role_instruction` | Role text included separately from the current task; empty by default |
| `argv` | First-launch argument array; must resolve to a nonempty array |
| `resume_argv` | Exact-history command array; absent inherits, `[]` explicitly disables Resume |
| `exit` | `{"type":"manual"}` or `{"type":"input","text":"/exit"}`; no preset defaults to manual |
| `session_instruction` | Optional prompt recipe for binding history; `""` explicitly disables inherited text |

`argv` and `resume_argv` replace the corresponding preset array in full. They are individual arguments, with no implicit shell evaluation/appending. Supported placeholders are `{prompt}`, `{home}`, `{cwd}`, `{project_dir}`, `{cli}`, `{session_id}`, `{model}`, `{effort}`, and `{agy_trusted_settings}`. When `model` or `effort` is configured on a Profile and the command template does not already embed `{model}` / `{effort}`, `canban-profiles::command()` prepends the catalog entry's CLI flag template (such as `-m <model>` and `-c model_reasoning_effort="<effort>"` for Codex, `--model <model>` and `--effort <effort>` for Claude Code and Antigravity, `-m <model>` and `--variant <effort>` for OpenCode) right after `argv[0]`, and deduplicates repeated `--add-dir` pairs. A fresh launch referencing `{session_id}` gets a preallocated UUID; otherwise the Agent binds its actual ID. Saved IDs are opaque; Canban never substitutes a Herdr terminal address or a latest-history search.

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

The SQL bootstrap creates visible default YAML with non-executing Backlog, Develop using `developer`, movement to Review using `reviewer`, and a human decision after its report. Historical Stage callback fields remain compatibility snapshots. Review means an ordinary report, followed by human disposition.

Selection order is explicit `@profile`, stage `profile`, then global `default_agent`. Routing tags were removed on 2026-11-10: neither Cards nor Profiles expose a `tags` field. Old JSON fields are ignored on decode without a dedicated migration; raw configuration files and historical records are not proactively rewritten. New `card.create`/`card.update` calls reject `tags`, including empty arrays. Desktop API level 3 rejects an older AppServer that could still apply tag-based routing; `mise run run` starts a matched UI/service build. Explicit unknown Profile IDs fail instead of falling back. Direct mentions do not change the workflow callback. Startup, Project creation and reload validate profile references before activating the new registry.

## Migration

Recognized legacy provider/batch configuration is converted once. Canban first saves the exact original bytes as `config/agents.legacy-<uuid>.json`, then atomically publishes validated JSON. Known provider launch/resume defaults are supplied through presets. Historical Request snapshots and conversations are not rewritten. A legacy opaque Herdr `template` is rejected with instructions to supply explicit commands; the original config stays intact. Herdr launch-template mode is no longer a Profile execution path. Kanban workflow templates are unaffected.

## Herdr catalog, onboarding, custom identities, and Project overrides (2026-11-12)

- **Built-in Herdr Agent catalog & host probing (`agent.catalog`)**: `canban-profiles::builtin_catalog()` ships interactive CLI presets for all 24 Herdr built-in agents (`codex`, `claude`, `agy`, `gemini`, `opencode`, `cursor` [`cursor-agent`], `pi`, `omp`, `copilot`, `droid`, `amp`, `kimi`, `kiro` [`kiro-cli`], `qwen`, `devin`, `cline`, `grok`, `hermes`, `kilo`, `qodercli`, `letta`, `mastracode`, `maki`, `muse`), plus dynamic `herdr:<template>` entries from `templates.list`, user preset files (`config/presets/*.json`), and custom `argv` CLI agents. `Host::probe_binaries` detects installed executables on `local` or any remote SSH `machine`.
- **Onboarding Wizard (`agent-setup`, `agent.setup`)**: Shown as a setup card on an empty un-onboarded Board (`!config.onboarded`) and re-runnable anytime from `Settings -> Agents & Identities -> Run Agent Setup…`. Users choose which Agents are enabled (`enabled: true`), pick a Primary Agent or per-role mapping for `@planner`, `@developer`, `@reviewer`, and `@merger`, and persist `onboarded: true` while preserving custom identities and custom `role_instruction`s. Existing `agents.json` configurations that already define `@developer`, `@reviewer`, and `@planner` automatically receive `@merger` (inheriting `@developer`'s backing Agent) on startup migration.
- **Enabled-Agent gating & `@mention` isolation**: `ui.profiles` contains only `enabled` profiles ordered `(default_agent, identities, direct_agents)`. Disabled profiles do not appear in `@` completion, Card/Review/Workflow pickers, or prose `@mention` addressing, and explicit dispatch returns `Agent @<id> is disabled`.
- **Default interactive identity (`@developer`) & Custom Identities (`config.agents.save`)**: `config.default_agent` defaults to `"developer"`, pre-selects the interactive recipient in Card `@` and Diff & Review, and directly dispatches `Edit with agent` in Project Workflows without a secondary Agent picker. Users can create, edit (`engine` / `role_instruction` / `enabled`), and delete custom identities (such as `@qa` or `@architect`) directly in `Settings -> Agents & Identities`.
- **Per-Project identity overrides (`Project.agent_overrides`, `project.agents`, `project-agents`)**: Each Project can override any role `@identity`'s backing Agent, `model`, and/or `effort` (`BTreeMap<role_id, ProjectAgentOverride>`, backward-compatible with legacy `"role": "agent"` JSON strings; direct CLI Agents are global-only) from the Board header (`Agents`), Project `⋯` menu (`Project Agents…`), Workflow Studio header (`Project Agents`), or `New Project` (`▸ Project Agents`), keeping the identity's `id` and `role_instruction` without editing Workflow YAML. Resolution precedence is `Project identity override` -> `Global @identity` -> `Global backing CLI Agent` -> `CLI built-in default`; switching an identity to a different backing Agent replaces unoverridden cross-provider `model` and `effort` values with the target Agent's global defaults unless explicitly overridden on the Project.
- **Antigravity (`agy`) workspace directories & session trust, plus global `model` / `effort` selectors (2026-11-13)**: The built-in `agy` preset passes deduplicated `--add-dir {home} --add-dir {cwd} --add-dir {project_dir}` together with session-scoped `--settings '{"trustedWorkspaces":[...]}'` (including canonicalized paths) so Card Git worktrees and the parent Project `.git` directory are accessible without triggering interactive trust prompts or mutating `~/.gemini/antigravity-cli/settings.json`. `agent.catalog` probes live `agy models` (with a bounded cache/timeout) and local Codex model caches alongside curated catalog defaults, and `Settings -> Agents & Identities` provides global `Model` (dropdown + custom model ID input fallback) and `Effort` selectors on both Role Identities and Enabled CLI Agents (`config.agents.save` `op: "set_profile_options"`). Desktop API version is bumped to `16` (`DESKTOP_API = 16`).
- **Built-in Canban CLI Skill & global `⌘T` Terminal shortcut (2026-11-14)**: `skills/canban/SKILL.md` is embedded in `canban-context`, materialized to `<CANBAN_HOME>/skills/canban/SKILL.md` when `AppServer` opens (and staged to remote homes), and exposed via `canban skill` / `canban --skill`. An optional **Canban CLI Skill** setup prompt appears below **Default Agent** in both `agent-setup` and `Settings -> Agents & Identities` with `npx skills add <CANBAN_HOME>/skills --skill canban -g`, **Copy**, and **Open Terminal (`⌘T`)**, backed by the global `new-terminal` (`⌘T`) shortcut (`DESKTOP_API = 32`).
- **Per-model effort capability detection & CLI flag filtering (2026-11-14, updated 2026-11-18)**: `ModelOption.supported_efforts` carries optional per-model reasoning effort levels (`None` falls back to `CatalogEntry.supported_efforts`, `Some([])` marks effort unsupported, and `Some(levels)` narrows to exact supported tiers). `agy` models collapse multi-variant `-{low|medium|high|xhigh|max}` entries from `agy models` into base-model families (e.g. `gemini-4-carbon` with `low|medium|high` and `gemini-4-hydrogen` with `low|medium|high|xhigh`, passing `--model <base> --effort <tier>`) while keeping standalone orchestrators (`gemini-pro-orchestrator`, `gemini-pro-orchestrator-argon`) and single fixed-tier entries (`gpt-oss-120b-medium`) at `Some([])` (no `--effort`); custom/unlisted orchestrator or already-suffixed `agy` slugs also omit `--effort`. Codex models read `supported_reasoning_levels` from `~/.codex/models_cache.json`, and Claude Haiku marks `Some([])`. Both `Settings -> Agents & Identities` and `Project / Agents` hide the Effort selector (and clear stale explicit effort overrides when switching to an incompatible model) when a model does not support effort, and `canban_profiles::command()` omits `--effort` / `model_reasoning_effort` flags when the effective model does not support the configured or inherited effort (`DESKTOP_API = 67`).
- **Collapsible Agent `Command` + `Argv` overrides in Settings (2026-11-14)**: Each enabled Agent row in `Settings -> Agents & Identities -> Agent defaults` exposes a collapsed-by-default `▸ Command` disclosure (`Custom` badge when overridden) with two monospace inputs: **Command** (`argv[0]`, e.g. `agy` or `/bin/agy-modified`) and **Argv** (`argv[1..]` formatted and parsed with POSIX quote word splitting via `canban_profiles::{split_posix_words, join_posix_words}`, without shell evaluation), a template placeholder hint (`{prompt} {cwd} {project_dir} {home} {session_id} {model} {effort} {cli} {agy_trusted_settings}`), **Reset**, and **Save** (`config.agents.save` `op: "set_profile_options"` `argv`). `+ Custom CLI` shares the same **Command** + **Argv** editor. When a preset-backed Agent overrides `argv[0]` without separately overriding `resume_argv`, `canban-profiles` automatically synchronizes `resume_argv[0]` to the custom executable, and `@identities` referencing that Agent inherit its customized `argv` and synced `resume_argv` (`DESKTOP_API = 47`).

## UI

Settings (`Agents & Identities`) manages default interactive Agent, role identities, Herdr CLI agent enablement, collapsible per-Agent `Command` + `Argv` overrides, custom CLI agents, and `Run Agent Setup…`; Board headers expose `Agents` (`project-agents`) alongside `Workflows`; Card comments offer enabled mentions ordered with `default_agent` first. Clicking a live Card Agent attaches to its terminal. Clicking an exited Agent resumes its exact conversation in a new terminal. Missing history is visible, with a separate **Start with retained Card context** action. Leaving the page/hiding the viewer only detaches it. X, Command+W and Close terminal instead invoke the unified AppServer close-and-terminate lifecycle. Exited entries leave the Terminal list/tabs; Card and Overview retain history and explicit Resume.

## Profile-only selection acceptance (2026-11-10)

Routing-tag fields, aliases, matching, Board badges and search terms are removed. Regression coverage checks explicit → stage → default selection, unknown explicit references failing without fallback, old Card/Profile JSON loading without the removed field, and rejected tag-bearing Card mutations leaving saved state unchanged. Existing template-role/callback tests still cover independent Developer/Reviewer assignments. Final workspace tests passed (137), including the API-level regression; strict workspace/all-target Clippy and App/Gallery packaging passed. Isolated `mise run acceptance` and `mise run acceptance-dev` passed. The latter verified API level 3, older-service replacement and retained PTY identity at `/private/tmp/cb-dev-start-v08xq969`. The general fixture run is `/private/tmp/cb-mffu3frp/home`.

Native production App checks used disposable data and real Herdr PTYs with deterministic provider commands. Command+Enter created one Card without tags; Task Edit saved the complete title/body. An unassigned Develop stage selected global `codex`, Review selected stage `reviewer`, and an explicit `@claude` selected that Profile with no stage callback, preserving the pending human Review gate. These names identify fixture Profiles, not live model/provider-authentication acceptance. Initial creation evidence is `/private/tmp/cb-routing-native-45x6k312/native-checks.json`; final keyboard-edit/workflow evidence is `/private/tmp/cb-routing-native-0t9ic0ku/native-checks.json`.

Acceptance reproduced a pre-existing Task Edit focus failure: subscribing the structural editor directly to the entire Draft rebuilt inputs during typing and saved only a prefix. `ui/card/task.rs` now selects only editing/expansion mode for that structural owner; text changes update mounted inputs. Final App and 850-point Gallery checks retained full continuous keyboard input and focus. Gallery also verified tag-free creation/editing and Local/GitHub type switching with title/body retained. Test apps and their isolated Cores were stopped; no user data/Core or provider credentials were changed. Chinese IME was excluded.
