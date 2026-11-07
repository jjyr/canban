#!/usr/bin/env python3
"""Disposable remote Git + real Herdr PTYs: global slots, depth-first work and human gates."""
from pathlib import Path
import json, os, subprocess, tempfile, time
repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-scheduling-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
env = {**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}, 'CANBAN_HOME':str(home)}
core_env = {'HC_SESSION':'scheduling-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
log = open(base/'server.log','w')
server = subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
def call(method, params=None):
    result = subprocess.run([str(cli),'call',method,json.dumps(params or {})],env=env,capture_output=True,text=True,timeout=90)
    if result.returncode: raise RuntimeError(result.stderr)
    return json.loads(result.stdout)
def wait(predicate, label, timeout=60):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        if server.poll() is not None: raise RuntimeError((base/'server.log').read_text())
        try: snapshot=call('app.snapshot')
        except RuntimeError: time.sleep(.1);continue
        if predicate(snapshot): return snapshot
        time.sleep(.15)
    raise RuntimeError(label+' timed out; evidence '+str(base))
def git(cwd,*args):
    clean={k:v for k,v in os.environ.items() if k not in ('GIT_DIR','GIT_WORK_TREE','GIT_INDEX_FILE')}
    return subprocess.run(['git','-C',str(cwd),*args],env=clean,capture_output=True,text=True,check=True).stdout.strip()
def send(run,text):
    subprocess.run(['hc','send',run['terminal'],text,'--json'],env={**os.environ,**core_env},capture_output=True,text=True,check=True,timeout=20)
def card(snapshot,id): return next(c for c in snapshot['cards'] if c['id']==id)
def run_for(snapshot,id,stage='develop'):
    requests={r['id'] for r in snapshot['requests'] if r['card_id']==id and r['stage']==stage}
    return next(a for a in snapshot['attempts'] if a['request_id'] in requests and a.get('provider_session_id'))
try:
    wait(lambda _:True,'startup')
    config=json.loads((home/'config/agents.json').read_text());config.update(hc_env=core_env,hc_session=core_env['HC_SESSION'])
    for profile in config['profiles']:
        profile.update(preset=None,exit={'type':'input','text':'/exit'},session_instruction='',argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'{prompt}'],resume_argv=['/usr/bin/python3',str(repo/'scripts/interactive-fixture.py'),'--resume','{session_id}','{prompt}'])
    (home/'config/agents.json').write_text(json.dumps(config));call('config.reload')
    source=base/'source';source.mkdir();remote=base/'remote.git';writer=base/'writer'
    git(source,'init','--bare',str(remote));git(source,'init','-b','main')
    for k,v in [('user.name','Acceptance'),('user.email','acceptance@example.invalid'),('core.hooksPath','/dev/null'),('commit.gpgsign','false')]:git(source,'config',k,v)
    (source/'seed').write_text('seed');git(source,'add','.');git(source,'commit','-m','seed');git(source,'remote','add','origin',str(remote));git(source,'push','-u','origin','main')
    git(source,'clone','-b','main',str(remote),str(writer))
    for k,v in [('user.name','Acceptance'),('user.email','acceptance@example.invalid'),('core.hooksPath','/dev/null'),('commit.gpgsign','false')]:git(writer,'config',k,v)
    (writer/'latest').write_text('remote context');git(writer,'add','.');git(writer,'commit','-m','remote update');git(writer,'push','origin','main');latest=git(writer,'rev-parse','HEAD')
    projects=[call('project.create',{'name':'Board '+str(i),'directory':str(source)}) for i in range(2)]
    cards=[]
    for i in range(9):
        c=call('card.create',{'project_id':projects[i%2]['id'],'title':'Concurrent '+str(i)})
        cards.append(c['id']);call('card.move',{'card_id':c['id'],'revision':1,'stage':'develop'})
    snapshot=wait(lambda s:len(s['attempts'])==8 and all(a.get('provider_session_id') for a in s['attempts']),'eight running Agents')
    assert call('scheduler.status')['active']==8 and call('scheduler.status')['max_agents']==8
    assert git(source,'rev-parse','HEAD')==latest
    for c in snapshot['cards']:
        if c['worktree']:
            assert c['branch'] and c['base_commit']==latest and Path(c['worktree'],'latest').exists()
            assert git(c['worktree'],'branch','--show-current')==c['branch']
    assert not any(a['card_id']==cards[8] for a in snapshot['attempts'])
    print('GLOBAL_EIGHT_AND_REMOTE_SYNC_OK',flush=True)
    send(run_for(snapshot,cards[0]),'finish')
    snapshot=wait(lambda s:card(s,cards[0])['human_gate'] and any(a['card_id']==cards[8] and a.get('provider_session_id') for a in s['attempts']),'Review before ninth Develop')
    review=run_for(snapshot,cards[0],'review');ninth=run_for(snapshot,cards[8])
    assert review['created_at'] < ninth['created_at']
    assert call('scheduler.status')['active']<=8
    count=len(snapshot['requests']);call('card.attention.clear',{'card_id':cards[0]})
    assert call('card.get',{'card_id':cards[0]})['human_gate']
    send(run_for(snapshot,cards[1]),'block')
    snapshot=wait(lambda s:card(s,cards[1])['human_gate'],'blocked Card')
    send(run_for(snapshot,cards[1]),'finish')
    snapshot=wait(lambda s:any(r['card_id']==cards[1] and r['state']=='completed' for r in s['requests']),'late completion')
    assert card(snapshot,cards[1])['stage']=='develop'
    assert not any(r['card_id']==cards[1] and r['stage']=='review' for r in snapshot['requests'])
    server.terminate();server.wait(timeout=10)
    server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
    snapshot=wait(lambda s:len(s['requests'])==count and card(s,cards[1])['human_gate'],'gate retained after restart')
    c=card(snapshot,cards[1]);call('card.continue',{'card_id':c['id'],'revision':c['revision']})
    snapshot=wait(lambda s:card(s,cards[1])['stage']=='review' and card(s,cards[1])['human_gate'],'human approved progression')
    print('DEPTH_FIRST_AND_HUMAN_GATE_OK',flush=True)
    active=call('scheduler.status')['active'];working_ids={a['id'] for a in snapshot['attempts'] if ((a.get('observed') or {}).get('status') or {}).get('state')=='working'};call('scheduler.configure',{'max_agents':2})
    extra=call('card.create',{'project_id':projects[0]['id'],'title':'Wait for lower limit'})
    call('card.move',{'card_id':extra['id'],'revision':1,'stage':'develop'})
    snapshot=call('app.snapshot');assert not any(a['card_id']==extra['id'] for a in snapshot['attempts'])
    assert 2 <= call('scheduler.status')['active'] <= active
    assert all(a['observed']['state']!='finished' for a in snapshot['attempts'] if a['id'] in working_ids)
    call('scheduler.configure',{'max_agents':8})
    snapshot=wait(lambda s:any(a['card_id']==extra['id'] and a.get('provider_session_id') for a in s['attempts']),'limit increase drains queue')
    (base/'evidence.json').write_text(json.dumps({'snapshot':snapshot,'latest_base':latest,'review_before_develop':[review['id'],ninth['id']],'status':call('scheduler.status')},indent=2))
    print('LIMIT_EDIT_OK\nSCHEDULING_ACCEPTANCE_PASSED\nBASE='+str(base),flush=True)
finally:
    if server.poll() is None:server.terminate();server.wait(timeout=10)
    subprocess.run(['hc','server','stop'],env={**os.environ,**core_env},capture_output=True,timeout=20)
    log.close()
