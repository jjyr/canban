#!/usr/bin/env python3
"""Interactive, persistent deterministic provider used only in scratch acceptance."""
import json,os,re,sys,uuid,termios,tty,base64
from pathlib import Path
import subprocess
home=Path(os.environ['CANBAN_HOME']);request=os.environ['CANBAN_REQUEST_ID'];attempt=int(os.environ['CANBAN_ATTEMPT'])
args=sys.argv[1:];session=args[args.index('--resume')+1] if '--resume' in args else str(uuid.uuid4())
history=home/'fixture-history'/f'{session}.json';history.parent.mkdir(exist_ok=True)
if '--resume' in args and not history.exists():
 print('PROVIDER_HISTORY_MISSING '+session,flush=True);sys.exit(3)
saved=json.loads(history.read_text()) if history.exists() else {'session':session,'turns':0}
history.write_text(json.dumps(saved))
def call(method,p):
 r=subprocess.run([os.environ['CANBAN_CLI'],'call',method,json.dumps(p)],capture_output=True,text=True,check=True);return json.loads(r.stdout)
call('agent.bind',{'request_id':request,'attempt':attempt,'session_id':session})
manifest=next(home.glob(f'projects/*/cards/*/runs/{request}/{attempt}/context.json'));context=json.loads(manifest.read_text())
print(f'\x1b[36mINTERACTIVE_READY {session}\x1b[0m',flush=True)
print('\x1b[?2004h',end='',flush=True)
def state(word,kind=None,title='',msg=''):
 fields=['state='+word]
 if kind:fields.append('kind='+kind)
 for k,v in [('title',title),('msg',msg)]:
  if v:fields.append(k+'='+base64.b64encode(v.encode()).decode())
 print('\x1b]7501;'+':'.join(fields)+'\x1b\\',end='',flush=True)
def reply(q):
 f=manifest.parent/f'answer-{q["id"]}.md';f.write_text(f'Answer for {q["id"]}; session={session}; turns={saved["turns"]}; Card={context["card"]["id"]}\n')
 call('agent.reply',{'request_key':'fixture-reply:'+q['id'],'question_id':q['id'],'request_id':request,'attempt':attempt,'body':f.read_text()})
 saved['turns']+=1;history.write_text(json.dumps(saved));print('QUESTION_REPLIED '+q['id'],flush=True)
def finish():
 history.write_text(json.dumps(saved))
 f=manifest.parent/'fixture-report.md';f.write_text(f'# Interactive report\nSession: {session}\nCard: {context["card"]["id"]}\n')
 call('execution.report',{'request_key':f'fixture-complete:{request}:{attempt}','execution':{'request_id':request,'attempt_number':attempt},'state':'completed','message':'Interactive fixture completed','body':f.read_text()})
 print('FINAL_RESPONSE_SAVED',flush=True)
 state('done')
state('working')
initial=context.get('questions',[])
if context['request']['stage']=='review' and context['request']['trigger']=='stage_entry':finish()
for q in initial:reply(q)
if initial:finish()
old=termios.tcgetattr(sys.stdin.fileno());tty.setraw(sys.stdin.fileno())
try:
 data='';paste=False
 while True:
  part=os.read(sys.stdin.fileno(),8192)
  if not part:break
  data+=part.decode('utf8',errors='replace')
  if data in ('\x1b','\x03'):
   state('idle');print('\r\nINTERRUPTED\r\n',end='',flush=True);data='';continue
  # Herdr uses bracketed paste; embedded newlines belong to one message.
  if '\x1b[200~' in data and '\x1b[201~' not in data:continue
  if data.endswith(('\r','\n')):
   line=data.replace('\x1b[200~','').replace('\x1b[201~','').strip();data=''
   if line=='/exit':history.write_text(json.dumps(saved));print('\r\nNORMAL_EXIT',flush=True);break
   with (manifest.parent/'input.jsonl').open('a') as audit:audit.write(json.dumps(line,ensure_ascii=False)+'\n')
   if line=='flood':
    for n in range(12000):print(f'\r\nLONG_OUTPUT {n:05d} · 中文 · '+('output '*15),end='')
    print('\r\nLONG_OUTPUT_COMPLETE\r\n',end='',flush=True);continue
   state('working')
   match=re.search(r'Canban question ([^\s]+)',line)
   if match:
    qid=match.group(1);q=next(q for q in call('app.snapshot',{})['questions'] if q['id']==qid);reply(q);state('working')
   elif line.startswith('note-write '):
    _,note_id,body=line.split(' ',2)
    note=call('note.get',{'note_id':note_id})
    result=subprocess.run([os.environ['CANBAN_CLI'],'note','write',note_id,'--revision',str(note['revision'])],input=body,capture_output=True,text=True,check=True)
    print('\r\nNOTE_UPDATED '+note_id+'\r\n',end='',flush=True)
   elif line in ('permission','permission-update','auth','question'):
    state('blocked','permission' if line.startswith('permission') else line,'Agent input required','Allow git diff?' if line=='permission-update' else 'Allow git status?')
    print('RUNTIME_WAIT '+line,flush=True)
   elif line=='idle':state('idle');print('IDLE_READY',flush=True)
   elif line=='working':state('working');print('WORKING_READY',flush=True)
   elif line=='done-only':state('done');print('DONE_WITHOUT_REPORT',flush=True)
   elif line=='block':
    call('execution.report',{'request_key':f'fixture-block:{request}:{attempt}','execution':{'request_id':request,'attempt_number':attempt},'state':'blocked','message':'Human answer required'})
    state('idle');print('HUMAN_INPUT_REQUIRED',flush=True)
   elif line=='finish':finish()
   else:print('\r\nINPUT '+line+'\r\n',end='',flush=True)
finally:termios.tcsetattr(sys.stdin.fileno(),termios.TCSADRAIN,old)
