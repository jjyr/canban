#!/usr/bin/env python3
"""Fetch commit-pinned libghostty-vt sources, excluding unused GUI assets."""
from pathlib import Path
import os, subprocess, tempfile, urllib.request, shutil
root=Path(__file__).resolve().parents[1]
revision='5813c9461d5af78b6307a1254d00b1bc190eaccd'
base=root/'.local/deps';source=base/'ghostty';marker=source/'.canban-revision'
if marker.exists() and marker.read_text()==revision and (source/'include/ghostty/vt.h').exists() and (source/'dist/cmake/ghostty-vt-config.cmake.in').exists():
    raise SystemExit(0)
base.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(dir=base) as staging:
    unpacked=Path(staging)/'source'
    # Git checks object hashes against this immutable commit. A partial checkout
    # avoids downloading GUI fonts/images which the VT-only build never uses.
    env=os.environ.copy()
    for scheme,url in urllib.request.getproxies().items():
        env.setdefault(scheme.lower()+'_proxy',url)
    def git(*args):
        result=subprocess.run(['git','-C',str(unpacked),*args],env=env,capture_output=True,text=True)
        if result.returncode:
            raise RuntimeError('Ghostty source fetch failed: '+result.stderr)
        return result.stdout.strip()
    unpacked.mkdir()
    git('init','--quiet')
    git('remote','add','origin','https://github.com/ghostty-org/ghostty.git')
    git('sparse-checkout','set','--no-cone','/build.zig','/build.zig.zon','/LICENSE','/dist/cmake/',
        '/include/','/pkg/','/src/','!/src/font/res/','!/src/terminal/testdata/')
    git('fetch','--depth','1','--filter=blob:none','origin',revision)
    git('checkout','--detach','FETCH_HEAD')
    if git('rev-parse','HEAD') != revision:
        raise RuntimeError('libghostty source commit mismatch')
    (unpacked/'.canban-revision').write_text(revision)
    if source.exists():shutil.rmtree(source)
    unpacked.rename(source)
print('Pinned libghostty-vt source ready: '+revision)
