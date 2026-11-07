#!/usr/bin/env python3
"""Explicit real-model opt-in: human input, cross-Board work and review/rework.
Leaves reviewed Cards unarchived so the delivery acceptance can merge their code.
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

def call(method, params=None):
    result = subprocess.run([str(cli), 'call', method, json.dumps(params or {})], env=env,
                            capture_output=True, text=True, timeout=90)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)

def wait(predicate, label, timeout=300):
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
    p1, source1 = project('human-input', {'slug.py': 'def slug(text):\n    return text\n',
        'test_slug.py': 'import unittest\nfrom slug import slug\nclass Tests(unittest.TestCase):\n    def test_words(self): self.assertEqual(slug("Hello World"),"hello-world")\n    def test_unicode(self): self.assertEqual(slug("  CAFÉ  世界  "),"café-世界")\n'})
    p2, source2 = project('review-rework', {'totals.py': 'def total(values):\n    return 0\n',
        'test_totals.py': 'import unittest\nfrom totals import total\nclass Tests(unittest.TestCase):\n    def test_total(self): self.assertEqual(total([2,3]),5)\n    def test_empty(self): self.assertEqual(total([]),0)\n'})
    common = ' Use current Canban session/report instructions. Run /usr/bin/python3 -m unittest -v. Keep source code and tests in this Card worktree. Do not commit or merge; a human will accept the reviewed changes later.'
    c1 = call('card.create', {'project_id': p1['id'], 'title': 'Slug function needs a human separator choice',
        'body': 'Implement slug(text): lowercase Unicode text, trim and collapse whitespace into a separator. In the FIRST Develop invocation, before implementation, report blocked and ask the human whether the separator should be - or _. Wait for the human @developer answer without blocking a shell command. After the answer, implement, test and report completed. Review should inspect the result and report normally.' + common})
    c2 = call('card.create', {'project_id': p2['id'], 'title': 'Total values, then handle a review change',
        'body': 'Implement total(values), summing an iterable of integers with zero for an empty iterable. This initial version permits negative values. Produce an ordinary review report in Review.' + common})
    for card in (c1, c2):
        call('card.move', {'card_id': card['id'], 'revision': card['revision'], 'stage': 'develop'})
    print('REAL_WORKFLOWS_HOME=' + str(home), flush=True)
    s = wait(lambda s: any(r['card_id'] == c1['id'] and r['state'] == 'blocked' for r in s['requests']) and reviewed(s, c2['id']), 'blocked and first review')
    blocked = next(c for c in s['cards'] if c['id'] == c1['id'])
    assert blocked['attention'] and any(n['card_id'] == c1['id'] for n in s['notices'])
    assert blocked['stage'] == 'develop'
    call('comment.add', {'card_id': c1['id'], 'body': '@developer The human chooses the hyphen (-) separator. Please acknowledge with canban reply using this Question ID, then implement the original slug task, run tests and report completed.'})
    developer = next(a for a in s['attempts'] if a['card_id'] == c2['id'] and next(r for r in s['requests'] if r['id'] == a['request_id'])['profile']['id'] == 'developer')
    before_sid = developer['provider_session_id']
    rework = next(c for c in s['cards'] if c['id'] == c2['id'])
    new_body = 'Review feedback changes the requirement: total(values) must raise ValueError when ANY input value is negative. Preserve normal and empty sums. Add a regression test proving negative input is rejected, implement this change and rerun tests. In Review verify the revised requirement and produce a report.' + common
    rework = call('card.update', {'card_id': c2['id'], 'revision': rework['revision'], 'title': rework['title'], 'body': new_body, 'tags': []})
    call('comment.add', {'card_id': c2['id'], 'body': 'Human review: reject negative values. Returning this Card to Develop for rework.'})
    rework = next(c for c in call('app.snapshot')['cards'] if c['id'] == c2['id'])
    call('card.move', {'card_id': c2['id'], 'revision': rework['revision'], 'stage': 'develop'})
    s = wait(lambda s: reviewed(s, c1['id']) and reviewed(s, c2['id']) and len([r for r in s['requests'] if r['card_id'] == c2['id']]) == 4, 'human answer and second review')
    assert all(q['state'] == 'replied' for q in s['questions'])
    dev_runs = [a for a in s['attempts'] if a['card_id'] == c2['id'] and next(r for r in s['requests'] if r['id'] == a['request_id'])['profile']['id'] == 'developer']
    assert len(dev_runs) == 2 and all(a['provider_session_id'] == before_sid for a in dev_runs)
    checks = {}
    for card, source in [(c1, source1), (c2, source2)]:
        cwd = Path(next(c for c in s['cards'] if c['id'] == card['id'])['worktree'])
        result = subprocess.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=cwd, capture_output=True, text=True, check=True)
        checks[card['id']] = result.stdout + result.stderr
        assert git(source, 'status', '--porcelain') == ''
    cwd = Path(next(c for c in s['cards'] if c['id'] == c2['id'])['worktree'])
    subprocess.run(['/usr/bin/python3', '-c', 'from totals import total\ntry: total([1,-1])\nexcept ValueError: pass\nelse: raise AssertionError("negative input accepted")'], cwd=cwd, check=True)
    evidence = {'snapshot': s, 'checks': checks, 'core_env': core_env, 'cards': [c1['id'], c2['id']], 'sources': [str(source1), str(source2)]}
    (base / 'evidence.json').write_text(json.dumps(evidence, indent=2))
    print('REAL_WORKFLOWS_ACCEPTANCE_PASSED\nBASE=' + str(base), flush=True)
finally:
    server.terminate()
    server.wait(timeout=10)
    subprocess.run(['hc', 'server', 'stop'], env={**os.environ, **core_env}, capture_output=True, timeout=15)
    log.close()
