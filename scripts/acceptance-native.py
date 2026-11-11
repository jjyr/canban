#!/usr/bin/env python3
"""Prepare an isolated native UI for manual/CUA acceptance; never uses ~/.canban.
The process stays running with the app, and stops only its own scratch Core.
"""
from pathlib import Path
import json, os, plistlib, shutil, subprocess, tempfile, time
repo=Path(__file__).resolve().parents[1]
base=Path(tempfile.mkdtemp(prefix='cb-native-',dir='/private/tmp'))
home=base/'home';cli=repo/'target/debug/canban'
app_id='com.canban.acceptance.'+base.name
bundle=base/'CanbanAcceptance.app';shutil.copytree(repo/'.local/Canban.app',bundle)
plist=bundle/'Contents/Info.plist'
info=plistlib.loads(plist.read_bytes());info.update(CFBundleIdentifier=app_id,CFBundleDisplayName='Canban Acceptance',CFBundleName='Canban Acceptance');plist.write_bytes(plistlib.dumps(info))
env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},'CANBAN_HOME':str(home),'CANBAN_APP_ID':app_id}
core_env={'HC_SESSION':'native-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
# This wrapper simulates temporary Core unavailability without stopping a real
# terminal or changing any user Core. Removing the marker restores the same ID.
wrapper=base/'hc-wrapper'
wrapper.write_text('#!/bin/sh\nif [ -e "'+str(base/'offline')+'" ]; then echo "Scratch core temporarily unavailable" >&2; exit 1; fi\nexec '+shutil.which('hc')+' "$@"\n');wrapper.chmod(0o755)
log=open(base/'server.log','w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(method,p=None):
 r=subprocess.run([str(cli),'call',method,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=90)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
try:
 for _ in range(100):
  try:call('app.ping');break
  except RuntimeError:time.sleep(.1)
 config=json.loads((home/'config/agents.json').read_text());config.update(hc=str(wrapper),hc_env=core_env,hc_session=core_env['HC_SESSION'])
 for p in config['profiles']:
  p.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
 (home/'config/agents.json').write_text(json.dumps(config,indent=2));call('config.reload')
 source=base/'source';source.mkdir();(source/'hello.txt').write_text('original\n')
 for args in [['init','-b','main'],['config','user.name','Acceptance'],['config','user.email','acceptance@example.invalid'],['config','core.hooksPath','/dev/null'],['config','commit.gpgsign','false'],['add','.'],['commit','-m','Seed']]:subprocess.run(['git','-C',str(source),*args],check=True,capture_output=True)
 local_workflow = call('workflow.template')
 local_workflow['workspace']['sync_base'] = False  # Fixture repositories have no remote.
 project=call('project.create',{'workflow':local_workflow,'name':'Native acceptance','directory':str(source)})
 card=call('card.create',{'project_id':project['id'],'title':'Terminal · 中文与 recovery','body':'Disposable native terminal acceptance. Type text; finish advances into Review.'})
 for i in range(12):call('card.create',{'project_id':project['id'],'title':'Planned work '+str(i+1),'body':'Board scroll retention fixture.'})
 call('card.move',{'card_id':card['id'],'revision':card['revision'],'stage':'develop'})
 for _ in range(100):
  s=call('app.snapshot')
  if s['attempts'] and s['attempts'][0].get('provider_session_id'):break
  time.sleep(.1)
 server.terminate();server.wait(timeout=10)
 metadata={'base':str(base),'home':str(home),'core_env':core_env,'app_id':app_id,'bundle':str(bundle),'card_id':card['id'],'project_id':project['id'],'first':s['attempts'][0]}
 (base/'native.json').write_text(json.dumps(metadata,indent=2));(repo/'.local/native-acceptance.json').write_text(json.dumps(metadata,indent=2))
 print('NATIVE_ACCEPTANCE_BASE='+str(base),flush=True)
 while True:
  server=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=env,stdout=log,stderr=log)
  metadata['ui_pid']=server.pid;(base/'native.json').write_text(json.dumps(metadata,indent=2))
  server.wait()
  if not (base/'restart').exists():break
  (base/'restart').unlink()
  # Atomic binary replacement also permits verifying a just-built UI fix.
  for name in ('canban-ui','canban'):
   staged=bundle/'Contents/MacOS'/('.next-'+name)
   shutil.copy2(repo/'.local/Canban.app/Contents/MacOS'/name,staged)
   os.replace(staged,bundle/'Contents/MacOS'/name)

finally:
 try: call('app.shutdown')
 except Exception: pass
 if server.poll() is None:server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=15);log.close()
