#!/usr/bin/env python3
"""Actual tenant tasks use the original bounded filename scanner; no supplier credentials."""
import hashlib
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


def call(path, pattern='*', **extra):
    return {'id': 'glob-'+uuid.uuid4().hex, 'type': 'function', 'function': {
        'name': 'glob', 'arguments': json.dumps(dict(path=path, pattern=pattern, **extra))}}


def cli(config, *args):
    r = subprocess.run([str(binary), 'gateway', *args, '--config', str(config)],
        env=env, capture_output=True, text=True, timeout=15)
    assert r.returncode == 0, (r.stdout, r.stderr)
    return json.loads(r.stdout)


try:
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-glob-') as temporary:
    root = Path(temporary); gp = port(); cfg = root/'gateway.json'; registry = root/'registry/users.sqlite3'
    gateway = dict(bind=f'127.0.0.1:{gp}', registry_path=str(registry), request_timeout_seconds=15,
        max_in_flight=4, tracked_turns=True, backends=[])
    backends = {}
    for who in ('alice', 'bob'):
        d = root/who; d.mkdir(mode=0o700); ws = d/'workspace'; ws.mkdir(mode=0o700)
        directory = 'tenant-documents/'+who; glob = ws/directory; glob.mkdir(parents=True)
        (glob/'nested').mkdir(); (glob/'nested'/'child.txt').write_text('BODY-MUST-NOT-LEAK')
        (glob/'report.bin').write_bytes(b'\xff\0x'); (glob/'.hidden').write_text('hidden body')
        (ws/'secret').write_text('OUTSIDE-PRIVATE-'+who)
        (glob/'link').symlink_to(ws/'secret'); os.link(ws/'secret', glob/'hardlink'); os.mkfifo(glob/'fifo')
        policy = {'roots': [directory]}; token = 'glob-fixture-'+uuid.uuid4().hex
        token_file = d/'token'; token_file.write_text(token); token_file.chmod(0o600); number = port()
        config = {'agent': {'name': who, 'description': 'Explicit directory task', 'workspace_path': str(ws),
            'system_instructions': 'Use only selected tools.', 'max_turns': 10, 'tool_timeout_secs': 2},
            'tools': {'glob': {'enabled': True}},
            'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{model.server_port}',
                'api_key': keys[who], 'model': 'fixture-glob'},
            'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
            'http': {'bind': f'127.0.0.1:{number}', 'persist': True, 'persist_path': '../state/sessions.sqlite3',
                'api_token': token, 'tracked_turns': True, 'tracked_turn_timeout_secs': 10,
                'shutdown_timeout_secs': 2, 'gateway_channel_chat': True, 'tenant_glob': policy}}
        path = d/'config.json'; path.write_text(json.dumps(config))
        b = dict(path=path, config=config, port=number, token=token, workspace=ws, directory=directory, glob=glob,
            policy=policy, log=d/'serve.log'); backends[who] = b
        b['process'] = launch(['serve', '--config', str(path)], number, b['log'])
        gateway['backends'].append(dict(id=who, url=f'http://127.0.0.1:{number}/', token_file=str(token_file), tenant_glob=policy))
    cfg.write_text(json.dumps(gateway)); users = {w: cli(cfg, 'user-add', '--backend', w) for w in backends}
    readonly = cli(cfg, 'key-add', '--user', users['alice']['user_id'], '--read-only')
    process = launch(['gateway', 'serve', '--config', str(cfg)], gp, root/'gateway.log')
    def api(who, path, method='GET', body=None, expected=200, native=False, token=None):
        b = backends[who]; code, value, headers = request(b['port'] if native else gp, path,
            b['token'] if native else (token or users[who]['token']), method, body, marker=native)
        assert code == expected, (who, path, code, value)
        if not native:
            assert headers.get('Cache-Control') == 'no-store' and headers.get('X-Content-Type-Options') == 'nosniff'
        return value
    caps = {w: api(w, '/api/turns/capabilities') for w in backends}
    def body(who, prompt, tools=None):
        tools = tools or ['glob']; v = dict(session_id='http:'+str(uuid.uuid4()), prompt=prompt, enabled_tools=tools, enabled_skills=[])
        if 'glob' in tools: v['glob_policy_sha256'] = caps[who]['glob']['policy_sha256']
        return v
    def complete(who, identity, native=False):
        return wait(lambda: (lambda v: v if v['receipt']['state'] != 'running' and not v['active'] else None)(
            api(who, '/api/turns/'+identity, native=native)), 'original glob receipt')
    def settled(who):
        wait(lambda: not sql(registry, 'SELECT * FROM write_holds WHERE user_id=?', (users[who]['user_id'],)), 'original glob gateway hold')
    def count(prompt):
        with lock: return len([p for p in posts if p['prompt'] == prompt])
    def submit(who, prompt, calls, native=False):
        tool_plans[(who,prompt)] = calls; identity = str(uuid.uuid4()); original = body(who,prompt)
        api(who, '/api/turns/'+identity, 'PUT', original, 202, native=native)
        if not native: settled(who)
        return identity, original, complete(who,identity,native)
    def output(value):
        decoded = json.loads(value['receipt']['result']['reply'].split(': ',1)[1])
        return json.loads(decoded) if isinstance(decoded,str) else decoded
    for who in backends:
        f = caps[who]['glob']; canonical = [1, backends[who]['policy']['roots'],256,500,32,2000,2000,65536]
        assert f == dict(protocol=1, roots=canonical[1], max_pattern_bytes=256,max_results=500,max_depth=32,max_scanned=2000,
            scan_budget_ms=2000,max_result_bytes=65536,policy_sha256=hashlib.sha256(json.dumps(canonical,separators=(',',':')).encode()).hexdigest())
        assert caps[who]['enabled_tools'] == ['datetime_now','json_query','glob']
    stop(process); wrong = json.loads(json.dumps(gateway)); wrong['backends'][0]['tenant_glob']['roots']=['tenant-documents/elsewhere']
    bad = root/'wrong.json'; bad.write_text(json.dumps(wrong))
    with (root/'wrong.log').open('wb') as log:
        p = subprocess.Popen([str(binary),'gateway','serve','--config',str(bad)],env=env,stdout=log,stderr=log); processes.append(p)
    assert p.wait(timeout=15) == 1
    process = launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    print('PASS tenant glob 1: exact paired default-off permission and canonical original budgets; mismatched startup refuses',flush=True)
    identities = {}; actual = []
    for who,b in backends.items():
        identity, original, v = submit(who, 'glob-success-'+who, [call(b['directory'],max_results=500)])
        assert v['receipt']['state']=='completed' and v['receipt']['result']['tool_names']==['glob'],v
        result = output(v); matches = result['matches']
        assert result['path']==b['directory'] and result['pattern']=='*' and result['truncated'] is False
        assert matches==[b['directory']+'/.hidden',b['directory']+'/nested/child.txt',b['directory']+'/report.bin'],matches
        assert result['match_count']==3
        assert 'BODY-MUST-NOT-LEAK' not in json.dumps(result) and 'OUTSIDE-PRIVATE' not in json.dumps(result)
        assert count('glob-success-'+who)==2
        with lock: initial=next(p for p in posts if p['prompt']=='glob-success-'+who)['body']
        schema=initial['tools'][0]['function']['parameters'];assert schema['properties']['path']['enum']==[b['directory']]
        assert schema['required']==['path','pattern'] and schema['additionalProperties'] is False
        identities[who]=(identity,original,v)
        actual.append(dict(user=who,turn_id=identity,root=b['directory'],policy_sha256=caps[who]['glob']['policy_sha256'],
            model_posts=count('glob-success-'+who),tool_names=v['receipt']['result']['tool_names'],match_count=len(matches),
            binary_name=b['directory']+'/report.bin' in matches,hidden_name=b['directory']+'/.hidden' in matches,private_body_in_result='OUTSIDE-PRIVATE' in json.dumps(result),
            result_sha256=hashlib.sha256(json.dumps(result,sort_keys=True,separators=(',',':')).encode()).hexdigest()))
    api('bob','/api/turns/'+identities['alice'][0],expected=404)
    api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    print('PASS tenant glob 2: actual two-user model/tool/receipt roundtrip lists binary and hidden filenames; skips links and special entries without body reads',flush=True)
    before=len(posts)
    for v in [dict(body('alice','missing'),glob_policy_sha256=None),dict(body('alice','stale'),glob_policy_sha256='a'*64),
        dict(body('alice','duplicate'),enabled_tools=['glob','glob']),dict(body('alice','extra',['datetime_now']),glob_policy_sha256='a'*64)]:
        for native in [False,True]:
            identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400,native=native)
            api('alice','/api/turns/'+identity,expected=404,native=native)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token'])
    assert len(posts)==before
    for path,extra in [('.',{}),(backends['bob']['directory'],{}),('tenant-documents/alice/',{}),
        ('tenant-documents/alice/nested',{}),('tenant-documents/alice',{'max_results':501}),('tenant-documents/alice',{'extra':True})]:
        prompt='glob-invalid-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(path,**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1 and v['receipt']['result'] is None,v
    print('PASS tenant glob 3: missing/stale/unselected/duplicate permission, readonly and out-of-scope native batch rejected before dispatch',flush=True)
    a=backends['alice']; original_glob=a['glob']; parked=original_glob.with_name('parked')
    original_glob.rename(parked); original_glob.symlink_to(parked,target_is_directory=True)
    _,_,v=submit('alice','glob-link-parent',[call(a['directory'])],native=True)
    assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count('glob-link-parent')==2,v
    original_glob.unlink(); parked.rename(original_glob)
    original_glob.rename(parked);original_glob.write_bytes(b'ordinary file cannot become a directory grant')
    _,_,v=submit('alice','glob-file-root',[call(a['directory'])],native=True)
    assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count('glob-file-root')==2,v
    original_glob.unlink();parked.rename(original_glob)
    for pattern,expected in [('*.txt',[a['directory']+'/nested/child.txt']), (a['directory']+'/**/*.txt',[a['directory']+'/nested/child.txt']), ('x'*256,[])]:
        prompt='glob-pattern-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(a['directory'],pattern=pattern)])
        assert output(v)['matches']==expected and count(prompt)==2,(pattern,v)
    for pattern in ['界'*86,'*\0']:
        prompt='glob-raw-pattern-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(a['directory'],pattern=pattern)],native=True)
        assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count(prompt)==2,v
    prompt='glob-schema-pattern-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(a['directory'],pattern=' '*256+'*')],native=True)
    assert v['receipt']['state']=='needs_review' and v['receipt']['result'] is None and count(prompt)==1,v
    _,_,v=submit('alice','glob-limited',[call(a['directory'],max_results=1)])
    assert output(v)['truncated'] is True and len(output(v)['matches'])==1
    print('PASS tenant glob 4: real linked/file roots refuse as NoEffect; filename/full-path patterns, raw UTF8 boundary and original truncation hold',flush=True)
    stop(a['process']); changed=json.loads(json.dumps(a['config']));changed['http']['tenant_glob']['roots']=['tenant-documents/changed']
    a['path'].write_text(json.dumps(changed));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    before=len(posts); identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','changed-backend'),503)
    api('alice','/api/turns/'+identity,expected=404);assert len(posts)==before
    assert not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users['alice']['user_id'],))
    stop(a['process']);a['path'].write_text(json.dumps(a['config']));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob','glob-cancel')]=gate
    tool_plans[('bob','glob-cancel')]=[call(backends['bob']['directory'])];identity=str(uuid.uuid4())
    api('bob','/api/turns/'+identity,'PUT',body('bob','glob-cancel'),202,native=True);assert gate['ready'].wait(5)
    api('bob','/api/turns/'+identity+'/cancel','POST',{},200,native=True);gate['release'].set()
    wait(lambda:not api('bob','/api/turns/'+identity,native=True)['active'],'real cancelled owner settles')
    assert count('glob-cancel')==1
    print('PASS tenant glob 5: changed backend permission rejects before gateway identity/hold; real cancellation prevents future glob dispatch',flush=True)
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-glob')]=[call(a['directory'])]
        browser=root/'browser.json';browser.write_text(json.dumps(dict(base=f'http://127.0.0.1:{gp}',alice=users['alice']['token'],bob=users['bob']['token'],readonly=readonly['token'])))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_glob_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90)
        assert r.returncode==0,r.returncode
    cli(cfg,'key-revoke','--key',users['alice']['key_id'])
    api('alice','/api/turns/'+identities['alice'][0],expected=401)
    print('PASS tenant glob 6: revoked personal Key cannot read old private facts; no automatic resubmission',flush=True)
    print('ACTUAL tenant glob task receipts '+json.dumps(actual,separators=(',',':')),flush=True)
    print('ALL tenant glob acceptance passed',flush=True)
finally:
 for p in processes:
    if p.poll() is None: stop(p)
 model.shutdown()
