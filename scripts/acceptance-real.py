#!/usr/bin/env python3
"""Opt-in real Codex development/review and cross-Board concurrency acceptance.

Creates two disposable Git Projects and an isolated Canban/Herdr home. Runs
four actual model Sessions, checks the changes/reports, and archives both Cards.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--timeout", type=int, default=600)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix="cb-real-", dir="/private/tmp"))
home = base / "home"
cli = repo / "target/debug/canban"
env = {**os.environ, "CANBAN_HOME": str(home)}
core_env = {"HC_SESSION": "canban-real-acceptance", "HC_SOCKET": str(base / "hc.sock"), "XDG_STATE_HOME": str(base / "hc-state"), "XDG_CONFIG_HOME": str(base / "hc-config")}
log = (base / "server.log").open("a")
server = subprocess.Popen([str(cli), "serve"], env=env, stdout=log, stderr=log)
history = []
overlap = False
last_progress = 0.0

def call(method, params=None):
    out = subprocess.run([str(cli), "--full","call", method, json.dumps(params or {})], env=env, capture_output=True, text=True, timeout=95)
    if out.returncode:
        raise RuntimeError(out.stderr)
    return json.loads(out.stdout)

def snapshot():
    global overlap, last_progress
    state = call("app.snapshot")
    active = [r for r in state["requests"] if r["state"] in ("starting", "running", "working")]
    active_projects = {r["project_id"] for r in active}
    bound = {a["request_id"] for a in state["attempts"] if a["terminal"]}
    observed_overlap = len({r["project_id"] for r in active if r["id"] in bound}) > 1
    overlap = overlap or observed_overlap
    observation = {"at": time.time(), "active_projects": sorted(active_projects), "bound_overlap": observed_overlap, "requests": [{"id": r["id"], "project_id": r["project_id"], "stage": r["stage"], "state": r["state"]} for r in state["requests"]]}
    history.append(observation)
    if time.monotonic() - last_progress > 10:
        print("PROGRESS " + json.dumps(observation), flush=True)
        last_progress = time.monotonic()
    return state

def wait_for(predicate, label, timeout=None):
    deadline = time.monotonic() + (timeout or args.timeout)
    while time.monotonic() < deadline:
        if server.poll() is not None:
            raise RuntimeError("AppServer exited: " + (base / "server.log").read_text())
        try:
            state = snapshot()
        except (RuntimeError, FileNotFoundError):
            time.sleep(.2)
            continue
        if predicate(state):
            return state
        failures = [r for r in state["requests"] if r["state"] in ("failed", "blocked", "needs_report", "unknown")]
        if failures:
            for attempt in state["attempts"]:
                if any(r["id"] == attempt["request_id"] for r in failures):
                    try:
                        print("AGENT_OUTPUT " + json.dumps(call("execution.output", {"attempt_id": attempt["id"]})), flush=True)
                    except RuntimeError:
                        pass
            raise RuntimeError("Real Agent did not complete: " + json.dumps(failures))
        time.sleep(.4)
    raise RuntimeError("Timed out waiting for " + label)

def git(directory, *argv):
    git_env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    return subprocess.run(["git", "-C", str(directory), *argv], env=git_env, check=True, capture_output=True, text=True).stdout.strip()

fixtures = [
    {"name": "Alpha", "filename": "greeting.py", "initial": "def greet(name):\n    return 'TODO'\n", "tests": "import unittest\nfrom greeting import greet\nclass GreetingTests(unittest.TestCase):\n    def test_name(self): self.assertEqual(greet('Ada'), 'Hello, Ada!')\n    def test_unicode(self): self.assertEqual(greet('世界'), 'Hello, 世界!')\n", "task": "Implement greeting.greet(name) to return exactly 'Hello, {name}!' including Unicode names."},
    {"name": "Beta", "filename": "bounds.py", "initial": "def clamp(value, lower, upper):\n    return value\n", "tests": "import unittest\nfrom bounds import clamp\nclass BoundsTests(unittest.TestCase):\n    def test_below(self): self.assertEqual(clamp(-3, 0, 10), 0)\n    def test_above(self): self.assertEqual(clamp(20, 0, 10), 10)\n    def test_inside(self): self.assertEqual(clamp(4, 0, 10), 4)\n    def test_invalid(self):\n        with self.assertRaises(ValueError): clamp(4, 10, 0)\n", "task": "Implement bounds.clamp(value, lower, upper): return the nearest bound when outside, preserve values inside, and raise ValueError if lower > upper."},
]

try:
    wait_for(lambda s: True, "AppServer", 20)
    config_path = home / "config/agents.json"
    config = json.loads(config_path.read_text())
    config["hc_session"] = core_env["HC_SESSION"]
    config["hc_env"] = core_env
    config["default_agent"] = "codex"
    config_path.write_text(json.dumps(config, indent=2))
    call("config.reload")
    core = call("herdr.status")
    print(f"REAL_ACCEPTANCE_HOME={home}\nHERDR_CORE={core['core']['id']}", flush=True)

    for fixture in fixtures:
        source = base / ("source-" + fixture["name"].lower())
        source.mkdir()
        (source / fixture["filename"]).write_text(fixture["initial"])
        (source / "test_task.py").write_text(fixture["tests"])
        (source / ".gitignore").write_text("__pycache__/\n*.pyc\n")
        git(source, "init")
        git(source, "add", ".")
        git(source, "-c", "user.name=Canban Acceptance", "-c", "user.email=acceptance@example.invalid", "-c", "core.hooksPath=/dev/null", "commit", "-m", "Seed real Agent acceptance task")
        real_yaml = f"""schema: 1
id: delivery:{fixture["name"]}
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
      instruction: Review the work for this card. Inspect the code and earlier results, then write an ordinary Markdown review report.
    next: __end
  - id: __end
    uses: complete
"""
        project = call("project.create", {"workflow_yaml": real_yaml, "name": "Real Board " + fixture["name"], "directory": str(source)})
        marker = "CANBAN_" + fixture["name"].upper() + "_" + base.name
        body = fixture["task"] + "\nRun /usr/bin/python3 -m unittest -v. Keep test_task.py unchanged. Do not modify any other Project or the source repository. No need to commit. Write a meaningful Markdown report inside the Canban run directory, including the Board isolation marker, task result and validation, then explicitly call the supplied CLI to report your outcome."
        body += f"\nBoard isolation marker: {marker}. Include this exact marker in every report."
        card = call("card.create", {"project_id": project["id"], "title": "Real " + fixture["name"] + " development and review", "body": body})
        fixture.update(source=str(source), project_id=project["id"], card_id=card["id"], marker=marker)

    # Both Boards are made executable before either model is expected to finish.
    for fixture in fixtures:
        call("card.move", {"card_id": fixture["card_id"], "revision": 1, "stage": "develop"})
    state = wait_for(lambda s: len(s["requests"]) == 4 and all(r["state"] == "completed" for r in s["requests"]) and all(a["terminal"] for a in s["attempts"]), "two real development and review stages")
    assert overlap, "Did not observe two bound executions active across Boards at once"
    assert len({a["terminal"] for a in state["attempts"]}) == 4
    assert all(a["core_id"] == core["core"]["id"] for a in state["attempts"])

    result = {"home": str(home), "core_id": core["core"]["id"], "concurrent_boards_observed": overlap, "boards": [], "observations": history}
    for fixture in fixtures:
        card = next(c for c in state["cards"] if c["id"] == fixture["card_id"])
        assert card["stage"] == "review"
        attempts = [a for a in state["attempts"] if a["card_id"] == card["id"]]
        assert len(attempts) == 2 and len({a["cwd"] for a in attempts}) == 1
        cwd = Path(attempts[0]["cwd"])
        assert cwd != Path(fixture["source"])
        assert (cwd / "test_task.py").read_text() == fixture["tests"]
        validation = subprocess.run(["/usr/bin/python3", "-m", "unittest", "-v"], cwd=cwd, capture_output=True, text=True)
        assert validation.returncode == 0, validation.stdout + validation.stderr
        assert (Path(fixture["source"]) / fixture["filename"]).read_text() == fixture["initial"]
        for attempt in attempts:
            context = json.loads((home / attempt["input_path"]).with_name("context.json").read_text())
            assert context["project"]["id"] == fixture["project_id"]
            assert context["card"]["id"] == fixture["card_id"]
            assert context["notes"] == [] and fixture["marker"] in context["card"]["body"]
        reports = [r for r in state["reports"] if r["card_id"] == card["id"] and r["state"] == "completed"]
        assert len(reports) == 2 and all(r["path"] for r in reports)
        bodies = []
        for report in reports:
            body = call("resource.read", {"card_id": card["id"], "path": report["path"]})["body"]
            assert fixture["marker"] in body
            assert all(other["marker"] not in body for other in fixtures if other != fixture)
            bodies.append(body)
        receipt = call("card.archive", {"card_id": card["id"], "revision": card["revision"]})
        assert (home / receipt["path"]).exists()
        for report, body in zip(reports, bodies):
            assert not (home / report["path"]).exists()
            assert call("resource.read", {"card_id": card["id"], "path": report["path"]})["body"] == body
        result["boards"].append({"name": fixture["name"], "project_id": fixture["project_id"], "card_id": fixture["card_id"], "worktree": str(cwd), "terminals": [a["terminal"] for a in attempts], "requests": [a["request_id"] for a in attempts], "archive": receipt["path"], "validation": validation.stdout + validation.stderr, "report_ids": [r["id"] for r in reports]})
        print(f"REAL_BOARD_{fixture['name'].upper()}_ARCHIVED worktree={cwd}", flush=True)
    assert len({b["worktree"] for b in result["boards"]}) == 2
    result["observations"] = history
    result["status"] = "passed"
    (base / "evidence.json").write_text(json.dumps(result, indent=2))
    print(f"REAL_AGENT_WORKFLOW_AND_CONCURRENCY_PASSED\nEVIDENCE={base / 'evidence.json'}", flush=True)
except Exception as error:
    (base / "failure.json").write_text(json.dumps({"error": str(error), "observations": history}, indent=2))
    print(f"REAL_ACCEPTANCE_FAILED {error}\nEVIDENCE={base / 'failure.json'}", flush=True)
    raise
finally:
    server.terminate()
    server.wait(timeout=10)
    config_path = home / "config/agents.json"
    if config_path.exists():
        config = json.loads(config_path.read_text())
        subprocess.run([config["hc"], "server", "stop"], env={**os.environ, **core_env}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
    log.close()
