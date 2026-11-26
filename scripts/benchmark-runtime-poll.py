#!/usr/bin/env python3
"""Compare current polling RPC shape with a shared-inventory prototype.
Uses a disposable Herdr Core and synthetic sleeping PTYs; never launches an Agent.
Not an end-to-end AppServer benchmark. See docs/runtime-observation.md.
"""
from pathlib import Path
import json,os,subprocess,tempfile,time,statistics,resource,select
base=Path(tempfile.mkdtemp(prefix='cb-poll-cost-',dir='/private/tmp'))
core={'HC_SESSION':'poll-cost','HC_SOCKET':str(base/'hc.sock'),'XDG_STATE_HOME':str(base/'state'),'XDG_CONFIG_HOME':str(base/'config')}
env={**os.environ,**core};seq=0
class Rpc:
 def __init__(self):
  self.p=subprocess.Popen(['hc','connect'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
  self.call('hello',{'generation':1})
 def call(self,method,params):
  global seq;seq+=1;n=seq;self.p.stdin.write((json.dumps({'jsonrpc':'2.0','id':n,'method':method,'params':params})+'\n').encode());self.p.stdin.flush()
  deadline=time.monotonic()+15
  while time.monotonic()<deadline:
   ready,_,_=select.select([self.p.stdout],[],[],max(0,deadline-time.monotonic()))
   if not ready:break
   line=self.p.stdout.readline()
   if not line:raise RuntimeError('Core disconnected')
   v=json.loads(line)
   if v.get('id')==n:
    if 'error' in v:raise RuntimeError(str(v['error']))
    return v.get('result'),len(line)
  raise RuntimeError('RPC timed out: '+method)
 def close(self):
  self.p.kill();self.p.wait(timeout=5)
def one(method,p):
 r=Rpc()
 try:return r.call(method,p)
 finally:r.close()
def cpu():
 r=resource.getrusage(resource.RUSAGE_CHILDREN);return r.ru_utime+r.ru_stime+time.process_time()
def sample(n,batched):
 elapsed=[];sizes=[];cores=[];cpu_start=cpu()
 for _ in range(20):
  start=time.monotonic();amount=0
  r=Rpc() if batched else None
  try:
   def call(m,p):
    nonlocal amount
    result,size=r.call(m,p) if r else one(m,p);amount+=size;return result
   if batched:
    call('server.status',{});call('terminal.list',{'all':True});call('server.status',{})
   else:
    call('server.status',{});call('terminal.list',{'all':True});call('server.status',{})
    if n:
     call('server.status',{})
     for address in addresses[:n]:call('terminal.get',{'terminal':address})
  finally:
   if r:r.close()
  elapsed.append((time.monotonic()-start)*1000);sizes.append(amount)
 used=(cpu()-cpu_start)*1000/20
 return {'median_ms':round(statistics.median(elapsed),2),'p95_ms':round(sorted(elapsed)[18],2),'client_cpu_ms_per_round':round(used,2),'reply_bytes_per_round':round(statistics.mean(sizes)),'hc_processes_per_round':1 if batched else 3+(1+n if n else 0),'samples':20}
addresses=[];results=[]
try:
 status,_=one('server.status',{});print('CORE_READY '+str(base),flush=True)
 for target in [0,8,32]:
  while len(addresses)<target:
   v,_=one('terminal.run',{'argv':['/usr/bin/python3','-c','import time; time.sleep(180)','x'*16384],'cwd':str(base),'env':{},'no_parent':True,'no_notify':True,'outlive_parent':True,'wait':False})
   addresses.append(v['address'])
  listing,size=one('terminal.list',{'all':True})
  rows=listing.get('terminals',[])
  results.append({'live_terminals':target,'terminal_list_bytes':size,'terminal_fields':list(rows[0]) if rows else [],'current':sample(target,False),'single_connection_batch':sample(target,True)})
  print(json.dumps(results[-1]),flush=True)
 for address in addresses:one('terminal.kill',{'terminal':address})
 time.sleep(.5)
 results.append({'live_terminals':0,'finished_history':len(addresses),'current':sample(0,False),'single_connection_batch':sample(0,True)})
 print(json.dumps(results[-1]),flush=True)
 (base/'results.json').write_text(json.dumps({'core_env':core,'results':results,'notes':'Synthetic local PTYs, 16 KiB argv per terminal; no model calls. Client CPU includes benchmark and hc children, excludes Core/holders/UI. Batch does not yet implement AppServer behavior.'},indent=2))
 print('BENCHMARK_RESULT='+str(base/'results.json'),flush=True)
finally:
 subprocess.run(['hc','server','stop'],env=env,capture_output=True,timeout=15)
