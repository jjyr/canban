#!/usr/bin/env python3
"""Real isolated Core multi-machine acceptance; no model credentials required.
Run against a disposable, already connected AppServer home and remote Core.
Never point --home at a user's live Canban database. The selected fixture root
on the remote host is created by this test. No host-specific connection details
are embedded here; keep those in the Machines inventory.
"""
import argparse,base64,json,os,pathlib,subprocess,time,uuid,hashlib
p=argparse.ArgumentParser();p.add_argument('--home',type=pathlib.Path,required=True);p.add_argument('--machine',required=True);p.add_argument('--fixture-root',required=True);a=p.parse_args()
repo=pathlib.Path(__file__).resolve().parents[1];cli=repo/'target/debug/canban';env={k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')};env['CANBAN_HOME']=str(a.home)
config=json.loads((a.home/'config/agents.json').read_text());core_env={**os.environ,**config['hc_env'],'HC_SESSION':config['hc_session']}
def call(m,p=None):
 r=subprocess.run([str(cli),'call',m,json.dumps(p or {})],env=env,text=True,capture_output=True,timeout=95)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def hc(m,p=None):
 proc=subprocess.Popen([config['hc'],'connect'],env=core_env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 try:
  proc.stdin.write(json.dumps({'jsonrpc':'2.0','id':1,'method':'hello','params':{'generation':1}})+'\n'+json.dumps({'jsonrpc':'2.0','id':2,'method':m,'params':p or {}})+'\n');proc.stdin.flush()
  while True:
   line=proc.stdout.readline()
   if not line:raise RuntimeError('Core disconnected: '+proc.stderr.read())
   v=json.loads(line)
   if v.get('id')==2:
    if 'error'in v:raise RuntimeError(str(v['error']))
    return v['result']
 finally:proc.terminate();proc.wait(timeout=10)
tracked=[]
def wait(pred,label,timeout=150):
 end=time.monotonic()+timeout
 while time.monotonic()<end:
  s=call('app.snapshot')
  if pred(s):return s
  bad=[r for r in s['requests'] if r['state'] in ['failed','unknown'] and r['card_id'] in tracked]
  if bad:raise RuntimeError(label+': failed execution '+str(bad)+'\n'+str(s['cards']))
  time.sleep(.3)
 raise RuntimeError('Timed out: '+label+'\n'+json.dumps(s,indent=2))
def run(script,*args):
 v=hc('machine.exec',{'on':machine,'argv':['python3','-c',script,*args],'cwd':'/','timeout':60})
 assert v['code']==0 and not v['timed_out'] and not v['truncated'],v
 return base64.b64decode(v['stdout']).decode()
def send(terminal,text):return hc('terminal.send',{'terminal':terminal,'text':text,'force':True})
def current(s,card):return next(c for c in s['cards'] if c['id']==card['id'])
def latest(s,card,stage=None):
 ids=[r['id'] for r in s['requests'] if r['card_id']==card['id'] and (stage is None or r['stage']==stage)]
 return next(x for x in reversed(s['attempts']) if x['request_id']in ids and x.get('terminal') and x.get('provider_session_id'))
for _ in range(100):
 try:call('app.ping');break
 except RuntimeError:time.sleep(.1)
ms=call('machine.list')['machines'];machine=next(m['id'] for m in ms if a.machine in [m['id'],m['name']]);root=a.fixture_root.rstrip('/')+'/'+uuid.uuid4().hex[:8]
fixture=root+'/fixture.py';hc('file.write',{'on':machine,'path':fixture,'data':base64.b64encode((repo/'scripts/interactive-fixture.py').read_bytes()).decode()})
local_fixture=repo/'scripts/interactive-fixture.py'
# A shared command selects the staged fixture on the remote host and the local
# fixture on this host without provider-specific Rust dispatch code.
selector='import os,runpy; runpy.run_path(os.environ.get("CANBAN_TEST_FIXTURE",'+repr(fixture)+'),run_name="__main__")'
for prof in config['profiles']:
 prof.update(preset=None,session_instruction='',exit={'type':'input','text':'/exit'},argv=['python3','-c',selector,'{prompt}'],resume_argv=['python3','-c',selector,'--resume','{session_id}','{prompt}'])
(a.home/'config/agents.json').write_text(json.dumps(config));call('config.reload');call('scheduler.configure',{'idle_minutes':0})
source=root+'/source'
run("import os,subprocess,sys;os.makedirs(sys.argv[1]);p=sys.argv[1];\nfor args in [['init','-b','main'],['config','user.name','Acceptance'],['config','user.email','acceptance@example.invalid'],['config','core.hooksPath','/dev/null'],['config','commit.gpgsign','false'],['commit','--allow-empty','-m','seed']]:subprocess.run(['git','-C',p]+args,check=True,stdout=subprocess.DEVNULL)\nprint('ready')",source)
remote_yaml="""schema: 1
id: delivery:remote
name: Development
workspace:
  create_worktree: true
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
      profile: reviewer
      instruction: Review the work for this card.
    next: approval:review
  - id: approval:review
    uses: approval
    with:
      title: Agent report ready; human review is required before continuing
    next: __end
  - id: __end
    uses: complete
"""
project=call('project.create',{'name':'Remote acceptance','location':{'machine':machine,'directory':source},'workflow_yaml':remote_yaml})
card=call('card.create',{'project_id':project['id'],'title':'Remote develop, review and resume','body':'Disposable multi-machine acceptance.'})
tracked.append(card['id'])
note=call('note.save',{'project_id':project['id'],'title':'Remote context.md','body':'# Remote context'})
shell=call('terminal.create',{'machine':machine});assert shell['runtime']['machine']==machine
shell_binding=shell['runtime']['terminal'];assert shell_binding.startswith(machine+'/')
send(shell_binding,'cd '+source)
s=wait(lambda s:any(c['terminal_id']==shell['id'] and c.get('project_id')==project['id'] for c in s['terminal_contexts']),'machine-aware manual directory association')
copy=call('terminal.create',{'inherit_terminal_id':shell['id']});assert copy['runtime']['machine']==machine and copy['runtime']['terminal'] and copy['runtime']['state']=='running'
print('REMOTE_SHELL_INHERIT_OK',flush=True)
call('card.move',{'card_id':card['id'],'revision':card['revision'],'stage':'develop'})
s=wait(lambda s:any(x['card_id']==card['id'] and x.get('provider_session_id') for x in s['attempts']),'remote developer bind');dev=latest(s,card,'develop')
assert dev['location']==project['location'] and dev['cwd']!=source and dev['terminal'].startswith(machine+'/')
assert dev['owner_core_id']==machine and dev['workspace']['branch']
# Runtime status and CLI-reported task blocking stay separate.
send(dev['terminal'],'permission');wait(lambda s:any(x['id']==dev['id'] and ((x.get('observed') or {}).get('status') or {}).get('state')=='blocked' for x in s['attempts']),'remote permission wait')
send(dev['terminal'],'working');send(dev['terminal'],'block');s=wait(lambda s:current(s,card).get('human_gate'),'remote reported blocked');assert current(s,card)['attention']
send(dev['terminal'],'note-write '+note['id']+' Updated through remote CLI')
wait(lambda s:any(n['id']==note['id'] and n['body']=='Updated through remote CLI' for n in s['notes']),'remote Note write')
run("import pathlib,sys;pathlib.Path(sys.argv[1],'delivery.txt').write_text('remote deliverable\\n')",dev['cwd'])
# Save a different future location while this invocation is still running.
future=root+'/future';run("import shutil,sys;shutil.copytree(sys.argv[1],sys.argv[2])",source,future)
changed=call('project.location',{'project_id':project['id'],'expected_location':project['location'],'location':{'machine':machine,'directory':future}})
assert latest(call('app.snapshot'),card)['location']['directory']==source
# Explicit resume uses original environment; ordinary review requires a handoff.
# Return Project before completion so the first full code delivery remains simple.
call('project.location',{'project_id':project['id'],'expected_location':changed['location'],'location':project['location']})
send(dev['terminal'],'finish')
s=wait(lambda s:any(r['id']==dev['request_id'] and r['state']=='completed' for r in s['requests']),'blocked work completion')
assert current(s,card)['stage']=='develop' and current(s,card)['human_gate']
call('card.continue',{'card_id':card['id'],'revision':current(s,card)['revision']})
s=wait(lambda s:current(s,card).get('human_gate') and any(r['card_id']==card['id'] and r['stage']=='review' and r['state']=='completed' for r in s['requests']),'remote review report',210)
s=wait(lambda s:all((x.get('observed') or {}).get('state')=='finished' for x in s['attempts'] if x['card_id']==card['id']),'normal exits',100)
plan=call('review.preview',{'card_id':card['id']});assert plan['merge_allowed'] and 'remote deliverable' in plan['patch']
# Review continues using original source even after Project changes machines.
local_source=a.home.parent/('alternate-local-'+card['id']);local_source.mkdir()
for args in [['init','-b','main'],['-c','user.name=Acceptance','-c','user.email=acceptance@example.invalid','commit','--allow-empty','-m','seed']]:subprocess.run(['git','-C',str(local_source),*args],check=True,capture_output=True)
changed=call('project.location',{'project_id':project['id'],'location':{'machine':'local','directory':str(local_source)}})
plan2=call('review.preview',{'card_id':card['id']});assert plan2['worktree']==dev['cwd'] and plan2['patch']==plan['patch']
merged=call('review.merge',{'card_id':card['id'],'revision':current(s,card)['revision'],'token':plan2['token'],'request_key':'remote-merge-'+uuid.uuid4().hex});assert merged['state']=='merged',merged
assert run("import pathlib,sys;print(pathlib.Path(sys.argv[1],'delivery.txt').read_text())",source).strip()=='remote deliverable'
assert not (local_source/'delivery.txt').exists()
s=call('app.snapshot');assert current(s,card)['archived_at']
call('card.restore',{'card_id':card['id'],'revision':current(s,card)['revision']})
print('REMOTE_WORKFLOW_BLOCK_NOTE_REVIEW_MERGE_OK',flush=True)
opened=call('agent.open',{'card_id':card['id'],'profile_id':'developer','expected_attempt_id':dev['id']})
s=wait(lambda s:any(x['request_id']==opened['request_id'] and x.get('terminal') and x.get('provider_session_id') for x in s['attempts']),'exact remote resume');resumed=next(x for x in s['attempts'] if x['request_id']==opened['request_id'])
assert resumed['location']==dev['location'] and resumed['cwd']==dev['cwd'] and resumed['provider_session_id']==dev['provider_session_id']
send(resumed['terminal'],'hello after resume')
# Route rename must not change stored qualified owner address.
hc('machine.rename',{'machine':next(m['name'] for m in hc('machine.list')['machines'] if m['id']==machine),'new_name':'renamed-'+uuid.uuid4().hex[:6]})
send(resumed['terminal'],'hello after rename')
assert hc('terminal.get',{'terminal':resumed['terminal']})['state']!='finished'
print('REMOTE_ORIGINAL_ENV_RESUME_AND_ROUTE_RENAME_OK',flush=True)
# An AppServer restart must preserve the same remote PTY and queued CLI call.
call('app.shutdown');time.sleep(.5)
send(resumed['terminal'],'note-write '+note['id']+' Updated across AppServer restart')
restart_log=open(a.home.parent/'restart-server.log','a')
restarted_server=subprocess.Popen([str(cli),'serve'],env=env,stdout=restart_log,stderr=restart_log,start_new_session=True)
for _ in range(100):
 try:call('app.ping');break
 except RuntimeError:time.sleep(.1)
s=wait(lambda s:any(n['id']==note['id'] and n['body']=='Updated across AppServer restart' for n in s['notes']),'remote report bridge after server restart')
assert next(x for x in s['attempts'] if x['id']==resumed['id'])['terminal']==resumed['terminal']
print('REMOTE_PTY_AND_CLI_RESTART_OK',flush=True)
# Disconnecting the route must not end its remote programs. Reconnect the same
# isolated destination Core under another alias and use its saved owner address.
connection=next(m for m in call('machine.list')['machines'] if m['id']==machine)
call('machine.remove',{'machine':machine,'confirmed':True})
params={'name':'reconnected-'+uuid.uuid4().hex[:6],'session':connection.get('session')}
params['via' if connection.get('via') else 'target']=connection['target']
restored=hc('machine.add',params);assert restored['id']==machine
for _ in range(100):
 if any(m['id']==machine and m['state']=='connected' for m in call('machine.list')['machines']):break
 time.sleep(.2)
send(resumed['terminal'],'hello after reconnect')
assert hc('terminal.get',{'terminal':resumed['terminal']})['state']!='finished'
print('REMOTE_DISCONNECT_RECONNECT_IDENTITY_OK',flush=True)

# The latest saved Project location drives newly admitted work (verified using a shell).
new_shell=call('terminal.create',{'project_id':project['id']});assert new_shell['runtime']['machine']=='local'
# End only this test's terminals, including resumed Agent, before archive.
call('agent.end',{'attempt_id':resumed['id']})
for t in [shell,copy,new_shell]:call('terminal.end',{'terminal_id':t['id']})
s=wait(lambda s:all((x.get('observed') or {}).get('state')=='finished' for x in s['attempts'] if x['card_id']==card['id']),'end resumed terminal')
card=current(s,card);arch=call('card.archive',{'card_id':card['id'],'revision':card['revision']});assert arch['archived']
assert (a.home/arch['path']).exists()
print('REMOTE_ARCHIVE_OK',flush=True)
(a.home.parent/'machines-result.json').write_text(json.dumps({'project_id':project['id'],'card_id':card['id'],'machine':machine,'root':root,'developer':dev,'resumed':resumed,'archive':arch},indent=2))
print('MULTI_MACHINE_ACCEPTANCE_PASSED',flush=True)
