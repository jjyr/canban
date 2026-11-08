#!/usr/bin/env python3
"""Open an empty, isolated native workspace for weak Terminal-context acceptance.
The helper owns only its copied app, scratch Canban home and scratch Herdr Core.
Create the first terminal in the UI before registering Projects through the CLI.
"""
from pathlib import Path
import json, os, plistlib, shutil, subprocess, tempfile, time

repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-terminal-context-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
app_id = 'com.canban.context.' + base.name
bundle = base / 'CanbanContext.app'
shutil.copytree(repo / '.local/Canban.app', bundle)
plist = bundle / 'Contents/Info.plist'
info = plistlib.loads(plist.read_bytes())
info.update(CFBundleIdentifier=app_id, CFBundleDisplayName='Canban Context Acceptance', CFBundleName='Canban Context Acceptance')
plist.write_bytes(plistlib.dumps(info))
env = {**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}, 'CANBAN_HOME':str(home), 'CANBAN_APP_ID':app_id}
core_env = {'HC_SESSION':'terminal-context-acceptance', 'HC_SOCKET':str(base/'hc.sock'), 'XDG_STATE_HOME':str(base/'hc-state'), 'XDG_CONFIG_HOME':str(base/'hc-config')}
wrapper = base / 'hc-wrapper'
wrapper.write_text('#!/bin/sh\nif [ -e "'+str(base/'offline')+'" ]; then echo "Scratch transport unavailable" >&2; exit 1; fi\nexec '+shutil.which('hc')+' "$@"\n')
wrapper.chmod(0o755)
log = open(base/'runtime.log','w')
server = subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(method, params=None):
    r = subprocess.run([str(cli),'call',method,json.dumps(params or {})],env=env,capture_output=True,text=True,timeout=30)
    if r.returncode: raise RuntimeError(r.stderr)
    return json.loads(r.stdout)
try:
    for _ in range(100):
        try: call('app.ping'); break
        except RuntimeError: time.sleep(.1)
    config = json.loads((home/'config/agents.json').read_text())
    config.update(hc=str(wrapper),hc_env=core_env,hc_session=core_env['HC_SESSION'])
    for profile in config['profiles']:
        profile.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
    (home/'config/agents.json').write_text(json.dumps(config,indent=2))
    call('config.reload')
    metadata = {'base':str(base),'home':str(home),'app_id':app_id,'bundle':str(bundle),'core_env':core_env}
    (base/'native.json').write_text(json.dumps(metadata,indent=2))
    (repo/'.local/terminal-context-acceptance.json').write_text(json.dumps(metadata,indent=2))
    assert not call('app.snapshot')['projects']
    server.terminate();server.wait(timeout=10)
    print('TERMINAL_CONTEXT_BASE='+str(base)+'\nAPP_ID='+app_id,flush=True)
    while True:
        server = subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=env,stdout=log,stderr=log)
        server.wait()
        if not (base/'restart').exists(): break
        (base/'restart').unlink()
        for name in ('canban-ui','canban'):
            shutil.copy2(repo/'.local/Canban.app/Contents/MacOS'/name,bundle/'Contents/MacOS'/name)
finally:
    try: call('app.shutdown')
    except Exception: pass
    if server.poll() is None: server.terminate();server.wait(timeout=10)
    subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=20)
    log.close()
