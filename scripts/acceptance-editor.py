#!/usr/bin/env python3
"""Prepare a disposable packaged app for native Card editor acceptance.
No real Agent, existing Core session, GitHub account or user Canban home is touched.
The launcher restarts when its own `restart` marker exists on window exit.
"""
from pathlib import Path
import json, os, plistlib, shutil, subprocess, tempfile, time
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-editor-', dir='/private/tmp'))
home = base / 'home'
bundle = base / 'CanbanEditorAcceptance.app'
shutil.copytree(repo / '.local/Canban.app', bundle)
app_id = 'com.canban.editor.' + base.name
plist = bundle / 'Contents/Info.plist'
info = plistlib.loads(plist.read_bytes())
info.update(CFBundleIdentifier=app_id, CFBundleDisplayName='Canban Editor Acceptance', CFBundleName='Canban Editor Acceptance')
plist.write_bytes(plistlib.dumps(info))
env = {k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}
core_env = {'HC_SESSION':'editor-acceptance', 'HC_SOCKET':str(base/'hc.sock'), 'XDG_STATE_HOME':str(base/'state'), 'XDG_CONFIG_HOME':str(base/'config')}
env.update(core_env)
env.update(CANBAN_HOME=str(home), CANBAN_APP_ID=app_id)
cli = repo / 'target/debug/canban'
log = open(base / 'server.log', 'w')
server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)
def call(method, params=None):
    result = subprocess.run([str(cli), 'call', method, json.dumps(params or {})], env=env, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)
try:
    for _ in range(100):
        try:
            call('app.ping')
            break
        except RuntimeError:
            time.sleep(.1)
    config = json.loads((home / 'config/agents.json').read_text())
    config.update(hc_env=core_env,hc_session=core_env['HC_SESSION'])
    (home / 'config/agents.json').write_text(json.dumps(config))
    call('config.reload')
    source = base / 'source'
    source.mkdir()
    project = call('project.create', {'name':'Editor acceptance','directory':str(source),'workspace':{'create_worktree':False,'sync_base':False}})
    cards = [call('card.create', {'project_id':project['id'],'title':title,'body':'## Task context\n\n- [ ] Verify native Markdown editing\n\n```rust\nlet value = 42;\n```'}) for title in ['Editor baseline', 'Related task', 'Background refresh task']]
    call('utilities.configure', {'last_project':project['id']})
    metadata = dict(base=str(base), home=str(home), bundle=str(bundle), app_id=app_id, project_id=project['id'], card_ids=[c['id'] for c in cards], server_pid=server.pid)
    (base / 'native.json').write_text(json.dumps(metadata, indent=2))
    print('EDITOR_ACCEPTANCE_BASE=' + str(base), flush=True)
    while True:
        ui = subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')], env=env, stdout=log, stderr=log)
        metadata['ui_pid']=ui.pid
        (base/'native.json').write_text(json.dumps(metadata,indent=2))
        ui.wait()
        if not (base/'restart').exists():
            break
        (base/'restart').unlink()
        for name in ('canban-ui','canban'):
            staged=bundle/'Contents/MacOS'/('.next-'+name)
            shutil.copy2(repo/'.local/Canban.app/Contents/MacOS'/name,staged)
            os.replace(staged,bundle/'Contents/MacOS'/name)
finally:
    try:
        call('app.shutdown')
    except Exception:
        pass
    if server.poll() is None:
        server.terminate()
        server.wait(timeout=10)
    subprocess.run(['hc','server','stop'],env=env,capture_output=True,timeout=15)
    log.close()
