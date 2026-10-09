#!/usr/bin/env python3
"""Real HTTP turn identities, native effects, SQLite rollback, cancel and restart.

Disposable local Brokerrouter contract fixture; no paid credentials or supplier,
Web/token-delivery, gateway tenant or durable tool-resume certification.
"""
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
provider_key = 'local-contract-' + uuid.uuid4().hex
api_token = 'operator-' + uuid.uuid4().hex
lock = threading.Lock()
posts, gets, faults, gates, results, processes = [], [], [], {}, {}, []


def chunk(model, delta=None, finish=None, usage=None):
    return {'id': 'fixture-completion', 'object': 'chat.completion.chunk', 'created': 1,
            'model': model, 'choices': [] if usage else [{'index': 0, 'delta': delta or {}, 'finish_reason': finish}], 'usage': usage}


class Provider(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, value):
        raw = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        try:
            assert self.headers['Authorization'] == 'Bearer ' + provider_key
            remote = self.path.split('/')[3]
            with lock:
                gets.append(self.path)
                receipt = results[remote]
            self.reply(receipt if self.path.endswith('/result') else {
                'id': remote, 'model': receipt['model'], 'purpose': 'model', 'status': 'succeeded'})
        except Exception as error:
            faults.append(repr(error))

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + provider_key
            assert self.headers['Accept'] == 'text/event-stream'
            assert self.headers.get('X-Brokerrouter-Turn-Id') is None
            raw = self.rfile.read(int(self.headers['Content-Length']))
            body = json.loads(raw)
            assert body['stream'] is True
            operation = self.headers['Idempotency-Key']
            assert str(uuid.UUID(operation)) == operation
            case = [m['content'] for m in body['messages'] if m['role'] == 'user'][-1]
            with lock:
                prior = sum(p['case'] == case for p in posts)
                assert not any(p['operation'] == operation for p in posts)
                remote = str(uuid.uuid4())
                posts.append({'case': case, 'operation': operation, 'remote': remote,
                              'body_hash': hashlib.sha256(raw).hexdigest(), 'body': body})
                gate = gates.get(case)
            calls = []
            if case in ('tools', 'lost-reply', 'cancel', 'budget', 'killed', 'shutdown') and prior == 0:
                calls = [{'id': 'call-0', 'type': 'function', 'function': {'name': 'file_write',
                          'arguments': json.dumps({'path': case + '.txt', 'content': 'exactly one authorized effect'})}}]
            if case == 'tool-failure' and prior == 0:
                calls = [{'id': 'call-0', 'type': 'function', 'function': {'name': 'file_write',
                          'arguments': json.dumps({'path': '../outside.txt', 'content': 'forbidden'})}},
                         {'id': 'call-1', 'type': 'function', 'function': {'name': 'file_write',
                          'arguments': json.dumps({'path': 'must-not-run.txt', 'content': 'later effect'})}}]
            if prior:
                assert case in ('tools', 'lost-reply')
                assert body['messages'][-1]['role'] == 'tool'
            message = {'role': 'assistant', 'content': '临时预览' if calls else '最终🦀回复'}
            if calls:
                message['tool_calls'] = calls
            receipt = {'id': 'fixture-completion', 'object': 'chat.completion', 'created': 1,
                       'model': body['model'], 'choices': [{'index': 0, 'message': message, 'finish_reason': 'tool_calls' if calls else 'stop'}],
                       'usage': {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}}
            with lock:
                results[remote] = receipt
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('x-brokerrouter-request-id', remote)
            self.send_header('Connection', 'close')
            self.end_headers()
            self.close_connection = True

            def send(value):
                self.wfile.write(('data: ' + (value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)) + '\r\n\r\n').encode())
                self.wfile.flush()

            send(chunk(body['model'], {'role': 'assistant', 'content': '临时预览' if calls else '最终🦀'}))
            if gate is not None and prior == 0:
                gate['ready'].set()
                assert gate['release'].wait(20), 'fixture gate expired'
            if calls:
                for i, call in enumerate(calls):
                    send(chunk(body['model'], {'tool_calls': [dict(call, index=i)]}))
                send(chunk(body['model'], {}, 'tool_calls'))
            else:
                send(chunk(body['model'], {'content': '回复'}, 'stop'))
            send(chunk(body['model'], usage={'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}))
            send('[DONE]')
        except (BrokenPipeError, ConnectionResetError):
            pass  # Intentional SIGKILL/stop fixture; never a supplier retry.
        except Exception as error:
            faults.append(repr(error))


server = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()


def eventually(check, description, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(.02)
    raise AssertionError(description + ' timed out')


def gate(case):
    gates[case] = {'ready': threading.Event(), 'release': threading.Event()}
    return gates[case]


def count(case):
    with lock:
        return sum(p['case'] == case for p in posts)


class App:
    def __init__(self, root, name, timeout=10, enabled=True):
        self.directory = root / name
        self.directory.mkdir(mode=0o700)
        self.workspace = self.directory / 'workspace'
        self.workspace.mkdir(mode=0o700)
        self.config = self.directory / 'config.json'
        self.settings = {'agent': {'name': name, 'description': 'Disposable HTTP turn acceptance',
            'system_instructions': 'Use only authorized native tools.', 'max_turns': 10,
            'workspace_path': str(self.workspace), 'max_tool_iterations': 3, 'tool_timeout_secs': 2},
            'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{server.server_port}', 'api_key': provider_key, 'model': 'fixture-http'},
            'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
            'http': {'bind': '127.0.0.1:0', 'persist': True, 'persist_path': '../state/sessions.sqlite3',
                     'api_token': api_token, 'tracked_turns': enabled, 'tracked_turn_timeout_secs': timeout, 'shutdown_timeout_secs': 2}}
        self.process = None
        self.save()

    def save(self):
        self.config.write_text(json.dumps(self.settings))

    @property
    def db(self):
        return self.directory / 'state/sessions.sqlite3'

    def sql(self, query, parameters=(), write=False):
        with sqlite3.connect(self.db, timeout=6) as connection:
            connection.row_factory = sqlite3.Row
            rows = [dict(r) for r in connection.execute(query, parameters)]
            if write:
                connection.commit()
            return rows

    def ledger(self):
        with sqlite3.connect(self.directory / 'state/model-calls/index.sqlite3') as connection:
            connection.row_factory = sqlite3.Row
            return [dict(r) for r in connection.execute('SELECT id,turn_id,body_hash,remote_id,state FROM model_calls ORDER BY seq')]

    def start(self):
        log = self.directory / ('serve-' + uuid.uuid4().hex + '.log')
        with log.open('wb') as output:
            self.process = subprocess.Popen([str(binary), 'serve', '--config', str(self.config)], stdout=output, stderr=output, env=env)
        processes.append(self.process)
        def ready():
            assert self.process.poll() is None, log.read_text()
            match = re.search(r'HTTP 服务已启动于 http://127\.0\.0\.1:([1-9][0-9]*)', log.read_text())
            if match:
                self.port = int(match.group(1))
                return self.http('/health')[0] == 200
        eventually(ready, 'actual server start', 15)

    def stop(self, kill=False):
        if self.process and self.process.poll() is None:
            self.process.kill() if kill else self.process.terminate()
            code = self.process.wait(timeout=6)
            if not kill:
                assert code == 0, code

    def http(self, path, method='GET', body=None, auth=True):
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        headers = {'Authorization': 'Bearer ' + api_token} if auth else {}
        data = json.dumps(body, ensure_ascii=False).encode() if body is not None else None
        if data is not None:
            headers['Content-Type'] = 'application/json'
        conn.request(method, path, data, headers)
        response = conn.getresponse()
        raw = response.read()
        assert provider_key.encode() not in raw and api_token.encode() not in raw
        conn.close()
        return response.status, json.loads(raw)

    def submit(self, case, session=None, identity=None):
        identity = identity or str(uuid.uuid4())
        body = {'session_id': session or 'http:' + str(uuid.uuid4()), 'prompt': case, 'enabled_tools': ['file_write']}
        status, envelope = self.http('/api/turns/' + identity, 'PUT', body)
        assert status == 202, (status, envelope)
        return identity, body

    def terminal(self, identity, expected='completed'):
        def done():
            status, value = self.http('/api/turns/' + identity)
            assert status == 200
            return value if value['receipt']['state'] != 'running' and not value['active'] else None
        value = eventually(done, 'authoritative terminal receipt')
        assert value['receipt']['state'] == expected, value
        return value['receipt']

    def history(self, session):
        return self.sql('SELECT messages FROM sessions WHERE id=?', (session,))


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-http-turns-') as temporary:
        root = Path(temporary)
        app = App(root, 'preflight', enabled=False)
        app.start()
        assert app.http('/api/turns/capabilities')[1]['enabled'] is False
        assert app.http('/api/turns/capabilities', auth=False)[0] == 401
        assert app.http('/api/turns/' + str(uuid.uuid4()), 'PUT', {'session_id': 'http:' + str(uuid.uuid4()), 'prompt': 'disabled', 'enabled_tools': ['file_write']})[0] == 503
        app.stop()
        for kind in ('missing-auth', 'missing-ledger', 'wrong-provider', 'unbounded-tools', 'tenant', 'unbounded-turn'):
            bad = json.loads(json.dumps(app.settings))
            bad['http']['tracked_turns'] = True
            if kind == 'missing-auth': bad['http'].pop('api_token')
            if kind == 'missing-ledger': bad['model_calls']['enabled'] = False
            if kind == 'wrong-provider': bad['provider']['provider_type'] = 'stub'
            if kind == 'unbounded-tools': bad['agent']['tool_timeout_secs'] = 31
            if kind == 'tenant': bad['http']['gateway_channel_chat'] = True
            if kind == 'unbounded-turn': bad['http']['tracked_turn_timeout_secs'] = 301
            path = root / (kind + '.json'); path.write_text(json.dumps(bad))
            result = subprocess.run([str(binary), 'serve', '--config', str(path)], env=env, capture_output=True, timeout=10)
            assert result.returncode != 0, kind
        app.start()
        identity = str(uuid.uuid4())
        # A wrong credential must be rejected before waiting for a partial body.
        sock = socket.create_connection(('127.0.0.1', app.port)); sock.settimeout(2)
        sock.sendall((f'PUT /api/turns/{identity} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer wrong-secret\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n' + '{').encode())
        assert b'401' in sock.recv(4096).split(b'\r\n', 1)[0]; sock.close()
        # A valid slow body is bounded independently of model/turn deadlines.
        sock = socket.create_connection(('127.0.0.1', app.port)); sock.settimeout(7)
        sock.sendall((f'PUT /api/turns/{identity} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {api_token}\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n' + '{').encode())
        started = time.monotonic(); assert b'408' in sock.recv(4096).split(b'\r\n', 1)[0]
        assert 4.5 <= time.monotonic() - started < 7; sock.close()
        assert app.http('/api/turns/' + identity, 'PUT', {'padding': 'x' * 65536})[0] == 413
        assert not app.sql('SELECT * FROM http_turns')
        app.stop()
        assert not posts
        print('PASS: opt-in/authentication/finite policy/tenant preflight and slow/oversized body reject before effects', flush=True)

        app = App(root, 'completed')
        app.start()
        control = gate('tools')
        identity, body = app.submit('tools')
        assert control['ready'].wait(10)
        initial = app.http('/api/turns/' + identity)[1]
        assert initial['active'] and initial['receipt']['state'] == 'running' and not initial['receipt']['session_committed']
        assert not app.history(body['session_id']) and not (app.workspace / 'tools.txt').exists()
        normalized = dict(body, enabled_skills=[])
        assert initial['receipt']['request_hash'] == hashlib.sha256(json.dumps(normalized, ensure_ascii=False, separators=(',', ':')).encode()).hexdigest()
        assert app.ledger()[0]['turn_id'] == identity and app.ledger()[0]['state'] == 'submitting'
        assert app.http('/api/turns/' + identity, 'PUT', body)[0] == 200 and count('tools') == 1
        assert app.http('/api/turns/' + identity, 'PUT', dict(body, prompt='conflict'))[0] == 409
        assert app.http('/api/turns/' + str(uuid.uuid4()), 'PUT', dict(body, session_id='http:' + str(uuid.uuid4())))[0] == 409
        assert app.http('/api/turns/' + identity + '/review', 'POST', {'decision': 'abandon', 'note': 'too early'})[0] == 409
        assert app.http('/api/chat', 'POST', {'session_id': body['session_id'], 'messages': [{'role': 'user', 'content': 'bypass'}]})[0] == 400
        assert app.http('/api/turns/' + identity, auth=False)[0] == 401
        control['release'].set()
        receipt = app.terminal(identity)
        assert receipt['session_committed'] and receipt['result']['tool_names'] == ['file_write']
        assert receipt['result']['reply'] == '最终🦀回复' and count('tools') == 2
        assert (app.workspace / 'tools.txt').read_text() == 'exactly one authorized effect'
        assert len(json.loads(app.history(body['session_id'])[0]['messages'])) == 2
        assert all(r['turn_id'] == identity and r['state'] == 'completed' for r in app.ledger())
        app.stop(kill=True); app.start()
        assert app.http('/api/turns/' + identity, 'PUT', body)[1]['receipt']['result'] == receipt['result']
        assert count('tools') == 2
        assert app.http('/api/turns/' + identity + '/result', 'DELETE')[1]['receipt']['result_purged']
        assert app.http('/api/turns/' + identity, 'PUT', body)[1]['receipt']['result'] is None and count('tools') == 2
        app.stop()
        print('PASS: durable original identity, native two-round effect, atomic history/result, duplicate/conflict and purge/restart', flush=True)

        app = App(root, 'lost-response'); app.start()
        control = gate('lost-reply')
        identity = str(uuid.uuid4()); body = {'session_id': 'http:' + str(uuid.uuid4()), 'prompt': 'lost-reply', 'enabled_tools': ['file_write']}
        writer = sqlite3.connect(app.db); writer.execute('BEGIN IMMEDIATE')
        raw = json.dumps(body).encode()
        request = (f'PUT /api/turns/{identity} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {api_token}\r\nContent-Type: application/json\r\nContent-Length: {len(raw)}\r\nConnection: close\r\n\r\n').encode() + raw
        sock = socket.create_connection(('127.0.0.1', app.port)); sock.sendall(request)
        time.sleep(.15)
        assert count('lost-reply') == 0 and writer.execute('SELECT count(*) FROM http_turns WHERE id=?', (identity,)).fetchone()[0] == 0
        # Fully sent request loses its original response; this is not a claim that
        # Axum cancelled its handler. The independently owned admitted work settles.
        sock.close(); writer.rollback(); writer.close()
        assert control['ready'].wait(10)
        assert app.http('/api/turns/' + identity, 'PUT', body)[0] == 200
        control['release'].set(); app.terminal(identity)
        assert count('lost-reply') == 2 and (app.workspace / 'lost-reply.txt').exists()
        app.stop()
        print('PASS: blocked durable admission precedes model POST; lost HTTP reply never resubmits an effect', flush=True)

        for case in ('cancel', 'budget'):
            app = App(root, case, timeout=1 if case == 'budget' else 10); app.start()
            control = gate(case); identity, body = app.submit(case)
            assert control['ready'].wait(10)
            if case == 'cancel':
                cancelled = app.http('/api/turns/' + identity + '/cancel', 'POST')[1]
                assert cancelled['receipt']['cancel_requested'] and cancelled['active']
            else:
                time.sleep(1.15)
            held = app.http('/api/turns/' + identity)[1]
            assert held['active'] and held['receipt']['state'] == 'running'
            assert app.http('/api/turns/' + identity + '/review', 'POST', {'decision': 'abandon', 'note': 'still settling'})[0] == 409
            assert not (app.workspace / (case + '.txt')).exists()
            control['release'].set()
            receipt = app.terminal(identity, 'needs_review')
            assert not receipt['session_committed'] and not app.history(body['session_id'])
            assert app.ledger()[0]['state'] == 'completed' and count(case) == 1
            assert not (app.workspace / (case + '.txt')).exists()
            assert app.http('/api/turns/' + str(uuid.uuid4()), 'PUT', body)[0] == 409
            assert app.http('/api/sessions/import', 'POST', {'id': body['session_id'], 'messages': []})[0] == 409
            assert app.http('/api/sessions/' + body['session_id'], 'DELETE')[0] == 409
            reviewed = app.http('/api/turns/' + identity + '/review', 'POST', {'decision': 'abandon', 'note': 'Reviewed model ledger and workspace; abandon incomplete turn'})[1]['receipt']
            assert reviewed['reviewed_ms'] is not None
            assert app.http('/api/turns/' + identity, 'PUT', body)[0] == 200 and count(case) == 1
            assert app.http('/api/turns/' + identity + '/result', 'DELETE')[0] == 200
            app.stop()
        print('PASS: explicit cancellation and total-turn deadline retain current settlement owner; no preview-authorized effects or replay', flush=True)

        app = App(root, 'failed-tool'); app.start()
        identity, body = app.submit('tool-failure')
        receipt = app.terminal(identity, 'needs_review')
        assert receipt['session_committed'] and receipt['result']['tool_names'] == ['file_write']
        assert count('tool-failure') == 1 and not (app.workspace / 'must-not-run.txt').exists() and not (app.directory / 'outside.txt').exists()
        assert app.http('/api/turns/' + identity, 'PUT', body)[0] == 200
        app.stop()
        print('PASS: failed native tool preserves review transcript and stops later batch effects/model dispatch', flush=True)

        app = App(root, 'rollback'); app.start()
        control = gate('storage-fault'); identity, body = app.submit('storage-fault')
        assert control['ready'].wait(10)
        app.sql("CREATE TRIGGER fixture_reject_finish BEFORE UPDATE OF state ON http_turns BEGIN SELECT RAISE(ABORT,'fixture terminal fault'); END", write=True)
        control['release'].set()
        orphan = eventually(lambda: (v if not v['active'] else None) if (v := app.http('/api/turns/' + identity)[1]) else None, 'actual owner returned after rejected terminal transaction')
        assert orphan['receipt']['state'] == 'running' and not app.history(body['session_id'])
        assert app.http('/api/turns/' + identity, 'PUT', body)[0] == 200 and count('storage-fault') == 1
        app.sql('DROP TRIGGER fixture_reject_finish', write=True)
        assert app.http('/api/turns/' + identity + '/review', 'POST', {'decision': 'abandon', 'note': 'SQL terminal fault reviewed; original model receipt is complete'})[1]['receipt']['reviewed_ms']
        app.stop()
        print('PASS: terminal SQL failure cannot partially commit history; orphaned identity requires explicit review', flush=True)

        for case in ('killed', 'shutdown'):
            app = App(root, case); app.start()
            control = gate(case); identity, body = app.submit(case)
            assert control['ready'].wait(10)
            eventually(lambda: app.ledger()[0]['remote_id'] is not None, 'remote identity durable before killing owner')
            started = time.monotonic(); app.stop(kill=case == 'killed')
            assert time.monotonic() - started < 6
            control['release'].set()
            app.start()
            receipt = app.http('/api/turns/' + identity)[1]['receipt']
            assert receipt['state'] == 'needs_review' and receipt['error'] == 'process_interrupted'
            assert count(case) == 1 and not (app.workspace / (case + '.txt')).exists() and not app.history(body['session_id'])
            assert app.http('/api/turns/' + identity, 'PUT', body)[0] == 200
            app.stop()
            if case == 'killed':
                operation = app.ledger()[0]['id']
                recovered = subprocess.run([str(binary), 'model-calls', '--config', str(app.config), 'recover', operation], env=env, capture_output=True, timeout=12)
                assert recovered.returncode == 0, recovered.stderr
                assert gets and count(case) == 1 and not (app.workspace / (case + '.txt')).exists()
                app.start()
                assert app.http('/api/turns/' + identity)[1]['receipt']['state'] == 'needs_review'
                app.stop()
        print('PASS: SIGKILL/shared shutdown grace preserve unresolved original identity; GET-only recovery never applies native effects/session', flush=True)
        assert not faults, faults
        print('PASS: seven HTTP turn integration groups; no paid credentials, no tenant/Web/durable claims', flush=True)
finally:
    for value in gates.values():
        value['release'].set()
    for process in processes:
        if process.poll() is None:
            process.kill(); process.wait(timeout=5)
    server.shutdown(); server.server_close()
