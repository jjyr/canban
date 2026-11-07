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
parser.add_argument("--gallery", action="store_true", help="Package only the standalone component example")
args = parser.parse_args()
name = "CanbanGallery" if args.gallery else "Canban"
app_id = "com.canban.gallery" if args.gallery else "com.canban.desktop"
executable = "gallery" if args.gallery else "canban-ui"
app = root / ".local" / (name + ".app")
binaries = {"gallery": root / "target/debug/examples/gallery"} if args.gallery else {
    binary: root / "target/debug" / binary for binary in ("canban-ui", "canban")
}
macos = app / "Contents/MacOS"
macos.mkdir(parents=True, exist_ok=True)
for binary, source_binary in binaries.items():
    with tempfile.NamedTemporaryFile(dir=macos, prefix=f".{binary}-", delete=False) as staging:
        staged = Path(staging.name)
    shutil.copy2(source_binary, staged)
    os.replace(staged, macos / binary)
with (app / "Contents/Info.plist").open("wb") as file:
    plistlib.dump({"CFBundleIdentifier": app_id, "CFBundleName": name, "CFBundleDisplayName": name, "CFBundleExecutable": executable, "CFBundlePackageType": "APPL", "CFBundleVersion": "1", "CFBundleShortVersionString": "0.1.0", "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "13.0"}, file)
print(app)

resources=app/"Contents/Resources/licenses"
resources.mkdir(parents=True,exist_ok=True)
for source in (root/"third_party").glob("*-LICENSE.txt"):
    shutil.copy2(source,resources/source.name)
shutil.copy2(root/"THIRD_PARTY_NOTICES.md",resources/"THIRD_PARTY_NOTICES.md")
