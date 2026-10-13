#!/usr/bin/env python3
"""Actual tenant file delivery through original tools/owners/HTTP/SQLite; no paid calls."""
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
import tenant_http_fixture as f
from tenant_http_browser import App

sha=lambda b:hashlib.sha256(b).hexdigest()
app=None
try:
  with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-write-') as tmp:
    root=Path(tmp)
    grant={'alice':['tenant-documents/report.md','tenant-documents/资料.md'],'bob':['tenant-documents/report.md']}
    app=App(root, file_write=grant)
    def api(who,path,method='GET',body=None,expected=200,token=None,native=False):
        b=app.backends[who]
        code,value,headers=f.request(b['port'] if native else app.gp,path,b['token'] if native else token or app.users[who]['token'],method,body,marker=native)
        assert code==expected,(who,path,code,value)
        return value
    caps={who:api(who,'/api/turns/capabilities') for who in grant}
    def body(who,prompt,tools=None):
        tools=tools or ['file_write']
        result={'session_id':'http:'+str(uuid.uuid4()),'prompt':prompt,'enabled_tools':tools,'enabled_skills':[]}
        if 'file_write' in tools:result['file_write_policy_sha256']=caps[who]['file_write']['policy_sha256']
        return result
    def plan(who,prompt,path,content,**extra):
        f.tool_plans[(who,prompt)]=[{'id':'write-'+uuid.uuid4().hex,'type':'function','function':{'name':'file_write','arguments':json.dumps(dict(path=path,content=content,**extra),ensure_ascii=False)}}]
    def count(prompt):
        with f.lock:return len([p for p in f.posts if p['prompt']==prompt])
    def complete(who,identity,native=False):
        return f.wait(lambda:(lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(api(who,'/api/turns/'+identity,native=native)),'original terminal write receipt')
    def settle(who):
        f.wait(lambda:not f.sql(app.registry,'SELECT * FROM write_holds WHERE user_id=?',(app.users[who]['user_id'],)),'original gateway write settlement')
    def submit(who,prompt,path,content,native=False,**extra):
        identity=str(uuid.uuid4());original=body(who,prompt);plan(who,prompt,path,content,**extra)
        api(who,'/api/turns/'+identity,'PUT',original,202,native=native)
        return identity,original,complete(who,identity,native)
    def observe_review_hold(who,identity,started,deadline,prefix):
        def observed():
            rows=f.sql(app.registry,'SELECT request_id,state FROM write_holds WHERE user_id=?',(app.users[who]['user_id'],))
            assert time.monotonic()<deadline, 'original 15s hold observation deadline exceeded'
            return rows if rows==[{'request_id':identity,'state':'needs_review'}] else None
        rows=f.wait(observed,'original gateway unknown write hold',seconds=max(0,deadline-time.monotonic()))
        assert time.monotonic()<deadline and rows==[{'request_id':identity,'state':'needs_review'}]
        print(prefix+json.dumps({'id':identity,'state':'needs_review','elapsed_ms':int((time.monotonic()-started)*1000),'within_original_15s':True,'same_absolute_deadline':True}),flush=True)
    def review_native(who,identity):
        value=api(who,'/api/turns/'+identity+'/review','POST',{'decision':'abandon','note':'Actual original model receipt and workspace effects inspected in disposable fixture.'},native=True)
        assert value['receipt']['reviewed_ms'] is not None
    assert f.request(app.gp,'/api/turns/capabilities')[0]==401
    receipts=[];identities={}
    for who in grant:
        c=caps[who]['file_write'];assert c['paths']==grant[who] and c['mode']=='overwrite' and c['max_bytes']==262144 and c['protocol']==1
        assert c['policy_sha256']==sha(json.dumps([1,grant[who],'overwrite',262144],ensure_ascii=False,separators=(',',':')).encode())
        assert caps[who]['enabled_tools']==['datetime_now','json_query','file_write']
        assert 'file_read' not in caps[who] and 'memory_search' not in caps[who]
    f.stop(app.process)
    wrong=json.loads(json.dumps(app.gateway));wrong['backends'][0]['tenant_file_write']['paths']=['tenant-documents/other.md'];bad=root/'mismatch.json';bad.write_text(json.dumps(wrong))
    r=subprocess.run([str(f.binary),'gateway','serve','--config',str(bad)],env=f.env,capture_output=True,timeout=15)
    assert r.returncode==1 and b'permission contract mismatch' in r.stderr,r.stderr
    app.start()
    print('PASS tenant write 1: paired exact separate write grant, canonical SHA, authentication and mismatched startup refusal',flush=True)
    for who in grant:
        content='\ufeff'+who+'-PRIVATE-output🦀\r\n\0\t end '
        identity,original,result=submit(who,'write-success-'+who,grant[who][0],content);settle(who)
        assert result['receipt']['state']=='completed' and result['receipt']['result']['tool_names']==['file_write'] and count('write-success-'+who)==2,result
        target=app.backends[who]['workspace']/grant[who][0];raw=target.read_bytes();assert raw==content.encode()
        before=len(f.posts);again=api(who,'/api/turns/'+identity,'PUT',original);assert again['receipt']==result['receipt'] and len(f.posts)==before
        api('bob' if who=='alice' else 'alice','/api/turns/'+identity,expected=404)
        identities[who]=(identity,original,result)
        receipts.append({'user':who,'id':identity,'policy_sha256':caps[who]['file_write']['policy_sha256'],'path':grant[who][0],'file_bytes':len(raw),'file_sha256':sha(raw),'model_posts':2,'tool_names':['file_write'],'duplicate_posts':0,'original_receipt_preserved':True})
        first=next(p for p in f.posts if p['prompt']=='write-success-'+who)['body']['tools'][0]['function']
        assert first['name']=='file_write' and first['parameters']['properties']['path']['enum']==grant[who] and first['parameters']['additionalProperties'] is False
        identity,_,result=submit(who,'overwrite-empty-'+who,grant[who][0],'');settle(who)
        assert target.read_bytes()==b'' and result['receipt']['state']=='completed'
        target.write_text('ORIGINAL-'+who)
    api('alice','/api/turns/'+identities['alice'][0],token=app.readonly['token'])
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly-write'),403,token=app.readonly['token'])
    print('PASS tenant write 2: two isolated actual files preserve complete UTF8/BOM/CRLF/NUL, explicit empty overwrite, original tool/schema/receipt and no UUID redispatch',flush=True)
    before=len(f.posts)
    for change in [lambda v:v.pop('file_write_policy_sha256'),lambda v:v.update(file_write_policy_sha256='a'*64),lambda v:v.update(enabled_tools=['datetime_now'])]:
        value=body('alice','invalid-permission');change(value);api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',value,400)
    assert len(f.posts)==before
    for name,path,extra in [('outside','MEMORY.md',{}),('alias','tenant-documents/./report.md',{}),('append',grant['alice'][0],{'mode':'append'}),('extra',grant['alice'][0],{'extra':True})]:
        identity,_,result=submit('alice','write-invalid-'+name,path,'BAD',native=True,**extra)
        assert result['receipt']['state']=='needs_review' and count('write-invalid-'+name)==1,result
        assert (app.backends['alice']['workspace']/grant['alice'][0]).read_text()=='ORIGINAL-alice'
        review_native('alice',identity)
    print('PASS tenant write 3: missing/changed/unselected SHA and exact native batch path/mode/extra preflight refuse without file effects or later model dispatch',flush=True)
    if '--browser' in sys.argv:
        plan('alice','browser-write',grant['alice'][0],'browser-private-output🦀')
        browser=root/'browser.json';browser.write_text(json.dumps({'base':f'http://127.0.0.1:{app.gp}','alice':app.users['alice']['token'],'bob':app.users['bob']['token'],'readonly':app.readonly['token']}))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_file_write_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90)
        assert r.returncode==0,r.returncode
        assert (app.backends['alice']['workspace']/grant['alice'][0]).read_text()=='browser-private-output🦀' and count('browser-write')==2
        settle('alice')
    target=app.backends['alice']['workspace']/grant['alice'][0];target.write_text('ORIGINAL-alice')
    protected=root/'protected';protected.write_text('DO-NOT-REPLACE')
    for name in ['symlink','hardlink','fifo','directory']:
        target.unlink()
        if name=='symlink':target.symlink_to(protected)
        elif name=='hardlink':os.link(protected,target)
        elif name=='fifo':os.mkfifo(target)
        else:target.mkdir()
        identity,_,result=submit('alice','write-unsafe-'+name,grant['alice'][0],'BAD',native=True)
        assert result['receipt']['state']=='needs_review' and count('write-unsafe-'+name)==1,result
        assert protected.read_text()=='DO-NOT-REPLACE';review_native('alice',identity)
        target.rmdir() if name=='directory' else target.unlink();target.write_text('ORIGINAL-alice')
    print('PASS tenant write 4: original symlink/hardlink/FIFO/directory safeguards and conservative Unknown stop further dispatch; protected files preserved',flush=True)
    fd=os.open(app.backends['alice']['workspace'],os.O_RDONLY)
    try:
        fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
        hold_started=time.monotonic();hold_deadline=hold_started+15
        identity,_,result=submit('alice','write-writer-busy',grant['alice'][0],'BAD')
        assert result['receipt']['state']=='needs_review' and count('write-writer-busy')==1 and target.read_text()=='ORIGINAL-alice',result
        observe_review_hold('alice',identity,hold_started,hold_deadline,'ACTUAL tenant write hold observation ')
        bob,_,value=submit('bob','write-independent-bob',grant['bob'][0],'BOB-independent');settle('bob')
        assert value['receipt']['state']=='completed' and (app.backends['bob']['workspace']/grant['bob'][0]).read_text()=='BOB-independent'
    finally:fcntl.flock(fd,fcntl.LOCK_UN);os.close(fd)
    app.review('alice',identity)
    print('PASS tenant write 5: real original workspace writer ownership refuses competing write, preserves review hold and allows independent Bob without automatic retry',flush=True)
    prompt='write-cancel';gate={'ready':threading.Event(),'release':threading.Event()};f.gates[('alice',prompt)]=gate
    identity=str(uuid.uuid4());plan('alice',prompt,grant['alice'][0],'CANCELLED');original=body('alice',prompt)
    api('alice','/api/turns/'+identity,'PUT',original,202);assert gate['ready'].wait(5)
    cancel_started=time.monotonic();cancel_deadline=cancel_started+15
    api('alice','/api/turns/'+identity+'/cancel','POST',{})
    gate['release'].set();value=complete('alice',identity)
    assert value['receipt']['cancel_requested'] and value['receipt']['state']=='needs_review' and count(prompt)==1 and target.read_text()=='ORIGINAL-alice',value
    observe_review_hold('alice',identity,cancel_started,cancel_deadline,'ACTUAL tenant write cancel hold observation ')
    app.review('alice',identity)
    print('PASS tenant write 6: actual submitted model cancellation blocks future file dispatch, keeps original cancel intent and review until explicit fixture reconciliation',flush=True)
    f.stop(app.process);b=app.backends['alice'];f.stop(b['process']);b['config']['http'].pop('tenant_file_write');b['path'].write_text(json.dumps(b['config']));app.gateway['backends'][0].pop('tenant_file_write');app.config_path.write_text(json.dumps(app.gateway))
    b['process']=f.launch(['serve','--config',str(b['path'])],b['port'],b['log']);app.start()
    assert 'file_write' not in api('alice','/api/turns/capabilities')
    identity,original,result=identities['alice'];before=len(f.posts)
    assert api('alice','/api/turns/'+identity,'PUT',original)['receipt']==result['receipt']
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','withdrawn-write'),400);assert len(f.posts)==before
    app.cli('key-revoke','--key',app.users['alice']['key_id']);api('alice','/api/turns/'+identity,expected=401)
    print('PASS tenant write 7: paired epoch withdrawal leaves original UUID facts/no redispatch; new writes and revoked Key refuse',flush=True)
    print('ACTUAL tenant write receipts '+json.dumps({'users':receipts,'writer_busy_model_posts':1,'cancel_model_posts':1,'cancel_file_effects':0,'actual_retry_posts':0,'old_UUID_redispatches':0,'readonly_write_refused':True,'cross_user_refused':True,'permission_withdrawn':True}),flush=True)
    assert not f.faults,f.faults
    print('ALL TENANT FILE WRITE GROUPS PASS',flush=True)
finally:
    for gate in f.gates.values():gate['release'].set()
    for p in reversed(f.processes):
        if p.poll() is None:p.terminate();p.wait(timeout=8)
    f.model.shutdown()
