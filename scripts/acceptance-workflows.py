#!/usr/bin/env python3
"""Unified workflows over real isolated AppServer/Core/PTYS; deterministic Agents.
--native keeps a scratch packaged app open until <base>/stop exists.
"""
import argparse, json, os, plistlib, shutil, subprocess, tempfile, time
from pathlib import Path

args=argparse.ArgumentParser();args.add_argument('--native',action='store_true');options=args.parse_args()
repo=Path(__file__).resolve().parents[1]
base=Path(tempfile.mkdtemp(prefix='cb-workflows-',dir='/private/tmp'));home=base/'home';source=base/'source';source.mkdir()
cli=repo/'target/debug/canban'
core={'HC_SESSION':'workflow-acceptance','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'state'),'XDG_CONFIG_HOME':str(base/'config')}
env={k:v for k,v in os.environ.items() if not k.startswith(('CANBAN_','HC_'))};env.update(core);env['CANBAN_HOME']=str(home)
config=home/'config';config.mkdir(parents=True)
profiles=[{'id':p,'preset':None,'argv':['/usr/bin/python3',str(repo/'scripts/fixture-agent.py'),'--session-id','{session_id}','{prompt}'],'resume_argv':['/usr/bin/python3',str(repo/'scripts/fixture-agent.py'),'--resume','{session_id}','{prompt}'],'exit':{'type':'manual'},'session_instruction':''} for p in ['developer','reviewer','codex','claude']]
(config/'agents.json').write_text(json.dumps({'hc':shutil.which('hc'),'hc_env':core,'hc_session':core['HC_SESSION'],'default_agent':'developer','profiles':profiles}))
log=open(base/'server.log','a');server=None;ui=None

def start():
 global server
 server=subprocess.Popen([str(cli),'serve'],env=env,stdout=log,stderr=log)
 for _ in range(100):
  try:return call('app.ping')
  except RuntimeError:time.sleep(.1)
 raise RuntimeError('AppServer failed to start')
def call(method,p=None):
 r=subprocess.run([str(cli),'call',method,json.dumps(p or {})],env=env,text=True,capture_output=True,timeout=90)
 if r.returncode:raise RuntimeError(r.stderr)
 return json.loads(r.stdout)
def wait(predicate,label):
 end=time.monotonic()+60
 while time.monotonic()<end:
  s=call('app.snapshot')
  if predicate(s):return s
  time.sleep(.15)
 raise RuntimeError('Timeout: '+label+'\n'+(base/'server.log').read_text())
def plan_yaml(instruction):
 return f"""schema: 1
id: plan-deliver
name: Plan and deliver
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: plan
  develop:
    event: card.column_entered
    start_at: build
    match:
      column: develop
steps:
  - id: plan
    uses: agent
    with:
      profile: codex
      instruction: {instruction}
    next: approve
  - id: approve
    uses: approval
    with:
      title: Approve the recorded plan
    next: move
    on_changes_requested: plan
  - id: move
    uses: card.move
    with:
      column: develop
    next: build
  - id: build
    uses: agent
    with:
      profile: developer
      instruction: Implement the approved plan
    next: review_move
  - id: review_move
    uses: card.move
    with:
      column: review
    next: review
  - id: review
    uses: agent
    with:
      profile: reviewer
      instruction: Review code and approved plan
    next: finish
  - id: finish
    uses: complete
"""
def upsert_yaml(filename,raw,check=False):
 c=call('workflow.sources',{'project_id':project['id']})
 sources=[{'filename':f['filename'],'source':f['source']} for f in c['sources'] if f['filename']!=filename]
 sources.append({'filename':filename,'source':raw})
 return call('workflow.check' if check else 'workflow.apply',{'project_id':project['id'],'revision':c['revision'],'sources':sources})
try:
 start()
 project=call('project.create',{'name':'Workflow acceptance','directory':str(source),'workspace':{'create_worktree':False,'sync_base':False}})
 first_yaml=plan_yaml('Produce plan FIRST_VERSION')
 assert upsert_yaml('plan-deliver.yaml',first_yaml,check=True)['valid']
 upsert_yaml('plan-deliver.yaml',first_yaml)
 subscriber=call('subscription.save',{'revision':0,'enabled':True,'selector':{'project_id':project['id'],'types':['card.created']},'target':{'type':'workflow','workflow_id':'plan-deliver','entry':'start'}})
 observer=call('subscription.save',{'revision':0,'enabled':True,'selector':{'project_id':project['id'],'types':['*']},'target':{'type':'consumer'}})
 card=call('card.create',{'project_id':project['id'],'title':'Plan in Backlog','body':'Preserve three columns and exact plan approval.'})
 s=wait(lambda s:any(r['state']=='waiting_approval' for r in s['workflow_runs']),'planning completes')
 run=s['workflow_runs'][0];assert s['cards'][0]['stage']=='backlog';assert len(s['requests'])==1
 upsert_yaml('plan-deliver.yaml',plan_yaml('Produce plan SECOND_VERSION'))
 assert call('workflow.run',{'run_id':run['id']})['definition_revision']==1
 before=len(s['requests']);server.terminate();server.wait(timeout=10);start()
 s=call('app.snapshot');assert len(s['requests'])==before and s['workflow_runs'][0]['state']=='waiting_approval'
 current=s['cards'][0];decision={'run_id':run['id'],'visit':run['visit'],'revision':current['revision'],'decision':'approve','request_key':'approve-plan-once'}
 call('workflow.decide',decision);call('workflow.decide',decision)
 s=wait(lambda s:s['workflow_runs'][0]['state']=='completed','development and review finish')
 assert len(s['requests'])==3 and s['cards'][0]['stage']=='review'
 assert len({r['workflow_run_id'] for r in s['requests']})==1
 assert any('report_id' in r['instruction'] for r in s['requests'][1:])
 deliveries=call('subscription.deliveries',{'subscription_id':observer['id']});assert deliveries
 call('subscription.ack',{'delivery_id':deliveries[0]['id']})
 second=call('card.create',{'project_id':project['id'],'title':'New revision','body':'Use the published revision immediately.'})
 s=wait(lambda s:len(s['workflow_runs'])==2 and s['workflow_runs'][1]['state']=='waiting_approval','new revision planning')
 assert s['workflow_runs'][1]['definition_revision']==2
 assert 'SECOND_VERSION' in next(r for r in s['requests'] if r['card_id']==second['id'])['instruction']
 evidence={'base':str(base),'project_id':project['id'],'card_id':second['id'],'first_run':run['id'],'api_version':call('app.ping')['api_version'],'schema':json.loads(subprocess.check_output([str(cli),'build-info'],env=env,text=True))['schema'],'checks':['backlog planning','explicit approval','frozen run revision','immediate new revision','restart checkpoint','duplicate decision receipt','single workflow-owned movement','real isolated PTY reports','consumer acknowledgement']}
 # Project YAML authoring and a configuration Agent share real transport,
 # without creating a Card/assignment or sending the initial prompt twice.
 params_file=base/'source-query.json';params_file.write_text(json.dumps({'project_id':project['id']}))
 collection=json.loads(subprocess.check_output([str(cli),'call','workflow.sources','@'+str(params_file)],env=env,text=True))
 assert len(collection['sources'])==2
 custom=next(f for f in collection['sources'] if f['active']['workflow_id']=='plan-deliver')
 assert custom['source'].startswith('schema: 1')
 assert call('workflow.get',{'workflow_id':custom['active']['workflow_id'],'revision':custom['active']['revision']})['source_yaml']==custom['source']
 assert not (home/'projects'/project['id']/'workflows').exists() and not (home/'config/workflows').exists()
 changed=[{'filename':f['filename'],'source':f['source']} for f in collection['sources']]
 custom=next(f for f in changed if f['filename']==custom['filename']);custom['source']='# Keep this original YAML comment\n'+custom['source']
 applied=call('workflow.apply',{'project_id':project['id'],'revision':collection['revision'],'sources':changed})
 assert next(f for f in applied['sources'] if f['filename']==custom['filename'])['source']==custom['source']
 try:call('workflow.apply',{'project_id':project['id'],'revision':collection['revision'],'sources':changed})
 except RuntimeError as e:assert 'revision_conflict' in str(e)
 else:raise AssertionError('Stale source overwrote newer publication')
 # A chosen external file is only transport; exporting does not create an
 # internal mirror, and submitting requires the current SQL collection token.
 exported=base/'exported-workflow.yaml'
 result=subprocess.run([str(cli),'workflow','export','--project-id',project['id'],'--name',custom['filename'],'--output',str(exported)],env=env,text=True,capture_output=True,check=True)
 assert exported.read_text()==custom['source']
 stale_export=subprocess.run([str(cli),'workflow','export','--project-id',project['id'],'--name',custom['filename'],'--output',str(exported)],env=env,text=True,capture_output=True);assert stale_export.returncode!=0
 submit_args=[str(cli),'workflow','submit','--project-id',project['id'],'--revision',applied['revision'],'--file',str(exported),'--name',custom['filename']]
 checked=subprocess.run(submit_args+['--check'],env=env,text=True,capture_output=True,check=True);assert json.loads(checked.stdout)['valid']
 assert call('workflow.sources',{'project_id':project['id']})['revision']==applied['revision']
 subprocess.run(submit_args,env=env,text=True,capture_output=True,check=True)
 assert not (home/'config/workflows').exists() and not (home/'projects'/project['id']/'workflows').exists()
 prompt=call('workflow.prompt',{'project_id':project['id']})['prompt']
 assert 'Project directory: '+str(source) in prompt and 'Workflow directory:' not in prompt
 before=call('app.snapshot');session=call('configuration.open',{'project_id':project['id'],'profile_id':'codex','prompt':prompt})
 s=wait(lambda s:(home/'configuration-evidence/1.json').exists(),'configuration Agent starts')
 payload=json.loads((home/'configuration-evidence/1.json').read_text());assert payload['prompt']==prompt
 assert len(s['requests'])==len(before['requests']) and len(s['cards'])==len(before['cards'])
 reused=call('configuration.open',{'project_id':project['id'],'profile_id':'codex','prompt':prompt});assert reused['reused'] and reused['terminal_id']==session['terminal_id']
 assert len(list((home/'configuration-evidence').glob('*.json')))==1
 history=s['configuration_sessions'][0]['provider_session_id'];assert history
 call('terminal.close',{'terminal_id':session['terminal_id']})
 wait(lambda s:next(t for t in s['terminals'] if t['id']==session['terminal_id'])['runtime']['state']=='finished','configuration exit observed')
 call('configuration.open',{'project_id':project['id'],'profile_id':'codex','prompt':prompt})
 s=wait(lambda s:(home/'configuration-evidence/2.json').exists(),'exact configuration history resumes')
 assert s['configuration_sessions'][0]['provider_session_id']==history
 assert '--resume' in json.loads((home/'configuration-evidence/2.json').read_text())['argv']
 evidence['checks']+=['original YAML preservation','collection revision conflict','SQL source revision matches publication','explicit YAML file submit/export','Project-owned configuration PTY','exact preview and launch prompt','Continue without reinjection','exact provider history resume']
 (base/'evidence.json').write_text(json.dumps(evidence,indent=2));print('WORKFLOW_ACCEPTANCE_OK '+str(base),flush=True)
 if options.native:
  bundle=base/'CanbanWorkflowAcceptance.app';shutil.copytree(repo/'.local/Canban.app',bundle)
  plist=bundle/'Contents/Info.plist';info=plistlib.loads(plist.read_bytes());info['CFBundleIdentifier']='com.canban.workflows.'+base.name;info['CFBundleName']='Canban Workflow Acceptance';plist.write_bytes(plistlib.dumps(info))
  subprocess.run(['codesign','--force','--deep','--sign','-',str(bundle)],check=True,capture_output=True)
  env['CANBAN_APP_ID']=info['CFBundleIdentifier'];ui=subprocess.Popen([str(bundle/'Contents/MacOS/canban-ui')],env=env,stdout=log,stderr=log)
  (base/'native.json').write_text(json.dumps({**evidence,'bundle':str(bundle),'pid':ui.pid,'home':str(home)},indent=2));(repo/'.local/workflow-acceptance.json').write_text((base/'native.json').read_text())
  print('NATIVE_WORKFLOW_APP '+str(bundle),flush=True)
  while not (base/'stop').exists() and ui.poll() is None:time.sleep(.2)
finally:
 if ui is not None and ui.poll() is None:ui.terminate();ui.wait(timeout=10)
 if server is not None and server.poll() is None:server.terminate();server.wait(timeout=10)
 subprocess.run(['hc','server','stop'],env={**env,**core},capture_output=True,timeout=15)
 log.close()
