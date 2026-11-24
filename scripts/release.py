#!/usr/bin/env python3
"""GitHub release dispatch, reservation, and publication; never modifies main or an existing release."""
import argparse, datetime, hashlib, json, os, pathlib, re, shutil, subprocess, sys, tarfile, time, urllib.error, urllib.parse, urllib.request
from zoneinfo import ZoneInfo

DESKTOP_PLATFORMS = ('macos-aarch64', 'linux-x86_64', 'linux-aarch64')
CLI_PLATFORMS = ('linux-x86_64', 'linux-aarch64')
ANDROID_PLATFORMS = ('android-arm64-v8a',)
ANDROID_BUNDLE_ID = 'dev.canban.mobile'
CI_WORKFLOW = 'ci.yml'

def derive_android_version_code(version):
    m=re.fullmatch(r'(\d{4})(\d{2})(\d{2})-(\d+)',str(version).strip())
    if not m:raise RuntimeError(f'Cannot derive Android version_code from release version: {version!r}')
    year,month,day,seq=(int(g) for g in m.groups())
    if not 2020<=year<=2147 or not 1<=month<=12 or not 1<=day<=31 or not 1<=seq<=99:
        raise RuntimeError(f'Android release version out of encodable range: {version!r}')
    return (year-2000)*1_000_000+month*10_000+day*100+seq

def pack_tar_gz(source, output):
    source=pathlib.Path(source)
    root_name='Canban.app' if source.suffix=='.app' else 'canban'
    with tarfile.open(output,'w:gz') as archive:
        archive.add(source,arcname=root_name,recursive=False)
        for entry in sorted(source.rglob('*')):
            if entry.is_symlink() or not (entry.is_file() or entry.is_dir()):
                raise RuntimeError(f'App bundle contains unsafe link or special file: {entry}')
            archive.add(entry,arcname=str(pathlib.Path(root_name)/entry.relative_to(source)),recursive=False)

def pack_cli_tar_gz(binary, output):
    binary=pathlib.Path(binary)
    if binary.is_symlink() or not binary.is_file():
        raise RuntimeError(f'CLI binary is missing or unsafe: {binary}')
    with tarfile.open(output,'w:gz') as archive:
        info=archive.gettarinfo(str(binary),arcname='canban')
        info.mode=0o755
        info.uid=0;info.gid=0;info.uname='';info.gname=''
        with binary.open('rb') as stream:
            archive.addfile(info,stream)

def collect_dist_files(directory):
    directory=pathlib.Path(directory)
    by_name={}
    lower_names={}
    if not directory.exists():return by_name
    for path in sorted(directory.rglob('*')):
        if not path.is_file():continue
        lower=path.name.lower()
        if lower in lower_names:
            raise RuntimeError(f'Duplicate or case-colliding release asset name {path.name}: {lower_names[lower]} and {path}')
        lower_names[lower]=path
        by_name[path.name]=path
    return by_name

def verify_dist_directory(directory, version, commit):
    files=collect_dist_files(directory)
    for platform in DESKTOP_PLATFORMS:
        manifest_name=f'manifest-{platform}.json'
        manifest_path=files.get(manifest_name)
        if manifest_path is None:
            raise RuntimeError(f'Missing desktop release manifest: {manifest_name}')
        manifest=json.loads(manifest_path.read_text())
        if manifest.get('version')!=version or manifest.get('commit')!=commit or manifest.get('platform')!=platform:
            raise RuntimeError(f'Desktop manifest identity mismatch in {manifest_path}')
        zst_name=f'Canban-{platform}.tar.zst'
        gz_name=f'Canban-{platform}.tar.gz'
        if manifest.get('asset')!=zst_name or manifest.get('tar_gz')!=gz_name:
            raise RuntimeError(f'Desktop manifest asset name mismatch in {manifest_path}')
        for archive_name,size_key,sha_key in ((zst_name,'size','sha256'),(gz_name,'tar_gz_size','tar_gz_sha256')):
            archive_path=files.get(archive_name)
            if archive_path is None:
                raise RuntimeError(f'Missing desktop release archive: {archive_name}')
            data=archive_path.read_bytes()
            if len(data)!=manifest.get(size_key) or hashlib.sha256(data).hexdigest()!=str(manifest.get(sha_key,'')).lower():
                raise RuntimeError(f'Desktop archive digest/size mismatch for {archive_path}')
    for platform in CLI_PLATFORMS:
        manifest_name=f'manifest-cli-{platform}.json'
        manifest_path=files.get(manifest_name)
        if manifest_path is None:
            raise RuntimeError(f'Missing CLI release manifest: {manifest_name}')
        manifest=json.loads(manifest_path.read_text())
        if manifest.get('version')!=version or manifest.get('commit')!=commit or manifest.get('platform')!=platform:
            raise RuntimeError(f'CLI manifest identity mismatch in {manifest_path}')
        gz_name=f'canban-cli-{platform}.tar.gz'
        if manifest.get('asset')!=gz_name:
            raise RuntimeError(f'CLI manifest asset name mismatch in {manifest_path}')
        archive_path=files.get(gz_name)
        if archive_path is None:
            raise RuntimeError(f'Missing CLI release archive: {gz_name}')
        data=archive_path.read_bytes()
        if len(data)!=manifest.get('size') or hashlib.sha256(data).hexdigest()!=str(manifest.get('sha256','')).lower():
            raise RuntimeError(f'CLI archive digest/size mismatch for {archive_path}')
    for platform in ANDROID_PLATFORMS:
        manifest_name=f'manifest-{platform}.json'
        manifest_path=files.get(manifest_name)
        if manifest_path is None:
            raise RuntimeError(f'Missing Android release manifest: {manifest_name}')
        manifest=json.loads(manifest_path.read_text())
        if manifest.get('version')!=version or manifest.get('commit')!=commit or manifest.get('platform')!=platform or manifest.get('bundle_id')!=ANDROID_BUNDLE_ID:
            raise RuntimeError(f'Android manifest identity mismatch in {manifest_path}')
        if not isinstance(manifest.get('version_code'),int) or manifest['version_code']<1:
            raise RuntimeError(f'Invalid Android version_code in {manifest_path}')
        signing_sha=str(manifest.get('signing_sha256',''))
        if not re.fullmatch(r'[0-9a-f]{64}',signing_sha):
            raise RuntimeError(f'Invalid Android signing_sha256 in {manifest_path}')
        apk_name=f'Canban-{platform}.apk'
        if manifest.get('asset')!=apk_name:
            raise RuntimeError(f'Android manifest asset name mismatch in {manifest_path}')
        apk_path=files.get(apk_name)
        if apk_path is None:
            raise RuntimeError(f'Missing Android release APK: {apk_name}')
        data=apk_path.read_bytes()
        if len(data)!=manifest.get('size') or hashlib.sha256(data).hexdigest()!=str(manifest.get('sha256','')).lower():
            raise RuntimeError(f'Android APK digest/size mismatch for {apk_path}')

def resolve_gh_token():
    token=os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if token:return token.strip()
    gh=shutil.which('gh')
    if gh:
        result=subprocess.run([gh,'auth','token'],capture_output=True,text=True)
        if result.returncode==0 and result.stdout.strip():return result.stdout.strip()
    raise RuntimeError('GitHub token required: set GH_TOKEN or sign in with `gh auth login`')

def api(method,path,body=None,raw=None,content_type='application/json',token=None):
    host='https://uploads.github.com' if path.startswith('/upload/') else 'https://api.github.com'
    if path.startswith('/upload/'): path=path[len('/upload'):]
    auth=token if token is not None else os.environ['GH_TOKEN']
    request=urllib.request.Request(host+path, data=raw if raw is not None else (json.dumps(body).encode() if body is not None else None), method=method,
        headers={'Authorization':'Bearer '+auth,'Accept':'application/vnd.github+json','Content-Type':content_type,'User-Agent':'Canban release'})
    with urllib.request.urlopen(request,timeout=120) as response:
        data=response.read()
        return json.loads(data) if data else None
def get_optional(path):
    try:return api('GET',path)
    except urllib.error.HTTPError as error:
        if error.code==404:return None
        raise

def parse_version(tag):
    parts=tag.split('-')
    if len(parts)!=2 or len(parts[0])!=8 or not parts[0].isdigit() or not parts[1].isdigit() or parts[1].startswith('0'):return None
    date,seq=int(parts[0]),int(parts[1])
    return (date,seq) if seq>0 else None

def find_previous_release_tag(repo,version):
    target=parse_version(version)
    if target is None:return None
    # Inspect only the newest page of releases rather than walking full release history;
    # evaluate all entries on that page numerically so multi-digit daily sequences
    # (assuming <= 10 tags/day in normal operation) still beat single-digit tags.
    releases=api('GET',f'/repos/{repo}/releases?per_page=30&page=1') or []
    best=None
    for release in releases:
        if release.get('draft'):continue
        tag=release.get('tag_name','')
        parsed=parse_version(tag)
        if parsed is not None and parsed<target and (best is None or parsed>best[0]):
            best=(parsed,tag)
    return best[1] if best is not None else None

def format_commit_bullet(entry):
    message=((entry.get('commit') or {}).get('message') or '').strip()
    summary=message.splitlines()[0].strip() if message else ''
    sha=(entry.get('sha') or '')[:7]
    if summary and sha:return f'- {summary} ({sha})'
    return f'- {summary or sha}'

def build_release_notes(repo,version,commit,limit=16000):
    prev_tag=find_previous_release_tag(repo,version)
    commits=[];extra=0
    if prev_tag is not None:
        comparison=get_optional(f'/repos/{repo}/compare/{urllib.parse.quote(prev_tag,safe="")}...{commit}')
        if comparison is not None:
            raw_commits=comparison.get('commits') or []
            total=comparison.get('total_commits',len(raw_commits))
            extra=max(0,total-len(raw_commits))
            non_merge=[c for c in raw_commits if len(c.get('parents') or [])<=1]
            commits=list(reversed(non_merge if non_merge else raw_commits))
        footer=f'**Full Changelog**: https://github.com/{repo}/compare/{prev_tag}...{version}'
    else:
        raw_commits=get_optional(f'/repos/{repo}/commits?sha={commit}&per_page=100') or []
        non_merge=[c for c in raw_commits if len(c.get('parents') or [])<=1]
        commits=non_merge if non_merge else list(raw_commits)
        footer=f'**Full Changelog**: https://github.com/{repo}/commits/{version}'
    if not commits and extra==0:return footer
    header="## What's Changed\n"
    bullets=[format_commit_bullet(c) for c in commits]
    kept=[];dropped=extra
    for idx,line in enumerate(bullets):
        candidate=header+'\n'.join([*kept,line])+f'\n\n{footer}'
        if len(candidate)>limit:
            dropped=len(bullets)-idx+extra
            break
        kept.append(line)
    while dropped>0 and kept:
        more=f'- ... and {dropped} more commits'
        body=header+'\n'.join([*kept,more])+f'\n\n{footer}'
        if len(body)<=limit:return body
        kept.pop();dropped+=1
    if dropped>0:
        more=f'- ... and {dropped} more commits'
        body=header+more+f'\n\n{footer}'
        return body if len(body)<=limit else footer[:limit]
    return header+'\n'.join(kept)+f'\n\n{footer}'

def find_release(repo,version):
    # GitHub's release-by-tag endpoint only resolves published releases.
    # The authenticated list includes drafts, so reserve retries and publication
    # can both find the same immutable release ID before it is published.
    for page in range(1,101):
        releases=api('GET',f'/repos/{repo}/releases?per_page=100&page={page}')
        for release in releases:
            if release['tag_name']==version:return release
        if len(releases)<100:return None
    raise RuntimeError('Release lookup exceeded pagination limit')

def reserve(repo,commit,run_id):
    run=api('GET',f'/repos/{repo}/actions/runs/{run_id}')
    day=datetime.datetime.fromisoformat(run['created_at'].replace('Z','+00:00')).astimezone(ZoneInfo('Asia/Shanghai')).strftime('%Y%m%d')
    for _ in range(100):
        refs=api('GET',f'/repos/{repo}/git/matching-refs/tags/{day}-')
        highest=0
        for ref in refs:
            tag=ref['ref'].removeprefix('refs/tags/')
            try:n=int(tag.split('-')[1])
            except (ValueError,IndexError):continue
            highest=max(highest,n)
            if ref['object']['type']=='tag':
                info=api('GET',f'/repos/{repo}/git/tags/{ref["object"]["sha"]}')
                try:record=json.loads(info['message'])
                except (ValueError,KeyError):continue
                if str(record.get('run_id'))==str(run_id):
                    if record['commit']!=commit:raise RuntimeError('Reserved release commit changed')
                    return tag
        version=f'{day}-{highest+1}'
        obj=api('POST',f'/repos/{repo}/git/tags',{'tag':version,'message':json.dumps({'run_id':str(run_id),'commit':commit}), 'object':commit,'type':'commit'})
        try:
            api('POST',f'/repos/{repo}/git/refs',{'ref':'refs/tags/'+version,'sha':obj['sha']})
            return version
        except urllib.error.HTTPError as error:
            if error.code!=422 or get_optional(f'/repos/{repo}/git/ref/tags/{version}') is None:raise
    raise RuntimeError('Release numbering exceeded retry bound')

def ci_runs(repo,sha,token=None):
    data=api('GET',f'/repos/{repo}/actions/workflows/{CI_WORKFLOW}/runs?head_sha={sha}&per_page=20',token=token) or {}
    return data.get('workflow_runs',[])

def classify_ci_runs(runs):
    if not runs:return ('missing',None)
    ordered=sorted(runs,key=lambda r:(r.get('created_at',''),r.get('id',0)),reverse=True)
    for run in ordered:
        if run.get('conclusion')=='success':return ('success',run)
    for run in ordered:
        if run.get('status')!='completed':return ('pending',run)
    return ('failure',ordered[0])

def wait_for_ci(repo,sha,token=None,timeout=5400,missing_grace=600,poll=30,sleep=time.sleep,clock=time.monotonic):
    start=clock()
    deadline=start+timeout
    missing_deadline=start+missing_grace
    sha7=sha[:7]
    while True:
        runs=ci_runs(repo,sha,token=token)
        state,run=classify_ci_runs(runs)
        if state=='success':return run
        if state=='failure':
            url=run.get('html_url') or f'https://github.com/{repo}/actions/workflows/{CI_WORKFLOW}'
            rid=run.get('id')
            hint=f'gh run rerun --failed {rid}' if rid is not None else 'gh run rerun --failed <run-id>'
            raise RuntimeError(f'CI failed for {sha7}: {url} (retry failed jobs with `{hint}`, then dispatch release again)')
        now=clock()
        if state=='missing' and now>=missing_deadline:
            raise RuntimeError(f'No CI run found for {sha7} within {missing_grace}s')
        if now>=deadline:
            raise RuntimeError(f'Timed out waiting for CI on {sha7} ({state})')
        if state=='pending':
            url=run.get('html_url') or ''
            msg=f'Waiting for CI ({run.get("status")}) on {sha7}: {url}' if url else f'Waiting for CI ({run.get("status")}) on {sha7}'
            print(msg,flush=True)
        else:
            print(f'Waiting for CI run to appear for {sha7}...',flush=True)
        sleep(poll)

def dispatch(repo,ref='main',workflow='release.yml',watch=False,poll_interval=2.0,discover_timeout=30.0):
    token=resolve_gh_token()
    ref_param=urllib.parse.quote(ref,safe='')
    sha=api('GET',f'/repos/{repo}/commits/{ref_param}',token=token)['sha']
    sha7=sha[:7]
    state,ci_run=classify_ci_runs(ci_runs(repo,sha,token=token))
    if state=='failure':
        url=ci_run.get('html_url') or f'https://github.com/{repo}/actions/workflows/{CI_WORKFLOW}'
        rid=ci_run.get('id')
        hint=f'gh run rerun --failed {rid}' if rid is not None else 'gh run rerun --failed <run-id>'
        raise RuntimeError(f'CI failed for {sha7}: {url} (fix or run `{hint}` before dispatching release)')
    if state=='pending':
        url=ci_run.get('html_url') or ''
        msg=f'CI still running for {sha7}; release ci-gate will wait: {url}' if url else f'CI still running for {sha7}; release ci-gate will wait'
        print(msg)
    elif state=='missing':
        print(f'No CI run found yet for {sha7}; release ci-gate will wait up to 10 min for {CI_WORKFLOW} to appear')
    else:
        print(f'CI green for {sha7}')
    dispatched_at=datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(seconds=2)
    api('POST',f'/repos/{repo}/actions/workflows/{workflow}/dispatches',{'ref':ref},token=token)
    deadline=time.monotonic()+discover_timeout
    run=None
    while time.monotonic()<deadline:
        listing=api('GET',f'/repos/{repo}/actions/workflows/{workflow}/runs?event=workflow_dispatch&branch={ref_param}&per_page=10',token=token) or {}
        for candidate in listing.get('workflow_runs',[]):
            created=datetime.datetime.fromisoformat(candidate['created_at'].replace('Z','+00:00'))
            if created>=dispatched_at:
                run=candidate
                break
        if run is not None:break
        time.sleep(poll_interval)
    if run is None:
        url=f'https://github.com/{repo}/actions/workflows/{workflow}'
        print(f'Triggered {workflow} on {repo}@{ref}; inspect runs at {url}')
        return None
    run_id=run['id'];html_url=run['html_url'];head_sha=run.get('head_sha','')[:7]
    print(f'Triggered release run #{run.get("run_number",run_id)} ({head_sha} on {ref}): {html_url}')
    if watch:
        gh=shutil.which('gh')
        if gh is None:raise RuntimeError('`--watch` requires GitHub CLI (`gh`) on PATH')
        subprocess.run([gh,'run','watch',str(run_id),'--repo',repo,'--exit-status'],check=True)
    return run

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command',choices=['dispatch','ci-gate','reserve','package','package-cli','package-android','verify-dist','publish'])
    parser.add_argument('--directory',type=pathlib.Path,default=pathlib.Path('dist'))
    parser.add_argument('--apk',type=pathlib.Path,default=pathlib.Path('.local/android/Canban-android-arm64-v8a.apk'),help='Signed release APK path (package-android only)')
    parser.add_argument('--signing-sha256',default=None,help='Verified Signer #1 SHA-256 certificate hex digest (package-android only)')
    parser.add_argument('--version-code',type=int,default=None,help='Android versionCode override (package-android only)')
    parser.add_argument('--ref',default='main',help='Branch or tag ref to release (dispatch only, default: main)')
    parser.add_argument('--repo',default=None,help='GitHub owner/repo override (default: GITHUB_REPOSITORY or jjyr/canban)')
    parser.add_argument('--watch',action='store_true',help='Watch the dispatched workflow run until completion (dispatch only)')
    parser.add_argument('--require-all-platforms',action='store_true',help='Verify macOS + Linux (x86_64 & aarch64) desktop/CLI and Android APK manifests/artifacts before publishing')
    args=parser.parse_args()
    repo=args.repo or os.environ.get('GITHUB_REPOSITORY','jjyr/canban')
    if args.command=='dispatch':
        dispatch(repo,ref=args.ref,watch=args.watch)
        return
    commit=os.environ['CANBAN_RELEASE_COMMIT'];version=os.environ.get('CANBAN_RELEASE_VERSION','')
    if args.command=='ci-gate':
        run=wait_for_ci(repo,commit)
        print('CI green: '+(run.get('html_url') or commit[:7]))
        return
    if args.command=='reserve':
        version=reserve(repo,commit,os.environ['GITHUB_RUN_ID'])
        if os.environ.get('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'],'a') as output:output.write('version='+version+'\n')
        release=find_release(repo,version)
        if release is None:
            notes=build_release_notes(repo,version,commit)
            api('POST',f'/repos/{repo}/releases',{'tag_name':version,'target_commitish':commit,'name':version,'body':notes,'draft':True,'prerelease':False,'generate_release_notes':False})
        print(version);return
    if args.command=='verify-dist':
        verify_dist_directory(args.directory,version,commit)
        print('Verified dist artifacts for '+version)
        return
    args.directory.mkdir(parents=True,exist_ok=True)
    if args.command=='package':
        helper=pathlib.Path('target/release/canban-updater');cli=pathlib.Path('target/release/canban')
        build=json.loads(subprocess.check_output([str(cli),'build-info'],text=True));assert build['version']==version and build['commit']==commit
        bundle=pathlib.Path('.local/Canban.app') if build['platform'].startswith('macos-') else pathlib.Path('.local/canban')
        if not bundle.is_dir():raise RuntimeError(f'Packaged desktop bundle is missing: {bundle}')
        name='Canban-'+build['platform']+'.tar.zst';asset=args.directory/name
        gz_name='Canban-'+build['platform']+'.tar.gz';gz_asset=args.directory/gz_name
        subprocess.run([str(helper),'pack',str(bundle),str(asset)],check=True)
        pack_tar_gz(bundle,gz_asset)
        manifest={**build,'rollback_floor':8,'asset':name,'size':asset.stat().st_size,'sha256':hashlib.sha256(asset.read_bytes()).hexdigest(),'tar_gz':gz_name,'tar_gz_size':gz_asset.stat().st_size,'tar_gz_sha256':hashlib.sha256(gz_asset.read_bytes()).hexdigest()}
        (args.directory/('manifest-'+build['platform']+'.json')).write_text(json.dumps(manifest,indent=2)+'\n');return
    if args.command=='package-cli':
        cli=pathlib.Path('target/release/canban')
        build=json.loads(subprocess.check_output([str(cli),'build-info'],text=True));assert build['version']==version and build['commit']==commit
        gz_name='canban-cli-'+build['platform']+'.tar.gz';gz_asset=args.directory/gz_name
        pack_cli_tar_gz(cli,gz_asset)
        manifest={**build,'asset':gz_name,'size':gz_asset.stat().st_size,'sha256':hashlib.sha256(gz_asset.read_bytes()).hexdigest()}
        (args.directory/('manifest-cli-'+build['platform']+'.json')).write_text(json.dumps(manifest,indent=2)+'\n');return
    if args.command=='package-android':
        if not version:raise RuntimeError('CANBAN_RELEASE_VERSION is required for package-android')
        if args.apk.is_symlink() or not args.apk.is_file():
            raise RuntimeError(f'Android APK is missing or unsafe: {args.apk}')
        signing_sha=(args.signing_sha256 or os.environ.get('ANDROID_SIGNING_SHA256','')).strip().lower()
        if not re.fullmatch(r'[0-9a-f]{64}',signing_sha):
            raise RuntimeError('package-android requires a 64-char hex --signing-sha256 (or ANDROID_SIGNING_SHA256)')
        version_code=args.version_code if args.version_code is not None else derive_android_version_code(version)
        if version_code<1:raise RuntimeError(f'Invalid Android version_code: {version_code}')
        platform='android-arm64-v8a';apk_name=f'Canban-{platform}.apk';dest_apk=args.directory/apk_name
        if args.apk.resolve()!=dest_apk.resolve():
            shutil.copy2(args.apk,dest_apk)
        data=dest_apk.read_bytes()
        manifest={
            'version':version,'commit':commit,'platform':platform,'abi':'arm64-v8a',
            'bundle_id':ANDROID_BUNDLE_ID,'version_code':version_code,
            'asset':apk_name,'size':len(data),'sha256':hashlib.sha256(data).hexdigest(),
            'signing_sha256':signing_sha,
        }
        (args.directory/f'manifest-{platform}.json').write_text(json.dumps(manifest,indent=2)+'\n');return
    release=find_release(repo,version)
    if release is None:raise RuntimeError('Reserved release is missing: '+version)
    if not release['draft']:
        print('Already published; immutable release unchanged');return
    if args.require_all_platforms:
        verify_dist_directory(args.directory,version,commit)
    dist_files=collect_dist_files(args.directory)
    # Draft retries may replace incomplete assets; published assets are immutable.
    for asset in release['assets']:api('DELETE',f'/repos/{repo}/releases/assets/{asset["id"]}')
    for name,path in sorted(dist_files.items()):
        ctype='application/vnd.android.package-archive' if name.endswith('.apk') else 'application/octet-stream'
        api('POST',f'/upload/repos/{repo}/releases/{release["id"]}/assets?name={name}',raw=path.read_bytes(),content_type=ctype)
    notes=build_release_notes(repo,version,commit)
    api('PATCH',f'/repos/{repo}/releases/{release["id"]}',{'draft':False,'prerelease':False,'body':notes})
    print('Published '+version)
if __name__=='__main__':main()
