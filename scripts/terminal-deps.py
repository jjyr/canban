#!/usr/bin/env python3
"""Fetch commit-pinned libghostty-vt sources, excluding unused GUI assets."""
from pathlib import Path
import os, subprocess, tempfile, urllib.request, shutil, sys, uuid

root = Path(__file__).resolve().parents[1]
revision = '5813c9461d5af78b6307a1254d00b1bc190eaccd'
base = root / '.local/deps'
source = base / 'ghostty'


def cache_dir() -> Path:
    override = os.environ.get('CANBAN_CACHE_DIR', '').strip()
    if override:
        return Path(override)
    home = os.environ.get('HOME', '').strip()
    if home:
        if sys.platform == 'darwin':
            return Path(home) / 'Library/Caches/canban'
        xdg = os.environ.get('XDG_CACHE_HOME', '').strip()
        return (Path(xdg) if xdg else Path(home) / '.cache') / 'canban'
    return root / '.local/cache'


def has_revision(path: Path) -> bool:
    marker = path / '.canban-revision'
    return (
        marker.exists()
        and marker.read_text() == revision
        and (path / 'include/ghostty/vt.h').exists()
        and (path / 'dist/cmake/ghostty-vt-config.cmake.in').exists()
    )


if has_revision(source):
    raise SystemExit(0)

cached = cache_dir() / 'ghostty-src' / revision
base.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(dir=base) as staging:
    unpacked = Path(staging) / 'source'
    if has_revision(cached):
        shutil.copytree(cached, unpacked, symlinks=True)
    else:
        # Git checks object hashes against this immutable commit. A partial checkout
        # avoids downloading GUI fonts/images which the VT-only build never uses.
        env = os.environ.copy()
        for scheme, url in urllib.request.getproxies().items():
            env.setdefault(scheme.lower() + '_proxy', url)

        def git(*args):
            result = subprocess.run(
                ['git', '-C', str(unpacked), *args],
                env=env,
                capture_output=True,
                text=True,
            )
            if result.returncode:
                raise RuntimeError('Ghostty source fetch failed: ' + result.stderr)
            return result.stdout.strip()

        unpacked.mkdir()
        git('init', '--quiet')
        git('remote', 'add', 'origin', 'https://github.com/ghostty-org/ghostty.git')
        git(
            'sparse-checkout',
            'set',
            '--no-cone',
            '/build.zig',
            '/build.zig.zon',
            '/LICENSE',
            '/dist/cmake/',
            '/include/',
            '/pkg/',
            '/src/',
            '!/src/font/res/',
            '!/src/terminal/testdata/',
        )
        git('fetch', '--depth', '1', '--filter=blob:none', 'origin', revision)
        git('checkout', '--detach', 'FETCH_HEAD')
        if git('rev-parse', 'HEAD') != revision:
            raise RuntimeError('libghostty source commit mismatch')
        (unpacked / '.canban-revision').write_text(revision)
        try:
            cached.parent.mkdir(parents=True, exist_ok=True)
            staged = cached.parent / f'.tmp-{revision}-{os.getpid()}-{uuid.uuid4().hex[:8]}'
            shutil.copytree(unpacked, staged, symlinks=True)
            try:
                staged.rename(cached)
            except OSError:
                shutil.rmtree(staged, ignore_errors=True)
        except OSError:
            pass
    if source.exists():
        shutil.rmtree(source, ignore_errors=True)
    try:
        unpacked.rename(source)
    except OSError:
        if not has_revision(source):
            raise
print('Pinned libghostty-vt source ready: ' + revision)
