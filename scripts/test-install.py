#!/usr/bin/env python3
"""Isolated regressions for scripts/install.sh (macOS & Linux desktop, --headless, Herdr PTY survival, systemd coordination, and safety guards)."""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
import pathlib
import shutil
import socket
import socketserver
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parents[1]
INSTALL_SH = ROOT / "scripts/install.sh"

spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def bundle_cli_relpath(is_macos: bool) -> pathlib.Path:
    return pathlib.Path("Contents/MacOS/canban" if is_macos else "bin/canban")


def make_fixture_release(source_bundle: pathlib.Path, work_dir: pathlib.Path, version: str, commit_char: str) -> dict:
    is_macos = source_bundle.suffix == ".app"
    bundle_name = "Canban.app" if is_macos else "canban"
    bundle = work_dir / "src" / bundle_name
    shutil.copytree(source_bundle, bundle)
    cli_rel = bundle_cli_relpath(is_macos)
    cli = bundle / cli_rel
    real_cli = cli.with_name("canban-real")
    cli.rename(real_cli)

    base_info = json.loads(subprocess.check_output([str(real_cli), "build-info"], text=True))
    info = {
        **base_info,
        "version": version,
        "commit": commit_char * 40,
    }
    wrapper = f"""#!/bin/sh
if [ "${{1:-}}" = "build-info" ]; then
  printf '%s\\n' '{json.dumps(info)}'
  exit 0
fi
exec "$(dirname "$0")/canban-real" "$@"
"""
    cli.write_text(wrapper)
    cli.chmod(0o755)
    if is_macos:
        run_checked(
            ["/usr/bin/codesign", "--force", "--deep", "--sign", "-", "--timestamp=none", str(bundle)]
        )
    else:
        meta_path = bundle / "bundle.json"
        meta = json.loads(meta_path.read_text()) if meta_path.is_file() else {}
        meta.update({
            "app_id": "com.canban.desktop",
            "name": "Canban",
            "executable": "canban-ui",
            "version": version,
            "commit": commit_char * 40,
        })
        meta_path.write_text(json.dumps(meta, indent=2) + "\n")

    gz_name = f"Canban-{info['platform']}.tar.gz"
    gz_path = work_dir / gz_name
    release.pack_tar_gz(bundle, gz_path)

    zst_name = f"Canban-{info['platform']}.tar.zst"
    zst_path = work_dir / zst_name
    zst_path.write_bytes(b"zstd-placeholder")

    manifest = {
        **info,
        "rollback_floor": 8,
        "asset": zst_name,
        "size": zst_path.stat().st_size,
        "sha256": hashlib.sha256(zst_path.read_bytes()).hexdigest(),
        "tar_gz": gz_name,
        "tar_gz_size": gz_path.stat().st_size,
        "tar_gz_sha256": hashlib.sha256(gz_path.read_bytes()).hexdigest(),
    }
    return {
        "version": version,
        "info": info,
        "manifest": manifest,
        "gz_name": gz_name,
        "gz_path": gz_path,
        "corrupt_sha": False,
    }


def make_synthetic_linux_feed(feed_root: pathlib.Path) -> None:
    """Build a self-contained file:// feed with Linux x86_64 + aarch64 desktop & CLI archives."""
    assets_dir = feed_root / "assets"
    assets_dir.mkdir(parents=True, exist_ok=True)
    releases_json = []

    for ver, commit_ch in (("20261115-1", "1"), ("20261115-2", "2")):
        rel_assets = []
        for arch, platform in (("x86_64", "linux-x86_64"), ("aarch64", "linux-aarch64")):
            info = {
                "version": ver,
                "commit": commit_ch * 40,
                "schema": 20,
                "api": 58,
                "platform": platform,
            }
            src_root = feed_root / f"build-{ver}-{arch}" / "canban"
            (src_root / "bin").mkdir(parents=True)
            (src_root / "share/applications").mkdir(parents=True)
            for sz in ("scalable", "256x256", "512x512"):
                (src_root / f"share/icons/hicolor/{sz}/apps").mkdir(parents=True)
            ext = "svg" if sz == "scalable" else "png"
            (src_root / "share/icons/hicolor/scalable/apps/com.canban.desktop.svg").write_text("<svg/>\n")
            (src_root / "share/icons/hicolor/256x256/apps/com.canban.desktop.png").write_bytes(b"png256")
            (src_root / "share/icons/hicolor/512x512/apps/com.canban.desktop.png").write_bytes(b"png512")
            (src_root / "share/applications/com.canban.desktop.desktop").write_text(
                "[Desktop Entry]\nType=Application\nName=Canban\nExec=canban-ui\nIcon=com.canban.desktop\n"
            )
            (src_root / "bundle.json").write_text(json.dumps({
                "app_id": "com.canban.desktop",
                "name": "Canban",
                "executable": "canban-ui",
                "version": ver,
                "commit": commit_ch * 40,
                "sequence": "1",
            }, indent=2) + "\n")

            cli_script = f"""#!/bin/sh
set -eu
HOME_ARG=""
while [ $# -gt 0 ]; do
  case "$1" in
    build-info)
      printf '%s\\n' '{json.dumps(info)}'
      exit 0
      ;;
    --home)
      HOME_ARG="$2"
      shift 2
      ;;
    call)
      METHOD="$2"
      if [ "$METHOD" = "app.ping" ] && [ -n "$HOME_ARG" ] && [ -f "$HOME_ARG/mock-server.pid" ]; then
        PID="$(cat "$HOME_ARG/mock-server.pid")"
        printf '{{"pid":%s,"home":"%s"}}\\n' "$PID" "$HOME_ARG"
        exit 0
      fi
      exit 1
      ;;
    *)
      shift
      ;;
  esac
done
exit 0
"""
            for b in ("canban", "canban-ui", "canban-updater"):
                p = src_root / "bin" / b
                p.write_text(cli_script if b == "canban" else "#!/bin/sh\nexit 0\n")
                p.chmod(0o755)

            ver_dir = assets_dir / ver
            ver_dir.mkdir(parents=True, exist_ok=True)
            desk_gz = ver_dir / f"Canban-{platform}.tar.gz"
            release.pack_tar_gz(src_root, desk_gz)
            desk_zst = ver_dir / f"Canban-{platform}.tar.zst"
            desk_zst.write_bytes(b"zst-unused")
            desk_manifest = {
                **info,
                "rollback_floor": 8,
                "asset": desk_zst.name,
                "size": desk_zst.stat().st_size,
                "sha256": hashlib.sha256(desk_zst.read_bytes()).hexdigest(),
                "tar_gz": desk_gz.name,
                "tar_gz_size": desk_gz.stat().st_size,
                "tar_gz_sha256": hashlib.sha256(desk_gz.read_bytes()).hexdigest(),
            }
            desk_manifest_path = ver_dir / f"manifest-{platform}.json"
            desk_manifest_path.write_text(json.dumps(desk_manifest, indent=2) + "\n")

            cli_ver_dir = ver_dir / "cli"
            cli_ver_dir.mkdir(parents=True, exist_ok=True)
            cli_gz = cli_ver_dir / f"canban-cli-{platform}.tar.gz"
            release.pack_cli_tar_gz(src_root / "bin/canban", cli_gz)
            cli_manifest = {
                **info,
                "asset": cli_gz.name,
                "size": cli_gz.stat().st_size,
                "sha256": hashlib.sha256(cli_gz.read_bytes()).hexdigest(),
            }
            cli_manifest_path = cli_ver_dir / f"manifest-cli-{platform}.json"
            cli_manifest_path.write_text(json.dumps(cli_manifest, indent=2) + "\n")

            for fpath in (desk_manifest_path, desk_gz, cli_manifest_path, cli_gz):
                rel_assets.append({
                    "name": fpath.name,
                    "browser_download_url": f"file://{fpath}",
                })

        releases_json.append({
            "tag_name": ver,
            "draft": False,
            "prerelease": False,
            "assets": rel_assets,
        })

    repo_dir = feed_root / "repos/fixture/canban"
    repo_dir.mkdir(parents=True, exist_ok=True)
    (repo_dir / "releases.json").write_text(json.dumps(releases_json, indent=2) + "\n")


def run_linux_mode_and_coexistence_tests():
    base = pathlib.Path(tempfile.mkdtemp(prefix="cb-install-linux-modes-"))
    try:
        feed_dir = base / "feed"
        make_synthetic_linux_feed(feed_dir)
        clean_env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("CANBAN_", "HC_", "XDG_"))
        }

        def invoke(args, extra_env=None, check=True):
            env = {
                **clean_env,
                "HOME": str(base / "fake-home"),
                "CANBAN_INSTALL_OS": "Linux",
                "CANBAN_INSTALL_ARCH": "x86_64",
                "CANBAN_INSTALL_REPO": "fixture/canban",
                **(extra_env or {}),
            }
            pathlib.Path(env["HOME"]).mkdir(parents=True, exist_ok=True)
            bash_bin = shutil.which("bash") or "/bin/bash"
            res = subprocess.run(
                [bash_bin, str(INSTALL_SH), "--feed", str(feed_dir), *args],
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
            )
            if check and res.returncode != 0:
                raise AssertionError(
                    f"Linux mode install.sh failed ({res.returncode}):\nSTDOUT:\n{res.stdout}\nSTDERR:\n{res.stderr}"
                )
            return res

        # A. Headless install on clean host (x86_64 -> 20261115-1 -> upgrade 20261115-2)
        h_bin = base / "headless/bin"
        h_home = base / "headless/home"
        res = invoke(["--headless", "--bin-dir", str(h_bin), "--home", str(h_home), "--version", "20261115-1"])
        assert "Installed Canban 20261115-1" in res.stdout, res.stdout
        assert (h_bin / "canban").is_file() and not (h_bin / "canban").is_symlink()
        assert not (h_bin / "canban-ui").exists()
        info1 = json.loads(subprocess.check_output([str(h_bin / "canban"), "build-info"], text=True))
        assert info1["version"] == "20261115-1" and info1["platform"] == "linux-x86_64"

        res = invoke(["--headless", "--bin-dir", str(h_bin), "--home", str(h_home)])
        assert "Installed Canban 20261115-2" in res.stdout, res.stdout
        info2 = json.loads(subprocess.check_output([str(h_bin / "canban"), "build-info"], text=True))
        assert info2["version"] == "20261115-2"

        # B. Linux aarch64 desktop install + missing GTK4 shared library hint
        arm_prefix = base / "arm64-local"
        arm_dest = arm_prefix / "opt/canban"
        arm_home = base / "arm64-home"
        mock_tools = base / "mock-tools"
        mock_tools.mkdir()
        mock_ldd = mock_tools / "ldd"
        mock_ldd.write_text("#!/bin/sh\necho '\tlibgtk-4.so.1 => not found'\n")
        mock_ldd.chmod(0o755)
        res = invoke(
            ["--dest", str(arm_dest), "--home", str(arm_home), "--version", "20261115-1"],
            extra_env={
                "CANBAN_INSTALL_ARCH": "aarch64",
                "PATH": f"{mock_tools}:{clean_env.get('PATH', '/usr/bin:/bin')}",
            },
        )
        assert "Installed Canban 20261115-1" in res.stdout, res.stdout
        assert "libgtk-4-1" in res.stdout and "pacman -S gtk4" in res.stdout, res.stdout
        assert (arm_prefix / "bin/canban").is_symlink()
        assert (arm_prefix / "bin/canban-ui").is_symlink()
        assert (arm_prefix / "share/applications/com.canban.desktop.desktop").is_file()
        assert (arm_prefix / "share/icons/hicolor/512x512/apps/com.canban.desktop.png").is_file()

        # C. Coexistence guard: `--headless` when desktop bundle is ALREADY installed upgrades full bundle!
        res = invoke(
            ["--headless", "--dest", str(arm_dest), "--home", str(arm_home)],
            extra_env={"CANBAN_INSTALL_ARCH": "aarch64"},
        )
        assert "Detected existing Canban desktop bundle" in res.stdout, res.stdout
        assert "Installed Canban 20261115-2" in res.stdout, res.stdout
        assert (arm_prefix / "bin/canban").is_symlink()
        desk_meta = json.loads((arm_dest / "bundle.json").read_text())
        assert desk_meta["version"] == "20261115-2", desk_meta

        # D. systemd --user canban.service stop/start coordination (isolated mock systemctl)
        sysd_dir = base / "systemd-unit"
        sysd_dir.mkdir()
        sysd_home = base / "sysd-home"
        sysd_home.mkdir()
        (sysd_dir / "canban.service").write_text(
            f'[Service]\nExecStart="/tmp/bin/canban" --home "{sysd_home}" serve\nRestart=always\n'
        )
        sysd_log = base / "systemctl.log"
        sysd_state = base / "service-active"
        sysd_state.write_text("1")
        mock_systemctl = mock_tools / "mock-systemctl"
        mock_systemctl.write_text(f"""#!/bin/sh
echo "$*" >> "{sysd_log}"
case "$*" in
  *is-active*canban.service*)
    [ -f "{sysd_state}" ] && exit 0 || exit 3
    ;;
  *stop*canban.service*)
    rm -f "{sysd_state}" "{sysd_home}/mock-server.pid"
    exit 0
    ;;
  *start*canban.service*)
    touch "{sysd_state}"
    echo "99999" > "{sysd_home}/mock-server.pid"
    exit 0
    ;;
esac
exit 0
""")
        mock_systemctl.chmod(0o755)

        # Unrelated --home must NOT stop/start canban.service
        other_home = base / "other-home"
        invoke(
            ["--headless", "--bin-dir", str(base / "other-bin"), "--home", str(other_home), "--version", "20261115-1"],
            extra_env={
                "CANBAN_SYSTEMCTL_BIN": str(mock_systemctl),
                "CANBAN_SYSTEMD_UNIT_DIR": str(sysd_dir),
            },
        )
        assert not sysd_log.exists(), f"Unexpected systemctl call for unrelated home: {sysd_log.read_text()}"

        # Matching --home stops canban.service before swap and starts it after swap!
        sysd_prefix = base / "sysd-local"
        res = invoke(
            ["--dest", str(sysd_prefix / "opt/canban"), "--home", str(sysd_home)],
            extra_env={
                "CANBAN_SYSTEMCTL_BIN": str(mock_systemctl),
                "CANBAN_SYSTEMD_UNIT_DIR": str(sysd_dir),
            },
        )
        calls = sysd_log.read_text().splitlines()
        assert "--user is-active --quiet canban.service" in calls, calls
        assert "--user stop canban.service" in calls, calls
        assert "--user start canban.service" in calls, calls
        assert calls.index("--user stop canban.service") < calls.index("--user start canban.service"), calls
        print("Linux desktop, --headless, coexistence, and systemd coordination tests OK", flush=True)
    finally:
        shutil.rmtree(base, ignore_errors=True)


def pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def run_checked(cmd, **kw):
    kw.setdefault("capture_output", True)
    kw.setdefault("text", True)
    res = subprocess.run(cmd, **kw)
    if res.returncode != 0:
        raise AssertionError(
            f"Command {cmd!r} failed ({res.returncode}):\nSTDOUT:\n{res.stdout}\nSTDERR:\n{res.stderr}"
        )
    return res


def rpc(sock_path: pathlib.Path, method: str, params=None):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.settimeout(10)
        s.connect(str(sock_path))
        s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}) + "\n").encode())
        resp = json.loads(s.makefile("rb").readline())
        if "error" in resp:
            raise RuntimeError(str(resp["error"]))
        return resp["result"]


def wait_until(fn, label: str, timeout: float = 30.0):
    deadline = time.monotonic() + timeout
    last_err = None
    while time.monotonic() < deadline:
        try:
            val = fn()
            if val:
                return val
        except Exception as exc:
            last_err = exc
        time.sleep(0.1)
    raise RuntimeError(f"Timed out waiting for {label}: {last_err}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bundle",
        type=pathlib.Path,
        default=ROOT / (".local/Canban.app" if sys.platform == "darwin" else ".local/canban"),
        help="Packaged local desktop bundle (.local/Canban.app or .local/canban)",
    )
    parser.add_argument(
        "--synthetic-only",
        action="store_true",
        help="Run only synthetic cross-platform install.sh regressions (no compiled Rust bundle required)",
    )
    args = parser.parse_args()

    run_linux_mode_and_coexistence_tests()
    if args.synthetic_only:
        print("ALL_SYNTHETIC_INSTALL_TESTS_PASSED", flush=True)
        return

    source_bundle = args.bundle.resolve()
    is_macos = source_bundle.suffix == ".app"
    cli_rel = bundle_cli_relpath(is_macos)
    if not (source_bundle / cli_rel).is_file():
        raise SystemExit(f"Packaged bundle not found at {source_bundle}; run `mise run build` first")

    hc = shutil.which("hc") or next(
        (
            str(p)
            for p in [
                pathlib.Path.home() / ".local/bin/hc",
                pathlib.Path("/opt/homebrew/bin/hc"),
                pathlib.Path("/usr/local/bin/hc"),
            ]
            if p.is_file()
        ),
        None,
    )
    if not hc:
        raise SystemExit("Herdr CLI `hc` is required in PATH for test-install.py")

    tmp_parent = "/private/tmp" if sys.platform == "darwin" and pathlib.Path("/private/tmp").is_dir() else None
    base = pathlib.Path(tempfile.mkdtemp(prefix="cb-install-test-", dir=tmp_parent))
    print(f"INSTALL_TEST_BASE={base}", flush=True)

    releases_store = {}
    core_env = {
        "HC_SESSION": "install-test",
        "HC_SOCKET": str(base / "hc.sock"),
        "XDG_STATE_HOME": str(base / "hc-state"),
        "XDG_CONFIG_HOME": str(base / "hc-config"),
    }
    clean_env = {k: v for k, v in os.environ.items() if not k.startswith(("CANBAN_", "HC_"))}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            origin = f"http://127.0.0.1:{self.server.server_address[1]}"
            if self.path.startswith("/repos/"):
                payload = []
                for ver, rel in releases_store.items():
                    payload.append({
                        "tag_name": ver,
                        "draft": False,
                        "prerelease": False,
                        "assets": [
                            {
                                "name": f"manifest-{rel['info']['platform']}.json",
                                "browser_download_url": f"{origin}/assets/{ver}/manifest.json",
                            },
                            {
                                "name": rel["gz_name"],
                                "browser_download_url": f"{origin}/assets/{ver}/{rel['gz_name']}",
                            },
                        ],
                    })
                body = json.dumps(payload).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            parts = self.path.strip("/").split("/")
            if len(parts) == 3 and parts[0] == "assets" and parts[1] in releases_store:
                rel = releases_store[parts[1]]
                if parts[2] == "manifest.json":
                    manifest = dict(rel["manifest"])
                    if rel["corrupt_sha"]:
                        manifest["tar_gz_sha256"] = "0" * 64
                    body = json.dumps(manifest).encode()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                if parts[2] == rel["gz_name"]:
                    data = rel["gz_path"].read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
            self.send_error(404)

        def log_message(self, *_):
            pass

    server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    api_url = f"http://127.0.0.1:{server.server_address[1]}"

    rel1 = make_fixture_release(source_bundle, base / "rel1", "20261114-1", "a")
    rel2 = make_fixture_release(source_bundle, base / "rel2", "20261114-2", "b")
    rel3 = make_fixture_release(source_bundle, base / "rel3", "20261114-3", "c")
    releases_store["20261114-1"] = rel1

    dest = (base / "Applications/Canban.app") if is_macos else (base / "local/opt/canban")
    home = base / "home"
    inst_env = {
        **clean_env,
        **core_env,
        "CANBAN_INSTALL_API": api_url,
        "CANBAN_INSTALL_REPO": "fixture/canban",
        "CANBAN_UPDATES_DISABLED": "1",
    }
    bash_bin = shutil.which("bash") or "/bin/bash"

    def run_install(*extra_args, extra_env=None, check=True):
        env = {**inst_env, **(extra_env or {})}
        res = subprocess.run(
            [bash_bin, str(INSTALL_SH), "--dest", str(dest), "--home", str(home), *extra_args],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        if check and res.returncode != 0:
            raise AssertionError(f"install.sh failed ({res.returncode}):\nSTDOUT:\n{res.stdout}\nSTDERR:\n{res.stderr}")
        return res

    old_server_proc = None
    try:
        # 1. Fresh install (20261114-1)
        res = run_install()
        assert "Installed Canban 20261114-1" in res.stdout, res.stdout
        installed_cli = dest / cli_rel
        assert installed_cli.is_file()
        installed_info = json.loads(subprocess.check_output([str(installed_cli), "build-info"], text=True))
        assert installed_info["version"] == "20261114-1", installed_info
        if is_macos:
            run_checked(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(dest)])
            q_check = subprocess.run(["/usr/bin/xattr", "-p", "com.apple.quarantine", str(dest)], capture_output=True, text=True)
            assert q_check.returncode != 0, "Expected no com.apple.quarantine on installed bundle"
        else:
            assert (base / "local/bin/canban").is_symlink()
            assert (base / "local/bin/canban-ui").is_symlink()
            assert (base / "local/share/applications/com.canban.desktop.desktop").is_file()
        print("1. Fresh install OK", flush=True)

        # 2. Idempotency & --force
        marker = dest / ("Contents/Resources/idempotent-marker" if is_macos else "share/idempotent-marker")
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text("keep")
        res = run_install()
        assert "already up to date" in res.stdout, res.stdout
        assert marker.exists(), "Up-to-date run unexpectedly replaced bundle"

        res = run_install("--force")
        assert "Installed Canban 20261114-1" in res.stdout, res.stdout
        assert not marker.exists(), "--force should replace bundle"
        print("2. Idempotency & --force OK", flush=True)

        # 3a. Live upgrade (20261114-1 -> 20261114-2) with running AppServer + live Herdr PTY
        releases_store["20261114-2"] = rel2
        log_file = (base / "server.log").open("w")
        old_server_proc = subprocess.Popen(
            [str(installed_cli), "--home", str(home), "serve"],
            env=inst_env,
            stdout=log_file,
            stderr=log_file,
        )
        old_ping = wait_until(lambda: rpc(home / "app.sock", "app.ping"), "initial AppServer")
        assert old_ping["pid"] == old_server_proc.pid

        term = rpc(home / "app.sock", "terminal.create", {"cwd": str(base)})
        term_id = term["runtime"]["terminal"]
        term_pid_before = json.loads(
            subprocess.check_output([hc, "get", term_id, "--json"], env={**clean_env, **core_env}, text=True)
        )["pid"]

        (home / "updates").mkdir(parents=True, exist_ok=True)
        (home / "updates/state.json").write_text(json.dumps({
            "phase": "available",
            "available": {"manifest": rel2["manifest"], "url": f"{api_url}/assets/20261114-2/{rel2['gz_name']}", "notes": ""},
            "progress": 0,
            "error": None,
            "checked_at": 1700000000,
            "last_check_started_at": 1700000000,
        }))

        # Corrupt SHA-256 refusal while live
        rel2["corrupt_sha"] = True
        bad_sha = run_install(check=False)
        assert bad_sha.returncode != 0 and "SHA-256 mismatch" in bad_sha.stderr, bad_sha.stderr
        assert rpc(home / "app.sock", "app.ping")["pid"] == old_server_proc.pid
        rel2["corrupt_sha"] = False

        # Agent self-replacement guard
        agent_guard = run_install(
            extra_env={"CANBAN_CARD": "61", "CANBAN_HOME": str(home)},
            check=False,
        )
        assert agent_guard.returncode != 0 and "Card Agent session" in agent_guard.stderr, agent_guard.stderr

        # Perform live upgrade to 20261114-2
        res = run_install()
        assert "Installed Canban 20261114-2" in res.stdout, res.stdout
        old_server_proc.wait(timeout=15)
        old_server_proc = None

        new_ping = wait_until(lambda: rpc(home / "app.sock", "app.ping"), "upgraded AppServer")
        assert new_ping["pid"] != old_ping["pid"]
        upgraded_info = json.loads(subprocess.check_output([str(installed_cli), "build-info"], text=True))
        assert upgraded_info["version"] == "20261114-2", upgraded_info

        term_pid_after = json.loads(
            subprocess.check_output([hc, "get", term_id, "--json"], env={**clean_env, **core_env}, text=True)
        )["pid"]
        assert term_pid_after == term_pid_before, f"PTY PID changed: {term_pid_before} -> {term_pid_after}"

        state_after = json.loads((home / "updates/state.json").read_text())
        assert state_after["phase"] == "current" and state_after["available"] is None, state_after
        assert state_after["checked_at"] == 1700000000, state_after
        print("3a. Live upgrade with AppServer + Herdr PTY preservation OK", flush=True)

        # Downgrade refusal (trying 20261114-1 while 20261114-2 is installed)
        downgrade = run_install("--version", "20261114-1", check=False)
        assert downgrade.returncode != 0 and "newer than target" in downgrade.stderr, downgrade.stderr

        # Shut down upgraded AppServer cleanly before schema migration test
        rpc(home / "app.sock", "app.shutdown", {"expected_pid": new_ping["pid"]})
        wait_until(lambda: not (home / "app.sock").exists(), "AppServer socket cleanup")
        wait_until(lambda: not pid_alive(new_ping["pid"]), "upgraded AppServer exit")

        # 3b. Database schema migration (10 -> current) & data preservation
        current_schema = rel3["info"]["schema"]
        if current_schema > 10:
            db = sqlite3.connect(home / "canban.sqlite")
            db.execute("CREATE TABLE keep(value TEXT)")
            db.execute("INSERT INTO keep VALUES('preserved')")
            db.commit()
            db.close()
            run_checked(
                [str(installed_cli), "--home", str(home), "database", "--target", "10"],
                env=inst_env,
            )
            db = sqlite3.connect(home / "canban.sqlite")
            assert db.execute("PRAGMA user_version").fetchone()[0] == 10
            db.close()

            releases_store["20261114-3"] = rel3
            res = run_install("--no-restart")
            assert "Installed Canban 20261114-3" in res.stdout, res.stdout
            assert f"Migrating Canban database schema (10 -> {current_schema})" in res.stdout, res.stdout
            assert (home / "updates/backup-before-20261114-3.sqlite").is_file()

            db = sqlite3.connect(home / "canban.sqlite")
            assert db.execute("PRAGMA user_version").fetchone()[0] == current_schema
            assert db.execute("SELECT value FROM keep").fetchall() == [("preserved",)]
            db.close()
            print(f"3b. Database migration (10 -> {current_schema}) + backup OK", flush=True)

        # 4. Active in-app update journal refusal
        (home / "updates/install.json").write_text(json.dumps({"token": "t", "phase": "migrating"}))
        pending_guard = run_install("--force", check=False)
        assert pending_guard.returncode != 0 and "in-app Canban update is pending" in pending_guard.stderr, pending_guard.stderr
        (home / "updates/install.json").unlink()
        print("4. Safety guards OK", flush=True)

        print("ALL_INSTALL_TESTS_PASSED", flush=True)
    finally:
        if old_server_proc and old_server_proc.poll() is None:
            old_server_proc.terminate()
            old_server_proc.wait(timeout=10)
        try:
            if (home / "app.sock").exists():
                rpc(home / "app.sock", "app.shutdown")
                time.sleep(0.5)
        except Exception:
            pass
        subprocess.run([hc, "server", "stop"], env={**clean_env, **core_env}, capture_output=True)
        server.shutdown()
        shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    main()
