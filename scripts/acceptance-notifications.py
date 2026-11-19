#!/usr/bin/env python3
"""Unified reminders over real isolated Herdr PTYs; optional native UI handoff."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--native-bundle', type=Path)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-notifications-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
# Keep bootstrap and configured session identical, always in scratch XDG paths.
core = {'HC_SESSION': 'default', 'HC_SOCKET': str(base/'hc.sock'),
        'XDG_STATE_HOME': str(base/'hc-state'), 'XDG_CONFIG_HOME': str(base/'hc-config')}
env = {**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},
       **core, 'CANBAN_HOME': str(home), 'CANBAN_NOTIFICATION_TRACE': '1'}
log = (base/'server.log').open('w')
server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)

def call(method, params=None):
    r = subprocess.run([str(cli), "--full",'call', method, json.dumps(params or {})],
                       env=env, capture_output=True, text=True, timeout=45)
    if r.returncode:
        raise RuntimeError(r.stderr)
    return json.loads(r.stdout)

def wait(predicate, label):
    deadline = time.monotonic()+40
    while time.monotonic() < deadline:
        try:
            snapshot = call('app.snapshot')
            if predicate(snapshot):
                return snapshot
        except RuntimeError:
            pass
        time.sleep(.15)
    raise RuntimeError(f'Timed out: {label}\n{(base/"server.log").read_text()}')

def items(card):
    return [i for i in call('notifications.list') if i['card_id'] == card['id']]

def read(card):
    sequence = max(e['sequence'] for e in call('timeline.list', {'card_id': card['id'], 'limit': 200})['events'])
    return call('notifications.read', {'card_id':card['id'], 'sequence':sequence})

def run(card):
    opened = call('agent.open', {'card_id':card['id'], 'profile_id':'developer'})
    s = wait(lambda s:any(t['id'] == opened['terminal_id'] and (t.get('agent') or {}).get('provider_session_id') for t in s['terminals']), 'bound provider')
    entry = next(t for t in s['terminals'] if t['id'] == opened['terminal_id'])
    return {'id': entry['id'], **entry['agent']}

def send(attempt, text, force=False):
    r = subprocess.run(['hc','send',attempt['terminal'],text,'--json',*(['--force'] if force else [])],
                       env={**os.environ,**core},capture_output=True,text=True,check=True)
    return json.loads(r.stdout)

def question(card, text):
    before = len(call('app.snapshot')['questions'])
    call('comment.add', {'card_id':card['id'], 'body':text, 'mentions':['developer']})
    return wait(lambda s:len(s['questions']) > before and all(q['state']=='replied' for q in s['questions']), 'structured Agent reply')

try:
    wait(lambda _:True, 'AppServer ready')
    config_path = home/'config/agents.json'
    config = json.loads(config_path.read_text())
    config.update(hc_env=core, hc_session=core['HC_SESSION'])
    for profile in config['profiles']:
        profile.update(preset=None, exit={'type':'input','text':'/exit'}, session_instruction='',
                       argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],
                       resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
    config_path.write_text(json.dumps(config))
    call('config.reload')
    workspace={'sync_base':False}
    project = call('project.create', {'workspace':workspace,'name':'Notifications','directory':str(base/'source')})
    other = call('project.create', {'workspace':workspace,'name':'Another Project','directory':str(base/'other')})
    for directory in [base/'source',base/'other']:
        (directory/'README.md').write_text('# Disposable notification acceptance\n')
        for command in [['init','-b','main'],['config','user.name','Acceptance'],['config','user.email','acceptance@example.invalid'],['config','core.hooksPath','/dev/null'],['config','commit.gpgsign','false'],['add','.'],['commit','-m','Seed']]:
            subprocess.run(['git','-C',str(directory),*command],check=True,capture_output=True)
    cards = {}
    for key, title, project_id, body in [
        ('short','Visible replies',project['id'],'A short Card for foreground reading.'),
        ('long','Replies below long context',project['id'],''.join(f'### Context {i}\n\nKeep unseen replies unread.\n\n' for i in range(30))),
        ('gate','Human decision stays pending',project['id'],'Reading a report never approves this task.'),
        ('other','Another Project reply',other['id'],'Cross-Project unread reply.')]:
        cards[key] = call('card.create', {'project_id':project_id,'title':title,'body':body})
    short_run = run(cards['short'])
    question(cards['short'],'Explain the current task')
    assert len(items(cards['short'])) == 1 and items(cards['short'])[0]['unread']
    send(short_run,'permission')
    wait(lambda s:any(t['id']==short_run['id'] and (t.get('agent') or {}).get('lifecycle',{}).get('waiting') for t in s['terminals']), 'permission episode')
    assert len(items(cards['short'])) == 2
    before = next(c for c in call('app.snapshot')['cards'] if c['id']==cards['short']['id'])
    position = read(cards['short'])
    assert len(items(cards['short'])) == 1 and items(cards['short'])[0]['pending']
    assert call('notifications.read',{'card_id':cards['short']['id'],'sequence':1}) == position
    assert before == next(c for c in call('app.snapshot')['cards'] if c['id']==cards['short']['id'])
    send(short_run,'working',force=True)
    wait(lambda s:any(t['id']==short_run['id'] and not (t.get('agent') or {}).get('lifecycle',{}).get('waiting') for t in s['terminals']), 'permission resolved')
    assert not items(cards['short'])
    print('REPLY_READ_PERMISSION_OK',flush=True)
    gate_run = run(cards['gate'])
    send(gate_run,'block')
    wait(lambda s:any(c['id']==cards['gate']['id'] and c.get('human_gate') for c in s['cards']), 'task gate')
    assert len(items(cards['gate'])) == 1  # unread report + unresolved gate
    read(cards['gate'])
    assert len(items(cards['gate'])) == 1 and not items(cards['gate'])[0]['unread']
    assert next(c for c in call('app.snapshot')['cards'] if c['id']==cards['gate']['id'])['human_gate']
    print('REPORT_GATE_DEDUP_OK',flush=True)
    for key in ['long','other']:
        run(cards[key]);question(cards[key],'Explain this Card')
    question(cards['short'],'New unread reply for native verification')
    call('utilities.configure', {'notification_sound':False})
    assert call('utilities.state')['preferences']['notification_sound'] is False
    call('utilities.configure', {'notification_sound':True})
    saved = call('app.snapshot')['card_reads']
    call('app.shutdown');server.wait(timeout=15)
    server = subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
    wait(lambda s:s['card_reads']==saved, 'durable read positions')
    assert len(items(cards['gate'])) == 1 and not items(cards['gate'])[0]['unread']
    metadata={'base':str(base),'home':str(home),'core_env':core,'projects':[project,other],'cards':cards,'short_run':short_run}
    (base/'metadata.json').write_text(json.dumps(metadata,indent=2))
    (repo/'.local/notifications-native.json').write_text(json.dumps(metadata,indent=2))
    print(f'NOTIFICATIONS_ACCEPTANCE_PASSED\nNOTIFICATIONS_HOME={home}',flush=True)
    if args.native_bundle:
        call('app.shutdown');server.wait(timeout=15)
        bundle=args.native_bundle.resolve()
        import plistlib
        env['CANBAN_APP_ID']=plistlib.loads((bundle/'Contents/Info.plist').read_bytes())['CFBundleIdentifier']
        for name in ['canban-ui','canban']:
            dest=bundle/'Contents/MacOS'/name
            staged=dest.with_name('.notifications-'+name)
            shutil.copy2(repo/'.local/Canban.app/Contents/MacOS'/name,staged);os.replace(staged,dest)
        server=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=env,stdout=log,stderr=log)
        server.wait()
finally:
    try:
        (base/'final-snapshot.json').write_text(json.dumps(call('app.snapshot'),indent=2))
        call('app.shutdown')
    except RuntimeError:
        pass
    if server.poll() is None:
        server.terminate();server.wait(timeout=15)
    subprocess.run(['hc','server','stop'],env={**os.environ,**core},capture_output=True,timeout=15)
    log.close()
