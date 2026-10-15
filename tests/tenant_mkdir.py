#!/usr/bin/env python3
"""Exact nonrecursive personal-Key directory creation through original IO/turn owners; owned fixtures only."""
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
def call(args,**extra):
    return {'id':'mkdir-'+uuid.uuid4().hex,'type':'function','function':{'name':'mkdir','arguments':json.dumps(dict(args,**extra))}}
def cli(config,*args):
    r=subprocess.run([str(binary),'gateway',*args,'--config',str(config)],env=env,capture_output=True,text=True,timeout=15)
    assert r.returncode==0,(r.stdout,r.stderr)
    return json.loads(r.stdout)
try:
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-mkdir-') as temporary:
    root=Path(temporary);gp=port();cfg=root/'gateway.json';registry=root/'registry/users.sqlite3'
    gateway=dict(bind=f'127.0.0.1:{gp}',registry_path=str(registry),request_timeout_seconds=15,max_in_flight=4,tracked_turns=True,backends=[])
    backends={}
    for who in ('alice','bob'):
        d=root/who;d.mkdir(mode=0o700);ws=d/'workspace';ws.mkdir(mode=0o700)
        docs=ws/'tenant-documents'/who;docs.mkdir(parents=True)
        policy={'paths':[f'tenant-documents/{who}/deliveries',f'tenant-documents/{who}/missing/child']}
        token='mkdir-fixture-'+uuid.uuid4().hex;token_file=d/'token';token_file.write_text(token);token_file.chmod(0o600);number=port()
        config={'agent':{'name':who,'description':'Explicit directory creation task','workspace_path':str(ws),'system_instructions':'Use only selected tools.','max_turns':10,'tool_timeout_secs':15},
            'tools':{'mkdir':{'enabled':True}},
            'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{model.server_port}','api_key':keys[who],'model':'fixture-mkdir'},
            'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
            'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3','api_token':token,'tracked_turns':True,
                'tracked_turn_timeout_secs':20,'shutdown_timeout_secs':2,'gateway_channel_chat':True,'tenant_mkdir':policy}}
        path=d/'config.json';path.write_text(json.dumps(config));b=dict(path=path,config=config,port=number,token=token,policy=policy,workspace=ws,log=d/'serve.log');backends[who]=b
        b['process']=launch(['serve','--config',str(path)],number,b['log'])
        gateway['backends'].append(dict(id=who,url=f'http://127.0.0.1:{number}/',token_file=str(token_file),tenant_mkdir=policy))
    cfg.write_text(json.dumps(gateway));users={w:cli(cfg,'user-add','--backend',w) for w in backends};readonly=cli(cfg,'key-add','--user',users['alice']['user_id'],'--read-only')
    process=launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    def api(who,path,method='GET',body=None,expected=200,native=False,token=None):
        b=backends[who];code,value,headers=request(b['port'] if native else gp,path,b['token'] if native else (token or users[who]['token']),method,body,marker=native)
        assert code==expected,(who,path,code,value)
        if not native:assert headers.get('Cache-Control')=='no-store' and headers.get('X-Content-Type-Options')=='nosniff'
        return value
    caps={w:api(w,'/api/turns/capabilities') for w in backends}
    def body(who,prompt,tools=None):
        tools=tools or ['mkdir'];v=dict(session_id='http:'+str(uuid.uuid4()),prompt=prompt,enabled_tools=tools,enabled_skills=[])
        if 'mkdir' in tools:v['mkdir_policy_sha256']=caps[who]['mkdir']['policy_sha256']
        return v
    def count(prompt):
        with lock:return len([p for p in posts if p['prompt']==prompt])
    def complete(who,identity,native=False):
        return wait(lambda:(lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(api(who,'/api/turns/'+identity,native=native)),'original mkdir receipt')
    def settled(who):wait(lambda:not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users[who]['user_id'],)),'original mkdir hold')
    def submit(who,prompt,calls,native=False):
        tool_plans[(who,prompt)]=calls;identity=str(uuid.uuid4());original=body(who,prompt)
        api(who,'/api/turns/'+identity,'PUT',original,202,native=native)
        deadline=time.monotonic()+15
        if not native:
            def settled_owner():
                assert time.monotonic()<deadline, 'original 15s mkdir settlement deadline exceeded'
                rows=sql(registry,'SELECT request_id,state FROM write_holds WHERE user_id=?',(users[who]['user_id'],))
                ready=not rows or rows==[{'request_id':identity,'state':'needs_review'}]
                assert time.monotonic()<deadline
                return ready
            wait(settled_owner,'original mkdir owner before client GET',seconds=max(0,deadline-time.monotonic()))
        v=complete(who,identity,native)
        assert time.monotonic()<deadline, 'original 15s mkdir completion deadline exceeded'
        return identity,original,v
    def observe_hold(who,identity,deadline):
        def observed():
            assert time.monotonic()<deadline, 'original 15s mkdir hold deadline exceeded'
            return sql(registry,'SELECT request_id,state FROM write_holds WHERE user_id=?',(users[who]['user_id'],))==[{'request_id':identity,'state':'needs_review'}]
        wait(observed,'original mkdir unknown hold',seconds=max(0,deadline-time.monotonic()))
        assert time.monotonic()<deadline
    def review(who,identity,native=True):
        api(who,'/api/turns/'+identity+'/review','POST',{'decision':'abandon','note':'Disposable fixture effects and original model receipt inspected.'},native=True)
        if not native:cli(cfg,'review-clear','--user',users[who]['user_id'],'--confirm-backend-idle','--note','Disposable fixture model and backend idle; mkdir effects inspected.')
    def unknown(who,prompt,pair,attempted=True,**extra):
        identity,_,v=submit(who,prompt,[call(pair,**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1,v
        if attempted:assert v['receipt']['result']['status']=='requireshumaninput' and v['receipt']['result']['tool_names']==['mkdir'],v
        else:assert v['receipt']['result'] is None,v
        review(who,identity);return identity
    actual=[];identities={}
    for who,b in backends.items():
        canonical=[1,b['policy']['paths'],'single_directory']
        assert caps[who]['mkdir']==dict(protocol=1,paths=b['policy']['paths'],mode='single_directory',policy_sha256=sha(json.dumps(canonical,separators=(',',':')).encode()))
        assert caps[who]['enabled_tools']==['datetime_now','json_query','mkdir']
        assert all(k not in caps[who] for k in ['move','copy','file_write'])
    stop(process);wrong=json.loads(json.dumps(gateway));wrong['backends'][0]['tenant_mkdir']['paths'][0]='tenant-documents/alice/changed'
    bad=root/'wrong.json';bad.write_text(json.dumps(wrong));r=subprocess.run([str(binary),'gateway','serve','--config',str(bad)],env=env,capture_output=True,timeout=15)
    assert r.returncode==1 and b'permission contract mismatch' in r.stderr,r.stderr
    process=launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    print('PASS tenant mkdir 1: exact default-off capability and canonical single-directory mode; mismatched startup refuses',flush=True)
    for who,b in backends.items():
        path=b['policy']['paths'][0];target=b['workspace']/path
        identity,original,v=submit(who,'mkdir-create-'+who,[call({'path':path})])
        assert v['receipt']['state']=='completed' and v['receipt']['result']['tool_names']==['mkdir'] and count('mkdir-create-'+who)==2,v
        assert target.is_dir() and not target.is_symlink() and not list(target.iterdir())
        created_id=identity
        with lock:first=next(p for p in posts if p['prompt']=='mkdir-create-'+who)['body']
        assert [t['function']['name'] for t in first['tools']]==['mkdir']
        schema=first['tools'][0]['function']['parameters'];assert schema=={'type':'object','properties':{'path':{'type':'string','enum':b['policy']['paths']}},'required':['path'],'additionalProperties':False},schema
        assert json.loads(json.loads(v['receipt']['result']['reply'].split(': ',1)[1]))==dict(path=path,created=True,existed=False,recursive=False)
        identities[who]=(identity,original,v);before=len(posts);assert api(who,'/api/turns/'+identity,'PUT',original)['receipt']==v['receipt'] and len(posts)==before
        old_inode=target.stat().st_ino
        existing_id,_,existing=submit(who,'mkdir-existing-'+who,[call({'path':path})])
        assert existing['receipt']['state']=='completed' and count('mkdir-existing-'+who)==2 and target.stat().st_ino==old_inode
        assert json.loads(json.loads(existing['receipt']['result']['reply'].split(': ',1)[1]))==dict(path=path,created=False,existed=True,recursive=False)
        actual.append(dict(user=who,created_turn_id=created_id,existing_turn_id=existing_id,policy_sha256=caps[who]['mkdir']['policy_sha256'],path=path,created=True,existing_inode_preserved=True,recursive=False,model_posts=4,tool_names=['mkdir'],created_result_sha256=sha(v['receipt']['result']['reply'].encode()),existing_result_sha256=sha(existing['receipt']['result']['reply'].encode())))
    api('bob','/api/turns/'+identities['alice'][0],expected=404);api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    print('PASS tenant mkdir 2: actual isolated two-user directory creation and idempotence preserve inode and original UUID facts with zero replay',flush=True)
    before=len(posts)
    for v in [dict(body('alice','missing'),mkdir_policy_sha256=None),dict(body('alice','stale'),mkdir_policy_sha256='a'*64),dict(body('alice','duplicate'),enabled_tools=['mkdir','mkdir']),dict(body('alice','extra',['datetime_now']),mkdir_policy_sha256='a'*64)]:
        for native in [False,True]:
            identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400,native=native);api('alice','/api/turns/'+identity,expected=404,native=native)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token']);assert len(posts)==before
    a=backends['alice'];path=a['policy']['paths'][0];target=a['workspace']/path;target.rmdir()
    for bad in [{'path':'tenant-documents/alice/private'},{'path':'tenant-documents/alice/./deliveries'},{'path':path,'recursive':False},{'path':path,'parents':False},{'path':backends['bob']['policy']['paths'][0]}]:
        unknown('alice','mkdir-invalid-'+uuid.uuid4().hex,bad,attempted=False);assert not target.exists()
    unknown('alice','mkdir-parent-missing',{'path':a['policy']['paths'][1]});assert not (a['workspace']/'tenant-documents/alice/missing').exists()
    print('PASS tenant mkdir 3: stale/unselected/readonly, ungranted paths, aliases and recursive flags refuse; missing parent is not created',flush=True)
    protected=root/'protected';protected.mkdir();(protected/'private').write_bytes(b'PRIVATE-UNCHANGED')
    for kind in ['symlink','file','hardlink','fifo']:
        if kind=='symlink':target.symlink_to(protected,target_is_directory=True)
        elif kind=='file':target.write_bytes(b'original-file')
        elif kind=='hardlink':os.link(protected/'private',target)
        else:os.mkfifo(target)
        unknown('alice','mkdir-unsafe-'+kind,{'path':path});assert (protected/'private').read_bytes()==b'PRIVATE-UNCHANGED'
        if kind=='file':assert target.read_bytes()==b'original-file'
        target.unlink()
    parent=target.parent;renamed=parent.with_name('saved-alice');parent.rename(renamed);parent.symlink_to(protected,target_is_directory=True)
    try:unknown('alice','mkdir-parent-link',{'path':path});assert not (protected/'deliveries').exists()
    finally:parent.unlink();renamed.rename(parent)
    print('PASS tenant mkdir 4: original directory handles reject linked parents and file/symlink/hardlink/FIFO leaves; protected content remains unchanged',flush=True)
    fd=os.open(a['workspace'],os.O_RDONLY)
    try:
        fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB);started=time.monotonic();identity,_,v=submit('alice','mkdir-writer-busy',[call({'path':path})])
        assert v['receipt']['state']=='needs_review' and count('mkdir-writer-busy')==1 and not target.exists(),v
        observe_hold('alice',identity,started+15)
    finally:fcntl.flock(fd,fcntl.LOCK_UN);os.close(fd)
    api('alice','/api/turns/'+identity);assert not target.exists();review('alice',identity,native=False)
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('alice','mkdir-cancel')]=gate;tool_plans[('alice','mkdir-cancel')]=[call({'path':path})];identity=str(uuid.uuid4())
    api('alice','/api/turns/'+identity,'PUT',body('alice','mkdir-cancel'),202);assert gate['ready'].wait(5)
    cancel_deadline=time.monotonic()+15
    api('alice','/api/turns/'+identity+'/cancel','POST',{});gate['release'].set();v=complete('alice',identity)
    assert v['receipt']['state']=='needs_review' and v['receipt']['cancel_requested'] and count('mkdir-cancel')==1 and not target.exists();observe_hold('alice',identity,cancel_deadline);review('alice',identity,native=False)
    stop(a['process']);changed=json.loads(json.dumps(a['config']));changed['http']['tenant_mkdir']['paths'][0]='tenant-documents/alice/changed';a['path'].write_text(json.dumps(changed));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    before=len(posts);identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','changed-backend'),503);api('alice','/api/turns/'+identity,expected=404)
    assert len(posts)==before and not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users['alice']['user_id'],))
    for _ in range(2):api('alice','/api/turns/'+identities['alice'][0])
    api('alice','/api/turns/'+identities['alice'][0],'PUT',identities['alice'][1]);assert len(posts)==before
    stop(a['process']);a['path'].write_text(json.dumps(a['config']));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    print('PASS tenant mkdir 5: original writer and Unknown holds persist until review; cancellation blocks future creation; cap mismatch precedes identity/hold',flush=True)
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-mkdir')]=[call({'path':path})]
        browser=root/'browser.json';browser.write_text(json.dumps(dict(base=f'http://127.0.0.1:{gp}',alice=users['alice']['token'],bob=users['bob']['token'],readonly=readonly['token'])))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_mkdir_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90);assert r.returncode==0,r.returncode
        assert count('browser-mkdir')==2 and target.is_dir() and not list(target.iterdir())
    cli(cfg,'key-revoke','--key',users['alice']['key_id']);api('alice','/api/turns/'+identities['alice'][0],expected=401)
    print('PASS tenant mkdir 6: revoked Key refuses original facts; explicit UI creates only the chosen directory with no replay or extra tool',flush=True)
    print('ACTUAL tenant mkdir task receipts '+json.dumps(actual,separators=(',',':')),flush=True)
    print('ALL tenant mkdir acceptance passed',flush=True)
finally:
 for gate in gates.values():gate['release'].set()
 for p in reversed(processes):
    if p.poll() is None:stop(p)
 model.shutdown()
