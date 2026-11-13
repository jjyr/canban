#!/usr/bin/env python3
"""Thin Agent CLI: files and stdin are read here; product state stays on AppServer.
The private spool travels over Core's authenticated file/exec connection. Each
call keeps the same request key across reconnects or a server restart.
"""
import argparse, json, os, pathlib, sys, time, uuid

def body(file):
    return pathlib.Path(file).read_text() if file and file != '-' else sys.stdin.read()

def main():
    parser=argparse.ArgumentParser(prog='canban')
    parser.add_argument('--endpoint', default=os.environ.get('CANBAN_ENDPOINT'))
    sub=parser.add_subparsers(dest='cmd',required=True)
    call=sub.add_parser('call');call.add_argument('method');call.add_argument('params',nargs='?',default='{}')
    sub.add_parser('snapshot');sub.add_parser('status')
    report=sub.add_parser('report');report.add_argument('--state',required=True);report.add_argument('--message',default='');report.add_argument('--file');report.add_argument('--request-key')
    bind=sub.add_parser('bind-session');bind.add_argument('--session-id',default=os.environ.get('CANBAN_SESSION_ID'),required=not bool(os.environ.get('CANBAN_SESSION_ID')))
    reply=sub.add_parser('reply');reply.add_argument('--to',required=True);reply.add_argument('--file',required=True)
    for p in [report,bind,reply]:
        p.add_argument('--request-id',default=os.environ.get('CANBAN_REQUEST_ID'));p.add_argument('--attempt',type=int,default=os.environ.get('CANBAN_ATTEMPT'))
    card=sub.add_parser('card');cards=card.add_subparsers(dest='card_op',required=True)
    cu=cards.add_parser('update');cu.add_argument('--title');cu.add_argument('--file');cu.add_argument('--request-key')
    ca=cards.add_parser('adopt');ca.add_argument('--stage',default='develop');ca.add_argument('--request-key')
    for p in [cu,ca]:
        p.add_argument('--request-id',default=os.environ.get('CANBAN_REQUEST_ID'));p.add_argument('--attempt',type=int,default=os.environ.get('CANBAN_ATTEMPT'))
    note=sub.add_parser('note');notes=note.add_subparsers(dest='op',required=True)
    p=notes.add_parser('list');p.add_argument('--project-id')
    p=notes.add_parser('read');p.add_argument('note_id');p.add_argument('--json',action='store_true')
    p=notes.add_parser('create');p.add_argument('--project-id',required=True);p.add_argument('--title',required=True);p.add_argument('--file')
    p=notes.add_parser('write');p.add_argument('note_id');p.add_argument('--revision',type=int,required=True);p.add_argument('--file')
    p=notes.add_parser('rename');p.add_argument('note_id');p.add_argument('title');p.add_argument('--revision',type=int,required=True)
    p=notes.add_parser('history');p.add_argument('note_id')
    p=notes.add_parser('revision');p.add_argument('note_id');p.add_argument('revision',type=int)
    a=parser.parse_args();raw=False;p={}
    if a.cmd=='call':m=a.method;p=json.loads(a.params)
    elif a.cmd=='snapshot':m='app.snapshot'
    elif a.cmd=='status':m='herdr.status'
    elif a.cmd=='report':
        m='execution.report';p={'state':a.state,'message':a.message,'execution':{'request_id':a.request_id,'attempt_number':a.attempt},'request_key':a.request_key or ('%s:%s:%s'%(a.state,a.request_id,a.attempt) if a.state in ['completed','backlog'] else uuid.uuid4().hex)}
        if a.file:p['body']=body(a.file)
    elif a.cmd=='card':
        if a.card_op=='update':
            assert a.title is not None or a.file is not None, 'Supply --title and/or --file to update the Card'
            m='card.update';p={'execution':{'request_id':a.request_id,'attempt_number':a.attempt}}
            if a.request_key:p['request_key']=a.request_key
            if a.title is not None:p['title']=a.title
            if a.file is not None:p['body']=body(a.file)
        else:
            m='card.adopt';p={'stage':a.stage,'execution':{'request_id':a.request_id,'attempt_number':a.attempt},'request_key':a.request_key or ('adopt:%s:%s:%s'%(a.stage,a.request_id,a.attempt))}
    elif a.cmd=='bind-session':m='agent.bind';p={'session_id':a.session_id,'request_id':a.request_id,'attempt':a.attempt}
    elif a.cmd=='reply':m='agent.reply';p={'question_id':a.to,'request_id':a.request_id,'attempt':a.attempt,'body':body(a.file),'request_key':'reply:'+a.to}
    elif a.cmd=='note':
        m='note.'+{'create':'save','read':'read'}.get(a.op,a.op)
        p={k:v for k,v in vars(a).items() if k in ['note_id','project_id','title','revision'] and v is not None}
        if a.op in ['create','write']:p['body']=body(a.file)
        raw=a.op=='read' and not a.json
    if m=='attachment.add' and 'source' in p:p['bytes']=list(pathlib.Path(p.pop('source')).read_bytes())
    p.setdefault('request_key',uuid.uuid4().hex)
    if m in ['note.save','note.write','note.rename','card.continue','card.update','card.adopt','card.move','execution.retry','review.merge','card.archive'] and 'execution' not in p and os.environ.get('CANBAN_REQUEST_ID'):
        p['execution']={'request_id':os.environ['CANBAN_REQUEST_ID'],'attempt_number':int(os.environ['CANBAN_ATTEMPT'])}
    assert a.endpoint and a.endpoint.startswith('spool:'), 'Canban reporting endpoint unavailable'
    root=pathlib.Path(a.endpoint[6:]);root.mkdir(parents=True,exist_ok=True,mode=0o700)
    key=uuid.uuid4().hex;req=root/(key+'.req');res=root/(key+'.res')
    payload=json.dumps({'method':m,'params':p}).encode()
    assert len(payload)<=4*1024*1024,'Remote CLI request exceeds 4 MiB; split the content'
    tmp=root/(key+'.tmp');tmp.write_bytes(payload);tmp.chmod(0o600);tmp.replace(req)
    deadline=time.monotonic()+180
    while not res.exists():
        if time.monotonic()>deadline:raise RuntimeError('AppServer reply unavailable; effect may be recorded. Inspect before retrying; request key: '+p['request_key'])
        time.sleep(.2)
    value=json.loads(res.read_text());req.unlink(missing_ok=True);res.unlink(missing_ok=True)
    if 'error' in value:raise RuntimeError(value['error'])
    result=value['result']
    print(result['body'] if raw else json.dumps(result,indent=2),end='' if raw else '\n')
try:main()
except Exception as e:
    print('canban: '+str(e),file=sys.stderr);sys.exit(1)
