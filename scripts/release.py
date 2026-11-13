#!/usr/bin/env python3
"""GitHub release dispatch, reservation, and publication; never modifies main or an existing release."""
import argparse, datetime, hashlib, json, os, pathlib, shutil, subprocess, sys, time, urllib.error, urllib.parse, urllib.request
from zoneinfo import ZoneInfo

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

def dispatch(repo,ref='main',workflow='release.yml',watch=False,poll_interval=2.0,discover_timeout=30.0):
    token=resolve_gh_token()
    dispatched_at=datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(seconds=2)
    api('POST',f'/repos/{repo}/actions/workflows/{workflow}/dispatches',{'ref':ref},token=token)
    deadline=time.monotonic()+discover_timeout
    run=None
    ref_param=urllib.parse.quote(ref,safe='')
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
    parser.add_argument('command',choices=['dispatch','reserve','package','publish'])
    parser.add_argument('--directory',type=pathlib.Path,default=pathlib.Path('dist'))
    parser.add_argument('--ref',default='main',help='Branch or tag ref to release (dispatch only, default: main)')
    parser.add_argument('--repo',default=None,help='GitHub owner/repo override (default: GITHUB_REPOSITORY or jjyr/canban)')
    parser.add_argument('--watch',action='store_true',help='Watch the dispatched workflow run until completion (dispatch only)')
    args=parser.parse_args()
    repo=args.repo or os.environ.get('GITHUB_REPOSITORY','jjyr/canban')
    if args.command=='dispatch':
        dispatch(repo,ref=args.ref,watch=args.watch)
        return
    commit=os.environ['CANBAN_RELEASE_COMMIT'];version=os.environ.get('CANBAN_RELEASE_VERSION','')
    if args.command=='reserve':
        version=reserve(repo,commit,os.environ['GITHUB_RUN_ID'])
        if os.environ.get('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'],'a') as output:output.write('version='+version+'\n')
        release=find_release(repo,version)
        if release is None:api('POST',f'/repos/{repo}/releases',{'tag_name':version,'target_commitish':commit,'name':version,'draft':True,'prerelease':True,'generate_release_notes':True})
        print(version);return
    args.directory.mkdir(parents=True,exist_ok=True)
    if args.command=='package':
        helper=pathlib.Path('target/release/canban-updater');cli=pathlib.Path('target/release/canban')
        build=json.loads(subprocess.check_output([str(cli),'build-info'],text=True));assert build['version']==version and build['commit']==commit
        name='Canban-'+build['platform']+'.tar.zst';asset=args.directory/name
        subprocess.run([str(helper),'pack','.local/Canban.app',str(asset)],check=True)
        manifest={**build,'rollback_floor':8,'asset':name,'size':asset.stat().st_size,'sha256':hashlib.sha256(asset.read_bytes()).hexdigest()}
        (args.directory/('manifest-'+build['platform']+'.json')).write_text(json.dumps(manifest,indent=2)+'\n');return
    release=find_release(repo,version)
    if release is None:raise RuntimeError('Reserved release is missing: '+version)
    if not release['draft']:
        print('Already published; immutable release unchanged');return
    # Draft retries may replace incomplete assets; published assets are immutable.
    for asset in release['assets']:api('DELETE',f'/repos/{repo}/releases/assets/{asset["id"]}')
    for path in sorted(args.directory.iterdir()):
        if path.is_file():api('POST',f'/upload/repos/{repo}/releases/{release["id"]}/assets?name={path.name}',raw=path.read_bytes(),content_type='application/octet-stream')
    api('PATCH',f'/repos/{repo}/releases/{release["id"]}',{'draft':False,'prerelease':True})
    print('Published '+version)
if __name__=='__main__':main()
