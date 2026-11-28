#!/usr/bin/env python3
"""Atomically install the packaged Canban desktop bundle (/Applications/Canban.app on macOS, ~/.local/opt/canban on Linux)."""
from pathlib import Path
import argparse
import fcntl
import json
import os
import plistlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time


DAEMON_STRIP_ENV_KEYS = frozenset({
    "HC_SCOPE",
    "HC_TERMINAL_ID",
    "HC_TERMINAL_SOCKET",
    "CANBAN_ENDPOINT",
    "CANBAN_CARD",
    "CARD_ID",
    "CANBAN_PROJECT",
    "CANBAN_PROFILE_ID",
    "CANBAN_SESSION_ID",
    "CANBAN_RUN_DIR",
})


def format_desktop_exec(executable: Path) -> str:
    raw = str(executable)
    if any(ch in raw for ch in (' ', '\t', '"', "'", '\\', '`', '$', '>', '<', '~', '|', '&', ';', '*', '?', '#', '(', ')')):
        escaped = (
            raw.replace("\\", "\\\\")
            .replace('"', '\\"')
            .replace("`", "\\`")
            .replace("$", "\\$")
        )
        return f'"{escaped}"'
    return raw


def atomic_symlink(target: Path, link_path: Path):
    link_path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=f".{link_path.name}-link-", dir=link_path.parent)
    os.close(fd)
    tmp_path = Path(tmp_name)
    tmp_path.unlink(missing_ok=True)
    os.symlink(target, tmp_path)
    os.replace(tmp_path, link_path)


def atomic_write_bytes(path: Path, data: bytes, mode: int = 0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}-", delete=False) as stream:
        stream.write(data)
        staged = Path(stream.name)
    os.chmod(staged, mode)
    os.replace(staged, path)


def rpc_unix(sock_path: Path, method: str, params: dict, timeout: float = 3.0) -> dict:
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}) + "\n"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.settimeout(timeout)
        conn.connect(str(sock_path))
        conn.sendall(payload.encode("utf-8"))
        chunks = []
        while True:
            chunk = conn.recv(65536)
            if not chunk:
                break
            chunks.append(chunk)
            if b"\n" in chunk:
                break
    raw = b"".join(chunks).split(b"\n", 1)[0].decode("utf-8")
    resp = json.loads(raw)
    if "error" in resp:
        raise RuntimeError(f"RPC {method}: {resp['error']}")
    return resp.get("result") or {}


def service_lock_free(home: Path) -> bool:
    lock_path = home / "server.lock"
    if not lock_path.exists():
        return True
    try:
        with open(lock_path, "r+b") as lock_file:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)
            return True
    except OSError:
        return False


def service_file_matches_home(unit_path: Path, home: Path) -> bool:
    if not unit_path.is_file():
        return False
    try:
        body = unit_path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return False
    resolved = home.resolve()
    return str(home) in body or str(resolved) in body


def clear_stale_update_state(home: Path) -> None:
    home = home.expanduser()
    updates_dir = home / "updates"
    if (updates_dir / "install.json").exists():
        return
    state_path = updates_dir / "state.json"
    if not state_path.is_file():
        return
    try:
        state = json.loads(state_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
    if not isinstance(state, dict) or state.get("phase") == "manual_recovery":
        return
    if state.get("available") is None and state.get("phase") not in (
        "available",
        "error",
        "checking",
        "downloading",
        "installing",
    ):
        return
    state["phase"] = "current"
    state["available"] = None
    state["progress"] = 0
    state["error"] = None
    atomic_write_bytes(
        state_path,
        (json.dumps(state, indent=2) + "\n").encode("utf-8"),
        mode=0o600,
    )


def restart_running_daemon(installed_cli: Path, home: Path, platform_kind: str) -> None:
    home = home.expanduser()
    if platform_kind == "darwin":
        plist_path = Path.home() / "Library/LaunchAgents/com.canban.daemon.plist"
        if service_file_matches_home(plist_path, home):
            listed = subprocess.run(
                ["launchctl", "list", "com.canban.daemon"],
                check=False,
                capture_output=True,
            )
            if listed.returncode == 0:
                uid = str(os.getuid())
                res = subprocess.run(
                    ["launchctl", "kickstart", "-k", f"gui/{uid}/com.canban.daemon"],
                    check=False,
                    capture_output=True,
                    text=True,
                )
                if res.returncode == 0:
                    print(
                        f"Restarted launchd service com.canban.daemon for {home}",
                        file=sys.stderr,
                    )
                    return
    elif platform_kind == "linux":
        xdg_config = Path(os.environ.get("XDG_CONFIG_HOME") or (Path.home() / ".config"))
        unit_path = xdg_config / "systemd/user/canban.service"
        if service_file_matches_home(unit_path, home):
            active = subprocess.run(
                ["systemctl", "--user", "is-active", "--quiet", "canban.service"],
                check=False,
                capture_output=True,
            )
            if active.returncode == 0:
                res = subprocess.run(
                    ["systemctl", "--user", "restart", "canban.service"],
                    check=False,
                    capture_output=True,
                    text=True,
                )
                if res.returncode == 0:
                    print(
                        f"Restarted systemd user service canban.service for {home}",
                        file=sys.stderr,
                    )
                    return

    sock_path = home / "app.sock"
    if not sock_path.exists():
        return
    try:
        ping = rpc_unix(sock_path, "app.ping", {}, timeout=3.0)
    except (OSError, ValueError, RuntimeError):
        return

    old_pid = ping.get("pid") if isinstance(ping.get("pid"), int) else None
    shutdown_params = {"expected_pid": old_pid} if old_pid is not None else {}
    try:
        rpc_unix(sock_path, "app.shutdown", shutdown_params, timeout=5.0)
    except (OSError, ValueError, RuntimeError) as exc:
        print(
            f"Warning: could not gracefully shut down Canban AppServer at {home}: {exc}",
            file=sys.stderr,
        )
        return

    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        sock_down = False
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                probe.settimeout(0.2)
                probe.connect(str(sock_path))
        except OSError:
            sock_down = True
        if sock_down and service_lock_free(home):
            break
        time.sleep(0.05)
    else:
        print(
            f"Warning: timed out waiting for old Canban AppServer (pid {old_pid}) at {home} to stop",
            file=sys.stderr,
        )
        return

    log_path = home / "server.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    daemon_env = {k: v for k, v in os.environ.items() if k not in DAEMON_STRIP_ENV_KEYS}
    with open(log_path, "ab") as log_file:
        subprocess.Popen(
            [str(installed_cli), "--home", str(home), "serve"],
            env=daemon_env,
            stdin=subprocess.DEVNULL,
            stdout=log_file,
            stderr=log_file,
            start_new_session=True,
        )

    ready_deadline = time.monotonic() + 10.0
    new_pid = None
    while time.monotonic() < ready_deadline:
        try:
            ready = rpc_unix(sock_path, "app.ping", {}, timeout=1.0)
            cand_pid = ready.get("pid") if isinstance(ready.get("pid"), int) else None
            if old_pid is None or cand_pid != old_pid:
                new_pid = cand_pid
                break
        except (OSError, ValueError, RuntimeError):
            pass
        time.sleep(0.05)

    if new_pid is not None:
        print(
            f"Restarted Canban AppServer at {home} (pid {old_pid} -> {new_pid})",
            file=sys.stderr,
        )
    else:
        print(
            f"Started replacement Canban AppServer at {home} (still initializing)",
            file=sys.stderr,
        )


platform_override = os.environ.get("CANBAN_PACKAGE_PLATFORM", "").strip()
if platform_override:
    platform_kind = platform_override
elif sys.platform == "darwin":
    platform_kind = "darwin"
elif sys.platform.startswith("linux"):
    platform_kind = "linux"
else:
    sys.exit(f"install-app supports macOS and Linux only (got {sys.platform})")

root = Path(__file__).resolve().parents[1]
default_source = root / (".local/Canban.app" if platform_kind == "darwin" else ".local/canban")
env_dest = os.environ.get("CANBAN_INSTALL_DEST") or os.environ.get("CANBAN_INSTALL_DIR")
default_dest = (
    Path(env_dest)
    if env_dest
    else (Path("/Applications/Canban.app") if platform_kind == "darwin" else (Path.home() / ".local/opt/canban"))
)

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--source",
    type=Path,
    default=default_source,
    help="Packaged unbound Canban bundle to install",
)
parser.add_argument(
    "--dest",
    type=Path,
    default=None,
    help="Destination bundle path (defaults to /Applications/Canban.app on macOS, ~/.local/opt/canban on Linux)",
)
parser.add_argument(
    "--bin-dir",
    type=Path,
    default=None,
    help="Directory for CLI/UI symlinks on Linux (defaults to ~/.local/bin or <prefix>/bin when --dest is customized)",
)
parser.add_argument(
    "--xdg-data-home",
    type=Path,
    default=None,
    help="XDG data directory for .desktop and icons on Linux (defaults to ~/.local/share or <prefix>/share when --dest is customized)",
)
parser.add_argument(
    "--home",
    type=Path,
    default=None,
    help="Canban home directory whose running AppServer should be restarted (defaults to ~/.canban when installing to the default destination)",
)
parser.add_argument(
    "--no-restart",
    action="store_true",
    help="Do not restart an already-running Canban AppServer after installing",
)
args = parser.parse_args()

custom_dest = args.dest is not None or bool(env_dest)
dest_arg = args.dest if args.dest is not None else default_dest
restart_home = (
    None
    if args.no_restart
    else (args.home if args.home is not None else (None if custom_dest else Path.home() / ".canban"))
)

if platform_kind == "darwin":
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

    dest = dest_arg.expanduser()
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
            os.utime(dest, None)
            lsregister = Path(
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
            )
            if lsregister.is_file():
                subprocess.run(
                    [str(lsregister), "-f", str(dest)],
                    check=False,
                    capture_output=True,
                )
        except Exception:
            if backup is not None and backup.exists() and not dest.exists():
                os.rename(backup, dest)
            raise
    finally:
        shutil.rmtree(staging_root, ignore_errors=True)

    if restart_home is not None:
        clear_stale_update_state(restart_home)
        restart_running_daemon(dest / "Contents/MacOS/canban", restart_home, platform_kind)

    print(dest)
elif platform_kind == "linux":
    source = args.source.resolve()
    if not source.is_dir():
        parser.error(f"Packaged bundle not found at {source}; run `mise run build-release` first")
    if (source / "share/dev-instance.json").exists():
        parser.error("Refusing to install a branch-bound development bundle")

    bundle_json_path = source / "bundle.json"
    if not bundle_json_path.is_file():
        parser.error(f"Missing bundle.json in {source}")
    try:
        meta = json.loads(bundle_json_path.read_text())
    except ValueError as exc:
        parser.error(f"Invalid bundle.json in {source}: {exc}")
    if meta.get("app_id") != "com.canban.desktop":
        parser.error(f"Expected app_id com.canban.desktop, got {meta.get('app_id')!r}")

    for required in ("canban-ui", "canban", "canban-updater"):
        if not (source / "bin" / required).is_file():
            parser.error(f"Missing required executable bin/{required} in {source}")

    dest = dest_arg.expanduser()
    dest_parent = dest.parent.resolve()
    dest_parent.mkdir(parents=True, exist_ok=True)
    dest = dest_parent / dest.name

    prefix = dest_parent.parent if dest_parent.name == "opt" else dest_parent
    env_bin_dir = os.environ.get("CANBAN_BIN_DIR")
    if args.bin_dir is not None:
        bin_dir = args.bin_dir.expanduser().resolve()
    elif env_bin_dir:
        bin_dir = Path(env_bin_dir).expanduser().resolve()
    elif custom_dest:
        bin_dir = prefix / "bin"
    else:
        bin_dir = (Path.home() / ".local/bin").resolve()

    env_xdg_data = os.environ.get("CANBAN_XDG_DATA_HOME")
    if args.xdg_data_home is not None:
        xdg_data_home = args.xdg_data_home.expanduser().resolve()
    elif env_xdg_data:
        xdg_data_home = Path(env_xdg_data).expanduser().resolve()
    elif custom_dest:
        xdg_data_home = prefix / "share"
    else:
        xdg_default = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local/share")
        xdg_data_home = Path(xdg_default).expanduser().resolve()

    staging_root = Path(tempfile.mkdtemp(prefix=".canban-install-", dir=dest_parent))
    staged = staging_root / "canban"
    backup = None
    try:
        shutil.copytree(source, staged, symlinks=False)
        for required in ("canban-ui", "canban", "canban-updater"):
            os.chmod(staged / "bin" / required, 0o755)
        if dest.exists() or dest.is_symlink():
            backup = staging_root / "canban-previous"
            os.rename(dest, backup)
        try:
            os.rename(staged, dest)
            os.utime(dest, None)
        except Exception:
            if backup is not None and backup.exists() and not dest.exists():
                os.rename(backup, dest)
            raise
    finally:
        shutil.rmtree(staging_root, ignore_errors=True)

    atomic_symlink(dest / "bin/canban", bin_dir / "canban")
    atomic_symlink(dest / "bin/canban-ui", bin_dir / "canban-ui")

    exec_cmd = format_desktop_exec(dest / "bin/canban-ui")
    desktop_entry = "\n".join([
        "[Desktop Entry]",
        "Type=Application",
        "Name=Canban",
        "Comment=AI-native Kanban workbench with Git worktree workflows and Herdr PTY terminals",
        f"Exec={exec_cmd}",
        "Icon=com.canban.desktop",
        "Terminal=false",
        "Categories=Development;",
        "StartupWMClass=com.canban.desktop",
        "",
    ])
    atomic_write_bytes(
        xdg_data_home / "applications/com.canban.desktop.desktop",
        desktop_entry.encode("utf-8"),
        mode=0o644,
    )

    for rel_icon in (
        Path("icons/hicolor/scalable/apps/com.canban.desktop.svg"),
        Path("icons/hicolor/256x256/apps/com.canban.desktop.png"),
        Path("icons/hicolor/512x512/apps/com.canban.desktop.png"),
    ):
        src_icon = dest / "share" / rel_icon
        if src_icon.is_file():
            atomic_write_bytes(xdg_data_home / rel_icon, src_icon.read_bytes(), mode=0o644)

    default_user_share = Path(os.environ.get("XDG_DATA_HOME") or (Path.home() / ".local/share")).expanduser().resolve()
    if xdg_data_home == default_user_share:
        update_db = shutil.which("update-desktop-database")
        if update_db:
            subprocess.run([update_db, str(xdg_data_home / "applications")], check=False, capture_output=True)
        update_icons = shutil.which("gtk-update-icon-cache")
        if update_icons:
            subprocess.run([update_icons, "-f", "-t", str(xdg_data_home / "icons/hicolor")], check=False, capture_output=True)

    if restart_home is not None:
        clear_stale_update_state(restart_home)
        restart_running_daemon(dest / "bin/canban", restart_home, platform_kind)

    print(dest)
else:
    sys.exit(f"install-app supports macOS and Linux only (got {platform_kind})")
