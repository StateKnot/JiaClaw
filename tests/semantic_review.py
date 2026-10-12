#!/usr/bin/env python3
"""Actual opt-in HTTP semantic inspection using the live owner; local setup gateway only."""
import copy
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import signal
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from host_log import read_running_log

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
parent_mode = '--frozen-parent' in sys.argv
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
secret = 'semantic-review-fixture-' + uuid.uuid4().hex
calls, failures = [], []
mutex = threading.Lock()
hold_post = threading.Event()
release_post = threading.Event()
behavior = None

class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_GET(self):
        with mutex: calls.append(('GET', self.path))
        self.send_error(500)
    def do_POST(self):
        global behavior
        try:
            with mutex: calls.append(('POST', self.path))
            assert self.path == '/v1/embeddings'
            assert self.headers['Authorization'] == 'Bearer ' + secret
            assert self.headers['Idempotency-Key']
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert body['model'] == 'fixture-embedding' and body['dimensions'] == 2
            assert 1 <= len(body['input']) <= 8 and body['encoding_format'] == 'float'
            if behavior == 'hold':
                behavior = None; hold_post.set()
                assert release_post.wait(10)
                self.close_connection = True
                return
            payload = json.dumps({'object': 'list', 'model': body['model'],
                'data': [{'object': 'embedding', 'index': i, 'embedding': [1.0, 0.0]}
                         for i in range(len(body['input']))],
                'usage': {'prompt_tokens': len(body['input']), 'total_tokens': len(body['input'])}}).encode()
            self.send_response(200); self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.send_header('x-brokerrouter-request-id', str(uuid.uuid4()))
            self.end_headers(); self.wfile.write(payload)
        except Exception as error:
            with mutex: failures.append(repr(error))
            self.send_error(500)

gateway = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
gateway.daemon_threads = True
threading.Thread(target=gateway.serve_forever, daemon=True).start()
servers = []

def count():
    with mutex:
        assert not failures, failures
        return len(calls)

def cli(case, action, fail=None):
    result = subprocess.run([str(binary), 'memory', 'semantic', action, '--config', str(case[2])],
                            env=env, capture_output=True, text=True, timeout=25)
    assert secret not in result.stdout + result.stderr
    if fail:
        assert result.returncode != 0 and fail in result.stdout + result.stderr, result.stderr
        return result
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)

def rows(case):
    db = case[0] / 'state/semantic/index.sqlite3'
    script = """import json,pathlib,sqlite3,sys
connection=sqlite3.connect(pathlib.Path(sys.argv[1]).as_uri()+'?mode=rw',uri=True,timeout=2)
try:
    connection.execute('PRAGMA query_only=ON')
    assert connection.execute('PRAGMA query_only').fetchone()==(1,)
    data=[[list(row) for row in connection.execute('SELECT * FROM '+table+' ORDER BY 1')]
          for table in ('semantic_meta','generation','operations')]
    print(json.dumps(data,default=lambda value:{'blob_hex':value.hex()}))
finally:
    connection.close()
"""
    result=subprocess.run([sys.executable,'-c',script,str(db)],env=env,capture_output=True,text=True,timeout=5)
    assert result.returncode==0,result.stderr
    return json.loads(result.stdout)

def start(case, injected=None):
    case[2].write_text(json.dumps(case[3]))
    log = open(case[0] / ('serve-' + uuid.uuid4().hex + '.log'), 'wb')
    child = subprocess.Popen([str(binary), 'serve', '--config', str(case[2])],
                             env=injected or env, stdout=log, stderr=log)
    servers.append((child, log))
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        text = read_running_log(Path(log.name))
        match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:\d+)', text)
        if match: return child, log, match[1]
        assert child.poll() is None, text
        time.sleep(.02)
    raise AssertionError('startup deadline')

def stop(child, log, killed=False):
    if child.poll() is None:
        child.send_signal(signal.SIGKILL if killed else signal.SIGTERM)
        child.wait(timeout=8)
    assert child.returncode == (-9 if killed else 0), read_running_log(Path(log.name))
    log.close()

def request(base, key, path='/api/memory/semantic', method='GET'):
    headers = {'Authorization': 'Bearer ' + key} if key is not None else {}
    r = urllib.request.Request(base + path, headers=headers, method=method)
    try: response = urllib.request.urlopen(r, timeout=5)
    except urllib.error.HTTPError as error: response = error
    with response:
        body = response.read()
        if not parent_mode:
            assert response.headers.get('Cache-Control') == 'no-store'
            assert response.headers.get('X-Content-Type-Options') == 'nosniff'
            assert response.headers.get('Referrer-Policy') == 'no-referrer'
        return response.status, body

def checked(base, token, freshness=None, ready=None, status=200):
    before = count(); code, raw = request(base, token)
    assert count() == before, 'semantic review sent a remote request'
    assert code == status, (code, raw)
    assert secret.encode() not in raw and b'private-source-text' not in raw
    value = json.loads(raw)
    if status == 200:
        assert value['protocol_version'] == 1 and value['local_only'] is True and value['runtime_applied'] is False
        assert value['freshness'] == freshness and value['search_ready_at_capture'] is ready
        assert not {'text', 'excerpt', 'vectors', 'credential', 'request_body'} & value.keys()
    return value

def read_control_header(contender, deadline):
    """Read one bounded wire header, consuming the existing absolute deadline."""
    reply = b''
    while not reply.endswith(b'\r\n\r\n'):
        remaining = deadline - time.monotonic()
        assert remaining > 0, 'memory body contender admission exceeded original observation deadline'
        contender.settimeout(remaining)
        try:
            part = contender.recv(1)
        except socket.timeout as error:
            raise AssertionError('memory body contender admission exceeded original observation deadline') from error
        assert part, 'memory body contender closed before admission receipt'
        reply += part
        assert len(reply) <= 4096, 'memory body contender response exceeded header budget'
    assert time.monotonic() < deadline, 'memory body contender admission exceeded original observation deadline'
    return reply

def expect_body_admission(contender, deadline):
    """Native Hyper Continue follows Body polling, after the control permit.

    Headers sent is not admission evidence. The fifth-request refusal below
    additionally proves this marker cannot be obtained without a permit.
    """
    assert read_control_header(contender, deadline) == b'HTTP/1.1 100 Continue\r\n\r\n', 'memory body contender was refused before admission'

with tempfile.TemporaryDirectory(prefix='jiaclaw-semantic-review-') as temporary:
    root = Path(temporary).resolve()
    def fixture(name):
        directory = root / name; workspace = directory / 'workspace'
        workspace.mkdir(parents=True, mode=0o700)
        (workspace / 'MEMORY.md').write_text('private-source-text tea\n')
        token = uuid.uuid4().hex
        config = {'agent': {'name':'semantic-review','description':'Disposable local fixture',
            'system_instructions':'Local fixture only.','max_turns':10,'workspace_path':str(workspace)},
            'provider':{'provider_type':'brokerrouter','base_url':'http://127.0.0.1:'+str(gateway.server_port),
                        'api_key':secret,'model':'must-not-chat'},
            'memory':{'path':'MEMORY.md','semantic':{'enabled':True,'model':'fixture-embedding',
                        'space_revision':'fixture-v1','dimensions':2,'timeout_secs':2,
                        'index_path':'../state/semantic/index.sqlite3'}},
            'http':{'bind':'127.0.0.1:0','api_token':token,'memory_review':True,'memory_edit':True,
                    'semantic_review':True,'persist':False,'shutdown_timeout_secs':1}}
        path = directory / 'config.json'; path.write_text(json.dumps(config))
        return directory, workspace, path, config, token
    try:
        case = fixture('main')
        if parent_mode:
            child, log, base = start(case)
            assert request(base, case[4], '/api/memory/semantic-capabilities')[0] == 404
            assert request(base, case[4])[0] == 404
            stop(child, log)
            assert count() == 0
            print('PASS frozen parent semantic review: both new routes absent; original Agent startup separate; no model request')
            sys.exit(0)
        child, log, base = start(case)
        for key in [None, 'wrong']:
            assert request(base, key)[0] == 401
        for path in ['/api/memory/semantic', '/api/memory/semantic-capabilities']:
            for method in ['POST', 'PUT', 'DELETE', 'OPTIONS']:
                assert request(base, case[4], path, method)[0] == 405
        code, raw = request(base, case[4], '/api/memory/semantic-capabilities')
        assert code == 200 and json.loads(raw)['operation'] == 'semantic-check'
        before = rows(case); checked(base, case[4], 'missing', False); assert rows(case) == before
        cli(case, 'check', fail='another process owns semantic state')
        stop(child, log)
        print('PASS semantic review 1: separate administrator capability, unauthorized/method refusal, private headers and original live store owner; no network')

        cli(case, 'refresh'); child, log, base = start(case)
        before = rows(case); checked(base, case[4], 'current', True); assert rows(case) == before
        old = os.stat(case[1] / 'MEMORY.md'); content = (case[1] / 'MEMORY.md').read_bytes()
        (case[1] / 'MEMORY.md').write_bytes(content.replace(b'tea', b'ink'))
        os.utime(case[1] / 'MEMORY.md', ns=(old.st_atime_ns, old.st_mtime_ns))
        checked(base, case[4], 'stale', False); assert rows(case) == before
        stop(child, log)
        case[3]['memory']['semantic']['space_revision'] = 'different-space'
        child, log, base = start(case); checked(base, case[4], 'space_mismatch', False); stop(child, log)
        print('PASS semantic review 2: current/stale/space mismatch from complete saved bytes, same size/mtime change and unchanged SQL tables')

        large = fixture('large'); large[3]['memory']['semantic']['sources'] = ['MEMORY.md','SOUL.md','USER.md']
        for name in large[3]['memory']['semantic']['sources']:
            (large[1] / name).write_bytes(('茶'*43690+'x\n').encode())
        child, log, base = start(large); value = checked(base, large[4], 'missing', False)
        assert value['source_chunks'] > 0 and len(value['sources']) == 3
        (large[1] / 'MEMORY.md').write_bytes((large[1] / 'MEMORY.md').read_bytes()+b'x')
        checked(base, large[4], status=422)
        (large[1] / 'MEMORY.md').write_bytes(b'\xff'); checked(base, large[4], status=422)
        (large[1] / 'MEMORY.md').unlink(); os.mkfifo(large[1] / 'MEMORY.md'); checked(base, large[4], status=422)
        (large[1] / 'MEMORY.md').unlink(); (large[1] / 'MEMORY.md').symlink_to('USER.md'); checked(base, large[4], status=422)
        stop(child, log)
        print('PASS semantic review 3: exact UTF8 128KiB x3 complete capture, byte overflow/invalid UTF8/FIFO/link refusal without raw text or network')

        held = fixture('held')
        lib = root / ('review-read.dylib' if sys.platform == 'darwin' else 'review-read.so')
        command = ['cc','-shared','-O2','-o',str(lib),str(Path(__file__).with_name('semantic_review_read.c'))]
        if sys.platform == 'darwin': command.insert(1,'-dynamiclib')
        else: command.extend(['-fPIC','-ldl'])
        subprocess.run(command, check=True, capture_output=True, timeout=30)
        receipts, contender_receipts = [], []
        for delay_ms, killed in [(0, False), (0, True), (400, False), (400, True)]:
            suffix = str(killed)+'-'+str(delay_ms); arm=root/('arm-'+suffix); marker=root/('read-'+suffix); release=root/('release-'+suffix)
            injected = dict(env); injected.update(JIACLAW_TEST_REVIEW_SOURCE=str(held[1]/'MEMORY.md'),
                JIACLAW_TEST_REVIEW_ARM=str(arm),JIACLAW_TEST_REVIEW_MARKER=str(marker),JIACLAW_TEST_REVIEW_RELEASE=str(release))
            injected['DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD'] = str(lib)
            child, log, base = start(held, injected); before = rows(held); arm.write_text('arm')
            port=int(base.rsplit(':',1)[1]); caller=socket.create_connection(('127.0.0.1',port),timeout=5)
            caller.sendall(('GET /api/memory/semantic HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer '+held[4]+'\r\nConnection: close\r\n\r\n').encode())
            deadline=time.monotonic()+5
            while not marker.exists() and time.monotonic()<deadline: time.sleep(.01)
            assert marker.exists(), 'actual source read was not reached'
            caller.close()
            cli(held,'check',fail='another process owns semantic state')
            slow, senders, send_errors = [], [], []
            deadline=time.monotonic()+2
            headers=('POST /api/memory/replace HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer '+held[4]+'\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n').encode()
            def send_headers(contender):
                try:
                    time.sleep(delay_ms / 1000)
                    contender.sendall(headers)
                except OSError as error:
                    send_errors.append(type(error).__name__)
            for _ in range(3):
                s=socket.create_connection(('127.0.0.1',port),timeout=5); slow.append(s)
                sender=threading.Thread(target=send_headers,args=(s,),daemon=True); sender.start(); senders.append(sender)
            for contender in slow: expect_body_admission(contender, deadline)
            for sender in senders: sender.join(max(0,deadline-time.monotonic()))
            assert not send_errors and all(not sender.is_alive() for sender in senders), send_errors
            while time.monotonic()<deadline:
                if request(base,held[4],'/api/memory')[0] == 503: break
                time.sleep(.01)
            else: raise AssertionError('disconnected read owner released control capacity before actual settlement')
            with socket.create_connection(('127.0.0.1',port),timeout=5) as fifth:
                fifth.sendall(headers)
                assert read_control_header(fifth,deadline).startswith(b'HTTP/1.1 503 '), 'busy contender received a body-poll receipt without a permit'
            assert rows(held)==before
            for s in slow: s.sendall(b'0\r\n\r\n'); s.close()
            if killed:
                stop(child,log,True); release.write_text('settle'); cli(held,'check')
            else:
                release.write_text('settle'); deadline=time.monotonic()+8
                while time.monotonic()<deadline:
                    code,_=request(base,held[4])
                    if code==200: break
                    assert code in [422,503]; time.sleep(.02)
                else: raise AssertionError('actual held owner did not settle')
                assert rows(held)==before; stop(child,log)
            receipt={'actual_source_read_held':True,'http_client_disconnected':True,'kill':killed,'exit':child.returncode}
            if delay_ms == 0: receipts.append(receipt)
            contender_receipts.append({'delay_ms':delay_ms,'kill':killed,'native_body_poll_receipts':3,'exhausted_controls':True,'fifth_refused_before_body_poll':True,'exit':child.returncode})
        print('ACTUAL semantic review read owner receipts '+json.dumps(receipts))
        print('ACTUAL semantic review contender admission receipts '+json.dumps(contender_receipts))
        print('PASS semantic review 4: actual thread-held read, disconnected HTTP and three held body owners keep all four controls; release/SIGKILL settlement')

        pending = fixture('pending'); behavior='hold'; hold_post.clear(); release_post.clear()
        child = subprocess.Popen([str(binary),'memory','semantic','refresh','--config',str(pending[2])],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert hold_post.wait(5); child.kill(); child.communicate(timeout=8); assert child.returncode==-9
        release_post.set(); time.sleep(.03)
        child,log,base=start(pending); before=rows(pending)
        value=checked(base,pending[4],'missing',False); assert value['pending']['state']=='unknown' and value['next_action']=='reconcile_pending'
        identity=value['pending']['id']; assert checked(base,pending[4],'missing',False)['pending']['id']==identity
        assert rows(pending)==before; stop(child,log)
        print('PASS semantic review 5: original submitted POST SIGKILL, ordinary Agent startup normalization separate; HTTP retains unknown identity/hold and never GET/replays')

        for index,change in enumerate(['review','semantic','token','gateway','driven']):
            bad=fixture('bad-'+change)
            if change=='review': bad[3]['http']['memory_review']=False
            if change=='semantic': bad[3]['memory']['semantic']['enabled']=False
            if change=='token': bad[3]['http']['api_token']=''
            if change=='gateway': bad[3]['http']['gateway_channel_chat']=True
            if change=='driven': bad[3]['scheduler']={'gateway_driven':True}
            bad[2].write_text(json.dumps(bad[3])); before=count()
            result=subprocess.run([str(binary),'serve','--config',str(bad[2])],env=env,capture_output=True,text=True,timeout=15)
            assert result.returncode!=0 and count()==before and not (bad[0]/'state/semantic').exists(), result.stderr
        disabled=fixture('disabled'); disabled[3]['http']['semantic_review']=False
        child,log,base=start(disabled); assert request(base,disabled[4])[0]==403; stop(child,log)
        print('PASS semantic review 6: separate default-off, missing review/semantic/token and gateway/driven startup refusals before new state/network')

        corrupt=fixture('corrupt'); cli(corrupt,'refresh')
        connection=sqlite3.connect(corrupt[0]/'state/semantic/index.sqlite3')
        try:
            blob=bytearray(connection.execute('SELECT body FROM generation').fetchone()[0])
            blob[-8:]=struct.pack('<ff',0.25,0.75)
            connection.execute('UPDATE generation SET body=?',(bytes(blob),)); connection.commit()
        finally: connection.close()
        child,log,base=start(corrupt); before=rows(corrupt)
        checked(base,corrupt[4],status=422); assert rows(corrupt)==before; stop(child,log)
        print('PASS semantic review 7: matching-manifest invalid vector refuses HTTP readiness and preserves complete original tables; no network')
    finally:
        release_post.set()
        for child,log in servers:
            if child.poll() is None: child.kill(); child.wait(timeout=8)
            if not log.closed: log.close()
        gateway.shutdown(); gateway.server_close()
