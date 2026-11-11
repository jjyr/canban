#!/usr/bin/env python3
"""Package the built native binaries as a local macOS application."""
from pathlib import Path
import argparse
import plistlib
import shutil
import os
import subprocess
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
rasterizer = shutil.which("rsvg-convert")
if rasterizer is None:
    parser.error("App icon generation requires rsvg-convert; install it with: brew install librsvg")
# Render the original vector at every macOS size, including Retina variants.
# Complete conversion before replacing bundle resources or executables.
with tempfile.TemporaryDirectory(prefix="canban-icon-") as directory:
    iconset = Path(directory) / "AppIcon.iconset"
    iconset.mkdir()
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            pixels = str(size * scale)
            suffix = "@2x" if scale == 2 else ""
            subprocess.run([
                rasterizer, "--width", pixels, "--height", pixels,
                "--output", str(iconset / f"icon_{size}x{size}{suffix}.png"),
                str(root / "crates/ui/assets/app-icon.svg"),
            ], check=True)
    icon = Path(directory) / "AppIcon.icns"
    subprocess.run(["/usr/bin/iconutil", "--convert", "icns", "--output", str(icon), str(iconset)], check=True)
    bundle_resources = app / "Contents/Resources"
    bundle_resources.mkdir(parents=True, exist_ok=True)
    shutil.copy2(icon, bundle_resources / icon.name)
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
    plistlib.dump({"CFBundleIdentifier": app_id, "CFBundleName": name, "CFBundleDisplayName": name, "CFBundleExecutable": executable, "CFBundlePackageType": "APPL", "CFBundleIconFile": "AppIcon.icns", "CFBundleVersion": sequence, "CFBundleShortVersionString": version, "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "13.0"}, file)
resources=app/"Contents/Resources/licenses"
resources.mkdir(parents=True,exist_ok=True)
for source in (root/"third_party").glob("*-LICENSE.txt"):
    shutil.copy2(source,resources/source.name)
shutil.copy2(root/"THIRD_PARTY_NOTICES.md",resources/"THIRD_PARTY_NOTICES.md")

# Every variant can be opened or moved through Finder. Replacing a linker-signed
# executable invalidates an existing bundle seal, including after Release -> Debug.
# Sign only after all binaries, plist and resources have reached their final paths.
subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(app)], check=True)
subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)], check=True)
print(app)
