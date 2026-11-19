#!/usr/bin/env python3
"""Opt-in live GitHub acceptance against an already authorized disposable repo.

Requires a running isolated AppServer, its Device Flow sign-in, a connected
test repository, and a seeded checkout. Never reads or prints API credentials.
GitHub CLI is used only to create/update test fixtures; application operations
use Canban's own GitHub App authorization.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--home', type=Path, required=True)
parser.add_argument('--repository', required=True)
parser.add_argument('--project-id', required=True)
parser.add_argument('--issue-number', type=int, required=True)
parser.add_argument('--allow-remote-writes', action='store_true')
parser.add_argument('--real-agent', action='store_true', help='Run real developer/reviewer Agents; otherwise only test source intake')
args = parser.parse_args()
if not args.allow_remote_writes:
    parser.error('Explicit --allow-remote-writes is required for fixture comments and PR delivery')
repo = Path(__file__).resolve().parents[1]
cli = repo/'target/debug/canban'
env = {k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}
checks = []

def command(argv, timeout=100):
    result = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return result.stdout.strip()

def call(method, params=None):
    return json.loads(command([str(cli),'--home',str(args.home),'--full','call',method,json.dumps(params or {})]))

def mutate(method, params):
    return call(method,{**params,'request_key':uuid.uuid4().hex})

def check(label):
    checks.append(label)
    print('PASSED '+label,flush=True)

def current(card):
    return call('card.get',{'card_id':card['id']})

def reviewed(snapshot,card_id):
    card=next(c for c in snapshot['cards'] if c['id']==card_id)
    requests=[r for r in snapshot['requests'] if r['card_id']==card_id]
    attempts=[a for a in snapshot['attempts'] if a['card_id']==card_id]
    return card['stage']=='review' and card.get('human_gate') and requests and all(r['state']=='completed' for r in requests) and attempts and all(a.get('close_state')=='closed' for a in attempts)

def wait_review(card_id):
    config=json.loads((args.home/'config/agents.json').read_text())
    core_env={**env,**config.get('hc_env',{})}
    trusted=set()
    for _ in range(600):
        snapshot=call('app.snapshot')
        for a in snapshot['attempts']:
            terminal=a.get('terminal')
            if not terminal or terminal in trusted:
                continue
            text=subprocess.run([config['hc'],'read',terminal],env=core_env,capture_output=True,text=True,timeout=15).stdout
            if 'Trust this folder?' in text and str(args.home.parent) in text and 'Trust and continue' in text:
                subprocess.run([config['hc'],'keys',terminal,'enter'],env=core_env,capture_output=True,check=True,timeout=15)
                trusted.add(terminal)
        failures=[r for r in snapshot['requests'] if r['card_id']==card_id and r['state'] in ('failed','unknown','needs_report','blocked')]
        if failures:
            raise RuntimeError('Agent requires inspection: '+json.dumps(failures))
        if reviewed(snapshot,card_id):
            return snapshot
        time.sleep(1)
    raise RuntimeError('Real Agent workflow did not reach completed human Review')

try:
    assert call('github.auth.status')['state']=='connected','Complete Device Flow first'
    call('github.auth.refresh')
    check('Device Flow credential refresh')
    project=mutate('project.github',{'project_id':args.project_id,'repository':args.repository,'intake':'all','delivery_mode':'github_pr'})
    call('github.sync',{'project_id':args.project_id})
    snapshot=call('app.snapshot')
    card=next(c for c in snapshot['cards'] if c['project_id']==args.project_id and c['content']['kind']=='github_issue' and c['content']['source']['number']==args.issue_number)
    assert card['stage']=='backlog' and 'tags' not in card
    assert not any(r['card_id']==card['id'] for r in snapshot['requests'])
    check('Automatic Open Issue intake is Backlog-only')
    instructions='Keep the change minimal. Preserve this local instruction through synchronization.'
    card=mutate('card.update',{'card_id':card['id'],'revision':card['revision'],'body':instructions})
    mutate('card.remove',{'card_id':card['id'],'revision':card['revision']})
    call('github.sync',{'project_id':args.project_id})
    assert all(c['id']!=card['id'] for c in call('app.snapshot')['cards'])
    rows=call('github.issue.search',{'project_id':args.project_id,'query':str(args.issue_number)})
    row=next(r for r in rows if r['issue']['number']==args.issue_number)
    assert row['removed'] and row['card_id']==card['id']
    restored=mutate('github.issue.add',{'project_id':args.project_id,'number':args.issue_number,'source_generation':row['source_generation']})
    assert restored['id']==card['id'] and restored['number']==card['number'] and restored['body']==instructions
    check('Local removal suppresses sync; explicit re-add restores identity/history')
    project=mutate('project.github',{'project_id':args.project_id,'repository':args.repository,'intake':'collaborators','delivery_mode':'github_pr'})
    call('github.sync',{'project_id':args.project_id})
    check('Real repository collaborator lookup and intake mode')
    # Markdown code fences avoid notifying an actual GitHub @codex user while
    # still exercising the local whitespace-based mention parser boundary.
    command(['gh','api',f'repos/{args.repository}/issues/{args.issue_number}/comments','-f','body=Canban acceptance fixture:\n```text\n@codex\n```'])
    time.sleep(2)
    call('github.sync',{'project_id':args.project_id})
    snapshot=call('app.snapshot')
    assert not any(r['card_id']==card['id'] for r in snapshot['requests'])
    assert not any(e['kind']=='github.comment' for e in call('timeline.list',{'card_id':card['id'],'limit':200})['events'])
    discussion=call('github.detail',{'reference':{'repository':args.repository,'kind':'issue','number':args.issue_number,'sha':None,'url':f'https://github.com/{args.repository}/issues/{args.issue_number}'},'section':'discussion','refresh':True})
    assert any('@codex' in c.get('body','') for c in discussion['data'])
    check('Remote comment is cache-backed discussion without durable Timeline import or dispatch')
    if args.real_agent:
        card=current(card)
        mutate('card.move',{'card_id':card['id'],'revision':card['revision'],'stage':'develop'})
        snapshot=wait_review(card['id'])
        card=current(card)
        assert Path(card['worktree'],'greeting.txt').read_text()=='hello from Canban\n'
        check('Real developer and reviewer consumed GitHub source context')
        preview=call('review.preview',{'card_id':card['id'],'target_branch':'main'})
        assert preview['merge_allowed'] and preview['files']
        payload={'card_id':card['id'],'revision':card['revision'],'token':preview['token'],'target_branch':'main','request_key':uuid.uuid4().hex}
        published=call('github.pr.publish',payload)
        assert published['state']=='published',published
        assert call('github.pr.publish',payload)==published
        pr=call('github.pr.refresh',{'card_id':card['id']})
        assert pr['number']==published['number'] and pr['head']==published['head']
        check('Reviewed commit/push/PR publication and durable receipt replay')
        card=current(card)
        payload={'card_id':card['id'],'revision':card['revision'],'head':pr['head'],'request_key':uuid.uuid4().hex}
        result=call('github.pr.merge',payload)
        assert result['state']=='merged',result
        assert call('github.pr.merge',payload)==result
        card=current(card)
        assert card['pull_request']['merged'] and card['delivery']
        assert card.get('archived_at') and card.get('archive')
        assert (args.home/card['archive']).exists()
        check('GitHub merge synchronization and automatic Card Archive')
    evidence={'repository':args.repository,'project_id':args.project_id,'card_id':card['id'],'checks':checks,'snapshot':call('app.snapshot')}
    path=args.home.parent/'github-evidence.json';path.write_text(json.dumps(evidence,indent=2))
    print('GITHUB_ACCEPTANCE_PASSED\nEVIDENCE='+str(path),flush=True)
except Exception as error:
    (args.home.parent/'github-failure.json').write_text(json.dumps({'error':str(error),'checks':checks},indent=2))
    raise
