#!/usr/bin/env python3
"""Capture an explicitly selected Canban process with macOS Instruments."""
import argparse
import datetime
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, help="Explicit UI or AppServer PID")
    parser.add_argument("--template", default="Allocations", help="Installed xctrace template name")
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--output", type=Path, help="Fresh output directory (default: /tmp)")
    parser.add_argument("--list-templates", action="store_true")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Instruments capture requires macOS")
    if args.list_templates:
        return subprocess.call(["xcrun", "xctrace", "list", "templates"])
    if not args.pid or args.pid <= 0:
        parser.error("Choose the UI or AppServer explicitly with --pid")
    if not 1 <= args.seconds <= 3600:
        parser.error("--seconds must be between 1 and 3600")
    process = subprocess.check_output(
        ["ps", "-ww", "-p", str(args.pid), "-o", "pid=,comm="], text=True,
    ).strip()
    executable = process.split(None, 1)[1] if process else ""
    if Path(executable).name not in ("canban-ui", "canban", "gallery"):
        parser.error("The selected PID is not a Canban UI, AppServer or Gallery process")
    output = args.output
    if output:
        output.mkdir(parents=True, exist_ok=False)
    else:
        output = Path(tempfile.mkdtemp(prefix="canban-profile-"))
    root = Path(__file__).resolve().parents[1]
    metadata = {
        "pid": args.pid, "process": process, "template": args.template,
        "seconds": args.seconds, "captured_at": datetime.datetime.now().astimezone().isoformat(),
        "platform": platform.platform(),
        "checkout": str(root),
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=root)),
    }
    build = subprocess.run([executable if Path(executable).name == "canban" else str(Path(executable).with_name("canban")),
                            "build-info"], capture_output=True, text=True)
    metadata["target_build"] = build.stdout.strip() if build.returncode == 0 else "unavailable"
    uuid = subprocess.run(["xcrun", "dwarfdump", "--uuid", executable], capture_output=True, text=True)
    metadata["target_uuid"] = uuid.stdout.strip() if uuid.returncode == 0 else "unavailable"
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    def snapshot(name):
        with (output / name).open("w") as stream:
            result = subprocess.run(["vmmap", "-summary", str(args.pid)], stdout=stream, stderr=subprocess.STDOUT)
        return result.returncode
    snapshot("before-vmmap.txt")
    print(f"Recording {process} -> {output}", flush=True)
    result = subprocess.run(["xcrun", "xctrace", "record", "--template", args.template,
                             "--attach", str(args.pid), "--time-limit", f"{args.seconds}s",
                             "--output", str(output / "capture.trace")],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    (output / "record.log").write_text(result.stdout)
    print(result.stdout, flush=True)
    # xctrace can produce a trace container despite failing to attach. Do not call that success.
    failed = result.returncode or "[Error]" in result.stdout or "Recording failed" in result.stdout
    capture_code = result.returncode or (1 if failed else 0)
    snapshot("after-vmmap.txt")
    metadata["capture_exit_code"] = capture_code
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output, flush=True)
    return capture_code


if __name__ == "__main__":
    raise SystemExit(main())
