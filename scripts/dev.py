#!/usr/bin/env python3
"""Run a freshly built desktop/service pair against the selected Canban home.

Only the tracked development UI is replaced. AppServer is stopped through RPC;
Herdr and its Agent PTYs remain alive. Explicit endpoints are attach-only and
must be updated on their owning host instead of being replaced by this runner.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[1]


def rpc(endpoint, method, timeout=5):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(timeout)
        stream.connect(str(endpoint))
        stream.sendall((json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': {}}) + '\n').encode())
        with stream.makefile('rb') as reader:
            response = json.loads(reader.readline(8 * 1024 * 1024))
        if 'error' in response:
            raise RuntimeError(str(response['error']))
        return response['result']


def ping(endpoint):
    try:
        return rpc(endpoint, 'app.ping')
    except (FileNotFoundError, ConnectionRefusedError):
        return None


def identity(pid):
    """Check both start time and command before signalling a saved PID."""
    def ps(field):
        result = subprocess.run(['/bin/ps', '-p', str(pid), '-o', field + '='], capture_output=True, text=True)
        return result.stdout.strip() if result.returncode == 0 else None
    return {'start': ps('lstart'), 'command': ps('command')}


def stop_ui(state):
    if not state.exists():
        return
    saved = json.loads(state.read_text())
    pid = saved['pid']
    if identity(pid) != saved['identity'] or not saved['identity']['start']:
        return
    print('Closing the previous development UI…', flush=True)
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 10
    while identity(pid) == saved['identity']:
        if time.monotonic() > deadline:
            raise RuntimeError('Previous development UI did not exit; close it before retrying')
        time.sleep(.1)


def restart_server(home, cli):
    build = json.loads(subprocess.check_output([str(cli), 'build-info'], text=True))
    endpoint = home / 'app.sock'
    previous = ping(endpoint)
    if previous is not None:
        if Path(previous['home']).resolve() != home:
            raise RuntimeError('The running AppServer belongs to a different home')
        print('Restarting AppServer with the current build; Herdr terminals stay alive…', flush=True)
        rpc(endpoint, 'app.shutdown')
        deadline = time.monotonic() + 30
        # Wait for the service lock, not just a disappearing socket. Never
        # unlink a live socket or force-kill a process that may still be writing.
        with (home / 'server.lock').open('a') as lock:
            while True:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    fcntl.flock(lock, fcntl.LOCK_UN)
                    break
                except BlockingIOError:
                    if time.monotonic() > deadline:
                        raise RuntimeError('AppServer is still stopping; no replacement was started')
                    time.sleep(.1)
    home.mkdir(parents=True, exist_ok=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith('CANBAN_')}
    with (home / 'server.log').open('ab') as log:
        server = subprocess.Popen([str(cli), '--home', str(home), 'serve'], env=env,
                                  stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                  start_new_session=True)
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if server.poll() is not None:
            raise RuntimeError('New AppServer exited; inspect ' + str(home / 'server.log'))
        current = ping(endpoint)
        if current is not None and current.get('pid') == server.pid and current.get('api_version') == build['api']:
            # Desktop utilities were the original stale-service failure.
            rpc(endpoint, 'utilities.state')
            print(f"AppServer ready: PID {server.pid}, API {current['api_version']}, home {home}", flush=True)
            return
        time.sleep(.1)
    raise RuntimeError('Current AppServer did not become ready; inspect ' + str(home / 'server.log'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-ui', action='store_true', help='Restart/verify only the development service')
    args = parser.parse_args()
    if 'CANBAN_ENDPOINT' in os.environ:
        raise RuntimeError('CANBAN_ENDPOINT is attach-only. Update that host separately, or unset it to run a local development build.')
    home = Path(os.environ.get('CANBAN_HOME', str(Path.home() / '.canban'))).expanduser().resolve()
    dev = REPO / '.local/dev'
    dev.mkdir(parents=True, exist_ok=True)
    key = hashlib.sha256(str(home).encode()).hexdigest()[:16]
    state = dev / (key + '.json')
    ui = None
    with (dev / (key + '.lock')).open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        stop_ui(state)
        subprocess.run([sys.executable, str(REPO / 'scripts/package.py'), '--dev'], cwd=REPO, check=True)
        macos = REPO / '.local/CanbanDev.app/Contents/MacOS'
        restart_server(home, macos / 'canban')
        if not args.no_ui:
            env = {k: v for k, v in os.environ.items() if not k.startswith('CANBAN_')}
            env.update(CANBAN_HOME=str(home), CANBAN_APP_ID='com.canban.dev')
            ui = subprocess.Popen([str(macos / 'canban-ui')], env=env, start_new_session=True)
            state.write_text(json.dumps({'pid': ui.pid, 'identity': identity(ui.pid)}))
    if ui is not None:
        try:
            return ui.wait()
        except KeyboardInterrupt:
            ui.terminate()
            ui.wait(timeout=10)
            return 130
        finally:
            if state.exists() and json.loads(state.read_text()).get('pid') == ui.pid:
                state.unlink()
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print('Canban development startup: ' + str(error), file=sys.stderr)
        sys.exit(1)
