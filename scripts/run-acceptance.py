#!/usr/bin/env python3
"""Run acceptance/test scripts with live log teeing, optional retries, and GitHub summaries."""
import argparse
import os
import pathlib
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from typing import Dict, List, Optional, Tuple

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parents[1]

INTEGRATION_SUITE = [
    "scripts/acceptance.py",
    "scripts/acceptance-events.py",
    "scripts/acceptance-dev.py",
    "scripts/acceptance-backend.py",
]


def safe_stem(script_path: str) -> str:
    stem = pathlib.Path(script_path).stem or "command"
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", stem)


def build_argv(cmd: List[str]) -> List[str]:
    exe_name = pathlib.Path(cmd[0]).name
    if exe_name in ("python", "python3") or exe_name.startswith("python3."):
        return [sys.executable] + cmd[1:]
    return [sys.executable] + cmd


def append_step_summary(lines: List[str]) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY", "").strip()
    if not summary_path:
        return
    path = pathlib.Path(summary_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


def run_entry(
    cmd: List[str],
    retries: int,
    log_dir: pathlib.Path,
) -> Dict[str, object]:
    argv = build_argv(cmd)
    name = " ".join(cmd)
    stem = safe_stem(cmd[0])
    max_attempts = 1 + max(0, retries)
    log_dir.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env.setdefault("PYTHONUNBUFFERED", "1")

    started = time.monotonic()
    last_log: Optional[pathlib.Path] = None
    outcome = "failed"
    attempts_used = 0

    for attempt in range(1, max_attempts + 1):
        attempts_used = attempt
        log_path = log_dir / f"{stem}.attempt{attempt}.log"
        last_log = log_path
        print(f"==> [{attempt}/{max_attempts}] {name}", flush=True)

        proc = subprocess.Popen(
            argv,
            cwd=str(ROOT),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        assert proc.stdout is not None
        with log_path.open("w", encoding="utf-8") as log_file:
            for line in iter(proc.stdout.readline, ""):
                sys.stdout.write(line)
                sys.stdout.flush()
                log_file.write(line)
                log_file.flush()
        proc.stdout.close()
        rc = proc.wait()

        if rc == 0:
            outcome = "passed" if attempt == 1 else "flaky"
            break
        print(f"==> [{attempt}/{max_attempts}] FAILED (exit {rc}): {name}", flush=True)

    elapsed = time.monotonic() - started
    return {
        "name": name,
        "outcome": outcome,
        "attempts": attempts_used,
        "duration": elapsed,
        "log": str(last_log) if last_log is not None else "",
    }


def emit_run_summary(results: List[Dict[str, object]]) -> int:
    print("\n=== Acceptance Summary ===", flush=True)
    for item in results:
        print(
            f"- {item['name']}: {item['outcome']} "
            f"(attempts={item['attempts']}, {item['duration']:.1f}s, log={item['log']})",
            flush=True,
        )
        if item["outcome"] == "flaky":
            print(
                f"::warning title=Flaky acceptance::{item['name']} passed on attempt {item['attempts']}",
                flush=True,
            )
        elif item["outcome"] == "failed":
            print(
                f"::error title=Acceptance failed::{item['name']}",
                flush=True,
            )

    md_lines = [
        "### Acceptance Summary",
        "",
        "| Script | Outcome | Attempts | Duration |",
        "| --- | --- | --- | --- |",
    ]
    for item in results:
        md_lines.append(
            f"| `{item['name']}` | {item['outcome']} | {item['attempts']} | {item['duration']:.1f}s |"
        )
    md_lines.append("")
    append_step_summary(md_lines)

    return 1 if any(item["outcome"] == "failed" for item in results) else 0


def summarize_junit(junit_path: pathlib.Path) -> int:
    if not junit_path.is_file():
        print(f"JUnit report not found at {junit_path}; skipping summary.", flush=True)
        return 0

    try:
        tree = ET.parse(str(junit_path))
    except ET.ParseError as exc:
        print(f"::warning title=JUnit parse error::Failed to parse {junit_path}: {exc}", flush=True)
        return 0

    root = tree.getroot()
    total = 0
    flaky: List[Tuple[str, int]] = []
    failed: List[str] = []

    for tc in root.iter("testcase"):
        total += 1
        classname = tc.attrib.get("classname", "").strip()
        name = tc.attrib.get("name", "").strip()
        label = f"{classname} {name}".strip() if classname else name
        flaky_nodes = tc.findall("flakyFailure")
        failure_nodes = tc.findall("failure")
        if flaky_nodes:
            flaky.append((label, len(flaky_nodes)))
        if failure_nodes:
            failed.append(label)

    print(
        f"Unit tests JUnit summary ({junit_path}): total={total}, flaky={len(flaky)}, failed={len(failed)}",
        flush=True,
    )
    for label, count in flaky:
        print(
            f"::warning title=Flaky unit test::{label} ({count} flaky attempt(s))",
            flush=True,
        )
    for label in failed:
        print(f"::error title=Unit test failed::{label}", flush=True)

    md_lines = [
        "### Unit Test Summary (nextest CI)",
        "",
        f"- Total: {total}",
        f"- Flaky (retried): {len(flaky)}",
        f"- Failed: {len(failed)}",
    ]
    if flaky:
        md_lines.extend(["", "| Flaky test | Retries |", "| --- | --- |"])
        for label, count in flaky:
            md_lines.append(f"| `{label}` | {count} |")
    if failed:
        md_lines.extend(["", "| Failed test |", "| --- |"])
        for label in failed:
            md_lines.append(f"| `{label}` |")
    md_lines.append("")
    append_step_summary(md_lines)
    return 0


def main() -> int:
    default_retries = int(os.environ.get("CANBAN_ACCEPTANCE_RETRIES", "0"))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--suite",
        choices=["integration"],
        default=None,
        help="Named acceptance suite to run",
    )
    parser.add_argument(
        "--retries",
        type=int,
        default=default_retries,
        help="Retry count on failure (default: CANBAN_ACCEPTANCE_RETRIES or 0)",
    )
    parser.add_argument(
        "--log-dir",
        type=pathlib.Path,
        default=ROOT / "target/acceptance-logs",
        help="Directory for per-attempt logs",
    )
    parser.add_argument(
        "--junit-summary",
        type=pathlib.Path,
        default=None,
        help="Parse a nextest JUnit XML report and emit summary/annotations",
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Optional single script command after --",
    )
    args = parser.parse_args()

    if args.junit_summary is not None:
        return summarize_junit(args.junit_summary)

    cmd = list(args.command)
    if cmd and cmd[0] == "--":
        cmd = cmd[1:]

    if args.suite and cmd:
        parser.error("Cannot combine --suite with a command after --")

    if cmd:
        entries = [cmd]
    else:
        entries = [[script] for script in INTEGRATION_SUITE]

    results = [run_entry(entry, args.retries, args.log_dir) for entry in entries]
    return emit_run_summary(results)


if __name__ == "__main__":
    raise SystemExit(main())
