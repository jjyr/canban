#!/usr/bin/env python3
"""Real Herdr transport, interactive fixture, durable replies and exact resume."""
from pathlib import Path
import json,os,subprocess,tempfile,time
repo=Path(__file__).resolve().parents[1];base=Path(tempfile.mkdtemp(prefix='cb-interactive-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
env={**os.environ,'CANBAN_HOME':str(home)};core_env={'HC_SESSION':'interactive-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
log=open(base/'server.log','w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(method,p=None):
 r=subprocess.run([str(cli),'call',method,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=90)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def wait(predicate,label,timeout=45):
 deadline=time.monotonic()+timeout
 while time.monotonic()<deadline:
  if server.poll() is not None:raise RuntimeError((base/'server.log').read_text())
  try:s=call('app.snapshot')
  except (RuntimeError,FileNotFoundError):time.sleep(.1);continue
  if predicate(s):return s
  time.sleep(.15)
 raise RuntimeError(f'{label} timed out\n{(base/"server.log").read_text()}\n{json.dumps(s)[:1500]}')
def send(terminal,text):
 r=subprocess.run(['hc','send',terminal,text,'--json'],env={**os.environ,**core_env},capture_output=True,text=True,timeout=30)
 if r.returncode:raise RuntimeError(r.stderr)
try:
 wait(lambda _:True,'startup')
 config=json.loads((home/'config/agents.json').read_text());config['hc_env']=core_env;config['hc_session']=core_env['HC_SESSION']
 for p in config['profiles']:
  p.update(provider='custom',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
 (home/'config/agents.json').write_text(json.dumps(config,indent=2));call('config.reload')
 project=call('project.create',{'name':'Interactive acceptance','directory':str(base/'source')})
 card=call('card.create',{'project_id':project['id'],'title':'Exercise interactive task'})
 call('card.move',{'card_id':card['id'],'revision':1,'stage':'develop'})
 s=wait(lambda s:len(s['attempts'])==1 and s['attempts'][0]['provider_session_id'] and s['attempts'][0]['terminal'],'bound interactive Agent')
 first=s['attempts'][0];sid=first['provider_session_id'];terminal=first['terminal']
 for _ in range(2):
  opened=call('agent.open',{'card_id':card['id'],'profile_id':'developer'});assert opened['request_id']==first['request_id']
 assert len(call('app.snapshot')['requests'])==1
 call('comment.add',{'card_id':card['id'],'body':'@developer explain the current context'})
 s=wait(lambda s:len(s['questions'])==1 and s['questions'][0]['state']=='replied','immediate reply')
 assert len(s['requests'])==1 and s['requests'][0]['state']=='running' and s['cards'][0]['stage']=='develop'
 assert not s['reports'];assert any(e['kind']=='comment.replied' for e in s['events'])
 control={'attempt_id':first['id'],'request_key':'interrupt-once'}
 receipt=call('agent.interrupt',control);assert call('agent.interrupt',control)==receipt
 assert len([e for e in call('app.snapshot')['events'] if e['kind']=='execution.control_requested'])==1
 wait(lambda s:s['attempts'][0]['observed'].get('status',{}).get('state')=='idle','interrupt observation')
 send(terminal,'continue');send(terminal,'finish')
 s=wait(lambda s:len(s['reports'])==2 and all(a['close_state']=='closed' for a in s['attempts']),'Develop/Review normal exit')
 assert s['cards'][0]['stage']=='review';assert len(s['conversations'])==2
 assert len({c['provider_session_id'] for c in s['conversations']})==2
 call('comment.add',{'card_id':card['id'],'body':'@developer recall earlier context'})
 s=wait(lambda s:len(s['reports'])==3 and len(s['attempts'])==3 and s['attempts'][-1]['close_state']=='closed','exact session resume')
 resumed=s['attempts'][-1];assert resumed['provider_session_id']==sid and resumed['terminal']!=terminal
 assert all(q['state']=='replied' for q in s['questions']);assert s['cards'][0]['stage']=='review'
 server.terminate();server.wait(timeout=10);server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
 s=wait(lambda s:len(s['conversations'])==2,'restart bindings')
 current=s['cards'][0];call('card.archive',{'card_id':card['id'],'revision':current['revision']})
 for question in s['questions']:
  assert 'Answer for' in call('resource.read',{'card_id':card['id'],'path':question['reply_path']})['body']
 archived=next(c for c in call('app.snapshot')['cards'] if c['id']==card['id'])
 call('card.restore',{'card_id':card['id'],'revision':archived['revision']})
 (base/'evidence.json').write_text(json.dumps(call('app.snapshot'),indent=2))
 print('INTERACTIVE_ACCEPTANCE_PASSED\nHOME='+str(home)+'\nSID='+sid,flush=True)
finally:
 server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=15);log.close()
