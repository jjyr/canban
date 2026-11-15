#!/usr/bin/env python3
"""Pre-release real upgrade gate against the latest published GitHub Release.

Downloads the newest non-draft release from GitHub (zero binary fixtures in Git),
boots it in an isolated home/Herdr Core, seeds representative workspace data,
runs a real `canban-updater` upgrade to the candidate build, and verifies
AppServer state, UI readiness RPCs, PTY survival, and snapshot cleanup.
"""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
import pathlib
import plistlib
import shutil
import signal
import socket
import socketserver
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.error
import urllib.request

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release_mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_mod)


class _NoAuthRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Strip GitHub Authorization header when redirected to signed blob storage."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        orig_host = urllib.request.urlparse(req.full_url).hostname or ""
        new_host = urllib.request.urlparse(newurl).hostname or ""
        if orig_host.lower() != new_host.lower():
            redirected.headers.pop("Authorization", None)
            redirected.unredirected_hdrs.pop("Authorization", None)
        return redirected


def select_latest_published_release(releases: list, platform: str, before_version: str = None):
    """Pick the highest non-draft YYYYMMDD-N release (strictly < before_version when versioned)."""
    ceiling = release_mod.parse_version(before_version) if before_version else None
    gz_name = f"Canban-{platform}.tar.gz"
    zst_name = f"Canban-{platform}.tar.zst"
    manifest_name = f"manifest-{platform}.json"

    best = None
    for entry in releases or []:
        if not entry or entry.get("draft"):
            continue
        tag = entry.get("tag_name", "")
        parsed = release_mod.parse_version(tag)
        if parsed is None:
            continue
        if ceiling is not None and parsed >= ceiling:
            continue
        assets = {a.get("name"): a for a in (entry.get("assets") or []) if a and a.get("name")}
        archive = assets.get(gz_name) or assets.get(zst_name)
        manifest = assets.get(manifest_name)
        if archive is None or manifest is None:
            continue
        if best is None or parsed > best[0]:
            best = (parsed, entry, archive, manifest)
    if best is None:
        return None
    return {
        "version": best[1]["tag_name"],
        "key": best[0],
        "release": best[1],
        "archive": best[2],
        "manifest": best[3],
    }


def download_github_asset(asset: dict, destination: pathlib.Path, token: str):
    """Download a private/public GitHub release asset via its API octet-stream endpoint."""
    url = asset.get("url") or asset.get("browser_download_url")
    if not url:
        raise RuntimeError(f"Asset URL missing for {asset.get('name')}")
    headers = {
        "Accept": "application/octet-stream",
        "User-Agent": "Canban release-upgrade-gate",
    }
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    opener = urllib.request.build_opener(_NoAuthRedirectHandler())
    request = urllib.request.Request(url, headers=headers)
    with opener.open(request, timeout=180) as response, destination.open("wb") as out:
        while True:
            chunk = response.read(65536)
            if not chunk:
                break
            out.write(chunk)


def extract_bundle(archive_path: pathlib.Path, extract_dir: pathlib.Path, updater_bin: pathlib.Path = None) -> pathlib.Path:
    extract_dir.mkdir(parents=True, exist_ok=True)
    if archive_path.name.endswith(".tar.gz"):
        with tarfile.open(archive_path, "r:gz") as tar:
            for member in tar.getmembers():
                parts = pathlib.PurePosixPath(member.name).parts
                if not parts or parts[0] != "Canban.app" or ".." in parts or member.issym() or member.islnk():
                    raise RuntimeError(f"Unsafe archive entry: {member.name}")
            tar.extractall(extract_dir)
    elif archive_path.name.endswith(".tar.zst"):
        zstd = shutil.which("zstd") or next(
            (p for p in ["/opt/homebrew/bin/zstd", "/usr/local/bin/zstd"] if pathlib.Path(p).is_file()),
            None,
        )
        if not zstd:
            raise RuntimeError(f"zstd is required to extract {archive_path.name}")
        proc = subprocess.Popen([zstd, "-dc", str(archive_path)], stdout=subprocess.PIPE)
        assert proc.stdout is not None
        with tarfile.open(fileobj=proc.stdout, mode="r|*") as tar:
            for member in tar:
                parts = pathlib.PurePosixPath(member.name).parts
                if not parts or parts[0] != "Canban.app" or ".." in parts or member.issym() or member.islnk():
                    raise RuntimeError(f"Unsafe archive entry: {member.name}")
                tar.extract(member, extract_dir)
        if proc.wait() != 0:
            raise RuntimeError(f"zstd decompression failed for {archive_path}")
    else:
        raise RuntimeError(f"Unsupported archive format: {archive_path.name}")
    bundle = extract_dir / "Canban.app"
    if not (bundle / "Contents/MacOS/canban").is_file():
        raise RuntimeError(f"Extracted bundle at {bundle} is incomplete")
    for name in ("canban", "canban-ui", "canban-updater"):
        p = bundle / "Contents/MacOS" / name
        if p.is_file():
            p.chmod(0o755)
    subprocess.run(["/usr/bin/xattr", "-dr", "com.apple.quarantine", str(bundle)], capture_output=True)
    return bundle


def write_ui_probe(path: pathlib.Path):
    """Headless UI shim that exercises real Bootstrap/workspace RPCs before writing ui-ready."""
    path.write_text(
        """#!/usr/bin/python3
import json, os, socket, subprocess, sys, time
from pathlib import Path

home = Path(os.environ["CANBAN_HOME"])
binary = Path(__file__).with_name("canban")
info = subprocess.check_output([str(binary), "build-info"])

def rpc(method, params=None):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.settimeout(10)
        s.connect(str(home / "app.sock"))
        s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}) + "\\n").encode())
        resp = json.loads(s.makefile("rb").readline())
        if "error" in resp:
            raise RuntimeError(f"{method} failed: {resp['error']}")
        return resp["result"]

token = os.environ.get("CANBAN_UPGRADE_TOKEN")
if token:
    # Exercise the exact startup RPC surface used by the desktop UI before signalling readiness.
    deadline = time.monotonic() + 20.0
    while True:
        try:
            rpc("app.ping")
            rpc("app.state")
            rpc("app.snapshot")
            rpc("update.state")
            break
        except Exception:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.1)
    for optional in ("draft.list", "utilities.state"):
        try:
            rpc(optional)
        except Exception:
            pass
    (home / "updates" / f"ui-ready-{token}").write_bytes(info)

while True:
    try:
        j = json.loads((home / "updates/install.json").read_text())
        if j.get("phase") in ("prepared", "stopping"):
            (home / "updates" / f"ui-saved-{j['token']}").write_text("ready")
    except Exception:
        pass
    time.sleep(0.1)
"""
    )
    path.chmod(0o755)


def prepare_candidate_fixture(
    candidate_bundle: pathlib.Path,
    dist_dir: pathlib.Path,
    work_dir: pathlib.Path,
    app_id: str,
    baseline_version: str,
) -> tuple[pathlib.Path, dict]:
    """Prepare a candidate .tar.zst + manifest with headless UI probe for the HTTP upgrade server."""
    staged = work_dir / "candidate-src/Canban.app"
    if dist_dir is not None:
        raw_info = json.loads(
            subprocess.check_output([str(candidate_bundle / "Contents/MacOS/canban"), "build-info"], text=True)
        )
        platform = raw_info["platform"]
        dist_manifest = json.loads((dist_dir / f"manifest-{platform}.json").read_text())
        dist_archive = dist_dir / dist_manifest["asset"]
        if dist_archive.stat().st_size != dist_manifest["size"]:
            raise RuntimeError("Packaged dist archive size mismatch")
        actual_sha = hashlib.sha256(dist_archive.read_bytes()).hexdigest()
        if actual_sha != dist_manifest["sha256"]:
            raise RuntimeError("Packaged dist archive SHA-256 mismatch")
        extracted = extract_bundle(dist_archive, work_dir / "dist-extracted")
        shutil.copytree(extracted, staged)
    else:
        shutil.copytree(candidate_bundle, staged)

    plist_path = staged / "Contents/Info.plist"
    plist = plistlib.loads(plist_path.read_bytes())
    plist.update(CFBundleIdentifier=app_id, CFBundleName="Canban Upgrade Gate")
    plist_path.write_bytes(plistlib.dumps(plist))

    write_ui_probe(staged / "Contents/MacOS/canban-ui")

    cli = staged / "Contents/MacOS/canban"
    info = json.loads(subprocess.check_output([str(cli), "build-info"], text=True))
    if release_mod.parse_version(info.get("version", "")) is None:
        base_parsed = release_mod.parse_version(baseline_version) or (20261115, 1)
        synth_version = f"{max(base_parsed[0], 20991231)}-{base_parsed[1] + 1}"
        head = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
        ).stdout.strip()
        synth_commit = head if len(head) == 40 else ("f" * 40)
        print(
            f"Building versioned candidate CLI/updater ({synth_version} @ {synth_commit[:7]})...",
            flush=True,
        )
        build_env = {
            **os.environ,
            "CARGO_TARGET_DIR": str(work_dir / "cargo-target"),
            "CANBAN_RELEASE_VERSION": synth_version,
            "CANBAN_RELEASE_COMMIT": synth_commit,
        }
        subprocess.run(
            ["cargo", "build", "--locked", "-p", "canban-cli", "-p", "canban-updater"],
            cwd=ROOT,
            env=build_env,
            check=True,
        )
        shutil.copy2(work_dir / "cargo-target/debug/canban", staged / "Contents/MacOS/canban")
        shutil.copy2(work_dir / "cargo-target/debug/canban-updater", staged / "Contents/MacOS/canban-updater")
        info = json.loads(subprocess.check_output([str(cli), "build-info"], text=True))

    artifact = work_dir / f"Canban-{info['platform']}.tar.zst"
    updater = staged / "Contents/MacOS/canban-updater"
    subprocess.run([str(updater), "pack", str(staged), str(artifact)], check=True)
    manifest = {
        **info,
        "rollback_floor": 8,
        "asset": artifact.name,
        "size": artifact.stat().st_size,
        "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
    }
    return artifact, manifest


def rpc(sock_path: pathlib.Path, method: str, params=None):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.settimeout(15)
        s.connect(str(sock_path))
        s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}) + "\n").encode())
        resp = json.loads(s.makefile("rb").readline())
        if "error" in resp:
            raise RuntimeError(f"{method}: {resp['error']}")
        return resp["result"]


def wait_until(fn, label: str, timeout: float = 60.0):
    deadline = time.monotonic() + timeout
    last_err = None
    while time.monotonic() < deadline:
        try:
            value = fn()
            if value:
                return value
        except Exception as exc:
            last_err = exc
        time.sleep(0.1)
    raise RuntimeError(f"Timed out waiting for {label}: {last_err}")


def find_hc() -> str:
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
        raise RuntimeError("Herdr CLI `hc` is required in PATH")
    return hc


def run_upgrade_gate(
    baseline_src: pathlib.Path,
    candidate_bundle: pathlib.Path,
    dist_dir: pathlib.Path = None,
    keep_temp: bool = False,
):
    hc = find_hc()
    base = pathlib.Path(tempfile.mkdtemp(prefix="cb-rel-upgrade-", dir="/private/tmp"))
    print(f"RELEASE_UPGRADE_BASE={base}", flush=True)

    app_id = "com.canban.release.upgrade.gate"
    target = base / "Applications/Canban.app"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(baseline_src, target)

    plist_path = target / "Contents/Info.plist"
    plist = plistlib.loads(plist_path.read_bytes())
    plist.update(CFBundleIdentifier=app_id, CFBundleName="Canban Upgrade Gate")
    plist_path.write_bytes(plistlib.dumps(plist))
    write_ui_probe(target / "Contents/MacOS/canban-ui")

    baseline_info = json.loads(
        subprocess.check_output([str(target / "Contents/MacOS/canban"), "build-info"], text=True)
    )
    artifact, manifest = prepare_candidate_fixture(
        candidate_bundle,
        dist_dir,
        base,
        app_id,
        baseline_info["version"],
    )

    if baseline_info["schema"] > manifest["schema"]:
        raise RuntimeError(
            f"Candidate schema ({manifest['schema']}) is older than published baseline {baseline_info['version']} ({baseline_info['schema']})"
        )
    if baseline_info["schema"] < manifest["rollback_floor"]:
        print(
            f"Baseline {baseline_info['version']} schema ({baseline_info['schema']}) is below candidate rollback_floor ({manifest['rollback_floor']}); skipping in-app upgrade check.",
            flush=True,
        )
        shutil.rmtree(base, ignore_errors=True)
        return

    class ReleaseFeedHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            origin = f"http://127.0.0.1:{self.server.server_address[1]}"
            if self.path.startswith("/repos/"):
                asset_entry = {
                    "name": artifact.name,
                    "size": artifact.stat().st_size,
                    "browser_download_url": f"{origin}/artifact",
                }
                meta_entry = {
                    "name": f"manifest-{manifest['platform']}.json",
                    "browser_download_url": f"{origin}/manifest",
                }
                body = json.dumps(
                    [
                        {
                            "tag_name": manifest["version"],
                            "draft": False,
                            "prerelease": False,
                            "body": "Pre-release upgrade gate candidate",
                            "assets": [asset_entry, meta_entry],
                        }
                    ]
                ).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if self.path == "/manifest":
                body = json.dumps(manifest).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if self.path == "/artifact":
                data = artifact.read_bytes()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            self.send_error(404)

        def log_message(self, *_):
            pass

    feed_server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), ReleaseFeedHandler)
    threading.Thread(target=feed_server.serve_forever, daemon=True).start()

    home = base / "home"
    core_env = {
        "HC_SESSION": "release-upgrade-gate",
        "HC_SOCKET": str(base / "hc.sock"),
        "XDG_STATE_HOME": str(base / "hc-state"),
        "XDG_CONFIG_HOME": str(base / "hc-config"),
    }
    clean_env = {k: v for k, v in os.environ.items() if not k.startswith(("CANBAN_", "HC_"))}
    env = {
        **clean_env,
        **core_env,
        "CANBAN_HOME": str(home),
        "CANBAN_APP_ID": app_id,
        "CANBAN_UPDATE_API": f"http://127.0.0.1:{feed_server.server_address[1]}",
        "CANBAN_UPDATE_REPOSITORY": "jjyr/canban",
    }

    cli = target / "Contents/MacOS/canban"
    log_file = (base / "server.log").open("w")
    server = subprocess.Popen([str(cli), "serve"], env=env, stdout=log_file, stderr=log_file)
    ui = None

    try:
        ping_before = wait_until(lambda: rpc(home / "app.sock", "app.ping"), "baseline AppServer")
        assert ping_before["build"]["version"] == baseline_info["version"], ping_before

        config_path = home / "config/agents.json"
        if config_path.is_file():
            cfg = json.loads(config_path.read_text())
            cfg.update(hc=hc, hc_env=core_env, hc_session=core_env["HC_SESSION"])
            config_path.write_text(json.dumps(cfg))
            try:
                rpc(home / "app.sock", "config.reload")
            except Exception:
                pass

        # Seed representative user data through the real baseline AppServer.
        repo_dir = base / "seed-repo"
        repo_dir.mkdir()
        for git_cmd in (
            ["init", "-b", "main"],
            ["config", "user.name", "Upgrade Gate"],
            ["config", "user.email", "upgrade-gate@example.invalid"],
            ["config", "core.hooksPath", "/dev/null"],
            ["config", "commit.gpgsign", "false"],
        ):
            subprocess.run(["git", "-C", str(repo_dir), *git_cmd], check=True, capture_output=True)
        (repo_dir / "README.md").write_text("# Seed Project\n")
        subprocess.run(["git", "-C", str(repo_dir), "add", "."], check=True, capture_output=True)
        subprocess.run(["git", "-C", str(repo_dir), "commit", "-m", "Initial seed"], check=True, capture_output=True)

        project = rpc(
            home / "app.sock",
            "project.create",
            {"name": "Release Upgrade Gate", "directory": str(repo_dir), "workspace": {"sync_base": False}},
        )
        card = rpc(
            home / "app.sock",
            "card.create",
            {
                "project_id": project["id"],
                "title": "Verify state across real release upgrade",
                "body": "Seeded by test-release-upgrade.py on baseline " + baseline_info["version"],
            },
        )
        try:
            rpc(
                home / "app.sock",
                "comment.add",
                {"card_id": card["id"], "body": "Pre-upgrade comment"},
            )
        except Exception:
            pass
        try:
            rpc(
                home / "app.sock",
                "draft.save",
                {
                    "id": f"comment:{card['id']}",
                    "expected_revision": 0,
                    "data": {"body": "Unsent draft across upgrade"},
                },
            )
        except Exception:
            pass

        terminal = rpc(home / "app.sock", "terminal.create", {"cwd": str(repo_dir)})
        term_name = terminal["runtime"]["terminal"]
        term_pid_before = json.loads(
            subprocess.check_output([hc, "get", term_name, "--json"], env={**clean_env, **core_env}, text=True)
        )["pid"]

        # Discover and install the candidate release via the real updater pipeline.
        rpc(home / "app.sock", "update.check")
        wait_until(
            lambda: (rpc(home / "app.sock", "update.state")["state"].get("available") or {})
            .get("manifest", {})
            .get("version")
            == manifest["version"],
            "candidate release discovery",
        )

        ui = subprocess.Popen([str(target / "Contents/MacOS/canban-ui")], env=env, stdout=log_file, stderr=log_file)
        wait_until(
            lambda: "canban-ui"
            in subprocess.run(["/bin/ps", "-p", str(ui.pid), "-o", "command="], capture_output=True, text=True).stdout,
            "baseline UI shim",
        )
        rpc(
            home / "app.sock",
            "update.install",
            {"version": manifest["version"], "ui_pid": ui.pid, "app_id": app_id},
        )

        def check_journal():
            p = home / "updates/install.json"
            if not p.is_file():
                return None
            j = json.loads(p.read_text())
            if j.get("phase") in ("rolled_back", "manual_recovery"):
                raise RuntimeError(f"Upgrade failed into {j.get('phase')}: {j.get('error')}")
            return j if j.get("phase") == "complete" else None

        completed = wait_until(check_journal, "upgrade completion", timeout=90.0)
        token = completed["token"]

        # Verify upgraded AppServer build identity, full state deserialization, data & PTY survival, and cleanup.
        ping_after = rpc(home / "app.sock", "app.ping")
        assert ping_after["build"]["version"] == manifest["version"], ping_after
        assert ping_after["build"]["schema"] == manifest["schema"], ping_after
        assert ping_after["build"]["api"] == manifest["api"], ping_after

        state_after = rpc(home / "app.sock", "app.state")
        snap_after = rpc(home / "app.sock", "app.snapshot")
        state_snap = state_after["snapshot"]
        assert any(
            p["id"] == project["id"] or p.get("name") == "Release Upgrade Gate"
            for p in state_snap["projects"]
        ), f"Seeded project missing after upgrade: {state_snap['projects']}"
        assert any(
            c["id"] == card["id"] or c.get("title") == "Verify state across real release upgrade"
            for c in state_snap["cards"]
        ), f"Seeded card missing after upgrade: {state_snap['cards']}"
        assert any(
            c["id"] == card["id"] or c.get("title") == "Verify state across real release upgrade"
            for c in snap_after["cards"]
        ), "Seeded card missing from snapshot"

        for method in ("draft.list", "utilities.state", "update.state", "herdr.check"):
            rpc(home / "app.sock", method)

        term_pid_after = json.loads(
            subprocess.check_output([hc, "get", term_name, "--json"], env={**clean_env, **core_env}, text=True)
        )["pid"]
        assert term_pid_after == term_pid_before, f"Herdr PTY died across upgrade ({term_pid_before} -> {term_pid_after})"

        assert not (home / "updates/jobs" / token / "database-before.sqlite").exists(), (
            "Temporary pre-upgrade SQLite snapshot was not cleaned up after completion"
        )

        print(
            f"RELEASE_UPGRADE_OK: {baseline_info['version']} (schema {baseline_info['schema']}, api {baseline_info['api']}) "
            f"-> {manifest['version']} (schema {manifest['schema']}, api {manifest['api']})",
            flush=True,
        )
    finally:
        try:
            if (home / "app.sock").exists():
                rpc(home / "app.sock", "app.shutdown")
        except Exception:
            pass
        if server.poll() is None:
            server.terminate()
            server.wait(timeout=15)
        if ui is not None and ui.poll() is None:
            ui.terminate()
            ui.wait(timeout=10)
        if (home / "updates/install.json").is_file():
            try:
                j = json.loads((home / "updates/install.json").read_text())
                for key in ("candidate_ui", "server", "recovery_ui"):
                    proc = j.get(key)
                    if proc:
                        ident = subprocess.run(
                            ["/bin/ps", "-p", str(proc["pid"]), "-o", "lstart=,command="],
                            capture_output=True,
                            text=True,
                        ).stdout.strip()
                        if ident == proc["identity"]:
                            try:
                                os.kill(proc["pid"], signal.SIGTERM)
                            except ProcessLookupError:
                                pass
            except Exception:
                pass
        ps_out = subprocess.check_output(["/bin/ps", "-axo", "pid=,command="], text=True)
        for line in ps_out.splitlines():
            if str(target / "Contents/MacOS/canban-ui") in line:
                pid = int(line.strip().split(None, 1)[0])
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        subprocess.run([hc, "server", "stop"], env={**clean_env, **core_env}, capture_output=True, timeout=20)
        feed_server.shutdown()
        feed_server.server_close()
        log_file.close()
        if not keep_temp:
            shutil.rmtree(base, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=None, help="GitHub owner/repo (default: GITHUB_REPOSITORY or jjyr/canban)")
    parser.add_argument(
        "--candidate",
        type=pathlib.Path,
        default=ROOT / ".local/Canban.app",
        help="Candidate Canban.app bundle path",
    )
    parser.add_argument(
        "--dist",
        type=pathlib.Path,
        default=None,
        help="Optional packaged dist/ directory from `release.py package`",
    )
    parser.add_argument(
        "--baseline",
        type=pathlib.Path,
        default=None,
        help="Optional local baseline Canban.app bundle (skips GitHub download)",
    )
    parser.add_argument("--keep-temp", action="store_true", help="Preserve temporary test directory")
    args = parser.parse_args()

    if not (args.candidate / "Contents/MacOS/canban").is_file():
        raise SystemExit(f"Candidate bundle missing at {args.candidate}; run `mise run build` first")

    cand_info = json.loads(
        subprocess.check_output([str(args.candidate / "Contents/MacOS/canban"), "build-info"], text=True)
    )
    platform = cand_info["platform"]

    if args.baseline is not None:
        run_upgrade_gate(args.baseline, args.candidate, dist_dir=args.dist, keep_temp=args.keep_temp)
        return

    repo = args.repo or os.environ.get("GITHUB_REPOSITORY", "jjyr/canban")
    token = release_mod.resolve_gh_token()
    releases = release_mod.api("GET", f"/repos/{repo}/releases?per_page=30&page=1", token=token) or []
    selected = select_latest_published_release(releases, platform, before_version=cand_info.get("version"))
    if selected is None:
        print(f"No prior published release with {platform} assets found in {repo}; skipping upgrade gate.", flush=True)
        return

    dl_dir = pathlib.Path(tempfile.mkdtemp(prefix="cb-rel-baseline-", dir="/private/tmp"))
    try:
        archive_asset = selected["archive"]
        archive_path = dl_dir / archive_asset["name"]
        print(
            f"Downloading latest published baseline {selected['version']} ({archive_asset['name']})...",
            flush=True,
        )
        download_github_asset(archive_asset, archive_path, token)
        baseline_bundle = extract_bundle(archive_path, dl_dir / "extracted")
        run_upgrade_gate(baseline_bundle, args.candidate, dist_dir=args.dist, keep_temp=args.keep_temp)
    finally:
        if not args.keep_temp:
            shutil.rmtree(dl_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
