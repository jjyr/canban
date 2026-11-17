#!/usr/bin/env python3
"""Real branch-isolated launch/stop acceptance. Disposable checkouts/home/Core only.

--native leaves two actual UIs open until the printed fixture's continue marker
exists, then completes the same stop/ownership/restart assertions.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--native', action='store_true')
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
tmp_root = '/private/tmp' if sys.platform == 'darwin' and Path('/private/tmp').is_dir() else None
base = Path(tempfile.mkdtemp(prefix='cbd-', dir=tmp_root))
env = {k:v for k,v in os.environ.items() if not k.startswith(('CANBAN_','HC_','GIT_'))}
core = {'HC_SESSION':'dev-acceptance','HC_SOCKET':str(base/'hc.sock'),
        'XDG_STATE_HOME':str(base/'state'),'XDG_CONFIG_HOME':str(base/'config')}
env.update(core)
log = (base/'acceptance.log').open('w')
cli = repo/'target/debug/canban'


def command(argv, check=True, **kw):
    return subprocess.run(list(map(str,argv)), env=kw.pop('env',env), capture_output=True,
                          text=True, timeout=kw.pop('timeout',60), check=check, **kw)


def checkout(name):
    path=base/name
    (path/'scripts').mkdir(parents=True)
    for script in ['dev.py','package.py']:
        shutil.copy2(repo/'scripts'/script,path/'scripts'/script)
    (path/'crates/ui').mkdir(parents=True)
    (path/'crates/ui/assets').symlink_to(repo/'crates/ui/assets')
    (path/'third_party').symlink_to(repo/'third_party')
    (path/'THIRD_PARTY_NOTICES.md').symlink_to(repo/'THIRD_PARTY_NOTICES.md')
    (path/'target').mkdir()
    (path/'target/debug').symlink_to(repo/'target/debug')
    for git_args in [('init','-b','main'),('config','user.name','Acceptance'),
                     ('config','user.email','acceptance@example.invalid'),
                     ('config','core.hooksPath','/dev/null'),('config','commit.gpgsign','false')]:
        command(['git','-C',path,*git_args])
    (path/'seed').write_text('fixture\n')
    command(['git','-C',path,'add','seed'])
    command(['git','-C',path,'commit','-m','Seed'])
    return path


def dev(path, *argv, check=True, extra=None):
    r=command([sys.executable,path/'scripts/dev.py',*argv],check=check,env={**env,**(extra or {})})
    log.write(r.stdout+r.stderr);log.flush()
    return r


def info(path, extra=None):
    return json.loads(dev(path,'info',extra=extra).stdout)


def seed(instance):
    home=Path(instance['home']);(home/'config').mkdir(parents=True,exist_ok=True)
    (home/'config/agents.json').write_text(json.dumps({'hc':shutil.which('hc'),
        'hc_session':core['HC_SESSION'],'hc_env':core,'default_agent':'developer',
        'profiles':[{'id':'developer','preset':'codex'},{'id':'reviewer','preset':'codex'}]}))


def call(instance, method, params=None, check=True):
    r=command([cli,'--home',instance['home'],'--full','call',method,json.dumps(params or {})],check=check)
    return json.loads(r.stdout) if r.returncode==0 else r


def ready(instance):
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        r=call(instance,'app.ping',check=False)
        if isinstance(r,dict):return r
        time.sleep(.1)
    raise RuntimeError('Service did not become ready: '+instance['home'])


def start(path):
    instance=info(path);seed(instance)
    dev(path,'start',*(() if args.native else ('--no-ui',)))
    ping=ready(instance)
    assert ping['updates_disabled'] is True
    assert call(instance,'update.check',check=False).returncode!=0
    assert call(instance,'app.shutdown',{'expected_pid':ping['pid']+1},check=False).returncode!=0
    assert call(instance,'app.ping')['pid']==ping['pid']
    return instance,ping


def stopped(instance):
    assert not isinstance(call(instance,'app.ping',check=False),dict)
    for role in ('ui','server'):
        assert not (Path(instance['directory'])/(role+'.json')).exists()


paths=[]
foreign=None
try:
    a_repo=checkout('a');paths.append(a_repo)
    a, a_ping=start(a_repo)
    terminal=call(a,'terminal.create',{'cwd':str(base)})
    binding=terminal['runtime']
    def hc(*argv):return json.loads(command([shutil.which('hc'),*argv]).stdout)
    before=hc('get',binding['terminal'],'--json')
    call(a,'utilities.configure',{'theme':'dracula'})
    command(['git','-C',a_repo,'switch','-c','feature/editor'])
    b,b_ping=start(a_repo)
    assert a['home']!=b['home'] and a['app_id']!=b['app_id']
    assert call(a,'app.ping')['pid']==a_ping['pid']
    other_repo=checkout('b');paths.append(other_repo)
    other=info(other_repo);seed(other)
    dev(other_repo,'start','--no-ui')
    other_ping=ready(other)
    assert other['app_id']!=a['app_id']
    assert dev(other_repo,'start','--no-ui',check=False,extra={'CANBAN_HOME':a['home']}).returncode!=0
    assert call(a,'app.ping')['pid']==a_ping['pid']
    # No inherited shell home: the packaged Agent CLI resolves its own binding.
    dev_bin_rel = 'CanbanDev.app/Contents/MacOS' if sys.platform == 'darwin' else 'CanbanDev/bin'
    packaged=Path(a['directory'])/dev_bin_rel/'canban'
    assert json.loads(command([packaged,'call','app.ping','{}']).stdout)['home']==a['home']
    metadata=dict(base=str(base),checkout=str(a_repo),instances=[a,b],other=other,core=core)
    (base/'native.json').write_text(json.dumps(metadata,indent=2))
    print('DEV_ACCEPTANCE_BASE='+str(base),flush=True)
    if args.native:
        print('Create '+str(base/'continue')+' after native inspection to finish.',flush=True)
        while not (base/'continue').exists():time.sleep(.2)
    dev(a_repo,'stop')
    stopped(b)
    if args.native:
        # Finder/LaunchServices or direct bundle launch supplies no CANBAN_HOME: both UI
        # and auto-started server must resolve the bundle binding and register for dev-stop.
        if sys.platform == 'darwin':
            command(['/usr/bin/open','-n',Path(b['directory'])/'CanbanDev.app'])
        else:
            subprocess.Popen([str(Path(b['directory'])/'CanbanDev/bin/canban-ui')],
                             stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
        ready(b)
        deadline=time.monotonic()+15
        while True:
            registration=Path(b['directory'])/'ui.json'
            if registration.exists() and json.loads(registration.read_text()).get('ready'):break
            if time.monotonic()>=deadline:raise RuntimeError('Finder-launched development UI did not become ready')
            time.sleep(.1)
        dev(a_repo,'stop');stopped(b)
    dev(a_repo,'stop')
    assert call(a,'app.ping')['pid']==a_ping['pid']
    assert hc('get',binding['terminal'],'--json')['pid']==before['pid']
    # Restart main after a branch switch; keep its records and PTY.
    command(['git','-C',a_repo,'switch','main'])
    dev(a_repo,'start','--no-ui')
    assert call(a,'app.ping')['pid']!=a_ping['pid']
    assert call(a,'utilities.state')['preferences']['theme']=='dracula'
    assert hc('get',binding['terminal'],'--json')['pid']==before['pid']
    command(['git','-C',a_repo,'switch','feature/editor'])
    dev(a_repo,'start','--no-ui')
    b_current=ready(b)
    dev(a_repo,'stop','--branch','main')
    stopped(a)
    assert call(b,'app.ping')['pid']==b_current['pid']
    # A stale/reused PID record must never authorize shutdown of another service.
    record_path=Path(b['directory'])/'server.json';original=record_path.read_bytes()
    record_path.write_bytes((Path(other['directory'])/'server.json').read_bytes())
    assert dev(a_repo,'stop',check=False).returncode!=0
    assert call(b,'app.ping')['pid']==b_current['pid']
    assert call(other,'app.ping')['pid']==other_ping['pid']
    record_path.write_bytes(original)
    assert dev(a_repo,'start','--no-ui',check=False,extra={'CANBAN_ENDPOINT':str(Path(other['home'])/'app.sock')}).returncode!=0
    assert call(other,'app.ping')['pid']==other_ping['pid']
    dev(a_repo,'stop','--all')
    stopped(a);stopped(b)
    assert call(other,'app.ping')['pid']==other_ping['pid']
    launches=[subprocess.Popen([sys.executable,str(a_repo/'scripts/dev.py'),'start','--no-ui'],
              env=env,stdout=log,stderr=log) for _ in range(2)]
    for launch in launches:assert launch.wait(timeout=60)==0
    ready(b)
    executable=str(Path(b['directory'])/dev_bin_rel/'canban')
    if sys.platform.startswith('linux'):
        running=[]
        for entry in os.listdir('/proc'):
            if entry.isdigit():
                try:
                    exe=os.readlink(f'/proc/{entry}/exe')
                    running.append(exe[:-10] if exe.endswith(' (deleted)') else exe)
                except OSError:
                    pass
        assert sum(line==executable for line in running)==1
    else:
        processes=command(['/bin/ps','-ww','-axo','comm=']).stdout.splitlines()
        assert sum(line.strip()==executable for line in processes)==1
    dev(a_repo,'stop');stopped(b)
    # Refuse an unrecorded server even if it uses a previously managed home.
    foreign_env={**env,'CANBAN_HOME':b['home']}
    foreign=subprocess.Popen([str(cli),'serve'],env=foreign_env,stdout=log,stderr=log)
    foreign_ping=ready(b)
    assert dev(a_repo,'stop',check=False).returncode!=0
    assert call(b,'app.ping')['pid']==foreign_ping['pid']
    call(b,'app.shutdown');foreign.wait(timeout=15);foreign=None
    # Explicit home overrides branch selection and remains stoppable after switching.
    custom_env={'CANBAN_HOME':str(base/'custom')}
    custom=info(a_repo,custom_env);seed(custom)
    dev(a_repo,'start','--no-ui',extra=custom_env)
    command(['git','-C',a_repo,'switch','main'])
    assert info(a_repo,custom_env)['key']==custom['key']
    dev(a_repo,'stop',extra=custom_env);stopped(custom)
    dev(a_repo,'stop','--all')
    result=dict(branch_isolation=True,checkout_isolation=True,branch_stop=True,
        stop_idempotent=True,all_scoped_to_checkout=True,unrecorded_server_preserved=True,
        wrong_process_record_preserved=True,endpoint_untouched=True,home_override=True,
        pty_pid=before['pid'],pty_survived=True,native=args.native,paid_agents_started=0,
        simultaneous_start_serialized=True,foreign_checkout_home_refused=True,
        finder_home_binding=args.native)
    (base/'results.json').write_text(json.dumps(result,indent=2))
    print('DEV_INSTANCE_ACCEPTANCE_OK '+str(base),flush=True)
except BaseException:
    print('DEV_ACCEPTANCE_FAILED '+str(base),flush=True)
    print((base/'acceptance.log').read_text()[-6000:],flush=True)
    raise
finally:
    if foreign and foreign.poll() is None:
        try:call(b,'app.shutdown')
        except Exception:pass
        foreign.wait(timeout=15)
    for path in paths:
        r=dev(path,'stop','--all',check=False)
        if r.returncode:print('Cleanup status: '+r.stderr,flush=True)
    command([shutil.which('hc'),'server','stop'],check=False)
    log.close()
