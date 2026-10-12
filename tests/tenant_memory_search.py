#!/usr/bin/env python3
"""Real explicit tenant keyword reuse; local fixtures do not certify memory quality."""
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
from tenant_http_fixture import (binary, env, keys, lock, posts, gates, faults, processes,
    tool_plans, wait, port, sql, request, model, stop, launch)


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def call(query='needle', **extra):
    return {'id':'search-'+uuid.uuid4().hex,'type':'function','function':{
        'name':'memory_search','arguments':json.dumps(dict(query=query, **extra))}}


def cli(config, *args):
    r=subprocess.run([str(binary),'gateway',*args,'--config',str(config)],env=env,
        capture_output=True,text=True,timeout=15)
    assert r.returncode==0,(args,r.stdout,r.stderr)
    return json.loads(r.stdout)


def failed_start(args, log):
    with log.open('wb') as output:
        p=subprocess.Popen([str(binary),*args],env=env,stdout=output,stderr=output)
    processes.append(p)
    assert p.wait(timeout=15)==1,log.read_text()


try:
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-memory-') as temporary:
    root=Path(temporary);registry=root/'registry/users.sqlite3';config_path=root/'gateway.json';gp=port()
    gateway={'bind':f'127.0.0.1:{gp}','registry_path':str(registry),'request_timeout_seconds':15,
        'max_in_flight':4,'tracked_turns':True,'backends':[]};backends={}
    for who in ('alice','bob'):
        directory=root/who;directory.mkdir(mode=0o700);workspace=directory/'workspace';workspace.mkdir(mode=0o700)
        (workspace/'notes').mkdir();(workspace/'MEMORY.md').write_text(who+'-needle-remembered\ncontext\n')
        (workspace/'notes/USER.md').write_text(who+'-ambient-user')
        (workspace/'SOUL.md').write_text(who+'-ambient-soul')
        (workspace/'private.txt').write_text('DO-NOT-DISCLOSE-'+who)
        policy={'paths':['MEMORY.md','notes/USER.md'] if who=='alice' else ['MEMORY.md']}
        token='fixture-memory-'+uuid.uuid4().hex;token_file=directory/'token';token_file.write_text(token);token_file.chmod(0o600)
        number=port();config={'agent':{'name':who,'description':'Explicit keyword memory reuse','workspace_path':str(workspace),
            'system_instructions':'Only explicitly authorized tools.','max_turns':10,'tool_timeout_secs':2},
            'identity':{'user_path':'notes/USER.md'},
            'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{model.server_port}',
                'api_key':keys[who],'model':'fixture-tenant-memory'},
            'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
            'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3',
                'api_token':token,'tracked_turns':True,'tracked_turn_timeout_secs':10,'shutdown_timeout_secs':2,
                'gateway_channel_chat':True,'tenant_memory_search':policy}}
        if who=='alice':
            config['memory']={'semantic':{'enabled':True,'model':'fixture-embedding','space_revision':'v1',
                'dimensions':4,'sources':['MEMORY.md'],'index_path':'../state/semantic/index.sqlite3','timeout_secs':5}}
        path=directory/'config.json';path.write_text(json.dumps(config))
        b=dict(path=path,config=config,port=number,token=token,workspace=workspace,policy=policy,log=directory/'serve.log')
        backends[who]=b;b['process']=launch(['serve','--config',str(path)],number,b['log'])
        gateway['backends'].append({'id':who,'url':f'http://127.0.0.1:{number}/','token_file':str(token_file),'tenant_memory_search':policy})
    config_path.write_text(json.dumps(gateway));users={w:cli(config_path,'user-add','--backend',w) for w in backends}
    readonly=cli(config_path,'key-add','--user',users['alice']['user_id'],'--read-only')
    process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    def api(who,path,method='GET',body=None,expected=200,token=None,native=False):
        b=backends[who];code,value,headers=request(b['port'] if native else gp,path,
            b['token'] if native else (token or users[who]['token']),method,body,marker=native)
        assert code==expected,(who,path,code,value)
        if not native:assert headers.get('Cache-Control')=='no-store' and headers.get('X-Content-Type-Options')=='nosniff'
        return value
    caps={w:api(w,'/api/turns/capabilities') for w in backends}
    for b in backends.values():
        diagnosis=subprocess.run([str(binary),'doctor','--config',str(b['path'])],env=env,capture_output=True,text=True,timeout=15)
        assert diagnosis.returncode==0 and 'tenant keyword' in diagnosis.stdout,(diagnosis.returncode,diagnosis.stdout,diagnosis.stderr)
    def body(who,prompt,tools=None):
        tools=tools or ['memory_search'];v={'session_id':'http:'+str(uuid.uuid4()),'prompt':prompt,'enabled_tools':tools,'enabled_skills':[]}
        if 'memory_search' in tools:v['memory_search_policy_sha256']=caps[who]['memory_search']['policy_sha256']
        return v
    def complete(who,identity,native=False):
        return wait(lambda:(lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(
            api(who,'/api/turns/'+identity,native=native)),'actual terminal memory receipt')
    def settled(who):
        deadline=time.monotonic()+15
        wait(lambda:not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users[who]['user_id'],)), 'original gateway settlement')
        assert time.monotonic()<deadline,'original 15s gateway settlement deadline exceeded'
    def submit(who,prompt,calls=None,native=False):
        if calls is not None:tool_plans[(who,prompt)]=calls
        identity=str(uuid.uuid4());original=body(who,prompt);api(who,'/api/turns/'+identity,'PUT',original,202,native=native)
        if not native:settled(who)
        return identity,original,complete(who,identity,native)
    def count(prompt):
        with lock:return len([p for p in posts if p['prompt']==prompt])
    def first(prompt):
        with lock:return next(p for p in posts if p['prompt']==prompt)['body']
    def output(value):
        decoded=json.loads(value['receipt']['result']['reply'].split(': ',1)[1])
        if isinstance(decoded,str):decoded=json.loads(decoded)
        assert isinstance(decoded,dict),decoded
        return decoded
    for who in backends:
        descriptor=caps[who]['memory_search'];assert descriptor['paths']==backends[who]['policy']['paths']
        canonical=[1,descriptor['paths'],'keyword',524288,1572864,1024,20,1024,65536]
        assert descriptor==dict(protocol=1,paths=canonical[1],mode='keyword',max_source_bytes=524288,max_total_bytes=1572864,
            max_query_bytes=1024,max_results=20,max_excerpt_bytes=1024,max_result_bytes=65536,
            policy_sha256=digest(json.dumps(canonical,separators=(',',':'))))
    stop(process);mismatch=json.loads(json.dumps(gateway));mismatch['backends'][0]['tenant_memory_search']['paths']=['SOUL.md']
    wrong=root/'mismatch.json';wrong.write_text(json.dumps(mismatch));failed_start(['gateway','serve','--config',str(wrong)],root/'mismatch.log')
    process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    print('PASS tenant memory 1: default-off paired exact keyword permission and canonical full bounds; mismatch startup refuses',flush=True)
    identities={}
    for who in backends:
        prompt='success-'+who;identity,original,value=submit(who,prompt,[call('NEEDLE')])
        assert value['receipt']['state']=='completed' and value['receipt']['result']['tool_names']==['memory_search'],value
        actual=output(value);assert actual['matches']==[{'path':'MEMORY.md','line':1,'excerpt':who+'-needle-remembered\ncontext'}],actual
        assert count(prompt)==2
        initial=first(prompt);system=initial['messages'][0]['content']
        assert who+'-ambient-soul' in system and who+'-ambient-user' in system and who+'-needle-remembered' in system
        assert 'DO-NOT-DISCLOSE' not in system
        definition=initial['tools'];assert len(definition)==1 and definition[0]['function']['name']=='memory_search'
        schema=definition[0]['function']['parameters'];assert schema['additionalProperties'] is False and set(schema['properties'])=={'query','max_results'}
        identities[who]=(identity,original,value)
    api('bob','/api/turns/'+identities['alice'][0],expected=404)
    api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    identity=str(uuid.uuid4());api('bob','/api/turns/'+identity,'PUT',body('bob','ambient-only',['datetime_now']),202,native=True)
    complete('bob',identity,True);assert 'bob-needle-remembered' in first('ambient-only')['messages'][0]['content']
    print('PASS tenant memory 2: two real keyword tasks reuse original sources with exact captured permission, isolated receipts and independent ambient prompt memory',flush=True)
    before=len(posts)
    invalid=[dict(body('alice','missing'),memory_search_policy_sha256=None),dict(body('alice','stale'),memory_search_policy_sha256='a'*64),
        dict(body('alice','eager'),enabled_skills=['private']),dict(body('alice','dup'),enabled_tools=['memory_search','memory_search']),
        dict(body('alice','unexpected',['datetime_now']),memory_search_policy_sha256='a'*64)]
    for v in invalid:
        identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400);api('alice','/api/turns/'+identity,expected=404)
        api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',v,400,native=True)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token']);assert len(posts)==before
    for extra in [{'paths':['private.txt']},{'mode':'semantic'},{'mode':'keyword'},{'extra':True},{'max_results':21}]:
        prompt='invalid-call-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1 and v['receipt']['result'] is None,v
    print('PASS tenant memory 3: permission/read-only/duplicate/eager/unknown argument and semantic-path refusals before effects; semantic-configured Agent stays restricted',flush=True)
    query_receipts=[]
    for query in ['\u2000'*400+'needle','needle'+'x'*1018]:
        prompt='query-budget-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(query)],native=True)
        assert v['receipt']['state']=='completed' and count(prompt)==2,v
        if len(query.encode())>1024:assert 'no_effect' in v['receipt']['result']['reply'] and 'error' in v['receipt']['result']['reply'],v
        else:assert output(v)['query']==query and output(v)['matches']==[]
        query_receipts.append({'wire_bytes':len(query.encode()),'characters':len(query),
            'effect_status':output(v).get('effect_status'),'model_posts':count(prompt),'state':v['receipt']['state']})
    print('ACTUAL tenant memory query: raw1206 UTF8/406 chars rejects before trim; exact1024 accepted',flush=True)
    a=backends['alice'];source=a['workspace']/'notes/USER.md';original=source.read_bytes()
    for name in ['symlink','parent-link','hardlink','fifo','directory','utf8','nul','oversize','missing','exact-budget']:
        source.unlink()
        if name=='symlink':source.symlink_to(a['workspace']/'private.txt')
        elif name=='parent-link':
            parent=source.parent;park=parent.with_name('parked');parent.rename(park);parent.symlink_to(park,target_is_directory=True)
        elif name=='hardlink':os.link(a['workspace']/'private.txt',source)
        elif name=='fifo':os.mkfifo(source)
        elif name=='directory':source.mkdir()
        elif name=='utf8':source.write_bytes(b'\xff')
        elif name=='nul':source.write_bytes(b'a\0b')
        elif name=='oversize':source.write_bytes(b'x'*524289)
        elif name=='exact-budget':source.write_bytes(b'x'*524288)
        _,_,v=submit('alice','unsafe-'+name,[call(max_results=1)],native=True)
        assert v['receipt']['state']=='completed' and count('unsafe-'+name)==2,v
        if name=='exact-budget':assert output(v)['matches'][0]['path']=='MEMORY.md'
        else:assert 'no_effect' in v['receipt']['result']['reply'] and 'error' in v['receipt']['result']['reply'] and 'needle-remembered' not in v['receipt']['result']['reply'],v
        if name=='parent-link':source.parent.unlink();park.rename(source.parent)
        elif name=='directory':source.rmdir()
        elif name!='missing':source.unlink()
        source.write_bytes(original)
    memory=a['workspace']/'MEMORY.md';saved=memory.read_bytes();memory.write_text(('needle'+'\x01'*1010+'\n')*20)
    _,_,v=submit('alice','escaped-result',[call(max_results=20)],native=True)
    assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count('escaped-result')==2,v
    memory.write_bytes(saved)
    print('PASS tenant memory 4: complete trailing source refuses links/unsafe parent/hardlink/FIFO/directory/UTF8/NUL/missing/524289; exact524288 and original 65536 JSON bound',flush=True)
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-memory')]=[call()]
        browser=root/'browser.json';browser.write_text(json.dumps({'base':f'http://127.0.0.1:{gp}',
            'alice':users['alice']['token'],'bob':users['bob']['token'],'readonly':readonly['token']}))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_memory_search_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90)
        assert r.returncode==0,r.returncode
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob','cancel-inflight')]=gate
    tool_plans[('bob','cancel-inflight')]=[call()];identity=str(uuid.uuid4());original_body=body('bob','cancel-inflight')
    api('bob','/api/turns/'+identity,'PUT',original_body,202,native=True);assert gate['ready'].wait(5)
    api('bob','/api/turns/'+str(uuid.uuid4()),'PUT',body('bob','busy-model'),409,native=True)
    api('bob','/api/turns/'+identity,'PUT',original_body,native=True)
    api('bob','/api/turns/'+identity+'/cancel','POST',{},native=True);gate['release'].set();cancel=complete('bob',identity,True)
    assert cancel['receipt']['cancel_requested'] and count('cancel-inflight')==1 and cancel['receipt']['result'] is None,cancel
    print('PASS tenant memory 5: actual submitted model owner/concurrent refusal/duplicate facts/explicit cancel stops future search and model dispatch',flush=True)
    b=backends['bob'];stop(b['process']);lib=root/('hold.dylib' if sys.platform=='darwin' else 'hold.so')
    args=['cc','-dynamiclib' if sys.platform=='darwin' else '-shared','-std=c11','-O2','-fPIC',str(Path(__file__).with_name('tenant_file_read_hold.c')),'-o',str(lib)]
    if sys.platform!='darwin':args.append('-ldl')
    subprocess.run(args,check=True,capture_output=True,timeout=30)
    arm=root/'arm';marker=root/'held';release=root/'release'
    injected=dict(env,JIACLAW_TEST_FILE_SOURCE=str(b['workspace']/'MEMORY.md'),JIACLAW_TEST_FILE_ARM=str(arm),JIACLAW_TEST_FILE_MARKER=str(marker),JIACLAW_TEST_FILE_RELEASE=str(release))
    injected['DYLD_INSERT_LIBRARIES' if sys.platform=='darwin' else 'LD_PRELOAD']=str(lib)
    b['process']=launch(['serve','--config',str(b['path'])],b['port'],b['log'],injected)
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob','held-search')]=gate;tool_plans[('bob','held-search')]=[call()]
    identity=str(uuid.uuid4());api('bob','/api/turns/'+identity,'PUT',body('bob','held-search'),202,native=True)
    assert gate['ready'].wait(5);arm.write_text('arm-after-ambient');gate['release'].set()
    try:
        wait(lambda:(root/'held-1').exists(),'actual keyword source reader held',5);arm.unlink()
        api('bob','/api/turns/'+identity+'/cancel','POST',{},native=True);held=complete('bob',identity,True)
        assert held['receipt']['cancel_requested'] and held['receipt']['state']=='needs_review' and count('held-search')==1,held
        _,_,busy=submit('bob','writer-still-held',[call()],native=True)
        assert 'workspace write lock busy' in busy['receipt']['result']['reply'] and 'no_effect' in busy['receipt']['result']['reply'],busy
        release.write_text('settle')
        deadline=time.monotonic()+5
        while True:
            _,_,after=submit('bob','settled-'+uuid.uuid4().hex,[call()],native=True)
            if 'bob-needle-remembered' in after['receipt']['result']['reply']:break
            assert time.monotonic()<deadline,'original keyword owner did not settle'
        assert len(list(root.glob('held-*')))==1
    finally:
        release.write_text('settle');stop(b['process'])
    b['process']=launch(['serve','--config',str(b['path'])],b['port'],b['log'])
    print('PASS tenant memory 6: actual keyword reader retains cooperative writer after cancel/waiter timeout; concurrent search refuses until real release, no replay',flush=True)
    stop(process);stop(a['process']);a['config']['http'].pop('tenant_memory_search');a['path'].write_text(json.dumps(a['config']))
    gateway['backends'][0].pop('tenant_memory_search');config_path.write_text(json.dumps(gateway))
    a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log']);process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    assert 'memory_search' not in api('alice','/api/turns/capabilities')
    before=len(posts);identity,original_body,v=identities['alice']
    assert api('alice','/api/turns/'+identity,'PUT',original_body)['receipt']['request_hash']==v['receipt']['request_hash']
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','withdrawn'),400);assert len(posts)==before
    cli(config_path,'key-revoke','--key',users['alice']['key_id']);api('alice','/api/turns/capabilities',expected=401)
    print('PASS tenant memory 7: paired withdrawal/default-off denies new search, historical captured permission/hash stays lookup-only, revoked and cross-user keys refuse',flush=True)
    receipts=[]
    for who,(identity,original_body,v) in identities.items():
        actual=output(v);receipts.append({'user':who,'id':identity,'request_hash':v['receipt']['request_hash'],
            'captured_permission':original_body['memory_search_policy_sha256'],'paths':backends[who]['policy']['paths'],
            'matches':actual['matches'],'tool_names':v['receipt']['result']['tool_names'],'model_posts':count('success-'+who),
            'state':v['receipt']['state'],'session_committed':v['receipt']['session_committed']})
    print('ACTUAL tenant memory reuse receipts '+json.dumps({'users':receipts,'cancel_model_posts':count('cancel-inflight'),
        'cancel_requested':cancel['receipt']['cancel_requested'],'held_reader_markers':len(list(root.glob('held-*'))),
        'held_cancel_requested':held['receipt']['cancel_requested'],'held_model_posts':count('held-search'),
        'busy_model_posts':count('writer-still-held'),'query_cases':query_receipts,
        'ambient_memory_preserved':all(w+'-ambient-soul' in first('success-'+w)['messages'][0]['content'] for w in backends)}),flush=True)
    assert not faults,faults
finally:
    for gate in gates.values():gate['release'].set()
    for p in processes:stop(p)
    model.shutdown();model.server_close()
