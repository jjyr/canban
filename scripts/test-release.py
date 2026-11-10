#!/usr/bin/env python3
"""Offline release-number reservation and immutable publication regressions."""
import sys
sys.dont_write_bytecode=True
import importlib.util, pathlib, unittest, urllib.error, os
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
        def api(method,path,body=None,**kwargs):calls.append(method);return {'draft':False}
        with patch.object(release,'api',api),patch.object(sys,'argv',['release.py','publish','--directory','/private/tmp/canban-release-test-empty']),patch.dict(os.environ,{'CANBAN_RELEASE_COMMIT':'c'*40,'CANBAN_RELEASE_VERSION':'20261110-1'}):release.main()
        self.assertEqual(calls,['GET'])
if __name__=='__main__':unittest.main()
