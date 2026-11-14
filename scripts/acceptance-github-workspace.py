#!/usr/bin/python3
"""Real AppServer/gh-process acceptance with a deterministic remote, optionally native UI.
No real GitHub writes, user credentials or live Herdr Core are used.
"""
import argparse, json, os, plistlib, shutil, sqlite3, subprocess, tempfile, time
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--native',action='store_true');args=parser.parse_args()
root=Path(__file__).resolve().parents[1];base=Path(tempfile.mkdtemp(prefix='cb-github-workspace-',dir='/private/tmp'));home=base/'home'
cli=root/'target/debug/canban';fixture=root/'scripts/fixture-github.py';fixture.chmod(0o755)
core={'HC_SESSION':'github-workspace-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'state'),'XDG_CONFIG_HOME':str(base/'config')}
env={k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')};env.update(core);env.update(CANBAN_HOME=str(home),CANBAN_GH_BIN=str(fixture),CANBAN_GITHUB_FIXTURE_DIR=str(base))
# All runtime connections use this Core from the first bootstrap; no default
# session briefly competes for its socket. Browser fixtures never invoke a model.
(home/'config').mkdir(parents=True,exist_ok=True)
(home/'config/agents.json').write_text(json.dumps({'hc':shutil.which('hc'),'hc_session':core['HC_SESSION'],'hc_env':core,'default_agent':'developer','profiles':[{'id':name,'argv':['/bin/echo','Browser fixture: no Agent execution requested']} for name in ['developer','reviewer']]}))
log=(base/'server.log').open('w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log);ui=None

def call(method,p=None,check=True):
 r=subprocess.run([str(cli),'call',method,json.dumps(p or {})],env=env,capture_output=True,text=True,timeout=90)
 if check and r.returncode:raise RuntimeError(method+': '+r.stderr)
 return json.loads(r.stdout) if not r.returncode else {'error':r.stderr}
def ready():
 for _ in range(150):
  try:return call('app.ping')
  except (RuntimeError,FileNotFoundError):time.sleep(.1)
 raise RuntimeError('AppServer did not start')
def wait(predicate):
 for _ in range(100):
  s=call('app.snapshot')
  if predicate(s):return s
  time.sleep(.1)
 raise AssertionError('Expected Card state did not appear')
def requests():return [json.loads(l) for l in (base/'requests.jsonl').read_text().splitlines()]
def remote(**changes):
 path=base/'remote.json';value=json.loads(path.read_text()) if path.exists() else {};value.update(changes);path.write_text(json.dumps(value))
try:
 ready();assert call('github.cli.status')['login']=='fixture'
 source=base/'source';source.mkdir()
 subprocess.run(['git','-C',str(source),'init','-b','main'],check=True,capture_output=True)
 subprocess.run(['git','-C',str(source),'remote','add','origin','git@github.com:fixture/repo.git'],check=True,capture_output=True)
 detected=call('project.detect_github',{'machine':'local','directory':str(source)})
 assert detected['detected']=={'repository':'fixture/repo','remote':'origin'}
 p=call('project.create',{'name':'GitHub workspace acceptance','directory':str(source)})
 assert p['github']['repository']=='fixture/repo' and p['github']['intake']=='off' and p['github']['pr_intake']=='off'
 call('github.sync',{'project_id':p['id']});assert call('app.snapshot')['cards']==[]
 call('project.github',{'project_id':p['id'],'repository':'fixture/repo','intake':'all','pr_intake':'all'})
 call('github.sync',{'project_id':p['id']})
 s=wait(lambda s:len(s['cards'])==2);assert all(c['stage']=='backlog' for c in s['cards']);assert not [t for t in s['terminals'] if t.get('agent')]
 issue=next(c for c in s['cards'] if c['content']['kind']=='github_issue');prcard=next(c for c in s['cards'] if c['content']['kind']=='github_pr')
 r={'repository':'fixture/repo','kind':'pr','number':2,'sha':None,'url':'https://github.com/fixture/repo/pull/2'}
 restored=call('github.item.add',{'project_id':p['id'],'reference':r});assert restored['id']==prcard['id']
 local=call('card.create',{'project_id':p['id'],'title':'Local implementation task','body':'Keep this content local.'})
 local=call('github.reference.add',{'card_id':local['id'],'revision':local['revision'],'url':r['url']});assert local['content']['kind']=='local'
 q={'kind':'notifications','state':'inbox','relation':'all','page':1}
 first=call('github.browse',q);assert first['has_next'];assert call('github.browse',q)['cached']
 second=call('github.browse',{**q,'page':2});assert second['items'][0]['reference']['kind']=='issue';assert not second['has_next']
 overview=call('github.detail',{'reference':r})['data'];assert overview['head']['sha']=='a'*40
 files=call('github.detail',{'reference':r,'section':'files','head':'a'*40});assert files['data'][0]['filename']=='src/lib.rs'
 assert call('github.detail',{'reference':r,'section':'files','head':'a'*40})['cached']
 comment={'reference':r,'body':'ACCEPTANCE_COMMENT_SENTINEL','request_key':'publish-once'}
 assert call('github.remote.comment',comment)['state']=='published';call('github.remote.comment',comment)
 assert len([v for v in requests() if v['method']=='POST' and v['path'].endswith('/comments')])==1
 review={'reference':r,'head':'a'*40,'body':'Review accepted fixture changes.','event':'COMMENT','comments':[{'path':'src/lib.rs','line':1,'side':'RIGHT','body':'Line comment'}],'request_key':'review-once'}
 assert call('github.remote.review',review)['state']=='published';call('github.remote.review',review)
 assert len([v for v in requests() if v['method']=='POST' and v['path'].endswith('/reviews')])==1
 reply={'reference':r,'comment_id':601,'body':'ACCEPTANCE_REPLY_SENTINEL','request_key':'reply-once'}
 assert call('github.remote.reply',reply)['state']=='published';call('github.remote.reply',reply)
 assert len([v for v in requests() if v['method']=='POST' and v['path'].endswith('/comments/601/replies')])==1
 edit_disc={'reference':r,'section':'discussion','comment_id':600,'body':'ACCEPTANCE_EDITED_COMMENT_SENTINEL','request_key':'edit-disc-once'}
 assert call('github.remote.comment.update',edit_disc)['state']=='published';call('github.remote.comment.update',edit_disc)
 assert len([v for v in requests() if v['method']=='PATCH' and v['path'].endswith('/issues/comments/600')])==1
 disc_after=call('github.detail',{'reference':r,'section':'discussion'})['data']
 assert any(c['id']==600 and c['body']=='ACCEPTANCE_EDITED_COMMENT_SENTINEL' for c in disc_after)
 edit_thread={'reference':r,'section':'threads','comment_id':601,'body':'ACCEPTANCE_EDITED_THREAD_SENTINEL','request_key':'edit-thread-once'}
 assert call('github.remote.comment.update',edit_thread)['state']=='published';call('github.remote.comment.update',edit_thread)
 assert len([v for v in requests() if v['method']=='PATCH' and v['path'].endswith('/pulls/comments/601')])==1
 threads_after=call('github.detail',{'reference':r,'section':'threads'})['data']
 assert any(c['id']==601 and c['body']=='ACCEPTANCE_EDITED_THREAD_SENTINEL' for c in threads_after)
 edit_rev={'reference':r,'section':'reviews','comment_id':700,'body':'ACCEPTANCE_EDITED_REVIEW_SENTINEL','request_key':'edit-rev-once'}
 assert call('github.remote.comment.update',edit_rev)['state']=='published';call('github.remote.comment.update',edit_rev)
 assert len([v for v in requests() if v['method']=='PUT' and v['path'].endswith('/pulls/2/reviews/700')])==1
 revs_after=call('github.detail',{'reference':r,'section':'reviews'})['data']
 assert any(rv['id']==700 and rv['body']=='ACCEPTANCE_EDITED_REVIEW_SENTINEL' for rv in revs_after)
 assert 'error' in call('github.remote.comment.delete',{'reference':r,'section':'reviews','comment_id':700,'request_key':'del-rev-rejected'},check=False)
 del_thread={'reference':r,'section':'threads','comment_id':800,'request_key':'del-thread-once'}
 assert call('github.remote.comment.delete',del_thread)['state']=='published';call('github.remote.comment.delete',del_thread)
 assert len([v for v in requests() if v['method']=='DELETE' and v['path'].endswith('/pulls/comments/800')])==1
 assert all(c['id']!=800 for c in call('github.detail',{'reference':r,'section':'threads'})['data'])
 assert all(c['id']!=800 for c in call('review.github_pr',{'repository':'fixture/repo','number':2})['threads'])
 del_disc={'reference':r,'section':'discussion','comment_id':600,'request_key':'del-disc-once'}
 assert call('github.remote.comment.delete',del_disc)['state']=='published';call('github.remote.comment.delete',del_disc)
 assert len([v for v in requests() if v['method']=='DELETE' and v['path'].endswith('/issues/comments/600')])==1
 assert all(c['id']!=600 for c in call('github.detail',{'reference':r,'section':'discussion'})['data'])
 denied={'reference':r,'body':'DENY_FIXTURE','request_key':'denied-comment'}
 assert call('github.remote.comment',denied)['state']=='failed'
 unknown={'reference':r,'body':'UNKNOWN_FIXTURE','request_key':'unknown-comment'}
 assert call('github.remote.comment',unknown)['state']=='unknown';call('github.remote.comment',unknown)
 assert len([v for v in requests() if v.get('body') and v['body'].get('body')=='UNKNOWN_FIXTURE'])==1
 remote(head='c'*40,title='Refreshed source title',updated_at='2026-11-10T02:00:00Z')
 assert 'error' in call('github.remote.review',{**review,'request_key':'stale-review'},check=False)
 assert len([v for v in requests() if v['method']=='POST' and v['path'].endswith('/reviews')])==1
 call('github.sync',{'project_id':p['id']});s=call('app.snapshot');assert next(c for c in s['cards'] if c['id']==issue['id'])['title']=='Refreshed source title';assert not [t for t in s['terminals'] if t.get('agent')]
 call('github.notification.read',{'thread_id':'9','request_key':'read-once'});call('github.notification.read',{'thread_id':'9','request_key':'read-once'});assert call('github.browse',q)['items'][0]['item']['unread']==False
 db=sqlite3.connect(home/'canban.sqlite');content=''.join(row[0] for row in db.execute('SELECT data FROM cards UNION ALL SELECT data FROM events UNION ALL SELECT data FROM github_issues'));assert 'REMOTE_BODY_SENTINEL' not in content;assert 'ACCEPTANCE_COMMENT_SENTINEL' not in content;assert 'ACCEPTANCE_REPLY_SENTINEL' not in content;assert 'ACCEPTANCE_EDITED_COMMENT_SENTINEL' not in content;assert 'ACCEPTANCE_EDITED_THREAD_SENTINEL' not in content;assert 'ACCEPTANCE_EDITED_REVIEW_SENTINEL' not in content;assert db.execute('SELECT count(*) FROM github_issues').fetchone()[0]==0;db.close()
 call('app.shutdown');server.wait(timeout=15);server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log);ready();assert call('github.browse',q)['items'];assert len(call('app.snapshot')['cards'])==3
 before_state=call('app.state')
 call('github.browse',{'kind':'issues','relation':'all','state':'open','query':'new query cache','page':2})
 after_state=call('app.state');assert after_state['snapshot']['cards']==before_state['snapshot']['cards'];assert after_state['github_revision']>before_state['github_revision']
 results={'passed':True,'base':str(base),'home':str(home),'project_id':p['id'],'issue_card':issue['id'],'pr_card':prcard['id'],'local_card':local['id'],'checks':['automatic Issue/PR Backlog intake','manual dedup','Local references','notification pagination/cache','versioned Diff','comment and Review receipts','stale-head rejection','source refresh without execution','notification read receipt','no mirrored remote bodies','restart/cache restore','cache-only changes propagate without Card changes']}
 (base/'results.json').write_text(json.dumps(results,indent=2));print(json.dumps(results),flush=True)
 if args.native:
  app_id='com.canban.github-acceptance.'+base.name;bundle=base/'CanbanGitHubAcceptance.app';shutil.copytree(root/'.local/Canban.app',bundle)
  info=bundle/'Contents/Info.plist';value=plistlib.loads(info.read_bytes());value.update(CFBundleIdentifier=app_id,CFBundleDisplayName='Canban GitHub Acceptance',CFBundleName='Canban GitHub Acceptance');info.write_bytes(plistlib.dumps(value))
  ui=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env={**env,'CANBAN_APP_ID':app_id},stdout=log,stderr=log)
  results.update(app_id=app_id,bundle=str(bundle),ui_pid=ui.pid);(root/'.local/github-workspace-acceptance.json').write_text(json.dumps(results,indent=2));print('NATIVE_GITHUB_ACCEPTANCE='+str(base),flush=True);ui.wait()
finally:
 try:call('app.shutdown')
 except Exception:pass
 if ui and ui.poll() is None:ui.terminate();ui.wait(timeout=10)
 if server.poll() is None:server.terminate();server.wait(timeout=15)
 subprocess.run(['hc','server','stop'],env={**env,**core},capture_output=True,timeout=15);log.close()
