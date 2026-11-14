#!/usr/bin/env python3
"""Deterministic acceptance agent, explicitly selected in an isolated test home."""
import json
import os
from pathlib import Path
import subprocess
import time
import sys

home = Path(os.environ["CANBAN_HOME"])
if os.environ.get("CANBAN_CONFIGURATION_ID"):
    key=os.environ["CANBAN_CONFIGURATION_ID"]
    generation=os.environ["CANBAN_CONFIGURATION_GENERATION"]
    folder=home/"configuration-evidence";folder.mkdir(exist_ok=True)
    prompt=sys.argv[-1] if len(sys.argv)>1 else ""
    (folder/(generation+".json")).write_text(json.dumps({"id":key,"generation":generation,"prompt":prompt,"session_id":os.environ.get("CANBAN_SESSION_ID"),"argv":sys.argv[1:]}))
    print("Project Workflow configuration assistant ready. No business Card created.",flush=True)
    for line in sys.stdin:
        if line.strip()=="/exit":break
        print("Configuration conversation: "+line.strip(),flush=True)
    sys.exit(0)

run_dir = Path(os.environ["CANBAN_RUN_DIR"])
manifest = run_dir / "context.json"
context = json.loads(manifest.read_text())
agent = context["agent"]
terminal_id = context["terminal_id"]
run_key = f"{terminal_id.replace(':', '-')}-{agent['number']}"
assignment = agent["instruction"]
stage = agent["stage"]
trigger = agent["trigger"]
print(f"Fixture received Card #{context['card']['number']} in {stage}", flush=True)
if "concurrency-barrier" in assignment:
    barrier = home / "acceptance-barrier"
    barrier.mkdir(exist_ok=True)
    (barrier / run_key).write_text(context["project"]["id"])
    deadline = time.monotonic() + 45
    while not (barrier / "release").exists():
        if time.monotonic() >= deadline:
            raise RuntimeError("Acceptance did not release concurrent Agents")
        time.sleep(.1)
if "block-me" in assignment:
    subprocess.run([os.environ["CANBAN_CLI"], "report", "--state", "blocked", "--message", "Fixture needs a human answer"], check=True)
elif trigger == "interactive" and "ADOPT_FROM_BACKLOG" in context["card"].get("body", ""):
    spec = run_dir / "card.md"
    spec.write_text("Confirmed design key points from interactive session\n")
    subprocess.run([os.environ["CANBAN_CLI"], "card", "update", "--title", "Adopted Interactive Spec", "--file", str(spec)], check=True)
    subprocess.run([os.environ["CANBAN_CLI"], "card", "adopt"], check=True)
    report = run_dir / f"report-{run_key}.md"
    report.write_text("# Interactive Develop report\n\nAdopted develop in current session.\n")
    subprocess.run([os.environ["CANBAN_CLI"], "report", "--state", "completed", "--message", "Adopted develop complete", "--file", str(report)], check=True)
else:
    if stage == "develop" and trigger in ("stage_entry", "workflow"):
        Path("canban-acceptance.txt").write_text("Herdr executed the Canban Card task.\n")
    report = run_dir / f"report-{run_key}.md"
    report.write_text(f"# {stage.title()} report\n\nCaptured Card: {context['card']['title']}\n\nReal Herdr PTY, explicit CLI result, and file-backed context verified.\n")
    subprocess.run([os.environ["CANBAN_CLI"], "report", "--state", "completed", "--message", f"Fixture {stage} complete", "--file", str(report)], check=True)
