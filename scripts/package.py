#!/usr/bin/env python3
"""Package the built native binaries as a local macOS application."""
from pathlib import Path
import plistlib
import shutil
import os
import tempfile

root = Path(__file__).resolve().parents[1]
app = root / ".local/Canban.app"
macos = app / "Contents/MacOS"
macos.mkdir(parents=True, exist_ok=True)
for binary in ("canban-ui", "canban"):
    with tempfile.NamedTemporaryFile(dir=macos, prefix=f".{binary}-", delete=False) as staging:
        staged = Path(staging.name)
    shutil.copy2(root / "target/debug" / binary, staged)
    os.replace(staged, macos / binary)
with (app / "Contents/Info.plist").open("wb") as file:
    plistlib.dump({"CFBundleIdentifier": "com.canban.desktop", "CFBundleName": "Canban", "CFBundleDisplayName": "Canban", "CFBundleExecutable": "canban-ui", "CFBundlePackageType": "APPL", "CFBundleVersion": "1", "CFBundleShortVersionString": "0.1.0", "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "13.0"}, file)
print(app)
