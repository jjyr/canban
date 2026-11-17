#!/usr/bin/env python3
"""Isolated event observation, cache invalidation and reconnect acceptance; no paid Agent."""
from pathlib import Path
import json, os, shutil, subprocess, tempfile, time
repo=Path(__file__).resolve().parents[1]
base=Path(tempfile.mkdtemp(prefix='cb-events-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
core={'HC_SESSION':'default','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},**core,'CANBAN_HOME':str(home)}
wrapper=base/'hc-wrapper'
wrapper.write_text('''#!/usr/bin/python3
import sys,os,json,subprocess,threading,pathlib
base=pathlib.Path(__file__).parent
if (base/'offline').exists():sys.exit(1)
p=subprocess.Popen(['''+repr(shutil.which('hc'))+''',*sys.argv[1:]],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
def upstream():
 try:
  for line in sys.stdin.buffer:
   try:
    v=json.loads(line)
    with (base/'methods.jsonl').open('a') as f:f.write(json.dumps({'method':v.get('method')})+'\\n')
   except Exception:pass
   p.stdin.write(line);p.stdin.flush()
 except Exception:pass
 finally:
  try:p.stdin.close()
  except Exception:pass
threading.Thread(target=upstream,daemon=True).start()
records={}
try:
 for line in p.stdout:
  if (base/'offline').exists():break
  v=json.loads(line)
  if v.get('id')==4 and 'terminals' in v.get('result',{}):records={t['address']:t for t in v['result']['terminals']}
  if v.get('method')=='event':
   e=v['params'];key=e.get('terminal');kind=e.get('type');data=e.get('data',{})
   if kind in ('created','restarted'):records[key]=data.copy()
   elif kind=='removed':records.pop(key,None)
   elif key in records:
    if kind=='foreground':records[key]['foreground']=data
    elif kind=='status':records[key].update({k:data.get(k) for k in ('status','records','reported')})
    elif kind=='finished':records[key].update(data);records[key]['state']='finished';records[key].pop('foreground',None)
   if (base/'missed').exists():
    (base/'missed').unlink();v={'jsonrpc':'2.0','method':'missed','params':{'seq':e['seq'],'terminals':list(records.values())}}
    line=(json.dumps(v)+'\\n').encode()
  sys.stdout.buffer.write(line);sys.stdout.buffer.flush()
finally:
 p.kill();p.wait()
''');wrapper.chmod(0o755)
log=(base/'server.log').open('w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(m,p=None):
 r=subprocess.run([str(cli),"--full",'call',m,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=40)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def wait(f,label,timeout=30):
 until=time.monotonic()+timeout
 while time.monotonic()<until:
  v=call('app.snapshot')
  if f(v):return v
  time.sleep(.1)
 raise RuntimeError(label+' timed out: '+str(base))
def hc(method,params):
 r=subprocess.run(['hc','connect'],input='\n'.join(json.dumps({'jsonrpc':'2.0','id':i,'method':m,'params':p}) for i,m,p in [(1,'hello',{'generation':1}),(2,method,params)])+'\n',env={**os.environ,**core},capture_output=True,text=True,timeout=20)
 replies=[json.loads(l) for l in r.stdout.splitlines()];reply=next(v for v in replies if v.get('id')==2)
 if 'error' in reply:raise RuntimeError(str(reply))
 return reply['result']
def methods():
 return [json.loads(l)['method'] for l in (base/'methods.jsonl').read_text().splitlines()] if (base/'methods.jsonl').exists() else []
def git(path,*args):return subprocess.check_output(['git','-C',str(path),*args],text=True,stderr=subprocess.DEVNULL).strip()
try:
 for _ in range(100):
  try:call('app.ping');break
  except RuntimeError:time.sleep(.1)
 config=json.loads((home/'config/agents.json').read_text());config.update(hc=str(wrapper),hc_env=core,hc_session=core['HC_SESSION'])
 for p in config['profiles']:p.update(preset=None,argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'],exit={'type':'input','text':'/exit'},session_instruction='')
 (home/'config/agents.json').write_text(json.dumps(config));call('config.reload')
 expected=hc('server.status',{})['core']['id']
 wait(lambda s:call('runtime.observation')['connected'] and call('runtime.observation')['core_id']==expected,'subscription')
 source=base/'source';source.mkdir();git(source,'init','-b','main')
 for k,v in [('user.name','Acceptance'),('user.email','acceptance@example.invalid'),('core.hooksPath','/dev/null'),('commit.gpgsign','false')]:git(source,'config',k,v)
 git(source,'commit','--allow-empty','-m','Seed')
 project=call('project.create',{'name':'Events','directory':str(source),'workspace':{'create_worktree':False,'sync_base':False}})
 terminal=call('terminal.create',{'cwd':str(source)})
 def context(s,id):return next((c for c in s['terminal_contexts'] if c['terminal_id']==id),{})
 wait(lambda s:context(s,terminal['id']).get('branch')=='main','initial context')
 probes=call('runtime.observation')['context_probes']
 second=call('terminal.create',{'cwd':str(source)})
 wait(lambda s:context(s,second['id']).get('branch')=='main','shared directory context')
 assert call('runtime.observation')['context_probes']==probes,'same cwd repeated Git discovery'
 before=len(methods());before_probes=call('runtime.observation')['context_probes']
 print('WAITING_FOR_IDLE_HEARTBEAT',flush=True);time.sleep(17)
 assert not {'terminal.list','terminal.get','server.status'} & set(methods()[before:]),methods()[before:]
 assert call('runtime.observation')['context_probes']==before_probes
 print('NO_IDLE_RUNTIME_SCANS_AND_SHARED_CWD_OK',flush=True)
 git(source,'switch','-c','changed')
 wait(lambda s:context(s,terminal['id']).get('branch')=='changed' and context(s,second['id']).get('branch')=='changed','HEAD invalidation')
 outside=base/'outside';outside.mkdir();hc('terminal.send',{'terminal':terminal['runtime']['terminal'],'text':'cd '+str(outside),'force':True})
 wait(lambda s:context(s,terminal['id']).get('cwd')==str(outside),'cwd event')
 observed=wait(lambda s:context(s,terminal['id']).get('directory_root')==str(outside.resolve()),'external canonical root')
 assert context(observed,terminal['id'])['project_id'] is None
 # Builtin cd keeps the shell PID; subdirectories must share the Git root.
 sub=source/'src';sub.mkdir()
 hc('terminal.send',{'terminal':terminal['runtime']['terminal'],'text':'cd '+str(sub),'force':True})
 observed=wait(lambda s:context(s,terminal['id']).get('cwd')==str(sub) and context(s,terminal['id']).get('directory_root')==str(source.resolve()),'subdirectory root')
 assert context(observed,terminal['id'])['project_id']==project['id']
 # Rapid changes must settle on the last cwd, never a late earlier discovery.
 hc('terminal.send',{'terminal':terminal['runtime']['terminal'],'text':'cd '+str(source)+'; cd '+str(sub)+'; cd '+str(outside),'force':True})
 observed=wait(lambda s:context(s,terminal['id']).get('cwd')==str(outside) and context(s,terminal['id']).get('directory_root')==str(outside.resolve()),'rapid cd final root')
 assert context(observed,terminal['id'])['project_id'] is None
 print('PLAIN_SHELL_CWD_ROOT_AND_RAPID_CD_OK',flush=True)
 git(outside,'init','-b','new-repository')
 wait(lambda s:context(s,terminal['id']).get('branch')=='new-repository','new .git invalidation')
 stub=base/'agy';stub.write_text('#!/usr/bin/env python3\nimport time\ntime.sleep(0.8)\n');stub.chmod(0o755)
 before_agent=len(methods())
 hc('terminal.send',{'terminal':terminal['runtime']['terminal'],'text':str(stub),'force':True})
 def term_row(s,tid):return next((t for t in s['terminals'] if t['id']==tid),{})
 s_live=wait(lambda s:term_row(s,terminal['id']).get('runtime',{}).get('observed_agent')=='agy','foreground agent push')
 assert term_row(s_live,terminal['id']).get('agent_engine') is None
 wait(lambda s:term_row(s,terminal['id']).get('runtime',{}).get('observed_agent') is None,'foreground shell return')
 assert not {'terminal.list','terminal.get','server.status'} & set(methods()[before_agent:]),methods()[before_agent:]
 print('FOREGROUND_AGENT_PUSH_ONLY_OK',flush=True)
 card=call('card.create',{'project_id':project['id'],'title':'Report-free failure'})
 opened=call('agent.open',{'card_id':card['id'],'profile_id':'codex'})
 def attempt(s):
  t=next((t for t in s['terminals'] if t['id']==opened['terminal_id']),None)
  return t['agent'] if t and (t.get('agent') or {}).get('provider_session_id') else None
 s=wait(lambda s:attempt(s),'Agent');a=attempt(s)
 hc('terminal.send',{'terminal':a['terminal'],'text':'permission','force':True})
 wait(lambda s:attempt(s).get('lifecycle',{}).get('waiting'),'permission')
 generation=call('runtime.observation')['generation'];(base/'missed').touch()
 hc('terminal.send',{'terminal':a['terminal'],'text':'permission-update','force':True})
 wait(lambda s:call('runtime.observation')['generation']>generation and 'diff' in (attempt(s).get('lifecycle',{}).get('waiting') or {}).get('message',''),'missed snapshot')
 hc('terminal.send',{'terminal':a['terminal'],'text':'/exit','force':True})
 s=wait(lambda s:attempt(s)['state']=='failed' and attempt(s).get('output_path'),'exit failure')
 assert not attempt(s)['lifecycle']['waiting']
 assert next(c for c in s['cards'] if c['id']==card['id'])['human_gate'] is None
 assert next(r for r in call('overview.list') if r['card']['id']==card['id'])['group']=='Closed'
 print('MISSED_SNAPSHOT_AND_FAILED_EXIT_OK',flush=True)
 generation=call('runtime.observation')['generation'];(base/'offline').touch()
 hc('terminal.send',{'terminal':second['runtime']['terminal'],'text':'echo reconnect','force':True})
 wait(lambda s:not call('runtime.observation')['connected'],'disconnect')
 assert next(t for t in call('app.snapshot')['terminals'] if t['id']==second['id'])['runtime']['state']!='finished'
 (base/'offline').unlink()
 wait(lambda s:call('runtime.observation')['connected'] and call('runtime.observation')['generation']>generation and context(s,second['id']).get('branch')=='changed','reconnect')
 print('RECONNECT_WITHOUT_FALSE_EXIT_OK',flush=True)
 (base/'evidence.json').write_text(json.dumps({'observation':call('runtime.observation'),'methods':methods(),'snapshot':call('app.snapshot')},indent=2))
 print('EVENT_ACCEPTANCE_PASSED '+str(base),flush=True)
finally:
 try:call('app.shutdown')
 except Exception:pass
 try:server.wait(timeout=10)
 except subprocess.TimeoutExpired:server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**os.environ,**core},capture_output=True,timeout=20);log.close()
