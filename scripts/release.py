#!/usr/bin/env python3
"""GitHub release reservation/publication; never modifies main or an existing release."""
import argparse, datetime, hashlib, json, os, pathlib, subprocess, urllib.request, urllib.error
from zoneinfo import ZoneInfo

def api(method,path,body=None,raw=None,content_type='application/json'):
    host='https://uploads.github.com' if path.startswith('/upload/') else 'https://api.github.com'
    if path.startswith('/upload/'): path=path[len('/upload'):]
    request=urllib.request.Request(host+path, data=raw if raw is not None else (json.dumps(body).encode() if body is not None else None), method=method,
        headers={'Authorization':'Bearer '+os.environ['GH_TOKEN'],'Accept':'application/vnd.github+json','Content-Type':content_type,'User-Agent':'Canban release'})
    with urllib.request.urlopen(request,timeout=120) as response:
        data=response.read()
        return json.loads(data) if data else None
def get_optional(path):
    try:return api('GET',path)
    except urllib.error.HTTPError as error:
        if error.code==404:return None
        raise

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

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('command',choices=['reserve','package','publish']);parser.add_argument('--directory',type=pathlib.Path,default=pathlib.Path('dist'));args=parser.parse_args()
    repo=os.environ.get('GITHUB_REPOSITORY','jjyr/canban');commit=os.environ['CANBAN_RELEASE_COMMIT'];version=os.environ.get('CANBAN_RELEASE_VERSION','')
    if args.command=='reserve':
        version=reserve(repo,commit,os.environ['GITHUB_RUN_ID'])
        if os.environ.get('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'],'a') as output:output.write('version='+version+'\n')
        release=get_optional(f'/repos/{repo}/releases/tags/{version}')
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
    release=api('GET',f'/repos/{repo}/releases/tags/{version}')
    if not release['draft']:
        print('Already published; immutable release unchanged');return
    # Draft retries may replace incomplete assets; published assets are immutable.
    for asset in release['assets']:api('DELETE',f'/repos/{repo}/releases/assets/{asset["id"]}')
    for path in sorted(args.directory.iterdir()):
        if path.is_file():api('POST',f'/upload/repos/{repo}/releases/{release["id"]}/assets?name={path.name}',raw=path.read_bytes(),content_type='application/octet-stream')
    api('PATCH',f'/repos/{repo}/releases/{release["id"]}',{'draft':False,'prerelease':True})
    print('Published '+version)
if __name__=='__main__':main()
