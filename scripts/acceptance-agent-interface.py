#!/usr/bin/env python3
"""Isolated real CLI/RPC acceptance and reproducible synthetic 62-call audit.

Optionally compare a pre-change CLI. No model API is invoked: candidate context
payloads are recorded separately, never mislabeled as measured model tokens.
"""
import argparse
import collections
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--baseline-cli', type=Path)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
args.output.mkdir(parents=True, exist_ok=True)
current = repo / 'target/debug/canban'
workflow = '''schema: 8
id: interface-audit
name: Interface audit
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
on:
  audit:
    event: manual
    start_at: done
steps:
  - id: done
    uses: complete
'''


def exercise(binary, name, optimized=False):
    out = args.output / name
    out.mkdir(exist_ok=True)
    base = Path(tempfile.mkdtemp(prefix='cb-interface-', dir='/private/tmp' if Path('/private/tmp').is_dir() else None))
    home = base / 'home'
    (home / 'config').mkdir(parents=True)
    # No external runtime is needed for this trigger-free fixture. In particular,
    # it cannot touch the user's live Herdr Core, installed home or repositories.
    (home / 'config/agents.json').write_text(json.dumps({
        'hc': '/usr/bin/false', 'hc_session': base.name,
        'hc_env': {'HC_SOCKET': str(base / 'hc.sock'), 'XDG_STATE_HOME': str(base / 'state'),
                   'XDG_CONFIG_HOME': str(base / 'config')},
        'default_agent': 'developer',
        'profiles': [{'id': 'developer', 'preset': 'codex'}, {'id': 'reviewer', 'preset': 'codex'}],
    }))
    env = {k: v for k, v in os.environ.items() if not k.startswith('CANBAN_')}
    env.update(CANBAN_HOME=str(home), CANBAN_UPDATES_DISABLED='1', CANBAN_GH_BIN='/usr/bin/false')
    log = (out / 'server.log').open('w')
    server = subprocess.Popen([str(binary), '--home', str(home), 'serve'], env=env, stdout=log, stderr=log)
    listener = socket.socket(socket.AF_UNIX)
    proxy_path = base / 'proxy.sock'
    listener.bind(str(proxy_path))
    listener.listen()
    wires = []
    proxy_errors = []

    def proxy():
        while True:
            try:
                client, _ = listener.accept()
            except OSError:
                return
            try:
                with client, socket.socket(socket.AF_UNIX) as upstream:
                    upstream.connect(str(home / 'app.sock'))
                    request = client.makefile('rb').readline()
                    upstream.sendall(request)
                    response = upstream.makefile('rb').readline()
                    wires.append((request, response))
                    client.sendall(response)
            except Exception as error:
                proxy_errors.append(repr(error))

    thread = threading.Thread(target=proxy, daemon=True)
    thread.start()
    rows = []

    def cli(method, params=None, measure=False, flags=()):
        params = params or {}
        before = len(wires)
        result = subprocess.run([str(binary), '--home', str(home), '--endpoint', str(proxy_path),
                                 *flags, 'call', method, json.dumps(params, ensure_ascii=False)],
                                env=env, capture_output=True, timeout=30)
        assert result.returncode == 0, result.stderr.decode()
        if name != 'baseline' and '--human' not in flags:
            assert result.stdout.count(b'\n') == 1, 'default output must be compact JSON'
        assert len(wires) == before + 1, (method, proxy_errors)
        request, response = wires[-1]
        value = json.loads(result.stdout)
        if measure:
            index = len(rows)
            (out / f'{index:02d}-request.json').write_bytes(request)
            (out / f'{index:02d}-response.json').write_bytes(response)
            (out / f'{index:02d}-stdout.json').write_bytes(result.stdout)
            # Explicit, reproducible local context selection, not a claim that a
            # provider consumed it. The actual agent receives only our summary.
            if method == 'card.list':
                cards = value if isinstance(value, list) else value['items']
                context = [{k: c[k] for k in ('number', 'title', 'stage', 'revision')} for c in cards]
            elif method == 'timeline.list':
                context = [{'id': e['id'], 'sequence': e['sequence'],
                            'body': e.get('body', e.get('data', {}).get('body', e.get('message')))}
                           for e in value['events'] if e['kind'].startswith('comment.')]
            elif method == 'card.get':
                context = {k: value[k] for k in ('number', 'stage', 'revision')}
            elif method == 'comment.add':
                context = {k: value[k] for k in ('id', 'card_id', 'sequence')}
            else:
                context = {k: value[k] for k in ('id', 'stage', 'revision')}
            payload = json.dumps(context, ensure_ascii=False, separators=(',', ':')).encode()
            (out / f'{index:02d}-candidate-context.json').write_bytes(payload)
            rows.append({'method': method, 'params_bytes': len(json.dumps(params, ensure_ascii=False).encode()),
                         'rpc_request_bytes': len(request), 'rpc_response_bytes': len(response),
                         'cli_stdout_bytes': len(result.stdout), 'candidate_context_bytes': len(payload)})
        return value

    try:
        deadline = time.monotonic() + 30
        while not (home / 'app.sock').exists():
            assert server.poll() is None, (out / 'server.log').read_text()
            assert time.monotonic() < deadline, 'AppServer did not start'
            time.sleep(.05)
        ping = cli('app.ping')
        ping['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        (out / 'build.json').write_text(json.dumps(ping, indent=2))
        source = base / 'source'
        source.mkdir()
        project = cli('project.create', {'name': 'Response audit', 'directory': str(source), 'workflow_yaml': workflow})
        # Comparable volume to the report, but synthetic; not a replay of private
        # historical bodies or a claim to reproduce the exact original bytes.
        description = ('Shared guidelines: preserve history and validate changes.\n' * 55)
        cards = [cli('card.create', {'project_id': project['id'], 'title': f'Audit card {n + 1}',
                                     'body': description + f'Unique task {n + 1}'}) for n in range(20)]
        listing = {'project_id': project['id']}
        if optimized:
            listing['stage'] = 'backlog'
        cli('card.list', listing, measure=True)
        for n, card in enumerate(cards[:14]):
            scope = {'card_id': card['id']}
            initial_params = {**scope, 'limit': 50}
            if optimized:
                initial_params['kinds'] = ['comment.created', 'comment.updated']
            page = cli('timeline.list', initial_params, measure=True)
            after = page.get('last_sequence', max([e['sequence'] for e in page['events']], default=0))
            if not optimized:
                cli('card.get', scope, measure=True)
            body = f'Review {n + 1}: ' + ('核对范围、证据和验证结果。' * 65)
            event = cli('comment.add', {**scope, 'body': body, 'mentions': [], 'request_key': f'audit-comment-{n}'}, measure=True)
            assert event['card_id'] == card['id']
            if not optimized:
                readback = cli('timeline.list', {**scope, 'after_sequence': after, 'limit': 50}, measure=True)
                assert any(e['id'] == event['id'] for e in readback['events'])
        for n, card in enumerate(cards[:2]):
            current_card = cli('card.get', {'card_id': card['id']}, measure=True)
            moved = cli('card.move', {'card_id': card['id'], 'revision': current_card['revision'],
                                      'stage': 'develop', 'request_key': f'audit-move-{n}'}, measure=True)
            assert moved['stage'] == 'develop'
        cli('card.list', {'project_id': project['id']}, measure=True)
        if name != 'baseline':
            # Exercise actual CLI content/format selection and pre-write rejection.
            p = {'card_id': cards[0]['id']}
            selected = cli('card.get', p, flags=('--fields', 'number,body,revision'))
            assert set(selected) == {'number', 'body', 'revision'}
            assert selected['body'].startswith('Shared guidelines:')
            assert cli('card.get', p, flags=('--full',))['body'] == selected['body']
            human = cli('card.get', p, flags=('--human',))
            assert human == cli('card.get', p)
            before = len(wires)
            bad = subprocess.run([str(binary), '--home', str(home), '--endpoint', str(proxy_path),
                                  'call', 'card.create', json.dumps({'project_id': project['id'],
                                   'title': 'must not exist', 'fields': ['typo']})], env=env, capture_output=True)
            assert bad.returncode != 0 and not bad.stdout and b'unknown_field' in bad.stderr
            assert len(wires) == before, 'invalid selection reached the server'
            bundle_skill = subprocess.check_output([str(binary), 'skill'], env=env)
            assert bundle_skill == (repo / 'skills/canban/SKILL.md').read_bytes()
            assert bundle_skill == (home / 'skills/canban/SKILL.md').read_bytes()
        totals = {key: sum(r[key] for r in rows) for key in rows[0] if key != 'method'}
        totals['calls'] = len(rows)
        totals['method_counts'] = dict(collections.Counter(r['method'] for r in rows))
        (out / 'metrics.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in rows))
        (out / 'totals.json').write_text(json.dumps(totals, indent=2))
        return totals
    finally:
        server.terminate()
        server.wait(timeout=15)
        listener.close()
        log.close()
        shutil.rmtree(base)


summary = {'fixture': 'synthetic 20 Cards / 14 comments / 2 moves',
           'context_measurement': 'candidate payloads only; no model or tokenizer invoked'}
if args.baseline_cli:
    summary['baseline'] = exercise(args.baseline_cli.resolve(), 'baseline')
summary['current_same_operations'] = exercise(current, 'current')
assert summary['current_same_operations']['calls'] == 62
summary['current_skill_strategy'] = exercise(current, 'strategy', optimized=True)
assert summary['current_skill_strategy']['calls'] == 34
if args.baseline_cli:
    for metric in ('rpc_response_bytes', 'cli_stdout_bytes'):
        summary[metric + '_reduction'] = 1 - summary['current_same_operations'][metric] / summary['baseline'][metric]
(args.output / 'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary, separators=(',', ':')))
