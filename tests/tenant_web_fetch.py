#!/usr/bin/env python3
"""Personal-Key web tasks against owned HTTP/model processes; no supplier credentials."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import uuid
from tenant_http_fixture import (binary, env, keys, lock, posts, gates, processes,
    tool_plans, wait, port, sql, request, model, stop, launch)

reads = []
class Source(BaseHTTPRequestHandler):
    def log_message(self, *_args): pass
    def do_GET(self):
        assert self.headers.get('Cookie') is None and self.headers.get('Authorization') is None
        with lock: reads.append(self.path)
        who = self.path.split('/')[1]
        if self.path.endswith('/start') or self.path.endswith('/blocked'):
            self.send_response(302)
            self.send_header('Location', '/'+who+('/final' if self.path.endswith('/start') else '/private'))
            self.send_header('Content-Length','0');self.end_headers();return
        body = ('PUBLIC '+who+' 尾部').encode()
        self.send_response(200);self.send_header('Content-Type','text/plain; charset=utf-8')
        self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
source = ThreadingHTTPServer(('127.0.0.1',0),Source)
threading.Thread(target=source.serve_forever,daemon=True).start()
source_base='http://127.0.0.1:'+str(source.server_port)
def call(url, **extra):
    return {'id':'fetch-'+uuid.uuid4().hex,'type':'function','function':{'name':'web_fetch','arguments':json.dumps(dict(url=url,**extra))}}
def cli(config,*args):
    r=subprocess.run([str(binary),'gateway',*args,'--config',str(config)],env=env,capture_output=True,text=True,timeout=15)
    assert r.returncode==0,(r.stdout,r.stderr)
    return json.loads(r.stdout)
try:
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-web-fetch-') as temporary:
    root=Path(temporary);gp=port();cfg=root/'gateway.json';registry=root/'registry/users.sqlite3'
    gateway=dict(bind=f'127.0.0.1:{gp}',registry_path=str(registry),request_timeout_seconds=15,max_in_flight=4,tracked_turns=True,backends=[])
    backends={}
    for who in ('alice','bob'):
        d=root/who;d.mkdir(mode=0o700);ws=d/'workspace';ws.mkdir(mode=0o700)
        urls=sorted(source_base+'/'+who+'/'+p for p in ('start','final','blocked'))
        policy=dict(urls=urls,allow_private=True);token='web-fetch-fixture-'+uuid.uuid4().hex
        token_file=d/'token';token_file.write_text(token);token_file.chmod(0o600);number=port()
        config={'agent':{'name':who,'description':'Explicit webpage task','workspace_path':str(ws),'system_instructions':'Use only selected tools.','max_turns':10,'tool_timeout_secs':15},
            'tools':{'web_fetch':{'enabled':True,'allow_private':True}},
            'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{model.server_port}','api_key':keys[who],'model':'fixture-web-fetch'},
            'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
            'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3','api_token':token,'tracked_turns':True,
                'tracked_turn_timeout_secs':20,'shutdown_timeout_secs':2,'gateway_channel_chat':True,'tenant_web_fetch':policy}}
        path=d/'config.json';path.write_text(json.dumps(config));b=dict(path=path,config=config,port=number,token=token,policy=policy,log=d/'serve.log');backends[who]=b
        b['process']=launch(['serve','--config',str(path)],number,b['log'])
        gateway['backends'].append(dict(id=who,url=f'http://127.0.0.1:{number}/',token_file=str(token_file),tenant_web_fetch=policy))
    cfg.write_text(json.dumps(gateway));users={w:cli(cfg,'user-add','--backend',w) for w in backends}
    readonly=cli(cfg,'key-add','--user',users['alice']['user_id'],'--read-only')
    process=launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    def api(who,path,method='GET',body=None,expected=200,native=False,token=None):
        b=backends[who];code,value,headers=request(b['port'] if native else gp,path,b['token'] if native else (token or users[who]['token']),method,body,marker=native)
        assert code==expected,(who,path,code,value)
        if not native: assert headers.get('Cache-Control')=='no-store' and headers.get('X-Content-Type-Options')=='nosniff'
        return value
    caps={w:api(w,'/api/turns/capabilities') for w in backends}
    def body(who,prompt,tools=None):
        tools=tools or ['web_fetch'];v=dict(session_id='http:'+str(uuid.uuid4()),prompt=prompt,enabled_tools=tools,enabled_skills=[])
        if 'web_fetch' in tools:v['web_fetch_policy_sha256']=caps[who]['web_fetch']['policy_sha256']
        return v
    def count(prompt):
        with lock:return len([p for p in posts if p['prompt']==prompt])
    def complete(who,identity,native=False):
        return wait(lambda:(lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(api(who,'/api/turns/'+identity,native=native)),'original web_fetch receipt')
    def settled(who):wait(lambda:not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users[who]['user_id'],)),'original gateway hold')
    def submit(who,prompt,calls,native=False):
        tool_plans[(who,prompt)]=calls;identity=str(uuid.uuid4());original=body(who,prompt)
        api(who,'/api/turns/'+identity,'PUT',original,202,native=native)
        result=complete(who,identity,native)
        if not native and result['receipt']['state']=='completed':settled(who)
        return identity,original,result
    actual=[];identities={}
    for who in backends:
        policy=backends[who]['policy'];canonical=[1,policy['urls'],True,1048577,1048576,15,5,50000]
        digest=hashlib.sha256(json.dumps(canonical,separators=(',',':')).encode()).hexdigest()
        assert caps[who]['web_fetch']==dict(protocol=1,urls=policy['urls'],allow_private=True,max_capture_bytes=1048577,max_body_bytes=1048576,timeout_seconds=15,max_redirects=5,max_chars=50000,policy_sha256=digest)
        assert caps[who]['enabled_tools']==['datetime_now','json_query','web_fetch']
    stop(process);wrong=json.loads(json.dumps(gateway));wrong['backends'][0]['tenant_web_fetch']['allow_private']=False
    bad=root/'wrong.json';bad.write_text(json.dumps(wrong))
    with (root/'wrong.log').open('wb') as log:
        p=subprocess.Popen([str(binary),'gateway','serve','--config',str(bad)],env=env,stdout=log,stderr=log);processes.append(p)
    assert p.wait(timeout=15)==1
    process=launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    print('PASS tenant web_fetch 1: paired exact destinations and canonical original HTTP budgets; mismatched startup refuses',flush=True)
    for who in backends:
        url=source_base+'/'+who+'/start';identity,original,v=submit(who,'fetch-success-'+who,[call(url)])
        assert v['receipt']['state']=='completed' and v['receipt']['result']['tool_names']==['web_fetch'],v
        reply=v['receipt']['result']['reply'];assert 'PUBLIC '+who+' 尾部' in reply and source_base+'/'+who+'/final' in reply,reply
        assert count('fetch-success-'+who)==2
        with lock:
            initial=next(p for p in posts if p['prompt']=='fetch-success-'+who)['body']
            assert reads==sum(([f'/{w}/start',f'/{w}/final'] for w in ('alice','bob')[:len(actual)+1]),[]),reads
        schema=initial['tools'][0]['function']['parameters'];assert schema['additionalProperties'] is False and schema['properties']['url']['enum']==backends[who]['policy']['urls']
        identities[who]=(identity,original,v)
        actual.append(dict(user=who,turn_id=identity,policy_sha256=caps[who]['web_fetch']['policy_sha256'],model_posts=2,http_gets=2,tool_names=['web_fetch'],final_url=source_base+'/'+who+'/final',reply_sha256=hashlib.sha256(reply.encode()).hexdigest()))
    api('bob','/api/turns/'+identities['alice'][0],expected=404);api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    print('PASS tenant web_fetch 2: actual two-user model/tool/receipt roundtrip with authorized manual redirect and complete final URL',flush=True)
    before=(len(posts),len(reads))
    for v in [dict(body('alice','missing'),web_fetch_policy_sha256=None),dict(body('alice','stale'),web_fetch_policy_sha256='a'*64),dict(body('alice','duplicate'),enabled_tools=['web_fetch','web_fetch']),dict(body('alice','extra',['datetime_now']),web_fetch_policy_sha256='a'*64)]:
        for native in [False,True]:
            identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400,native=native);api('alice','/api/turns/'+identity,expected=404,native=native)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token'])
    assert (len(posts),len(reads))==before
    for url,extra in [(source_base+'/bob/final',{}),(source_base+'/alice/private',{}),(' '+source_base+'/alice/final',{}),(source_base+'/alice/final',{'headers':{}}),(source_base+'/alice/final',{'max_chars':499})]:
        prompt='fetch-invalid-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(url,**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1 and len(reads)==before[1],v
    print('PASS tenant web_fetch 3: stale/unselected/duplicate/readonly and exact tool schema reject before HTTP dispatch',flush=True)
    before=len(reads);_,_,v=submit('alice','fetch-denied-redirect',[call(source_base+'/alice/blocked')],native=True)
    assert v['receipt']['state']=='needs_review' and count('fetch-denied-redirect')==1 and len(reads)==before+1 and reads[-1]=='/alice/blocked',v
    assert '/alice/private' not in reads
    print('PASS tenant web_fetch 4: unauthorized redirect target receives zero requests; original Unknown effects and needs_review retained',flush=True)
    a=backends['alice'];stop(a['process']);changed=json.loads(json.dumps(a['config']));changed['http']['tenant_web_fetch']['allow_private']=False;a['path'].write_text(json.dumps(changed))
    a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log']);before=(len(posts),len(reads));identity=str(uuid.uuid4())
    api('alice','/api/turns/'+identity,'PUT',body('alice','changed-backend'),503);api('alice','/api/turns/'+identity,expected=404)
    assert (len(posts),len(reads))==before and not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users['alice']['user_id'],))
    for _ in range(2):api('alice','/api/turns/'+identities['alice'][0])
    api('alice','/api/turns/'+identities['alice'][0],'PUT',identities['alice'][1]);assert (len(posts),len(reads))==before
    stop(a['process']);a['path'].write_text(json.dumps(a['config']));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob','fetch-cancel')]=gate;tool_plans[('bob','fetch-cancel')]=[call(source_base+'/bob/final')]
    identity=str(uuid.uuid4());before=len(reads);api('bob','/api/turns/'+identity,'PUT',body('bob','fetch-cancel'),202,native=True);assert gate['ready'].wait(5)
    api('bob','/api/turns/'+identity+'/cancel','POST',{},200,native=True);gate['release'].set();wait(lambda:not api('bob','/api/turns/'+identity,native=True)['active'],'cancelled owner settles')
    assert count('fetch-cancel')==1 and len(reads)==before
    print('PASS tenant web_fetch 5: changed permission rejects before identity/hold; old UUID facts do not replay; cancellation blocks future HTTP',flush=True)
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-web-fetch')]=[call(source_base+'/alice/start')]
        browser=root/'browser.json';browser.write_text(json.dumps(dict(base=f'http://127.0.0.1:{gp}',alice=users['alice']['token'],bob=users['bob']['token'],readonly=readonly['token'])))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_web_fetch_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90);assert r.returncode==0,r.returncode
        assert count('browser-web-fetch')==2
    cli(cfg,'key-revoke','--key',users['alice']['key_id']);api('alice','/api/turns/'+identities['alice'][0],expected=401)
    print('PASS tenant web_fetch 6: revoked Key cannot recover private results; no automatic HTTP or model request',flush=True)
    print('ACTUAL tenant web_fetch task receipts '+json.dumps(actual,separators=(',',':')),flush=True)
    print('ALL tenant web_fetch acceptance passed',flush=True)
finally:
 for p in processes:
    if p.poll() is None:stop(p)
 source.shutdown();model.shutdown()
