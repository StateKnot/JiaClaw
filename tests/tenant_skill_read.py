#!/usr/bin/env python3
"""Real paired tenant skill reuse; local provider fixture is not quality certification."""
import contextlib
import hashlib
import json
import os
from pathlib import Path
import signal
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


def raw(name, body):
    return '---\nname: '+name+'\ndescription: PRIVATE-description-'+name+'\ntriggers: [activate]\n---\n'+body


def write_library(workspace, bodies):
    entries=[]
    for name, body in bodies.items():
        directory=workspace/'skills'/name;directory.mkdir(parents=True,exist_ok=True)
        text=raw(name,body);(directory/'SKILL.md').write_text(text)
        entries.append({'directory':name,'source':{'repository':'https://github.com/example/reviewed',
            'revision':'a'*40,'skill_sha256':digest(text)}})
    (workspace/'skills.lock.json').write_text(json.dumps({'version':1,'skills':entries}))


def call(name, version, **extra):
    return {'id':'skill-'+uuid.uuid4().hex,'type':'function','function':{
        'name':'skill_read','arguments':json.dumps(dict(name=name,content_sha256=version,**extra))}}


def cli(config,*args):
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
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-skill-') as temporary:
    root=Path(temporary);registry=root/'registry/users.sqlite3';config_path=root/'gateway.json';gp=port()
    gateway={'bind':f'127.0.0.1:{gp}','registry_path':str(registry),'request_timeout_seconds':15,
        'max_in_flight':4,'tracked_turns':True,'backends':[]};backends={}
    for who in ('alice','bob'):
        directory=root/who;directory.mkdir(mode=0o700);workspace=directory/'workspace';workspace.mkdir(mode=0o700)
        bodies={'reviewed':who+'-REVIEWED-body\nUse only authorized tools.','unshared':who+'-UNSHARED-body'}
        write_library(workspace,bodies);policy={'skills':[{'name':'reviewed','content_sha256':digest(bodies['reviewed'])}]}
        token='fixture-skill-'+uuid.uuid4().hex;token_file=directory/'token';token_file.write_text(token);token_file.chmod(0o600)
        number=port();config={'agent':{'name':who,'description':'Scoped disposable tenant skill task','system_instructions':'Only explicitly authorized tools.','workspace_path':str(workspace),'max_turns':10,
            'tool_timeout_secs':2,'skill_lock_required':True},
            'tools':{'skill_read':{'enabled':True,'resources_enabled':False}},
            'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{model.server_port}',
                'api_key':keys[who],'model':'fixture-tenant-skill'},
            'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
            'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3',
                'api_token':token,'tracked_turns':True,'tracked_turn_timeout_secs':10,'shutdown_timeout_secs':2,
                'gateway_channel_chat':True,'tenant_skill_read':policy}}
        path=directory/'config.json';path.write_text(json.dumps(config))
        b=dict(path=path,config=config,port=number,token=token,workspace=workspace,bodies=bodies,
               log=directory/'serve.log',policy=policy);backends[who]=b
        b['process']=launch(['serve','--config',str(path)],number,b['log'])
        gateway['backends'].append({'id':who,'url':f'http://127.0.0.1:{number}/',
            'token_file':str(token_file),'tenant_skill_read':policy})
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
    # Parent missing capability is an actual negative through the same public interface.
    assert all('skill_read' in c for c in caps.values()),caps
    def body(who,prompt,tools=None):
        tools=tools or ['skill_read'];v={'session_id':'http:'+str(uuid.uuid4()),'prompt':prompt,'enabled_tools':tools,'enabled_skills':[]}
        if 'skill_read' in tools:v['skill_read_policy_sha256']=caps[who]['skill_read']['policy_sha256']
        return v
    def complete(who,identity,native=False):
        return wait(lambda:(lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(
            api(who,'/api/turns/'+identity,native=native)),'actual terminal skill receipt')
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
    for who in backends:
        descriptor=caps[who]['skill_read'];assert descriptor['skills']==backends[who]['policy']['skills']
        assert descriptor['max_body_bytes']==131072 and descriptor['max_result_bytes']==262144
        canonical=[1,[[s['name'],s['content_sha256']] for s in descriptor['skills']],131072,262144]
        assert descriptor['policy_sha256']==digest(json.dumps(canonical,ensure_ascii=False,separators=(',',':')))
    mismatch=json.loads(json.dumps(gateway));mismatch['backends'][0]['tenant_skill_read']['skills'][0]['content_sha256']='a'*64
    wrong=root/'mismatch.json';wrong.write_text(json.dumps(mismatch));stop(process)
    failed_start(['gateway','serve','--config',str(wrong)],root/'mismatch.log')
    process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    print('PASS tenant skill 1: default-off paired exact body-version capability, authenticated canonical permission and mismatched startup refusal',flush=True)
    identities={}
    for who in backends:
        prompt='success-'+who;identity,original,value=submit(who,prompt,[call('reviewed',digest(backends[who]['bodies']['reviewed']))])
        assert value['receipt']['state']=='completed' and value['receipt']['result']['tool_names']==['skill_read'],value
        reply=value['receipt']['result']['reply'];assert who+'-REVIEWED-body' in reply and 'UNSHARED' not in reply
        assert count(prompt)==2
        initial=first(prompt);system=initial['messages'][0]['content'];assert 'reviewed' in system
        assert all(secret not in system for secret in ['UNSHARED','unshared','PRIVATE-description','REVIEWED-body','https://github.com/example'])
        definitions=initial['tools'];assert len(definitions)==1 and definitions[0]['function']['name']=='skill_read'
        parameters=definitions[0]['function']['parameters'];assert parameters['additionalProperties'] is False
        assert parameters['properties']['name']['enum']==['reviewed']
        assert parameters['properties']['content_sha256']['enum']==[digest(backends[who]['bodies']['reviewed'])]
        identities[who]=(identity,original,value)
    api('bob','/api/turns/'+identities['alice'][0],expected=404)
    api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','time-only',['datetime_now']),202,native=True)
    complete('alice',identity,True);assert 'Available Skills' not in first('time-only')['messages'][0]['content']
    print('PASS tenant skill 2: two actual users reuse original loaded body, minimal prompt/catalog/schema, no eager text and private historical facts',flush=True)
    before=len(posts)
    invalid=[dict(body('alice','missing'),skill_read_policy_sha256=None),dict(body('alice','stale'),skill_read_policy_sha256='a'*64),
        dict(body('alice','eager'),enabled_skills=['reviewed']),dict(body('alice','resource'),enabled_tools=['skill_resource_read']),
        dict(body('alice','dup'),enabled_tools=['skill_read','skill_read']),dict(body('alice','unexpected',['datetime_now']),skill_read_policy_sha256='a'*64)]
    for v in invalid:
        identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400);api('alice','/api/turns/'+identity,expected=404)
        api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',v,400,native=True)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token']);assert len(posts)==before
    for name,version,extra in [('unshared',digest(backends['alice']['bodies']['unshared']),{}),('reviewed','a'*64,{}),('reviewed',digest(backends['alice']['bodies']['reviewed']),{'path':'private'})]:
        prompt='invalid-call-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(name,version,**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1 and v['receipt']['result'] is None,v
    print('PASS tenant skill 3: readonly/hash/eager/resource/duplicate/unknown-argument refusals, original whole-batch preflight and no unauthorized body',flush=True)
    # Existing complete safe source reader, original lock and raw/JSON bounds remain mandatory.
    a=backends['alice'];source=a['workspace']/'skills/reviewed/SKILL.md';original=source.read_bytes()
    for name in ['symlink','hardlink','fifo','directory','utf8','nul','oversize','source-lock']:
        source.unlink()
        external=root/'external';external.write_bytes(original)
        if name=='symlink':source.symlink_to(external)
        elif name=='hardlink':os.link(external,source)
        elif name=='fifo':os.mkfifo(source)
        elif name=='directory':source.mkdir()
        elif name=='utf8':source.write_bytes(b'\xff')
        elif name=='nul':source.write_bytes(b'a\0b')
        elif name=='oversize':source.write_bytes(b'x'*131073)
        else:source.write_bytes(original+b'changed')
        api('alice','/api/skills/reload','POST',{},400,native=True)
        assert api('alice','/api/turns/capabilities',native=True)['skill_read']==caps['alice']['skill_read']
        if name=='directory':source.rmdir()
        else:source.unlink()
        source.write_bytes(original)
    _,_,v=submit('alice','cached-read',[call('reviewed',digest(a['bodies']['reviewed']))],native=True)
    assert 'alice-REVIEWED-body' in v['receipt']['result']['reply']
    stop(process);stop(a['process'])
    for mode in ['lock','tool','resources','empty','duplicate','unsorted','overcount','name-bytes','missing','version']:
        candidate=json.loads(json.dumps(a['config']))
        entries=candidate['http']['tenant_skill_read']['skills']
        if mode=='lock':candidate['agent']['skill_lock_required']=False
        elif mode=='tool':candidate['tools']['skill_read']['enabled']=False
        elif mode=='resources':candidate['tools']['skill_read']['resources_enabled']=True
        elif mode=='empty':entries.clear()
        elif mode=='duplicate':entries.append(dict(entries[0]))
        elif mode=='unsorted':entries.insert(0,{'name':'z','content_sha256':'a'*64})
        elif mode=='overcount':candidate['http']['tenant_skill_read']['skills']=[{'name':f'a{i:02}','content_sha256':'a'*64} for i in range(17)]
        elif mode=='name-bytes':entries[0]['name']='x'*129
        elif mode=='missing':entries[0]['name']='not-loaded'
        else:entries[0]['content_sha256']='a'*64
        rejected=root/(mode+'.json');rejected.write_text(json.dumps(candidate));failed_start(['serve','--config',str(rejected)],root/(mode+'.log'))
    reviewed=a['bodies']['reviewed']
    prefix=raw('reviewed','');full='x'*(131072-len(prefix.encode()))
    for label,content in [('exact-raw',full),('escaped-json','\x01'*60000)]:
        a['bodies']['reviewed']=content;write_library(a['workspace'],a['bodies'])
        a['config']['http']['tenant_skill_read']['skills'][0]['content_sha256']=digest(content)
        a['path'].write_text(json.dumps(a['config']))
        a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
        caps['alice']['skill_read']=api('alice','/api/turns/capabilities',native=True)['skill_read']
        _,_,value=submit('alice',label,[call('reviewed',digest(content))],native=True)
        assert value['receipt']['state']=='completed' and count(label)==2,value
        reply=value['receipt']['result']['reply']
        if label=='exact-raw':assert full in reply and len((a['workspace']/'skills/reviewed/SKILL.md').read_bytes())==131072
        else:assert 'no_effect' in reply and 'error' in reply and '\x01' not in reply
        stop(a['process'])
    a['bodies']['reviewed']=reviewed;write_library(a['workspace'],a['bodies'])
    a['config']['http']['tenant_skill_read']['skills'][0]['content_sha256']=digest(reviewed);a['path'].write_text(json.dumps(a['config']))
    a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    caps['alice']['skill_read']=api('alice','/api/turns/capabilities',native=True)['skill_read']
    process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    print('ACTUAL tenant skill budgets: exact complete raw 131072 accepted; escaped native JSON over262144 rejected with NoEffect; ten stopped-owner startup negatives',flush=True)
    print('PASS tenant skill 4: original safe whole-source/mandatory lock/raw and JSON budgets reject unsafe inputs atomically and retain exact loaded body',flush=True)
    # Browser runs on the same real fixture before reconfiguring either identity.
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-skill')]=[call('reviewed',digest(a['bodies']['reviewed']))]
        browser=root/'browser.json';browser.write_text(json.dumps({'base':f'http://127.0.0.1:{gp}',
            'alice':users['alice']['token'],'bob':users['bob']['token'],'readonly':readonly['token']}))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_skill_read_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90)
        assert r.returncode==0,r.returncode
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('alice','reload-inflight')]=gate
    tool_plans[('alice','reload-inflight')]=[call('reviewed',digest(a['bodies']['reviewed']))]
    identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','reload-inflight'),202,native=True)
    assert gate['ready'].wait(5)
    a['bodies']['reviewed']='alice-REPLACEMENT-body';write_library(a['workspace'],a['bodies'])
    api('alice','/api/skills/reload','POST',{},native=True)
    api('alice','/api/turns/capabilities',expected=503,native=True)
    before=len(posts);api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','changed'),503,native=True)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','changed-gateway'),503)
    assert len(posts)==before;gate['release'].set();value=complete('alice',identity,True)
    assert value['receipt']['state']=='completed' and 'no_effect' in value['receipt']['result']['reply'],value
    assert 'REPLACEMENT' not in value['receipt']['result']['reply']
    assert count('reload-inflight')==2
    print('PASS tenant skill 5: actual submitted model/reload rejects old body and new admission, same registry owner and known NoEffect correction without replacement disclosure',flush=True)
    # Original explicit cancel stops future tool/model dispatch. Keep the actual model waiter gated.
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob','cancel-inflight')]=gate
    tool_plans[('bob','cancel-inflight')]=[call('reviewed',digest(backends['bob']['bodies']['reviewed']))]
    identity=str(uuid.uuid4());api('bob','/api/turns/'+identity,'PUT',body('bob','cancel-inflight'),202,native=True)
    assert gate['ready'].wait(5);api('bob','/api/turns/'+identity+'/cancel','POST',{},native=True)
    gate['release'].set();value=complete('bob',identity,True)
    assert value['receipt']['cancel_requested'] and count('cancel-inflight')==1,value
    cancel_receipt=value
    assert value['receipt']['result'] is None or 'REVIEWED-body' not in value['receipt']['result']['reply']
    print('PASS tenant skill 6: actual model gate/explicit cancellation retains original operation identity and prevents future skill/model dispatch',flush=True)
    stop(process);stop(a['process']);a['config']['http'].pop('tenant_skill_read');a['path'].write_text(json.dumps(a['config']))
    gateway['backends'][0].pop('tenant_skill_read');config_path.write_text(json.dumps(gateway))
    a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log']);process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    assert 'skill_read' not in api('alice','/api/turns/capabilities')
    before=len(posts);identity,original,value=identities['alice']
    assert api('alice','/api/turns/'+identity,'PUT',original)['receipt']['request_hash']==value['receipt']['request_hash']
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','withdrawn'),400)
    assert len(posts)==before
    identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','default-off',['datetime_now']),202)
    settled('alice');complete('alice',identity);assert 'Available Skills' not in first('default-off')['messages'][0]['content']
    cli(config_path,'key-revoke','--key',users['alice']['key_id']);api('alice','/api/turns/capabilities',expected=401)
    print('PASS tenant skill 7: paired withdrawal/default-off hides all catalog, old UUID facts retain hash/no redispatch, revoked keys and isolated users',flush=True)
    receipts=[]
    for who,(original_id,original_body,original_result) in identities.items():
        actual_body=json.loads(original_result['receipt']['result']['reply'].split(': ',1)[1])
        receipts.append({'user':who,'id':original_id,'request_hash':original_result['receipt']['request_hash'],
            'captured_permission':original_body['skill_read_policy_sha256'],'loaded_body_sha256':digest(actual_body),
            'tool_names':original_result['receipt']['result']['tool_names'],'model_posts':count('success-'+who),
            'state':original_result['receipt']['state'],'session_committed':original_result['receipt']['session_committed']})
    print('ACTUAL tenant skill reuse receipts '+json.dumps({'users':receipts,'reload_model_posts':count('reload-inflight'),
        'cancel_model_posts':count('cancel-inflight'),'cancel_requested':cancel_receipt['receipt']['cancel_requested']}),flush=True)

finally:
 for gate in gates.values():gate['release'].set()
 for p in reversed(processes):stop(p)
 model.shutdown();model.server_close()
