#!/usr/bin/env python3
"""Opt-in local acceptance through real hc, PTYs, CLI and SQLite; no paid model."""
import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("--home", type=Path)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
base = args.home or Path(tempfile.mkdtemp(prefix="cb-", dir="/private/tmp"))
home = base / "home"
cli = repo / "target/debug/canban"
source = base / "source"
source.mkdir(parents=True, exist_ok=True)
env = os.environ.copy()
env["CANBAN_HOME"] = str(home)
core_env = {"HC_SESSION": "canban-acceptance", "HC_SOCKET": str(base / "hc.sock"), "XDG_STATE_HOME": str(base / "hc-state"), "XDG_CONFIG_HOME": str(base / "hc-config")}
log = open(base / "server.log", "a")
server = subprocess.Popen([str(cli), "serve"], env=env, stdout=log, stderr=log)

def call(method, params=None):
    result = subprocess.run([str(cli), "call", method, json.dumps(params or {})], env=env, text=True, capture_output=True, timeout=95)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)

def wait_for(predicate, label, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if server.poll() is not None:
            raise RuntimeError((base / "server.log").read_text())
        try:
            value = call("app.snapshot")
        except (RuntimeError, FileNotFoundError):
            time.sleep(.15)
            continue
        if predicate(value):
            return value
        time.sleep(.25)
    raise RuntimeError(f"Timed out: {label}\n{(base / 'server.log').read_text()}")

try:
    wait_for(lambda _: True, "AppServer starts")
    config_path = home / "config/agents.json"
    config = json.loads(config_path.read_text())
    config["hc_session"] = core_env["HC_SESSION"]
    config["hc_env"] = core_env
    config["default_agent"] = "fixture"
    config["profiles"].append({"id": "fixture", "argv": ["/usr/bin/python3", str(repo / "scripts/fixture-agent.py")], "tags": ["fixture"], "capacity": 2})
    config_path.write_text(json.dumps(config, indent=2))
    call("config.reload")
    core = call("herdr.status")
    assert core["core"]["id"]
    print(f"HERDR_CONNECTED {core['version']} core={core['core']['id']}", flush=True)
    project = call("project.create", {"name": "MVP Acceptance", "directory": str(source)})
    note = call("note.save", {"project_id": project["id"], "title": "Human Context", "body": "Only modify the disposable acceptance project."})
    card = call("card.create", {"project_id": project["id"], "title": "Verify automatic development and review", "body": "Create the acceptance deliverable and report the result."})
    call("attachment.add", {"card_id": card["id"], "name": "sample.txt", "bytes": list(b"pasted resource")})
    move = {"card_id": card["id"], "revision": card["revision"], "stage": "develop", "request_key": "acceptance-move"}
    call("card.move", move)
    call("card.move", move)
    state = wait_for(lambda s: len(s["reports"]) >= 2 and all(r["state"] == "completed" for r in s["requests"]), "Develop -> Review reports")
    assert len(state["requests"]) == 2
    current = next(c for c in state["cards"] if c["id"] == card["id"])
    assert current["stage"] == "review"
    assert (source / "canban-acceptance.txt").exists()
    assert all(a["core_id"] == core["core"]["id"] and a["terminal"] for a in state["attempts"])
    print("DEVELOP_REVIEW_OK explicit results, runtime bindings, delivery and work product", flush=True)
    call("comment.add", {"card_id": card["id"], "body": "@fixture explain the change"})
    state = wait_for(lambda s: len(s["reports"]) >= 3, "Fresh mention Session")
    assert next(c for c in state["cards"] if c["id"] == card["id"])["stage"] == "review"
    call("comment.add", {"card_id": card["id"], "body": "@fixture block-me"})
    state = wait_for(lambda s: any(n["card_id"] == card["id"] for n in s["notices"]), "Blocked notice")
    assert next(c for c in state["cards"] if c["id"] == card["id"])["attention"]
    print("MENTION_BLOCKED_OK fresh Session and durable attention notice", flush=True)
    report = next(r for r in state["reports"] if r["path"])
    original = call("resource.read", {"card_id": card["id"], "path": report["path"]})["body"]
    current = next(c for c in state["cards"] if c["id"] == card["id"])
    archived = call("card.archive", {"card_id": current["id"], "revision": current["revision"], "request_key": "acceptance-archive"})
    assert (home / archived["path"]).exists()
    assert not (home / report["path"]).exists()
    assert call("resource.read", {"card_id": card["id"], "path": report["path"]})["body"] == original
    current = next(c for c in call("app.snapshot")["cards"] if c["id"] == card["id"])
    call("card.restore", {"card_id": current["id"], "revision": current["revision"]})
    assert (home / report["path"]).exists()
    with sqlite3.connect(home / "canban.sqlite") as db:
        assert db.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
    print("ARCHIVE_RESTORE_WAL_OK compressed reports remain readable", flush=True)
    server.terminate()
    server.wait(timeout=10)
    server = subprocess.Popen([str(cli), "serve"], env=env, stdout=log, stderr=log)
    restored = wait_for(lambda s: len(s["reports"]) >= 4, "AppServer restart")
    assert len(restored["requests"]) == 4
    print(f"RESTART_OK\nACCEPTANCE_HOME={home}\nACCEPTANCE_PASSED", flush=True)
finally:
    server.terminate()
    server.wait(timeout=10)
    subprocess.run(["hc", "server", "stop"], env={**os.environ, **core_env}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    log.close()
