#!/usr/bin/env python3
"""Isolated real-OS event observation, FSEvents/.git invalidation, and foreground-process detection."""
from pathlib import Path
import json, os, shutil, subprocess, tempfile, time
repo=Path(__file__).resolve().parents[1]
base=Path(tempfile.mkdtemp(prefix='cb-events-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
core={'HC_SESSION':'default','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
env={**{k:v for k,v in os.environ.items() if not k.startswith(('CANBAN_','HC_','HERDR_'))},**core,'CANBAN_HOME':str(home)}
wrapper=base/'hc-wrapper'
wrapper.write_text('''#!/usr/bin/python3
import sys,json,subprocess,threading,pathlib
base=pathlib.Path(__file__).parent
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
try:
 for line in p.stdout:
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
 r=subprocess.run(['hc','connect'],input='\n'.join(json.dumps({'jsonrpc':'2.0','id':i,'method':m,'params':p}) for i,m,p in [(1,'hello',{'generation':1}),(2,method,params)])+'\n',env=env,capture_output=True,text=True,timeout=20)
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
 before=len(methods())
 print('SHARED_CWD_CACHE_OK',flush=True)
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
 assert not {'terminal.list','terminal.get','server.status'} & set(methods()[before:]),methods()[before:]
 assert not {'terminal.list','terminal.get','server.status'} & set(methods()[before_agent:]),methods()[before_agent:]
 print('FOREGROUND_AGENT_PUSH_ONLY_OK',flush=True)
 (base/'evidence.json').write_text(json.dumps({'observation':call('runtime.observation'),'methods':methods(),'snapshot':call('app.snapshot')},indent=2))
 print('EVENT_ACCEPTANCE_PASSED '+str(base),flush=True)
finally:
 try:call('app.shutdown')
 except Exception:pass
 try:server.wait(timeout=10)
 except subprocess.TimeoutExpired:server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env=env,capture_output=True,timeout=20);log.close()
