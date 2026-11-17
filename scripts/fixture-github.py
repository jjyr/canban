#!/usr/bin/python3
"""Deterministic gh API transport for isolated acceptance; never contacts GitHub."""
import json, os, sys, time
from pathlib import Path
from urllib.parse import urlsplit, parse_qs
base=Path(os.environ['CANBAN_GITHUB_FIXTURE_DIR'])
state_path=base/'remote.json'
s=json.loads(state_path.read_text()) if state_path.exists() else {}
args=sys.argv[1:];method=args[args.index('--method')+1] if '--method' in args else 'GET'
path=next(a for a in args if a.startswith('/'));url=urlsplit(path);p=url.path;q=parse_qs(url.query);page=int(q.get('page',['1'])[0])
body=json.load(sys.stdin) if '--input' in args else None
with (base/'requests.jsonl').open('a') as f:f.write(json.dumps({'method':method,'path':path,'body':body})+'\n')
if method == 'GET' and p == s.get('delay_path'): time.sleep(s.get('delay_seconds', 2))
user={'id':7,'login':'fixture'}
def issue(n):
 return {'id':100+n,'node_id':f'I_{n}','number':n,'title':s.get('title','Cache-backed GitHub task') if n==1 else 'Review the native diff','body':'REMOTE_BODY_SENTINEL: GitHub source description.','state':s.get('issue_state','open'),'html_url':f'https://github.com/fixture/repo/'+('pull/2' if n==2 else f'issues/{n}'),'user':user,'labels':[{'name':'bug'}],'assignees':[user],'updated_at':s.get('updated_at','2026-11-10T00:00:00Z'),'pull_request':{'url':'https://api.github.com/repos/fixture/repo/pulls/2'} if n==2 else None}
pr={**issue(2),'id':202,'node_id':'PR_2','state':s.get('pr_state','open'),'merged':s.get('merged',False),'draft':False,'head':{'sha':s.get('head','a'*40),'ref':'feature/cache'},'base':{'sha':'b'*40,'ref':'main'},'requested_reviewers':[user]}
repo={'id':1,'full_name':'fixture/repo','default_branch':'main','html_url':'https://github.com/fixture/repo'}
code=200;link=None;poll=None;result=None
if method == 'GET' and p == s.get('fail_path'):
 print('HTTP/2 403 Forbidden\n\n'+json.dumps({'message':'fixture access denied'}));sys.exit(1)
if method!='GET':
 if body and body.get('body') in ['DENY_FIXTURE','UNKNOWN_FIXTURE']:
  code=403 if body['body']=='DENY_FIXTURE' else 500;result={'message':'Fixture rejection'}
 elif p.startswith('/notifications/threads/'):
  s['done']=method=='DELETE';s['unread']=False;code=204
 elif method in ['PATCH','PUT'] and '/reviews/' in p:
  rid=int(p.rsplit('/',1)[1]);items=s.setdefault('reviews',[]);found=next((x for x in items if x['id']==rid),None)
  if found is None:
   found={'id':rid,'html_url':f'https://github.com/fixture/repo/pull/2#pullrequestreview-{rid}','state':'COMMENTED','user':user,'submitted_at':'2026-11-10T01:00:00Z'}
   items.append(found)
  found['body']=body['body'];found['updated_at']='2026-11-10T01:30:00Z';result=found
 elif method=='PATCH' and '/pulls/comments/' in p:
  cid=int(p.rsplit('/',1)[1]);items=s.setdefault('threads',[]);found=next((x for x in items if x['id']==cid),None)
  if found is None:
   found={'id':cid,'html_url':f'https://github.com/fixture/repo/pull/2#discussion_r{cid}','path':'src/lib.rs','line':1,'side':'RIGHT','user':user,'created_at':'2026-11-10T01:00:00Z'}
   items.append(found)
  found['body']=body['body'];found['updated_at']='2026-11-10T01:30:00Z';result=found
 elif method=='PATCH' and '/issues/comments/' in p:
  cid=int(p.rsplit('/',1)[1]);items=s.setdefault('comments',[]);found=next((x for x in items if x['id']==cid),None)
  if found is None:
   found={'id':cid,'html_url':f'https://github.com/fixture/repo/pull/2#issuecomment-{cid}','user':user,'created_at':'2026-11-10T01:00:00Z'}
   items.append(found)
  found['body']=body['body'];found['updated_at']='2026-11-10T01:30:00Z';result=found
 elif method=='DELETE' and '/pulls/comments/' in p:
  cid=int(p.rsplit('/',1)[1]);s['threads']=[x for x in s.get('threads',[]) if x['id']!=cid];code=204
 elif method=='DELETE' and '/issues/comments/' in p:
  cid=int(p.rsplit('/',1)[1]);s['comments']=[x for x in s.get('comments',[]) if x['id']!=cid];code=204
 elif p.endswith('/reviews'):
  if body.get('commit_id')!=pr['head']['sha']:code=422;result={'message':'stale head'}
  else:
   rid=700+len(s.get('reviews',[]))
   result={'id':rid,'html_url':f'https://github.com/fixture/repo/pull/2#pullrequestreview-{rid}'};s.setdefault('reviews',[]).append({**result,'state':body['event'],'body':body.get('body',''),'user':user,'submitted_at':'2026-11-10T01:00:00Z'})
   for idx,c in enumerate(body.get('comments') or []):
    tid=800+len(s.get('threads',[]))+idx
    s.setdefault('threads',[]).append({'id':tid,'html_url':f'https://github.com/fixture/repo/pull/2#discussion_r{tid}','path':c['path'],'line':c['line'],'side':c.get('side','RIGHT'),'body':c['body'],'user':user,'created_at':'2026-11-10T01:00:00Z','updated_at':'2026-11-10T01:00:00Z'})
 elif p.endswith('/replies') and '/pulls/' in p:
  tid=650+len(s.get('threads',[]))
  result={'id':tid,'html_url':f'https://github.com/fixture/repo/pull/2#discussion_r{tid}','path':'src/lib.rs','line':1,'side':'RIGHT','in_reply_to_id':int(p.split('/')[-2]),'body':body['body'],'user':user,'created_at':'2026-11-10T01:00:00Z','updated_at':'2026-11-10T01:00:00Z'};s.setdefault('threads',[]).append(result)
 elif '/comments' in p:
  cid=600+len(s.get('comments',[]))
  result={'id':cid,'html_url':f'https://github.com/fixture/repo/pull/2#issuecomment-{cid}','body':body['body'],'user':user,'created_at':'2026-11-10T01:00:00Z','updated_at':'2026-11-10T01:00:00Z'};s.setdefault('comments',[]).append(result)
 else:code=404;result={'message':'fixture route not found'}
 state_path.write_text(json.dumps(s))
elif p=='/user':result=user
elif p=='/notifications':
 poll=1
 result=[] if s.get('done',False) else [{'id':'9' if page==1 else '10','unread':s.get('unread',True),'reason':'review_requested','subject':{'title':s.get('notification_title','Review the native diff') if page==1 else 'Cache-backed GitHub task','url':f'https://api.github.com/repos/fixture/repo/'+('pulls/2' if page==1 else 'issues/1'),'type':'PullRequest' if page==1 else 'Issue'},'repository':repo,'updated_at':'2026-11-10T00:00:00Z'}]
 if page==1:link='<https://api.github.com/notifications?page=2>; rel="next"'
elif p=='/search/issues':
 is_pr='is:pr' in q.get('q',[''])[0];n=2 if is_pr else (1 if page==1 else 3);result={'items':[issue(n)],'total_count':2}
 if page==1:link='<https://api.github.com/search/issues?page=2>; rel="next"'
elif p=='/repos/fixture/repo':result=repo
elif p.endswith('/collaborators'):result=[user]
elif p=='/repos/fixture/repo/issues':
 page_size=int(q.get('per_page',['100'])[0]);result=[issue(n) for n in range(1,s.get('issue_count',2)+1)][(page-1)*page_size:page*page_size]
elif p.startswith('/repos/fixture/repo/issues/') and p.endswith('/comments'):result=s.get('comments',[])
elif p.startswith('/repos/fixture/repo/issues/') and p.rsplit('/',1)[1].isdigit():result=issue(int(p.rsplit('/',1)[1]))
elif p=='/repos/fixture/repo/pulls':result=[pr]
elif p=='/repos/fixture/repo/pulls/2':result=pr
elif p.endswith('/pulls/2/files'):result=[{'filename':'src/lib.rs','status':'modified','additions':1,'deletions':1,'patch':'@@ -1 +1 @@\n-old_value\n+new_value'}]
elif p.endswith('/pulls/2/reviews'):result=s.get('reviews',[])
elif p.endswith('/pulls/2/comments'):result=s.get('threads',[])
elif p.endswith('/check-runs'):result={'check_runs':[{'name':'CI','status':'completed','conclusion':'success'}],'total_count':1}
elif p.endswith('/status'):result={'state':'success','total_count':1}
else:code=404;result={'message':'fixture route not found'}
print(f'HTTP/2.0 {code}')
print('Content-Type: application/json')
if link:print('Link: '+link)
if poll:print('X-Poll-Interval: '+str(poll))
print()
if code!=204:print(json.dumps(result))
sys.exit(0 if code<400 else 1)
