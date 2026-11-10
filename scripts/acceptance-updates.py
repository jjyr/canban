#!/usr/bin/env python3
"""Isolated HTTP releases + real service/Herdr restart; optional native handoff.
Prepare baseline/candidate bundles with distinct compiled build identities first.
"""
import argparse,hashlib,http.server,json,os,pathlib,plistlib,shutil,signal,socketserver,subprocess,tempfile,threading,time
P=pathlib.Path
parser=argparse.ArgumentParser();parser.add_argument('--baseline',type=P,required=True);parser.add_argument('--candidate',type=P,required=True);parser.add_argument('--native',action='store_true');args=parser.parse_args()
repo=P(__file__).resolve().parents[1];base=P(tempfile.mkdtemp(prefix='cb-updates-',dir='/private/tmp'));candidate_info=json.loads(subprocess.check_output([str(args.candidate/'Contents/MacOS/canban'),'build-info'],text=True))
print('UPDATE_ACCEPTANCE_BASE='+str(base),flush=True)

def fake_ui(path,fail=False,save_delay=1):
    path.write_text('#!/usr/bin/python3\n'+("raise SystemExit(29)\n" if fail else f'''import os,json,time,subprocess
from pathlib import Path
home=Path(os.environ['CANBAN_HOME']);binary=Path(__file__).with_name('canban')
info=subprocess.check_output([str(binary),'build-info'])
if os.environ.get('CANBAN_UPGRADE_TOKEN'):
 (home/'updates'/('ui-ready-'+os.environ['CANBAN_UPGRADE_TOKEN'])).write_bytes(info)
while True:
 try:
  j=json.loads((home/'updates/install.json').read_text())
  if j['phase'] in ['prepared','stopping']:
   time.sleep({save_delay});(home/'updates'/('ui-saved-'+j['token'])).write_text('ready')
 except (FileNotFoundError,KeyError):pass
 time.sleep(.1)
'''));path.chmod(0o755)

def run_case(name):
    root=base/name;root.mkdir();home=root/'home';target=root/'CanbanAcceptance.app';candidate=root/'candidate/Canban.app';shutil.copytree(args.baseline,target);shutil.copytree(args.candidate,candidate)
    app_id='com.canban.update.acceptance.'+name
    for bundle in [target,candidate]:
        plist=bundle/'Contents/Info.plist';v=plistlib.loads(plist.read_bytes());v.update(CFBundleIdentifier=app_id,CFBundleName='Canban Update Acceptance');plist.write_bytes(plistlib.dumps(v))
    if args.native:
        for bundle in [target,candidate]:subprocess.run(['/usr/bin/codesign','--force','--sign','-','--timestamp=none',str(bundle)],check=True,capture_output=True)
    if not args.native:
        fake_ui(target/'Contents/MacOS/canban-ui');fake_ui(candidate/'Contents/MacOS/canban-ui',fail=name in ['boot-failure','down-failure'])
    if name in ['up-failure','down-failure','schema-rollback']:
        binary=candidate/'Contents/MacOS/canban';binary.rename(binary.with_name('canban-real'))
        binary.write_text('''#!/usr/bin/python3
import os,sys,json,sqlite3,subprocess
from pathlib import Path
args=sys.argv[1:]
if CASE in ['down-failure','schema-rollback'] and args==['build-info']:
 info=json.loads(subprocess.check_output([str(Path(__file__).with_name('canban-real')),'build-info']));info['schema']=9;print(json.dumps(info));sys.exit(0)
if 'database' in args:
 home=Path(args[args.index('--home')+1]);marker=home/'updates/migration-probe'
 number=int(marker.read_text())+1 if marker.exists() else 1;marker.write_text(str(number))
 if (CASE=='up-failure' and number==1) or (CASE=='down-failure' and number>1):
  print('Injected migration failure',file=sys.stderr);sys.exit(31)
 if CASE in ['schema-rollback','down-failure']:
  target=int(args[args.index('--target')+1]);db=sqlite3.connect(home/'canban.sqlite');db.execute('BEGIN IMMEDIATE')
  db.execute('CREATE TABLE future_schema_fixture(id INTEGER)' if target==9 else 'DROP TABLE future_schema_fixture');db.execute('PRAGMA user_version='+str(target));db.commit();db.close();print(json.dumps({'schema':target}));sys.exit(0)
os.execv(str(Path(__file__).with_name('canban-real')),[str(Path(__file__).with_name('canban-real')),*args])
'''.replace('CASE',repr(name)));binary.chmod(0o755)
    artifact=root/('Canban-'+candidate_info['platform']+'.tar.zst')
    subprocess.run([str(args.candidate/'Contents/MacOS/canban-updater'),'pack',str(candidate),str(artifact)],check=True)
    case_info={**candidate_info,'schema':9} if name in ['schema-rollback','down-failure'] else candidate_info
    manifest={**case_info,'rollback_floor':8,'asset':artifact.name,'size':artifact.stat().st_size,'sha256':hashlib.sha256(artifact.read_bytes()).hexdigest()}
    if name=='corrupt':manifest['sha256']='0'*64
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            url='http://127.0.0.1:'+str(self.server.server_address[1])
            if self.path.startswith('/repos/'):
                asset={'name':artifact.name,'size':artifact.stat().st_size,'browser_download_url':url+'/artifact'}
                meta={'name':'manifest-'+candidate_info['platform']+'.json','browser_download_url':url+'/manifest'}
                body=json.dumps([{'tag_name':'20990101-999','draft':True,'assets':[asset,meta]},{'tag_name':candidate_info['version'],'draft':False,'prerelease':True,'body':'Disposable update fixture','assets':[asset,meta]}]).encode()
            elif self.path=='/manifest':body=json.dumps(manifest).encode()
            elif self.path=='/artifact':
                self.send_response(200);self.send_header('Content-Length',str(artifact.stat().st_size));self.end_headers()
                with artifact.open('rb') as stream:
                    while True:
                        block=stream.read(65536)
                        if not block:break
                        self.wfile.write(block);time.sleep(.002)
                return
            else:self.send_error(404);return
            self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        def log_message(self,*_):pass
    release_server=socketserver.ThreadingTCPServer(('127.0.0.1',0),Handler);threading.Thread(target=release_server.serve_forever,daemon=True).start()
    core={'HC_SESSION':'default','HC_SOCKET':str(root/'hc.sock'),'XDG_STATE_HOME':str(root/'hc-state'),'XDG_CONFIG_HOME':str(root/'hc-config')}
    env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},**core,'CANBAN_HOME':str(home),'CANBAN_UPDATE_API':'http://127.0.0.1:'+str(release_server.server_address[1]),'CANBAN_APP_ID':app_id}
    cli=target/'Contents/MacOS/canban';log=(root/'server.log').open('w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log);ui=None
    def call(method,p=None):
        r=subprocess.run([str(cli),'call',method,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=30)
        if r.returncode:raise RuntimeError(r.stderr)
        return json.loads(r.stdout)
    def wait(fn,label,timeout=60):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            try:
                value=fn()
                if value:return value
            except (RuntimeError,FileNotFoundError,json.JSONDecodeError):pass
            time.sleep(.1)
        raise RuntimeError('Timed out '+label+'; evidence '+str(root))
    def journal():return json.loads((home/'updates/install.json').read_text())
    try:
        wait(lambda:call('app.ping'),'service');initial_pid=call('app.ping')['pid']
        config_path=home/'config/agents.json';config=json.loads(config_path.read_text());config.update(hc_env=core,hc_session='default')
        for profile in config['profiles']:
            profile.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
        config_path.write_text(json.dumps(config));call('config.reload')
        flow=home/'config/workflows/default.json';v=json.loads(flow.read_text());v['workspace']['sync_base']=False;flow.write_text(json.dumps(v))
        source=root/'source';source.mkdir()
        for command in [['init','-b','main'],['config','user.name','Acceptance'],['config','user.email','acceptance@example.invalid'],['config','core.hooksPath','/dev/null'],['config','commit.gpgsign','false']]:subprocess.run(['git','-C',str(source),*command],capture_output=True,check=True)
        (source/'seed').write_text('preserve');subprocess.run(['git','-C',str(source),'add','.'],check=True,capture_output=True);subprocess.run(['git','-C',str(source),'commit','-m','Seed'],check=True,capture_output=True)
        project=call('project.create',{'name':'Self update acceptance','directory':str(source)})
        card=call('card.create',{'project_id':project['id'],'title':'Keep the Agent alive across update','body':'Disposable update acceptance.'});call('card.move',{'card_id':card['id'],'revision':card['revision'],'stage':'develop'})
        attempt=wait(lambda:next((a for a in call('app.snapshot')['attempts'] if a.get('provider_session_id')),None),'provider binding')
        core_id=attempt['owner_core_id'];terminal=attempt['terminal'];worktree=call('card.get',{'card_id':card['id']})['worktree']
        wait(lambda:call('update.state')['state'].get('available'),'automatic release discovery')
        assert call('update.state')['state']['available']['manifest']['version']==candidate_info['version']
        agent_install=subprocess.run([str(cli),'call','update.install',json.dumps({'version':candidate_info['version']})],env={**env,'CANBAN_REQUEST_ID':attempt['request_id'],'CANBAN_ATTEMPT':str(attempt['number'])},capture_output=True,text=True,timeout=15)
        assert agent_install.returncode!=0 and 'explicit user action' in agent_install.stderr
        ui=subprocess.Popen([str(target/'Contents/MacOS/canban-ui')],env=env,stdout=log,stderr=log)
        sent=threading.Event()
        def report_during_handoff():
            try:
                wait(lambda:journal()['phase']=='stopping','upgrade handoff',timeout=1800 if args.native else 60)
                subprocess.run(['hc','send',terminal,'finish','--force','--json'],env={**os.environ,**core},capture_output=True,text=True,check=True,timeout=20)
                sent.set()
            except Exception as error:(root/'sender-error').write_text(str(error))
        if name!='corrupt':threading.Thread(target=report_during_handoff,daemon=True).start()
        if args.native:
            metadata={'base':str(base),'root':str(root),'home':str(home),'bundle':str(target),'app_id':app_id,'card':card,'attempt':attempt,'env':{k:env[k] for k in [*core,'CANBAN_HOME','CANBAN_UPDATE_API','CANBAN_APP_ID']}}
            (repo/'.local/update-native.json').write_text(json.dumps(metadata,indent=2));print('UPDATE_NATIVE_READY='+str(root),flush=True)
            wait(lambda:(root/'native-done').exists(),'native interaction',timeout=1800)
        else:
            wait(lambda:subprocess.run(['/bin/ps','-p',str(ui.pid),'-o','command='],capture_output=True,text=True).stdout.find('canban-ui')>=0,'fake UI')
            try:call('update.install',{'ui_pid':ui.pid,'app_id':app_id,'version':'20000101-1'});raise AssertionError('Stale update target admitted')
            except RuntimeError as error:assert 'version changed' in str(error)
            call('update.install',{'ui_pid':ui.pid,'app_id':app_id,'version':candidate_info['version']})
            try:call('update.install',{'ui_pid':ui.pid,'app_id':app_id,'version':candidate_info['version']});raise AssertionError('Duplicate install admitted')
            except RuntimeError:pass
        if name=='corrupt':
            wait(lambda:call('update.state')['state']['phase']=='error','corrupt rejection')
            assert call('app.ping')['pid']==initial_pid and not (home/'updates/install.json').exists()
        else:
            expected='complete' if name=='normal' else 'manual_recovery' if name=='down-failure' else 'rolled_back'
            wait(lambda:journal()['phase']==expected,expected)
            assert sent.is_set()
            if expected!='manual_recovery':
                wait(lambda:any(r['card_id']==card['id'] and r['stage']=='develop' and r['state']=='completed' for r in call('app.snapshot')['requests']),'queued completion delivered')
                state=call('app.snapshot');original=next(a for a in state['attempts'] if a['id']==attempt['id']);assert original['terminal']==terminal and original['owner_core_id']==core_id
                assert call('card.get',{'card_id':card['id']})['worktree']==worktree
                assert len([r for r in state['reports'] if r['request_id']==attempt['request_id'] and r['state']=='completed'])==1
                if expected=='complete':assert call('app.ping')['build']==candidate_info
                else:assert call('app.ping')['build']['version']!=candidate_info['version']
            else:
                assert list((home/'updates/outbox').glob('*.json'))
                probe=subprocess.run([str(cli),'serve'],env=env,capture_output=True,text=True,timeout=10);assert probe.returncode!=0 and 'pending' in probe.stderr
        (root/'result.json').write_text(json.dumps({'case':name,'terminal':terminal,'core_id':core_id,'journal':journal() if (home/'updates/install.json').exists() else None},indent=2))
        print('UPDATE_'+name.upper().replace('-','_')+'_OK',flush=True)
    finally:
        try:call('app.shutdown')
        except Exception:pass
        if server.poll() is None:server.terminate();server.wait(timeout=15)
        if ui is not None and ui.poll() is None:ui.terminate();ui.wait(timeout=10)
        if (home/'updates/install.json').exists():
            j=journal()
            for key in ['candidate_ui','server','recovery_ui']:
                process=j.get(key)
                if process:
                    info=subprocess.run(['/bin/ps','-p',str(process['pid']),'-o','lstart=,command='],capture_output=True,text=True).stdout.strip()
                    if info==process['identity']:
                        try:os.kill(process['pid'],signal.SIGTERM)
                        except ProcessLookupError:pass
        # Restored fake UIs are not candidate UI children; identify only this
        # disposable target path, never a global app/process name.
        ps=subprocess.check_output(['/bin/ps','-axo','pid=,command='],text=True)
        for line in ps.splitlines():
            if str(target/'Contents/MacOS/canban-ui') in line:
                pid=int(line.strip().split(None,1)[0])
                try:os.kill(pid,signal.SIGTERM)
                except ProcessLookupError:pass
        subprocess.run(['hc','server','stop'],env={**os.environ,**core},capture_output=True,timeout=20)
        release_server.shutdown();release_server.server_close();log.close()
for name in (['normal'] if args.native else ['normal','corrupt','up-failure','boot-failure','schema-rollback','down-failure']):run_case(name)
print('UPDATES_ACCEPTANCE_PASSED',flush=True)
