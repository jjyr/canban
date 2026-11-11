#!/usr/bin/env python3
"""Backend activation/daily-check acceptance: disposable apps, homes, HTTP and Core.
--native exercises the real Update backend button; --newer-native verifies entry
against a newer reported backend. Native modes wait for the printed marker.
"""
import argparse, fcntl, hashlib, http.server, json, os, plistlib, shutil, signal, socket
import sqlite3, subprocess, tempfile, threading, time
from pathlib import Path as P
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--native',action='store_true');parser.add_argument('--newer-native',action='store_true')
args=parser.parse_args();repo=P(__file__).resolve().parents[1]
base=P(tempfile.mkdtemp(prefix='cbb-',dir='/private/tmp'))
env={k:v for k,v in os.environ.items() if not k.startswith(('CANBAN_','HC_'))}
requests=[];fail_http=False
class HTTP(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  requests.append(self.path);self.send_response(503 if fail_http else 200);self.end_headers();self.wfile.write(b'[]')
 def log_message(self,*_):pass
http=http.server.ThreadingHTTPServer(('127.0.0.1',0),HTTP)
threading.Thread(target=http.serve_forever,daemon=True).start()
api='http://127.0.0.1:'+str(http.server_address[1])
print('BACKEND_ACCEPTANCE_BASE='+str(base),flush=True)

def run(argv,environment,check=True):
 r=subprocess.run(list(map(str,argv)),env=environment,capture_output=True,text=True,timeout=60)
 if check and r.returncode:raise RuntimeError(r.stderr+'\n'+r.stdout)
 return r

def rpc(endpoint,method,params=None):
 with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as s:
  s.settimeout(10);s.connect(str(endpoint));s.sendall((json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params or {}})+'\n').encode())
  v=json.loads(s.makefile('rb').readline())
  if 'error' in v:raise RuntimeError(str(v['error']))
  return v['result']

def wait(fn,label,timeout=45,old=None):
 until=time.monotonic()+timeout
 while time.monotonic()<until:
  if old is not None:old.poll()  # Reap the legacy child before a recovery attempt.
  try:
   result=fn()
   if result:return result
  except (OSError,RuntimeError,ValueError):pass
  time.sleep(.1)
 raise RuntimeError('Timed out '+label+'; evidence '+str(base))

def bootstrap(name, quiet=True):
 root=base/name;home=root/'home';(home/'config').mkdir(parents=True)
 core={'HC_SESSION':'backend-acceptance','HC_SOCKET':str(root/'hc.sock'),'XDG_STATE_HOME':str(root/'state'),'XDG_CONFIG_HOME':str(root/'config')}
 (home/'config/agents.json').write_text(json.dumps({'hc':shutil.which('hc'),'hc_session':core['HC_SESSION'],'hc_env':core,'default_agent':'developer','profiles':[{'id':'developer','preset':'codex'},{'id':'reviewer','preset':'codex'}]}))
 if quiet:
  (home/'updates').mkdir();(home/'updates/state.json').write_text(json.dumps({'phase':'current','available':None,'progress':0,'error':None,'checked_at':int(time.time()),'last_check_started_at':int(time.time())}))
 bundle=root/'Canban.app';shutil.copytree(repo/'.local/Canban.app',bundle)
 app_id='com.canban.backend.'+root.name
 p=bundle/'Contents/Info.plist';v=plistlib.loads(p.read_bytes());v.update(CFBundleIdentifier=app_id,CFBundleName='Canban Backend Acceptance');p.write_bytes(plistlib.dumps(v))
 environment={**env,**core,'CANBAN_HOME':str(home),'CANBAN_APP_ID':app_id,'CANBAN_UPDATE_API':api}
 return root,home,bundle,environment,core

def serve(bundle,environment,log):
 return subprocess.Popen([str(bundle/'Contents/MacOS/canban'),'serve'],env=environment,stdout=log,stderr=log)

def stop(home,process):
 if process and process.poll() is None:
  try:rpc(home/'app.sock','app.shutdown')
  except (OSError,RuntimeError):pass
  process.wait(timeout=15)

def core_stop(core):run([shutil.which('hc'),'server','stop'],{**env,**core},False)

def cleanup(home,bundle,environment,core,processes):
 for process in processes:stop(home,process)
 journal=home/'updates/install.json'
 if journal.exists():
  saved=json.loads(journal.read_text())
  for field in ('candidate_ui','server','recovery_ui'):
   p=saved.get(field)
   if p:
    actual=run(['/bin/ps','-p',p['pid'],'-o','lstart=,command='],env,False).stdout.strip()
    if actual==p['identity']:
     if field=='server':
      try:rpc(home/'app.sock','app.shutdown')
      except (OSError,RuntimeError):pass
     else:
      try:os.kill(p['pid'],signal.SIGTERM)
      except ProcessLookupError:pass
 core_stop(core)

def fake_ui(path):
 path.write_text('''#!/usr/bin/python3
import os,subprocess,time
from pathlib import Path
home=Path(os.environ['CANBAN_HOME'])
if (home/'fail-boot').exists():raise SystemExit(29)
info=subprocess.check_output([str(Path(__file__).with_name('canban')),'build-info'])
(home/'updates'/('ui-ready-'+os.environ['CANBAN_UPGRADE_TOKEN'])).write_bytes(info)
while True:time.sleep(.1)
''');path.chmod(0o755)

def old_service(home,environment,log):
 file=home.parent/'old.py';file.write_text('''import fcntl,json,os,socket,sys
from pathlib import Path
h=Path(sys.argv[1]);s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
with (h/'server.lock').open('a') as lock:
 fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 (h/'app.sock').unlink(missing_ok=True);s.bind(str(h/'app.sock'));s.listen()
 while True:
  with s.accept()[0] as c:
   r=json.loads(c.makefile('rb').readline());quit=r['method']=='app.shutdown'
   result={'stopping':True} if quit else {'pid':os.getpid(),'home':str(h),'version':'0.1.0','api_version':4}
   c.sendall((json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result})+'\\n').encode())
  if quit:break
 s.close();(h/'app.sock').unlink(missing_ok=True)
''')
 return subprocess.Popen(['/usr/bin/python3',str(file),str(home)],env=environment,stdout=log,stderr=log)

def local_case(name,schema,native=False):
 root,home,bundle,environment,core=bootstrap(name);log=(root/'run.log').open('w')
 cli=bundle/'Contents/MacOS/canban';ui=None;old=None;seed=None;before_http=len(requests)
 try:
  seed=serve(bundle,environment,log);wait(lambda:rpc(home/'app.sock','app.ping'),'seed')
  terminal=rpc(home/'app.sock','terminal.create',{'cwd':str(root)})
  terminal_pid=json.loads(run([shutil.which('hc'),'get',terminal['runtime']['terminal'],'--json'],{**env,**core}).stdout)['pid']
  stop(home,seed)
  db=sqlite3.connect(home/'canban.sqlite');db.execute('CREATE TABLE keep(value TEXT)');db.execute("INSERT INTO keep VALUES('preserved')");db.commit();db.close()
  if schema<10:run([cli,'database','--target','8'],environment)
  if schema==7:
   db=sqlite3.connect(home/'canban.sqlite');db.executescript('DROP TABLE card_reads; PRAGMA user_version=7;');db.close()
  # Count actual migration CLI invocations and inject bounded failures.
  cli.rename(cli.with_name('canban-real'))
  cli.write_text('''#!/usr/bin/python3
import os,sys
from pathlib import Path
if 'database' in sys.argv:
 home=Path(sys.argv[sys.argv.index('--home')+1])
 with (home/'migration-calls').open('a') as f:f.write('migration\\n')
 if (home/'fail-up').exists():raise SystemExit(31)
os.execv(str(Path(__file__).with_name('canban-real')),[str(Path(__file__).with_name('canban-real')),*sys.argv[1:]])
''');cli.chmod(0o755)
  if not native:fake_ui(bundle/'Contents/MacOS/canban-ui')
  else:run(['/usr/bin/codesign','--force','--deep','--sign','-','--timestamp=none',bundle],environment)
  if name=='up-failure':(home/'fail-up').touch()
  if name=='boot-failure':(home/'fail-boot').touch()
  hashes={str(p.relative_to(bundle)):hashlib.sha256(p.read_bytes()).hexdigest() for p in bundle.rglob('*') if p.is_file()}
  old=old_service(home,environment,log);peer=wait(lambda:rpc(home/'app.sock','app.ping'),'legacy',old=old)
  if native:
   ui=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=environment,stdout=log,stderr=log)
   metadata=dict(base=str(base),root=str(root),home=str(home),bundle=str(bundle),app_id=environment['CANBAN_APP_ID'],core=core)
   (root/'native.json').write_text(json.dumps(metadata,indent=2));print('BACKEND_NATIVE_READY='+str(root),flush=True)
  else:
   token='local-fixture';job=home/'updates/jobs'/token;job.mkdir(parents=True)
   shutil.copy2(bundle/'Contents/MacOS/canban-updater',job/'canban-updater')
   build=json.loads(run([cli,'build-info'],environment).stdout)
   identity=run(['/bin/ps','-p',old.pid,'-o','lstart=,command='],environment).stdout.strip()
   j=dict(local={'peer':{'pid':old.pid,'identity':identity},'backup':str(job/'database-before.sqlite')},token=token,phase='prepared',home=str(home),target=str(bundle),staged=str(bundle),previous=str(job/'unused-app-backup'),build=build,schema_before=schema,before=None,ui=None,app_id=environment['CANBAN_APP_ID'],error=None,server=None,candidate_ui=None,recovery_ui=None)
   (home/'updates/install.json').write_text(json.dumps(j))
   helper=subprocess.Popen([str(job/'canban-updater'),'--home',str(home)],env=environment,stdout=log,stderr=log)
  def journal():return json.loads((home/'updates/install.json').read_text())
  expected='manual_recovery' if name in ('up-failure','boot-failure') else 'complete'
  wait(lambda:journal()['phase']==expected,expected,timeout=1800 if native else 60,old=old)
  old.wait(timeout=15)
  if not native:helper.wait(timeout=15)
  if expected=='manual_recovery':
   db=sqlite3.connect(home/'canban.sqlite');assert db.execute('PRAGMA user_version').fetchone()[0]==schema;assert db.execute('SELECT * FROM keep').fetchall()==[('preserved',)];db.close()
   assert run([cli,'serve'],environment,False).returncode!=0
   (home/'fail-up').unlink(missing_ok=True);(home/'fail-boot').unlink(missing_ok=True)
   run([home/'updates/jobs'/journal()['token']/'canban-updater','--home',home,'--recover'],environment)
   wait(lambda:journal()['phase']=='complete','local retry')
  if native:
   print('BACKEND_NATIVE_UPDATED='+str(root),flush=True)
   wait(lambda:(root/'native-done').exists(),'native inspection',timeout=1800)
  assert len(requests)==before_http,requests
  assert hashes=={str(p.relative_to(bundle)):hashlib.sha256(p.read_bytes()).hexdigest() for p in bundle.rglob('*') if p.is_file()}
  calls=(home/'migration-calls').read_text().splitlines() if (home/'migration-calls').exists() else []
  assert len(calls)==(0 if schema==10 else 2 if expected=='manual_recovery' else 1),calls
  assert (home/'updates/jobs'/journal()['token']/'database-before.sqlite').exists()==(schema<10)
  db=sqlite3.connect(home/'canban.sqlite');assert db.execute('PRAGMA user_version').fetchone()[0]==10;assert db.execute('SELECT * FROM keep').fetchall()==[('preserved',)];db.close()
  assert json.loads(run([shutil.which('hc'),'get',terminal['runtime']['terminal'],'--json'],{**env,**core}).stdout)['pid']==terminal_pid
  (root/'result.json').write_text(json.dumps(dict(schema_before=schema,migration_calls=len(calls),downloads=0,bundle_unchanged=True,pty_preserved=True,journal=journal()),indent=2))
  print('LOCAL_'+name.upper().replace('-','_')+'_OK',flush=True)
 finally:
  if old and old.poll() is None:stop(home,old)
  if ui and ui.poll() is None:ui.terminate();ui.wait(timeout=15)
  cleanup(home,bundle,environment,core,[])
  log.close()

def daily_case():
 global fail_http
 root,home,bundle,environment,core=bootstrap('daily',False);log=(root/'run.log').open('w');server=None;start_count=len(requests)
 def restart():
  nonlocal server
  stop(home,server);server=serve(bundle,environment,log);wait(lambda:rpc(home/'app.sock','app.ping'),'daily service')
 def state():return rpc(home/'app.sock','update.state')['state']
 try:
  restart();wait(lambda:len(requests)==start_count+1 and state()['phase']=='current','initial daily check')
  restart();time.sleep(1.2);assert len(requests)==start_count+1
  stop(home,server);server=None
  p=home/'updates/state.json';v=json.loads(p.read_text());v['phase']='checking';p.write_text(json.dumps(v))
  restart();wait(lambda:state()['phase']=='error','interrupted check state');assert len(requests)==start_count+1
  rpc(home/'app.sock','update.check');wait(lambda:len(requests)==start_count+2 and state()['phase']=='current','manual bypass')
  fail_http=True;rpc(home/'app.sock','update.check');wait(lambda:len(requests)==start_count+3 and state()['phase']=='error','failed attempt')
  restart();time.sleep(1.2);assert len(requests)==start_count+3
  stop(home,server);server=None;fail_http=False
  p=home/'updates/state.json';v=json.loads(p.read_text());v['last_check_started_at']-=86401;p.write_text(json.dumps(v))
  restart();wait(lambda:len(requests)==start_count+4 and state()['phase']=='current','next daily interval')
  (root/'result.json').write_text(json.dumps(dict(first_check=1,restart_checks=0,manual_checks=2,overdue_checks=1),indent=2))
  print('DAILY_CHECK_OK',flush=True)
 finally:
  fail_http=False;cleanup(home,bundle,environment,core,[server]);log.close()

def newer_native():
 root,home,bundle,environment,core=bootstrap('newer');log=(root/'run.log').open('w')
 run(['/usr/bin/codesign','--force','--deep','--sign','-','--timestamp=none',bundle],environment)
 server=serve(bundle,environment,log);ui=None;proxy=None;done=threading.Event()
 try:
  wait(lambda:rpc(home/'app.sock','app.ping'),'newer seed')
  (home/'app.sock').rename(home/'actual.sock')
  proxy=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);proxy.bind(str(home/'app.sock'));proxy.listen();proxy.settimeout(.2)
  def relay():
   while not done.is_set():
    try:client,_=proxy.accept()
    except socket.timeout:continue
    except OSError:
     if done.is_set():break
     raise
    with client:
     request=json.loads(client.makefile('rb').readline())
     result=rpc(home/'actual.sock',request['method'],request.get('params'))
     if request['method']=='app.ping':result['api_version']+=1
     client.sendall((json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result})+'\n').encode())
  worker=threading.Thread(target=relay,daemon=True);worker.start()
  ui=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=environment,stdout=log,stderr=log)
  (root/'native.json').write_text(json.dumps(dict(root=str(root),home=str(home),bundle=str(bundle),app_id=environment['CANBAN_APP_ID']),indent=2))
  print('NEWER_BACKEND_NATIVE_READY='+str(root),flush=True)
  wait(lambda:(root/'native-done').exists(),'newer native inspection',timeout=1800)
  assert not (home/'updates/install.json').exists()
  (root/'result.json').write_text(json.dumps(dict(normal_entry=True,no_handoff=True),indent=2))
  print('NEWER_BACKEND_NATIVE_OK',flush=True)
 finally:
  if ui and ui.poll() is None:ui.terminate();ui.wait(timeout=15)
  try:rpc(home/'actual.sock','app.shutdown')
  except (OSError,RuntimeError):pass
  server.wait(timeout=15);done.set()
  if proxy:proxy.close()
  core_stop(core);log.close()

try:
 if args.newer_native:newer_native()
 elif args.native:local_case('native',10,True)
 else:
  for name,schema in [('same-schema',10),('migration',8),('legacy',7),('up-failure',8),('boot-failure',8)]:local_case(name,schema)
  daily_case()
 print('BACKEND_ACCEPTANCE_PASSED',flush=True)
finally:http.shutdown();http.server_close()
