#!/usr/bin/env python3
"""Isolated regressions for scripts/install.sh (fresh install, upgrades, Herdr PTY survival, and safety guards)."""
import hashlib
import http.server
import importlib.util
import json
import os
import pathlib
import plistlib
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
SOURCE_APP = ROOT / ".local/Canban.app"

spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def make_fixture_release(work_dir: pathlib.Path, version: str, commit_char: str) -> dict:
    bundle = work_dir / "src" / "Canban.app"
    shutil.copytree(SOURCE_APP, bundle)
    cli = bundle / "Contents/MacOS/canban"
    real_cli = bundle / "Contents/MacOS/canban-real"
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
    subprocess.run(
        ["/usr/bin/codesign", "--force", "--deep", "--sign", "-", "--timestamp=none", str(bundle)],
        check=True,
        capture_output=True,
    )

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
    if not (SOURCE_APP / "Contents/MacOS/canban").is_file():
        raise SystemExit(f"Packaged bundle not found at {SOURCE_APP}; run `mise run build` first")
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

    base = pathlib.Path(tempfile.mkdtemp(prefix="cb-install-test-", dir="/private/tmp"))
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

    rel1 = make_fixture_release(base / "rel1", "20261114-1", "a")
    rel2 = make_fixture_release(base / "rel2", "20261114-2", "b")
    rel3 = make_fixture_release(base / "rel3", "20261114-3", "c")
    releases_store["20261114-1"] = rel1

    dest = base / "Applications/Canban.app"
    home = base / "home"
    inst_env = {
        **clean_env,
        **core_env,
        "CANBAN_INSTALL_API": api_url,
        "CANBAN_INSTALL_REPO": "fixture/canban",
        "CANBAN_UPDATES_DISABLED": "1",
    }

    def run_install(*args, extra_env=None, check=True):
        env = {**inst_env, **(extra_env or {})}
        res = subprocess.run(
            ["/bin/bash", str(INSTALL_SH), "--dest", str(dest), "--home", str(home), *args],
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
        assert (dest / "Contents/MacOS/canban").is_file()
        installed_info = json.loads(subprocess.check_output([str(dest / "Contents/MacOS/canban"), "build-info"], text=True))
        assert installed_info["version"] == "20261114-1", installed_info
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(dest)], check=True, capture_output=True)
        q_check = subprocess.run(["/usr/bin/xattr", "-p", "com.apple.quarantine", str(dest)], capture_output=True, text=True)
        assert q_check.returncode != 0, "Expected no com.apple.quarantine on installed bundle"
        print("1. Fresh install OK", flush=True)

        # 2. Idempotency & --force
        marker = dest / "Contents/Resources/idempotent-marker"
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
            [str(dest / "Contents/MacOS/canban"), "--home", str(home), "serve"],
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
        upgraded_info = json.loads(subprocess.check_output([str(dest / "Contents/MacOS/canban"), "build-info"], text=True))
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

        # 3b. Database schema migration (10 -> current) & data preservation
        current_schema = rel3["info"]["schema"]
        if current_schema > 10:
            db = sqlite3.connect(home / "canban.sqlite")
            db.execute("CREATE TABLE keep(value TEXT)")
            db.execute("INSERT INTO keep VALUES('preserved')")
            db.commit()
            db.close()
            subprocess.run(
                [str(dest / "Contents/MacOS/canban"), "--home", str(home), "database", "--target", "10"],
                env=inst_env,
                check=True,
                capture_output=True,
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
