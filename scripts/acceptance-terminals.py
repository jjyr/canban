#!/usr/bin/env python3
"""Prepare isolated real Herdr PTYs for manual/CUA Terminal workspace acceptance.
Never opens ~/.canban or the user's Core. Providers are deterministic fixtures.
Close the acceptance app to stop this helper and its task-owned Core.
"""
from pathlib import Path
import json, os, plistlib, shutil, subprocess, tempfile, time
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-terminals-', dir='/private/tmp'))
home = base / 'home'; cli = repo / 'target/debug/canban'
app_id = 'com.canban.terminals.' + base.name
bundle = base / 'CanbanTerminals.app'; shutil.copytree(repo / '.local/Canban.app', bundle)
plist = bundle / 'Contents/Info.plist'; info = plistlib.loads(plist.read_bytes())
info.update(CFBundleIdentifier=app_id, CFBundleDisplayName='Canban Terminals Acceptance', CFBundleName='Canban Terminals Acceptance')
plist.write_bytes(plistlib.dumps(info))
env = {**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}, 'CANBAN_HOME':str(home), 'CANBAN_APP_ID':app_id}
core_env = {'HC_SESSION':'terminals-acceptance', 'HC_SOCKET':str(base/'hc.sock'), 'XDG_STATE_HOME':str(base/'hc-state'), 'XDG_CONFIG_HOME':str(base/'hc-config')}
wrapper = base / 'hc-wrapper'
wrapper.write_text('#!/bin/sh\nif [ -e "'+str(base/'offline')+'" ]; then echo "Scratch transport unavailable" >&2; exit 1; fi\nexec '+shutil.which('hc')+' "$@"\n'); wrapper.chmod(0o755)
log = open(base/'runtime.log', 'w'); server = subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(method, params=None):
    r = subprocess.run([str(cli),'call',method,json.dumps(params or {})],env=env,capture_output=True,text=True,timeout=30)
    if r.returncode: raise RuntimeError(r.stderr)
    return json.loads(r.stdout)
def wait(predicate, label):
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        try:
            snapshot=call('app.snapshot')
            if predicate(snapshot): return snapshot
        except RuntimeError: pass
        time.sleep(.15)
    raise RuntimeError(label+' timed out: '+str(base))
def attempt(snapshot, card):
    return next(a for a in reversed(snapshot['attempts']) if a['card_id']==card['id'] and a.get('provider_session_id'))
try:
    wait(lambda _:True,'AppServer')
    config=json.loads((home/'config/agents.json').read_text());config.update(hc=str(wrapper),hc_env=core_env,hc_session=core_env['HC_SESSION'])
    for p in config['profiles']:
        p.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
    (home/'config/agents.json').write_text(json.dumps(config,indent=2));call('config.reload')
    call('scheduler.configure',{'max_agents':2,'idle_minutes':0})
    workflow=json.loads((home/'config/workflows/default.json').read_text());workflow['workspace'].update(create_worktree=False,sync_base=False)
    projects=[call('project.create',{'name':name,'directory':str(base/name.lower()),'workflow':workflow}) for name in ['Alpha','Beta']]
    for project in projects:
        subprocess.run(['git','-C',project['directory'],'init','-b','main'],check=True,capture_output=True)
    def card(project,title):return call('card.create',{'project_id':project['id'],'title':title,'body':'Disposable native Terminal workspace acceptance. Keep this Card context isolated.'})
    history=card(projects[0],'Closed discussion');call('agent.open',{'card_id':history['id'],'profile_id':'codex'})
    s=wait(lambda s:any(a['card_id']==history['id'] and a.get('provider_session_id') for a in s['attempts']),'closed conversation initialization');old=attempt(s,history)
    subprocess.run(['hc','send',old['terminal'],'/exit','--json','--force'],env={**os.environ,**core_env},capture_output=True,check=True,timeout=20)
    wait(lambda s:any(a['id']==old['id'] and (a.get('observed') or {}).get('state')=='finished' for a in s['attempts']),'normal history exit')
    live=[]
    for p in projects:
        c=card(p,p['name']+' working task');live.append(c)
        call('card.move',{'card_id':c['id'],'revision':c['revision'],'stage':'develop'})
    wait(lambda s:sum(bool(a.get('provider_session_id')) and (a.get('observed') or {}).get('state')!='finished' for a in s['attempts'])==2,'two live PTYs')
    queued=card(projects[1],'Queued discussion');call('agent.open',{'card_id':queued['id'],'profile_id':'reviewer'})
    s=wait(lambda s:any(r['card_id']==queued['id'] and r['state']=='queued' for r in s['requests']),'queued conversation')
    note=call('note.save',{'project_id':projects[0]['id'],'title':'Terminal decisions.md','body':'# Terminal workspace\n\nRetain this Note while navigating.'})
    metadata={'base':str(base),'home':str(home),'core_env':core_env,'app_id':app_id,'bundle':str(bundle),'projects':projects,'live_cards':live,'closed_card':history,'closed_attempt':old,'queued_card':queued,'note':note}
    (base/'before.json').write_text(json.dumps(s,indent=2));(base/'native.json').write_text(json.dumps(metadata,indent=2))
    (repo/'.local/terminal-workspace-acceptance.json').write_text(json.dumps(metadata,indent=2))
    server.terminate();server.wait(timeout=10)
    print('TERMINAL_WORKSPACE_BASE='+str(base)+'\nAPP_ID='+app_id,flush=True)
    while True:
        # A CUA restart probe keeps the same task-owned Core and durable home.
        server=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=env,stdout=log,stderr=log)
        code=server.wait()
        if not (base/'restart').exists():
            if code: raise RuntimeError('Acceptance app exited with status '+str(code))
            break
        (base/'restart').unlink()
        shutil.copy2(repo/'.local/Canban.app/Contents/MacOS/canban-ui',bundle/'Contents/MacOS/canban-ui')
        shutil.copy2(cli,bundle/'Contents/MacOS/canban')
finally:
    try: call('app.shutdown')
    except Exception: pass
    if server.poll() is None:server.terminate();server.wait(timeout=10)
    subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=20)
    log.close()
