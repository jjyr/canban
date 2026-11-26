#!/usr/bin/env python3
"""Opt-in local acceptance through real hc, PTYs, CLI and SQLite; no paid model."""
import argparse
import json
import os
import shutil
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
env = {k: v for k, v in os.environ.items() if not k.startswith(("CANBAN_", "HC_", "HERDR_"))}
env["CANBAN_HOME"] = str(home)
core_env = {"HC_SESSION": "canban-acceptance", "HC_SOCKET": str(base / "hc.sock"), "XDG_STATE_HOME": str(base / "hc-state"), "XDG_CONFIG_HOME": str(base / "hc-config")}
adopt_fixture = base / "adopt-fixture.py"
adopt_fixture.write_text(r'''import json, os, subprocess, sys, termios, tty, uuid
from pathlib import Path
home = Path(os.environ["CANBAN_HOME"])
sid = str(uuid.uuid4())
subprocess.run([os.environ["CANBAN_CLI"], "bind-session", "--session-id", sid], check=True, capture_output=True)
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty.setraw(fd)
try:
    print("\x1b]7501;state=working\x1b\\", end="", flush=True)
    (home / "adopt-ready.json").write_text(json.dumps({"pid": os.getpid(), "terminal": os.environ.get("HC_TERMINAL_ID"), "session": sid}))
    buf = ""
    while True:
        chunk = os.read(fd, 4096)
        if not chunk:
            break
        buf += chunk.decode("utf-8", errors="replace")
        if buf and all(c in ("\x1b", "\x03") for c in buf):
            print("\x1b]7501;state=idle\x1b\\", end="", flush=True)
            buf = ""
        elif "\x1b[200~" in buf and "\x1b[201~" not in buf:
            continue
        elif buf.endswith(("\r", "\n")):
            line = buf.replace("\x1b[200~", "").replace("\x1b[201~", "").strip()
            buf = ""
            if line == "adopt":
                subprocess.run([os.environ["CANBAN_CLI"], "card", "adopt"], check=True, capture_output=True)
            elif line == "/exit":
                break
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
''')
# Configure the isolated Core before the first service starts. Bootstrapping with
# the default session and then changing sessions shares HC_SOCKET between two
# Core identities and can leave the observer attached to the bootstrap Core.
config_dir=home/"config"
config_dir.mkdir(parents=True,exist_ok=True)
initial_config=config_dir/"agents.json"
if not initial_config.exists():
    initial_config.write_text(json.dumps({"hc":shutil.which("hc"),"hc_session":core_env["HC_SESSION"],"hc_env":core_env,"default_agent":"developer","profiles":[{"id":"codex","preset":"codex"},{"id":"claude","preset":"claude"},{"id":"developer","preset":"codex"},{"id":"reviewer","preset":"codex"}]}))
log = open(base / "server.log", "a")
env.update(core_env)
server = subprocess.Popen([str(cli), "serve"], env=env, stdout=log, stderr=log)

def call(method, params=None):
    result = subprocess.run([str(cli), "--full","call", method, json.dumps(params or {})], env=env, text=True, capture_output=True, timeout=95)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)

def agents(snapshot):
    return [t["agent"] for t in snapshot["terminals"] if t.get("agent")]

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
    config["capacity"] = 1  # Legacy keys must no longer restrict concurrency.
    config["profiles"].append({"id": "fixture", "argv": ["/usr/bin/python3", str(repo / "scripts/fixture-agent.py")], "capacity": 1})
    config["profiles"].append({"id": "adopt", "preset": None, "resume_argv": [], "session_instruction": "", "exit": {"type": "input", "text": "/exit"}, "argv": ["/usr/bin/python3", str(adopt_fixture)]})
    for profile in config["profiles"]:
        if profile["id"] in ("developer", "reviewer"):
            profile["preset"] = None
            profile["resume_argv"] = []
            profile["session_instruction"] = ""
            profile["exit"] = {"type":"manual"}
            profile["argv"] = ["/usr/bin/python3", str(repo / "scripts/fixture-agent.py")]
            profile["role_instruction"] = "ACCEPTANCE_ROLE_" + profile["id"].upper()
    config_path.write_text(json.dumps(config, indent=2))
    # Restart AppServer for the fixture profiles while retaining the same isolated Core.
    server.terminate()
    server.wait(timeout=10)
    server = subprocess.Popen([str(cli), "serve"], env=env, stdout=log, stderr=log)
    wait_for(lambda _: True, "Configured AppServer starts")
    core = call("herdr.status")
    assert core["core"]["id"]
    print(f"HERDR_CONNECTED {core['version']} core={core['core']['id']}", flush=True)
    review_workflow_yaml = """schema: 1
id: delivery:{id}
name: Development
workspace:
  create_worktree: true
  sync_base: false
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
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory. Explain your changes and validation in your report.
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: agent:review
  - id: agent:review
    uses: agent
    with:
      profile: reviewer
      instruction: Review the changes and report the result.
    next: __end
  - id: __end
    uses: complete
"""
    project = call("project.create", {"workflow_yaml": review_workflow_yaml.format(id="mvp"), "name": "MVP Acceptance", "directory": str(source)})
    note = call("note.save", {"project_id": project["id"], "title": "Human Context", "body": "Only modify the disposable acceptance project."})
    card = call("card.create", {"project_id": project["id"], "title": "Verify automatic development and review", "body": "Create the acceptance deliverable and report the result."})
    call("attachment.add", {"card_id": card["id"], "name": "sample.txt", "bytes": list(b"pasted resource")})
    move = {"card_id": card["id"], "revision": card["revision"], "stage": "develop", "request_key": "acceptance-move"}
    call("card.move", move)
    call("card.move", move)
    state = wait_for(lambda s: len(s["reports"]) >= 2 and len(agents(s)) == 2 and all(r["state"] == "completed" for r in agents(s)), "Develop -> Review reports")
    assert len(agents(state)) == 2
    assert {r["stage"]: r["profile"]["id"] for r in agents(state)} == {"develop": "developer", "review": "reviewer"}
    for attempt in agents(state):
        prompt = (home / attempt["input_path"]).read_text()
        context = json.loads((home / attempt["input_path"]).with_name("context.json").read_text())
        assert context["notes"] == [] and "canban note" not in prompt
        assert attempt["profile"]["role_instruction"] in prompt
        assert "ACCEPTANCE_ROLE_" + attempt["profile"]["id"].upper() in prompt
    assert call("note.read", {"note_id": note["id"]})["body"] == note["body"]  # Legacy data remains intact.
    help_text = subprocess.run([str(cli), "--help"], capture_output=True, text=True, check=True).stdout
    assert not any(line.strip().startswith("note ") for line in help_text.splitlines())
    current = next(c for c in state["cards"] if c["id"] == card["id"])
    assert current["stage"] == "review"
    assert (source / "canban-acceptance.txt").exists()
    assert all(a["core_id"] == core["core"]["id"] and a["terminal"] for a in agents(state))
    print("DEVELOP_REVIEW_OK separate template-selected profiles, captured roles, explicit results and runtime bindings", flush=True)
    call("comment.add", {"card_id": card["id"], "body": "@fixture explain the change"})
    state = wait_for(lambda s: len(s["reports"]) >= 3, "Fresh mention Session")
    assert next(c for c in state["cards"] if c["id"] == card["id"])["stage"] == "review"
    call("comment.add", {"card_id": card["id"], "body": "@fixture block-me"})
    state = wait_for(lambda s: any(r["card_id"] == card["id"] and r["state"] == "blocked" for r in s["reports"]) and any(n["card_id"] == card["id"] for n in s["notices"]), "Blocked report and notice")
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
    assert len(agents(restored)) == 3 and max(a["number"] for a in agents(restored)) == 4
    print("RESTART_OK", flush=True)
    cards = []
    for index in range(3):
        project = call("project.create", {"workspace":{"sync_base":False},"name": f"Concurrent {index}", "directory": str(base / f"concurrent-{index}")})
        card = call("card.create", {"project_id": project["id"], "title": "Concurrent Agent"})
        cards.append(card["id"])
        call("comment.add", {"card_id": card["id"], "body": "@fixture concurrency-barrier"})
    barrier = home / "acceptance-barrier"
    state = wait_for(lambda s: barrier.exists() and len(list(barrier.iterdir())) == 3
                     and len([a for a in agents(s) if a["card_id"] in cards and a["terminal"]]) == 3,
                     "Three same-profile Agents running across Boards within the global budget")
    concurrent = [a for a in agents(state) if a["card_id"] in cards]
    assert len({a["project_id"] for a in concurrent}) == 3
    assert len({a["terminal"] for a in concurrent}) == 3
    assert all(a["core_id"] == core["core"]["id"] for a in concurrent)
    (barrier / "release").touch()
    wait_for(lambda s: len(s["reports"]) == 7 and all(r["state"] == "completed" for r in agents(s) if r["card_id"] in cards),
             "Concurrent Agents report independently")
    print("GLOBAL_BUDGET_CONCURRENCY_OK three Boards, one profile, ignored legacy capacity=1", flush=True)
    # Smoke-test real-PTY adoption and interrupt delivery over the isolated Herdr Core.
    adopt_project = call("project.create", {"name": "Adopt Smoke", "directory": str(base / "adopt-source"), "workspace": {"create_worktree": False, "sync_base": False}})
    adopt_card = call("card.create", {"project_id": adopt_project["id"], "title": "Adopt and interrupt real PTY", "hold_auto_merge": True})
    manual = call("terminal.create", {"card_id": adopt_card["id"], "agent": True, "profile_id": "adopt", "name": "Adopt smoke PTY"})
    adopt_terminal = manual["runtime"]["terminal"]
    wait_for(lambda _: (home / "adopt-ready.json").exists(), "Adopt smoke fixture ready")
    before_adopt = json.loads((home / "adopt-ready.json").read_text())
    assert before_adopt["terminal"] == adopt_terminal
    subprocess.run(["hc", "send", adopt_terminal, "adopt", "--json"], env=env, capture_output=True, text=True, check=True, timeout=20)
    adopted_state = wait_for(lambda s: any((t.get("agent") or {}).get("card_id") == adopt_card["id"] for t in s["terminals"]), "Real PTY adoption")
    adopted_entry = next(t for t in adopted_state["terminals"] if (t.get("agent") or {}).get("card_id") == adopt_card["id"])
    adopted_run = adopted_entry["agent"]
    assert adopted_entry["id"] != manual["id"] and adopted_run["terminal"] == adopt_terminal
    assert adopted_run["provider_session_id"] == before_adopt["session"]
    assert adopted_run["observed"]["pid"] == before_adopt["pid"] and adopted_run["state"] == "running"
    interrupt_req = {"terminal_id": adopted_entry["id"], "request_key": "acceptance-interrupt"}
    interrupt_receipt = call("agent.interrupt", interrupt_req)
    assert interrupt_receipt["state"] == "sent" and call("agent.interrupt", interrupt_req) == interrupt_receipt
    wait_for(lambda s: any(t["id"] == adopted_entry["id"] and (((t.get("agent") or {}).get("observed") or {}).get("status") or {}).get("state") == "idle" for t in s["terminals"]),
             "Real PTY interrupt observation")
    print(f"ADOPT_INTERRUPT_SMOKE_OK\nACCEPTANCE_HOME={home}\nACCEPTANCE_PASSED", flush=True)
finally:
    server.terminate()
    server.wait(timeout=10)
    subprocess.run(["hc", "server", "stop"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    log.close()
