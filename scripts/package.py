#!/usr/bin/env python3
"""Package the built native binaries as a local macOS application."""
from pathlib import Path
import argparse
import plistlib
import shutil
import os
import tempfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
kind = parser.add_mutually_exclusive_group()
kind.add_argument("--gallery", action="store_true", help="Package only the standalone component example")
kind.add_argument("--dev", action="store_true", help="Package the development app with its own bundle identity")
parser.add_argument("--release", action="store_true", help="Package optimized release binaries")
args = parser.parse_args()
name = "CanbanGallery" if args.gallery else "CanbanDev" if args.dev else "Canban"
app_id = "com.canban.gallery" if args.gallery else "com.canban.dev" if args.dev else "com.canban.desktop"
executable = "gallery" if args.gallery else "canban-ui"
app = root / ".local" / (name + ".app")
profile = "release" if args.release else "debug"
binaries = {"gallery": root / "target" / profile / "examples/gallery"} if args.gallery else {
    binary: root / "target" / profile / binary for binary in ("canban-ui", "canban", "canban-updater")
}
macos = app / "Contents/MacOS"
macos.mkdir(parents=True, exist_ok=True)
for binary, source_binary in binaries.items():
    with tempfile.NamedTemporaryFile(dir=macos, prefix=f".{binary}-", delete=False) as staging:
        staged = Path(staging.name)
    shutil.copy2(source_binary, staged)
    os.replace(staged, macos / binary)
version = os.environ.get("CANBAN_RELEASE_VERSION", "0.1.0")
sequence = os.environ.get("GITHUB_RUN_NUMBER", "1")
with (app / "Contents/Info.plist").open("wb") as file:
    plistlib.dump({"CFBundleIdentifier": app_id, "CFBundleName": name, "CFBundleDisplayName": name, "CFBundleExecutable": executable, "CFBundlePackageType": "APPL", "CFBundleVersion": sequence, "CFBundleShortVersionString": version, "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "13.0"}, file)
print(app)

resources=app/"Contents/Resources/licenses"
resources.mkdir(parents=True,exist_ok=True)
for source in (root/"third_party").glob("*-LICENSE.txt"):
    shutil.copy2(source,resources/source.name)
shutil.copy2(root/"THIRD_PARTY_NOTICES.md",resources/"THIRD_PARTY_NOTICES.md")

if args.release:
    import subprocess
    subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(app)], check=True)
