#!/usr/bin/env python3
"""Actual tenant tasks use the original bounded literal content scanner; no supplier credentials."""
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


def call(path, pattern='Needle', **extra):
    return {'id': 'grep-'+uuid.uuid4().hex, 'type': 'function', 'function': {
        'name': 'grep', 'arguments': json.dumps(dict(path=path, pattern=pattern, **extra))}}


def cli(config, *args):
    r = subprocess.run([str(binary), 'gateway', *args, '--config', str(config)],
        env=env, capture_output=True, text=True, timeout=15)
    assert r.returncode == 0, (r.stdout, r.stderr)
    return json.loads(r.stdout)


try:
 with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-grep-') as temporary:
    root = Path(temporary); gp = port(); cfg = root/'gateway.json'; registry = root/'registry/users.sqlite3'
    gateway = dict(bind=f'127.0.0.1:{gp}', registry_path=str(registry), request_timeout_seconds=15,
        max_in_flight=4, tracked_turns=True, backends=[])
    backends = {}
    for who in ('alice', 'bob'):
        d = root/who; d.mkdir(mode=0o700); ws = d/'workspace'; ws.mkdir(mode=0o700)
        directory = 'tenant-documents/'+who; grep = ws/directory; grep.mkdir(parents=True)
        (grep/'nested').mkdir(); (grep/'nested'/'child.txt').write_text('heading\n  Needle literal.*\n尾部 Needle\n')
        (grep/'report.bin').write_bytes(b'\xff\0x'); (grep/'.hidden').write_text('Needle hidden'); (grep/'oversized').write_text('Needle'*50000)
        (ws/'secret').write_text('OUTSIDE-PRIVATE-'+who)
        (grep/'link').symlink_to(ws/'secret'); os.link(ws/'secret', grep/'hardlink'); os.mkfifo(grep/'fifo')
        policy = {'roots': [directory]}; token = 'grep-fixture-'+uuid.uuid4().hex
        token_file = d/'token'; token_file.write_text(token); token_file.chmod(0o600); number = port()
        config = {'agent': {'name': who, 'description': 'Explicit directory task', 'workspace_path': str(ws),
            'system_instructions': 'Use only selected tools.', 'max_turns': 10, 'tool_timeout_secs': 2},
            'tools': {'grep': {'enabled': True}},
            'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{model.server_port}',
                'api_key': keys[who], 'model': 'fixture-grep'},
            'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
            'http': {'bind': f'127.0.0.1:{number}', 'persist': True, 'persist_path': '../state/sessions.sqlite3',
                'api_token': token, 'tracked_turns': True, 'tracked_turn_timeout_secs': 10,
                'shutdown_timeout_secs': 2, 'gateway_channel_chat': True, 'tenant_grep': policy}}
        path = d/'config.json'; path.write_text(json.dumps(config))
        b = dict(path=path, config=config, port=number, token=token, workspace=ws, directory=directory, grep=grep,
            policy=policy, log=d/'serve.log'); backends[who] = b
        b['process'] = launch(['serve', '--config', str(path)], number, b['log'])
        gateway['backends'].append(dict(id=who, url=f'http://127.0.0.1:{number}/', token_file=str(token_file), tenant_grep=policy))
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
        tools = tools or ['grep']; v = dict(session_id='http:'+str(uuid.uuid4()), prompt=prompt, enabled_tools=tools, enabled_skills=[])
        if 'grep' in tools: v['grep_policy_sha256'] = caps[who]['grep']['policy_sha256']
        return v
    def complete(who, identity, native=False):
        return wait(lambda: (lambda v: v if v['receipt']['state'] != 'running' and not v['active'] else None)(
            api(who, '/api/turns/'+identity, native=native)), 'original grep receipt')
    def settled(who):
        wait(lambda: not sql(registry, 'SELECT * FROM write_holds WHERE user_id=?', (users[who]['user_id'],)), 'original grep gateway hold')
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
        f = caps[who]['grep']; canonical = [1, backends[who]['policy']['roots'],512,128,262144,16777216,200,200,32,2000,2000,65536]
        assert f == dict(protocol=1, roots=canonical[1], max_pattern_bytes=512,max_glob_bytes=128,max_file_bytes=262144,max_read_bytes=16777216,max_matches=200,max_snippet_chars=200,max_depth=32,max_scanned=2000,
            scan_budget_ms=2000,max_result_bytes=65536,policy_sha256=hashlib.sha256(json.dumps(canonical,separators=(',',':')).encode()).hexdigest())
        assert caps[who]['enabled_tools'] == ['datetime_now','json_query','grep']
        assert all(k not in caps[who] for k in ('glob','tree','file_read','memory_search'))
    stop(process); wrong = json.loads(json.dumps(gateway)); wrong['backends'][0]['tenant_grep']['roots']=['tenant-documents/elsewhere']
    bad = root/'wrong.json'; bad.write_text(json.dumps(wrong))
    with (root/'wrong.log').open('wb') as log:
        p = subprocess.Popen([str(binary),'gateway','serve','--config',str(bad)],env=env,stdout=log,stderr=log); processes.append(p)
    assert p.wait(timeout=15) == 1
    process = launch(['gateway','serve','--config',str(cfg)],gp,root/'gateway.log')
    print('PASS tenant grep 1: exact paired default-off permission and canonical original budgets; mismatched startup refuses',flush=True)
    identities = {}; actual = []
    for who,b in backends.items():
        identity, original, v = submit(who, 'grep-success-'+who, [call(b['directory'],max_matches=200)])
        assert v['receipt']['state']=='completed' and v['receipt']['result']['tool_names']==['grep'],v
        result = output(v); matches = result['matches']
        assert result['path']==b['directory'] and result['pattern']=='Needle' and result['truncated'] is False
        assert matches==[dict(path=b['directory']+'/.hidden',line=1,snippet='Needle hidden'),dict(path=b['directory']+'/nested/child.txt',line=2,snippet='  Needle literal.*'),dict(path=b['directory']+'/nested/child.txt',line=3,snippet='尾部 Needle')],matches
        assert result['match_count']==3
        assert 'OUTSIDE-PRIVATE' not in json.dumps(result) and 'oversized' not in json.dumps(result) and 'report.bin' not in json.dumps(result)
        assert count('grep-success-'+who)==2
        with lock: initial=next(p for p in posts if p['prompt']=='grep-success-'+who)['body']
        schema=initial['tools'][0]['function']['parameters'];assert schema['properties']['path']['enum']==[b['directory']]
        assert schema['required']==['path','pattern'] and schema['additionalProperties'] is False
        identities[who]=(identity,original,v)
        actual.append(dict(user=who,turn_id=identity,root=b['directory'],policy_sha256=caps[who]['grep']['policy_sha256'],
            model_posts=count('grep-success-'+who),tool_names=v['receipt']['result']['tool_names'],match_count=len(matches),
            binary_skipped=all(m['path']!=b['directory']+'/report.bin' for m in matches),hidden_body=matches[0]['snippet']=='Needle hidden',private_body_in_result='OUTSIDE-PRIVATE' in json.dumps(result),
            result_sha256=hashlib.sha256(json.dumps(result,sort_keys=True,separators=(',',':')).encode()).hexdigest()))
    api('bob','/api/turns/'+identities['alice'][0],expected=404)
    api('alice','/api/turns/'+identities['alice'][0],token=readonly['token'])
    print('PASS tenant grep 2: actual two-user model/tool/receipt roundtrip returns exact literal lines and hidden body; skips binary, oversized, links and special entries',flush=True)
    before=len(posts)
    for v in [dict(body('alice','missing'),grep_policy_sha256=None),dict(body('alice','stale'),grep_policy_sha256='a'*64),
        dict(body('alice','duplicate'),enabled_tools=['grep','grep']),dict(body('alice','extra',['datetime_now']),grep_policy_sha256='a'*64)]:
        for native in [False,True]:
            identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',v,400,native=native)
            api('alice','/api/turns/'+identity,expected=404,native=native)
    api('alice','/api/turns/'+str(uuid.uuid4()),'PUT',body('alice','readonly'),403,token=readonly['token'])
    assert len(posts)==before
    for path,extra in [('.',{}),(backends['bob']['directory'],{}),('tenant-documents/alice/',{}),
        ('tenant-documents/alice/nested',{}),('tenant-documents/alice',{'max_matches':201}),('tenant-documents/alice',{'extra':True})]:
        prompt='grep-invalid-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(path,**extra)],native=True)
        assert v['receipt']['state']=='needs_review' and count(prompt)==1 and v['receipt']['result'] is None,v
    print('PASS tenant grep 3: missing/stale/unselected/duplicate permission, readonly and out-of-scope native batch rejected before dispatch',flush=True)
    a=backends['alice']; original_grep=a['grep']; parked=original_grep.with_name('parked')
    original_grep.rename(parked); original_grep.symlink_to(parked,target_is_directory=True)
    _,_,v=submit('alice','grep-link-parent',[call(a['directory'])],native=True)
    assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count('grep-link-parent')==2,v
    original_grep.unlink(); parked.rename(original_grep)
    original_grep.rename(parked);original_grep.write_bytes(b'ordinary file cannot become a directory grant')
    _,_,v=submit('alice','grep-file-root',[call(a['directory'])],native=True)
    assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count('grep-file-root')==2,v
    original_grep.unlink();parked.rename(original_grep)
    expected=[dict(path=a['directory']+'/nested/child.txt',line=2,snippet='  Needle literal.*')]
    for pattern,options,matches in [('  Needle literal.*',{'glob':'*.txt'},expected), ('  Needle literal.*',{'glob':a['directory']+'/**/*.txt'},expected), ('x'*512,{},[])]:
        prompt='grep-pattern-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(a['directory'],pattern=pattern,**options)])
        assert output(v)['matches']==matches and count(prompt)==2,(pattern,v)
    for pattern in ['界'*171,'Needle\0']:
        prompt='grep-raw-pattern-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(a['directory'],pattern=pattern)],native=True)
        assert v['receipt']['state']=='completed' and 'no_effect' in v['receipt']['result']['reply'] and count(prompt)==2,v
    prompt='grep-schema-pattern-'+uuid.uuid4().hex;_,_,v=submit('alice',prompt,[call(a['directory'],pattern='x'*513)],native=True)
    assert v['receipt']['state']=='needs_review' and v['receipt']['result'] is None and count(prompt)==1,v
    _,_,v=submit('alice','grep-limited',[call(a['directory'],pattern='needle',case_insensitive=True,max_matches=1)])
    assert output(v)['truncated'] is True and len(output(v)['matches'])==1
    print('PASS tenant grep 4: real linked/file roots refuse as NoEffect; literal whitespace, filename/full-path filter, raw UTF8 boundary and original truncation hold',flush=True)
    stop(a['process']); changed=json.loads(json.dumps(a['config']));changed['http']['tenant_grep']['roots']=['tenant-documents/changed']
    a['path'].write_text(json.dumps(changed));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    before=len(posts); identity=str(uuid.uuid4());api('alice','/api/turns/'+identity,'PUT',body('alice','changed-backend'),503)
    api('alice','/api/turns/'+identity,expected=404);assert len(posts)==before
    assert not sql(registry,'SELECT * FROM write_holds WHERE user_id=?',(users['alice']['user_id'],))
    stop(a['process']);a['path'].write_text(json.dumps(a['config']));a['process']=launch(['serve','--config',str(a['path'])],a['port'],a['log'])
    gate={'ready':threading.Event(),'release':threading.Event()};gates[('bob','grep-cancel')]=gate
    tool_plans[('bob','grep-cancel')]=[call(backends['bob']['directory'])];identity=str(uuid.uuid4())
    api('bob','/api/turns/'+identity,'PUT',body('bob','grep-cancel'),202,native=True);assert gate['ready'].wait(5)
    api('bob','/api/turns/'+identity+'/cancel','POST',{},200,native=True);gate['release'].set()
    wait(lambda:not api('bob','/api/turns/'+identity,native=True)['active'],'real cancelled owner settles')
    assert count('grep-cancel')==1
    print('PASS tenant grep 5: changed backend permission rejects before gateway identity/hold; real cancellation prevents future grep dispatch',flush=True)
    if '--browser' in sys.argv:
        tool_plans[('alice','browser-grep')]=[call(a['directory'])]
        browser=root/'browser.json';browser.write_text(json.dumps(dict(base=f'http://127.0.0.1:{gp}',alice=users['alice']['token'],bob=users['bob']['token'],readonly=readonly['token'])))
        r=subprocess.run(['node',str(Path(__file__).with_name('tenant_grep_browser.cjs')),str(browser)],env=os.environ.copy(),timeout=90)
        assert r.returncode==0,r.returncode
    cli(cfg,'key-revoke','--key',users['alice']['key_id'])
    api('alice','/api/turns/'+identities['alice'][0],expected=401)
    print('PASS tenant grep 6: revoked personal Key cannot read old private facts; no automatic resubmission',flush=True)
    print('ACTUAL tenant grep task receipts '+json.dumps(actual,separators=(',',':')),flush=True)
    print('ALL tenant grep acceptance passed',flush=True)
finally:
 for p in processes:
    if p.poll() is None: stop(p)
 model.shutdown()
