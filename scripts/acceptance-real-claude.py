#!/usr/bin/env python3
"""Opt-in real Claude context/reply/normal-exit/Resume acceptance in isolated Core."""
from pathlib import Path
import json, os, shutil, subprocess, tempfile, time
repo=Path(__file__).resolve().parents[1]
base=Path(tempfile.mkdtemp(prefix='cb-real-claude-',dir='/private/tmp'));home=base/'home';cli=repo/'target/debug/canban'
env={**{k:v for k,v in os.environ.items() if not k.startswith('CANBAN_')},'CANBAN_HOME':str(home)}
core={'HC_SESSION':'real-claude-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'hc-state'),'XDG_CONFIG_HOME':str(base/'hc-config')}
log=open(base/'server.log','w');server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
trusted=set()
def call(method,p=None):
 r=subprocess.run([str(cli),"--full",'call',method,json.dumps(p or {})],env=env,text=True,capture_output=True,timeout=90)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def wait(predicate,label,timeout=180):
 end=time.monotonic()+timeout
 while time.monotonic()<end:
  try:s=call('app.snapshot')
  except RuntimeError:time.sleep(.2);continue
  for a in s['attempts']:
   if not a.get('terminal') or a.get('close_state')=='closed':continue
   screen=subprocess.run([shutil.which('hc'),'read',a['terminal']],env={**os.environ,**core},capture_output=True,text=True,timeout=15).stdout
   (base/('terminal-'+str(a['number'])+'-'+a['request_id']+'.txt')).write_text(screen)
   if any(text in screen for text in ['Invalid API key','Please run /login','Not logged in','Authentication failed','credit balance is too low','401 User not found','API Error: 401']):
    raise RuntimeError('Claude provider authentication or billing blocked; retained terminal evidence at '+str(base))
   if a['terminal'] not in trusted and str(base) in screen and ('Trust this folder?' in screen or 'Yes, I trust this folder' in screen):
    if '❯ No, exit' in screen:
     subprocess.run([shutil.which('hc'),'keys',a['terminal'],'down'],env={**os.environ,**core},capture_output=True,check=True,timeout=15)
    subprocess.run([shutil.which('hc'),'keys',a['terminal'],'enter'],env={**os.environ,**core},capture_output=True,check=True,timeout=15);trusted.add(a['terminal'])
  if predicate(s):return s
  if any(r['state'] in ['failed','unknown','needs_report'] for r in s['requests']):raise RuntimeError('Claude execution needs inspection: '+str(base))
  time.sleep(.5)
 raise RuntimeError(label+' timed out; evidence: '+str(base))
try:
 wait(lambda _:True,'AppServer startup',20)
 c=json.loads((home/'config/agents.json').read_text());c.update(hc=shutil.which('hc'),hc_env=core,hc_session=core['HC_SESSION']);(home/'config/agents.json').write_text(json.dumps(c));call('config.reload');call('scheduler.configure',{'idle_minutes':1})
 p=call('project.create',{'name':'Real Claude acceptance','directory':str(base/'source'),'workspace':{'create_worktree':False,'sync_base':False}})
 marker='CLAUDE_CONTEXT_'+base.name.replace('-','_')
 card=call('card.create',{'project_id':p['id'],'title':'Claude context and reply smoke','body':'Only exercise Canban replies. Do not change source files. Card context marker: '+marker+'. Follow current Question reply and outcome reporting instructions.'})
 call('comment.add',{'card_id':card['id'],'body':'@claude Read the Card context, then reply to this Question with its exact context marker and explicitly report completion. No source edits are needed.'})
 print('REAL_CLAUDE_HOME='+str(home),flush=True)
 s=wait(lambda s:s['attempts'] and s['attempts'][0]['close_state']=='closed' and s['questions'] and all(q['state']=='replied' for q in s['questions']),'real Claude task and normal exit')
 first=s['attempts'][0];sid=first['provider_session_id']
 q=s['questions'][0];assert marker in call('resource.read',{'card_id':card['id'],'path':q['reply_path']})['body']
 call('comment.add',{'card_id':card['id'],'body':'@claude Follow-up only: recall the exact context marker from our existing conversation. Do not change files. Reply using this Question ID and report completion.'})
 s=wait(lambda s:len(s['attempts'])==2 and all(a['close_state']=='closed' for a in s['attempts']) and all(q['state']=='replied' for q in s['questions']),'Claude exact Resume')
 assert s['attempts'][-1]['provider_session_id']==sid and s['attempts'][-1]['terminal']!=first['terminal']
 q=s['questions'][-1];assert marker in call('resource.read',{'card_id':card['id'],'path':q['reply_path']})['body']
 (base/'evidence.json').write_text(json.dumps({'status':'passed','snapshot':s,'core_env':core},indent=2))
 print('REAL_CLAUDE_NOTE_REPLY_EXIT_RESUME_PASSED\nBASE='+str(base),flush=True)
except Exception as error:
 (base/'failure.json').write_text(json.dumps({'error':str(error)},indent=2));print('REAL_CLAUDE_FAILED '+str(error)+'\nBASE='+str(base),flush=True);raise
finally:
 server.terminate();server.wait(timeout=10)
 subprocess.run([shutil.which('hc'),'server','stop'],env={**os.environ,**core},capture_output=True,timeout=20);log.close()
