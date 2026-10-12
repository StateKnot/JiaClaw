#!/usr/bin/env python3
"""Actual scoped tenant model/tool/HTTP/SQLite tasks; no supplier or quality claim."""
import contextlib
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from tenant_http_fixture import (binary, env, keys, lock, posts, gates, faults, processes,
    tool_plans, wait, port, sql, request, model, stop, launch)
from tenant_file_gateway import ReceiptRelay

relay = None

def call(path, **extra):
    return {'id':'read-' + uuid.uuid4().hex, 'type':'function',
            'function':{'name':'file_read','arguments':json.dumps(dict(path=path, **extra))}}

def planned(who, prompt, path, **extra):
    tool_plans[(who,prompt)] = [call(path, **extra)]

def cli(config, *args, expected=0):
    r = subprocess.run([str(binary),'gateway',*args,'--config',str(config)], env=env,
                       capture_output=True,text=True,timeout=15)
    assert r.returncode == expected, (args,r.returncode,r.stdout,r.stderr)
    return json.loads(r.stdout) if expected == 0 else r.stderr

def failed_start(args, log):
    with log.open('wb') as output:
        p = subprocess.Popen([str(binary),*args], env=env, stdout=output,stderr=output)
    processes.append(p)
    assert p.wait(timeout=15) == 1, log.read_text()

try:
  with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-file-') as temporary:
    root = Path(temporary); registry = root/'registry/users.sqlite3'; config_path = root/'gateway.json'
    gp = port(); gateway = {'bind':f'127.0.0.1:{gp}','registry_path':str(registry),
        'request_timeout_seconds':15,'max_in_flight':4,'tracked_turns':True,'backends':[]}
    backends = {}
    for who in ('alice','bob'):
        directory = root/who; directory.mkdir(mode=0o700)
        workspace = directory/'workspace'; workspace.mkdir(mode=0o700)
        documents = workspace/'tenant-documents'; documents.mkdir()
        source = documents/'notes.txt'; source.write_text(who + '-PRIVATE-source\nsecond line\n')
        (documents/'资料.txt').write_text(who + '-资料🦀')
        (workspace/'unshared.txt').write_text('DO-NOT-DISCLOSE-' + who)
        paths = ['tenant-documents/notes.txt','tenant-documents/资料.txt'] if who=='alice' else ['tenant-documents/notes.txt']
        policy = {'paths':paths}
        token = 'fixture-file-backend-' + uuid.uuid4().hex
        token_file = directory/'token';token_file.write_text(token);token_file.chmod(0o600)
        number = port()
        config = {'agent':{'name':who,'description':'Disposable scoped tenant task',
            'system_instructions':'Only explicitly authorized tools.','max_turns':10,
            'workspace_path':str(workspace),'tool_timeout_secs':2},
            'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{model.server_port}',
                        'api_key':keys[who],'model':'fixture-tenant-file'},
            'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
            'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3',
                'api_token':token,'tracked_turns':True,'tracked_turn_timeout_secs':10,
                'shutdown_timeout_secs':2,'gateway_channel_chat':True,'tenant_file_read':policy}}
        path = directory/'config.json';path.write_text(json.dumps(config))
        b = dict(path=path,config=config,port=number,token=token,workspace=workspace,source=source,
                 db=directory/'state/sessions.sqlite3',log=directory/'serve.log')
        backends[who] = b
        b['process'] = launch(['serve','--config',str(path)],number,b['log'])
        gateway['backends'].append({'id':who,'url':f'http://127.0.0.1:{number}/',
            'token_file':str(token_file),'tenant_file_read':policy})
    # A real local relay makes competing receipt reads fail closed. The success
    # fixture must observe the gateway's own settlement before adding a reader.
    relay = ReceiptRelay(backends['alice']['port'])
    gateway['backends'][0]['url'] = relay.url
    config_path.write_text(json.dumps(gateway))
    users = {who:cli(config_path,'user-add','--backend',who) for who in ('alice','bob')}
    readonly = cli(config_path,'key-add','--user',users['alice']['user_id'],'--read-only')
    process = launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    def api(who,path,method='GET',body=None,expected=200,token=None,native=False):
        b=backends[who]
        code,value,headers=request(b['port'] if native else gp,path,
            b['token'] if native else (token or users[who]['token']),method,body,marker=native)
        assert code==expected,(who,path,code,value)
        if not native:
            assert headers.get('Cache-Control')=='no-store' and headers.get('X-Content-Type-Options')=='nosniff'
        return value
    caps = {who:api(who,'/api/turns/capabilities') for who in backends}
    def body(who,prompt,tools=None):
        tools=tools or ['file_read']
        v={'session_id':'http:'+str(uuid.uuid4()),'prompt':prompt,'enabled_tools':tools,'enabled_skills':[]}
        if 'file_read' in tools:v['file_read_policy_sha256']=caps[who]['file_read']['policy_sha256']
        return v
    def complete(who,identity,native=False):
        return wait(lambda: (lambda v:v if v['receipt']['state']!='running' and not v['active'] else None)(
            api(who,'/api/turns/'+identity,native=native)),'actual terminal file receipt')
    def submit(who,prompt,path,native=False,settle_gateway=False,**extra):
        identity=str(uuid.uuid4());planned(who,prompt,path,**extra)
        original=body(who,prompt)
        api(who,'/api/turns/'+identity,'PUT',original,202,native=native)
        if settle_gateway:
            assert not native
            gateway_settled(who)
        result=complete(who,identity,native)
        return identity,original,result
    def count(prompt):
        with lock:return len([p for p in posts if p['prompt']==prompt])
    settlement_checks=[]
    def gateway_settled(who):
        deadline=time.monotonic()+15
        try:
            wait(lambda:not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users[who]['user_id'],)),
                 'gateway actual settlement')
            assert time.monotonic()<deadline, 'gateway actual settlement exceeded original observation deadline'
        except Exception:
            print('ACTUAL tenant file settlement failure ' + json.dumps({
                'holds':sql(registry,'SELECT state,reason FROM write_holds WHERE user_id=?',(users[who]['user_id'],)),
                'private_get_receipts':relay.snapshot()}),flush=True)
            raise
        settlement_checks.append({'user':who,'hold_empty':True,'before_client_receipt':True,'within_original_15s':True})
    # Paired capability, disabled legacy backend, and mismatched startup.
    assert caps['alice']['file_read']['paths']==gateway['backends'][0]['tenant_file_read']['paths']
    assert caps['bob']['file_read']['paths']==['tenant-documents/notes.txt']
    assert request(gp,'/api/turns/capabilities')[0]==401
    mismatch=json.loads(json.dumps(gateway));mismatch['bind']=f'127.0.0.1:{port()}'
    mismatch['backends'][0]['tenant_file_read']['paths']=['tenant-documents/other.txt']
    wrong=root/'mismatch.json';wrong.write_text(json.dumps(mismatch))
    # Stop the actual registry owner before testing another configured process.
    stop(process);failed_start(['gateway','serve','--config',str(wrong)],root/'mismatch.log')
    process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    print('PASS tenant file 1: explicit paired exact grant, authenticated capability and mismatched startup fail closed',flush=True)
    # Full-source read and model result on two real isolated users.
    identities={}
    for who in backends:
        identity,original,value=submit(who,'success-'+who,'tenant-documents/notes.txt',settle_gateway=True)
        assert value['receipt']['state']=='completed' and value['receipt']['result']['tool_names']==['file_read'],value
        reply=value['receipt']['result']['reply'];assert who+'-PRIVATE-source' in reply
        assert ('bob' if who=='alice' else 'alice')+'-PRIVATE-source' not in reply
        assert count('success-'+who)==2
        identities[who]=(identity,original,value)
        first=next(p for p in posts if p['prompt']=='success-'+who)
        definitions=first['body']['tools'];assert len(definitions)==1 and definitions[0]['function']['name']=='file_read'
        assert definitions[0]['function']['parameters']['properties']['path']['enum']==caps[who]['file_read']['paths']
    api('bob','/api/turns/'+identities['alice'][0],expected=404)
    api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    private_gets=relay.snapshot()
    assert private_gets and all(row=={'status':200,'overlap':False} for row in private_gets),private_gets
    print('ACTUAL tenant file gateway settlement receipts ' + json.dumps({
        'settlement_checks':settlement_checks,'private_get_receipts':private_gets,
        'relay_overlap_refusal_configured':True}),flush=True)
    # Retire the controlled relay before the other HTTP/browser/owner matrices;
    # the unchanged native endpoint remains the gateway's configured target.
    stop(process)
    gateway['backends'][0]['url'] = f"http://127.0.0.1:{backends['alice']['port']}/"
    config_path.write_text(json.dumps(gateway))
    relay.close();relay = None
    process = launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    print('PASS tenant file 2: actual model/tool/source/receipt reuse, exact advertised enum, two users and read-only historical GET',flush=True)
    # Refusals precede provider submission and permanent registry identity.
    before=len(posts)
    invalid=[dict(body('alice','missing'),file_read_policy_sha256=None),dict(body('alice','stale'),file_read_policy_sha256='a'*64),
        dict(body('alice','alias-tool'),enabled_tools=['read_file']),dict(body('alice','write'),enabled_tools=['file_write']),
        dict(body('alice','skills'),enabled_skills=['calculator']),dict(body('alice','dup'),enabled_tools=['file_read','file_read'])]
    for value in invalid:
        identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',value,400)
        api('alice','/api/turns/'+identity,expected=404)
        api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',value,400,native=True)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token'])
    assert len(posts)==before
    # Model attempts a forbidden path; native whole-batch preflight must refuse.
    prompt='forbidden-path';tool_plans[('alice',prompt)]=[call('tenant-documents/notes.txt'),call('unshared.txt')]
    identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice',prompt),202,native=True)
    value=complete('alice',identity,True);assert value['receipt']['state']=='needs_review' and count(prompt)==1,value
    assert value['receipt']['result'] is None
    print('PASS tenant file 3: readonly/hash/name/skill/duplicate refusals before model and original whole-batch path preflight',flush=True)
    # Existing capability reader refuses each unsafe source, before line slicing.
    source=backends['alice']['source'];original_bytes=source.read_bytes()
    for name in ('symlink','parent-link','hardlink','fifo','directory','utf8','nul','oversize','exact-budget'):
        source.unlink()
        if name=='symlink':source.symlink_to(backends['alice']['workspace']/'unshared.txt')
        elif name=='parent-link':
            directory=source.parent;park=directory.with_name('parked-documents');directory.rename(park);directory.symlink_to(park,target_is_directory=True)
        elif name=='hardlink':os.link(backends['alice']['workspace']/'unshared.txt',source)
        elif name=='fifo':os.mkfifo(source)
        elif name=='directory':source.mkdir()
        elif name=='utf8':source.write_bytes(b'\xff')
        elif name=='nul':source.write_bytes(b'a\0b')
        else:source.write_bytes(b'x\n'+b'y'*(262144-(2 if name=='exact-budget' else 1)))
        identity,_,value=submit('alice','unsafe-'+name,'tenant-documents/notes.txt',native=True,limit=1)
        if name=='exact-budget':
            assert value['receipt']['state']=='completed',value
            output=json.loads(json.loads(value['receipt']['result']['reply'].split(': ',1)[1]));assert output['content']=='x' and output['size_bytes']==262144 and output['truncated'] is True,output
        else:
            assert value['receipt']['state']=='completed' and '\"effect_status\":\"no_effect\"' in value['receipt']['result']['reply'] and '\"error\"' in value['receipt']['result']['reply'],value
            assert 'DO-NOT-DISCLOSE' not in value['receipt']['result']['reply']
        if name=='parent-link':source.parent.unlink();park.rename(source.parent);source.write_bytes(original_bytes)
        elif name=='directory':source.rmdir();source.write_bytes(original_bytes)
        else:source.unlink();source.write_bytes(original_bytes)
    print('PASS tenant file 4: links/unsafe parents/FIFO/directory/UTF8/NUL/oversize reject, exact 256KiB full-source-before-slice accepts',flush=True)
    # Real browser same model/tool/SQLite fixture, if explicitly requested.
    if '--browser' in sys.argv:
        planned('alice','browser-file','tenant-documents/notes.txt')
        browser=root/'browser.json';browser.write_text(json.dumps({'base':f'http://127.0.0.1:{gp}',
            'alice':users['alice']['token'],'bob':users['bob']['token'],'readonly':readonly['token'],
            'original':identities['alice'][0]}))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_file_read_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90)
        assert r.returncode==0,r.returncode
    # Withdraw paired grant; facts remain old identity GET-only, new reads fail.
    stop(process);a=backends['alice'];stop(a['process']);a['config']['http'].pop('tenant_file_read');a['path'].write_text(json.dumps(a['config']))
    gateway['backends'][0].pop('tenant_file_read');config_path.write_text(json.dumps(gateway))
    a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
    assert 'file_read' not in api('alice','/api/turns/capabilities')
    before=len(posts);identity,original,value=identities['alice']
    assert api('alice','/api/turns/'+identity,'PUT',original)['receipt']['request_hash']==value['receipt']['request_hash']
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','withdrawn'),400)
    assert len(posts)==before
    cli(config_path,'key-revoke','--key',users['alice']['key_id'])
    api('alice','/api/turns/'+identity,expected=401)
    assert len(posts)==before
    print('PASS tenant file 5: paired grant withdrawal preserves original fact/hash, no redispatch, new reads/refreshed revoked Key reject',flush=True)
    # Two concurrent original requests while the first real model owner is held.
    prompt='concurrent-file';planned('bob',prompt,'tenant-documents/notes.txt')
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob',prompt)]=gate
    identity=str(uuid.uuid4());original=body('bob',prompt)
    api('bob','/api/turns/'+identity,'PUT',original,202);assert gate['ready'].wait(5)
    api('bob','/api/turns/'+str(uuid.uuid4()),'PUT',body('bob','busy-file'),429)
    api('bob','/api/turns/'+identity,'PUT',original)
    assert count(prompt)==1
    api('bob','/api/turns/'+identity+'/cancel','POST');gate['release'].set()
    value=complete('bob',identity);assert value['receipt']['cancel_requested'] and value['receipt']['state']=='needs_review',value
    assert count(prompt)==1
    print('PASS tenant file 6: actual held model, duplicate GET-only/concurrent refusal and persisted cancellation stop future file dispatch',flush=True)
    # Eight actual reads outlive their timed-out public tool waiters. A ninth
    # cannot obtain the original shared I/O permit, and no file body is returned.
    b=backends['bob'];stop(b['process'])
    lib=root/('hold.dylib' if sys.platform=='darwin' else 'hold.so')
    compile_args=['cc','-dynamiclib' if sys.platform=='darwin' else '-shared','-std=c11','-O2',
        '-fPIC',str(Path(__file__).with_name('tenant_file_read_hold.c')),'-o',str(lib)]
    if sys.platform!='darwin':compile_args.append('-ldl')
    subprocess.run(compile_args,check=True,capture_output=True,timeout=30)
    arm=root/'arm';marker=root/'held';release=root/'release'
    injected=dict(env,JIACLAW_TEST_FILE_SOURCE=str(b['source']),JIACLAW_TEST_FILE_ARM=str(arm),
        JIACLAW_TEST_FILE_MARKER=str(marker),JIACLAW_TEST_FILE_RELEASE=str(release))
    injected['DYLD_INSERT_LIBRARIES' if sys.platform=='darwin' else 'LD_PRELOAD']=str(lib)
    b['process']=launch(['serve','--config',str(b['path'])],b['port'],b['log'],injected)
    arm.write_text('arm')
    try:
        for n in range(1,9):
            prompt='held-read-'+str(n);planned('bob',prompt,'tenant-documents/notes.txt')
            identity=str(uuid.uuid4());api('bob','/api/turns/'+identity,'PUT',body('bob',prompt),202,native=True)
            wait(lambda:(root/('held-'+str(n))).exists(),'actual source read held',5)
            api('bob','/api/turns/'+identity+'/cancel','POST',native=True)
            value=complete('bob',identity,True)
            assert value['receipt']['state']=='needs_review' and value['receipt']['cancel_requested'] and count(prompt)==1,value
        prompt='ninth-read';_,_,value=submit('bob',prompt,'tenant-documents/notes.txt',native=True)
        assert 'I/O capacity busy' in value['receipt']['result']['reply'] and 'no_effect' in value['receipt']['result']['reply'],value
        assert len(list(root.glob('held-*')))==8
        release.write_text('settle')
        deadline=time.monotonic()+5
        while True:
            _,_,value=submit('bob','after-release-'+uuid.uuid4().hex,'tenant-documents/notes.txt',native=True)
            if 'bob-PRIVATE-source' in value['receipt']['result']['reply']:break
            assert time.monotonic()<deadline,'actual read owner failed to release its I/O capacity'
            time.sleep(.02)
    finally:
        release.write_text('settle')
        # Keep the release capability alive until the injected process actually
        # exits; deleting the temporary tree first would leave its readers held.
        stop(b['process'])
    print('PASS tenant file 7: eight actual source owners retain I/O capacity after explicit cancel/tool-waiter timeout; ninth refuses until real release, no replay',flush=True)
    assert not faults,faults
finally:
    for gate in gates.values():gate['release'].set()
    for p in processes:stop(p)
    if relay:relay.close()
    model.shutdown();model.server_close()
