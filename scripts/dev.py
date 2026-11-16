#!/usr/bin/env python3
"""Start/stop the current checkout + branch development instance. Never stops Herdr."""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]
ROOT = REPO / '.local/dev'


def atomic(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
        json.dump(value, stream, indent=2)
        stream.write('\n')
        temporary = Path(stream.name)
    os.replace(temporary, path)


def read(path):
    return json.loads(path.read_text()) if path.exists() else None


def git(*args):
    return subprocess.check_output(['git', '-C', str(REPO), *args], text=True).strip()


def bundle_bin(directory):
    directory = Path(directory)
    if sys.platform == 'darwin':
        return directory / 'CanbanDev.app/Contents/MacOS'
    return directory / 'CanbanDev/bin'


def select(branch=None, home=None):
    branch = branch or git('branch', '--show-current') or 'detached-' + git('rev-parse', '--short=12', 'HEAD')
    home = home or os.environ.get('CANBAN_HOME')
    if home:
        home = Path(home).expanduser().resolve()
        selection = 'home:' + str(home)
        label = 'custom'
    else:
        selection = 'branch:' + branch
        label = re.sub(r'[^A-Za-z0-9-]+', '-', branch).strip('-')[:20] or 'branch'
    key = hashlib.sha256((str(REPO) + '\0' + selection).encode()).hexdigest()[:12]
    directory = ROOT / (label + '-' + key)
    home = home or directory / 'home'
    if home == (Path.home() / '.canban').resolve():
        raise RuntimeError('Development commands do not manage the installed ~/.canban; choose a development home')
    sock_limit = 104 if sys.platform == 'darwin' else 108
    if len(os.fsencode(home / 'app.sock')) >= sock_limit:
        platform_label = 'macOS' if sys.platform == 'darwin' else 'Unix domain sockets'
        raise RuntimeError(f'Development socket path is too long for {platform_label}; set CANBAN_HOME to a shorter absolute path')
    return dict(checkout=str(REPO), branch=branch, key=key, directory=str(directory),
                home=str(home), app_id='com.canban.dev.' + key)


def validate(instance):
    directory = Path(instance['directory'])
    if instance['checkout'] != str(REPO) or directory.parent != ROOT:
        raise RuntimeError('Instance belongs to a different checkout')
    if Path(instance['home']).resolve() == (Path.home() / '.canban').resolve():
        raise RuntimeError('Refusing to manage the installed Canban home')
    if not Path(instance['home']).is_absolute():
        raise RuntimeError('Instance home must be absolute')


@contextmanager
def locked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def rpc(endpoint, method, params=None, timeout=5):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(timeout)
        stream.connect(str(endpoint))
        stream.sendall((json.dumps(dict(jsonrpc='2.0', id=1, method=method, params=params or {})) + '\n').encode())
        with stream.makefile('rb') as reader:
            response = json.loads(reader.readline(8 * 1024 * 1024))
        if 'error' in response:
            raise RuntimeError(str(response['error']))
        return response['result']


def ping(home):
    try:
        return rpc(home / 'app.sock', 'app.ping')
    except (FileNotFoundError, ConnectionRefusedError):
        return None


def identity(pid):
    ps_bin = '/bin/ps' if sys.platform == 'darwin' else (shutil.which('ps') or '/bin/ps')

    def ps(field):
        result = subprocess.run([ps_bin, '-ww', '-p', str(pid), '-o', field + '='], capture_output=True, text=True)
        return result.stdout.strip() if result.returncode == 0 else ''

    stat = ps('stat')
    if not stat or stat.startswith('Z'):
        return None
    if sys.platform.startswith('linux'):
        try:
            executable = os.readlink(f'/proc/{pid}/exe')
        except OSError:
            return None
        if executable.endswith(' (deleted)'):
            executable = executable[:-10]
    else:
        executable = ps('comm')
    return dict(start=ps('lstart'), command=ps('command'), executable=executable)


def live(record, executable):
    if not record:
        return False
    current = identity(record['pid'])
    return bool(current and current == record['identity'] and current['executable'] == str(executable))


def wait_free(path, label, timeout=20):
    deadline = time.monotonic() + timeout
    with path.open('a') as stream:
        while True:
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise RuntimeError(label + ' did not stop; no process was force-killed')
                time.sleep(.1)


def assert_owner(instance):
    home = Path(instance['home'])
    owner = read(home / '.dev-owner.json')
    expected = dict(checkout=instance['checkout'], directory=instance['directory'])
    if owner and owner != expected:
        raise RuntimeError('This home is managed by another development instance')
    return expected


def stop_locked(instance):
    directory, home = Path(instance['directory']), Path(instance['home'])
    assert_owner(instance)
    bins = bundle_bin(directory)
    server = read(directory / 'server.json')
    current = ping(home)
    if current and (not live(server, bins / 'canban') or current.get('pid') != server['pid']
                    or Path(current['home']).resolve() != home):
        raise RuntimeError('Another or unrecorded AppServer owns this home; refusing to stop it')
    ui = read(directory / 'ui.json')
    if live(ui, bins / 'canban-ui'):
        # The desktop translates SIGTERM into normal quit and flushes drafts.
        os.kill(ui['pid'], signal.SIGTERM)
        deadline = time.monotonic() + 20
        while live(ui, bins / 'canban-ui'):
            if time.monotonic() >= deadline:
                raise RuntimeError('Development UI did not quit; AppServer remains running')
            time.sleep(.1)
    wait_free(directory / 'ui.lock', 'Development UI')
    if current:
        rpc(home / 'app.sock', 'app.shutdown', {'expected_pid': server['pid']})
    elif live(server, bins / 'canban'):
        raise RuntimeError('Recorded AppServer is not responding; refusing to force-kill it')
    wait_free(home / 'server.lock', 'AppServer')
    for name in ('ui.json', 'server.json'):
        (directory / name).unlink(missing_ok=True)
    print('Stopped development instance: ' + instance['branch'] + ' · ' + str(home), flush=True)


def stop(instance):
    validate(instance)
    directory, home = Path(instance['directory']), Path(instance['home'])
    if not directory.exists():
        print('Development instance is already stopped: ' + instance['branch'])
        return
    if not home.exists():
        raise RuntimeError('Recorded development home is missing; inspect ' + str(directory))
    with locked(directory / 'control.lock'), locked(home / '.dev-control.lock'):
        stop_locked(instance)


def environment(instance):
    # Preserve intentional update fixtures/configuration, but never inherited Agent or endpoint bindings.
    env = {k: v for k, v in os.environ.items() if not k.startswith('CANBAN_')}
    env.update({k: v for k, v in os.environ.items() if k.startswith('CANBAN_UPDATE_')})
    env.update(CANBAN_HOME=instance['home'], CANBAN_APP_ID=instance['app_id'], CANBAN_UPDATES_DISABLED='1')
    return env


def start(instance, no_ui=False):
    validate(instance)
    directory, home = Path(instance['directory']), Path(instance['home'])
    home.mkdir(parents=True, exist_ok=True)
    with locked(directory / 'control.lock'), locked(home / '.dev-control.lock'):
        owner = assert_owner(instance)
        stop_locked(instance)
        atomic(home / '.dev-owner.json', owner)
        descriptor = directory / 'instance.json'
        atomic(descriptor, instance)
        subprocess.run([sys.executable, str(REPO / 'scripts/package.py'), '--dev', '--dev-instance', str(descriptor)], cwd=REPO, check=True)
        bins = bundle_bin(directory)
        env = environment(instance)
        build = json.loads(subprocess.check_output([str(bins / 'canban'), 'build-info'], env=env, text=True))
        try:
            with (home / 'server.log').open('ab') as log:
                server = subprocess.Popen([str(bins / 'canban'), '--home', str(home), 'serve'], env=env,
                                          stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            atomic(directory / 'server.json', dict(pid=server.pid, identity=identity(server.pid)))
            deadline = time.monotonic() + 30
            while True:
                if server.poll() is not None:
                    raise RuntimeError('AppServer exited; inspect ' + str(home / 'server.log'))
                current = ping(home)
                if current and current.get('pid') == server.pid:
                    if Path(current['home']).resolve() != home or current.get('build') != build or not current.get('updates_disabled'):
                        raise RuntimeError('Development AppServer identity/build/policy mismatch')
                    rpc(home / 'app.sock', 'utilities.state')
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError('AppServer readiness timed out')
                time.sleep(.1)
            if not no_ui:
                with (home / 'ui.log').open('ab') as log:
                    ui = subprocess.Popen([str(bins / 'canban-ui')], env=env, stdin=subprocess.DEVNULL,
                                          stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic() + 30
                while True:
                    registration = read(directory / 'ui.json')
                    if registration and registration.get('error'):
                        raise RuntimeError(registration['error'])
                    if registration and registration.get('pid') == ui.pid and registration.get('ready'):
                        break
                    if ui.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError('Development UI failed to start; inspect ' + str(home / 'ui.log'))
                    time.sleep(.1)
            print(json.dumps(instance, indent=2), flush=True)
        except BaseException:
            stop_locked(instance)
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['start', 'stop', 'info'], nargs='?', default='start')
    parser.add_argument('--branch', help='Select a branch to stop, even after switching branches')
    parser.add_argument('--all', action='store_true', help='Stop every recorded instance of this checkout')
    parser.add_argument('--home', help='Explicit development home; overrides CANBAN_HOME')
    parser.add_argument('--no-ui', action='store_true', help='Start only the development AppServer')
    args = parser.parse_args()
    if args.command == 'start' and (args.branch or args.all):
        parser.error('Start always uses the checked-out branch; --branch/--all select instances to stop')
    if args.all and (args.command != 'stop' or args.branch or args.home):
        parser.error('--all is for stop and cannot be combined with --branch/--home')
    if args.branch and (args.home or os.environ.get('CANBAN_HOME')):
        parser.error('--branch cannot be combined with an explicit home')
    if args.command != 'info' and 'CANBAN_ENDPOINT' in os.environ:
        raise RuntimeError('CANBAN_ENDPOINT is attach-only; development commands will not manage that service')
    if args.all:
        errors = []
        for path in sorted(ROOT.glob('*/instance.json')):
            try:
                stop(read(path))
            except Exception as error:
                errors.append(str(path) + ': ' + str(error))
        if errors:
            raise RuntimeError('\n'.join(errors))
    else:
        instance = select(args.branch, args.home)
        if args.command == 'info':
            print(json.dumps(instance, indent=2))
        elif args.command == 'stop':
            stop(instance)
        else:
            start(instance, args.no_ui)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print('Canban development: ' + str(error), file=sys.stderr)
        sys.exit(1)
