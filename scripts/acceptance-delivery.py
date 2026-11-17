#!/usr/bin/env python3
"""Deliver the real, already-reviewed workflow fixtures without another model call.
Usage: mise exec -- python3 scripts/acceptance-delivery.py /private/tmp/cb-real-workflows-...
"""
from pathlib import Path
import json, os, subprocess, sys, time
repo = Path(__file__).resolve().parents[1]
base = Path(sys.argv[1]).resolve()
evidence = json.loads((base / 'evidence.json').read_text())
env = {**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}, 'CANBAN_HOME': str(base / 'home')}
cli = repo / 'target/debug/canban'
log = open(base / 'delivery-server.log', 'w')
server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)
def call(method, params=None):
    result = subprocess.run([str(cli), "--full",'call', method, json.dumps(params or {})], env=env, capture_output=True, text=True, timeout=90)
    if result.returncode: raise RuntimeError(result.stderr)
    return json.loads(result.stdout)
def git(source,*args):
    clean = {k:v for k,v in os.environ.items() if k not in ('GIT_DIR','GIT_WORK_TREE','GIT_INDEX_FILE')}
    return subprocess.run(['git','-C',source,*args],env=clean,capture_output=True,text=True,check=True).stdout.strip()
try:
    until=time.monotonic()+15
    while time.monotonic()<until:
        try: call('app.ping');break
        except RuntimeError: time.sleep(.1)
    deliveries=[]
    for card_id,source in zip(evidence['cards'],evidence['sources']):
        card=call('card.get',{'card_id':card_id})
        assert card['stage']=='review' and not card['archived_at']
        preview=call('review.preview',{'card_id':card_id,'target_branch':'main'})
        assert preview['merge_allowed'] and preview['files'] and preview['patch']
        (base / ('diff-'+card_id+'.patch')).write_text(preview['patch'])
        before=git(source,'rev-parse','HEAD')
        # The explicit command is what accepts the complete reviewed checkout.
        request={'card_id':card_id,'revision':card['revision'],'target_branch':'main','token':preview['token'],'message':'Accept Canban #'+str(card['number'])+': '+card['title'],'request_key':'delivery-'+card_id}
        result=call('review.merge',request)
        assert result['state']=='merged',result
        assert call('review.merge',request)==result
        assert git(source,'rev-parse','HEAD')==result['commit']!=before
        assert git(source,'status','--porcelain')==''
        checks=subprocess.run(['/usr/bin/python3','-m','unittest','-v'],cwd=source,capture_output=True,text=True,check=True)
        archived=call('card.get',{'card_id':card_id})
        assert archived['archive'] and archived['archived_at'] and archived['delivery']['commit']==result['commit']
        assert not Path(card['worktree']).exists()
        snapshot=call('app.snapshot')
        report=next(r for r in snapshot['reports'] if r['card_id']==card_id and r.get('path'))
        retained=call('resource.read',{'card_id':card_id,'path':report['path']})['body']
        assert call('resource.read',{'card_id':card_id,'path':report['path']})['body']==retained
        call('card.restore',{'card_id':card_id,'revision':archived['revision']})
        assert git(source,'rev-parse','HEAD')==result['commit']
        deliveries.append({'card_id':card_id,'before':before,'result':result,'files':preview['files'],'checks':checks.stdout+checks.stderr,'archive':archived['archive']})
    (base / 'delivery-evidence.json').write_text(json.dumps(deliveries,indent=2))
    print('REAL_DELIVERY_ACCEPTANCE_PASSED\nBASE='+str(base),flush=True)
finally:
    server.terminate();server.wait(timeout=10);log.close()
