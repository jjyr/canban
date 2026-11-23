#!/usr/bin/env python3
"""Build, bundle, launch, and exercise the standalone iOS Simulator app (.local/ios/Canban.app)."""
from __future__ import annotations

import argparse
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IOS_SIM_TARGET = "aarch64-apple-ios-sim"
BUNDLE_ID = "dev.canban.mobile"
EXECUTABLE_NAME = "canban-ios"
URL_SCHEME = "canban"
IOS_DEPLOYMENT_TARGET = "15.0"


def _cargo_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("RUSTUP_AUTO_INSTALL", "0")
    env["IPHONEOS_DEPLOYMENT_TARGET"] = IOS_DEPLOYMENT_TARGET
    return env


def _run(cmd: list[str], *, env: dict[str, str] | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, env=env, text=True, check=check)


def _capture(cmd: list[str], *, env: dict[str, str] | None = None) -> str:
    return subprocess.check_output(cmd, env=env, text=True)


def ensure_sim_target_installed(env: dict[str, str]) -> None:
    installed = _capture(["rustup", "target", "list", "--installed"], env=env).splitlines()
    if IOS_SIM_TARGET not in installed:
        _run(["rustup", "target", "add", IOS_SIM_TARGET], env=env)


def build_sim(*, release: bool = False, out_app: Path | None = None) -> Path:
    env = _cargo_env()
    ensure_sim_target_installed(env)

    cmd = ["cargo", "build", "-p", "canban-ios", "--target", IOS_SIM_TARGET]
    if release:
        cmd.append("--release")
    _run(cmd, env=env)

    profile = "release" if release else "debug"
    binary = ROOT / "target" / IOS_SIM_TARGET / profile / EXECUTABLE_NAME
    if not binary.is_file():
        raise FileNotFoundError(f"Expected built iOS simulator binary at {binary}")

    app_dir = out_app or (ROOT / ".local" / "ios" / "Canban.app")
    app_dir.parent.mkdir(parents=True, exist_ok=True)

    staging = Path(tempfile.mkdtemp(prefix=".Canban.app-", dir=app_dir.parent))
    try:
        shutil.copy2(binary, staging / EXECUTABLE_NAME)
        os.chmod(staging / EXECUTABLE_NAME, 0o755)

        info_plist = {
            "CFBundleName": "Canban",
            "CFBundleDisplayName": "Canban",
            "CFBundleIdentifier": BUNDLE_ID,
            "CFBundleExecutable": EXECUTABLE_NAME,
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": os.environ.get("CANBAN_RELEASE_VERSION", "0.1.0"),
            "CFBundleVersion": os.environ.get("GITHUB_RUN_NUMBER", "1"),
            "CFBundleSupportedPlatforms": ["iPhoneSimulator"],
            "MinimumOSVersion": IOS_DEPLOYMENT_TARGET,
            "UIDeviceFamily": [1, 2],
            "LSRequiresIPhoneOS": True,
            "UILaunchScreen": {},
            "UIApplicationSupportsMultipleScenes": False,
            "CADisableMinimumFrameDurationOnPhone": True,
            "NSAppTransportSecurity": {"NSAllowsLocalNetworking": True},
            "NSLocalNetworkUsageDescription": (
                "Connect to Canban daemons on your local network or Tailscale."
            ),
            "UISupportedInterfaceOrientations": [
                "UIInterfaceOrientationPortrait",
                "UIInterfaceOrientationLandscapeLeft",
                "UIInterfaceOrientationLandscapeRight",
            ],
            "UISupportedInterfaceOrientations~ipad": [
                "UIInterfaceOrientationPortrait",
                "UIInterfaceOrientationPortraitUpsideDown",
                "UIInterfaceOrientationLandscapeLeft",
                "UIInterfaceOrientationLandscapeRight",
            ],
            "CFBundleURLTypes": [
                {
                    "CFBundleTypeRole": "Editor",
                    "CFBundleURLName": BUNDLE_ID,
                    "CFBundleURLSchemes": [URL_SCHEME],
                }
            ],
        }
        with (staging / "Info.plist").open("wb") as fh:
            plistlib.dump(info_plist, fh, fmt=plistlib.FMT_XML)

        _run(["/usr/bin/codesign", "--force", "--sign", "-", str(staging)])

        if app_dir.exists():
            shutil.rmtree(app_dir)
        os.replace(staging, app_dir)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise

    print(f"Packaged iOS Simulator bundle: {app_dir}")
    return app_dir


def _runtime_version_key(runtime_id: str) -> tuple[int, ...]:
    nums = re.findall(r"\d+", runtime_id)
    return tuple(int(n) for n in nums)


def _list_iphone_devices() -> list[tuple[tuple[int, ...], bool, str, str]]:
    raw = _capture(["xcrun", "simctl", "list", "devices", "available", "-j"])
    data = json.loads(raw)
    devices: list[tuple[tuple[int, ...], bool, str, str]] = []
    for runtime_id, entries in data.get("devices", {}).items():
        if "SimRuntime.iOS" not in runtime_id:
            continue
        ver = _runtime_version_key(runtime_id)
        for entry in entries:
            if not entry.get("isAvailable", False):
                continue
            name = entry.get("name", "")
            udid = entry.get("udid", "")
            if not udid or "iPhone" not in name:
                continue
            booted = entry.get("state") == "Booted"
            devices.append((ver, booted, name, udid))
    return devices


def resolve_simulator_udid(device_arg: str | None) -> tuple[str, str, bool]:
    devices = _list_iphone_devices()
    if device_arg:
        for _, booted, name, udid in devices:
            if device_arg == udid or device_arg.lower() == name.lower():
                return udid, name, booted
        raise RuntimeError(f"Available iPhone simulator {device_arg!r} not found")

    booted_devices = [d for d in devices if d[1]]
    if booted_devices:
        booted_devices.sort(key=lambda d: (d[0], "Pro" in d[2], d[2]), reverse=True)
        _, _, name, udid = booted_devices[0]
        return udid, name, True

    if not devices:
        raise RuntimeError("No available iPhone simulator runtimes found via xcrun simctl")

    devices.sort(key=lambda d: (d[0], "Pro" in d[2], d[2]), reverse=True)
    _, _, name, udid = devices[0]
    return udid, name, False


def _open_simulator_ui() -> None:
    if subprocess.run(["open", "-ga", "Simulator"], capture_output=True).returncode == 0:
        return
    dev_dir = subprocess.run(["xcode-select", "-p"], capture_output=True, text=True)
    if dev_dir.returncode == 0 and dev_dir.stdout.strip():
        hub = Path(dev_dir.stdout.strip()).parent / "Applications" / "DeviceHub.app"
        if hub.exists():
            _run(["open", "-g", str(hub)], check=False)


def ensure_simulator_booted(device_arg: str | None, *, open_ui: bool = True) -> str:
    udid, name, already_booted = resolve_simulator_udid(device_arg)
    if not already_booted:
        print(f"Booting simulator {name} ({udid})...")
        _run(["xcrun", "simctl", "boot", udid], check=False)
        _run(["xcrun", "simctl", "bootstatus", udid, "-b"])
    else:
        print(f"Using booted simulator {name} ({udid})")
    if open_ui:
        _open_simulator_ui()
    return udid


def run_sim(
    *,
    build: bool = True,
    release: bool = False,
    device_arg: str | None = None,
    open_ui: bool = True,
) -> None:
    app_dir = ROOT / ".local" / "ios" / "Canban.app"
    if build or not app_dir.exists():
        app_dir = build_sim(release=release, out_app=app_dir)

    udid = ensure_simulator_booted(device_arg, open_ui=open_ui)
    _run(["xcrun", "simctl", "terminate", udid, BUNDLE_ID], check=False)
    _run(["xcrun", "simctl", "install", udid, str(app_dir)])
    _run(["xcrun", "simctl", "launch", udid, BUNDLE_ID])
    print(f"Launched {BUNDLE_ID} on simulator {udid}")


def open_url(url: str, *, device_arg: str | None = None) -> None:
    udid = ensure_simulator_booted(device_arg, open_ui=False)
    _run(["xcrun", "simctl", "openurl", udid, url])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_build = sub.add_parser("build-sim", help="Compile canban-ios and package .local/ios/Canban.app")
    p_build.add_argument("--release", action="store_true", help="Build with Cargo release profile")
    p_build.add_argument("--out", type=Path, help="Output .app bundle path")

    p_run = sub.add_parser("run-sim", help="Install and launch Canban.app in the iOS Simulator")
    p_run.add_argument("--no-build", action="store_true", help="Skip building if .local/ios/Canban.app exists")
    p_run.add_argument("--release", action="store_true", help="Build/launch release profile")
    p_run.add_argument("--device", help="Simulator UDID or device name (e.g. 'iPhone 18 Pro')")
    p_run.add_argument("--no-open", action="store_true", help="Do not open Simulator.app window")

    p_url = sub.add_parser("open-url", help="Send a deep link URL to the booted iOS Simulator")
    p_url.add_argument("url", help="Deep link URL (e.g. canban://...)")
    p_url.add_argument("--device", help="Simulator UDID or device name")

    args = parser.parse_args()
    if args.cmd == "build-sim":
        build_sim(release=args.release, out_app=args.out)
    elif args.cmd == "run-sim":
        run_sim(
            build=not args.no_build,
            release=args.release,
            device_arg=args.device,
            open_ui=not args.no_open,
        )
    elif args.cmd == "open-url":
        open_url(args.url, device_arg=args.device)
    return 0


if __name__ == "__main__":
    sys.exit(main())
