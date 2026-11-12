#!/usr/bin/env python3
"""Atomically install the packaged Canban.app into /Applications/Canban.app."""
from pathlib import Path
import argparse
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile

if sys.platform != "darwin":
    sys.exit(f"install-app currently supports macOS only (got {sys.platform})")

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--source",
    type=Path,
    default=root / ".local/Canban.app",
    help="Packaged unbound Canban.app bundle to install",
)
parser.add_argument(
    "--dest",
    type=Path,
    default=Path("/Applications/Canban.app"),
    help="Destination bundle path (defaults to /Applications/Canban.app)",
)
args = parser.parse_args()

source = args.source.resolve()
if not source.is_dir() or source.suffix != ".app":
    parser.error(f"Packaged bundle not found at {source}; run `mise run build-release` first")
if (source / "Contents/Resources/dev-instance.json").exists():
    parser.error("Refusing to install a branch-bound development bundle into /Applications")

plist_path = source / "Contents/Info.plist"
if not plist_path.is_file():
    parser.error(f"Missing Info.plist in {source}")
info = plistlib.loads(plist_path.read_bytes())
if info.get("CFBundleIdentifier") != "com.canban.desktop":
    parser.error(
        f"Expected CFBundleIdentifier com.canban.desktop, got {info.get('CFBundleIdentifier')!r}"
    )

for required in ("canban-ui", "canban", "canban-updater"):
    if not (source / "Contents/MacOS" / required).is_file():
        parser.error(f"Missing required executable Contents/MacOS/{required} in {source}")

dest = args.dest
dest_parent = dest.parent.resolve()
dest_parent.mkdir(parents=True, exist_ok=True)
dest = dest_parent / dest.name

staging_root = Path(tempfile.mkdtemp(prefix=".Canban-install-", dir=dest_parent))
staged = staging_root / "Canban.app"
backup = None
try:
    shutil.copytree(source, staged, symlinks=False)
    subprocess.run(
        ["/usr/bin/xattr", "-dr", "com.apple.quarantine", str(staged)],
        check=False,
        capture_output=True,
    )
    subprocess.run(
        ["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(staged)],
        check=True,
    )
    subprocess.run(
        ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(staged)],
        check=True,
    )
    if dest.exists():
        backup = staging_root / "Canban-previous.app"
        os.rename(dest, backup)
    try:
        os.rename(staged, dest)
    except Exception:
        if backup is not None and backup.exists() and not dest.exists():
            os.rename(backup, dest)
        raise
finally:
    shutil.rmtree(staging_root, ignore_errors=True)

print(dest)
