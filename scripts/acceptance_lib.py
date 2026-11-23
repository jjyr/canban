"""Shared race-resistant primitives for Canban acceptance scripts."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import time
from typing import Any, Callable


FINISHED_AGENT_STATES = frozenset({'finished', 'failed', 'cancelled', 'idle_closed', 'archived'})


def isolated_herdr_env(
    base: Path | str,
    *,
    session: str = 'acceptance',
    base_env: dict[str, str] | None = None,
) -> tuple[dict[str, str], dict[str, str]]:
    """Build a Herdr-isolated process env (stripping inherited HC_*/HERDR_*) and its core vars."""
    root = Path(base)
    core = {
        'HC_SESSION': session,
        'HC_SOCKET': str(root / 'hc.sock'),
        'XDG_STATE_HOME': str(root / 'hc-state'),
        'XDG_CONFIG_HOME': str(root / 'hc-config'),
    }
    source = os.environ if base_env is None else base_env
    env = {k: v for k, v in source.items() if not k.startswith(('HC_', 'HERDR_'))}
    env.update(core)
    return env, core


def call(
    cli: Path | str,
    env: dict[str, str],
    method: str,
    params: dict[str, Any] | None = None,
    *,
    full: bool = True,
    timeout: float = 90,
) -> Any:
    """Invoke a Canban JSON-RPC method via the CLI."""
    cmd = [str(cli)]
    if full:
        cmd.append('--full')
    cmd.extend(['call', method, json.dumps(params or {})])
    result = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f'{method} exited with code {result.returncode}')
    return json.loads(result.stdout)


def wait_until(
    fetch: Callable[[], Any],
    predicate: Callable[[Any], Any],
    label: str,
    *,
    timeout: float = 50,
    interval: float = 0.15,
    log_path: Path | None = None,
) -> Any:
    """Poll `fetch()` until `predicate(value)` is truthy or `timeout` elapses."""
    deadline = time.monotonic() + timeout
    last_error: str | None = None
    while time.monotonic() < deadline:
        try:
            value = fetch()
        except RuntimeError as exc:
            last_error = str(exc)
            time.sleep(interval)
            continue
        if predicate(value):
            return value
        time.sleep(interval)
    detail = f'{label} timed out after {timeout}s'
    if last_error:
        detail += f' (last error: {last_error})'
    if log_path is not None and log_path.exists():
        tail = '\n'.join(log_path.read_text(errors='replace').splitlines()[-80:])
        detail += f'\n--- {log_path} ---\n{tail}'
    raise RuntimeError(detail)


def card_events(
    call_fn: Callable[[str, dict[str, Any] | None], Any],
    card_id: str,
    *,
    limit: int = 200,
    full_cli: bool = True,
) -> list[dict[str, Any]]:
    """Fetch Card timeline events via `timeline.list` without conflicting view flags."""
    params: dict[str, Any] = {'card_id': card_id, 'limit': limit}
    if not full_cli:
        params['view'] = 'full'
    return call_fn('timeline.list', params)['events']


def live_agents_connected(
    snapshot: dict[str, Any],
    terminal_ids: set[str] | list[str] | None = None,
) -> bool:
    """Return True when every active (non-finished) agent terminal is connected."""
    wanted = set(terminal_ids) if terminal_ids is not None else None
    matched = 0
    for terminal in snapshot.get('terminals', []):
        agent = terminal.get('agent')
        if not agent:
            continue
        if wanted is not None:
            if terminal['id'] not in wanted:
                continue
            matched += 1
        elif agent.get('state') in FINISHED_AGENT_STATES or not agent.get('terminal'):
            continue
        if agent.get('connection_state') != 'connected':
            return False
    return wanted is None or matched == len(wanted)


def restart_server(
    cli: Path | str,
    env: dict[str, str],
    log: Any,
    server: subprocess.Popen[Any],
    call_fn: Callable[[str, dict[str, Any] | None], Any],
    *,
    graceful: bool = True,
    wait_connected: bool = True,
    terminal_ids: set[str] | list[str] | None = None,
    extra_predicate: Callable[[dict[str, Any]], Any] | None = None,
    label: str = 'server restart',
    timeout: float = 50,
    log_path: Path | None = None,
) -> tuple[subprocess.Popen[Any], dict[str, Any]]:
    """Restart AppServer and wait until it is ready and live agents have reconnected."""
    if graceful:
        call_fn('app.shutdown', None)
        server.wait(timeout=15)
    else:
        server.terminate()
        server.wait(timeout=10)
    restarted = subprocess.Popen([str(cli), 'serve'], env=env, stdout=log, stderr=log)

    def fetch() -> dict[str, Any]:
        if restarted.poll() is not None:
            raise RuntimeError(f'restarted AppServer exited with code {restarted.returncode}')
        return call_fn('app.snapshot', None)

    def ready(snapshot: dict[str, Any]) -> bool:
        if wait_connected and not live_agents_connected(snapshot, terminal_ids=terminal_ids):
            return False
        if extra_predicate is not None and not extra_predicate(snapshot):
            return False
        return True

    snapshot = wait_until(fetch, ready, label, timeout=timeout, log_path=log_path)
    return restarted, snapshot


def assert_event_interval(
    call_fn: Callable[[str, dict[str, Any] | None], Any],
    card_id: str,
    start_kind: str,
    end_kind: str,
    *,
    min_ms: int,
    max_ms: int | None = None,
    full_cli: bool = True,
) -> int:
    """Assert elapsed milliseconds between durable server-side event timestamps."""
    events = card_events(call_fn, card_id, full_cli=full_cli)
    starts = [e for e in events if e.get('kind') == start_kind]
    ends = [e for e in events if e.get('kind') == end_kind]
    assert starts, f'missing {start_kind} event in {[e.get("kind") for e in events]}'
    assert ends, f'missing {end_kind} event in {[e.get("kind") for e in events]}'
    elapsed_ms = int(ends[-1]['created_at']) - int(starts[-1]['created_at'])
    assert elapsed_ms >= min_ms, (
        f'{start_kind} -> {end_kind} elapsed {elapsed_ms}ms < {min_ms}ms'
    )
    if max_ms is not None:
        assert elapsed_ms <= max_ms, (
            f'{start_kind} -> {end_kind} elapsed {elapsed_ms}ms > {max_ms}ms'
        )
    return elapsed_ms
