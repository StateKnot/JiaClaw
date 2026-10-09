#!/usr/bin/env python3
"""Real CLI SSE, native tools, receipts, slow/disconnected output and restart.

Disposable localhost protocols only; this does not certify an upstream gateway,
supplier usage/accounting, hosted streams, tenants or durable tool recovery.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'warn'
key = 'stream-fixture-' + uuid.uuid4().hex
lock = threading.Lock()
posts, gets, failures, controls, receipts = [], [], [], {}, {}
processes = []


def chunk(model, delta=None, finish=None, usage=None, empty=False):
    return {'id': 'completion-fixture', 'object': 'chat.completion.chunk', 'created': 1,
            'model': model, 'choices': [] if empty else [{'index': 0, 'delta': delta or {},
            'finish_reason': finish}], 'usage': usage}


def tool(index=0, name='file_write', arguments=None):
    return {'index': index, 'id': 'call-' + str(index), 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(arguments or {
                'path': 'stream-effect.txt', 'content': 'native stream effect'})}}


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def json_reply(self, value):
        raw = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        try:
            assert self.headers['Authorization'] == 'Bearer ' + key
            parts = self.path.split('/')
            remote = parts[3]
            with lock:
                gets.append(self.path)
                receipt = receipts[remote]
            if self.path.endswith('/result'):
                self.json_reply(receipt)
            else:
                self.json_reply({'id': remote, 'model': receipt['model'], 'purpose': 'model', 'status': 'succeeded'})
        except Exception as error:
            with lock:
                failures.append(repr(error))

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + key
            assert self.headers['Accept'] == 'text/event-stream'
            assert self.headers.get('X-Brokerrouter-Turn-Id') is None
            raw = self.rfile.read(int(self.headers['Content-Length']))
            body = json.loads(raw)
            assert body['stream'] is True
            operation = self.headers['Idempotency-Key']
            assert str(uuid.UUID(operation)) == operation
            case = next(m['content'] for m in body['messages'] if m['role'] == 'user')
            remote = str(uuid.uuid4())
            with lock:
                assert operation not in {p['operation'] for p in posts}
                prior = sum(p['case'] == case for p in posts)
                posts.append({'case': case, 'operation': operation, 'remote': remote,
                              'body_hash': hashlib.sha256(raw).hexdigest(), 'body': body})
                control = controls.get(case)
            model = body['model']
            calls = []
            if case in ('tools', 'invalid-batch', 'recover', 'killed', 'terminated', 'closed', 'slow') and not prior:
                calls = [tool()]
                if case == 'invalid-batch':
                    calls.append(tool(1, 'not_authorized'))
            if case == 'tool-failure' and not prior:
                calls = [tool(arguments={'path': '../outside.txt', 'content': 'x'}),
                         tool(1, arguments={'path': 'must-not-execute.txt', 'content': 'x'})]
            if prior:
                assert case == 'tools'
                assert body['messages'][-1]['role'] == 'tool'
                assert body['messages'][-1]['tool_call_id'] == 'call-0'
            message = {'role': 'assistant', 'content': None if calls else '真实🦀流式回复'}
            if calls:
                message['tool_calls'] = [{k: v for k, v in call.items() if k != 'index'} for call in calls]
            finish = 'tool_calls' if calls else 'stop'
            receipt = {'id': 'completion-fixture', 'object': 'chat.completion', 'created': 1,
                       'model': model, 'choices': [{'index': 0, 'message': message, 'finish_reason': finish}],
                       'usage': {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}}
            with lock:
                receipts[remote] = receipt
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream; charset=utf-8')
            self.send_header('x-brokerrouter-request-id', remote)
            self.send_header('Connection', 'close')
            self.end_headers()
            self.close_connection = True

            def send(data, split=False):
                encoded = ('data: ' + (data if isinstance(data, str) else json.dumps(data, ensure_ascii=False)) + '\r\n\r\n').encode()
                if split:
                    for byte in encoded:
                        self.wfile.write(bytes([byte]))
                        self.wfile.flush()
                else:
                    self.wfile.write(encoded)
                    self.wfile.flush()

            self.wfile.write(b': fixture comment\r\n\r\n')
            send(chunk(model, {'role': 'assistant', 'content': '真实🦀'}), split=True)
            if case in ('slow', 'closed'):
                for _ in range(240):
                    send(chunk(model, {'content': 'x' * 1024}))
            if control is not None:
                control['ready'].set()
                assert control['release'].wait(25), 'fixture release gate expired'
            if case == 'recover':
                return  # Missing finish/usage/[DONE] is uncertain even with a valid preview.
            if case == 'bad-model':
                send(chunk('different-model', {}, 'stop'))
            elif case == 'duplicate':
                send(json.dumps(chunk(model, {}, 'stop')).replace('"model": "' + model + '"',
                     '"model": null, "model": "' + model + '"'))
            elif calls:
                # Arguments are fragmented; the first valid-looking call never grants authority.
                for call in calls:
                    args = call['function']['arguments']
                    first = dict(call, function={'name': 'file_' if call['function']['name'] == 'file_write' else call['function']['name'],
                                                'arguments': args[:5]})
                    send(chunk(model, {'tool_calls': [first]}))
                    send(chunk(model, {'tool_calls': [{'index': call['index'], 'function': {
                        'name': 'write' if call['function']['name'] == 'file_write' else '', 'arguments': args[5:]}}]}))
                send(chunk(model, {}, finish))
            else:
                send(chunk(model, {'content': 'r' * 128000 if case == 'replay' else '流式回复'}, finish))
            if case != 'missing-usage':
                send(chunk(model, usage={'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}, empty=True))
            send('[DONE]')
            if case == 'trailing':
                send(chunk(model, {'content': 'untrusted trailing'}))
        except (BrokenPipeError, ConnectionResetError):
            pass  # Deliberate kill/output interruption can close the HTTP peer.
        except Exception as error:
            with lock:
                failures.append(repr(error))


server = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()


def settings(root, case, ledger=True):
    directory = root / case
    directory.mkdir(mode=0o700)
    workspace = directory / 'workspace'
    workspace.mkdir(mode=0o700)
    data = {'agent': {'name': 'cli-stream-fixture', 'description': 'Offline stream acceptance',
                     'system_instructions': 'Use approved native tools only.', 'max_turns': 10,
                     'workspace_path': str(workspace), 'max_tool_iterations': 3, 'tool_timeout_secs': 2},
            'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{server.server_port}',
                         'api_key': key, 'model': 'fixture-stream'},
            'model_calls': {'enabled': ledger, 'store_path': '../state/model-calls/index.sqlite3'},
            'http': {'persist': True, 'persist_path': '../state/sessions.sqlite3'}}
    config = directory / 'config.json'
    config.write_text(json.dumps(data))
    return directory, workspace, config


def rows(directory):
    with sqlite3.connect(directory / 'state/model-calls/index.sqlite3') as connection:
        connection.row_factory = sqlite3.Row
        return [dict(r) for r in connection.execute('SELECT id,body_hash,remote_id,state,receipt IS NOT NULL AS has_receipt FROM model_calls ORDER BY seq')]


def eventually(check, description, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.02)
    raise AssertionError(description + ' timed out')


def start(config, case, reader=True):
    process = subprocess.Popen([str(binary), 'chat', '--config', str(config), '--stream',
                                '--session', case, '--no-auto-skill', case],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    events = queue.Queue()
    processes.append(process)
    process.reader_thread = None
    if reader:
        def consume():
            try:
                for raw in iter(process.stdout.readline, b''):
                    events.put(json.loads(raw))
            except Exception as error:
                events.put({'reader_error': repr(error)})
        process.reader_thread = threading.Thread(target=consume, daemon=True)
        process.reader_thread.start()
    return process, events


def collected(process, events, success=True):
    status = process.wait(timeout=20)
    if process.reader_thread is not None:
        process.reader_thread.join(timeout=3)
        assert not process.reader_thread.is_alive(), 'reader failed to drain actual EOF'
    error = process.stderr.read().decode()
    assert (status == 0) == success, (status, error)
    values = []
    while not events.empty():
        values.append(events.get())
    assert not any('reader_error' in event for event in values), values
    assert key not in json.dumps(values), 'credential leaked'
    return values


def admin(config, *args):
    result = subprocess.run([str(binary), 'model-calls', '--config', str(config), *args],
                            capture_output=True, env=env, timeout=15)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-cli-stream-') as temporary:
        root = Path(temporary)
        directory, workspace, config = settings(root, 'delayed')
        controls['delayed'] = {'ready': threading.Event(), 'release': threading.Event()}
        process, events = start(config, 'delayed')
        first, preview = events.get(timeout=15), events.get(timeout=15)
        assert first['event'] == 'model_started' and preview == {'event': 'preview', 'round': 0, 'text': '真实🦀'}
        assert process.poll() is None and not controls['delayed']['release'].is_set()
        record = rows(directory)[0]
        request = next(p for p in posts if p['case'] == 'delayed')
        assert record['state'] == 'submitting' and record['remote_id'] == first['remote_id']
        assert record['id'] == request['operation'] == first['operation_id'] and record['body_hash'] == request['body_hash']
        controls['delayed']['release'].set()
        remaining = collected(process, events)
        done = remaining[-1]
        assert done['event'] == 'done' and done['reply'] == '真实🦀流式回复' and done['persisted'] is True
        assert rows(directory)[0]['state'] == 'completed'
        with sqlite3.connect(directory / 'state/sessions.sqlite3') as connection:
            assert connection.execute('SELECT count(*) FROM sessions').fetchone()[0] == 1
        print('PASS: actual preview before gated settlement; original identity/hash persisted; final receipt/session committed', flush=True)

        directory, workspace, config = settings(root, 'replay')
        process, events = start(config, 'replay')
        values = collected(process, events)
        assert values[-1]['reply'] == '真实🦀' + 'r' * 128000
        previews = [v['text'] for v in values if v['event'] == 'preview']
        assert ''.join(previews) == values[-1]['reply'] and all(len(p.encode()) <= 1024 for p in previews)
        print('PASS: replay-sized single SSE event retains UTF-8 byte limits and final authority', flush=True)

        directory, workspace, config = settings(root, 'store-failure')
        controls['store-failure'] = {'ready': threading.Event(), 'release': threading.Event()}
        process, events = start(config, 'store-failure')
        assert events.get(timeout=15)['event'] == 'model_started'
        assert events.get(timeout=15)['event'] == 'preview'
        with sqlite3.connect(directory / 'state/sessions.sqlite3') as connection:
            connection.execute("CREATE TRIGGER fixture_commit_fault BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'fixture commit failure'); END")
        controls['store-failure']['release'].set()
        values = collected(process, events, success=False)
        assert not any(v['event'] == 'done' for v in values)
        assert rows(directory)[0]['state'] == 'completed'
        with sqlite3.connect(directory / 'state/sessions.sqlite3') as connection:
            assert connection.execute('SELECT count(*) FROM sessions').fetchone()[0] == 0
        print('PASS: committed model receipt with failed session commit never emits done or claims persistence', flush=True)

        for case in ('tools', 'invalid-batch', 'tool-failure'):
            directory, workspace, config = settings(root, case)
            process, events = start(config, case)
            values = collected(process, events, success=case != 'invalid-batch')
            matching = [p for p in posts if p['case'] == case]
            if case == 'tools':
                assert (workspace / 'stream-effect.txt').read_text() == 'native stream effect'
                assert len(matching) == 2 and values[-1]['event'] == 'done'
                assert len([v for v in values if v['event'] == 'tool_completed']) == 1
                assert len({v['turn_id'] for v in values if v['event'] == 'model_started'}) == 1
                assert all(r['state'] == 'completed' for r in rows(directory))
            elif case == 'invalid-batch':
                assert not (workspace / 'stream-effect.txt').exists() and len(matching) == 1
                assert not any(v['event'] in ('tool_completed', 'done') for v in values)
                assert rows(directory)[0]['state'] == 'completed', 'model receipt != tool authority'
            else:
                assert not (workspace / 'must-not-execute.txt').exists() and len(matching) == 1
                assert values[-1]['event'] == 'done' and values[-1]['status'] == 'requireshumaninput'
        print('PASS: real two-round native execution; whole-batch authority and post-attempt review stop', flush=True)

        for case in ('bad-model', 'duplicate', 'missing-usage', 'trailing', 'recover'):
            directory, workspace, config = settings(root, case)
            process, events = start(config, case)
            values = collected(process, events, success=False)
            assert not any(v['event'] == 'done' for v in values) and not (workspace / 'stream-effect.txt').exists()
            operation = admin(config, 'status')['pending']
            assert operation['state'] == 'unknown' and operation['remote_id']
            assert len([p for p in posts if p['case'] == case]) == 1
            if case == 'recover':
                remote = operation['remote_id']
                with lock:
                    receipts[remote]['usage']['total_tokens'] = 999
                rejected = subprocess.run([str(binary), 'model-calls', '--config', str(config), 'recover', operation['id']],
                                          capture_output=True, env=env, timeout=15)
                assert rejected.returncode != 0 and admin(config, 'status')['pending']['id'] == operation['id']
                with lock:
                    receipts[remote]['usage']['total_tokens'] = 5
                result = admin(config, 'recover', operation['id'])
                assert result['applied_to_turn'] is False and admin(config, 'status')['pending'] is None
                assert not (workspace / 'stream-effect.txt').exists()
        print('PASS: malformed/duplicate/trailing/incomplete streams hold; GET-only recovery never executes returned tools', flush=True)

        for case in ('terminated', 'killed', 'slow', 'closed'):
            directory, workspace, config = settings(root, case)
            controls[case] = {'ready': threading.Event(), 'release': threading.Event()}
            process, events = start(config, case, reader=case in ('terminated', 'killed'))
            if case in ('terminated', 'killed'):
                assert events.get(timeout=15)['event'] == 'model_started'
                assert events.get(timeout=15)['event'] == 'preview'
                process.send_signal(signal.SIGTERM if case == 'terminated' else signal.SIGKILL)
                if case == 'terminated':
                    # The original worker must still own the ledger while a submitted request is gated.
                    blocked = subprocess.run([str(binary), 'model-calls', '--config', str(config), 'status'],
                                             capture_output=True, env=env, timeout=15)
                    assert blocked.returncode != 0
                    assert process.poll() is None
            elif case == 'closed':
                assert json.loads(process.stdout.readline())['event'] == 'model_started'
                assert json.loads(process.stdout.readline())['event'] == 'preview'
                process.stdout.close()
            else:
                assert controls[case]['ready'].wait(15)
                # Wait for the actual stdout deadline with no reader; still no settled gateway result.
                time.sleep(6)
                assert process.poll() is None and rows(directory)[0]['state'] == 'submitting'
            controls[case]['release'].set()
            if case == 'killed':
                assert process.wait(timeout=15) == -signal.SIGKILL
                pending = admin(config, 'status')['pending']
                assert pending['state'] == 'unknown'
                assert admin(config, 'recover', pending['id'])['applied_to_turn'] is False
            else:
                collected(process, events, success=False)
                assert admin(config, 'status')['pending'] is None
                assert rows(directory)[0]['state'] == 'completed'
            assert len([p for p in posts if p['case'] == case]) == 1
            assert not (workspace / 'stream-effect.txt').exists()
        print('PASS: SIGTERM and broken/slow stdout drain one original model without tools; SIGKILL restart holds and GET-recovers', flush=True)

        directory, workspace, config = settings(root, 'disabled', ledger=False)
        process, events = start(config, 'disabled')
        values = collected(process, events, success=False)
        assert values == [] and not any(p['case'] == 'disabled' for p in posts)
        assert not (directory / 'state').exists()
        assert not failures, failures
        print('PASS: unsupported configuration rejects before state or model submission; no paid credential used', flush=True)
finally:
    for control in controls.values():
        control['release'].set()
    for process in processes:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
    server.shutdown()
    server.server_close()
