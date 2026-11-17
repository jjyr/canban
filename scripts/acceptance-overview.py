#!/usr/bin/env python3
"""Isolated real Herdr PTYs: status budget, global Overview and resumable idle exit."""
from pathlib import Path
import json, os, subprocess, tempfile, time
repo=Path(__file__).resolve().parents[1]
base=Path(tempfile.mkdtemp(prefix='cb-overview-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},'CANBAN_HOME':str(home)}
core_env={'HC_SESSION':'default','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
env.update(core_env)
log=open(base/'server.log','w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(m,p=None):
 r=subprocess.run([str(cli),"--full",'call',m,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=90)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def wait(f,label,timeout=60):
 deadline=time.monotonic()+timeout
 while time.monotonic()<deadline:
  try:s=call('app.snapshot')
  except RuntimeError:time.sleep(.15);continue
  if f(s):return s
  time.sleep(.15)
 raise RuntimeError(label+' timed out '+str(base))
def bound(s):return [{'id':t['id'],**t['agent']} for t in s['terminals'] if (t.get('agent') or {}).get('provider_session_id')]
def run(s,c):return next(a for a in reversed(bound(s)) if a['card_id']==c)
def send(a,text,force=False):
 p=subprocess.run(['hc','send',a['terminal'],text,'--json',*(['--force'] if force else [])],env={**os.environ,**core_env},capture_output=True,text=True,timeout=20)
 if p.returncode:raise RuntimeError(p.stderr)
def card(project,title):return call('card.create',{'project_id':project['id'],'title':title})
def move(c):call('card.move',{'card_id':c['id'],'revision':c['revision'],'stage':'develop'})
def rows():return call('overview.list')
try:
 wait(lambda _:True,'server')
 config=json.loads((home/'config/agents.json').read_text());config.update(hc_env=core_env,hc_session=core_env['HC_SESSION'])
 for p in config['profiles']:
  p.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
 (home/'config/agents.json').write_text(json.dumps(config));call('config.reload')
 assert call('scheduler.status')['max_agents']==8
 call('scheduler.configure',{'max_agents':2,'idle_minutes':1})
 workspace={'create_worktree':False,'sync_base':False}
 projects=[call('project.create',{'name':'Overview '+str(i),'directory':str(base/('project-'+str(i))),'workspace':workspace}) for i in range(2)]
 cards=[card(projects[i%2],'Working '+str(i)) for i in range(4)]
 for c in cards:move(c)
 s=wait(lambda s:len(bound(s))==2,'two running')
 first=run(s,cards[0]['id']);second=run(s,cards[1]['id'])
 assert call('scheduler.status')['active']==2
 send(first,'permission')
 s=wait(lambda s:len(bound(s))==3 and run(s,cards[0]['id']).get('lifecycle',{}).get('waiting'),'permission releases slot')
 assert not next(c for c in s['cards'] if c['id']==cards[0]['id'])['human_gate']
 reasons=next(r for r in rows() if r['card']['id']==cards[0]['id'])['reasons'];assert [r['source'] for r in reasons]==['runtime']
 assert len({r['project_name'] for r in rows()})==2
 before=len(s['notices']);send(first,'permission-update',True)
 s=wait(lambda s:any((a.get('lifecycle',{}).get('waiting') or {}).get('message')=='Allow git diff?' for a in bound(s)),'updated wait context')
 assert len(s['notices'])==before
 preview=call('overview.preview',{'row_id':cards[0]['id']+':developer','terminal_id':first['id']});assert 'RUNTIME_WAIT' in preview['text'] and len(preview['text'].encode())<=32768
 try:call('overview.preview',{'row_id':cards[0]['id']+':developer','terminal_id':'wrong'})
 except RuntimeError:pass
 else:raise AssertionError('stale preview allowed')
 send(first,'working',True)
 s=wait(lambda s:call('scheduler.status')['active']==3,'manual resumption above limit')
 assert len(bound(s))==3 and not any(a['card_id']==cards[3]['id'] for a in bound(s))
 send(second,'done-only')
 s=wait(lambda s:call('scheduler.status')['active']==2,'done releases slot without admitting at limit')
 assert len(bound(s))==3
 send(run(s,cards[2]['id']),'idle')
 s=wait(lambda s:len(bound(s))==4 and any(a['card_id']==cards[3]['id'] for a in bound(s)),'idle releases another slot')
 assert not next(c for c in s['cards'] if c['id']==cards[1]['id'])['human_gate']
 assert next(r for r in rows() if r['card']['id']==cards[1]['id'])['group']=='Idle'
 print('STATUS_BUDGET_AND_GLOBAL_OVERVIEW_OK',flush=True)
 # Runtime input wait and Canban task decision coexist; resolving one never clears the other.
 send(first,'block');s=wait(lambda s:next(c for c in s['cards'] if c['id']==cards[0]['id'])['human_gate'],'task gate')
 send(first,'permission');s=wait(lambda s:run(s,cards[0]['id']).get('lifecycle',{}).get('waiting'),'combined runtime wait')
 row=next(r for r in rows() if r['card']['id']==cards[0]['id']);assert {r['source'] for r in row['reasons']}=={'runtime','task'}
 send(first,'working',True);s=wait(lambda s:run(s,cards[0]['id'])['lifecycle']['waiting'] is None,'runtime resolved');assert next(c for c in s['cards'] if c['id']==cards[0]['id'])['human_gate']
 # Create an interactive conversation, not a new session class. Leave it idle.
 call('scheduler.configure',{'max_agents':8})
 # A permission wait can end in an unreported exit. Keep the task gate, not a phantom Terminal wait.
 exited=card(projects[1],'Exited Agent needs a saved-output recovery path')
 opened_exit=call('agent.open',{'card_id':exited['id'],'profile_id':'codex'})
 s=wait(lambda s:any(a['id']==opened_exit['terminal_id'] for a in bound(s)),'unreported Agent start')
 exit_run=run(s,exited['id']);send(exit_run,'permission')
 wait(lambda s:run(s,exited['id']).get('lifecycle',{}).get('waiting'),'exit fixture permission')
 send(exit_run,'/exit',True)
 s=wait(lambda s:run(s,exited['id'])['state']=='failed' and run(s,exited['id']).get('output_path'),'unreported exit captured')
 exit_run=run(s,exited['id'])
 assert exit_run['observed']['state']=='finished' and exit_run['lifecycle']['waiting'] is None
 exit_row=next(r for r in rows() if r['card']['id']==exited['id'])
 assert exit_row['group']=='Closed' and not exit_row['reasons']
 exit_params={'row_id':exit_row['id'],'terminal_id':exit_run['id']}
 exit_output=call('overview.preview',exit_params)
 assert exit_output['source']=='Saved execution output (read-only)' and exit_output['observed_at'] is None
 assert 'NORMAL_EXIT' in exit_output['text']
 print('EXITED_FAILED_AND_SAVED_OUTPUT_OK',flush=True)
 idle=card(projects[1],'Idle resumable discussion');opened=call('agent.open',{'card_id':idle['id'],'profile_id':'codex'})
 s=wait(lambda s:any(a['id']==opened['terminal_id'] for a in bound(s)),'interactive Agent')
 idle_run=run(s,idle['id']);send(idle_run,'idle');started=time.monotonic()
 done_card=card(projects[1],'Done resumable discussion');done_open=call('agent.open',{'card_id':done_card['id'],'profile_id':'codex'})
 s=wait(lambda s:any(a['id']==done_open['terminal_id'] for a in bound(s)),'done interactive Agent')
 done_run=run(s,done_card['id']);send(done_run,'done-only')
 s=wait(lambda s:run(s,idle['id'])['lifecycle'].get('idle_since'),'idle observed')
 print('WAITING_FOR_ONE_MINUTE_IDLE_RECLAMATION',flush=True)
 s=wait(lambda s:run(s,idle['id'])['state']=='idle_closed','normal idle exit',timeout=75)
 assert time.monotonic()-started>=58
 s=wait(lambda s:run(s,done_card['id'])['state']=='idle_closed','done discussion idle exit',timeout=15)
 assert run(s,idle['id'])['observed']['state']=='finished'
 assert run(s,cards[2]['id'])['observed']['state']=='live'  # Unfinished automatic stage stays protected.
 assert next(c for c in s['cards'] if c['id']==idle['id'])['stage']=='backlog'
 assert not next(c for c in s['cards'] if c['id']==idle['id'])['human_gate']
 assert not any(r['card_id']==idle['id'] for r in s['reports'])
 resumed=call('agent.open',{'card_id':idle['id'],'profile_id':'codex','expected_terminal_id':idle_run['id']})
 s=wait(lambda s:any(a['id']==resumed['terminal_id'] and a['number']==2 and (a.get('observed') or {}).get('state')=='live' for a in bound(s)),'exact resume')
 assert run(s,idle['id'])['provider_session_id']==idle_run['provider_session_id']
 row=next(r for r in rows() if r['card']['id']==idle['id']);assert row['terminal']['agent']['number']==2
 assert len([r for r in rows() if r['card']['id']==idle['id']])==1
 # Destroy only this scratch Core; closed execution context must remain readable.
 subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=20,check=True)
 assert call('overview.preview',exit_params)['text']==exit_output['text']
 print('SAVED_OUTPUT_WITHOUT_ORIGINAL_CORE_OK',flush=True)
 (base/'evidence.json').write_text(json.dumps({'rows':rows(),'snapshot':s,'status':call('scheduler.status'),'preview':preview},indent=2))
 print('IDLE_NORMAL_EXIT_AND_EXACT_RESUME_OK\nOVERVIEW_ACCEPTANCE_PASSED\nBASE='+str(base),flush=True)
finally:
 try: call('app.shutdown')
 except Exception: pass
 if server.poll() is None:server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=20);log.close()
