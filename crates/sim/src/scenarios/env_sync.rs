//! Scenario #5 — Global Environment & Secrets synchronization under clock skew,
//! real competing LWW writes across skewed hosts, `Unset` project-scope overrides (`env.preview`),
//! network partitions, secret rotation, deletion tombstones, and zero plaintext secret leaks.
use crate::{
    exec::SimShellOutcome,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{EnvEntryMeta, EnvPreviewRow, EnvWriteEntry, Profile};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

fn collect_all_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.is_dir() {
            collect_all_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

const ENV_MASK_WORKFLOW_YAML: &str = r#"schema: 12
id: env-mask-wf
name: Env Masking Workflow
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:codex_check
    match:
      column: develop
steps:
  - id: agent:codex_check
    uses: agent
    env:
      WF_SEC_REF: "${{ secrets.P_SEC }}"
    with:
      profile: codex
      instruction: Check launch isolation
    next: script:ok
  - id: script:ok
    uses: script
    env:
      EXPANDED_P: "${{ secrets.P_SEC }}"
      EXPANDED_G: "${{ secrets.G_SEC }}"
      EXPANDED_V: "${{ vars.G_MODE }}-${{ vars.P_ENV }}"
    run: "sim-env-ok"
    next: script:fail
  - id: script:fail
    uses: script
    env:
      FAIL_SEC: "${{ secrets.P_SEC }}"
    run: "sim-env-fail"
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#;

const WARM_PTY_WORKFLOW_YAML: &str = r#"schema: 12
id: warm-pty-wf
name: Warm PTY Workflow
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:dev
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: agent:dev
    uses: agent
    with:
      profile: developer
      instruction: Implement task
    next: __end
  - id: agent:review
    uses: agent
    with:
      profile: developer
      instruction: Review task with rotated secret
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "env_sync");
    cfg.host_clock_skew_ms.insert("remote-a".into(), -15_000);
    cfg.host_clock_skew_ms.insert("remote-b".into(), 20_000);

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let secret_v1 = format!("sk-sim-topsecret-v1-{seed}");
        let secret_v2 = format!("sk-sim-rotated-v2-{seed}");
        oracle.track_secret(secret_v1.clone());
        oracle.track_secret(secret_v2.clone());

        // 1. Initial global env + secret save on `local` -> converges to both remotes.
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v1" },
                    "SHARED_LWW": { "kind": "plain", "value": "initial" },
                    "API_TOKEN": { "kind": "secret", "value": secret_v1 },
                }
            }),
            None,
        )?;
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                snap.global_env.get("SHARED_PLAIN")
                    == Some(&EnvEntryMeta::Plain { value: "v1".into() }),
                "Host {host} missing SHARED_PLAIN=v1: {:?}",
                snap.global_env
            );
            ensure!(
                matches!(
                    snap.global_env.get("API_TOKEN"),
                    Some(EnvEntryMeta::Secret { .. })
                ),
                "Host {host} missing masked API_TOKEN secret: {:?}",
                snap.global_env
            );
            oracle.record_committed_env(
                host,
                "SHARED_PLAIN",
                EnvEntryMeta::Plain { value: "v1".into() },
            );
        }

        // 2. Real competing writes across skewed hosts (`remote-b` is +20s ahead of `local`):
        //    Partition `remote-b`, write `SHARED_LWW = "remote-b-newer"` directly on `remote-b`
        //    at `local_now + 20s`, and write `SHARED_LWW = "local-older"` on `local` at `local_now`.
        //    When reconnected, LWW (`rec.updated_at < existing_ts`) MUST protect `remote-b`'s newer
        //    write against `local`'s older timestamp!
        world.partition("ui-client", "remote-b");
        world.advance(Duration::from_millis(120))?;
        oracle.mark_secret_queued_offline();

        world.server_call(
            "remote-b",
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v1" },
                    "SHARED_LWW": { "kind": "plain", "value": "remote-b-newer" },
                    "API_TOKEN": { "kind": "secret", "keep_existing": true },
                }
            }),
        )?;
        oracle.record_committed_env(
            "remote-b",
            "SHARED_LWW",
            EnvEntryMeta::Plain {
                value: "remote-b-newer".into(),
            },
        );
        for host in ["local-server", "remote-a", "remote-b"] {
            oracle.remove_committed_env(host, "SHARED_PLAIN");
        }
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v2-rotated" },
                    "SHARED_LWW": { "kind": "plain", "value": "local-older" },
                    "API_TOKEN": { "kind": "secret", "value": secret_v2 },
                }
            }),
            None,
        )?;
        oracle.record_committed_env(
            "local-server",
            "SHARED_PLAIN",
            EnvEntryMeta::Plain {
                value: "v2-rotated".into(),
            },
        );
        world.client_sync_global_env_now()?;
        oracle.check_step_invariants(world)?;

        // Heal link and let global env sync push `local`'s older write to `remote-b`.
        world.heal_and_quiesce(20_000)?;
        let b_snap = world.server("remote-b").unwrap().snapshot()?;
        ensure!(
            b_snap.global_env.get("SHARED_PLAIN")
                == Some(&EnvEntryMeta::Plain {
                    value: "v2-rotated".into(),
                }),
            "remote-b did not receive SHARED_PLAIN=v2-rotated after heal: {:?}",
            b_snap.global_env
        );
        // Confirm LWW protected `remote-b`'s newer timestamp on `SHARED_LWW`!
        ensure!(
            b_snap.global_env.get("SHARED_LWW")
                == Some(&EnvEntryMeta::Plain {
                    value: "remote-b-newer".into(),
                }),
            "LWW violation: stale local push overwrote newer remote-b write! got {:?}",
            b_snap.global_env.get("SHARED_LWW")
        );
        for host in ["remote-a", "remote-b"] {
            oracle.record_committed_env(
                host,
                "SHARED_PLAIN",
                EnvEntryMeta::Plain {
                    value: "v2-rotated".into(),
                },
            );
        }
        world.hit("env_sync.lww_stale_rejected");

        // Now advance virtual clock past `remote-b`'s +20s skew so `local` issues a strictly newer
        // timestamp and ALL hosts converge identically!
        world.advance(Duration::from_secs(30))?;
        oracle.remove_committed_env("remote-b", "SHARED_LWW");
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v2-rotated" },
                    "SHARED_LWW": { "kind": "plain", "value": "local-newest" },
                    "API_TOKEN": { "kind": "secret", "value": secret_v2 },
                }
            }),
            None,
        )?;
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                snap.global_env.get("SHARED_LWW")
                    == Some(&EnvEntryMeta::Plain {
                        value: "local-newest".into(),
                    }),
                "Host {host} failed to converge to local-newest: {:?}",
                snap.global_env.get("SHARED_LWW")
            );
            oracle.record_committed_env(
                host,
                "SHARED_LWW",
                EnvEntryMeta::Plain {
                    value: "local-newest".into(),
                },
            );
        }
        world.hit("env_sync.lww_converged");

        // 3. Project-scope `Unset` override + `env.preview` provenance & secret masking check.
        let dir = world.project_dir("env-unset")?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "local", "name": "Env Override Proj", "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "project",
                "target_id": pid,
                "entries": {
                    "SHARED_LWW": { "kind": "unset" },
                    "PROJECT_ONLY": { "kind": "plain", "value": "from-project" },
                }
            }),
            Some(&pid),
        )?;
        let preview = world.client_call(
            "env.preview",
            json!({ "project_id": pid, "profile_id": "developer" }),
            Some(&pid),
        )?;
        let rows: Vec<EnvPreviewRow> = serde_json::from_value(preview["rows"].clone())?;
        let unset_row = rows
            .iter()
            .find(|r| r.key == "SHARED_LWW")
            .expect("SHARED_LWW missing from env.preview");
        ensure!(
            unset_row.is_unset,
            "Project-scope Unset override must mark SHARED_LWW as is_unset"
        );
        let proj_row = rows
            .iter()
            .find(|r| r.key == "PROJECT_ONLY")
            .expect("PROJECT_ONLY missing from env.preview");
        ensure!(proj_row.source_layer == "Project");
        let sec_row = rows
            .iter()
            .find(|r| r.key == "API_TOKEN")
            .expect("API_TOKEN missing from env.preview");
        ensure!(
            sec_row.is_secret && sec_row.configured && !sec_row.display_value.contains(&secret_v2),
            "env.preview leaked secret plaintext!"
        );
        world.hit("env_sync.project_unset_override");

        // 4. Tombstone deletion across partition + `remote-b` crash/bounce.
        for host in ["local-server", "remote-a", "remote-b"] {
            oracle.remove_committed_env(host, "SHARED_PLAIN");
            oracle.remove_committed_env(host, "SHARED_LWW");
        }
        world.partition("ui-client", "remote-b");
        world.advance(Duration::from_millis(120))?;
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v3-final" },
                }
            }),
            None,
        )?;
        oracle.record_committed_env(
            "local-server",
            "SHARED_PLAIN",
            EnvEntryMeta::Plain {
                value: "v3-final".into(),
            },
        );
        world.client_sync_global_env_now()?;
        world.crash_server("remote-b")?;
        world.advance(Duration::from_millis(200))?;
        world.bounce_server("remote-b")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                !snap.global_env.contains_key("API_TOKEN"),
                "Host {host} failed to apply tombstone deletion for API_TOKEN"
            );
            ensure!(
                !snap.global_env.contains_key("SHARED_LWW"),
                "Host {host} failed to apply tombstone deletion for SHARED_LWW"
            );
            ensure!(
                snap.global_env.get("SHARED_PLAIN")
                    == Some(&EnvEntryMeta::Plain {
                        value: "v3-final".into(),
                    }),
                "Host {host} did not converge to SHARED_PLAIN=v3-final"
            );
            oracle.record_committed_env(
                host,
                "SHARED_PLAIN",
                EnvEntryMeta::Plain {
                    value: "v3-final".into(),
                },
            );
        }
        oracle.check_committed_durability(world)?;

        // 5. Env layering precedence (`Global < Agent < Identity < Project < Workflow`),
        //    `Unset` overrides, `CANBAN_*` core-var protection, `project.agents` backing-engine
        //    override (`developer -> claude`), script stdout/stderr/`$CANBAN_OUTPUT` masking,
        //    write-only secret isolation scan, warm-PTY invalidation on secret rotation, and
        //    `replace_profiles` preserving SQLite agent env & secrets!
        let local_srv = world.server("local").context("local server missing")?;
        let local_home = world.servers["local-server"].home.clone();
        let exec_local = world
            .exec_runner("local")
            .context("local SimExec unavailable")?;
        let herdr_local = world.herdr("local").context("local SimHerdr unavailable")?;

        let layer_dir = world.project_dir(&format!("env-layers-{seed}"))?;
        let layer_proj_val = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Env Layers {seed}"),
                "directory": layer_dir,
            }),
            None,
        )?;
        let layer_pid = layer_proj_val["id"].as_str().unwrap().to_owned();
        let layer_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": layer_pid }),
            Some(&layer_pid),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": layer_pid,
                "revision": layer_sources["revision"],
                "sources": [{ "filename": "env-mask-wf.yaml", "source": ENV_MASK_WORKFLOW_YAML }],
            }),
            Some(&layer_pid),
        )?;

        // Ensure `@developer` backs onto `codex` initially.
        let _ = world.server_call(
            "local",
            "config.agents.save",
            json!({
                "op": "upsert_identity",
                "id": "developer",
                "engine": "codex",
                "enabled": true,
            }),
        )?;

        let sec_global = format!("GLOBAL_SECRET_SENTINEL_9f8e7d_{seed}");
        let sec_codex_ov = format!("CODEX_OVERRIDE_SENTINEL_1122_{seed}");
        let sec_proj = format!("PROJ_SECRET_SENTINEL_1a2b3c_{seed}");
        let sec_agent = format!("AGENT_SECRET_SENTINEL_4d5e6f_{seed}");
        let sec_codex = format!("CODEX_SECRET_SENTINEL_7c8b9a_{seed}");
        let tok_warm_v1 = format!("tok_warm_initial_111111_{seed}");
        let tok_warm_v2 = format!("tok_warm_rotated_222222_{seed}");
        for s in [
            &sec_global,
            &sec_codex_ov,
            &sec_proj,
            &sec_agent,
            &sec_codex,
            &tok_warm_v1,
            &tok_warm_v2,
        ] {
            oracle.track_secret(s.clone());
        }

        // Global, Agent (`codex`/`claude`), Identity (`developer`), and Project layers.
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "global",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("SHARED_PLAIN".into(), EnvWriteEntry::Plain { value: "v3-final".into() }),
                    ("GLOBAL_ONLY".into(), EnvWriteEntry::Plain { value: "g".into() }),
                    ("SHARED".into(), EnvWriteEntry::Plain { value: "global".into() }),
                    ("G_MODE".into(), EnvWriteEntry::Plain { value: "prod".into() }),
                    ("SEC_GLOBAL".into(), EnvWriteEntry::Secret { value: Some(sec_global.clone()), keep_existing: false }),
                    ("G_SEC".into(), EnvWriteEntry::Secret { value: Some(sec_global.clone()), keep_existing: false }),
                    ("CANBAN_PROJECT".into(), EnvWriteEntry::Plain { value: "hacked-global".into() }),
                ]),
            }),
        )?;
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "agent",
                "target_id": "codex",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("CLI_ONLY".into(), EnvWriteEntry::Plain { value: "codex".into() }),
                    ("SHARED".into(), EnvWriteEntry::Plain { value: "codex".into() }),
                    ("CLI_OR_ROLE".into(), EnvWriteEntry::Plain { value: "cli".into() }),
                    ("CLI_UNSET_AT_PROJ".into(), EnvWriteEntry::Plain { value: "keep".into() }),
                    ("SEC_GLOBAL".into(), EnvWriteEntry::Secret { value: Some(sec_codex_ov.clone()), keep_existing: false }),
                    ("A_SEC".into(), EnvWriteEntry::Secret { value: Some(sec_agent.clone()), keep_existing: false }),
                    ("CODEX_SEC".into(), EnvWriteEntry::Secret { value: Some(sec_codex.clone()), keep_existing: false }),
                ]),
            }),
        )?;
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "agent",
                "target_id": "claude",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("CLAUDE_ONLY".into(), EnvWriteEntry::Plain { value: "claude".into() }),
                    ("SHARED".into(), EnvWriteEntry::Plain { value: "claude".into() }),
                ]),
            }),
        )?;
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "agent",
                "target_id": "developer",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("ROLE_ONLY".into(), EnvWriteEntry::Plain { value: "role".into() }),
                    ("SHARED".into(), EnvWriteEntry::Plain { value: "role".into() }),
                    ("CLI_OR_ROLE".into(), EnvWriteEntry::Unset),
                ]),
            }),
        )?;
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "project",
                "target_id": layer_pid,
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("PROJECT_ONLY".into(), EnvWriteEntry::Plain { value: "proj".into() }),
                    ("SHARED".into(), EnvWriteEntry::Plain { value: "proj".into() }),
                    ("P_ENV".into(), EnvWriteEntry::Plain { value: "staging".into() }),
                    ("P_SEC".into(), EnvWriteEntry::Secret { value: Some(sec_proj.clone()), keep_existing: false }),
                    ("CLI_UNSET_AT_PROJ".into(), EnvWriteEntry::Unset),
                    ("CANBAN_HOME".into(), EnvWriteEntry::Unset),
                ]),
            }),
        )?;

        // `vault.key` must have restrictive `0600` Unix permissions.
        let key_mode = std::fs::metadata(local_home.join("vault.key"))?
            .permissions()
            .mode()
            & 0o777;
        ensure!(
            key_mode == 0o600,
            "Expected vault.key permissions 0600, got {key_mode:#o}"
        );

        // `env.preview` provenance + secret desensitization across all layers.
        let layer_preview = world.client_call(
            "env.preview",
            json!({ "project_id": layer_pid, "profile_id": "developer" }),
            Some(&layer_pid),
        )?;
        let preview_rows: Vec<EnvPreviewRow> =
            serde_json::from_value(layer_preview["rows"].clone())?;
        let find_row = |k: &str| {
            preview_rows
                .iter()
                .find(|r| r.key == k)
                .unwrap_or_else(|| panic!("Missing env.preview row {k}"))
        };
        ensure!(find_row("SHARED").source_layer == "Project");
        ensure!(find_row("ROLE_ONLY").source_layer == "Identity (@developer)");
        ensure!(find_row("CLI_ONLY").source_layer == "Agent (@codex)");
        ensure!(find_row("GLOBAL_ONLY").source_layer == "Global");
        ensure!(find_row("CLI_OR_ROLE").is_unset);
        ensure!(find_row("CLI_UNSET_AT_PROJ").is_unset);
        let sec_prev = find_row("SEC_GLOBAL");
        ensure!(
            sec_prev.is_secret
                && sec_prev.configured
                && sec_prev.source_layer == "Agent (@codex)"
                && !sec_prev.display_value.contains(&sec_codex_ov),
            "SEC_GLOBAL preview mismatch or leak: {sec_prev:?}"
        );

        // `resolve_runtime_env`: all 10 protected `CANBAN_*` keys stay immutable & never unset!
        {
            let cfg = local_srv.config();
            let dev_prof = cfg.profiles.iter().find(|p| p.id == "developer").unwrap();
            let project_obj = local_srv
                .snapshot()?
                .projects
                .into_iter()
                .find(|p| p.id == layer_pid)
                .unwrap();
            let protected = BTreeMap::from([
                ("CANBAN_CLI".into(), "/usr/local/bin/canban".into()),
                ("CANBAN_ENDPOINT".into(), "/tmp/app.sock".into()),
                (
                    "CANBAN_HOME".into(),
                    local_home.to_string_lossy().into_owned(),
                ),
                ("CANBAN_PROJECT".into(), layer_pid.clone()),
                ("CANBAN_CARD".into(), "42".into()),
                ("CARD_ID".into(), "card-uuid-42".into()),
                ("CANBAN_RUN_DIR".into(), "/tmp/run-dir".into()),
                ("CANBAN_SESSION_ID".into(), "sess-1".into()),
                ("CANBAN_PROFILE_ID".into(), "developer".into()),
                ("CANBAN_SOURCE_TRUST".into(), "local".into()),
            ]);
            let wf_env = BTreeMap::from([
                ("WF_FROM_VAR".into(), "${{ vars.SHARED }}".into()),
                ("WF_FROM_SEC".into(), "${{ secrets.SEC_GLOBAL }}".into()),
                ("SHARED".into(), "step".into()),
            ]);
            let resolved = local_srv.resolve_runtime_env(
                Some(dev_prof),
                Some(&project_obj),
                &protected,
                Some(&wf_env),
                Some("local"),
            )?;
            ensure!(resolved.env.get("SHARED").map(String::as_str) == Some("step"));
            ensure!(resolved.env.get("WF_FROM_VAR").map(String::as_str) == Some("proj"));
            ensure!(
                resolved.env.get("WF_FROM_SEC").map(String::as_str) == Some(sec_codex_ov.as_str())
            );
            ensure!(resolved.env.get("ROLE_ONLY").map(String::as_str) == Some("role"));
            ensure!(resolved.env.get("CLI_ONLY").map(String::as_str) == Some("codex"));
            ensure!(resolved.env.get("PROJECT_ONLY").map(String::as_str) == Some("proj"));
            ensure!(resolved.env.get("GLOBAL_ONLY").map(String::as_str) == Some("g"));
            ensure!(!resolved.env.contains_key("CLI_OR_ROLE"));
            ensure!(!resolved.env.contains_key("CLI_UNSET_AT_PROJ"));
            ensure!(resolved.unset_keys.contains("CLI_OR_ROLE"));
            ensure!(resolved.unset_keys.contains("CLI_UNSET_AT_PROJ"));
            for (k, v) in &protected {
                ensure!(
                    resolved.env.get(k) == Some(v) && !resolved.unset_keys.contains(k),
                    "Protected core key {k} was overwritten or unset!"
                );
            }

            // `project.agents` backing-engine override (`developer -> claude`) switches Agent env!
            let updated_proj_val = world.client_call(
                "project.agents",
                json!({
                    "project_id": layer_pid,
                    "agent_overrides": { "developer": "claude" },
                }),
                Some(&layer_pid),
            )?;
            let updated_proj: canban_types::Project = serde_json::from_value(updated_proj_val)?;
            let resolved_claude = local_srv.resolve_runtime_env(
                Some(dev_prof),
                Some(&updated_proj),
                &protected,
                Some(&wf_env),
                Some("local"),
            )?;
            ensure!(resolved_claude.env.get("CLAUDE_ONLY").map(String::as_str) == Some("claude"));
            ensure!(!resolved_claude.env.contains_key("CLI_ONLY"));
            ensure!(resolved_claude.env.get("ROLE_ONLY").map(String::as_str) == Some("role"));
            ensure!(
                resolved_claude.env.get("WF_FROM_SEC").map(String::as_str)
                    == Some(sec_global.as_str())
            );
        }

        // Script expansion, stdout/stderr/`$CANBAN_OUTPUT` masking & write-only secret scan!
        exec_local.set_script_default(
            "script:ok",
            SimShellOutcome::ok(format!("leak_stdout={sec_proj} global={sec_global}\n"))
                .with_output("secret_out", sec_proj.clone())
                .with_output("safe_out", "prod-staging"),
        );
        exec_local.set_script_default(
            "script:fail",
            SimShellOutcome::fail(
                7,
                format!("fail_stdout={sec_proj}\n"),
                format!("fail_stderr={sec_proj}\n"),
            ),
        );
        let mask_card = world.client_call(
            "card.create",
            json!({ "project_id": layer_pid, "title": "Run Env Scripts" }),
            Some(&layer_pid),
        )?;
        let mask_cid = mask_card["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "card.move",
            json!({
                "card_id": mask_cid,
                "revision": mask_card["revision"],
                "stage": "develop",
            }),
            Some(&layer_pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == mask_cid && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (mask_tid, mask_addr) = {
            let snap = local_srv.snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == mask_cid))
                .unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let _ = world.server_call(
            "local",
            "execution.report",
            json!({
                "terminal_id": mask_tid,
                "request_key": format!("env-codex-done-{seed}"),
                "state": "completed",
                "message": "done",
            }),
        )?;
        herdr_local.finish_terminal(&mask_addr, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(mask_cid.as_str())
                    && r.outputs.contains_key("script:ok")
                    && r.step.as_deref() == Some("script:fail")
                    && r.state != "waiting_script"
            }))
        })?;

        // Confirm SimExec received exact expanded secrets/vars in `env_set` and unset keys in `env_remove`.
        let ok_cmd = exec_local
            .recorded_commands()
            .into_iter()
            .find(|c| c.env_set.get("CANBAN_STEP_ID").map(String::as_str) == Some("script:ok"))
            .context("Expected script:ok recorded command")?;
        ensure!(ok_cmd.env_set.get("EXPANDED_P").map(String::as_str) == Some(sec_proj.as_str()));
        ensure!(ok_cmd.env_set.get("EXPANDED_G").map(String::as_str) == Some(sec_global.as_str()));
        ensure!(ok_cmd.env_set.get("EXPANDED_V").map(String::as_str) == Some("prod-staging"));
        ensure!(ok_cmd.env_remove.iter().any(|k| k == "CLI_UNSET_AT_PROJ"));

        let mask_snap = local_srv.snapshot()?;
        let mask_run = mask_snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id.as_deref() == Some(mask_cid.as_str()))
            .unwrap();
        ensure!(
            mask_run
                .outputs
                .get("script:ok")
                .and_then(|v| v.get("safe_out"))
                .and_then(Value::as_str)
                == Some("prod-staging")
        );
        let masked_out = mask_run
            .outputs
            .get("script:ok")
            .and_then(|v| v.get("secret_out"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        ensure!(
            masked_out.contains("***") && !masked_out.contains(&sec_proj),
            "script:ok secret_out was not masked: {masked_out}"
        );

        let mut all_files = Vec::new();
        collect_all_files(&local_home, &mut all_files);
        let combined_logs: String = all_files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "log"))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect::<Vec<_>>()
            .join("\n");
        ensure!(
            combined_logs.contains("leak_stdout=***")
                && combined_logs.contains("fail_stdout=***")
                && combined_logs.contains("fail_stderr=***"),
            "Expected *** secret masking in script stdout/stderr logs: {combined_logs}"
        );
        let sentinels = [
            sec_global.as_str(),
            sec_codex_ov.as_str(),
            sec_proj.as_str(),
            sec_agent.as_str(),
            sec_codex.as_str(),
        ];
        for sentinel in sentinels {
            ensure!(
                !combined_logs.contains(sentinel),
                "Secret sentinel leaked into script .log!"
            );
        }

        // Write-only secret scan across ALL RPC surfaces, files, and non-vault SQLite tables!
        let surfaces = [
            std::fs::read(local_home.join("config/agents.json"))?,
            std::fs::read(local_home.join("canban.sqlite"))?,
            std::fs::read(local_home.join("canban.sqlite-wal")).unwrap_or_default(),
            serde_json::to_vec(&local_srv.call("app.state", json!({}))?)?,
            serde_json::to_vec(&local_srv.call("config.get", json!({}))?)?,
            serde_json::to_vec(&local_srv.call("agent.catalog", json!({}))?)?,
            serde_json::to_vec(&local_srv.call("project.list", json!({}))?)?,
            serde_json::to_vec(&local_srv.call(
                "env.preview",
                json!({ "project_id": layer_pid, "profile_id": "codex" }),
            )?)?,
            serde_json::to_vec(&local_srv.call("host.env.keys", json!({}))?)?,
            serde_json::to_vec(&local_srv.events()?)?,
            serde_json::to_vec(&mask_snap)?,
        ];
        for (idx, bytes) in surfaces.iter().enumerate() {
            for sentinel in sentinels {
                ensure!(
                    !contains_bytes(bytes, sentinel.as_bytes()),
                    "Secret sentinel leaked in surface #{idx}"
                );
            }
        }
        {
            let ro_conn = rusqlite::Connection::open_with_flags(
                local_home.join("canban.sqlite"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let mut tbl_stmt = ro_conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT IN ('vault_entries')",
            )?;
            let tables: Vec<String> = tbl_stmt
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for table in tables {
                let mut q = ro_conn.prepare(&format!("SELECT * FROM {table}"))?;
                let cols = q.column_count();
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? {
                    for col in 0..cols {
                        if let rusqlite::types::ValueRef::Text(b) = row.get_ref(col)? {
                            for sentinel in sentinels {
                                ensure!(
                                    !contains_bytes(b, sentinel.as_bytes()),
                                    "Secret sentinel leaked in SQLite table {table}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Warm-PTY invalidation on secret rotation + `replace_profiles` preserving agent env!
        let warm_dir = world.project_dir(&format!("warm-pty-{seed}"))?;
        let warm_proj_val = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Warm PTY {seed}"),
                "directory": warm_dir,
            }),
            None,
        )?;
        let warm_pid = warm_proj_val["id"].as_str().unwrap().to_owned();
        let warm_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": warm_pid }),
            Some(&warm_pid),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": warm_pid,
                "revision": warm_sources["revision"],
                "sources": [{ "filename": "warm-pty-wf.yaml", "source": WARM_PTY_WORKFLOW_YAML }],
            }),
            Some(&warm_pid),
        )?;
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "agent",
                "target_id": "developer",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("AGENT_PLAIN".into(), EnvWriteEntry::Plain { value: "v1".into() }),
                    ("AGENT_UNSET".into(), EnvWriteEntry::Unset),
                    ("API_TOKEN".into(), EnvWriteEntry::Secret { value: Some(tok_warm_v1.clone()), keep_existing: false }),
                ]),
            }),
        )?;
        let warm_card = world.client_call(
            "card.create",
            json!({ "project_id": warm_pid, "title": "Warm PTY Card" }),
            Some(&warm_pid),
        )?;
        let warm_cid = warm_card["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "card.move",
            json!({
                "card_id": warm_cid,
                "revision": warm_card["revision"],
                "stage": "develop",
            }),
            Some(&warm_pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == warm_cid && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (warm_tid, warm_addr_1, warm_fp_1) = {
            let snap = local_srv.snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == warm_cid))
                .unwrap();
            let a = t.agent.as_ref().unwrap();
            ensure!(!a.applied_env_fingerprint.is_empty());
            (
                t.id.clone(),
                a.terminal.clone().unwrap(),
                a.applied_env_fingerprint.clone(),
            )
        };
        // Complete turn while keeping warm PTY live and `done` (would normally continue in-place).
        herdr_local.set_status(&warm_addr_1, "done", Some(1));
        world.advance(Duration::from_millis(250))?;
        let _ = world.server_call(
            "local",
            "execution.report",
            json!({
                "terminal_id": warm_tid,
                "request_key": format!("warm-done-1-{seed}"),
                "state": "completed",
                "message": "done",
            }),
        )?;
        // Rotate `developer` secret -> move to `review` -> warm PTY MUST be killed & relaunched with new fingerprint!
        let _ = world.server_call(
            "local",
            "env.save",
            json!({
                "scope": "agent",
                "target_id": "developer",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("AGENT_PLAIN".into(), EnvWriteEntry::Plain { value: "v1".into() }),
                    ("AGENT_UNSET".into(), EnvWriteEntry::Unset),
                    ("API_TOKEN".into(), EnvWriteEntry::Secret { value: Some(tok_warm_v2.clone()), keep_existing: false }),
                ]),
            }),
        )?;
        let warm_rev = local_srv
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == warm_cid)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": warm_cid, "revision": warm_rev, "stage": "review" }),
            Some(&warm_pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == warm_tid
                    && t.agent.as_ref().is_some_and(|a| {
                        a.state == "running"
                            && a.terminal
                                .as_deref()
                                .is_some_and(|addr| addr != warm_addr_1)
                            && a.applied_env_fingerprint != warm_fp_1
                    })
            }))
        })?;
        ensure!(
            herdr_local.killed_terminals().contains(&warm_addr_1),
            "Expected warm PTY {warm_addr_1} to be killed on secret rotation"
        );
        let warm_addr_2 = local_srv
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == warm_tid)
            .unwrap()
            .agent
            .unwrap()
            .terminal
            .unwrap();
        herdr_local.finish_terminal(&warm_addr_2, 0, "exited");

        // `config.agents.save(op: "replace_profiles")` preserves target host SQLite Agent Env & Secrets!
        let cat = local_srv.call("agent.catalog", json!({}))?;
        let raw_profiles: Vec<Profile> = serde_json::from_value(cat["raw_profiles"].clone())?;
        let _ = local_srv.call(
            "config.agents.save",
            json!({ "op": "replace_profiles", "profiles": raw_profiles }),
        )?;
        let snap_after_replace = local_srv.snapshot()?;
        let dev_meta = snap_after_replace.agent_env("local", "developer");
        ensure!(
            dev_meta.get("AGENT_PLAIN") == Some(&EnvEntryMeta::Plain { value: "v1".into() })
                && dev_meta.get("AGENT_UNSET") == Some(&EnvEntryMeta::Unset)
                && matches!(
                    dev_meta.get("API_TOKEN"),
                    Some(EnvEntryMeta::Secret {
                        configured: true,
                        ..
                    })
                ),
            "replace_profiles clobbered SQLite agent env: {dev_meta:?}"
        );
        {
            let dev_prof = local_srv
                .config()
                .profiles
                .into_iter()
                .find(|p| p.id == "developer")
                .unwrap();
            let warm_proj = snap_after_replace
                .projects
                .into_iter()
                .find(|p| p.id == warm_pid)
                .unwrap();
            let resolved_after = local_srv.resolve_runtime_env(
                Some(&dev_prof),
                Some(&warm_proj),
                &BTreeMap::new(),
                None,
                Some("local"),
            )?;
            ensure!(
                resolved_after.env.get("API_TOKEN").map(String::as_str)
                    == Some(tok_warm_v2.as_str()),
                "replace_profiles lost rotated developer secret!"
            );
        }

        // Sync final global env changes before final quiescent invariant checks.
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;
        world.hit("env_sync.secret_masking_and_warm_pty");
        world.hit("env_sync.tombstone_bounce_converged");

        Ok(finalize_digest(world))
    })
}
