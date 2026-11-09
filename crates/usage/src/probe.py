# Subscription readers adapted from Herdr a6db99b4 (Apache-2.0) and its
# CodexBar provider contracts (MIT). Credentials never leave this process/host.
import os,sys,json,subprocess,urllib.request,urllib.error,datetime,hashlib,unicodedata,base64
from pathlib import Path
LIMIT=2*1024*1024

def file(path):
    try:
        with open(path,'rb') as f:
            b=f.read(LIMIT+1)
        return json.loads(b) if len(b)<=LIMIT else None
    except (OSError,ValueError):return None

def command(argv,timeout=5):
    try:return subprocess.run(argv,capture_output=True,timeout=timeout)
    except (OSError,subprocess.TimeoutExpired):return None

def fetch(url,token,headers):
    req=urllib.request.Request(url,headers={'Authorization':'Bearer '+token,'Accept':'application/json',**headers})
    try:
        with urllib.request.urlopen(req,timeout=12) as r:
            b=r.read(LIMIT+1)
        if len(b)>LIMIT:raise ValueError()
        return json.loads(b),None
    except urllib.error.HTTPError as e:return None,'HTTP '+str(e.code)
    except Exception:return None,'Usage service unavailable'

def window(name,percent,reset):
    if not isinstance(percent,(int,float)) or isinstance(percent,bool) or not 0<=percent<=100:return None
    if isinstance(reset,(int,float)):
        try:reset=datetime.datetime.fromtimestamp(reset,datetime.timezone.utc).isoformat()
        except (ValueError,OverflowError,OSError):reset=None
    return {'name':name,'used':percent,'resets_at':reset if isinstance(reset,str) else None}

def row(provider,auth='none',account='Current account',windows=None,error=None):
    return {'provider':provider,'account':account,'auth':auth,'windows':[w for w in (windows or []) if w], 'error':error}

def codex():
    auth=file(Path(os.environ.get('CODEX_HOME',str(Path.home()/'.codex')))/'auth.json') or {}
    tokens=auth.get('tokens') or {};token=tokens.get('access_token');account=tokens.get('account_id')
    if not token:
        a=(file(Path(os.environ.get('XDG_DATA_HOME',str(Path.home()/'.local/share')))/'opencode/auth.json') or {}).get('openai') or {}
        if a.get('type')=='oauth':token=a.get('access');account=a.get('accountId')
    if not isinstance(token,str) or not token:return row('codex')
    email='Current account'
    try:
        part=tokens['id_token'].split('.')[1];claims=json.loads(base64.urlsafe_b64decode(part+'='*((-len(part))%4)));email=claims.get('email') or email
    except (KeyError,ValueError,IndexError):pass
    headers={'User-Agent':'codex-cli','OpenAI-Beta':'codex-1','originator':'Codex Desktop'}
    if account:headers['ChatGPT-Account-Id']=str(account)
    data,error=fetch('https://chatgpt.com/backend-api/wham/usage',token,headers)
    windows=[]
    if data:
        limits=data.get('rate_limit') or {}
        for key,name in [('primary_window','Session'),('secondary_window','Weekly')]:
            v=limits.get(key) or {}
            length=v.get('limit_window_seconds')
            if isinstance(length,(int,float)):
                if abs(length-18000)<=60:name='Session'
                elif abs(length-604800)<=60:name='Weekly'
            windows.append(window(name,v.get('used_percent'),v.get('reset_at')))
        review=(data.get('code_review_rate_limit') or {}).get('primary_window') or {}
        windows.append(window('Code review',review.get('used_percent'),review.get('reset_at')))
    return row('codex','present',email,windows,error)

def claude():
    directory=Path(os.environ.get('CLAUDE_CONFIG_DIR',str(Path.home()/'.claude')))
    settings=file(directory/'settings.json') or {}
    endpoint=os.environ.get('ANTHROPIC_BASE_URL') or (settings.get('env') or {}).get('ANTHROPIC_BASE_URL')
    if settings.get('apiKeyHelper') or (endpoint and endpoint.rstrip('/')!='https://api.anthropic.com' and not endpoint.startswith('https://api.anthropic.com/')):return row('claude')
    credentials=None;denied=False
    if sys.platform=='darwin':
        service='Claude Code-credentials'
        if os.environ.get('CLAUDE_CONFIG_DIR'):
            service+='-'+hashlib.sha256(unicodedata.normalize('NFC',str(directory)).encode()).hexdigest()[:8]
        for account in [os.environ.get('USER'),None]:
            args=['/usr/bin/security','find-generic-password','-s',service,'-w']
            if account:args+=['-a',account]
            result=command(args)
            if result and result.returncode==0:
                try:
                    candidate=json.loads(result.stdout)
                    if (candidate.get('claudeAiOauth') or {}).get('accessToken'):credentials=candidate;break
                except (ValueError,AttributeError):pass
            elif result is None or result.returncode not in (44,):denied=True
        # No application-owned token copy or refresh is performed.
    credentials=credentials or file(directory/'.credentials.json') or {}
    token=(credentials.get('claudeAiOauth') or {}).get('accessToken')
    metadata=file(directory/'.claude.json') or file(Path.home()/'.claude.json') or {}
    email=(metadata.get('oauthAccount') or {}).get('emailAddress') or 'Current account'
    if not token:return row('claude','unavailable' if denied else 'none',error='Credential access unavailable' if denied else None)
    data,error=fetch('https://api.anthropic.com/api/oauth/usage',token,{'anthropic-beta':'oauth-2025-04-20','User-Agent':'claude-code/2.1.0'})
    windows=[]
    if data:
        for limit in data.get('limits') or []:
            kind=limit.get('kind');name={'session':'Session','weekly_all':'Weekly'}.get(kind)
            if kind=='weekly_scoped':name=(((limit.get('scope') or {}).get('model') or {}).get('display_name'))
            if name:windows.append(window(name,limit.get('percent'),limit.get('resets_at')))
        if not any(windows):
            for key,name in [('five_hour','Session'),('seven_day','Weekly'),('seven_day_opus','Opus'),('seven_day_sonnet','Sonnet')]:
                v=data.get(key) or {};windows.append(window(name,v.get('utilization'),v.get('resets_at')))
    return row('claude','present',email,windows,error)

def cswap():
    result=command(['cswap','list','--json'],15)
    if not result or result.returncode:return []
    try:
        data=json.loads(result.stdout)
        if data.get('schema_version')!=1:return []
        rows=[]
        for account in data.get('accounts',[])[:20]:
            usage=account.get('usage') or account.get('lastGoodUsage') or {}
            windows=[]
            for key,name in [('fiveHour','Session'),('sevenDay','Weekly')]:
                v=usage.get(key) or {};windows.append(window(name,v.get('pct'),v.get('resetsAt')))
            for scoped in usage.get('scoped',[]):
                if scoped.get('name'):windows.append(window(scoped['name'],scoped.get('pct'),scoped.get('resetsAt')))
            if any(windows):
                r=row('claude','present',account.get('email') or account.get('alias') or 'cswap account',windows)
                r['stale']=not bool(account.get('usage'));rows.append(r)
        return rows
    except (ValueError,AttributeError,TypeError):return []

if __name__=='__main__':
    rows=[]
    for provider,read in [('codex',codex),('claude',claude)]:
        try:rows.append(read())
        except Exception:rows.append(row(provider,'unavailable',error='Credential or usage response unavailable'))
    extra=cswap()
    for r in extra:
        if not any(x['provider']==r['provider'] and x['auth']=='present' and x['account']==r['account'] for x in rows):rows.append(r)
    print(json.dumps(rows))
