#!/usr/bin/env python3
"""Real-binary model receipt and GET-only recovery acceptance.

Every endpoint is a disposable localhost fixture. No paid model, supplier key,
remote service, tool-loop recovery or billing certification is involved.
"""
import copy
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
import urllib.request
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
secret = 'model-call-fixture-' + uuid.uuid4().hex
rotated_secret = 'model-call-rotated-' + uuid.uuid4().hex
api_token = 'model-call-host-' + uuid.uuid4().hex
model = 'fixture-chat-v1'
mcp_secret = 'model-call-mcp-' + uuid.uuid4().hex
env['MODEL_CALL_FIXTURE_MCP_TOKEN'] = mcp_secret
mcp_methods = []
mcp_descriptor = {'name': 'probe', 'description': 'Read a local fixture counter.',
                  'inputSchema': {'type': 'object', 'properties': {}, 'additionalProperties': False}}
lock = threading.Lock()
posts, gets, errors = [], [], []
behaviors, receipts, remote_states = {}, {}, {}
server_events = []


def server_event(case, event):
    with lock:
        server_events.append({'case': case, 'event': event, 'at': round(time.monotonic(), 4)})


def eventually(check, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.05)
    raise AssertionError(description + ' timed out')


def snapshot():
    with lock:
        assert not errors, errors
        return copy.deepcopy(posts), list(gets)


def response(body, case, tools=False):
    if tools:
        name = 'mcp_receipt_probe' if case == 'cancelled' else 'file_write'
        arguments = {} if case == 'cancelled' else {'path': case + '.txt', 'content': 'fixture tool output'}
        message = {'role': 'assistant', 'content': None, 'tool_calls': [{
            'id': 'call-' + case, 'type': 'function', 'function': {'name': name,
            'arguments': json.dumps(arguments)}}]}
    else:
        message = {'role': 'assistant', 'content': 'fixture receipt ' + case}
    return {'id': 'fixture-' + uuid.uuid4().hex, 'object': 'chat.completion',
            'model': body['model'], 'choices': [{'index': 0, 'message': message,
            'finish_reason': 'tool_calls' if tools else 'stop'}],
            'usage': {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}}


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, status, value, remote_id=None):
        payload = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        if remote_id is not None:
            self.send_header('x-brokerrouter-request-id', remote_id)
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        case = 'unclassified'
        try:
            if self.path == '/mcp/':
                assert self.headers.get('Authorization') == 'Bearer ' + mcp_secret
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                method = request['method']
                with lock:
                    mcp_methods.append(method)
                if method == 'server/discover':
                    result = {'resultType': 'complete', 'supportedVersions': ['2026-07-28'],
                              'capabilities': {'tools': {}}, 'ttlMs': 0, 'cacheScope': 'private',
                              '_meta': {'io.modelcontextprotocol/serverInfo': {'name': 'fixture', 'version': '1'}}}
                elif method == 'tools/list':
                    result = {'resultType': 'complete', 'tools': [mcp_descriptor]}
                elif method == 'tools/call':
                    assert request['params'] == {'name': 'probe', 'arguments': {}}
                    result = {'resultType': 'complete', 'content': [{'type': 'text', 'text': 'fixture probe'}],
                              'isError': False}
                else:
                    raise AssertionError(method)
                self.reply(200, {'jsonrpc': '2.0', 'id': request['id'], 'result': result})
                return
            assert self.path == '/v1/chat/completions', self.path
            assert self.headers.get('Authorization') in {'Bearer ' + secret, 'Bearer ' + rotated_secret}
            key = self.headers.get('Idempotency-Key')
            raw = self.rfile.read(int(self.headers['Content-Length']))
            body = json.loads(raw)
            assert body['stream'] is False
            summary = any(message['role'] == 'system' and message.get('content', '').startswith(
                'Summarize the following conversation') for message in body['messages'])
            prompt = next(message.get('content', '') for message in reversed(body['messages'])
                          if message['role'] == 'user')
            case = 'summary' if summary else re.search(r'calls-case:([a-z0-9_-]+)', prompt).group(1)
            followup = body['messages'][-1]['role'] == 'tool'
            tool = case in {'native', 'recover_tool', 'cancelled'} and not followup
            receipt = response(body, case, tool)
            remote_id = str(uuid.uuid4())
            with lock:
                assert key and key not in {item['key'] for item in posts}, 'model POST replayed'
                posts.append({'case': case, 'key': key, 'body_hash': hashlib.sha256(raw).hexdigest(),
                              'body': body, 'remote_id': remote_id})
                receipts[remote_id] = receipt
                remote_states[remote_id] = 'succeeded'
                behavior = behaviors.get(case)
            if summary:
                assert not body.get('tools'), 'summary advertised tools'
            if tool:
                expected_tool = 'mcp_receipt_probe' if case == 'cancelled' else 'file_write'
                assert {item['function']['name'] for item in body['tools']} == {expected_tool}
            if isinstance(behavior, tuple):
                submitted, released = behavior
                server_event(case, 'gate_wait')
                submitted.set()
                assert released.wait(20), 'fixture response was not released'
                server_event(case, 'gate_released')
            if behavior == 'disconnect':
                self.close_connection = True
                self.connection.shutdown(socket.SHUT_RDWR)
                self.connection.close()
            elif behavior == 'invalid':
                self.reply(200, {'model': body['model'], 'choices': []}, remote_id)
            elif behavior == 'missing_id':
                self.reply(200, receipt)
            else:
                self.reply(200, receipt, remote_id)
                server_event(case, 'response_sent')
        except (BrokenPipeError, ConnectionResetError) as error:
            server_event(case, type(error).__name__)  # Expected only in SIGKILL injection.
        except Exception as error:
            server_event(case, type(error).__name__)
            with lock:
                errors.append(type(error).__name__ + ': ' + str(error))
            try:
                self.reply(500, {'error': 'fixture assertion failed'})
            except OSError:
                pass

    def do_GET(self):
        try:
            assert self.headers.get('Authorization') == 'Bearer ' + secret
            match = re.fullmatch(r'/v1/requests/([0-9a-f-]{36})(/result)?', self.path)
            assert match, self.path
            remote_id, result = match.groups()
            with lock:
                gets.append(self.path)
                receipt = copy.deepcopy(receipts[remote_id])
                state = remote_states[remote_id]
            if result:
                # Fixed upstream api.rs returns Json<Value>, without the POST UUID header.
                assert state == 'succeeded'
                self.reply(200, receipt)
            else:
                self.reply(200, {'id': remote_id, 'model': receipt['model'], 'purpose': 'model',
                                'status': state, 'currency': 'CNY', 'reserved_cny': '0',
                                'cost_cny': '0', 'attempts': []})
        except Exception as error:
            with lock:
                errors.append(type(error).__name__ + ': ' + str(error))
            self.reply(500, {'error': 'fixture assertion failed'})


gateway = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
gateway.daemon_threads = True
threading.Thread(target=gateway.serve_forever, daemon=True).start()


def settings(root, name, enabled=True):
    directory = root / name
    directory.mkdir(mode=0o700)
    workspace = directory / 'workspace'
    workspace.mkdir(mode=0o700)
    # Existing session deployments may have a 0755 state parent; the ledger
    # must create its own private child without chmodding unrelated state.
    (directory / 'state').mkdir(mode=0o755)
    (directory / 'state').chmod(0o755)
    config = {
        'agent': {'name': 'model-calls-fixture', 'description': 'Offline acceptance',
                  'system_instructions': 'Use only explicitly approved tools.', 'max_turns': 10,
                  'max_tool_iterations': 3, 'workspace_path': str(workspace)},
        'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{gateway.server_port}',
                     'api_key': secret, 'model': model},
        'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
        'model_calls': {'enabled': enabled, 'store_path': '../state/model-calls/index.sqlite3'},
    }
    path = directory / 'config.json'
    path.write_text(json.dumps(config))
    return directory, workspace, path, config


def cli(path, *arguments, fail=None):
    completed = subprocess.run([str(binary), 'model-calls', '--config', str(path), *arguments],
                               env=env, capture_output=True, text=True, timeout=20)
    text = completed.stdout + completed.stderr
    assert all(token not in text for token in (secret, rotated_secret, api_token, mcp_secret)), 'credential leaked'
    snapshot()
    if fail is not None:
        assert completed.returncode != 0, (arguments, text)
        assert fail in text, (arguments, fail, text)
        return text
    assert completed.returncode == 0, (arguments, text)
    return json.loads(completed.stdout)


def operations(directory):
    # Query-only inspection verifies actual durable hashes. mode=rw opens only
    # an existing DB and lets SQLite create WAL bookkeeping after a checkpoint;
    # some SQLite builds cannot reopen WAL files with mode=ro and no sidecars.
    # query_only prevents SQL mutations. Never use immutable on a live worker DB.
    path = directory / 'state/model-calls/index.sqlite3'
    assert path.is_file(), 'model-call database missing at configured private path'
    with closing(sqlite3.connect(path.as_uri() + '?mode=rw', uri=True)) as db:
        db.execute('PRAGMA query_only=ON')
        db.row_factory = sqlite3.Row
        return [dict(row) for row in db.execute('SELECT id,turn_id,purpose,session_hash,round,model,'
                'body_hash,remote_id,state,recovered,receipt IS NOT NULL AS has_receipt FROM model_calls ORDER BY seq')]


def receipt_diagnostics(directory, host, submitted, released):
    # Whitelisted state only: no HTTP body, prompt, response, token, note or path.
    details = {'host_exit': host.process.poll(), 'sqlite_version': sqlite3.sqlite_version,
               'submitted': submitted.is_set(), 'released': released.is_set()}
    try:
        fields = ('id', 'turn_id', 'purpose', 'round', 'remote_id', 'state', 'recovered', 'has_receipt')
        details['operations'] = [{key: row[key] for key in fields} for row in operations(directory)[-4:]]
    except Exception as error:
        details['database_error'] = type(error).__name__
    with lock:
        details['post_count'] = sum(item['case'] == 'cancelled' for item in posts)
        details['get_count'] = len(gets)
        details['mcp_calls'] = mcp_methods.count('tools/call')
        details['server_errors'] = [error.split(':', 1)[0] for error in errors[-4:]]
        details['server_events'] = [item for item in server_events if item['case'] == 'cancelled'][-8:]
    markers = ('scheduler worker failed', 'model worker failed', 'needs_review',
               'session_storage_error', '会话存储失败', '会话提交成功', 'received signal',
               '收到关闭信号', 'HTTP 服务已启动', '服务器已关闭')
    log = host.log.read_text()
    # Save only known event labels/counts; never echo the free-form host log.
    details['host_log_events'] = {marker: log.count(marker) for marker in markers if marker in log}
    return json.dumps(details, sort_keys=True)


def wait_for_receipt(directory, host, submitted, released):
    def completed():
        row = operations(directory)[0]
        if row['state'] == 'unknown' or host.process.poll() is not None:
            raise AssertionError('receipt worker stopped before completion')
        return row['state'] == 'completed'
    try:
        eventually(completed, 'detached receipt commit')
    except Exception as error:
        raise AssertionError(str(error) + '; diagnostics=' + receipt_diagnostics(
            directory, host, submitted, released)) from error


def ledger_matches(directory, expected):
    rows = operations(directory)
    assert len(rows) == len(expected), (rows, expected)
    by_id = {row['id']: row for row in rows}
    for item in expected:
        row = by_id[item['key']]
        assert row['body_hash'] == item['body_hash'], row
        assert row['model'] == item['body']['model'], row
        assert str(uuid.UUID(row['id'])) == row['id'], row
    return rows


class Host:
    def __init__(self, directory, config_path):
        self.directory, self.config_path = directory, config_path
        self.process = None
        self.log = directory / ('host-' + uuid.uuid4().hex + '.log')

    def __enter__(self):
        with self.log.open('wb') as output:
            self.process = subprocess.Popen([str(binary), 'serve', '--config', str(self.config_path)],
                                            env=env, stdout=output, stderr=output)
        def started():
            assert self.process.poll() is None, self.log.read_text()
            found = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', self.log.read_text())
            return found.group(1) if found else None
        try:
            self.base = eventually(started, 'host startup')
        except Exception:
            self.__exit__()
            raise
        return self

    def __exit__(self, *_args):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        assert all(token not in self.log.read_text() for token in (secret, rotated_secret, api_token, mcp_secret))

    def request(self, path, method='GET', body=None):
        headers = {'Authorization': 'Bearer ' + api_token}
        if body is not None:
            headers['Content-Type'] = 'application/json'
        request = urllib.request.Request(self.base + path, method=method, headers=headers,
                                         data=json.dumps(body).encode() if body is not None else None)
        try:
            reply = urllib.request.urlopen(request, timeout=20)
        except urllib.error.HTTPError as error:
            reply = error
        with reply:
            payload = reply.read()
            return reply.status, json.loads(payload) if payload else None

    def chat(self, case, tools=False, **extra):
        body = {'session_id': 'calls-' + case, 'messages': [{'role': 'user', 'content': 'calls-case:' + case}],
                'enabled_tools': ['file_write'] if tools else [], 'auto_skills': False}
        body.update(extra)
        return self.request('/api/chat', 'POST', body)


def clear(path, operation):
    before = snapshot()
    cli(path, 'review-clear', operation, '--note', 'fixture independent reconciliation', fail='')
    assert cli(path, 'review-clear', operation, '--note', 'fixture independent reconciliation',
               '--confirm-reconciled') == {'operation_id': operation, 'state': 'cleared', 'recovered': False}
    assert snapshot() == before, 'review-clear made a network call'
    assert cli(path, 'status')['pending'] is None


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-model-calls-') as temporary:
        root = Path(temporary).resolve()
        directory, workspace, path, config = settings(root, 'disabled', False)
        with Host(directory, path) as host:
            assert host.chat('disabled')[0] == 200
        before = snapshot()
        cli(path, 'status', fail='')
        assert snapshot() == before
        assert not (directory / 'state/model-calls/index.sqlite3').exists()
        print('PASS: disabled ledger preserves chat and creates no receipt database')

        directory, workspace, path, config = settings(root, 'receipts')
        config['session'] = {'summarize_on_overflow': True, 'keep_recent': 10}
        config['routing'] = {'summary': {'model': 'fixture-summary'}}
        path.write_text(json.dumps(config))
        start = len(snapshot()[0])
        with Host(directory, path) as host:
            assert host.chat('native', tools=True)[0] == 200
            assert (workspace / 'native.txt').read_text() == 'fixture tool output'
            before = snapshot()
            cli(path, 'status', fail='another process owns')
            assert snapshot() == before
            history = [{'role': 'user' if i % 2 == 0 else 'assistant',
                        'content': f'calls-case:overflow PRIVATE-PROMPT-{i}'} for i in range(49)]
            assert host.chat('overflow', messages=history)[0] == 200
            assert host.chat('overflow')[0] == 200
        observed = snapshot()[0][start:]
        rows = ledger_matches(directory, observed)
        assert all(row['state'] == 'completed' and row['has_receipt'] for row in rows)
        native = [row for row, item in zip(rows, observed) if item['case'] == 'native']
        assert len(native) == 2 and len({row['turn_id'] for row in native}) == 1
        assert [row['round'] for row in native] == [0, 1]
        summary = [row for row in rows if row['purpose'] == 'summary']
        assert len(summary) == 1 and summary[0]['model'] == 'fixture-summary'
        before = snapshot()
        status = cli(path, 'status')
        assert status['pending'] is None and status['retained_receipts'] == len(rows)
        assert [item['id'] for item in status['recent']] == [row['id'] for row in reversed(rows)]
        assert all(item['body_hash'] == next(row['body_hash'] for row in rows if row['id'] == item['id'])
                   for item in status['recent'])
        assert cli(path, 'result', rows[-1]['id'])['model'] == model
        assert snapshot() == before, 'local result/status accessed the network'
        database = directory / 'state/model-calls/index.sqlite3'
        assert stat.S_IMODE(database.stat().st_mode) == 0o600
        assert stat.S_IMODE(database.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE((directory / 'state').stat().st_mode) == 0o755
        assert b'PRIVATE-PROMPT-' not in database.read_bytes(), 'raw request persisted in ledger'
        assert 'fixture receipt' not in json.dumps(status), 'status exposed response body'
        print('PASS: exact POST identity/hash, native round receipts, summary, private storage and restart inspection')

        directory, workspace, path, config = settings(root, 'summary-hold')
        config['session'] = {'summarize_on_overflow': True, 'keep_recent': 10}
        path.write_text(json.dumps(config))
        with Host(directory, path) as host:
            history = [{'role': 'user' if i % 2 == 0 else 'assistant',
                        'content': f'calls-case:summary_hold history {i}'} for i in range(49)]
            assert host.chat('summary_hold', messages=history)[0] == 200
            with lock:
                behaviors['summary'] = 'invalid'
            count = len(snapshot()[0])
            assert host.chat('summary_hold')[0] >= 400
            new = snapshot()[0][count:]
            assert len(new) == 1 and new[0]['case'] == 'summary', 'summary failure continued into chat POST'
        pending = cli(path, 'status')['pending']
        assert pending['purpose'] == 'summary' and pending['state'] == 'unknown'
        before = snapshot()
        with Host(directory, path) as host:
            assert host.chat('after-summary-hold')[0] >= 400
        assert snapshot() == before
        clear(path, pending['id'])
        with lock:
            del behaviors['summary']
        print('PASS: uncertain summary persists hold and blocks compaction fallback from submitting another call')

        directory, workspace, path, config = settings(root, 'known-remote')
        with lock:
            behaviors['recover_tool'] = 'invalid'
        start = len(snapshot()[0])
        with Host(directory, path) as host:
            assert host.chat('recover_tool', tools=True)[0] >= 400
            session_before = host.request('/api/sessions/calls-recover_tool')
            count = len(snapshot()[0])
            status, body = host.chat('blocked')
            assert status >= 400 and 'needs_review' in json.dumps(body)
            assert len(snapshot()[0]) == count
        original = ledger_matches(directory, snapshot()[0][start:])[0]
        pending = cli(path, 'status')['pending']
        assert pending['id'] == original['id'] and pending['state'] == 'unknown'
        assert pending['remote_id'] == original['remote_id']
        with Host(directory, path) as host:
            assert host.chat('restart-blocked')[0] >= 400
        changed = copy.deepcopy(config)
        changed['provider']['api_key'] = rotated_secret
        changed['provider']['model'] = 'fixture-another-model'
        path.write_text(json.dumps(changed))
        before = snapshot()
        cli(path, 'recover', original['id'], fail='original endpoint and virtual key')
        with Host(directory, path) as host:
            assert host.chat('changed-binding')[0] >= 400
        assert snapshot() == before, 'binding change bypassed hold'
        path.write_text(json.dumps(config))
        with lock:
            remote_states[original['remote_id']] = 'submission_unknown'
        cli(path, 'recover', original['id'], fail='hold retained')
        assert cli(path, 'status')['pending']['id'] == original['id']
        with lock:
            remote_states[original['remote_id']] = 'succeeded'
        count = len(snapshot()[0])
        recovered = cli(path, 'recover', original['id'])
        assert recovered == {'operation_id': original['id'], 'state': 'completed',
                             'recovered': True, 'applied_to_turn': False}
        result = cli(path, 'result', original['id'])
        assert result['choices'][0]['message']['tool_calls'][0]['function']['name'] == 'file_write'
        assert len(snapshot()[0]) == count and not (workspace / 'recover_tool.txt').exists()
        assert snapshot()[1][-2:] == [f"/v1/requests/{original['remote_id']}",
                                     f"/v1/requests/{original['remote_id']}/result"]
        restored = operations(directory)[0]
        assert restored['id'] == original['id'] and restored['body_hash'] == original['body_hash']
        assert restored['recovered'] == 1 and cli(path, 'status')['pending'] is None
        with Host(directory, path) as host:
            assert host.request('/api/sessions/calls-recover_tool') == session_before
        print('PASS: restart and changed bindings retain hold; known UUID GET-only recovery never applies tools/session')

        for case in ('missing_id', 'disconnect'):
            directory, workspace, path, config = settings(root, case)
            with lock:
                behaviors[case] = case
            with Host(directory, path) as host:
                assert host.chat(case)[0] >= 400
            pending = cli(path, 'status')['pending']
            assert pending['state'] == 'unknown' and pending['remote_id'] is None
            before = snapshot()
            cli(path, 'recover', pending['id'], fail='remote request ID unavailable')
            with Host(directory, path) as host:
                assert host.chat('cannot-replay')[0] >= 400
            assert snapshot() == before
            clear(path, pending['id'])
            with Host(directory, path) as host:
                assert host.chat('new-after-review')[0] == 200
            assert len(snapshot()[0]) == len(before[0]) + 1
        print('PASS: missing header/disconnect cannot be recovered by POST; explicit review is audited and permits a new call')

        directory, workspace, path, config = settings(root, 'killed')
        submitted, released = threading.Event(), threading.Event()
        with lock:
            behaviors['killed'] = (submitted, released)
        interrupted = []
        with Host(directory, path) as host:
            def chat_until_killed():
                try:
                    interrupted.append(host.chat('killed'))
                except (OSError, ValueError) as error:
                    interrupted.append(type(error).__name__)
            caller = threading.Thread(target=chat_until_killed, daemon=True)
            caller.start()
            assert submitted.wait(10), 'model did not receive call before SIGKILL'
            admitted = operations(directory)[0]
            assert admitted['state'] == 'submitting'
            host.process.kill()
            host.process.wait(timeout=5)
            released.set()
            caller.join(timeout=5)
            assert not caller.is_alive()
        before = snapshot()
        pending = cli(path, 'status')['pending']
        assert pending['id'] == admitted['id'] and pending['state'] == 'unknown'
        assert operations(directory)[0]['body_hash'] == admitted['body_hash']
        with Host(directory, path) as host:
            assert host.chat('after-sigkill')[0] >= 400
        assert snapshot() == before, 'restart resubmitted killed call'
        clear(path, admitted['id'])
        print('PASS: SIGKILL after actual model submission preserves the original unknown operation without replay')

        directory, workspace, path, config = settings(root, 'cancelled')
        config['scheduler'] = {'enabled': True}
        # The fixed descriptor is entirely ASCII/integer-free, so sorted compact
        # JSON is its RFC 8785 representation; never approve arbitrary discovery.
        descriptor_hash = 'sha256:' + hashlib.sha256(json.dumps(
            mcp_descriptor, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        config['mcp'] = {'servers': [{'name': 'receipt',
            'endpoint': f'http://127.0.0.1:{gateway.server_port}/mcp/',
            'bearer_token_env': 'MODEL_CALL_FIXTURE_MCP_TOKEN',
            'tools': [{'name': 'probe', 'alias': 'probe', 'effect': 'read_only',
                       'descriptor_sha256': descriptor_hash}]}]}
        path.write_text(json.dumps(config))
        submitted, released = threading.Event(), threading.Event()
        with lock:
            behaviors['cancelled'] = (submitted, released)
        start = len(snapshot()[0])
        with Host(directory, path) as host:
            status, job = host.request('/api/jobs', 'POST', {'name': 'cancel-parent',
                'prompt': 'calls-case:cancelled', 'schedule': {'kind': 'interval', 'seconds': 1},
                'enabled_tools': ['mcp_receipt_probe'], 'timeout_secs': 1})
            assert status in (200, 201), (status, job)
            assert submitted.wait(10), 'scheduled call did not reach model'
            def interrupted_run():
                status, runs = host.request('/api/jobs/' + job['id'] + '/runs')
                assert status == 200
                return next((run for run in runs if run['status'] == 'interrupted'), None)
            interrupted_run = eventually(interrupted_run, 'parent timeout before receipt')
            assert operations(directory)[0]['state'] == 'submitting'
            ledger_path = directory / 'state/model-calls/index.sqlite3'
            assert all(Path(str(ledger_path) + suffix).is_file() for suffix in ('-wal', '-shm')), \
                'query-only observer removed the active ledger WAL/SHM'
            before = snapshot()
            status, blocked = host.chat('while-worker-active')
            assert status >= 400 and len(snapshot()[0]) == len(before[0])
            released.set()
            wait_for_receipt(directory, host, submitted, released)
            with lock:
                assert mcp_methods.count('tools/call') == 0, 'cancelled parent executed returned MCP tool'
            assert not interrupted_run['response']['tool_calls'], interrupted_run
            assert host.request('/api/jobs/' + job['id'] + '/runs')[1][0]['id'] == interrupted_run['id']
        rows = ledger_matches(directory, snapshot()[0][start:])
        assert len(rows) == 1 and rows[0]['purpose'] == 'scheduled' and rows[0]['state'] == 'completed'
        assert cli(path, 'status')['pending'] is None
        assert cli(path, 'result', rows[0]['id'])['choices'][0]['finish_reason'] == 'tool_calls'
        print('PASS: parent cancellation retains worker/receipt ownership and never executes returned tools')
finally:
    with lock:
        for behavior in behaviors.values():
            if isinstance(behavior, tuple):
                behavior[1].set()
    gateway.shutdown()
    gateway.server_close()

snapshot()
print('Model-call acceptance passed (7 groups; localhost fixtures only).')
