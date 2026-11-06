#!/usr/bin/env python3
"""Fetch immutable libghostty-vt sources; preserve licenses and verify the archive."""
from pathlib import Path
import hashlib, subprocess, tempfile, urllib.request, shutil
root=Path(__file__).resolve().parents[1]
revision='291fd050b9e9c293c8adb6a140742649b7e42d3a'
checksum='f35b464f1aee22ffda9f6359038b9ad18a784342b4a4fa1cb97a79fb9ade79a4'
base=root/'.local/deps';source=base/'ghostty';marker=source/'.canban-revision'
if marker.exists() and marker.read_text()==revision and (source/'include/ghostty/vt.h').exists():
    raise SystemExit(0)
base.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(dir=base) as staging:
    staging=Path(staging);archive=staging/'source.tar.gz'
    with urllib.request.urlopen('https://codeload.github.com/ghostty-org/ghostty/tar.gz/'+revision,timeout=60) as response, archive.open('wb') as target:
        shutil.copyfileobj(response,target)
    assert hashlib.sha256(archive.read_bytes()).hexdigest()==checksum,'libghostty source checksum mismatch'
    subprocess.run(['/usr/bin/tar','-xzf',str(archive),'-C',str(staging)],check=True)
    unpacked=staging/('ghostty-'+revision)
    (unpacked/'.canban-revision').write_text(revision)
    if source.exists():shutil.rmtree(source)
    unpacked.rename(source)
print('Pinned libghostty-vt source ready: '+revision)
