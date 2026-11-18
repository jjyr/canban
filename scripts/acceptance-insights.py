#!/usr/bin/env python3
"""Retained Insights source facts through an isolated AppServer/Core and real PTY.

--native opens a copy of the packaged production app for CUA/manual interaction.
Create <base>/stop to close only this fixture and its Core.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--native', action='store_true')
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-insights-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
if not cli.exists() and (repo / '.local/Canban.app/Contents/MacOS/canban').exists():
    cli = repo / '.local/Canban.app/Contents/MacOS/canban'
core = {'HC_SESSION': 'insights-acceptance', 'HC_SOCKET': str(base / 'hc.sock'),
        'XDG_STATE_HOME': str(base / 'state'), 'XDG_CONFIG_HOME': str(base / 'config')}
env = {k: v for k, v in os.environ.items() if not k.startswith(('CANBAN_', 'HC_'))}
env.update(core)
env.update(CANBAN_HOME=str(home), CANBAN_UPDATES_DISABLED='1', CANBAN_GH_BIN='/usr/bin/false')
config = home / 'config'
config.mkdir(parents=True)
profiles = [{'id': role, 'preset': None, 'argv': ['/usr/bin/python3', str(repo / 'scripts/fixture-agent.py')],
             'resume_argv': [], 'exit': {'type': 'manual'}, 'session_instruction': ''}
            for role in ['developer', 'reviewer']]
(config / 'agents.json').write_text(json.dumps({'hc': shutil.which('hc'), 'hc_session': core['HC_SESSION'],
    'hc_env': core, 'default_agent': 'developer', 'profiles': profiles}))
log = (base / 'server.log').open('a')
server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)
ui = None


def call(method, params=None):
    result = subprocess.run([str(cli), 'call', method, json.dumps(params or {})], env=env,
                            capture_output=True, text=True, timeout=60)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def wait_for(predicate, label):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        try:
            snapshot = call('app.snapshot')
            if predicate(snapshot):
                return snapshot
        except RuntimeError:
            pass
        time.sleep(.2)
    raise RuntimeError(label + '\n' + (base / 'server.log').read_text()[-6000:])


try:
    wait_for(lambda _: True, 'AppServer did not start')
    projects = []
    for name in ['Insights acceptance', 'Second Project']:
        source = base / name.replace(' ', '-')
        source.mkdir()
        (source / 'README.md').write_text('Disposable Insights acceptance Project.\n')
        for git_args in [['init', '-b', 'main'], ['config', 'user.name', 'Acceptance'],
                         ['config', 'user.email', 'acceptance@example.invalid'],
                         ['config', 'core.hooksPath', '/dev/null'], ['config', 'commit.gpgsign', 'false'],
                         ['add', '.'], ['commit', '-m', 'Seed']]:
            subprocess.run(['git', '-C', str(source), *git_args], check=True, capture_output=True)
        projects.append(call('project.create', {'name': name, 'directory': str(source),
                          'workspace': {'create_worktree': True, 'sync_base': False}}))
    card = call('card.create', {'project_id': projects[0]['id'], 'title': 'Delivered Card for Insights', 'body': 'Deterministic PTY delivery.'})
    call('card.move', {'card_id': card['id'], 'revision': card['revision'], 'stage': 'develop'})
    snapshot = wait_for(lambda s: any(c['id'] == card['id'] and (c.get('delivery') or {}).get('merged_at', 0)
                                    for c in s['cards']), 'Fixture Card was not merged')
    facts = [e for e in snapshot['events'] if e['card_id'] == card['id']]
    assert any(e['kind'] == 'execution.started' for e in facts)
    assert any(e['kind'] == 'review.merged' for e in facts)
    for i in range(4):
        call('card.create', {'project_id': projects[i % 2]['id'], 'title': 'Planned Card ' + str(i+1), 'body': 'Retained creation fact.'})
    snapshot = call('app.snapshot')
    assert len(snapshot['projects']) == 2
    assert len(snapshot['cards']) == 5
    assert sum(bool(c.get('delivery')) for c in snapshot['cards']) == 1
    (base / 'snapshot.json').write_text(json.dumps(snapshot, indent=2))
    print('INSIGHTS_FACTS_ACCEPTED=' + str(base), flush=True)
    if args.native:
        server.terminate()
        server.wait(timeout=10)
        app_id = 'com.canban.insights-acceptance.' + base.name
        bundle = base / 'CanbanInsightsAcceptance.app'
        shutil.copytree(repo / '.local/Canban.app', bundle)
        plist = bundle / 'Contents/Info.plist'
        info = plistlib.loads(plist.read_bytes())
        info.update(CFBundleIdentifier=app_id, CFBundleName='Canban Insights Acceptance', CFBundleDisplayName='Canban Insights Acceptance')
        plist.write_bytes(plistlib.dumps(info))
        executable = bundle / 'Contents/MacOS/canban-ui'
        assert hashlib.sha256(executable.read_bytes()).digest() == hashlib.sha256((repo / '.local/Canban.app/Contents/MacOS/canban-ui').read_bytes()).digest()
        env['CANBAN_APP_ID'] = app_id
        ui = subprocess.Popen([str(executable)], env=env, stdout=log, stderr=log)
        metadata = {'base': str(base), 'home': str(home), 'core': core, 'bundle': str(bundle),
                    'app_id': app_id, 'pid': ui.pid, 'project_id': projects[0]['id'], 'card_id': card['id']}
        (base / 'native.json').write_text(json.dumps(metadata, indent=2))
        (repo / '.local/insights-acceptance.json').write_text(json.dumps(metadata, indent=2))
        print('NATIVE_INSIGHTS_READY=' + json.dumps(metadata), flush=True)
        while ui.poll() is None and not (base / 'stop').exists():
            time.sleep(.5)
finally:
    if ui and ui.poll() is None:
        ui.terminate()
        ui.wait(timeout=10)
    try:
        call('app.shutdown')
    except (RuntimeError, subprocess.TimeoutExpired):
        pass
    if server.poll() is None:
        server.terminate()
        server.wait(timeout=10)
    subprocess.run(['hc', 'server', 'stop'], env={**os.environ, **core}, capture_output=True, timeout=15)
    log.close()
