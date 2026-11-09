#!/usr/bin/env python3
"""Verify dev-service replacement using an isolated home/Core and a live PTY."""
from pathlib import Path
import fcntl
import json
import os
import socket
import subprocess
import tempfile
import threading
import time

repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-dev-start-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
env = {k: v for k, v in os.environ.items() if not k.startswith('CANBAN_')}
env['CANBAN_HOME'] = str(home)
core_env = {'HC_SESSION': 'dev-start-acceptance', 'HC_SOCKET': str(base / 'hc.sock'),
            'XDG_STATE_HOME': str(base / 'state'), 'XDG_CONFIG_HOME': str(base / 'config')}
log = (base / 'acceptance.log').open('w')


def call(method, params=None):
    result = subprocess.run([str(cli), 'call', method, json.dumps(params or {})], env=env,
                            capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def ready():
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            return call('app.ping')
        except RuntimeError:
            time.sleep(.1)
    raise RuntimeError('AppServer readiness timeout')


def stopped():
    call('app.shutdown')
    deadline = time.monotonic() + 15
    while (home / 'app.sock').exists():
        if time.monotonic() > deadline:
            raise RuntimeError('AppServer shutdown timeout')
        time.sleep(.1)


def run_dev(extra_env=None, check=True):
    return subprocess.run(['/usr/bin/python3', str(repo / 'scripts/dev.py'), '--no-ui'],
                          env={**env, **(extra_env or {})}, stdout=log, stderr=log,
                          timeout=45, check=check)


def hc(*args):
    result = subprocess.run([config['hc'], *args], env={**env, **core_env}, capture_output=True,
                            text=True, timeout=15, check=True)
    return json.loads(result.stdout)


legacy_error = []
def legacy_service(available):
    """Old service supports ping/shutdown, with no desktop utilities/API version."""
    try:
        with (home / 'server.lock').open('a') as lock, socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            listener.bind(str(home / 'app.sock'))
            listener.listen()
            available.set()
            while True:
                with listener.accept()[0] as stream:
                    request = json.loads(stream.makefile('rb').readline())
                    stop = request['method'] == 'app.shutdown'
                    result = {'stopping': True} if stop else {'version': '0.1.0', 'home': str(home)}
                    stream.sendall((json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}) + '\n').encode())
                if stop:
                    break
            (home / 'app.sock').unlink()
    except Exception as error:
        legacy_error.append(error)
        available.set()


server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)
try:
    original = ready()
    config = json.loads((home / 'config/agents.json').read_text())
    config.update(hc_env=core_env, hc_session=core_env['HC_SESSION'])
    (home / 'config/agents.json').write_text(json.dumps(config))
    call('config.reload')
    terminal = call('terminal.create', {'cwd': str(base)})
    binding = terminal['runtime']
    before = hc('get', binding['terminal'], '--json')
    call('utilities.configure', {'theme': 'dracula'})
    run_dev()
    current = ready()
    assert current['pid'] != original['pid']
    assert call('utilities.state')['preferences'] == {'theme': 'dracula'}
    after = hc('get', binding['terminal'], '--json')
    assert before['pid'] == after['pid'] and after['state'] == 'live'
    saved = next(t for t in call('app.snapshot')['terminals'] if t['id'] == terminal['id'])
    assert saved['runtime']['core_id'] == binding['core_id']
    assert saved['runtime']['terminal'] == binding['terminal']
    stopped()
    available = threading.Event()
    worker = threading.Thread(target=legacy_service, args=(available,), daemon=True)
    worker.start()
    assert available.wait(5)
    assert not legacy_error, legacy_error
    assert 'api_version' not in call('app.ping')
    run_dev()
    worker.join(timeout=5)
    assert not worker.is_alive() and not legacy_error
    upgraded = ready()
    assert upgraded['api_version'] == 4
    assert call('utilities.state')['preferences']['theme'] == 'dracula'
    assert hc('get', binding['terminal'], '--json')['pid'] == before['pid']
    assert run_dev({'CANBAN_ENDPOINT': str(home / 'app.sock')}, check=False).returncode != 0
    assert call('app.ping')['pid'] == upgraded['pid']
    (base / 'results.json').write_text(json.dumps({
        'home': str(home), 'initial_pid': original['pid'], 'restarted_pid': current['pid'],
        'upgraded_pid': upgraded['pid'], 'core_terminal': binding['terminal'],
        'pty_pid': before['pid'], 'theme_survived': True, 'legacy_upgrade': True,
        'explicit_endpoint_untouched': True, 'paid_agents_started': 0
    }, indent=2) + '\n')
    print('DEV_START_ACCEPTANCE_OK ' + str(base), flush=True)
finally:
    try:
        call('app.shutdown')
    except Exception:
        pass
    try:
        hc('server', 'stop')
    except Exception:
        pass
    server.wait(timeout=10)
    log.close()
