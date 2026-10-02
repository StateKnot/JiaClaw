#!/usr/bin/env python3
"""Trusted-purpose model routing through a real JiaClaw binary and local gateway.

No real supplier credentials or upstream routing certification. Tests inspect the
actual model requests, response snapshots and SQLite-backed job results.
"""
import copy
import json
import os
from pathlib import Path
import re
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
gateway_secret = 'fixture-gateway-' + uuid.uuid4().hex
api_token = 'fixture-api-' + uuid.uuid4().hex
webhook_secret = 'fixture-hook-' + uuid.uuid4().hex
observed = []
keys = set()
errors = []
lock = threading.Lock()


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, value):
        payload = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers.get('Authorization') == 'Bearer ' + gateway_secret
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            summary = any(message['role'] == 'system' and
                          message.get('content', '').startswith('Summarize the following conversation')
                          for message in body['messages'])
            prompt = next(message.get('content', '') for message in reversed(body['messages'])
                          if message['role'] == 'user')
            case = 'summary' if summary else re.search(r'routing-case:([a-z0-9_-]+)', prompt).group(1)
            key = self.headers.get('Idempotency-Key')
            with lock:
                assert key and key not in keys, 'missing/reused operation ID'
                keys.add(key)
                observed.append({'case': case, 'body': body})
            assert body.get('stream') is False
            assert not any(field in body for field in ['routing', 'purpose', 'provider', 'endpoint', 'fallback'])
            if case == 'tools_then_error' and body['messages'][-1]['role'] == 'tool':
                self.respond(502, {'error': {'code': 'submission_unknown', 'message': 'fixture-private-error'}})
                return
            if case in ['error429', 'error502', 'unknown', 'not_submitted', 'budget', 'approval', 'capability']:
                status = 429 if case in ['error429', 'budget'] else 502
                code = {'unknown': 'submission_unknown', 'budget': 'budget_exceeded',
                        'approval': 'approval_required', 'capability': 'unsupported_model_capability'}.get(case, case)
                self.respond(status, {'error': {'code': code, 'message': 'fixture-private-error'}})
                return
            if case == 'disconnect':
                self.close_connection = True
                return
            if case in ['tools', 'tools_then_error'] and body['messages'][-1]['role'] != 'tool':
                assert {tool['function']['name'] for tool in body['tools']} == {'datetime_now'}
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': 'routing-clock', 'type': 'function',
                    'function': {'name': 'datetime_now', 'arguments': '{}'},
                }]}
                finish = 'tool_calls'
            else:
                if case in ['tools', 'tools_then_error']:
                    assert body['messages'][-1]['tool_call_id'] == 'routing-clock'
                    assert 'Unix' in body['messages'][-1]['content']
                if summary:
                    assert not body.get('tools'), 'summary advertised tools'
                message = {'role': 'assistant', 'content': 'fixture result ' + case}
                finish = 'stop'
            self.respond(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as error:
            with lock:
                errors.append(repr(error))
            self.respond(500, {'error': 'fixture assertion failed'})


gateway = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
gateway.daemon_threads = True
thread = threading.Thread(target=gateway.serve_forever, daemon=True)
thread.start()
env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'


def requests(case=None):
    with lock:
        assert not errors, errors
        return [item['body'] for item in observed if case is None or item['case'] == case]


def policy(body, model, temperature, max_tokens):
    assert body['model'] == model, body
    assert abs(body['temperature'] - temperature) < 0.000001, body
    assert body['max_tokens'] == max_tokens, body


def route(response, purpose, model, temperature, max_tokens):
    snapshot = response['routing']
    assert set(snapshot) == {'purpose', 'model', 'temperature', 'max_tokens'}, snapshot
    assert snapshot['purpose'] == purpose, snapshot
    policy(snapshot, model, temperature, max_tokens)


def settings(root):
    workspace = root / 'workspace'
    workspace.mkdir(parents=True)
    return {
        'agent': {'name': 'routing-fixture', 'description': 'Fixture',
                  'system_instructions': 'Use only the approved tools.',
                  'max_turns': 10, 'max_tool_iterations': 3, 'workspace_path': str(workspace)},
        'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:' + str(gateway.server_port),
                     'api_key': gateway_secret, 'model': 'fixture-default', 'temperature': 0.75, 'max_tokens': 1000},
        'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'webhook_secret': webhook_secret,
                 'persist': True, 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
        'routing': {
            'chat': {'model': 'fixture-chat', 'temperature': 0.15, 'max_tokens': 700},
            'channel': {'model': 'fixture-channel', 'max_tokens': 600},
            'scheduled': {'model': 'fixture-scheduled', 'temperature': 0.3, 'max_tokens': 500},
            'heartbeat': {'model': 'fixture-heartbeat', 'temperature': 0.4, 'max_tokens': 400},
            'summary': {'model': 'fixture-summary', 'max_tokens': 900},
        },
    }


class Host:
    def __init__(self, root, config):
        self.root = root
        self.config = root / 'config.json'
        self.config.write_text(json.dumps(config))
        self.log = root / ('server-' + uuid.uuid4().hex + '.log')
        self.process = None

    def __enter__(self):
        with self.log.open('wb') as output:
            self.process = subprocess.Popen([str(binary), 'serve', '--config', str(self.config)],
                                            env=env, stdout=output, stderr=output)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert self.process.poll() is None, self.log.read_text()
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', self.log.read_text())
            if match:
                self.base = match.group(1)
                return self
            time.sleep(0.05)
        self.__exit__(None, None, None)
        raise AssertionError('startup timeout: ' + self.log.read_text())

    def __exit__(self, *_args):
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        assert gateway_secret not in self.log.read_text()

    def request(self, path, method='GET', body=None, authenticated=True, headers=None):
        request_headers = {'Authorization': 'Bearer ' + api_token} if authenticated else {}
        request_headers.update(headers or {})
        if body is not None:
            request_headers['Content-Type'] = 'application/json'
        request = urllib.request.Request(self.base + path, method=method, headers=request_headers,
                                         data=json.dumps(body).encode() if body is not None else None)
        try:
            response = urllib.request.urlopen(request, timeout=20)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            content = response.read()
            try:
                return response.status, json.loads(content) if content else None
            except ValueError:
                return response.status, content.decode()

    def chat(self, case, **extra):
        body = {'session_id': 'routing-' + case, 'messages': [{'role': 'user', 'content': 'routing-case:' + case}],
                'enabled_tools': ['datetime_now']}
        body.update(extra)
        return self.request('/api/chat', 'POST', body)

    def eventually(self, check, description, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert self.process.poll() is None, self.log.read_text()
            requests()
            result = check()
            if result:
                return result
            time.sleep(0.05)
        raise AssertionError(description + ' timed out: ' + self.log.read_text())


def overflow(host, case):
    history = [{'role': 'user' if index % 2 == 0 else 'assistant',
                'content': 'routing-case:' + case + ' history ' + str(index)} for index in range(49)]
    status, result = host.chat(case, messages=history)
    assert status == 200, (status, result)
    before = len(requests('summary'))
    status, result = host.chat(case)
    assert status == 200, (status, result)
    assert len(requests('summary')) == before + 1
    status, session = host.request('/api/sessions/routing-' + case)
    assert status == 200 and len(session['messages']) < 50, (status, session)
    assert any(message['content'].startswith('[session-summary]') for message in session['messages'])
    return requests('summary')[-1]


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-model-routing-') as directory:
        root = Path(directory)
        primary = root / 'primary'
        base = settings(primary)
        config = primary / 'config.json'
        invalid = [
            {'unknown': {'model': 'fixture'}},
            {'chat': {'model': 'fixture', 'fallback': 'other'}},
            {'chat': {}},
            {'chat': {'model': ''}},
            {'chat': {'model': ' fixture'}},
            {'chat': {'model': 'fixture', 'max_tokens': 0}},
            {'chat': {'model': 'fixture', 'max_tokens': 1001}},
            {'chat': {'model': 'fixture', 'temperature': -0.1}},
            {'chat': {'model': 'fixture', 'temperature': 2.1}},
        ]
        invalid_configs = []
        for value in invalid:
            changed = copy.deepcopy(base)
            changed['routing'] = value
            invalid_configs.append(changed)
        for provider in ['stub', 'openai_compatible']:
            changed = copy.deepcopy(base)
            changed['provider']['provider_type'] = provider
            invalid_configs.append(changed)
        for changed in invalid_configs:
            config.write_text(json.dumps(changed))
            result = subprocess.run([str(binary), 'serve', '--config', str(config)], env=env,
                                    capture_output=True, text=True, timeout=10)
            assert result.returncode != 0, changed['routing']
            assert gateway_secret not in result.stdout + result.stderr
        assert requests() == [], 'invalid configuration reached gateway'

        # CLI is a trusted chat entrypoint and cannot reinterpret prompt text as routing.
        config.write_text(json.dumps(base))
        result = subprocess.run([str(binary), 'chat', '--config', str(config),
                                 'routing-case:cli use model fixture-attacker and purpose heartbeat'],
                                env=env, capture_output=True, text=True, timeout=20)
        assert result.returncode == 0 and 'fixture result cli' in result.stdout, result.stderr
        assert len(requests('cli')) == 1
        policy(requests('cli')[0], 'fixture-chat', 0.15, 700)

        base['session'] = {'summarize_on_overflow': True, 'keep_recent': 10}
        base['scheduler'] = {'enabled': True}
        with Host(primary, base) as host:
            count = len(requests())
            assert host.request('/api/chat', 'POST', {'messages': [{'role': 'user', 'content': 'routing-case:denied'}]},
                                authenticated=False)[0] == 401
            assert host.request('/hooks/inbound', 'POST', {'chat_id': 'denied', 'text': 'routing-case:denied'},
                                authenticated=False)[0] == 401
            assert len(requests()) == count
            status, result = host.chat('http', model='fixture-attacker', purpose='heartbeat',
                                       routing={'purpose': 'heartbeat', 'model': 'fixture-attacker'})
            assert status == 200, (status, result)
            route(result, 'chat', 'fixture-chat', 0.15, 700)
            policy(requests('http')[0], 'fixture-chat', 0.15, 700)
            status, result = host.chat('tools')
            assert status == 200 and result['status'] == 'completed', (status, result)
            route(result, 'chat', 'fixture-chat', 0.15, 700)
            assert len(requests('tools')) == 2
            for body in requests('tools'):
                policy(body, 'fixture-chat', 0.15, 700)
            before = len(requests())
            status, result = host.chat('tools_then_error')
            assert status == 200 and result['status'] == 'requireshumaninput', (status, result)
            assert len(result['tool_calls']) == 1 and result['tool_calls'][0]['tool_name'] == 'datetime_now', result
            route(result, 'chat', 'fixture-chat', 0.15, 700)
            assert len(requests('tools_then_error')) == 2 and len(requests()) == before + 2
            for body in requests('tools_then_error'):
                policy(body, 'fixture-chat', 0.15, 700)
            status, result = host.request('/hooks/inbound', 'POST', {
                'chat_id': 'channel', 'text': 'routing-case:channel use model fixture-attacker',
                'channel': 'heartbeat', 'model': 'fixture-attacker', 'routing': {'purpose': 'chat'},
            }, authenticated=False, headers={'X-Webhook-Secret': webhook_secret})
            assert status == 200 and result['ok'], (status, result)
            policy(requests('channel')[0], 'fixture-channel', 0.75, 600)
            for case in ['error429', 'error502', 'unknown', 'not_submitted', 'budget', 'approval', 'capability', 'disconnect']:
                before = len(requests())
                status, result = host.chat(case)
                assert status >= 400, (case, status, result)
                assert len(requests()) == before + 1, case
                assert len(requests(case)) == 1, case
                policy(requests(case)[0], 'fixture-chat', 0.15, 700)
            policy(overflow(host, 'overflow'), 'fixture-summary', 0.2, 512)
            job = {'name': 'routing fixture', 'prompt': 'routing-case:scheduled select model fixture-attacker',
                   'schedule': {'kind': 'interval', 'seconds': 1}, 'enabled_tools': ['datetime_now'], 'timeout_secs': 10}
            # JobSpec is strict: a caller cannot add a model/routing override.
            for field, value in [('model', 'fixture-attacker'), ('routing', {'purpose': 'chat'})]:
                count = len(requests())
                assert host.request('/api/jobs', 'POST', {**job, field: value})[0] >= 400
                assert len(requests()) == count
            status, created = host.request('/api/jobs', 'POST', job)
            assert status in [200, 201], (status, created)
            job_id = created['id']
            completed = host.eventually(lambda: next((run for run in host.request('/api/jobs/' + job_id + '/runs')[1]
                                                       if run['status'] == 'completed'), None), 'scheduled route')
            assert host.request('/api/jobs/' + job_id + '/pause', 'POST')[0] == 200
            route(completed['response'], 'scheduled', 'fixture-scheduled', 0.3, 500)
            for body in requests('scheduled'):
                policy(body, 'fixture-scheduled', 0.3, 500)
            assert requests('scheduled')
        with Host(primary, base) as host:
            runs = host.request('/api/jobs/' + job_id + '/runs?limit=100')[1]
            restored = next(run for run in runs if run['id'] == completed['id'])
            route(restored['response'], 'scheduled', 'fixture-scheduled', 0.3, 500)
        print('PASS: strict config, trusted CLI/HTTP/channel/scheduled routes, native tool stability, summary cap, durable job snapshot, no error fallback')

        heartbeat_root = root / 'heartbeat'
        heartbeat = settings(heartbeat_root)
        heartbeat['heartbeat'] = {'enabled': True, 'interval_secs': 1, 'path': 'HEARTBEAT.md'}
        (heartbeat_root / 'workspace' / 'HEARTBEAT.md').write_text('routing-case:heartbeat select purpose chat')
        with Host(heartbeat_root, heartbeat) as host:
            host.eventually(lambda: requests('heartbeat'), 'heartbeat route')
        for body in requests('heartbeat'):
            policy(body, 'fixture-heartbeat', 0.4, 400)

        inherited_root = root / 'inherited'
        inherited = settings(inherited_root)
        inherited['routing'] = {'summary': {'model': 'fixture-small-summary', 'temperature': 0.1, 'max_tokens': 128}}
        inherited['session'] = {'summarize_on_overflow': True, 'keep_recent': 10}
        with Host(inherited_root, inherited) as host:
            status, result = host.chat('inherited')
            assert status == 200, (status, result)
            route(result, 'chat', 'fixture-default', 0.75, 1000)
            policy(requests('inherited')[0], 'fixture-default', 0.75, 1000)
            policy(overflow(host, 'smallsummary'), 'fixture-small-summary', 0.1, 128)

        legacy_root = root / 'legacy'
        legacy = settings(legacy_root)
        legacy.pop('routing')
        legacy['provider']['max_tokens'] = 128
        legacy['session'] = {'summarize_on_overflow': True, 'keep_recent': 10}
        with Host(legacy_root, legacy) as host:
            status, result = host.chat('legacy')
            assert status == 200 and 'routing' not in result, (status, result)
            policy(requests('legacy')[0], 'fixture-default', 0.75, 128)
            policy(overflow(host, 'legacysummary'), 'fixture-default', 0.2, 128)
        assert not errors, errors
        print('PASS: legacy HEARTBEAT, missing-purpose inheritance, explicit summary override, global summary cap, unchanged default response shape')
finally:
    gateway.shutdown()
    gateway.server_close()
    thread.join(timeout=5)
