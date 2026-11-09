#!/usr/bin/env python3
"""Explicit real-model opt-in: human input, cross-Board work and review/rework.
Validates live replies, exact resume, database Notes, merge/archive and restore.
"""
from pathlib import Path
import json, os, subprocess, tempfile, time

repo = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix='cb-real-workflows-', dir='/private/tmp'))
home = base / 'home'
cli = repo / 'target/debug/canban'
env = {**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')}, 'CANBAN_HOME': str(home)}
core_env = {'HC_SESSION': 'workflow-acceptance', 'HC_SOCKET': str(base / 'hc.sock'),
            'XDG_STATE_HOME': str(base / 'hc-state'), 'XDG_CONFIG_HOME': str(base / 'hc-config')}
log = open(base / 'server.log', 'w')
server = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)
trusted = set()
observations = []
last_progress = 0.0
overlap = False

def call(method, params=None):
    result = subprocess.run([str(cli), 'call', method, json.dumps(params or {})], env=env,
                            capture_output=True, text=True, timeout=90)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)

def wait(predicate, label, timeout=420):
    global last_progress, overlap
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        if server.poll() is not None:
            raise RuntimeError((base / 'server.log').read_text())
        try:
            snapshot = call('app.snapshot')
        except RuntimeError:
            time.sleep(.2)
            continue
        for run in snapshot['attempts']:
            terminal = run.get('terminal')
            if not terminal or terminal in trusted or run.get('close_state') == 'closed':
                continue
            view = subprocess.run(['hc', 'read', terminal], env={**os.environ, **core_env},
                                  capture_output=True, text=True, timeout=15).stdout
            if 'Trust this folder?' in view and str(base) in view and 'Trust and continue' in view:
                subprocess.run(['hc', 'keys', terminal, 'enter'], env={**os.environ, **core_env},
                               capture_output=True, check=True, timeout=15)
                trusted.add(terminal)
        active = [r for r in snapshot['requests'] if r['state'] in ('starting','running','working')]
        bound = {a['request_id'] for a in snapshot['attempts'] if a.get('terminal')}
        simultaneous = len({r['project_id'] for r in active if r['id'] in bound}) > 1
        overlap = overlap or simultaneous
        observation = {'at':time.time(),'phase':label,'overlap':simultaneous,'requests':[{'id':r['id'],'stage':r['stage'],'profile':r['profile']['id'],'state':r['state']} for r in snapshot['requests']]}
        observations.append(observation)
        if time.monotonic()-last_progress > 15:
            print('PROGRESS '+json.dumps(observation),flush=True);last_progress=time.monotonic()
        failures=[r for r in snapshot['requests'] if r['state'] in ('failed','unknown','needs_report')]
        if failures:
            (base/'failure-snapshot.json').write_text(json.dumps(snapshot,indent=2))
            raise RuntimeError(label+': execution needs inspection: '+json.dumps(failures))
        if predicate(snapshot):
            return snapshot
        time.sleep(.5)
    raise RuntimeError(f'{label} timed out; evidence: {base}')

def git(directory, *args):
    clean = {k: v for k, v in os.environ.items() if k not in ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE')}
    return subprocess.run(['git', '-C', str(directory), *args], env=clean, check=True,
                          capture_output=True, text=True).stdout.strip()

def project(name, files):
    source = base / name
    source.mkdir()
    for filename, body in files.items():
        (source / filename).write_text(body)
    (source / '.gitignore').write_text('__pycache__/\n')
    git(source, 'init', '-b', 'main')
    git(source, 'config', 'user.name', 'Canban Acceptance')
    git(source, 'config', 'user.email', 'acceptance@example.invalid')
    git(source, 'config', 'core.hooksPath', '/dev/null')
    git(source, 'config', 'commit.gpgsign', 'false')
    git(source, 'add', '.')
    git(source, 'commit', '-m', 'Seed acceptance repository')
    workflow_path = home / 'config/workflows/default.json'
    workflow = json.loads(workflow_path.read_text())
    workflow['workspace']['sync_base'] = False
    workflow_path.write_text(json.dumps(workflow))
    return call('project.create', {'name': name, 'directory': str(source)}), source

def reviewed(snapshot, card_id):
    requests = [r for r in snapshot['requests'] if r['card_id'] == card_id]
    attempts = [a for a in snapshot['attempts'] if a['card_id'] == card_id]
    card = next(c for c in snapshot['cards'] if c['id'] == card_id)
    return (card['stage'] == 'review' and requests and all(r['state'] == 'completed' for r in requests)
            and attempts and all(a['close_state'] == 'closed' for a in attempts))

try:
    wait(lambda _: True, 'startup', 20)
    config = json.loads((home / 'config/agents.json').read_text())
    config.update(hc_env=core_env, hc_session=core_env['HC_SESSION'])
    (home / 'config/agents.json').write_text(json.dumps(config, indent=2))
    call('config.reload')
    call('scheduler.configure', {'idle_minutes': 1})
    p1, source1 = project('human-input', {'slug.py': 'def slug(text):\n    return text\n',
        'test_slug.py': 'import unittest\nfrom slug import slug\nclass Tests(unittest.TestCase):\n    def test_words(self): self.assertEqual(slug("Hello World"),"hello-world")\n    def test_unicode(self): self.assertEqual(slug("  CAFÉ  世界  "),"café-世界")\n'})
    p2, source2 = project('review-rework', {'totals.py': 'def total(values):\n    return 0\n',
        'test_totals.py': 'import unittest\nfrom totals import total\nclass Tests(unittest.TestCase):\n    def test_total(self): self.assertEqual(total([2,3]),5)\n    def test_empty(self): self.assertEqual(total([]),0)\n'})
    marker='REAL_CONTEXT_'+base.name.replace('-','_')
    note=call('note.save',{'project_id':p1['id'],'title':'Acceptance context.md','body':'# Acceptance context\n'+marker})
    common = ' Use current Canban session/report instructions. Run /usr/bin/python3 -m unittest -v. Keep source code and tests in this Card worktree. Do not commit or merge; a human will accept the reviewed changes later.'
    c1 = call('card.create', {'project_id': p1['id'], 'title': 'Slug function needs a human separator choice',
        'body': 'Implement slug(text): lowercase Unicode text, trim and collapse whitespace into a separator. In the FIRST Develop invocation, before implementation, report blocked and ask the human whether the separator should be - or _. Wait for the human @developer answer without blocking a shell command. After the answer, implement, test and report completed. Review should inspect the result and report normally.' + common})
    c2 = call('card.create', {'project_id': p2['id'], 'title': 'Total values, then handle a review change',
        'body': 'Implement total(values), summing an iterable of integers with zero for an empty iterable. This initial version permits negative values. Produce an ordinary review report in Review.' + common})
    print('REAL_WORKFLOWS_HOME=' + str(home), flush=True)
    (base/'metadata.json').write_text(json.dumps({'base':str(base),'home':str(home),'core_env':core_env,'cards':[c1['id'],c2['id']],'sources':[str(source1),str(source2)],'note_id':note['id'],'marker':marker},indent=2))
    for card in (c1, c2):
        call('card.move', {'card_id': card['id'], 'revision': card['revision'], 'stage': 'develop'})
    s = wait(lambda s: any(r['card_id'] == c1['id'] and r['state'] == 'blocked' for r in s['requests']) and reviewed(s, c2['id']), 'blocked and first review')
    blocked = next(c for c in s['cards'] if c['id'] == c1['id'])
    assert blocked['attention'] and any(n['card_id'] == c1['id'] for n in s['notices'])
    assert blocked['stage'] == 'develop'
    blocked_cwd=Path(next(a['cwd'] for a in s['attempts'] if a['card_id']==c1['id']))
    assert (blocked_cwd/'slug.py').read_text()=='def slug(text):\n    return text\n'
    assert overlap, 'Expected overlapping bound Agents on two Boards'
    call('comment.add', {'card_id': c1['id'], 'body': '@developer The human chooses the hyphen (-) separator. Please acknowledge with canban reply using this Question ID, include the exact marker from Acceptance context.md, then append one line saying `Human chose hyphen` to that database Note via canban note write with its current revision (preserve its marker). Implement the original slug task, run tests and report completed.'})
    developer = next(a for a in s['attempts'] if a['card_id'] == c2['id'] and next(r for r in s['requests'] if r['id'] == a['request_id'])['profile']['id'] == 'developer')
    before_sid = developer['provider_session_id']
    rework = next(c for c in s['cards'] if c['id'] == c2['id'])
    new_body = 'Review feedback changes the requirement: total(values) must raise ValueError when ANY input value is negative. Preserve normal and empty sums. Add a regression test proving negative input is rejected, implement this change and rerun tests. In Review verify the revised requirement and produce a report.' + common
    rework = call('card.update', {'card_id': c2['id'], 'revision': rework['revision'], 'title': rework['title'], 'body': new_body, 'tags': []})
    call('comment.add', {'card_id': c2['id'], 'body': 'Human review: reject negative values. Returning this Card to Develop for rework.'})
    rework = next(c for c in call('app.snapshot')['cards'] if c['id'] == c2['id'])
    call('card.move', {'card_id': c2['id'], 'revision': rework['revision'], 'stage': 'develop'})
    s = wait(lambda s:any(r['card_id']==c1['id'] and r['stage']=='develop' and r['state']=='completed' for r in s['requests']), 'blocked task completed; human gate retained')
    blocked_done=next(c for c in s['cards'] if c['id']==c1['id'])
    assert blocked_done['stage']=='develop' and blocked_done['human_gate']
    assert not any(r['card_id']==c1['id'] and r['stage']=='review' for r in s['requests'])
    call('card.continue',{'card_id':c1['id'],'revision':blocked_done['revision']})
    s = wait(lambda s: reviewed(s, c1['id']) and reviewed(s, c2['id']) and len([r for r in s['requests'] if r['card_id'] == c2['id']]) == 4, 'human approval and second review')
    assert all(q['state'] == 'replied' for q in s['questions'])
    dev_runs = [a for a in s['attempts'] if a['card_id'] == c2['id'] and next(r for r in s['requests'] if r['id'] == a['request_id'])['profile']['id'] == 'developer']
    assert len(dev_runs) == 2 and all(a['provider_session_id'] == before_sid for a in dev_runs)
    note_now=call('note.read',{'note_id':note['id']})
    assert marker in note_now['body'] and 'Human chose hyphen' in note_now['body'] and note_now['revision']>note['revision']
    assert any(e.get('note_id')==note['id'] and e.get('request_id') for e in s['note_events'])
    question=next(q for q in s['questions'] if q['card_id']==c1['id'])
    assert marker in call('resource.read',{'card_id':c1['id'],'path':question['reply_path']})['body']
    first_dev=next(a for a in s['attempts'] if a['card_id']==c1['id'] and next(r for r in s['requests'] if r['id']==a['request_id'])['profile']['id']=='developer')
    call('comment.add',{'card_id':c1['id'],'body':'@developer Follow-up only. Do not change code or Notes, and keep the Card in Review. Recall the exact marker from our earlier conversation and the human separator choice. Publish both via the supplied Canban reply Question ID, then explicitly report this follow-up completed. This must use the existing conversation.'})
    s=wait(lambda s:len([a for a in s['attempts'] if a['card_id']==c1['id']])==3 and all(q['state']=='replied' for q in s['questions']) and all(a['close_state']=='closed' for a in s['attempts']), 'exact developer Resume after review')
    resumed=next(a for a in reversed(s['attempts']) if a['card_id']==c1['id'] and next(r for r in s['requests'] if r['id']==a['request_id'])['profile']['id']=='developer')
    assert resumed['id']!=first_dev['id'] and resumed['terminal']!=first_dev['terminal'] and resumed['provider_session_id']==first_dev['provider_session_id'] and resumed['cwd']==first_dev['cwd']
    assert next(c for c in s['cards'] if c['id']==c1['id'])['stage']=='review'
    followup=next(q for q in reversed(s['questions']) if q['card_id']==c1['id'])
    assert marker in call('resource.read',{'card_id':c1['id'],'path':followup['reply_path']})['body']
    print('LIVE_REPLY_NOTE_HUMAN_GATE_AND_EXACT_RESUME_PASSED',flush=True)
    checks = {}
    for card, source in [(c1, source1), (c2, source2)]:
        cwd = Path(next(c for c in s['cards'] if c['id'] == card['id'])['worktree'])
        result = subprocess.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=cwd, capture_output=True, text=True, check=True)
        checks[card['id']] = result.stdout + result.stderr
        assert git(source, 'status', '--porcelain') == ''
    cwd = Path(next(c for c in s['cards'] if c['id'] == c2['id'])['worktree'])
    subprocess.run(['/usr/bin/python3', '-c', 'from totals import total\ntry: total([1,-1])\nexcept ValueError: pass\nelse: raise AssertionError("negative input accepted")'], cwd=cwd, check=True)
    deliveries=[]
    for card,source in [(c1,source1),(c2,source2)]:
        current=call('card.get',{'card_id':card['id']})
        plan=call('review.preview',{'card_id':card['id'],'target_branch':'main'})
        assert plan['merge_allowed'] and plan['patch'] and plan['files'],plan
        (base/('diff-'+card['id']+'.patch')).write_text(plan['patch'])
        request={'card_id':card['id'],'revision':current['revision'],'token':plan['token'],'target_branch':'main','request_key':'accept-real-'+card['id']}
        result=call('review.merge',request);assert result['state']=='merged',result
        assert call('review.merge',request)==result
        assert git(source,'rev-parse','HEAD')==result['commit']
        subprocess.run(['/usr/bin/python3','-m','unittest','-v'],cwd=source,check=True,capture_output=True,text=True)
        assert git(source,'status','--porcelain')==''
        current=call('card.get',{'card_id':card['id']})
        reports=[r for r in s['reports'] if r['card_id']==card['id'] and r.get('path')]
        bodies={r['path']:call('resource.read',{'card_id':card['id'],'path':r['path']})['body'] for r in reports}
        archived=call('card.archive',{'card_id':card['id'],'revision':current['revision']})
        assert (home/archived['path']).exists()
        assert not Path(current['worktree']).exists(), 'Clean delivered worktree should be removed'
        for path,body in bodies.items():assert call('resource.read',{'card_id':card['id'],'path':path})['body']==body
        archived_card=call('card.get',{'card_id':card['id']})
        call('card.restore',{'card_id':card['id'],'revision':archived_card['revision']})
        assert git(source,'rev-parse','HEAD')==result['commit']
        assert len(call('app.snapshot')['requests'])==len(s['requests']), 'Restore must not dispatch work'
        restored=call('card.get',{'card_id':card['id']})
        if card['id']==c1['id']:
            prior=next(a for a in reversed(s['attempts']) if a['card_id']==card['id'] and next(r for r in s['requests'] if r['id']==a['request_id'])['profile']['id']=='developer')
            before_count=len(s['attempts']);before_note=call('note.read',{'note_id':note['id']})
            call('comment.add',{'card_id':card['id'],'body':'@developer After Archive and Restore, recall our exact context marker and the human separator choice. Reply to this Question and explicitly report completion. Do not modify code or Notes.'})
            s=wait(lambda s:len(s['attempts'])==before_count+1 and all(a['close_state']=='closed' for a in s['attempts']) and all(q['state']=='replied' for q in s['questions']), 'post-Archive exact Resume')
            resumed=s['attempts'][-1]
            assert resumed['provider_session_id']==prior['provider_session_id'] and resumed['cwd']==prior['cwd'] and resumed['terminal']!=prior['terminal']
            assert Path(resumed['cwd']).is_dir() and git(resumed['cwd'],'status','--porcelain')==''
            assert git(source,'rev-parse','HEAD')==result['commit']
            after_note=call('note.read',{'note_id':note['id']});assert (after_note['revision'],after_note['body'])==(before_note['revision'],before_note['body'])
            question=next(q for q in reversed(s['questions']) if q['card_id']==card['id'])
            assert marker in call('resource.read',{'card_id':card['id'],'path':question['reply_path']})['body']
            restored=call('card.get',{'card_id':card['id']})
            print('POST_ARCHIVE_REAL_RESUME_PASSED',flush=True)
        final_archive=call('card.archive',{'card_id':card['id'],'revision':restored['revision']})
        deliveries.append({'card_id':card['id'],'commit':result['commit'],'archive':final_archive['path']})
    print('REAL_REVIEW_REWORK_MERGE_ARCHIVE_RESTORE_PASSED',flush=True)
    evidence = {'snapshot': s, 'checks': checks, 'core_env': core_env, 'cards': [c1['id'], c2['id']], 'sources': [str(source1), str(source2)],'deliveries':deliveries,'observations':observations,'overlap':overlap}
    (base / 'evidence.json').write_text(json.dumps(evidence, indent=2))
    print('REAL_WORKFLOWS_ACCEPTANCE_PASSED\nBASE=' + str(base), flush=True)
except Exception as error:
    (base/'failure.json').write_text(json.dumps({'error':str(error),'observations':observations},indent=2))
    print('REAL_WORKFLOWS_FAILED '+str(error)+'\nBASE='+str(base),flush=True)
    raise
finally:
    server.terminate()
    server.wait(timeout=10)
    subprocess.run(['hc', 'server', 'stop'], env={**os.environ, **core_env}, capture_output=True, timeout=15)
    log.close()
