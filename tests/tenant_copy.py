#!/usr/bin/env python3
"""Exact create-only personal-Key copies through original IO/turn owners; owned fixtures only."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from tenant_http_fixture import (binary, env, keys, lock, posts, gates, processes,
    tool_plans, wait, port, sql, request, model, stop, launch)

sha=lambda b:hashlib.sha256(b).hexdigest()
def call(pair,**extra):
    return {'id':'copy-'+uuid.uuid4().hex,'type':'function','function':{'name':'copy','arguments':json.dumps(dict(pair,**extra))}}
def cli(config,*args):
    r=subprocess.run([str(binary),'gateway',*args,'--config',str(config)],env=env,capture_output=True,text=True,timeout=15)
    assert r.returncode==0,(r.stdout,r.stderr)
    return json.loads(r.stdout)
try:
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-copy-') as temporary:
    root=Path(temporary);gp=port();cfg=root/'gateway.json';registry=root/'registry/users.sqlite3'
    gateway=dict(bind=f'127.0.0.1:{gp}',registry_path=str(registry),request_timeout_seconds=15,max_in_flight=4,tracked_turns=True,backends=[])
    backends={}
    for who in ('alice','bob'):
        d=root/who;d.mkdir(mode=0o700);ws=d/'workspace';ws.mkdir(mode=0o700)
        docs=ws/'tenant-documents'/who;docs.mkdir(parents=True)
        policy={'pairs':[dict(from_=f'tenant-documents/{who}/a.bin',to=f'tenant-documents/{who}/b.bin'),dict(from_=f'tenant-documents/{who}/c.bin',to=f'tenant-documents/{who}/d.bin')]}
        for pair in policy['pairs']:pair['from']=pair.pop('from_')
        raw=b'\0\xff\xfeBINARY-'+who.encode()+b'\r\n'+bytes(range(256))*4096
        (ws/policy['pairs'][0]['from']).write_bytes(raw);(ws/policy['pairs'][1]['from']).write_bytes(b'other')
        token='copy-fixture-'+uuid.uuid4().hex;token_file=d/'token';token_file.write_text(token);token_file.chmod(0o600);number=port()
        config={'agent':{'name':who,'description':'Explicit binary copy task','workspace_path':str(ws),'system_instructions':'Use only selected tools.','max_turns':10,'tool_timeout_secs':15},
            'tools':{'copy':{'enabled':True}},
            'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{model.server_port}','api_key':keys[who],'model':'fixture-copy'},
            'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
            'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3','api_token':token,'tracked_turns':True,
                'tracked_turn_timeout_secs':20,'shutdown_timeout_secs':2,'gateway_channel_chat':True,'tenant_copy':policy}}
        path=d/'config.json';path.write_text(json.dumps(config));b=dict(path=path,config=config,port=number,token=token,policy=policy,workspace=ws,raw=raw,log=d/'serve.log');backends[who]=b
        b['process']=launch(['serve','--config',str(path)],number,b['log'])
        gateway['backends'].append(dict(id=who,url=f'http://127.0.0.1:{number}/',token_file=str(token_file),tenant_copy=policy))
    cfg.write_text(json.dumps(gateway));users={w:cli(cfg,'user-add','--backend',w) for w in backends};readonly=cli(cfg,'key-add','--user',users['alice']['user_id'],'--read-only')
    process=launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    def api(who,path,method='GET',body=None,expected=200,native=False,token=None):
        b=backends[who];code,value,headers=request(b['port'] if native else gp,path,b['token'] if native else (token or users[who]['token']),method,body,marker=native)
        assert code==expected,(who,path,code,value)
        if not native:assert headers.get('Cache-Control')=='no-store' and headers.get('X-Content-Type-Options')=='nosniff'
        return value
    caps={w:api(w,'/api/turns/capabilities') for w in backends}
    def body(who,prompt,tools=None):
        tools=tools or ['copy'];v=dict(session_id='http:'+str(uuid.uuid4()),prompt=prompt,enabled_tools=tools,enabled_skills=[])
        if 'copy' in tools:v['copy_policy_sha256']=caps[who]['copy']['policy_sha256']
        return v
    def count(prompt):
        with lock:return len([p for p in posts if p['prompt']==prompt])
    def complete(who,identity,native=False):
        return wait(lambda:(lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(api(who,'/api/turns/'+identity,native=native)),'original copy receipt')
    def settled(who):wait(lambda:not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users[who]['user_id'],)),'original copy hold')
    def submit(who,prompt,calls,native=False):
        tool_plans[(who,prompt)]=calls;identity=str(uuid.uuid4());original=body(who,prompt)
        api(who,'/api/turns/'+identity,'PUT',original,202,native=native);v=complete(who,identity,native)
        if not native and v['receipt']['state']=='completed':settled(who)
        return identity,original,v
    def observe_hold(who,identity,deadline):
        def observed():
            assert time.monotonic()<deadline, 'original 15s copy hold deadline exceeded'
            return sql(registry,'SELECT request_id,state FROM write_holds WHERE user_id=?',(users[who]['user_id'],))==[{'request_id':identity,'state':'needs_review'}]
        wait(observed,'original copy unknown hold',seconds=max(0,deadline-time.monotonic()))
        assert time.monotonic()<deadline
    def review(who,identity,native=True):
        api(who,'/api/turns/'+identity+'/review','POST',{'decision':'abandon','note':'Disposable fixture effects and original model receipt inspected.'},native=True)
        if not native:cli(cfg,'review-clear','--user',users[who]['user_id'],'--confirm-backend-idle','--note','Disposable fixture model and backend idle; copy effects inspected.')
    def unknown(who,prompt,pair,attempted=True,**extra):
        identity,_,v=submit(who,prompt,[call(pair,**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1,v
        if attempted:assert v['receipt']['result']['status']=='requireshumaninput' and v['receipt']['result']['tool_names']==['copy'],v
        else:assert v['receipt']['result'] is None,v
        review(who,identity);return identity
    actual=[];identities={}
    for who,b in backends.items():
        canonical=[1,[[p['from'],p['to']] for p in b['policy']['pairs']],'create_only',67108864]
        assert caps[who]['copy']==dict(protocol=1,pairs=b['policy']['pairs'],mode='create_only',max_bytes=67108864,policy_sha256=sha(json.dumps(canonical,separators=(',',':')).encode()))
        assert caps[who]['enabled_tools']==['datetime_now','json_query','copy']
    stop(process);wrong=json.loads(json.dumps(gateway));wrong['backends'][0]['tenant_copy']['pairs'][0]['to']='tenant-documents/alice/other.bin'
    bad=root/'wrong.json';bad.write_text(json.dumps(wrong));r=subprocess.run([str(binary),'gateway','serve','--config',str(bad)],env=env,capture_output=True,timeout=15)
    assert r.returncode==1 and b'permission contract mismatch' in r.stderr,r.stderr
    process=launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    print('PASS tenant copy 1: exact paired default-off capability and canonical original binary budget; mismatched startup refuses',flush=True)
    for who,b in backends.items():
        pair=b['policy']['pairs'][0];identity,original,v=submit(who,'copy-success-'+who,[call(pair)])
        assert v['receipt']['state']=='completed' and v['receipt']['result']['tool_names']==['copy'] and count('copy-success-'+who)==2,v
        assert (b['workspace']/pair['from']).read_bytes()==b['raw'] and (b['workspace']/pair['to']).read_bytes()==b['raw']
        with lock:first=next(p for p in posts if p['prompt']=='copy-success-'+who)['body']
        assert [t['function']['name'] for t in first['tools']]==['copy']
        schema=first['tools'][0]['function']['parameters'];assert schema['required']==['from','to'] and schema['additionalProperties'] is False and len(schema['oneOf'])==2
        assert 'BINARY' not in json.dumps(posts[-1]['body']) and '\\u00ff' not in v['receipt']['result']['reply']
        identities[who]=(identity,original,v);before=len(posts);assert api(who,'/api/turns/'+identity,'PUT',original)['receipt']==v['receipt'] and len(posts)==before
        actual.append(dict(user=who,turn_id=identity,policy_sha256=caps[who]['copy']['policy_sha256'],source=pair['from'],destination=pair['to'],file_bytes=len(b['raw']),file_sha256=sha(b['raw']),source_preserved=True,overwritten=False,model_posts=2,tool_names=['copy']))
    api('bob','/api/turns/'+identities['alice'][0],expected=404);api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    print('PASS tenant copy 2: actual isolated two-user complete binary copies preserve source and original UUID facts with zero replay',flush=True)
    before=len(posts)
    for v in [dict(body('alice','missing'),copy_policy_sha256=None),dict(body('alice','stale'),copy_policy_sha256='a'*64),dict(body('alice','duplicate'),enabled_tools=['copy','copy']),dict(body('alice','extra',['datetime_now']),copy_policy_sha256='a'*64)]:
        for native in [False,True]:
            identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400,native=native);api('alice','/api/turns/'+identity,expected=404,native=native)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token']);assert len(posts)==before
    a=backends['alice'];pair=a['policy']['pairs'][0];target=a['workspace']/pair['to'];source=a['workspace']/pair['from']
    for bad in [dict(pair,to=a['policy']['pairs'][1]['to']),dict(pair,**{'from':'tenant-documents/alice/./a.bin'}),{'source':pair['from'],'destination':pair['to']},dict(pair,overwrite=True),backends['bob']['policy']['pairs'][0]]:
        unknown('alice','copy-invalid-'+uuid.uuid4().hex,bad,attempted=False);assert target.read_bytes()==a['raw'] and not (a['workspace']/a['policy']['pairs'][1]['to']).exists()
    unknown('alice','copy-existing',pair);assert target.read_bytes()==a['raw'];target.unlink()
    print('PASS tenant copy 3: stale/unselected/readonly, recombined pairs, aliases and overwrite refuse; existing target preserved',flush=True)
    protected=root/'protected';protected.write_bytes(b'PRIVATE-UNCHANGED')
    for kind in ['symlink','hardlink','fifo','directory','oversize','missing']:
        source.unlink()
        if kind=='symlink':source.symlink_to(protected)
        elif kind=='hardlink':os.link(protected,source)
        elif kind=='fifo':os.mkfifo(source)
        elif kind=='directory':source.mkdir()
        elif kind=='oversize':
            with source.open('wb') as stream:stream.truncate(67108865)
        unknown('alice','copy-unsafe-'+kind,pair);assert not target.exists() and protected.read_bytes()==b'PRIVATE-UNCHANGED'
        if kind=='directory':source.rmdir()
        elif kind!='missing':source.unlink()
        source.write_bytes(a['raw'])
    for kind in ['symlink','hardlink','fifo','directory']:
        if kind=='symlink':target.symlink_to(protected)
        elif kind=='hardlink':os.link(protected,target)
        elif kind=='fifo':os.mkfifo(target)
        else:target.mkdir()
        unknown('alice','copy-target-'+kind,pair);assert protected.read_bytes()==b'PRIVATE-UNCHANGED'
        target.rmdir() if kind=='directory' else target.unlink()
    # Exact 64 MiB accepted by the same streaming copy engine, then verify all bytes.
    with source.open('wb') as stream:stream.truncate(67108864)
    _,_,v=submit('alice','copy-exact-limit',[call(pair)]);assert v['receipt']['state']=='completed' and source.stat().st_size==target.stat().st_size==67108864 and sha(source.read_bytes())==sha(target.read_bytes());target.unlink();source.write_bytes(a['raw'])
    print('PASS tenant copy 4: original symlink/hardlink/FIFO/directory/missing and 64 MiB boundary safeguards preserve protected files',flush=True)
    fd=os.open(a['workspace'],os.O_RDONLY)
    try:
        fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB);started=time.monotonic();identity,_,v=submit('alice','copy-writer-busy',[call(pair)])
        assert v['receipt']['state']=='needs_review' and count('copy-writer-busy')==1 and not target.exists(),v
        observe_hold('alice',identity,started+15)
    finally:fcntl.flock(fd,fcntl.LOCK_UN);os.close(fd)
    api('alice','/api/turns/'+identity);assert not target.exists();review('alice',identity,native=False)
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('alice','copy-cancel')]=gate;tool_plans[('alice','copy-cancel')]=[call(pair)];identity=str(uuid.uuid4())
    api('alice','/api/turns/'+identity,'PUT',body('alice','copy-cancel'),202);assert gate['ready'].wait(5)
    cancel_deadline=time.monotonic()+15
    api('alice','/api/turns/'+identity+'/cancel','POST',{});gate['release'].set();v=complete('alice',identity)
    assert v['receipt']['state']=='needs_review' and v['receipt']['cancel_requested'] and count('copy-cancel')==1 and not target.exists();observe_hold('alice',identity,cancel_deadline);review('alice',identity,native=False)
    stop(a['process']);changed=json.loads(json.dumps(a['config']));changed['http']['tenant_copy']['pairs'][0]['to']='tenant-documents/alice/changed.bin';a['path'].write_text(json.dumps(changed));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    before=len(posts);identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','changed-backend'),503);api('alice','/api/turns/'+identity,expected=404)
    assert len(posts)==before and not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users['alice']['user_id'],))
    for _ in range(2):api('alice','/api/turns/'+identities['alice'][0])
    api('alice','/api/turns/'+identities['alice'][0],'PUT',identities['alice'][1]);assert len(posts)==before
    stop(a['process']);a['path'].write_text(json.dumps(a['config']));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    print('PASS tenant copy 5: original writer and Unknown holds persist until review; cancellation blocks future copy; cap mismatch precedes identity/hold',flush=True)
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-copy')]=[call(pair)]
        browser=root/'browser.json';browser.write_text(json.dumps(dict(base=f'http://127.0.0.1:{gp}',alice=users['alice']['token'],bob=users['bob']['token'],readonly=readonly['token'])))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_copy_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90);assert r.returncode==0,r.returncode
        assert count('browser-copy')==2 and target.read_bytes()==a['raw'] and source.read_bytes()==a['raw']
    cli(cfg,'key-revoke','--key',users['alice']['key_id']);api('alice','/api/turns/'+identities['alice'][0],expected=401)
    print('PASS tenant copy 6: revoked Key refuses original facts; explicit UI copy has no automatic replay or extra source-body tool',flush=True)
    print('ACTUAL tenant copy task receipts '+json.dumps(actual,separators=(',',':')),flush=True)
    print('ALL tenant copy acceptance passed',flush=True)
finally:
 for gate in gates.values():gate['release'].set()
 for p in reversed(processes):
    if p.poll() is None:stop(p)
 model.shutdown()
