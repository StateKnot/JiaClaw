#!/usr/bin/env python3
"""Two real private backends/gateway; no paid or supplier certification."""
import http.client
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import uuid
from tenant_http_fixture import (binary, env, keys, lock, posts, gates, faults, processes,
                                 wait, port, sql, request, model, stop, launch)

try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-stream-') as temporary:
        root = Path(temporary)
        registry = root / 'registry/users.sqlite3'
        gateway = {'bind': '127.0.0.1:' + str(port()), 'registry_path': str(registry),
                   'request_timeout_seconds': 15, 'max_in_flight': 4, 'tracked_turns': True, 'backends': []}
        config_path = root / 'gateway.json'
        gp = int(gateway['bind'].split(':')[1])
        backends = {}
        for who in ('alice', 'bob'):
            directory = root / who
            directory.mkdir(mode=0o700)
            workspace = directory / 'workspace'
            workspace.mkdir(mode=0o700)
            token = 'fixture-backend-' + uuid.uuid4().hex
            token_file = directory / 'token'
            token_file.write_text(token)
            token_file.chmod(0o600)
            number = port()
            config = {'agent': {'name': who, 'description': 'Disposable tenant HTTP acceptance',
                'system_instructions': 'Use only explicitly authorized tools.', 'max_turns': 10,
                'workspace_path': str(workspace), 'tool_timeout_secs': 2},
                'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{model.server_port}', 'api_key': keys[who], 'model': 'fixture-tenant-http'},
                'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
                'http': {'bind': f'127.0.0.1:{number}', 'persist': True, 'persist_path': '../state/sessions.sqlite3',
                         'api_token': token, 'tracked_turns': True, 'tracked_turn_timeout_secs': 10,
                         'shutdown_timeout_secs': 2, 'gateway_channel_chat': True}}
            path = directory / 'config.json'
            path.write_text(json.dumps(config))
            backends[who] = {'path': path, 'config': config, 'port': number, 'token': token,
                             'db': directory / 'state/sessions.sqlite3', 'ledger': directory / 'state/model-calls/index.sqlite3', 'log': directory / 'serve.log'}
            gateway['backends'].append({'id': who, 'url': f'http://127.0.0.1:{number}/', 'token_file': str(token_file)})
            backends[who]['process'] = launch(['serve', '--config', str(path)], number, backends[who]['log'])
        config_path.write_text(json.dumps(gateway))

        def cli(*args):
            result = subprocess.run([str(binary), 'gateway', *args, '--config', str(config_path)], env=env,
                                    capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, (args, result.stdout, result.stderr)
            return json.loads(result.stdout)
        users = {who: cli('user-add', '--backend', who) for who in ('alice', 'bob')}
        readonly = cli('key-add', '--user', users['alice']['user_id'], '--read-only')
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')

        def api(who, path, method='GET', body=None, expected=200, token=None):
            status, value, headers = request(gp, path, token or users[who]['token'], method, body)
            assert status == expected, (who, method, path, status, value)
            assert headers['Cache-Control'] == 'no-store' and headers['X-Content-Type-Options'] == 'nosniff'
            return value
        def hold(who):
            return sql(registry, 'SELECT * FROM write_holds WHERE user_id=?', (users[who]['user_id'],))
        def body(prompt, session=None):
            return {'session_id': session or 'http:' + str(uuid.uuid4()), 'prompt': prompt, 'enabled_tools': ['datetime_now']}
        def complete(who, identity):
            def done():
                value = api(who, '/api/turns/' + identity)
                return value if value['receipt']['state'] != 'running' and not value['active'] else None
            return wait(done, 'terminal original receipt')
        def gate(who, prompt):
            v = {'ready': threading.Event(), 'release': threading.Event()}
            gates[(who, prompt)] = v
            return v
        def count(who, prompt):
            with lock:
                return sum(p['who'] == who and p['prompt'] == prompt for p in posts)

        class Stream:
            def __init__(self, who, prompt, identity=None):
                self.id = identity or str(uuid.uuid4())
                self.body = body(prompt)
                self.connection = http.client.HTTPConnection('127.0.0.1', gp, timeout=20)
                self.connection.request('PUT', '/api/turns/' + self.id + '/stream', json.dumps(self.body).encode(), {'Authorization':'Bearer '+users[who]['token'], 'Content-Type':'application/json','Accept':'text/event-stream'})
                self.response = self.connection.getresponse()
                assert self.response.status == 202, self.response.read()
                assert self.response.getheader('Content-Type') == 'text/event-stream; charset=utf-8'
                assert self.response.getheader('Cache-Control') == 'no-store'
                assert self.response.getheader('X-Accel-Buffering') == 'no'
                self.events = []
                self.wire = 0
            def next(self):
                name, data = None, None
                while True:
                    line = self.response.readline(2*1024*1024+32*1024)
                    self.wire += len(line)
                    assert self.wire <= 12*1024*1024
                    for secret in [*keys.values(), *[v['token'] for v in backends.values()]]:
                        assert secret.encode() not in line
                    if not line:
                        assert name is None and data is None, 'incomplete stream requires original lookup'
                        return None
                    if line.startswith(b':'): continue
                    if line == b'\n':
                        assert name and data
                        event = json.loads(data)
                        assert event['event'] == name
                        self.events.append(event)
                        return event
                    if line.startswith(b'event: '):
                        assert name is None
                        name = line[7:].strip().decode()
                    elif line.startswith(b'data: '):
                        assert data is None
                        data = line[6:].strip()
                    else: raise AssertionError('unexpected frame')
            def preview(self):
                while True:
                    event = self.next()
                    assert event and event['event'] not in ('done','error')
                    if event['event'] == 'preview': break
                assert self.events[0]['event'] == 'admitted' and self.events[0]['receipt']['id'] == self.id
                assert next(v for v in self.events if v['event']=='model_started')['turn_id'] == self.id
            def finish(self):
                while self.next() is not None: pass
                self.close()
                return self.events[-1]
            def close(self):
                self.response.close(); self.connection.close()

        wrong = str(uuid.uuid4())
        assert api('alice','/api/turns/capabilities')['gateway_protocol'] == 2
        api('alice','/api/turns/'+wrong+'/stream','PUT',body('readonly-stream'),expected=403,token=readonly['token'])
        api('alice','/api/turns/'+wrong+'/stream','PUT',dict(body('bad-tool'),enabled_tools=['file_write']),expected=400)
        for path in ('/api/turns/'+wrong+'/stream?backend=bob','/api/turns/'+wrong+'/stream/stream'):
            api('alice',path,'PUT',body('invalid-path'),expected=404)
        with socket.create_connection(('127.0.0.1',gp),timeout=2) as sock:
            sock.sendall((f'PUT /api/turns/{wrong}/stream HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {readonly["token"]}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{').encode())
            assert b'403' in sock.recv(4096).split(b'\r\n',1)[0]
        assert not posts and not hold('alice') and not sql(registry,'SELECT * FROM http_turn_requests')
        print('PASS tenant SSE 1: explicit route, strict mode/tools and read-only rejection before partial body or effects',flush=True)

        g=gate('alice','early-preview')
        stream=Stream('alice','early-preview'); stream.preview()
        assert g['ready'].wait(10)
        a=backends['alice']
        wait(lambda: sql(a['ledger'],'SELECT remote_id FROM model_calls WHERE turn_id=?',(stream.id,))[0]['remote_id'],'original remote identity')
        assert hold('alice')[0]['request_id']==stream.id
        assert sql(a['db'],'SELECT id FROM http_turns')[0]['id']==stream.id
        assert not sql(a['db'],'SELECT * FROM sessions WHERE id=?',(stream.body['session_id'],))
        duplicate=api('alice','/api/turns/'+stream.id+'/stream','PUT',stream.body)
        assert duplicate['active'] and duplicate['receipt']['state']=='running'
        api('alice','/api/turns/'+stream.id+'/stream','PUT',dict(stream.body,prompt='changed'),expected=409)
        api('bob','/api/turns/'+stream.id,expected=404)
        bob=Stream('bob','isolated-stream',stream.id)
        assert bob.finish()['receipt']['result']['reply']=='bob: isolated-stream'
        g['release'].set()
        final=stream.finish()
        assert final['event']=='done' and final['receipt']['state']=='completed'
        assert final['receipt']['result']['reply']=='alice: early-preview'
        assert not hold('alice'), 'done only after original backend idle and gateway hold commit'
        assert count('alice','early-preview')==count('bob','isolated-stream')==1
        assert api('alice','/api/turns/'+stream.id,token=readonly['token'])['receipt']['state']=='completed'
        assert json.loads(sql(a['db'],'SELECT messages FROM sessions WHERE id=?',(stream.body['session_id'],))[0]['messages'])[-1]['content']=='alice: early-preview'
        assert api('alice','/api/turns/'+stream.id+'/stream','PUT',stream.body)['receipt']['state']=='completed'
        print('PASS tenant SSE 2: true preview before model settlement, same UUID across stores, isolated streams, lookup-only repeats and committed done',flush=True)

        g=gate('alice','explicit-cancel')
        stream=Stream('alice','explicit-cancel');stream.preview();assert g['ready'].wait(10)
        intent=api('alice','/api/turns/'+stream.id+'/cancel','POST')
        assert intent['receipt']['cancel_requested'] and intent['active']
        assert hold('alice') and api('alice','/api/turns/'+stream.id)['active']
        g['release'].set();stream.finish()
        final=complete('alice',stream.id)
        assert final['receipt']['state']=='needs_review' and final['receipt']['cancel_requested']
        wait(lambda: hold('alice')[0]['state']=='needs_review','retained cancelled hold')
        assert sql(a['ledger'],'SELECT state FROM model_calls WHERE turn_id=?',(stream.id,))[0]['state']=='completed'
        assert not sql(a['db'],'SELECT * FROM sessions WHERE id=?',(stream.body['session_id'],))
        assert count('alice','explicit-cancel')==1
        cli('review-clear','--user',users['alice']['user_id'],'--confirm-backend-idle','--note','Fixture terminal owner and model ledger verified; no replay')
        print('PASS tenant SSE 3: durable explicit cancel preserves submitted model settlement and review hold without history or redispatch',flush=True)

        g=gate('alice','consumer-disconnect')
        stream=Stream('alice','consumer-disconnect');stream.preview();assert g['ready'].wait(10)
        stream.close()
        wait(lambda: api('alice','/api/turns/'+stream.id)['receipt']['cancel_requested'],'durable disconnect cancellation')
        assert api('alice','/api/turns/'+stream.id)['active']
        assert hold('alice')[0]['request_id']==stream.id
        g['release'].set()
        assert complete('alice',stream.id)['receipt']['state']=='needs_review'
        wait(lambda: hold('alice')[0]['state']=='needs_review','disconnect settlement')
        assert count('alice','consumer-disconnect')==1
        cli('review-clear','--user',users['alice']['user_id'],'--confirm-backend-idle','--note','Fixture disconnected original owner and ledger verified; no replay')
        print('PASS tenant SSE 4: socket loss persists cancel, keeps execution capacity through actual remote settlement, never auto reconnects',flush=True)

        g=gate('alice','gateway-killed-stream')
        stream=Stream('alice','gateway-killed-stream');stream.preview();assert g['ready'].wait(10)
        stop(process,kill=True);stream.close()
        process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
        assert hold('alice')[0]['state']=='needs_review'
        before=count('alice','gateway-killed-stream')
        assert api('alice','/api/turns/'+stream.id+'/stream','PUT',stream.body)['receipt']['id']==stream.id
        g['release'].set();complete('alice',stream.id)
        assert hold('alice')[0]['state']=='needs_review'
        assert count('alice','gateway-killed-stream')==before==1
        assert api('alice','/api/turns')['requests'][0]['id']==stream.id
        print('PASS tenant SSE 5: actual SIGKILL/restart retains identity and review capacity; repeated stream PUT only GETs original receipt',flush=True)
        # Original observation deadline is shorter than the submitted backend/model.
        cli('review-clear','--user',users['alice']['user_id'],'--confirm-backend-idle','--note','Fixture killed gateway receipt and stopped backend owner verified')
        stop(process)
        b=backends['bob'];stop(b['process'])
        b['config']['http']['tracked_turn_timeout_secs']=30
        b['path'].write_text(json.dumps(b['config']))
        b['process']=launch(['serve','--config',str(b['path'])],b['port'],b['log'])
        gateway['request_timeout_seconds']=10;gateway['max_in_flight']=1
        config_path.write_text(json.dumps(gateway))
        process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
        g=gate('bob','original-stream-deadline')
        started=time.monotonic();stream=Stream('bob','original-stream-deadline');stream.preview()
        terminal=stream.finish()
        assert time.monotonic()-started < 13 and terminal['event']=='error'
        wait(lambda: hold('bob')[0]['state']=='needs_review','original observation hold',seconds=2)
        assert api('bob','/api/turns/'+stream.id)['active']
        api('alice','/api/turns/'+str(uuid.uuid4())+'/stream','PUT',body('blocked-original-capacity'),expected=429)
        g['release'].set();complete('bob',stream.id)
        assert count('bob','original-stream-deadline')==1 and count('alice','blocked-original-capacity')==0
        cli('review-clear','--user',users['bob']['user_id'],'--confirm-backend-idle','--note','Fixture original deadline owner and ledger verified; reclaim only by restart')
        api('bob','/api/turns/'+str(uuid.uuid4())+'/stream','PUT',body('still-reserved'),expected=429)
        stop(process)
        process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
        assert Stream('bob','after-reviewed-restart').finish()['receipt']['state']=='completed'
        assert count('bob','still-reserved')==0
        print('PASS tenant SSE 6: original deadline never resets; unknown actual model retains shared capacity until idle review and restart',flush=True)
        # Fault injection happens only after the actual native body reaches EOF:
        # real terminal commit/current owner completion precede corrupted delivery.
        stop(process)
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        corruption = 'model_identity'
        class CorruptStream(BaseHTTPRequestHandler):
            def log_message(self, *_args): pass
            def forward(self):
                upstream = http.client.HTTPConnection('127.0.0.1', backends['alice']['port'], timeout=15)
                try:
                    data = self.rfile.read(int(self.headers.get('Content-Length','0')))
                    headers = {name:self.headers[name] for name in ('Authorization','Content-Type','x-jiaclaw-gateway-turns') if name in self.headers}
                    upstream.request(self.command, self.path, data or None, headers)
                    response = upstream.getresponse()
                    raw = response.read(12*1024*1024+1)
                    assert len(raw)<=12*1024*1024
                    if self.command == 'PUT' and self.path.endswith('/stream') and response.status == 202:
                        actual_hold=hold('alice')
                        assert actual_hold and actual_hold[0]['state']=='in_flight','actual gateway admission must precede corrupted delivery'
                        frames = raw.split(b'\n\n'); changed = False
                        for i, frame in enumerate(frames):
                            if corruption == 'model_identity' and frame.startswith(b'event: model_started\n'):
                                event = json.loads(frame.split(b'\ndata: ',1)[1]); event['turn_id']=str(uuid.uuid4())
                                frames[i]=b'event: model_started\ndata: '+json.dumps(event).encode(); changed=True; break
                            if corruption == 'tool_authority' and frame.startswith(b'event: done\n'):
                                event={'event':'tool_completed','round':0,'tool_call_id':'fixture-unapproved','tool_name':'file_write'}
                                frames.insert(i,b'event: tool_completed\ndata: '+json.dumps(event).encode());changed=True;break
                        if corruption: assert changed, 'actual original event must be corrupted'
                        raw=b'\n\n'.join(frames)
                    self.send_response(response.status)
                    self.send_header('Content-Type',response.getheader('Content-Type'))
                    self.send_header('Content-Length',str(len(raw)))
                    self.end_headers();self.wfile.write(raw)
                except (BrokenPipeError, ConnectionResetError): pass
                except Exception as error:
                    with lock: faults.append(type(error).__name__+': '+str(error))
                finally: upstream.close()
            do_GET=do_PUT=do_POST=forward
        proxy=ThreadingHTTPServer(('127.0.0.1',0),CorruptStream);proxy.daemon_threads=True
        threading.Thread(target=proxy.serve_forever,daemon=True).start()
        for backend in gateway['backends']:
            if backend['id']=='alice': backend['url']=f'http://127.0.0.1:{proxy.server_port}/'
        config_path.write_text(json.dumps(gateway))
        process=launch(['gateway','serve','--config',str(config_path)],gp,root/'gateway.log')
        try:
            for corruption in ('model_identity','tool_authority'):
                prompt='committed-corrupt-'+corruption
                stream=Stream('alice',prompt)
                assert stream.finish()['event']=='error'
                before_lookup=hold('alice')
                original=complete('alice',stream.id)
                assert original['receipt']['state']=='completed' and original['receipt']['session_committed'] and not original['active']
                assert count('alice',prompt)==1
                assert sql(backends['alice']['ledger'],'SELECT state FROM model_calls WHERE turn_id=?',(stream.id,))[0]['state']=='completed'
                current=hold('alice')
                print(json.dumps({'corruption':corruption,'original_completed':True,'native_active':False,'model_posts':count('alice',prompt),'gateway_hold_before_GET':before_lookup[0]['state'] if before_lookup else None,'gateway_hold':current[0]['state'] if current else None}),flush=True)
                assert current and current[0]['state']=='needs_review' and current[0]['request_id']==stream.id,'rejected original SSE contract must retain review even after a completed valid GET'
                # Original identity stays lookup-only while review blocks new writes.
                assert api('alice','/api/turns/'+stream.id+'/stream','PUT',stream.body)['receipt']['state']=='completed'
                api('alice','/api/turns/'+str(uuid.uuid4())+'/stream','PUT',body('blocked-corrupt-contract'),expected=409)
                assert count('alice','blocked-corrupt-contract')==0
                cli('review-clear','--user',users['alice']['user_id'],'--confirm-backend-idle','--note','Fixture rejected wire contract independently reviewed; original native/model owners verified idle')
            corruption=None
            assert Stream('alice','valid-after-corrupt-review').finish()['receipt']['state']=='completed'
            assert not hold('alice')
            # A valid native 200 JSON lookup predating the gateway mapping is not
            # a failed delivery and must retain the original successful semantics.
            native_id=str(uuid.uuid4());native_body=body('native-before-gateway-mapping')
            assert request(backends['alice']['port'],'/api/turns/'+native_id,backends['alice']['token'],'PUT',native_body,marker=True)[0]==202
            wait(lambda: (lambda v:v['receipt']['state']=='completed' and not v['active'])(request(backends['alice']['port'],'/api/turns/'+native_id,backends['alice']['token'],marker=True)[1]),'native existing completed identity')
            assert api('alice','/api/turns/'+native_id+'/stream','PUT',native_body)['receipt']['state']=='completed'
            wait(lambda: not hold('alice'),'valid JSON lookup clears matching successful hold')
            assert count('alice','native-before-gateway-mapping')==1
            print('PASS tenant SSE 7: actual committed native result cannot clear rejected identity/tool stream review; idle capacity releases, duplicates remain GET-only and valid native JSON lookup succeeds',flush=True)
        finally:
            proxy.shutdown();proxy.server_close()
finally:
    for g in gates.values(): g['release'].set()
    for p in reversed(processes): stop(p)
    model.shutdown()
