#!/usr/bin/env python3
"""Isolated real Core: retained stage reuse, live followups, relay input/idle reset and Archive cleanup."""
from pathlib import Path
import json,os,socket,struct,subprocess,tempfile,time,sqlite3,statistics
repo=Path(__file__).resolve().parents[1];base=Path(tempfile.mkdtemp(prefix='cb-retained-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},'CANBAN_HOME':str(home)}
core={'HC_SESSION':'default','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'state'),'XDG_CONFIG_HOME':str(base/'config')}
env.update(core)
log=open(base/'server.log','w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log);viewer=None

def call(method,p=None):
 r=subprocess.run([str(cli),"--full",'call',method,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=40)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def agent(s):
 return next(({'id':t['id'],**t['agent']} for t in s['terminals'] if t.get('agent')),None)
def wait(test,label,timeout=45):
 end=time.monotonic()+timeout
 while time.monotonic()<end:
  try:s=call('app.snapshot')
  except RuntimeError:time.sleep(.1);continue
  if test(s):return s
  time.sleep(.15)
 raise RuntimeError(label+' timed out: '+str(base))
def send(terminal,text):
 r=subprocess.run(['hc','send',terminal,text,'--json'],env={**os.environ,**core},capture_output=True,text=True,timeout=20)
 if r.returncode:raise RuntimeError(r.stderr)
def frame(channel,kind,data):viewer.sendall(struct.pack('<IBI',channel,kind,len(data))+data)
def exact(n):
 data=b''
 while len(data)<n:
  chunk=viewer.recv(n-len(data))
  if not chunk:raise RuntimeError('relay ended')
  data+=chunk
 return data
def read_frame():
 c,k,n=struct.unpack('<IBI',exact(9));return c,k,exact(n)
def output_until(marker):
 output=b''
 while marker not in output:
  c,k,data=read_frame()
  if c==channel and k==0:output+=data;frame(channel,1,struct.pack('<I',len(data)))
 return output
try:
 wait(lambda s:True,'startup')
 config=json.loads((home/'config/agents.json').read_text());config.update(hc_env=core,hc_session=core['HC_SESSION'])
 for p in config['profiles']:
  p.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
 (home/'config/agents.json').write_text(json.dumps(config));call('config.reload');call('scheduler.configure',{'idle_minutes':1})
 retained_yaml="""schema: 1
id: delivery:retained
name: Development
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory.
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: agent:review
  - id: agent:review
    uses: agent
    with:
      profile: developer
      instruction: Review the changes in the same retained session.
    next: __end
  - id: __end
    uses: complete
"""
 project=call('project.create',{'name':'Retained','directory':str(base/'source'),'workflow_yaml':retained_yaml})
 card=call('card.create',{'project_id':project['id'],'title':'Retained workflow'})
 call('card.move',{'card_id':card['id'],'revision':card['revision'],'stage':'develop'})
 s=wait(lambda s:agent(s) and agent(s).get('provider_session_id'),'Agent bound');first=agent(s);terminal=first['terminal'];sid=first['provider_session_id']
 send(terminal,'finish')
 s=wait(lambda s:len(s['reports'])==2 and agent(s)['number']==2 and agent(s)['state']=='completed','same-profile Review in retained PTY')
 current=agent(s);assert current['terminal']==terminal and current['provider_session_id']==sid
 assert any(e['kind']=='execution.continued' for e in s['events']) and current['state']=='completed'
 call('comment.add',{'card_id':card['id'],'body':'Followup after completion','mentions':['developer']})
 s=wait(lambda s:s['questions'] and all(q['state']=='replied' for q in s['questions']),'completed-session reply')
 assert len(s['reports'])==2 and agent(s)['number']==2 and s['cards'][0]['stage']=='review'
 print('RETAINED_WORKFLOW_AND_FOLLOWUP_OK',flush=True)
 viewer=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);viewer.settimeout(15);viewer.connect(str(home/'app.sock'))
 viewer.sendall((json.dumps({'jsonrpc':'2.0','id':0,'method':'herdr.connect','params':{'terminal':terminal}})+'\n').encode())
 line=b''
 while not line.endswith(b'\n'):line+=exact(1)
 assert json.loads(line)['result']['ready']
 viewer.sendall((json.dumps({'jsonrpc':'2.0','id':1,'method':'hello','params':{'generation':1,'framed':True,'features':['automatic_input']}})+'\n').encode())
 line=b''
 while not line.endswith(b'\n'):line+=exact(1)
 assert 'result' in json.loads(line)
 frame(0,0,json.dumps({'jsonrpc':'2.0','id':2,'method':'terminal.attach','params':{'terminal':terminal,'cols':100,'rows':30}}).encode())
 while True:
  c,k,data=read_frame()
  if c==0 and k==0 and json.loads(data).get('id')==2:channel=json.loads(data)['result']['channel'];break
 samples=[]
 for i in range(20):
  marker=('LATENCY_'+str(i)).encode();start=time.monotonic();frame(channel,0,marker+b'\r');output_until(b'INPUT '+marker);samples.append((time.monotonic()-start)*1000)
 frame(channel,0,b'idle\r');output_until(b'IDLE_READY')
 idle_started=time.monotonic()
 s=wait(lambda s:agent(s)['lifecycle'].get('last_interaction_at') and agent(s)['lifecycle'].get('idle_since'),'human activity observed')
 old=round(time.time()*1000)-120000
 db=sqlite3.connect(home/'canban.sqlite');row=json.loads(db.execute('select data from terminals where id=?',(current['id'],)).fetchone()[0]);row['agent']['lifecycle']['idle_since']=old;row['agent']['lifecycle']['last_interaction_at']=old;row['agent']['completed_at']=old;db.execute('update terminals set data=? where id=?',(json.dumps(row),current['id']));db.commit();db.close()
 time.sleep(1.2);s=call('app.snapshot');assert agent(s)['observed']['state']=='live'
 print('HUMAN_INPUT_RESETS_IDLE_OK; RELAY_ROUNDTRIP_MS median=%.2f p95=%.2f'%(statistics.median(samples),sorted(samples)[18]),flush=True)
 # Resume working before the 30s completed-idle window expires, then re-enter idle; reclamation must skip cleanly and run on the next idle window.
 frame(channel,0,b'working\r');output_until(b'WORKING_READY')
 wait(lambda s:(agent(s).get('observed') or {}).get('status',{}).get('state')=='working' and agent(s)['lifecycle'].get('idle_since') is None,'working skips completed idle reclamation')
 frame(channel,0,b'idle\r');output_until(b'IDLE_READY')
 idle_started=time.monotonic()
 print('WAITING_FOR_30S_COMPLETED_IDLE_EXIT',flush=True)
 s=wait(lambda s:(agent(s).get('observed') or {}).get('state')=='finished','completed 30s idle exit while viewer open',timeout=45)
 elapsed=time.monotonic()-idle_started
 assert 28<=elapsed<50,f'Expected ~30s completed idle reclamation (with idle_minutes=1), got {elapsed:.2f}s'
 assert agent(s)['state']=='idle_closed';viewer.close();viewer=None
 print('IDLE_EXIT_WITH_OPEN_VIEWER_OK',flush=True)
 opened=call('agent.open',{'card_id':card['id'],'profile_id':'developer','expected_terminal_id':current['id']})
 s=wait(lambda s:agent(s)['number']==3 and (agent(s).get('observed') or {}).get('state')=='live','explicit exact Resume')
 assert agent(s)['provider_session_id']==sid
 live=agent(s);card=s['cards'][0];call('card.archive',{'card_id':card['id'],'revision':card['revision']})
 s=call('app.snapshot');assert agent(s)['observed']['state']=='finished' and agent(s)['lifecycle']['close_reason']=='archive'
 assert call('resource.read',{'card_id':card['id'],'path':s['questions'][0]['reply_path']})['body']
 (base/'evidence.json').write_text(json.dumps({'snapshot':s,'relay_roundtrip_ms':samples},indent=2))
 print('ARCHIVE_CLOSE_AND_HISTORY_OK\nRETAINED_ACCEPTANCE_PASSED\nBASE='+str(base),flush=True)
finally:
 if viewer:viewer.close()
 if server.poll() is None:server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**os.environ,**core},capture_output=True,timeout=20);log.close()
