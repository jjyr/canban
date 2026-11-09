#!/usr/bin/env python3
"""Explicit real-model opt-in: interactive Codex, live @reply, exit and resume."""
from pathlib import Path
import json,os,subprocess,tempfile,time
repo=Path(__file__).resolve().parents[1];base=Path(tempfile.mkdtemp(prefix='cb-real-interactive-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
env={**os.environ,'CANBAN_HOME':str(home)};core_env={'HC_SESSION':'real-interactive','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
log=open(base/'server.log','w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(method,p=None):
 r=subprocess.run([str(cli),'call',method,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=90)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
trusted=set()
def wait(predicate,label,timeout=300):
 end=time.monotonic()+timeout
 while time.monotonic()<end:
  if server.poll() is not None:raise RuntimeError((base/'server.log').read_text())
  try:s=call('app.snapshot')
  except (RuntimeError,FileNotFoundError):time.sleep(.2);continue
  # Only the known disposable repository created by this harness is trusted.
  # Do not answer arbitrary provider approval or permission dialogs.
  for run in s['attempts']:
   terminal=run.get('terminal')
   if not terminal or terminal in trusted:continue
   view=subprocess.run(['hc','read',terminal],env={**os.environ,**core_env},capture_output=True,text=True,timeout=15).stdout
   if 'Trust this folder?' in view and str(base) in view and 'Trust and continue' in view:
    subprocess.run(['hc','keys',terminal,'enter'],env={**os.environ,**core_env},capture_output=True,check=True,timeout=15);trusted.add(terminal)
    print('TRUSTED_DISPOSABLE_REPO '+terminal,flush=True)
  if predicate(s):return s
  time.sleep(.5)
 raise RuntimeError(f'{label} timed out; evidence home {home}\n'+(base/'server.log').read_text()[-3500:])
try:
 wait(lambda _:True,'startup',20)
 config=json.loads((home/'config/agents.json').read_text());config['hc_env']=core_env;config['hc_session']=core_env['HC_SESSION']
 (home/'config/agents.json').write_text(json.dumps(config,indent=2));call('config.reload');call('scheduler.configure',{'idle_minutes':1})
 source=base/'source';source.mkdir();(source/'greeting.py').write_text('def greet(name):\n    return "TODO"\n')
 (source/'test_greeting.py').write_text('import unittest\nfrom greeting import greet\nclass Tests(unittest.TestCase):\n    def test_ascii(self):self.assertEqual(greet("Ada"),"Hello, Ada!")\n    def test_unicode(self):self.assertEqual(greet("世界"),"Hello, 世界!")\n')
 (source/'.gitignore').write_text('__pycache__/\n')
 for args in [['init'],['add','.'],['-c','user.name=Acceptance','-c','user.email=acceptance@example.invalid','-c','core.hooksPath=/dev/null','commit','-m','Seed']]:subprocess.run(['git','-C',str(source),*args],check=True,capture_output=True)
 workflow_path = home / 'config/workflows/default.json'
 local_workflow = json.loads(workflow_path.read_text())
 local_workflow['workspace']['sync_base'] = False  # Fixture repositories have no remote.
 workflow_path.write_text(json.dumps(local_workflow))
 project=call('project.create',{'name':'Real interactive','directory':str(source)})
 marker='MARKER_'+base.name.replace('-','_')
 card=call('card.create',{'project_id':project['id'],'title':'Implement Unicode greeting','body':'Isolation marker: '+marker+'. Implement greet(name) to return exactly "Hello, {name}!" including Unicode. Keep tests unchanged; run /usr/bin/python3 -m unittest -v. For the Develop stage ONLY: bind your session first and report working; read the context, then wait for a Canban question before implementing/reporting completed. Do not use a blocking shell wait; the UI will deliver a follow-up shortly. In Review, inspect changes/tests and report immediately. Use current reporting instructions and do not commit.'})
 print('REAL_INTERACTIVE_HOME='+str(home),flush=True)
 call('card.move',{'card_id':card['id'],'revision':1,'stage':'develop'})
 s=wait(lambda s:bool(s['attempts']) and s['attempts'][0]['provider_session_id'] and s['attempts'][0]['terminal'],'provider session binding')
 first=s['attempts'][0];sid=first['provider_session_id'];print('BOUND '+sid+' '+first['terminal'],flush=True)
 call('comment.add',{'card_id':card['id'],'body':'@developer Reply to this question via the supplied Canban reply command: confirm the exact isolation marker from the Card context. After publishing that reply, implement the original greeting task, run its tests, and report the original task completed. You may proceed now.'})
 s=wait(lambda s:len(s['reports'])>=2 and all(a['close_state']=='closed' for a in s['attempts']),'real Develop/Review and normal exits')
 assert len(s['requests'])==2 and len(s['questions'])==1 and s['questions'][0]['state']=='replied'
 q=s['questions'][0];assert marker in call('resource.read',{'card_id':card['id'],'path':q['reply_path']})['body']
 assert s['cards'][0]['stage']=='review';assert len({c['provider_session_id'] for c in s['conversations']})==2
 cwd=Path(first['cwd']);tests=subprocess.run(['/usr/bin/python3','-m','unittest','-v'],cwd=cwd,text=True,capture_output=True,check=True)
 assert (source/'greeting.py').read_text()=='def greet(name):\n    return "TODO"\n'
 call('comment.add',{'card_id':card['id'],'body':'@developer Follow-up only: without changing files, recall the exact isolation marker you saw earlier. Publish a reply using this question ID and then report this follow-up completed. Do not repeat development or change the Card stage.'})
 s=wait(lambda s:len(s['reports'])>=3 and len(s['attempts'])==3 and s['attempts'][-1]['close_state']=='closed','real exact-ID resume')
 last=s['attempts'][-1];assert last['provider_session_id']==sid and last['terminal']!=first['terminal']
 assert s['cards'][0]['stage']=='review';assert all(q['state']=='replied' for q in s['questions'])
 q=s['questions'][-1];assert marker in call('resource.read',{'card_id':card['id'],'path':q['reply_path']})['body']
 current=s['cards'][0];call('card.archive',{'card_id':card['id'],'revision':current['revision']})
 for q in s['questions']:assert marker in call('resource.read',{'card_id':card['id'],'path':q['reply_path']})['body']
 (base/'evidence.json').write_text(json.dumps({'snapshot':call('app.snapshot'),'tests':tests.stdout+tests.stderr,'session':sid},indent=2))
 print('REAL_INTERACTIVE_ACCEPTANCE_PASSED\nHOME='+str(home),flush=True)
finally:
 server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=15);log.close()
