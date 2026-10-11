#!/usr/bin/env python3
"""Real CLI model verification. Disposable local gateway; no vendor credentials/billing."""
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import signal
import select
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
secret = 'probe-fixture-' + uuid.uuid4().hex
env['JIACLAW_MODEL_PROBE_MCP_TOKEN'] = secret
private_marker = 'private-workspace-' + uuid.uuid4().hex
model = 'probe-chat-route'
posts, gets, errors, cases, receipts = [], [], [], {}, {}
lock = threading.Lock()


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def write_json(self, status, value, remote=None):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        if remote:
            self.send_header('x-brokerrouter-request-id', remote)
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        try:
            case = self.path.split('/')[1]
            assert self.path == '/' + case + '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + secret
            original = self.headers['Idempotency-Key']
            assert str(uuid.UUID(original)) == original
            raw = self.rfile.read(int(self.headers['Content-Length']))
            body = json.loads(raw)
            assert body['model'] == model
            assert body['temperature'] == 0.0 and body['stream'] is False
            assert 1 <= body['max_tokens'] <= 128
            assert 'tools' not in body and 'parallel_tool_calls' not in body
            assert len(body['messages']) == 2
            assert [m['role'] for m in body['messages']] == ['system', 'user']
            challenge = body['messages'][1]['content']
            assert challenge.startswith('JIACLAW_VERIFY_') and len(challenge) == 47
            assert all(len(m['content']) <= 256 for m in body['messages'])
            assert private_marker not in json.dumps(body) and secret not in json.dumps(body)
            mode = cases[case]
            remote = str(uuid.uuid4())
            value = {'id': 'fixture-response', 'object': 'chat.completion', 'model': model,
                     'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': challenge},
                                  'finish_reason': 'stop'}],
                     'usage': {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}}
            if mode['kind'] == 'echo':
                value['choices'][0]['message']['content'] = 'wrong verification code'
            elif mode['kind'] == 'model':
                value['model'] = 'unexpected-model'
            elif mode['kind'] == 'tools':
                value['choices'][0] = {'index': 0, 'message': {'role': 'assistant', 'content': None,
                    'tool_calls': [{'id': 'fixture-tool', 'type': 'function',
                                    'function': {'name': 'file_write', 'arguments': '{}'}}]},
                    'finish_reason': 'tool_calls'}
            elif mode['kind'] == 'oversized':
                value['choices'][0]['message']['content'] = 'X' * (2 * 1024 * 1024 + 1)
            with lock:
                posts.append({'case': case, 'operation_id': original, 'remote_id': remote,
                              'body': body, 'body_hash': hashlib.sha256(raw).hexdigest()})
                receipts[remote] = value
            mode['entered'].set()
            if mode['kind'] in {'term', 'kill'}:
                data = json.dumps(value).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.send_header('x-brokerrouter-request-id', remote)
                self.end_headers()
                self.wfile.flush()
                assert mode['release'].wait(12), 'fixture release not observed'
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                mode['settled'].set()
            elif mode['kind'] == 'status':
                self.write_json(401, {'error': secret + private_marker}, remote)
            else:
                self.write_json(200, value, None if mode['kind'] == 'identity' else remote)
        except Exception as error:
            with lock:
                errors.append(type(error).__name__ + ': ' + str(error))
            self.write_json(500, {'error': 'fixture contract failure'})

    def do_GET(self):
        try:
            parts = self.path.split('/')
            assert parts[2:4] == ['v1', 'requests']
            assert self.headers['Authorization'] == 'Bearer ' + secret
            remote = parts[4]
            assert remote in receipts
            with lock:
                gets.append(self.path)
            if len(parts) == 6 and parts[5] == 'result':
                self.write_json(200, receipts[remote])
            else:
                assert len(parts) == 5
                self.write_json(200, {'id': remote, 'model': model, 'purpose': 'model', 'status': 'succeeded'})
        except Exception as error:
            with lock:
                errors.append(type(error).__name__ + ': ' + str(error))
            self.write_json(500, {'error': 'fixture contract failure'})


def snapshot(case=None):
    with lock:
        assert not errors, errors
        return [p for p in posts if case is None or p['case'] == case], list(gets)


def eventual(check, label):
    end = time.monotonic() + 8
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(.025)
    raise AssertionError(label)


def cancellation_received(process):
    """Observe actual CLI handling, not merely successful OS signal submission."""
    observed = b''
    deadline = time.monotonic() + 4
    while time.monotonic() < deadline:
        ready, _, _ = select.select([process.stderr], [], [], deadline - time.monotonic())
        if ready:
            data = os.read(process.stderr.fileno(), 65536)
            assert data, 'CLI exited before cancellation receipt'
            observed += data
            if b'Model probe cancellation received;' in observed:
                return observed
    raise AssertionError('actual CLI cancellation not observed')


def command(config, *args, probe=False, success=True, confirm=True):
    argv = [str(binary), 'model-probe' if probe else 'model-calls', '--config', str(config)]
    if probe and confirm:
        argv.append('--confirm-billing')
    completed = subprocess.run(argv + list(args), env=env, capture_output=True, text=True, timeout=20)
    text = completed.stdout + completed.stderr
    assert secret not in text and private_marker not in text, 'private content leaked'
    snapshot()
    assert (completed.returncode == 0) == success, (argv[1:], completed.returncode, text)
    return completed


def status(config):
    return json.loads(command(config, 'status').stdout)


def operation_rows(directory):
    dbpath = directory / 'state/model-calls/index.sqlite3'
    with closing(sqlite3.connect(dbpath.as_uri() + '?mode=rw', uri=True)) as db:
        db.execute('PRAGMA query_only=ON')
        db.row_factory = sqlite3.Row
        return [dict(r) for r in db.execute('SELECT id,turn_id,state,remote_id,has_tools,body_hash,receipt IS NOT NULL AS has_receipt FROM model_calls ORDER BY seq')]


def configure(root, case, kind='ok', **overrides):
    directory = root / case
    workspace = directory / 'workspace'
    workspace.mkdir(parents=True)
    for name in ['AGENTS.md', 'SOUL.md', 'USER.md', 'MEMORY.md']:
        (workspace / name).write_text(private_marker)
    (workspace / 'skills').mkdir()
    if os.name == 'posix':
        # An ambient skill read would block; this command must not discover skills.
        (workspace / 'skills' / 'must-not-read').mkdir()
        os.mkfifo(workspace / 'skills' / 'must-not-read' / 'SKILL.md')
    config = {'agent': {'name': 'model-probe-fixture', 'description': 'Local acceptance',
                       'system_instructions': private_marker, 'max_turns': 10,
                       'max_tool_iterations': 3, 'workspace_path': str(workspace)},
              'provider': {'provider_type': 'brokerrouter', 'base_url': endpoint + '/' + case,
                           'api_key': secret, 'model': 'base-model', 'max_tokens': 1024},
              'routing': {'chat': {'model': model, 'max_tokens': 64}},
              'memory': {'semantic': {'enabled': True, 'model': 'fixture-embed',
                          'space_revision': 'probe-v1', 'dimensions': 8,
                          'index_path': '../state/semantic.sqlite3'}},
              'mcp': {'servers': [{'name': 'must-not-initialize',
                      'endpoint': endpoint + '/' + case + '/mcp/', 'timeout_secs': 1,
                      'bearer_token_env': 'JIACLAW_MODEL_PROBE_MCP_TOKEN',
                      'tools': [{'name': 'private_lookup', 'alias': 'private_lookup',
                                 'descriptor_sha256': 'sha256:' + '0' * 64, 'effect': 'read_only'}]}]},
              'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
              'http': {'persist': True, 'persist_path': '../state/sessions.sqlite3'}}
    for section, fields in overrides.items():
        config.setdefault(section, {}).update(fields)
    path = directory / 'config.json'
    path.write_text(json.dumps(config))
    cases[case] = {'kind': kind, 'entered': threading.Event(), 'release': threading.Event(), 'settled': threading.Event()}
    return path, directory, config


server = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
server.daemon_threads = True
endpoint = 'http://127.0.0.1:' + str(server.server_port)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-model-probe-') as raw:
        root = Path(raw).resolve()
        cfg, directory, _ = configure(root, 'consent')
        result = command(cfg, probe=True, success=False, confirm=False)
        assert '--confirm-billing' in result.stderr and not snapshot()[0]
        assert not (directory / 'state').exists()
        for n, change in enumerate([
            {'provider': {'provider_type': 'stub'}},
            {'provider': {'provider_type': 'openai_compatible'}},
            {'model_calls': {'enabled': False}},
            {'provider': {'api_key': None}},
            {'provider': {'base_url': 'http://example.invalid/v1'}},
            {'routing': {'chat': {'model': 'bad\nmodel'}}},
        ]):
            cfg, directory, _ = configure(root, 'preflight-' + str(n), **change)
            rejected = command(cfg, probe=True, success=False)
            assert '无法解析 JSON 配置' not in rejected.stderr, 'fixture never reached model preflight'
            assert not (directory / 'state').exists() and not snapshot()[0]
        print('PASS probe 1: explicit billing/configuration gates reject before network and private state')

        cfg, directory, config = configure(root, 'valid')
        result = json.loads(command(cfg, probe=True).stdout)
        assert result['verified'] is True and result['routing'] == {'purpose': 'chat', 'model': model, 'temperature': 0.0, 'max_tokens': 64}
        submitted = snapshot('valid')[0]
        assert len(submitted) == 1 and result['operation_id'] == submitted[0]['operation_id'] and result['remote_id'] == submitted[0]['remote_id']
        stored = status(cfg)
        assert stored['pending'] is None and stored['recent'][0]['state'] == 'completed'
        assert stored['recent'][0]['has_tools'] is False and stored['recent'][0]['session_hash'] is None
        assert operation_rows(directory)[0]['body_hash'] == submitted[0]['body_hash']
        assert not (directory / 'state/sessions.sqlite3').exists()
        assert not (directory / 'state/semantic.sqlite3').exists()
        assert not snapshot()[1]
        body = json.loads(command(cfg, 'result', result['operation_id']).stdout)
        assert body['choices'][0]['message']['content'] == submitted[0]['body']['messages'][1]['content']
        cfg, directory, _ = configure(root, 'capped', routing={'chat': {'model': model, 'max_tokens': 512}})
        assert json.loads(command(cfg, probe=True).stdout)['routing']['max_tokens'] == 128
        assert len(snapshot('capped')[0]) == 1
        print('PASS probe 2: single exact Chat route, capped no-tools request excludes ambient private files; original receipt persisted')

        cfg, directory, _ = configure(root, 'wrong-echo', kind='echo')
        result = json.loads(command(cfg, probe=True, success=False).stdout)
        assert result['verified'] is False and status(cfg)['recent'][0]['state'] == 'completed'
        assert len(snapshot('wrong-echo')[0]) == 1
        print('PASS probe 3: failed verification stays distinct from completed model receipt and never retries')

        for kind in ['status', 'identity', 'model', 'tools', 'oversized']:
            cfg, directory, _ = configure(root, 'failure-' + kind, kind=kind)
            assert not command(cfg, probe=True, success=False).stdout
            stored = status(cfg)
            assert stored['pending']['state'] == 'unknown' and stored['retained_receipts'] == 0
            command(cfg, probe=True, success=False)
            assert len(snapshot('failure-' + kind)[0]) == 1
        assert not snapshot()[1]
        print('PASS probe 4: HTTP, identity, model, tool and output failures retain durable hold and block repeated POST')

        cfg, directory, _ = configure(root, 'admission')
        status(cfg)
        dbpath = directory / 'state/model-calls/index.sqlite3'
        with closing(sqlite3.connect(dbpath)) as db:
            db.execute("CREATE TRIGGER fail_probe_admit BEFORE INSERT ON model_call_audit WHEN NEW.action='admitted' BEGIN SELECT RAISE(ABORT,'fixture'); END")
            db.commit()
        command(cfg, probe=True, success=False)
        assert not snapshot('admission')[0] and not operation_rows(directory)
        print('PASS probe 5: real admission/audit rollback prevents any model POST')

        for kind in ['term', 'kill']:
            cfg, directory, _ = configure(root, kind, kind=kind)
            mode = cases[kind]
            process = subprocess.Popen([str(binary), 'model-probe', '--config', str(cfg), '--confirm-billing'],
                                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
            try:
                assert mode['entered'].wait(8), 'actual POST not observed'
                eventual(lambda: operation_rows(directory)[-1]['remote_id'], 'original remote identity not persisted')
                process.send_signal(signal.SIGTERM if kind == 'term' else signal.SIGKILL)
                observed = b''
                if kind == 'term':
                    observed = cancellation_received(process)
                    # No sleep-based owner assertion: competing real CLI must be refused.
                    command(cfg, 'status', success=False)
                    assert process.poll() is None, 'caller released owner before receipt settlement'
                mode['release'].set()
                stdout, stderr = process.communicate(timeout=10)
                stderr = (observed + stderr).decode()
                assert process.returncode != 0 and not stdout, (kind, process.returncode, len(stdout), stderr)
                assert secret not in stderr and private_marker not in stderr
                assert mode['settled'].wait(5)
            finally:
                mode['release'].set()
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)
            stored = status(cfg)
            assert len(snapshot(kind)[0]) == 1
            if kind == 'term':
                assert stored['pending'] is None and stored['recent'][0]['state'] == 'completed'
                print('PASS probe 6: actual SIGTERM preserves exclusive receipt owner until real response settlement; no success delivery or replay')
            else:
                pending = stored['pending']
                assert pending['state'] == 'unknown'
                command(cfg, probe=True, success=False)
                assert len(snapshot(kind)[0]) == 1
                recovered = json.loads(command(cfg, 'recover', pending['id']).stdout)
                assert recovered['recovered'] is True and recovered['applied_to_turn'] is False
                assert status(cfg)['pending'] is None and len(snapshot(kind)[0]) == 1
                assert len(snapshot()[1]) == 2
                print('PASS probe 7: actual SIGKILL retains original identity on restart; GET-only recovery sends no new model POST')
        snapshot()
finally:
    for mode in cases.values():
        mode['release'].set()
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)
