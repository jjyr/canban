#!/usr/bin/env python3
"""Offline release-number reservation and immutable publication regressions."""
import sys
sys.dont_write_bytecode=True
import importlib.util, json, pathlib, tarfile, tempfile, unittest, urllib.error, urllib.request, os, hashlib
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('release',pathlib.Path(__file__).with_name('release.py'));release=importlib.util.module_from_spec(spec);spec.loader.exec_module(release)
class Releases(unittest.TestCase):
    def test_numeric_sequence_timezone_and_retry_identity(self):
        refs=[{'ref':'refs/tags/20261110-9','object':{'type':'commit','sha':'old'}}];calls=[]
        def api(method,path,body=None,**kwargs):
            calls.append((method,path,body))
            if '/actions/runs/' in path:return {'created_at':'2026-11-09T18:00:00Z'}
            if '/matching-refs/' in path:return refs
            if method=='POST' and path.endswith('/tags'):return {'sha':'reserved-object'}
            if method=='POST' and path.endswith('/refs'):return {}
            raise AssertionError((method,path))
        with patch.object(release,'api',api):self.assertEqual(release.reserve('fixture/app','c'*40,'123'),'20261110-10')
        ref=next(c[2] for c in calls if c[1].endswith('/refs'));self.assertEqual(ref['ref'],'refs/tags/20261110-10')
        def retry(method,path,body=None,**kwargs):
            if '/actions/runs/' in path:return {'created_at':'2026-11-09T18:00:00Z'}
            if '/matching-refs/' in path:return [{'ref':'refs/tags/20261110-10','object':{'type':'tag','sha':'object'}}]
            if '/git/tags/' in path:return {'message':'{"run_id":"123","commit":"'+'c'*40+'"}'}
            raise AssertionError('Retry attempted a write')
        with patch.object(release,'api',retry):self.assertEqual(release.reserve('fixture/app','c'*40,'123'),'20261110-10')
    def test_atomic_tag_collision_advances_without_reusing_another_runs_number(self):
        refs=[];attempts=[]
        def api(method,path,body=None,**kwargs):
            if '/actions/runs/' in path:return {'created_at':'2026-11-10T01:00:00Z'}
            if '/matching-refs/' in path:return refs
            if method=='POST' and path.endswith('/tags'):return {'sha':'object'}
            if method=='POST' and path.endswith('/refs'):
                attempts.append(body['ref'])
                if len(attempts)==1:
                    refs.append({'ref':body['ref'],'object':{'type':'commit','sha':'another-run'}})
                    raise urllib.error.HTTPError('https://api.github.com/fixture',422,'exists',{},None)
                return {}
            if '/git/ref/tags/' in path:return {'ref':'exists'}
            raise AssertionError((method,path))
        with patch.object(release,'api',api):self.assertEqual(release.reserve('fixture/app','c'*40,'123'),'20261110-2')
        self.assertEqual(attempts,['refs/tags/20261110-1','refs/tags/20261110-2'])
    def test_published_release_is_never_mutated(self):
        calls=[]
        def api(method,path,body=None,**kwargs):calls.append(method);return [{'tag_name':'20261110-1','draft':False}]
        with patch.object(release,'api',api),patch.object(sys,'argv',['release.py','publish','--directory','/private/tmp/canban-release-test-empty']),patch.dict(os.environ,{'CANBAN_RELEASE_COMMIT':'c'*40,'CANBAN_RELEASE_VERSION':'20261110-1'}):release.main()
        self.assertEqual(calls,['GET'])
    def test_reserve_and_publish_find_the_same_draft_without_release_by_tag(self):
        calls=[];releases=[{'id':10,'tag_name':'20261109-2','draft':False}]
        def api(method,path,body=None,**kwargs):
            calls.append((method,path,body,kwargs))
            if '/releases/tags/' in path:raise urllib.error.HTTPError(path,404,'Draft is not published',{},None)
            if method=='GET' and '/releases?' in path:return releases
            if method=='GET' and '/compare/' in path:
                return {'total_commits':3,'commits':[
                    {'sha':'1111111aaaa','parents':[{'sha':'p0'}],'commit':{'message':'feat: older feature\n\nbody'}},
                    {'sha':'2222222bbbb','parents':[{'sha':'p1'},{'sha':'p2'}],'commit':{'message':'Merge commit into branch'}},
                    {'sha':'3333333cccc','parents':[{'sha':'p1'}],'commit':{'message':'fix: newest bugfix'}},
                ]}
            if method=='POST' and path=='/repos/fixture/app/releases':
                releases.append({'id':42,'tag_name':body['tag_name'],'draft':True,'assets':[{'id':7}]});return releases[-1]
            if method=='DELETE' or path.startswith('/upload/'):return {}
            if method=='PATCH':return {**releases[-1],**body}
            raise AssertionError((method,path,body))
        with tempfile.TemporaryDirectory(prefix='cb-release-regression-') as directory:
            pathlib.Path(directory,'manifest-macos-aarch64.json').write_text('{}')
            pathlib.Path(directory,'Canban-macos-aarch64.tar.zst').write_bytes(b'fixture')
            with patch.object(release,'api',api),patch.object(release,'reserve',return_value='20261110-1'),patch.dict(os.environ,{'GITHUB_REPOSITORY':'fixture/app','GITHUB_RUN_ID':'123','CANBAN_RELEASE_COMMIT':'c'*40,'CANBAN_RELEASE_VERSION':'20261110-1'}):
                for command in ['reserve','reserve','publish']:
                    with patch.object(sys,'argv',['release.py',command,'--directory',directory]):release.main()
        self.assertEqual(sum(m=='POST' and p=='/repos/fixture/app/releases' for m,p,_,_ in calls),1)
        created=next(b for m,p,b,_ in calls if m=='POST' and p=='/repos/fixture/app/releases')
        self.assertIs(created['prerelease'],False)
        self.assertIs(created['generate_release_notes'],False)
        expected_notes="## What's Changed\n- fix: newest bugfix (3333333)\n- feat: older feature (1111111)\n\n**Full Changelog**: https://github.com/fixture/app/compare/20261109-2...20261110-1"
        self.assertEqual(created['body'],expected_notes)
        uploads=[p for m,p,_,_ in calls if m=='POST' and p.startswith('/upload/')]
        self.assertEqual(len(uploads),2)
        self.assertTrue(all('/releases/42/assets?' in p for p in uploads))
        self.assertEqual(calls[-1][:3],('PATCH','/repos/fixture/app/releases/42',{'draft':False,'prerelease':False,'body':expected_notes}))
        self.assertTrue(any(m=='POST' and p.startswith('/upload/') for m,p,_,_ in calls[:-1]))
    def test_release_notes_single_page_numeric_base_merge_filter_and_truncation(self):
        # Single-page lookup (no pagination walk), skips drafts & newer/equal versions, and orders YYYYMMDD-N numerically (10 > 9).
        releases=[
            {'tag_name':'20261114-11','draft':True},
            {'tag_name':'20261114-12','draft':False},
            {'tag_name':'20261114-9','draft':False},
            {'tag_name':'20261114-10','draft':False},
            {'tag_name':'20261113-4','draft':False},
        ]
        routes=[]
        def api(method,path,body=None,**kwargs):
            routes.append(path)
            if '/releases?' in path:
                self.assertIn('page=1',path)
                return releases
            if '/compare/20261114-10...' in path:
                return {'total_commits':4,'commits':[
                    {'sha':'aaaaaaa1111','parents':[{'sha':'0'}],'commit':{'message':'feat: first commit'}},
                    {'sha':'bbbbbbb2222','parents':[{'sha':'1'},{'sha':'2'}],'commit':{'message':"Merge branch 'main' into card"}},
                    {'sha':'ccccccc3333','parents':[{'sha':'1'}],'commit':{'message':'fix: second commit\n\nextra detail'}},
                    {'sha':'ddddddd4444','parents':[{'sha':'3'}],'commit':{'message':'perf: third commit'}},
                ]}
            raise AssertionError((method,path))
        with patch.object(release,'api',api):
            notes=release.build_release_notes('fixture/app','20261114-11','f'*40)
        self.assertEqual(sum('/releases?' in r for r in routes),1)
        self.assertEqual(notes,(
            "## What's Changed\n"
            "- perf: third commit (ddddddd)\n"
            "- fix: second commit (ccccccc)\n"
            "- feat: first commit (aaaaaaa)\n\n"
            "**Full Changelog**: https://github.com/fixture/app/compare/20261114-10...20261114-11"
        ))
        # Initial release without a prior published tag falls back to commit listing (newest-first, non-merge)
        def initial_api(method,path,body=None,**kwargs):
            if '/releases?' in path:return [{'tag_name':'20261114-1','draft':True}]
            if '/commits?sha=' in path:
                return [
                    {'sha':'9999999ffff','parents':[{'sha':'1'},{'sha':'2'}],'commit':{'message':'Merge pull request'}},
                    {'sha':'8888888eeee','parents':[{'sha':'1'}],'commit':{'message':'feat: bootstrap app'}},
                    {'sha':'7777777dddd','parents':[],'commit':{'message':'chore: initial commit'}},
                ]
            raise AssertionError((method,path))
        with patch.object(release,'api',initial_api):
            initial_notes=release.build_release_notes('fixture/app','20261114-1','e'*40)
        self.assertEqual(initial_notes,(
            "## What's Changed\n"
            "- feat: bootstrap app (8888888)\n"
            "- chore: initial commit (7777777)\n\n"
            "**Full Changelog**: https://github.com/fixture/app/commits/20261114-1"
        ))
        # Truncation respects limit and accounts for omitted commits beyond the 250-commit compare window
        many=[{'sha':f'{i:07x}0000','parents':[{'sha':'p'}],'commit':{'message':f'feat: change number {i} with description'}} for i in range(20)]
        def big_api(method,path,body=None,**kwargs):
            if '/releases?' in path:return [{'tag_name':'20261114-1','draft':False}]
            if '/compare/' in path:return {'total_commits':30,'commits':many}
            raise AssertionError((method,path))
        with patch.object(release,'api',big_api):
            truncated=release.build_release_notes('fixture/app','20261114-2','d'*40,limit=260)
        self.assertLessEqual(len(truncated),260)
        self.assertIn('- feat: change number 19 with description (0000013)',truncated)
        self.assertIn('more commits',truncated)
        self.assertTrue(truncated.endswith('**Full Changelog**: https://github.com/fixture/app/compare/20261114-1...20261114-2'))
    def test_draft_lookup_paginates_and_missing_publish_never_creates_a_release(self):
        first=[{'tag_name':f'other-{i}'} for i in range(100)]
        with patch.object(release,'api',side_effect=[first,[{'tag_name':'20261110-1','id':42,'draft':True}]]) as request:
            self.assertEqual(release.find_release('fixture/app','20261110-1')['id'],42)
            self.assertIn('page=2',request.call_args.args[1])
        with tempfile.TemporaryDirectory(prefix='cb-release-missing-') as directory:
            with patch.object(release,'api',return_value=[]) as request,patch.object(sys,'argv',['release.py','publish','--directory',directory]),patch.dict(os.environ,{'CANBAN_RELEASE_COMMIT':'c'*40,'CANBAN_RELEASE_VERSION':'20261110-1'}):
                with self.assertRaisesRegex(RuntimeError,'Reserved release is missing'):release.main()
                self.assertEqual(request.call_count,1)
                self.assertEqual(request.call_args.args[0],'GET')
    def test_failed_upload_never_publishes_the_draft(self):
        draft={'tag_name':'20261110-1','id':42,'draft':True,'assets':[]};calls=[]
        def api(method,path,body=None,**kwargs):
            calls.append((method,path))
            if method=='GET':return [draft]
            raise RuntimeError('Injected upload failure')
        with tempfile.TemporaryDirectory(prefix='cb-release-upload-') as directory:
            pathlib.Path(directory,'Canban-macos-aarch64.tar.zst').write_bytes(b'fixture')
            with patch.object(release,'api',api),patch.object(sys,'argv',['release.py','publish','--directory',directory]),patch.dict(os.environ,{'CANBAN_RELEASE_COMMIT':'c'*40,'CANBAN_RELEASE_VERSION':'20261110-1'}):
                with self.assertRaisesRegex(RuntimeError,'Injected upload failure'):release.main()
        self.assertFalse(any(method=='PATCH' for method,_ in calls))
    def test_workflow_is_manual_only_official_release_with_pre_release_integration_tests(self):
        root=pathlib.Path(__file__).resolve().parents[1]
        workflow=root/'.github/workflows/release.yml'
        content=workflow.read_text()
        mise_content=(root/'mise.toml').read_text()
        self.assertTrue(content.startswith('name: Release\n'))
        self.assertNotIn('internal release',content.lower())
        self.assertIn('workflow_dispatch:',content)
        self.assertNotIn('push:',content)
        self.assertIn('concurrency:',content)
        self.assertIn('cancel-in-progress: false',content)
        self.assertIn('timeout-minutes:',content)
        self.assertIn('Swatinem/rust-cache@v2',content)
        self.assertIn('~/Library/Caches/canban',content)
        self.assertIn('https://releases.herdr.dev/core/install.sh',content)
        self.assertIn('HC_VERSION:',content)
        self.assertNotIn('actions/checkout@v4',content)
        self.assertNotIn('jdx/mise-action@v2',content)
        self.assertNotIn('actions/upload-artifact@v4',content)
        self.assertNotIn('get.nexte.st/latest',content)
        self.assertIn('cargo-nextest',mise_content)
        self.assertIn('cargo nextest run --workspace --locked',mise_content)
        self.assertIn('CANBAN_GH_BIN = "/usr/bin/false"',mise_content)
        self.assertNotIn('\n      GH_TOKEN:',content.split('steps:')[0])
        self.assertIn('mise run test-integration',content)
        self.assertIn('scripts/test-install.py',content)
        self.assertIn('scripts/test-release-upgrade.py',content)
        self.assertIn('scripts/test-release-upgrade.py --dist dist',content)
        self.assertLess(content.index('mise run test-integration'),content.index('scripts/release.py reserve'))
        self.assertLess(content.index('scripts/test-install.py'),content.index('scripts/release.py reserve'))
        self.assertLess(content.index('scripts/test-release-upgrade.py\n'),content.index('scripts/release.py reserve'))
        self.assertLess(content.index('scripts/release.py reserve'),content.index('mise run build-release'))
        self.assertLess(content.index('mise run build-release'),content.index('scripts/release.py package'))
        self.assertLess(content.index('scripts/release.py package'),content.index('scripts/test-release-upgrade.py --dist dist'))
        self.assertLess(content.index('scripts/test-release-upgrade.py --dist dist'),content.index('scripts/release.py publish'))
        self.assertIn('cargo build --release --locked -p canban-cli',content)
        self.assertIn('scripts/release.py package-cli',content)
        self.assertIn('ubuntu-24.04-arm',content)
    def test_upgrade_gate_baseline_selection_and_redirect_auth_stripping(self):
        gate_spec=importlib.util.spec_from_file_location('test_release_upgrade',pathlib.Path(__file__).with_name('test-release-upgrade.py'))
        gate=importlib.util.module_from_spec(gate_spec);gate_spec.loader.exec_module(gate)
        releases=[
            {'tag_name':'20261115-10','draft':True,'assets':[{'name':'Canban-macos-aarch64.tar.gz'},{'name':'manifest-macos-aarch64.json'}]},
            {'tag_name':'20261115-9','draft':False,'assets':[{'name':'Canban-macos-aarch64.tar.gz','url':'https://api.github.com/assets/9'},{'name':'manifest-macos-aarch64.json'}]},
            {'tag_name':'20261115-10','draft':False,'assets':[{'name':'Canban-macos-aarch64.tar.zst','url':'https://api.github.com/assets/10'},{'name':'manifest-macos-aarch64.json'}]},
            {'tag_name':'20261115-11','draft':False,'assets':[{'name':'Canban-macos-aarch64.tar.zst'}]},  # missing manifest
        ]
        pick=gate.select_latest_published_release(releases,'macos-aarch64')
        self.assertEqual(pick['version'],'20261115-10')
        bounded=gate.select_latest_published_release(releases,'macos-aarch64',before_version='20261115-10')
        self.assertEqual(bounded['version'],'20261115-9')
        self.assertIsNone(gate.select_latest_published_release([],'macos-aarch64'))

        handler=gate._NoAuthRedirectHandler()
        orig=urllib.request.Request('https://api.github.com/repos/jjyr/canban/releases/assets/1',headers={'Authorization':'Bearer secret-token','User-Agent':'Canban'})
        cross=handler.redirect_request(orig,None,302,'Found',{},'https://objects.githubusercontent.com/signed-blob')
        self.assertFalse(cross.has_header('Authorization'))
        same=handler.redirect_request(orig,None,302,'Found',{},'https://api.github.com/repos/jjyr/canban/releases/assets/2')
        self.assertEqual(same.get_header('Authorization'),'Bearer secret-token')
    def test_dispatch_triggers_workflow_discovers_run_and_supports_watch(self):
        calls=[];subproc=[]
        now=release.datetime.datetime.now(release.datetime.timezone.utc).isoformat().replace('+00:00','Z')
        def api(method,path,body=None,**kwargs):
            calls.append((method,path,body,kwargs.get('token')))
            if method=='POST' and path=='/repos/fixture/app/actions/workflows/release.yml/dispatches':return None
            if method=='GET' and '/actions/workflows/release.yml/runs?' in path:
                return {'workflow_runs':[{'id':987,'run_number':12,'head_sha':'abcdef1234567','html_url':'https://github.com/fixture/app/actions/runs/987','created_at':now}]}
            raise AssertionError((method,path))
        def fake_run(argv,**kwargs):
            subproc.append(argv)
            return type('Result',(),{'returncode':0,'stdout':'gh-token-xyz\n'})()
        with patch.object(release,'api',api),patch.object(release.shutil,'which',return_value='/usr/local/bin/gh'),patch.object(release.subprocess,'run',side_effect=fake_run),patch.dict(os.environ,{},clear=True),patch.object(sys,'argv',['release.py','dispatch','--repo','fixture/app','--ref','release/branch','--watch']):
            release.main()
        self.assertEqual(calls[0],('POST','/repos/fixture/app/actions/workflows/release.yml/dispatches',{'ref':'release/branch'},'gh-token-xyz'))
        self.assertIn('branch=release%2Fbranch',calls[1][1])
        self.assertEqual(subproc,[['/usr/local/bin/gh','auth','token'],['/usr/local/bin/gh','run','watch','987','--repo','fixture/app','--exit-status']])
    def test_package_creates_tar_zst_and_tar_gz_with_manifest_digests(self):
        with tempfile.TemporaryDirectory(prefix='cb-release-pkg-') as directory:
            root=pathlib.Path(directory)
            (root/'target/release').mkdir(parents=True)
            (root/'.local/Canban.app/Contents/MacOS').mkdir(parents=True)
            (root/'.local/Canban.app/Contents/Resources').mkdir(parents=True)
            (root/'.local/Canban.app/Contents/Info.plist').write_text('plist')
            (root/'.local/Canban.app/Contents/MacOS/canban').write_text('#!/bin/sh\necho ok\n')
            build={'version':'20261114-1','commit':'c'*40,'schema':12,'api':36,'platform':'macos-aarch64'}
            cli=root/'target/release/canban'
            cli.write_text('#!/usr/bin/env python3\nimport json\nprint(json.dumps('+repr(build)+'))\n');cli.chmod(0o755)
            helper=root/'target/release/canban-updater'
            helper.write_text('#!/usr/bin/env python3\nimport pathlib, sys\npathlib.Path(sys.argv[3]).write_bytes(b"zst-fixture")\n');helper.chmod(0o755)
            dist=root/'dist'
            cwd=os.getcwd()
            try:
                os.chdir(root)
                with patch.dict(os.environ,{'CANBAN_RELEASE_COMMIT':'c'*40,'CANBAN_RELEASE_VERSION':'20261114-1'}),patch.object(sys,'argv',['release.py','package','--directory',str(dist)]):
                    release.main()
            finally:
                os.chdir(cwd)
            zst=dist/'Canban-macos-aarch64.tar.zst'
            gz=dist/'Canban-macos-aarch64.tar.gz'
            manifest_path=dist/'manifest-macos-aarch64.json'
            self.assertTrue(zst.is_file());self.assertTrue(gz.is_file());self.assertTrue(manifest_path.is_file())
            manifest=json.loads(manifest_path.read_text())
            self.assertEqual(manifest['asset'],'Canban-macos-aarch64.tar.zst')
            self.assertEqual(manifest['size'],zst.stat().st_size)
            self.assertEqual(manifest['sha256'],hashlib.sha256(zst.read_bytes()).hexdigest())
            self.assertEqual(manifest['tar_gz'],'Canban-macos-aarch64.tar.gz')
            self.assertEqual(manifest['tar_gz_size'],gz.stat().st_size)
            self.assertEqual(manifest['tar_gz_sha256'],hashlib.sha256(gz.read_bytes()).hexdigest())
            with tarfile.open(gz,'r:gz') as archive:
                names=archive.getnames()
            self.assertIn('Canban.app',names)
            self.assertIn('Canban.app/Contents/MacOS/canban',names)
            self.assertIn('Canban.app/Contents/Info.plist',names)
    def test_package_cli_creates_linux_tar_gz_and_manifest(self):
        with tempfile.TemporaryDirectory(prefix='cb-release-cli-pkg-') as directory:
            root=pathlib.Path(directory)
            (root/'target/release').mkdir(parents=True)
            build={'version':'20261115-1','commit':'d'*40,'schema':14,'api':52,'platform':'linux-x86_64'}
            cli=root/'target/release/canban'
            cli.write_text('#!/usr/bin/env python3\nimport json\nprint(json.dumps('+repr(build)+'))\n');cli.chmod(0o755)
            dist=root/'dist'
            cwd=os.getcwd()
            try:
                os.chdir(root)
                with patch.dict(os.environ,{'CANBAN_RELEASE_COMMIT':'d'*40,'CANBAN_RELEASE_VERSION':'20261115-1'}),patch.object(sys,'argv',['release.py','package-cli','--directory',str(dist)]):
                    release.main()
            finally:
                os.chdir(cwd)
            gz=dist/'canban-linux-x86_64.tar.gz'
            manifest_path=dist/'manifest-cli-linux-x86_64.json'
            self.assertTrue(gz.is_file());self.assertTrue(manifest_path.is_file())
            manifest=json.loads(manifest_path.read_text())
            self.assertEqual(manifest['asset'],'canban-linux-x86_64.tar.gz')
            self.assertEqual(manifest['sha256'],hashlib.sha256(gz.read_bytes()).hexdigest())
            with tarfile.open(gz,'r:gz') as archive:
                member=archive.getmember('canban')
                self.assertEqual(member.mode & 0o777,0o755)
if __name__=='__main__':unittest.main()
