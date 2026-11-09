import unittest,importlib.util,tempfile,json,os
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('probe',Path(__file__).with_name('probe.py'));p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
class ProbeTests(unittest.TestCase):
    def test_no_signin_hides_and_api_keys_do_not_become_subscription_tokens(self):
        with tempfile.TemporaryDirectory() as d,patch.dict(os.environ,{'HOME':d,'CODEX_HOME':d,'CLAUDE_CONFIG_DIR':d},clear=True),patch.object(p.sys,'platform','linux'):
            self.assertEqual(p.codex()['auth'],'none');self.assertEqual(p.claude()['auth'],'none')
            Path(d,'auth.json').write_text(json.dumps({'OPENAI_API_KEY':'fixture-secret'}));self.assertEqual(p.codex()['auth'],'none')
    def test_codex_extracts_windows_without_returning_auth(self):
        with tempfile.TemporaryDirectory() as d,patch.dict(os.environ,{'HOME':d,'CODEX_HOME':d},clear=True),patch.object(p,'fetch',return_value=({'rate_limit':{'primary_window':{'used_percent':22,'reset_at':1800000000,'limit_window_seconds':604800},'secondary_window':{'used_percent':68,'limit_window_seconds':18000}}},None)):
            Path(d,'auth.json').write_text(json.dumps({'tokens':{'access_token':'fixture-secret','account_id':'fixture-id'}}))
            r=p.codex();self.assertEqual([w['name'] for w in r['windows']],['Weekly','Session']);self.assertEqual(r['windows'][0]['used'],22);self.assertNotIn('fixture-secret',json.dumps(r))
    def test_claude_file_and_new_model_windows_and_alternative_endpoint(self):
        with tempfile.TemporaryDirectory() as d,patch.dict(os.environ,{'HOME':d,'CLAUDE_CONFIG_DIR':d},clear=True),patch.object(p.sys,'platform','linux'),patch.object(p,'fetch',return_value=({'limits':[{'kind':'session','percent':34,'resets_at':'tomorrow'},{'kind':'weekly_scoped','scope':{'model':{'display_name':'Sonnet'}},'percent':48}]},None)):
            Path(d,'.credentials.json').write_text(json.dumps({'claudeAiOauth':{'accessToken':'fixture-secret'}}))
            r=p.claude();self.assertEqual([w['used'] for w in r['windows']],[34,48]);self.assertNotIn('fixture-secret',json.dumps(r))
            Path(d,'settings.json').write_text(json.dumps({'apiKeyHelper':'other'}));self.assertEqual(p.claude()['auth'],'none')
    def test_cswap_camelcase_schema_and_stale_windows(self):
        import subprocess
        result=subprocess.CompletedProcess([],0,stdout=json.dumps({'schema_version':1,'accounts':[{'email':'demo','lastGoodUsage':{'fiveHour':{'pct':27,'resetsAt':'tomorrow'}},'usageStatus':'token_expired'}]}).encode())
        with patch.object(p,'command',return_value=result):
            r=p.cswap();self.assertEqual(r[0]['windows'][0]['used'],27);self.assertTrue(r[0]['stale'])
if __name__=='__main__':unittest.main()
