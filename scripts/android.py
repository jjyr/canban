#!/usr/bin/env python3
"""Build, package, and run the standalone Android APK (.local/android/Canban.apk) without Gradle."""
from __future__ import annotations

import argparse
import dataclasses
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ANDROID_TRIPLE = "aarch64-linux-android"
ANDROID_ABI = "arm64-v8a"
BUNDLE_ID = "dev.canban.mobile"
MIN_SDK = 26
COMPILE_SDK = 35
AVD_NAME = "canban_pixel"


@dataclasses.dataclass
class AndroidSdk:
    root: Path
    ndk: Path
    build_tools: Path
    android_jar: Path
    adb: Path
    emulator: Path

    @property
    def toolchain(self) -> Path:
        system = platform.system()
        host = (
            "darwin-x86_64"
            if system == "Darwin"
            else "windows-x86_64"
            if system == "Windows"
            else "linux-x86_64"
        )
        return self.ndk / "toolchains" / "llvm" / "prebuilt" / host

    def tool(self, name: str) -> Path:
        suffix = ".exe" if platform.system() == "Windows" else ""
        return self.build_tools / f"{name}{suffix}"


def _version_sort_key(path: Path) -> list[int]:
    parts: list[int] = []
    for tok in path.name.replace("-", ".").split("."):
        try:
            parts.append(int(tok))
        except ValueError:
            parts.append(0)
    return parts


def _newest_subdir(parent: Path) -> Path | None:
    if not parent.is_dir():
        return None
    dirs = [p for p in parent.iterdir() if p.is_dir()]
    if not dirs:
        return None
    dirs.sort(key=_version_sort_key)
    return dirs[-1]


def find_sdk() -> AndroidSdk:
    candidates = [
        Path(os.environ[k])
        for k in ("ANDROID_HOME", "ANDROID_SDK_ROOT")
        if os.environ.get(k)
    ]
    candidates.append(Path.home() / "Library" / "Android" / "sdk")
    candidates.append(Path.home() / "Android" / "Sdk")
    sdk_root = next((p for p in candidates if p.is_dir()), None)
    if sdk_root is None:
        raise RuntimeError(
            "Android SDK not found (checked ANDROID_HOME, ANDROID_SDK_ROOT, ~/Library/Android/sdk)"
        )

    ndk = (
        Path(os.environ["ANDROID_NDK_HOME"])
        if os.environ.get("ANDROID_NDK_HOME")
        else _newest_subdir(sdk_root / "ndk")
    )
    if ndk is None or not ndk.is_dir():
        raise RuntimeError(f"No NDK found under {sdk_root / 'ndk'}")

    build_tools = _newest_subdir(sdk_root / "build-tools")
    if build_tools is None:
        raise RuntimeError(f"No build-tools found under {sdk_root / 'build-tools'}")

    android_jar = sdk_root / "platforms" / f"android-{COMPILE_SDK}" / "android.jar"
    if not android_jar.is_file():
        raise RuntimeError(f"Missing platform jar: {android_jar}")

    adb = sdk_root / "platform-tools" / "adb"
    if not adb.is_file():
        resolved_adb = shutil.which("adb")
        if resolved_adb is None:
            raise RuntimeError("adb not found in platform-tools or PATH")
        adb = Path(resolved_adb)

    emulator = sdk_root / "emulator" / "emulator"
    return AndroidSdk(
        root=sdk_root,
        ndk=ndk,
        build_tools=build_tools,
        android_jar=android_jar,
        adb=adb,
        emulator=emulator,
    )


def _find_jdk_tool(name: str) -> str:
    env_home = os.environ.get("JAVA_HOME")
    if env_home:
        cand = Path(env_home) / "bin" / name
        if cand.is_file():
            return str(cand)
    homebrew_openjdk = Path("/opt/homebrew/opt/openjdk/bin") / name
    if homebrew_openjdk.is_file():
        return str(homebrew_openjdk)
    resolved = shutil.which(name)
    if resolved:
        return resolved
    raise RuntimeError(f"JDK tool `{name}` not found (set JAVA_HOME)")


def _run(cmd: list[str], *, env: dict[str, str] | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, env=env, text=True, check=check)


def _capture(cmd: list[str], *, env: dict[str, str] | None = None) -> str:
    return subprocess.check_output(cmd, env=env, text=True)


def ensure_android_target(env: dict[str, str]) -> None:
    installed = _capture(["rustup", "target", "list", "--installed"], env=env).splitlines()
    if ANDROID_TRIPLE not in installed:
        _run(["rustup", "target", "add", ANDROID_TRIPLE], env=env)


def ndk_cargo_env(sdk: AndroidSdk, triple: str = ANDROID_TRIPLE) -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("RUSTUP_AUTO_INSTALL", "0")
    toolchain = sdk.toolchain
    bin_dir = toolchain / "bin"
    clang_target = "armv7a-linux-androideabi" if triple == "armv7-linux-androideabi" else triple
    clang = bin_dir / f"{clang_target}{MIN_SDK}-clang"
    clangxx = bin_dir / f"{clang_target}{MIN_SDK}-clang++"
    ar = bin_dir / "llvm-ar"
    env_triple = triple.replace("-", "_")

    env[f"CC_{env_triple}"] = str(clang)
    env[f"CXX_{env_triple}"] = str(clangxx)
    env[f"AR_{env_triple}"] = str(ar)
    env[f"CARGO_TARGET_{env_triple.upper()}_LINKER"] = str(clang)

    sysroot = str(toolchain / "sysroot").replace("\\", "/")
    extra = f'--target={clang_target}{MIN_SDK} --sysroot="{sysroot}"'
    clang_root = toolchain / "lib" / "clang"
    if clang_root.is_dir():
        clang_inc = next(
            (p / "include" for p in sorted(clang_root.iterdir()) if (p / "include").is_dir()),
            None,
        )
        if clang_inc is not None:
            inc_str = str(clang_inc).replace("\\", "/")
            extra += f' -idirafter "{inc_str}"'
    env[f"BINDGEN_EXTRA_CLANG_ARGS_{env_triple}"] = extra
    return env


def _find_nsh_java_dir(env: dict[str, str]) -> Path:
    meta_raw = _capture(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", ANDROID_TRIPLE],
        env=env,
    )
    meta = json.loads(meta_raw)
    for pkg in meta.get("packages", []):
        if pkg.get("name") == "nsh-platform-android":
            java_dir = Path(pkg["manifest_path"]).parent / "java"
            if java_dir.is_dir():
                return java_dir
    raise RuntimeError("Could not locate `nsh-platform-android/java` via `cargo metadata`")


def _ensure_java_runtime(sdk: AndroidSdk, env: dict[str, str]) -> Path:
    runtime_dir = ROOT / ".local" / "android" / "runtime"
    dex_path = runtime_dir / "classes.dex"
    java_dir = _find_nsh_java_dir(env)
    sources = sorted(java_dir.rglob("*.java"))
    if not sources:
        raise RuntimeError(f"No .java files found under {java_dir}")

    if dex_path.is_file():
        dex_mtime = dex_path.stat().st_mtime
        if all(src.stat().st_mtime <= dex_mtime for src in sources):
            return dex_path

    classes_dir = runtime_dir / "classes"
    if classes_dir.exists():
        shutil.rmtree(classes_dir)
    classes_dir.mkdir(parents=True, exist_ok=True)

    print("Compiling nsh-platform-android Java runtime...")
    _run(
        [
            _find_jdk_tool("javac"),
            "--release",
            "17",
            "-Xlint:-options",
            "-encoding",
            "UTF-8",
            "-classpath",
            str(sdk.android_jar),
            "-d",
            str(classes_dir),
            *(str(s) for s in sources),
        ]
    )
    jar_path = runtime_dir / "classes.jar"
    if jar_path.exists():
        jar_path.unlink()
    _run([_find_jdk_tool("jar"), "cf", str(jar_path), "-C", str(classes_dir), "."])
    _run(
        [
            str(sdk.tool("d8")),
            "--min-api",
            str(MIN_SDK),
            "--lib",
            str(sdk.android_jar),
            "--output",
            str(runtime_dir),
            str(jar_path),
        ]
    )
    if not dex_path.is_file():
        raise RuntimeError(f"Expected {dex_path} after running d8")
    return dex_path


def _check_elf_16kb_alignment(sdk: AndroidSdk, so_path: Path) -> None:
    objdump = sdk.toolchain / "bin" / "llvm-objdump"
    headers = _capture([str(objdump), "-p", str(so_path)])
    loads = 0
    for line in headers.splitlines():
        tokens = line.split()
        if tokens and tokens[0] == "LOAD":
            loads += 1
            if "align 2**" not in line:
                raise RuntimeError(f"Missing alignment on ELF LOAD segment: {line}")
            exp = int(line.rsplit("align 2**", 1)[1].strip())
            if exp < 14:
                raise RuntimeError(f"{so_path} LOAD segment alignment 2**{exp} < 2**14 (16 KB)")
    if loads == 0:
        raise RuntimeError(f"No ELF LOAD segments found in {so_path}")


def _ensure_debug_keystore() -> Path:
    keystore = Path.home() / ".android" / "debug.keystore"
    if keystore.is_file():
        return keystore
    keystore.parent.mkdir(parents=True, exist_ok=True)
    _run(
        [
            _find_jdk_tool("keytool"),
            "-genkeypair",
            "-v",
            "-keystore",
            str(keystore),
            "-storepass",
            "android",
            "-alias",
            "androiddebugkey",
            "-keypass",
            "android",
            "-keyalg",
            "RSA",
            "-keysize",
            "2048",
            "-validity",
            "10000",
            "-dname",
            "CN=Android Debug,O=Android,C=US",
        ]
    )
    return keystore


def _manifest_xml() -> str:
    return f"""<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="{BUNDLE_ID}">
    <uses-permission android:name="android.permission.INTERNET" />
    <queries>
        <intent>
            <action android:name="android.intent.action.PROCESS_TEXT" />
            <data android:mimeType="text/plain" />
        </intent>
    </queries>
    <application android:label="Canban"
        android:usesCleartextTraffic="true"
        android:extractNativeLibs="false"
        android:enableOnBackInvokedCallback="true"
        android:supportsRtl="true"
        android:theme="@android:style/Theme.Material.NoActionBar">
        <meta-data android:name="com.longbridge.nsh.RUST" android:value="true" />
        <provider android:name="com.longbridge.nsh.FilesProvider"
            android:authorities="{BUNDLE_ID}.files"
            android:exported="false"
            android:grantUriPermissions="true" />
        <activity android:name="com.longbridge.nsh.AppActivity" android:exported="true"
            android:launchMode="singleTask"
            android:windowSoftInputMode="adjustResize"
            android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density|fontScale|locale|layoutDirection">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
            <intent-filter>
                <action android:name="android.intent.action.VIEW" />
                <category android:name="android.intent.category.DEFAULT" />
                <category android:name="android.intent.category.BROWSABLE" />
                <data android:scheme="canban" />
            </intent-filter>
        </activity>
    </application>
</manifest>
"""


def check_android() -> None:
    sdk = find_sdk()
    env = ndk_cargo_env(sdk)
    ensure_android_target(env)
    _run(
        [
            "cargo",
            "build",
            "-p",
            "canban-terminal",
            "--no-default-features",
            "--target",
            ANDROID_TRIPLE,
            "--example",
            "vt_smoke",
        ],
        env=env,
    )
    _run(
        [
            "cargo",
            "check",
            "-p",
            "canban-android",
            "--target",
            ANDROID_TRIPLE,
        ],
        env=env,
    )


def build_apk(*, release: bool = False, out_apk: Path | None = None) -> Path:
    sdk = find_sdk()
    env = ndk_cargo_env(sdk)
    ensure_android_target(env)

    cmd = ["cargo", "build", "-p", "canban-android", "--target", ANDROID_TRIPLE]
    if release:
        cmd.append("--release")
    _run(cmd, env=env)

    profile = "release" if release else "debug"
    raw_so = ROOT / "target" / ANDROID_TRIPLE / profile / "libcanban_android.so"
    if not raw_so.is_file():
        raise FileNotFoundError(f"Expected built cdylib at {raw_so}")
    _check_elf_16kb_alignment(sdk, raw_so)

    dex_path = _ensure_java_runtime(sdk, env)
    apk_path = out_apk or (ROOT / ".local" / "android" / "Canban.apk")
    apk_path.parent.mkdir(parents=True, exist_ok=True)

    work = Path(tempfile.mkdtemp(prefix=".Canban.apk-", dir=apk_path.parent))
    try:
        so_to_pack = raw_so
        if release:
            stripped_so = work / "libnsh.so"
            _run(
                [
                    str(sdk.toolchain / "bin" / "llvm-strip"),
                    "--strip-unneeded",
                    "-o",
                    str(stripped_so),
                    str(raw_so),
                ]
            )
            _check_elf_16kb_alignment(sdk, stripped_so)
            so_to_pack = stripped_so

        manifest = work / "AndroidManifest.xml"
        manifest.write_text(_manifest_xml(), encoding="utf-8")

        flat_res: list[str] = []
        res_dir = ROOT / "crates" / "android" / "res"
        if res_dir.is_dir():
            compiled_res = work / "compiled-res"
            compiled_res.mkdir(parents=True, exist_ok=True)
            for res_file in sorted(res_dir.rglob("*.xml")):
                _run(
                    [
                        str(sdk.tool("aapt2")),
                        "compile",
                        str(res_file),
                        "-o",
                        str(compiled_res),
                    ]
                )
            flat_res = [str(p) for p in sorted(compiled_res.glob("*.flat"))]

        base_apk = work / "base.apk"
        _run(
            [
                str(sdk.tool("aapt2")),
                "link",
                "-o",
                str(base_apk),
                "-I",
                str(sdk.android_jar),
                "--manifest",
                str(manifest),
                "--min-sdk-version",
                str(MIN_SDK),
                "--target-sdk-version",
                str(COMPILE_SDK),
                "--version-code",
                os.environ.get("GITHUB_RUN_NUMBER", "1"),
                "--version-name",
                os.environ.get("CANBAN_RELEASE_VERSION", "0.1.0"),
                "--debug-mode",
                *flat_res,
            ]
        )

        unaligned_apk = work / "unaligned.apk"
        with (
            zipfile.ZipFile(base_apk, "r") as src_zip,
            zipfile.ZipFile(unaligned_apk, "w") as dst_zip,
        ):
            for info in src_zip.infolist():
                dst_zip.writestr(info, src_zip.read(info.filename))
            dst_zip.write(dex_path, "classes.dex", compress_type=zipfile.ZIP_DEFLATED)
            # extractNativeLibs="false" requires storing native shared libraries uncompressed.
            dst_zip.write(
                so_to_pack,
                f"lib/{ANDROID_ABI}/libnsh.so",
                compress_type=zipfile.ZIP_STORED,
            )

        aligned_apk = work / "aligned.apk"
        _run(
            [
                str(sdk.tool("zipalign")),
                "-P",
                "16",
                "-f",
                "4",
                str(unaligned_apk),
                str(aligned_apk),
            ]
        )
        keystore = _ensure_debug_keystore()
        _run(
            [
                str(sdk.tool("apksigner")),
                "sign",
                "--ks",
                str(keystore),
                "--ks-key-alias",
                "androiddebugkey",
                "--ks-pass",
                "pass:android",
                "--key-pass",
                "pass:android",
                str(aligned_apk),
            ]
        )
        os.replace(aligned_apk, apk_path)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    print(f"Packaged Android APK: {apk_path}")
    return apk_path


def _adb_cmd(sdk: AndroidSdk, serial: str | None, *args: str) -> list[str]:
    base = [str(sdk.adb)]
    if serial:
        base.extend(["-s", serial])
    base.extend(args)
    return base


def _online_devices(sdk: AndroidSdk) -> list[str]:
    out = _capture([str(sdk.adb), "devices"])
    serials: list[str] = []
    for line in out.splitlines()[1:]:
        parts = line.split()
        if len(parts) >= 2 and parts[1] == "device":
            serials.append(parts[0])
    return serials


def ensure_emulator_booted(
    sdk: AndroidSdk,
    *,
    serial: str | None = None,
    avd_name: str = AVD_NAME,
    headless: bool = False,
) -> str:
    if serial:
        return serial
    devices = _online_devices(sdk)
    if devices:
        preferred = next((d for d in devices if d.startswith("emulator-")), devices[0])
        print(f"Using online Android device {preferred}")
        return preferred

    if not sdk.emulator.is_file():
        raise RuntimeError(f"Android emulator binary not found at {sdk.emulator}")

    log_dir = ROOT / ".local" / "android"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / "emulator.log"
    cmd = [str(sdk.emulator), "-avd", avd_name, "-no-snapshot-load", "-no-boot-anim"]
    if headless:
        cmd.append("-no-window")
    env = os.environ.copy()
    env.setdefault("ANDROID_HOME", str(sdk.root))
    env.setdefault("ANDROID_SDK_ROOT", str(sdk.root))

    print(f"Booting Android Emulator AVD `{avd_name}` (log: {log_path})...")
    with log_path.open("wb") as log_file:
        subprocess.Popen(
            cmd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )

    _run([str(sdk.adb), "wait-for-device"])
    deadline = time.monotonic() + 180.0
    while time.monotonic() < deadline:
        devices = _online_devices(sdk)
        if devices:
            target = next((d for d in devices if d.startswith("emulator-")), devices[0])
            res = subprocess.run(
                _adb_cmd(sdk, target, "shell", "getprop", "sys.boot_completed"),
                capture_output=True,
                text=True,
            )
            if res.returncode == 0 and res.stdout.strip() == "1":
                print(f"Android Emulator ready ({target})")
                return target
        time.sleep(1.5)
    raise RuntimeError(f"Timed out waiting for Android emulator boot (check {log_path})")


def run_emu(
    *,
    build: bool = True,
    release: bool = False,
    serial: str | None = None,
    avd_name: str = AVD_NAME,
    headless: bool = False,
) -> None:
    sdk = find_sdk()
    apk_path = ROOT / ".local" / "android" / "Canban.apk"
    if build or not apk_path.is_file():
        apk_path = build_apk(release=release, out_apk=apk_path)

    target = ensure_emulator_booted(sdk, serial=serial, avd_name=avd_name, headless=headless)
    _run(_adb_cmd(sdk, target, "install", "-r", "-t", str(apk_path)))
    _run(
        _adb_cmd(
            sdk,
            target,
            "shell",
            "am",
            "start",
            "-W",
            "-n",
            f"{BUNDLE_ID}/com.longbridge.nsh.AppActivity",
        )
    )
    print(f"Launched {BUNDLE_ID} on Android target {target}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("check", help="Cross-compile check canban-terminal and canban-android for aarch64-linux-android")

    p_build = sub.add_parser("build-apk", help="Compile canban-android and package .local/android/Canban.apk")
    p_build.add_argument("--release", action="store_true", help="Build with Cargo release profile and strip symbols")
    p_build.add_argument("--out", type=Path, help="Output APK path")

    p_boot = sub.add_parser("boot-emu", help="Boot AVD and wait until Android is ready")
    p_boot.add_argument("--avd", default=AVD_NAME, help="AVD name")
    p_boot.add_argument("--headless", action="store_true", help="Start emulator with -no-window")

    p_run = sub.add_parser("run-emu", help="Build, install, and launch Canban.apk on Android emulator/device")
    p_run.add_argument("--no-build", action="store_true", help="Skip building if .local/android/Canban.apk exists")
    p_run.add_argument("--release", action="store_true", help="Build/launch release APK")
    p_run.add_argument("--serial", help="Specific adb device serial")
    p_run.add_argument("--avd", default=AVD_NAME, help="AVD name when booting emulator")
    p_run.add_argument("--headless", action="store_true", help="Start emulator with -no-window")

    args = parser.parse_args()
    if args.cmd == "check":
        check_android()
    elif args.cmd == "build-apk":
        build_apk(release=args.release, out_apk=args.out)
    elif args.cmd == "boot-emu":
        sdk = find_sdk()
        ensure_emulator_booted(sdk, avd_name=args.avd, headless=args.headless)
    elif args.cmd == "run-emu":
        run_emu(
            build=not args.no_build,
            release=args.release,
            serial=args.serial,
            avd_name=args.avd,
            headless=args.headless,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
