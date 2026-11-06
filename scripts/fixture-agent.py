#!/usr/bin/env python3
"""Deterministic acceptance agent, explicitly selected in an isolated test home."""
import json
import os
from pathlib import Path
import subprocess
import time

home = Path(os.environ["CANBAN_HOME"])
request = os.environ["CANBAN_REQUEST_ID"]
manifest = next(home.glob(f"projects/*/cards/*/runs/{request}/{os.environ['CANBAN_ATTEMPT']}/context.json"))
context = json.loads(manifest.read_text())
assignment = context["request"]["instruction"]
stage = context["request"]["stage"]
print(f"Fixture received Card #{context['card']['number']} in {stage}", flush=True)
if "concurrency-barrier" in assignment:
    barrier = home / "acceptance-barrier"
    barrier.mkdir(exist_ok=True)
    (barrier / request).write_text(context["project"]["id"])
    deadline = time.monotonic() + 45
    while not (barrier / "release").exists():
        if time.monotonic() >= deadline:
            raise RuntimeError("Acceptance did not release concurrent Agents")
        time.sleep(.1)
if "block-me" in assignment:
    subprocess.run([os.environ["CANBAN_CLI"], "report", "--state", "blocked", "--message", "Fixture needs a human answer"], check=True)
else:
    if stage == "develop" and context["request"]["trigger"] == "stage_entry":
        Path("canban-acceptance.txt").write_text("Herdr executed the Canban Card task.\n")
    report = Path.cwd() / f"report-{request}.md"
    report.write_text(f"# {stage.title()} report\n\nCaptured Card: {context['card']['title']}\n\nReal Herdr PTY, explicit CLI result, and file-backed context verified.\n")
    subprocess.run([os.environ["CANBAN_CLI"], "report", "--state", "completed", "--message", f"Fixture {stage} complete", "--file", str(report)], check=True)
