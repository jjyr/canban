#!/usr/bin/env python3
"""Real Herdr transport, interactive fixture, durable replies and exact resume."""
from pathlib import Path
import json,os,subprocess,tempfile,time
repo=Path(__file__).resolve().parents[1];base=Path(tempfile.mkdtemp(prefix='cb-interactive-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},'CANBAN_HOME':str(home)};core_env={'HC_SESSION':'interactive-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
env.update(core_env)
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
  p.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
 (home/'config/agents.json').write_text(json.dumps(config,indent=2));call('config.reload')
 workflow_path = home / 'config/workflows/default.json'
 local_workflow = json.loads(workflow_path.read_text())
 local_workflow['workspace']['sync_base'] = False  # Fixture repositories have no remote.
 workflow_path.write_text(json.dumps(local_workflow))
 project=call('project.create',{'name':'Interactive acceptance','directory':str(base/'source')})
 note=call('note.save',{'project_id':project['id'],'title':'Context.md','body':'Original captured context'})
 ref=call('note.save',{'project_id':project['id'],'title':'Reference.md','body':'[Context](note:'+note['id']+')'})
 card=call('card.create',{'project_id':project['id'],'title':'Exercise interactive task','source_note_id':note['id']})
 call('card.move',{'card_id':card['id'],'revision':1,'stage':'develop'})
 s=wait(lambda s:len(s['attempts'])==1 and s['attempts'][0]['provider_session_id'] and s['attempts'][0]['terminal'],'bound interactive Agent')
 first=s['attempts'][0];sid=first['provider_session_id'];terminal=first['terminal']
 # Agent writes through the real CLI with its injected execution binding.
 send(terminal,'note-write '+note['id']+' Agent-written database Note')
 s=wait(lambda s:any(n['id']==note['id'] and n['body']=='Agent-written database Note' for n in s['notes']),'Agent Note CLI write')
 event=next(e for e in s['note_events'] if e['note_id']==note['id'] and e['kind']=='note.updated')
 assert event['actor']=='@developer' and event['card_id']==card['id'] and event['request_id']==first['request_id'] and event['attempt']==1
 raw=subprocess.run([str(cli),'note','read',note['id']],env=env,capture_output=True,text=True,check=True);assert raw.stdout=='Agent-written database Note'
 stale=subprocess.run([str(cli),'note','write',note['id'],'--revision','1'],input='Stale human overwrite',env=env,capture_output=True,text=True);assert stale.returncode!=0
 renamed=subprocess.run([str(cli),'note','rename',note['id'],'Renamed.md','--revision','2'],env=env,capture_output=True,text=True,check=True);assert json.loads(renamed.stdout)['id']==note['id']
 assert call('note.get',{'note_id':ref['id']})['links']==[note['id']]
 historical=call('note.revision',{'note_id':note['id'],'revision':1});assert historical['body']=='Original captured context'
 assert 'Original captured context' in (home/first['input_path']).with_name('context.json').read_text()
 # Restart the host while the provider is live; reconnect exact binding.
 server.terminate();server.wait(timeout=10);server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
 s=wait(lambda s:s['attempts'][0].get('connection_state')=='connected','live host restart')
 assert len(s['attempts'])==1 and s['attempts'][0]['terminal']==terminal
 # Temporary transport loss is visible, deduplicated, and never relaunches.
 config['hc']='/usr/bin/false';(home/'config/agents.json').write_text(json.dumps(config));call('config.reload')
 s=wait(lambda s:s['attempts'][0].get('connection_state')=='unavailable','offline attention')
 notices=len(s['notices']);time.sleep(1.2);assert len(call('app.snapshot')['notices'])==notices
 config['hc']='hc';(home/'config/agents.json').write_text(json.dumps(config));call('config.reload')
 s=wait(lambda s:s['attempts'][0].get('connection_state')=='connected','original core reconnect')
 assert len(s['attempts'])==1 and s['attempts'][0]['terminal']==terminal and s['attempts'][0]['provider_session_id']==sid
 # Transport recovery preserves the human gate until explicitly reviewed.
 assert s['cards'][0]['human_gate']
 call('card.continue',{'card_id':card['id'],'revision':s['cards'][0]['revision']})

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
 s=wait(lambda s:len(s['reports'])==2 and all(((a.get('observed') or {}).get('status') or {}).get('state')=='done' for a in s['attempts']),'Develop/Review retained terminals')
 assert s['cards'][0]['stage']=='review';assert len(s['conversations'])==2
 assert len({c['provider_session_id'] for c in s['conversations']})==2
 call('comment.add',{'card_id':card['id'],'body':'@developer recall earlier context'})
 s=wait(lambda s:all(q['state']=='replied' for q in s['questions']) and len(s['questions'])==2,'live completed-session followup')
 assert len(s['attempts'])==2 and len(s['reports'])==2
 call('terminal.close',{'terminal_id':card['id']+':developer'})
 wait(lambda s:s['attempts'][0]['observed']['state']=='finished','explicit close before Resume')
 call('comment.add',{'card_id':card['id'],'body':'@developer resume and recall'})
 s=wait(lambda s:len(s['attempts'])==3 and len(s['reports'])==3 and all(q['state']=='replied' for q in s['questions']),'exact session Resume')
 resumed=s['attempts'][-1];assert resumed['provider_session_id']==sid and resumed['terminal']!=terminal
 assert s['cards'][0]['stage']=='review'
 call('terminal.close',{'terminal_id':card['id']+':developer'})
 wait(lambda s:s['attempts'][-1]['observed']['state']=='finished','close resumed terminal')
 server.terminate();server.wait(timeout=10);server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
 s=wait(lambda s:len(s['conversations'])==2,'restart bindings')
 # A provider must reject a missing saved conversation, never silently create
 # unrelated history. Recovery is an explicit fresh-context action.
 (home/'fixture-history'/f'{sid}.json').unlink()
 call('agent.open',{'card_id':card['id'],'profile_id':'developer'})
 s=wait(lambda s:len(s['attempts'])==4 and s['attempts'][-1]['state']=='needs_report','missing provider history')
 assert s['cards'][0]['attention'] and s['attempts'][-1]['provider_session_id']==sid
 call('agent.open',{'card_id':card['id'],'profile_id':'developer','fresh':True})
 s=wait(lambda s:len(s['attempts'])==5 and s['attempts'][-1]['provider_session_id'] not in (None,sid),'explicit fresh context')
 send(s['attempts'][-1]['terminal'],'finish')
 s=wait(lambda s:s['attempts'][-1]['state']=='completed','fresh context completion retained')
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
