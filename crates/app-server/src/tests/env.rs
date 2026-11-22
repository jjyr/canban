use super::*;
use canban_types::{EnvEntryMeta, EnvPreviewRow, EnvWriteEntry, Profile};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

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

#[test]
fn env_layers_precedence_unset_override_and_core_context_protection() {
    let (_dir, app, project, _card) = setup();

    // Ensure @developer backs onto codex initially.
    app.call(
        "config.agents.save",
        json!({
            "op": "upsert_identity",
            "id": "developer",
            "engine": "codex",
            "enabled": true,
        }),
    )
    .unwrap();

    // Global layer.
    app.call(
        "env.save",
        json!({
            "scope": "global",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("GLOBAL_ONLY".into(), EnvWriteEntry::Plain { value: "g".into() }),
                ("SHARED".into(), EnvWriteEntry::Plain { value: "global".into() }),
                (
                    "SEC_GLOBAL".into(),
                    EnvWriteEntry::Secret {
                        value: Some("sec_global_sentinel_1".into()),
                        keep_existing: false,
                    },
                ),
                (
                    "CANBAN_PROJECT".into(),
                    EnvWriteEntry::Plain {
                        value: "hacked-global".into(),
                    },
                ),
            ]),
        }),
    )
    .unwrap();

    // CLI Agent layer: codex and claude.
    app.call(
        "env.save",
        json!({
            "scope": "agent",
            "target_id": "codex",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("CLI_ONLY".into(), EnvWriteEntry::Plain { value: "codex".into() }),
                ("SHARED".into(), EnvWriteEntry::Plain { value: "codex".into() }),
                ("CLI_OR_ROLE".into(), EnvWriteEntry::Plain { value: "cli".into() }),
                ("CLI_UNSET_AT_PROJ".into(), EnvWriteEntry::Plain { value: "keep".into() }),
                (
                    "SEC_GLOBAL".into(),
                    EnvWriteEntry::Secret {
                        value: Some("sec_codex_override_1".into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();
    app.call(
        "env.save",
        json!({
            "scope": "agent",
            "target_id": "claude",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("CLAUDE_ONLY".into(), EnvWriteEntry::Plain { value: "claude".into() }),
                ("SHARED".into(), EnvWriteEntry::Plain { value: "claude".into() }),
            ]),
        }),
    )
    .unwrap();

    // Role @Identity layer: developer overrides codex and unsets CLI_OR_ROLE.
    app.call(
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
    )
    .unwrap();

    // Project layer: overrides SHARED, unsets CLI_UNSET_AT_PROJ, attempts to unset core CANBAN_HOME.
    app.call(
        "env.save",
        json!({
            "scope": "project",
            "target_id": project.id,
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("PROJECT_ONLY".into(), EnvWriteEntry::Plain { value: "proj".into() }),
                ("SHARED".into(), EnvWriteEntry::Plain { value: "proj".into() }),
                ("CLI_UNSET_AT_PROJ".into(), EnvWriteEntry::Unset),
                ("CANBAN_HOME".into(), EnvWriteEntry::Unset),
            ]),
        }),
    )
    .unwrap();

    // Preview confirms provenance + desensitized secrets (no plaintext in SEC_GLOBAL).
    let preview_val = app
        .call(
            "env.preview",
            json!({ "project_id": project.id, "profile_id": "developer" }),
        )
        .unwrap();
    let preview_rows: Vec<EnvPreviewRow> =
        serde_json::from_value(preview_val["rows"].clone()).unwrap();
    let find_row = |k: &str| preview_rows.iter().find(|r| r.key == k).unwrap();
    assert_eq!(find_row("SHARED").source_layer, "Project");
    assert_eq!(find_row("ROLE_ONLY").source_layer, "Identity (@developer)");
    assert_eq!(find_row("CLI_ONLY").source_layer, "Agent (@codex)");
    assert_eq!(find_row("GLOBAL_ONLY").source_layer, "Global");
    assert!(find_row("CLI_OR_ROLE").is_unset);
    assert!(find_row("CLI_UNSET_AT_PROJ").is_unset);
    let sec_row = find_row("SEC_GLOBAL");
    assert!(sec_row.is_secret && sec_row.configured);
    assert!(!sec_row.display_value.contains("sec_codex_override_1"));
    assert_eq!(sec_row.source_layer, "Agent (@codex)");

    // Resolve runtime env with all 10 protected core keys + workflow template expansion.
    let cfg = app.config();
    let dev_prof = cfg.profiles.iter().find(|p| p.id == "developer").unwrap();
    let protected = BTreeMap::from([
        ("CANBAN_CLI".into(), "/usr/local/bin/canban".into()),
        ("CANBAN_ENDPOINT".into(), "/tmp/app.sock".into()),
        (
            "CANBAN_HOME".into(),
            app.home.to_string_lossy().into_owned(),
        ),
        ("CANBAN_PROJECT".into(), project.id.clone()),
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
    let resolved = app
        .resolve_runtime_env(
            Some(dev_prof),
            Some(&project),
            &protected,
            Some(&wf_env),
            Some("local"),
        )
        .unwrap();

    assert_eq!(resolved.env.get("SHARED").map(String::as_str), Some("step"));
    assert_eq!(
        resolved.env.get("WF_FROM_VAR").map(String::as_str),
        Some("proj")
    );
    assert_eq!(
        resolved.env.get("WF_FROM_SEC").map(String::as_str),
        Some("sec_codex_override_1")
    );
    assert_eq!(
        resolved.env.get("ROLE_ONLY").map(String::as_str),
        Some("role")
    );
    assert_eq!(
        resolved.env.get("CLI_ONLY").map(String::as_str),
        Some("codex")
    );
    assert_eq!(
        resolved.env.get("PROJECT_ONLY").map(String::as_str),
        Some("proj")
    );
    assert_eq!(
        resolved.env.get("GLOBAL_ONLY").map(String::as_str),
        Some("g")
    );
    assert!(!resolved.env.contains_key("CLI_OR_ROLE"));
    assert!(!resolved.env.contains_key("CLI_UNSET_AT_PROJ"));
    assert!(resolved.unset_keys.contains("CLI_OR_ROLE"));
    assert!(resolved.unset_keys.contains("CLI_UNSET_AT_PROJ"));
    for (k, v) in &protected {
        assert_eq!(
            resolved.env.get(k),
            Some(v),
            "Protected core key {k} must stay immutable"
        );
        assert!(
            !resolved.unset_keys.contains(k),
            "Protected core key {k} must never be unset"
        );
    }

    // Project backing-engine override: @developer -> claude switches CLI Agent env cleanly.
    let updated_project: Project = serde_json::from_value(
        app.call(
            "project.agents",
            json!({
                "project_id": project.id,
                "agent_overrides": { "developer": "claude" },
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let resolved_claude = app
        .resolve_runtime_env(
            Some(dev_prof),
            Some(&updated_project),
            &protected,
            Some(&wf_env),
            Some("local"),
        )
        .unwrap();
    assert_eq!(
        resolved_claude.env.get("CLAUDE_ONLY").map(String::as_str),
        Some("claude")
    );
    assert!(!resolved_claude.env.contains_key("CLI_ONLY"));
    assert_eq!(
        resolved_claude.env.get("ROLE_ONLY").map(String::as_str),
        Some("role")
    );
    assert_eq!(
        resolved_claude.env.get("WF_FROM_SEC").map(String::as_str),
        Some("sec_global_sentinel_1")
    );
}

#[test]
fn env_script_expansion_output_masking_and_write_only_secret_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    super::terminal_cases::core(&app, dir.path());

    let sec_global = "GLOBAL_SECRET_SENTINEL_9f8e7d";
    let sec_proj = "PROJ_SECRET_SENTINEL_1a2b3c";
    let sec_agent = "AGENT_SECRET_SENTINEL_4d5e6f";
    let sec_codex = "CODEX_SECRET_SENTINEL_7c8b9a";

    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let wf_yaml = format!(
        r#"schema: {schema}
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
      WF_SEC_REF: "${{{{ secrets.P_SEC }}}}"
    with:
      profile: codex
      instruction: Check launch isolation
    next: script:ok
  - id: script:ok
    uses: script
    env:
      EXPANDED_P: "${{{{ secrets.P_SEC }}}}"
      EXPANDED_G: "${{{{ secrets.G_SEC }}}}"
      EXPANDED_V: "${{{{ vars.G_MODE }}}}-${{{{ vars.P_ENV }}}}"
    run: |
      set -e
      [ "${{#EXPANDED_P}}" = "{p_len}" ]
      [ "${{#EXPANDED_G}}" = "{g_len}" ]
      [ "$EXPANDED_V" = "prod-staging" ]
      echo "leak_stdout=$EXPANDED_P global=$EXPANDED_G"
      echo "secret_out=$EXPANDED_P" >> "$CANBAN_OUTPUT"
      echo "safe_out=$EXPANDED_V" >> "$CANBAN_OUTPUT"
    next: script:fail
  - id: script:fail
    uses: script
    env:
      FAIL_SEC: "${{{{ secrets.P_SEC }}}}"
    run: |
      echo "fail_stdout=$FAIL_SEC"
      echo "fail_stderr=$FAIL_SEC" >&2
      exit 7
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#,
        schema = canban_workflow_engine::migrations::CURRENT,
        p_len = sec_proj.len(),
        g_len = sec_global.len(),
    );

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Env Mask Repo",
                "directory": repo,
                "workflow_yaml": wf_yaml,
            }),
        )
        .unwrap(),
    )
    .unwrap();

    app.call(
        "env.save",
        json!({
            "scope": "global",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("G_MODE".into(), EnvWriteEntry::Plain { value: "prod".into() }),
                (
                    "G_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some(sec_global.into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();
    app.call(
        "env.save",
        json!({
            "scope": "agent",
            "target_id": "codex",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                (
                    "A_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some(sec_agent.into()),
                        keep_existing: false,
                    },
                ),
                (
                    "CODEX_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some(sec_codex.into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();
    app.call(
        "env.save",
        json!({
            "scope": "project",
            "target_id": project.id,
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("P_ENV".into(), EnvWriteEntry::Plain { value: "staging".into() }),
                (
                    "P_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some(sec_proj.into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();

    // vault.key created lazily with restrictive 0600 Unix permissions.
    let key_mode = std::fs::metadata(app.home.join("vault.key"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(key_mode, 0o600);

    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({ "view": "full", "project_id": project.id, "title": "Run Env Scripts" }),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({ "view": "full", "card_id": card.id, "revision": card.revision, "stage": "develop" }),
    )
    .unwrap();
    app.tick().unwrap();

    // Verify Codex argv did NOT leak custom agent/project secrets into CLI arguments.
    let launches_raw = std::fs::read_to_string(dir.path().join("launches")).unwrap();
    let launch_val: Value = serde_json::from_str(launches_raw.lines().next().unwrap()).unwrap();
    let argv_json = serde_json::to_string(&launch_val["argv"]).unwrap();
    assert!(!argv_json.contains(sec_agent));
    assert!(!argv_json.contains(sec_codex));
    assert!(!argv_json.contains(sec_proj));
    assert!(!argv_json.contains(sec_global));

    // Complete codex agent step to execute script:ok -> script:fail.
    let agent_term = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card.id))
        .unwrap();
    app.call(
        "execution.report",
        json!({
            "terminal_id": agent_term.id,
            "request_key": "codex-done",
            "state": "completed",
            "message": "done",
        }),
    )
    .unwrap();
    app.pump_scripts().unwrap();

    let snap = app.snapshot().unwrap();
    let run = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(card.id.as_str()))
        .unwrap();
    assert_eq!(
        run.outputs
            .get("script:ok")
            .and_then(|v| v.get("safe_out"))
            .and_then(Value::as_str),
        Some("prod-staging")
    );
    let masked_out = run
        .outputs
        .get("script:ok")
        .and_then(|v| v.get("secret_out"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(masked_out.contains("***"));
    assert!(!masked_out.contains(sec_proj));

    // Check script .log files on disk exist and mask every secret with ***.
    let mut all_files = Vec::new();
    collect_all_files(&app.home, &mut all_files);
    let log_files: Vec<PathBuf> = all_files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .cloned()
        .collect();
    assert!(!log_files.is_empty(), "Expected script .log files on disk");
    let combined_logs: String = log_files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined_logs.contains("leak_stdout=***")
            && combined_logs.contains("fail_stdout=***")
            && combined_logs.contains("fail_stderr=***"),
        "Expected *** masking in script logs: {combined_logs}"
    );
    for sentinel in [sec_global, sec_proj, sec_agent, sec_codex] {
        assert!(!combined_logs.contains(sentinel));
    }

    // Write-only secret scan across ALL RPC surfaces and files.
    let surfaces = [
        std::fs::read(app.home.join("config/agents.json")).unwrap(),
        std::fs::read(app.home.join("canban.sqlite")).unwrap(),
        std::fs::read(app.home.join("canban.sqlite-wal")).unwrap_or_default(),
        serde_json::to_vec(&app.call("app.state", json!({})).unwrap()).unwrap(),
        serde_json::to_vec(&app.call("config.get", json!({})).unwrap()).unwrap(),
        serde_json::to_vec(&app.call("agent.catalog", json!({})).unwrap()).unwrap(),
        serde_json::to_vec(&app.call("project.list", json!({})).unwrap()).unwrap(),
        serde_json::to_vec(
            &app.call(
                "env.preview",
                json!({ "project_id": project.id, "profile_id": "codex" }),
            )
            .unwrap(),
        )
        .unwrap(),
        serde_json::to_vec(&app.call("host.env.keys", json!({})).unwrap()).unwrap(),
        serde_json::to_vec(&app.events().unwrap()).unwrap(),
        serde_json::to_vec(&snap).unwrap(),
    ];
    for (idx, bytes) in surfaces.iter().enumerate() {
        for sentinel in [sec_global, sec_proj, sec_agent, sec_codex] {
            assert!(
                !contains_bytes(bytes, sentinel.as_bytes()),
                "Secret sentinel leaked in surface #{idx}"
            );
        }
    }

    // Scan every non-vault SQLite table including operations & env_entries.
    let store = app.store.lock().unwrap();
    let mut tbl_stmt = store
        .db()
        .conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT IN ('vault_entries')",
        )
        .unwrap();
    let tables: Vec<String> = tbl_stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for table in tables {
        let mut q = store
            .db()
            .conn
            .prepare(&format!("SELECT * FROM {table}"))
            .unwrap();
        let cols = q.column_count();
        let mut rows = q.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for col in 0..cols {
                if let rusqlite::types::ValueRef::Text(b) = row.get_ref(col).unwrap() {
                    for sentinel in [sec_global, sec_proj, sec_agent, sec_codex] {
                        assert!(
                            !contains_bytes(b, sentinel.as_bytes()),
                            "Secret sentinel leaked in SQLite table {table}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn external_pr_and_cardless_external_runs_skip_secrets_and_log_notice() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let repo = dir.path().join("ext-repo");
    std::fs::create_dir_all(&repo).unwrap();

    let sec_global = "EXT_SKIP_GLOBAL_777777";
    let sec_proj = "EXT_SKIP_PROJ_888888";

    let wf_yaml = format!(
        r#"schema: {schema}
id: ext-sec-wf
name: External Trust Check
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
    start_at: script:ext_check
    match:
      column: develop
steps:
  - id: script:ext_check
    uses: script
    env:
      SEC_REF: "${{{{ secrets.PROJ_SEC }}}}"
      PLAIN_REF: "${{{{ vars.SAFE_PLAIN }}}}"
    run: |
      set -e
      [ -z "${{GLOBAL_SEC:-}}" ]
      [ -z "${{PROJ_SEC:-}}" ]
      [ -z "${{SEC_REF:-}}" ]
      [ "$PLAIN_REF" = "SAFE_PLAIN_VAL" ]
      [ "$SAFE_PLAIN" = "SAFE_PLAIN_VAL" ]
      echo "external-script-ok"
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#,
        schema = canban_workflow_engine::migrations::CURRENT
    );

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "External PR Repo",
                "directory": repo,
                "workflow_yaml": wf_yaml,
            }),
        )
        .unwrap(),
    )
    .unwrap();

    app.call(
        "env.save",
        json!({
            "scope": "global",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([(
                "GLOBAL_SEC".into(),
                EnvWriteEntry::Secret {
                    value: Some(sec_global.into()),
                    keep_existing: false,
                },
            )]),
        }),
    )
    .unwrap();
    app.call(
        "env.save",
        json!({
            "scope": "project",
            "target_id": project.id,
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("SAFE_PLAIN".into(), EnvWriteEntry::Plain { value: "SAFE_PLAIN_VAL".into() }),
                (
                    "PROJ_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some(sec_proj.into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();

    let mut card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({ "view": "full", "project_id": project.id, "title": "External PR" }),
        )
        .unwrap(),
    )
    .unwrap();
    card.source_trust = Some("external_pr".into());
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&card))
        .unwrap();

    app.call(
        "card.move",
        json!({ "view": "full", "card_id": card.id, "revision": card.revision, "stage": "develop" }),
    )
    .unwrap();
    app.pump_scripts().unwrap();

    let mut files = Vec::new();
    collect_all_files(&app.home, &mut files);
    let logs: String = files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logs.contains("external-script-ok"), "Logs: {logs}");
    assert!(
        logs.contains("Skipped secret(s)")
            && (logs.contains("GLOBAL_SEC") || logs.contains("PROJ_SEC")),
        "Expected skipped-secret notice in script logs: {logs}"
    );
    assert!(!logs.contains(sec_global));
    assert!(!logs.contains(sec_proj));

    // Cardless untrusted external run must also skip all secrets and expand ${{ secrets.* }} -> "".
    let wf_env = BTreeMap::from([("REF".into(), "${{ secrets.PROJ_SEC }}".into())]);
    let ext_resolved = app
        .resolve_runtime_env(
            None,
            Some(&project),
            &BTreeMap::new(),
            Some(&wf_env),
            Some("external"),
        )
        .unwrap();
    assert!(!ext_resolved.env.contains_key("GLOBAL_SEC"));
    assert!(!ext_resolved.env.contains_key("PROJ_SEC"));
    assert_eq!(ext_resolved.env.get("REF").map(String::as_str), Some(""));
    assert_eq!(
        ext_resolved.env.get("SAFE_PLAIN").map(String::as_str),
        Some("SAFE_PLAIN_VAL")
    );
    assert!(!ext_resolved.skipped_untrusted_secrets.is_empty());
}

#[test]
fn warm_pty_invalidated_on_secret_change_and_replace_profiles_preserves_agent_env() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    super::terminal_cases::core(&app, dir.path());
    std::fs::write(
        dir.path().join("test-hc"),
        r#"#!/usr/bin/python3
import sys,json,os
base=os.path.dirname(__file__)
for line in sys.stdin:
 p=json.loads(line);m=p['method'];args=p.get('params',{});r={}
 if m=='hello':r={'generation':1}
 elif m=='server.status':r={'core':{'id':'original'}}
 elif m=='terminal.run':
  with open(base+'/launches','a') as f:f.write(json.dumps(args)+'\n')
  r={'address':'t1'}
 elif m in ['terminal.get','terminal.list']:
  launches=[json.loads(l) for l in open(base+'/launches')] if os.path.exists(base+'/launches') else []
  cwd=launches[-1]['cwd'] if launches else base
  t={'address':'t1','cwd':cwd,'foreground':{'cwd':cwd},'state':'finished' if os.path.exists(base+'/finished') else 'live'}
  if os.path.exists(base+'/done') and not os.path.exists(base+'/finished'):
   t['status']={'state':'done'}
  r={'terminals':[t]} if m=='terminal.list' else t
 elif m in ['terminal.kill','terminal.interrupt']:
  with open(base+'/controls','a') as f:f.write(m+'\n')
  if m=='terminal.kill':open(base+'/finished','w').close()
  r={'sent':True}
 print(json.dumps({'jsonrpc':'2.0','id':p['id'],'result':r}),flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(
        dir.path().join("test-hc"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let repo = dir.path().join("warm-repo");
    std::fs::create_dir_all(&repo).unwrap();
    let wf_yaml = format!(
        r#"schema: {schema}
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
"#,
        schema = canban_workflow_engine::migrations::CURRENT
    );
    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Warm PTY Repo",
                "directory": repo,
                "workflow_yaml": wf_yaml,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({ "view": "full", "project_id": project.id, "title": "Warm PTY Card" }),
        )
        .unwrap(),
    )
    .unwrap();

    app.call(
        "env.save",
        json!({
            "scope": "agent",
            "target_id": "developer",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("AGENT_PLAIN".into(), EnvWriteEntry::Plain { value: "v1".into() }),
                ("AGENT_UNSET".into(), EnvWriteEntry::Unset),
                (
                    "API_TOKEN".into(),
                    EnvWriteEntry::Secret {
                        value: Some("tok_initial_111111".into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();

    app.call(
        "card.move",
        json!({ "view": "full", "card_id": card.id, "revision": card.revision, "stage": "develop" }),
    )
    .unwrap();
    app.tick().unwrap();

    let first_entry = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card.id))
        .unwrap();
    let run1 = first_entry.agent.clone().unwrap();
    assert!(!run1.applied_env_fingerprint.is_empty());

    // Mark warm PTY turn done so scheduler normally continues in place, and finish the active turn.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut e: TerminalEntry = db.get(&first_entry.id)?;
            let a = e.agent.as_mut().unwrap();
            a.connection_state = "connected".into();
            a.observed = Some(json!({ "state": "live", "status": { "state": "done" } }));
            db.save(&e)
        })
        .unwrap();
    std::fs::write(dir.path().join("done"), "").unwrap();
    app.call(
        "execution.report",
        json!({
            "terminal_id": first_entry.id,
            "request_key": "warm-done-1",
            "state": "completed",
            "message": "done",
        }),
    )
    .unwrap();

    // Update secret -> fingerprint must change so warm PTY is killed and rebuilt on follow-up mention.
    app.call(
        "env.save",
        json!({
            "scope": "agent",
            "target_id": "developer",
            "entries": BTreeMap::<String, EnvWriteEntry>::from([
                ("AGENT_PLAIN".into(), EnvWriteEntry::Plain { value: "v1".into() }),
                ("AGENT_UNSET".into(), EnvWriteEntry::Unset),
                (
                    "API_TOKEN".into(),
                    EnvWriteEntry::Secret {
                        value: Some("tok_rotated_222222".into()),
                        keep_existing: false,
                    },
                ),
            ]),
        }),
    )
    .unwrap();

    let card_rev = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap()
        .revision;
    app.call(
        "card.move",
        json!({ "view": "full", "card_id": card.id, "revision": card_rev, "stage": "review" }),
    )
    .unwrap();

    let snap_mid = app.snapshot().unwrap();
    let refreshed: TerminalEntry = {
        let store = app.store.lock().unwrap();
        store.db().get(&first_entry.id).unwrap()
    };
    let expected_fp = app
        .expected_agent_env_fingerprint_with_db(
            app.store.lock().unwrap().db(),
            &snap_mid,
            &refreshed,
        )
        .unwrap();
    assert_ne!(run1.applied_env_fingerprint, expected_fp);

    app.tick().unwrap();

    let controls = std::fs::read_to_string(dir.path().join("controls")).unwrap_or_default();
    assert!(
        controls.contains("terminal.kill"),
        "Expected warm PTY replacement via terminal.kill, controls: {controls}"
    );
    let launches: Vec<Value> = std::fs::read_to_string(dir.path().join("launches"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(launches.len(), 2);
    assert_eq!(
        launches[1]["env"]["API_TOKEN"].as_str(),
        Some("tok_rotated_222222")
    );

    // Copy/replace profiles leaves target host Agent Env & Secrets in SQLite untouched.
    let cat = app.call("agent.catalog", json!({})).unwrap();
    let raw_profiles: Vec<Profile> = serde_json::from_value(cat["raw_profiles"].clone()).unwrap();
    app.call(
        "config.agents.save",
        json!({
            "op": "replace_profiles",
            "profiles": raw_profiles,
        }),
    )
    .unwrap();
    let snap_after = app.snapshot().unwrap();
    let dev_meta = snap_after.agent_env("local", "developer");
    assert_eq!(
        dev_meta.get("AGENT_PLAIN"),
        Some(&EnvEntryMeta::Plain { value: "v1".into() })
    );
    assert_eq!(dev_meta.get("AGENT_UNSET"), Some(&EnvEntryMeta::Unset));
    assert!(matches!(
        dev_meta.get("API_TOKEN"),
        Some(EnvEntryMeta::Secret {
            configured: true,
            ..
        })
    ));
    let dev_prof = app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    let resolved_after = app
        .resolve_runtime_env(
            Some(&dev_prof),
            Some(&project),
            &BTreeMap::new(),
            None,
            Some("local"),
        )
        .unwrap();
    assert_eq!(
        resolved_after.env.get("API_TOKEN").map(String::as_str),
        Some("tok_rotated_222222")
    );
}
