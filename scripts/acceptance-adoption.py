#!/usr/bin/env python3
"""Adopt an existing real PTY through the ordinary CLI, with an isolated Core/home.

--native opens the packaged app before adoption; type 'adopt' in its terminal,
then create <base>/continue to run restart/report/idle-exit/resume checks.
"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--native', action='store_true')
options = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-adoption-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
core = {'HC_SESSION': 'adoption-acceptance', 'HC_SOCKET': str(base / 'hc.sock'),
        'XDG_STATE_HOME': str(base / 'state'), 'XDG_CONFIG_HOME': str(base / 'config')}
env = {k: v for k, v in os.environ.items() if not k.startswith(('CANBAN_', 'HC_'))}
env.update(core, CANBAN_HOME=str(home), CANBAN_UPDATES_DISABLED='1', CANBAN_GH_BIN='/usr/bin/false')
fixture = base / 'provider.py'
fixture.write_text(r'''import json, os, subprocess, sys, uuid
from pathlib import Path
home = Path(os.environ['CANBAN_HOME'])
args = sys.argv[1:]
session = args[args.index('--resume') + 1] if '--resume' in args else str(uuid.uuid4())
history = home / ('history-' + session)
if '--resume' in args: assert history.exists()
history.write_text(session)
def cli(*args):
    r = subprocess.run([os.environ['CANBAN_CLI'], *args], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)
def status(value): print('\x1b]7501;state=' + value + '\x1b\\', end='', flush=True)
cli('bind-session', '--session-id', session)
info = {'pid': os.getpid(), 'terminal': os.environ.get('HC_TERMINAL_ID'), 'session': session}
(home / 'provider.json').write_text(json.dumps(info))
print('MANUAL_READY ' + session, flush=True)
status('working')
for line in sys.stdin:
    line = line.strip()
    if line == 'adopt':
        receipt = cli('card', 'adopt')
        (home / 'adopted.json').write_text(json.dumps(receipt))
        print('ADOPTED_SAME_PROCESS ' + str(os.getpid()), flush=True)
        status('done')
    elif line == 'finish':
        cli('report', '--state', 'completed', '--message', 'Adopted fixture completed')
        print('FINAL_RESPONSE_SAVED', flush=True)
        status('done')
    elif line == '/exit':
        history.write_text(session)
        (home / 'exited.json').write_text(json.dumps(info))
        print('NORMAL_EXIT ' + session, flush=True)
        break
    else:
        status('working')
        print('INPUT ' + line, flush=True)
''')
(home / 'config').mkdir(parents=True)
profile = {'id': 'developer', 'preset': None, 'session_instruction': '',
           'argv': ['/usr/bin/python3', str(fixture)],
           'resume_argv': ['/usr/bin/python3', str(fixture), '--resume', '{session_id}'],
           'exit': {'type': 'input', 'text': '/exit'}}
(home / 'config/agents.json').write_text(json.dumps({
    'hc': shutil.which('hc'), 'hc_env': core, 'hc_session': core['HC_SESSION'],
    'default_agent': 'developer', 'profiles': [profile],
    'scheduler': {'max_agents': 8, 'idle_minutes': 1}}))
log = (base / 'server.log').open('w')
server = ui = None


def call(method, params=None):
    r = subprocess.run([str(cli), '--full', 'call', method, json.dumps(params or {})],
                       env=env, capture_output=True, text=True, timeout=45)
    if r.returncode:
        raise RuntimeError(r.stderr)
    return json.loads(r.stdout)


def wait(predicate, label, timeout=50):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            snapshot = call('app.snapshot')
        except RuntimeError:
            time.sleep(.15)
            continue
        if predicate(snapshot):
            return snapshot
        time.sleep(.15)
    raise RuntimeError(label + ' timed out: ' + str(base) + '\n' + (base / 'server.log').read_text())


def start():
    global server
    server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)
    wait(lambda _: True, 'server startup')


def send(terminal, text):
    subprocess.run(['hc', 'send', terminal, text, '--json'], env=env,
                   capture_output=True, text=True, check=True, timeout=20)


def agent(snapshot):
    return next(t['agent'] for t in snapshot['terminals'] if t.get('agent'))


try:
    start()
    project = call('project.create', {'name': 'Terminal adoption', 'directory': str(base / 'source'),
                                    'workspace': {'create_worktree': False, 'sync_base': False}})
    # Stop at human Review so delivery cannot obscure terminal lifecycle checks.
    card = call('card.create', {'project_id': project['id'], 'title': 'Adopt this existing terminal',
                               'hold_auto_merge': True})
    call('card.hold_auto_merge', {'card_id': card['id'], 'revision': card['revision'], 'hold': True})
    manual = call('terminal.create', {'card_id': card['id'], 'agent': True,
                                      'profile_id': 'developer', 'name': 'Original manual terminal'})
    wait(lambda s: (home / 'provider.json').exists(), 'manual provider startup')
    before = json.loads((home / 'provider.json').read_text())
    terminal = manual['runtime']['terminal']
    assert before['terminal'] == terminal
    assert not any(t.get('agent') for t in call('app.snapshot')['terminals'])
    if options.native:
        bundle = base / 'CanbanAdoption.app'
        shutil.copytree(repo / '.local/Canban.app', bundle)
        plist = bundle / 'Contents/Info.plist'
        info = plistlib.loads(plist.read_bytes())
        info.update(CFBundleIdentifier='com.canban.adoption.' + base.name, CFBundleName='Canban Adoption')
        plist.write_bytes(plistlib.dumps(info))
        subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(bundle)], check=True, capture_output=True)
        env['CANBAN_APP_ID'] = info['CFBundleIdentifier']
        ui = subprocess.Popen([str(bundle / 'Contents/MacOS/canban-ui')], env=env, stdout=log, stderr=log)
        metadata = {'base': str(base), 'app_id': info['CFBundleIdentifier'], 'pid': ui.pid,
                    'terminal_id': manual['id'], 'terminal': terminal, 'core': core, 'home': str(home)}
        (repo / '.local/adoption-acceptance.json').write_text(json.dumps(metadata, indent=2))
        print('NATIVE_ADOPTION_READY ' + json.dumps(metadata), flush=True)
        wait(lambda _: (base / 'continue').exists(), 'native adoption interaction', timeout=600)
    else:
        send(terminal, 'adopt')
    adopted = wait(lambda s: any(t.get('agent') for t in s['terminals']), 'runtime adoption')
    entry = next(t for t in adopted['terminals'] if t.get('agent'))
    run = entry['agent']
    assert entry['id'] != manual['id'] and run['terminal'] == terminal
    assert run['provider_session_id'] == before['session']
    assert run['observed']['pid'] == before['pid']
    assert run['state'] == 'running' and run['workflow_run_id']
    assert not any(t.get('runtime', {}).get('terminal') == terminal for t in adopted['terminals'] if t.get('runtime'))
    send(terminal, 'adopt')
    time.sleep(.5)
    assert agent(call('app.snapshot'))['number'] == run['number']
    # Restart only AppServer. The Core still owns the original live process.
    call('app.shutdown')
    server.wait(timeout=15)
    start()
    restored = wait(lambda s: agent(s)['connection_state'] == 'connected', 'restored binding')
    assert agent(restored)['observed']['pid'] == before['pid']
    assert agent(restored)['provider_session_id'] == before['session']
    send(terminal, 'finish')
    completed_at = time.monotonic()
    ended = wait(lambda s: agent(s)['state'] == 'idle_closed'
                 and (agent(s).get('observed') or {}).get('state') == 'finished',
                 'completed idle exit', timeout=50)
    assert time.monotonic() - completed_at >= 28
    closed = agent(ended)
    assert closed['state'] == 'idle_closed' and closed['lifecycle']['close_reason'] == 'idle'
    assert any(r['state'] == 'completed' and r['terminal_id'] == entry['id'] for r in ended['reports'])
    assert closed['provider_session_id'] == before['session']
    assert closed['observed']['exit_code'] == 0
    assert json.loads((home / 'exited.json').read_text()) == before
    output = call('execution.output', {'terminal_id': entry['id']})
    # The ordinary lifecycle saves output before sending /exit.
    assert 'FINAL_RESPONSE_SAVED' in json.dumps(output)
    call('agent.open', {'card_id': card['id'], 'profile_id': 'developer', 'expected_terminal_id': entry['id']})
    resumed = wait(lambda s: agent(s)['number'] > run['number'] and (agent(s).get('observed') or {}).get('state') == 'live', 'exact resume')
    assert agent(resumed)['provider_session_id'] == before['session']
    assert agent(resumed)['terminal'] != terminal
    (base / 'evidence.json').write_text(json.dumps({'before': before, 'adopted': adopted,
        'restored': restored, 'ended': ended, 'resumed': resumed}, indent=2))
    print('ADOPTION_ACCEPTANCE_PASSED BASE=' + str(base), flush=True)
finally:
    if ui is not None and ui.poll() is None:
        ui.terminate()
        ui.wait(timeout=10)
    if server is not None and server.poll() is None:
        try:
            call('app.shutdown')
            server.wait(timeout=15)
        except (RuntimeError, subprocess.TimeoutExpired):
            server.terminate()
            server.wait(timeout=10)
    subprocess.run(['hc', 'server', 'stop'], env=env, capture_output=True, timeout=20)
    log.close()
