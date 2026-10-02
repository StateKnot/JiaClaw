#!/usr/bin/env python3
"""Real binary + HTTP fixture acceptance for Brokerrouter native tool turns.

No paid model is called. This proves JiaClaw's wire contract, admission and
failure behavior; it does not certify a real Brokerrouter supplier/model.
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
api_token = 'fixture-host-' + uuid.uuid4().hex
upstream_private_body = 'PRIVATE-UPSTREAM-BODY-' + uuid.uuid4().hex
requests_by_case = {}
idempotency_keys = set()
fixture_errors = []
text_instruction = '```tool\n' + json.dumps({
    'tool_name': 'file_write',
    'arguments': {'path': 'text-must-not-execute.txt', 'content': 'unexpected'},
}) + '\n```'


def call(call_id, name='file_write', arguments=None):
    if arguments is None:
        arguments = {'path': 'must-not-execute.txt', 'content': 'unexpected'}
    return {'id': call_id, 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(arguments)}}


success_calls = [
    call('call-z-first', arguments={'path': 'alpha.txt', 'content': 'alpha'}),
    call('call-a-second', arguments={'path': 'beta.txt', 'content': 'beta'}),
]
success_assistant = {'role': 'assistant', 'content': None, 'tool_calls': success_calls}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, body):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + gateway_secret
            key = self.headers.get('Idempotency-Key')
            assert key and key not in idempotency_keys
            idempotency_keys.add(key)
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            case = next(message['content'] for message in request['messages'] if message['role'] == 'user')
            observed = requests_by_case.setdefault(case, [])
            observed.append(request)
            # Per-request authority controls the native catalog, too.
            assert {tool['function']['name'] for tool in request['tools']} == {'file_write'}
            assert all(tool['type'] == 'function' for tool in request['tools'])
            assert request['stream'] is False
            finish_reason = 'tool_calls'
            if case == 'success':
                if len(observed) == 1:
                    message = success_assistant
                else:
                    assert len(observed) == 2
                    assert request['messages'][-3] == success_assistant
                    results = request['messages'][-2:]
                    assert [result['role'] for result in results] == ['tool', 'tool']
                    assert [result['tool_call_id'] for result in results] == ['call-z-first', 'call-a-second']
                    assert 'alpha.txt' in results[0]['content']
                    assert 'beta.txt' in results[1]['content']
                    message = {'role': 'assistant', 'content': 'Native roundtrip completed.'}
                    finish_reason = 'stop'
            elif case == 'text_only':
                assert len(observed) == 1
                message = {'role': 'assistant', 'content': text_instruction}
                finish_reason = 'stop'
            elif case == 'http_error':
                assert len(observed) == 1  # Ambiguous submitted calls must not be retried.
                self.respond(502, {'error': {'message': upstream_private_body + ' ' + gateway_secret}})
                return
            elif case in ['error_after_write', 'invalid_after_write', 'duplicate_across_rounds', 'iteration_budget']:
                first = call(case + '-first', arguments={'path': case + '.txt', 'content': 'committed'})
                if len(observed) == 1:
                    message = {'role': 'assistant', 'content': None, 'tool_calls': [first]}
                else:
                    assert len(observed) == 2
                    assert request['messages'][-2] == {'role': 'assistant', 'content': None, 'tool_calls': [first]}
                    assert request['messages'][-1]['role'] == 'tool'
                    assert request['messages'][-1]['tool_call_id'] == first['id']
                    if case == 'error_after_write':
                        self.respond(502, {'error': {'message': upstream_private_body + ' ' + gateway_secret}})
                        return
                    if case == 'duplicate_across_rounds':
                        calls = [call(first['id'])]
                    else:
                        calls = [call(case + '-pending')]
                        if case == 'invalid_after_write':
                            calls.append(call('not-authorized-after-write', 'datetime_now', {}))
                    message = {'role': 'assistant', 'content': None, 'tool_calls': calls}
            else:
                assert len(observed) == 1, 'rejected batch unexpectedly continued'
                calls = [call('valid-before-rejection')]
                if case == 'unauthorized':
                    calls.append(call('denied-tool', 'datetime_now', {}))
                elif case == 'unknown_tool':
                    calls.append(call('unknown-tool', 'not_registered', {}))
                elif case == 'duplicate_id':
                    calls.append(call('valid-before-rejection'))
                elif case == 'empty_id':
                    calls.append(call(''))
                elif case == 'malformed_arguments':
                    invalid = call('invalid-json')
                    invalid['function']['arguments'] = '{"path":'
                    calls.append(invalid)
                elif case == 'non_object_arguments':
                    calls.append(call('array-arguments', arguments=['unexpected']))
                elif case == 'invalid_schema':
                    calls.append(call('invalid-type', arguments={'path': 'invalid.txt', 'content': 123}))
                elif case == 'too_many_calls':
                    calls = [call('bounded-' + str(index)) for index in range(33)]
                elif case == 'truncated':
                    finish_reason = 'length'
                elif case == 'missing_finish_reason':
                    finish_reason = None
                elif case == 'wrong_role':
                    pass
                else:
                    raise AssertionError('unknown fixture case: ' + case)
                message = {'role': 'user' if case == 'wrong_role' else 'assistant',
                           'content': None, 'tool_calls': calls}
            choice = {'message': copy.deepcopy(message)}
            if finish_reason is not None:
                choice['finish_reason'] = finish_reason
            self.respond(200, {'choices': [choice]})
        except Exception as error:
            fixture_errors.append(repr(error))
            self.respond(500, {'error': 'fixture assertion failed'})


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
process = None
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-native-tools-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        config = root / 'config.json'
        config.write_text(json.dumps({
            'agent': {'name': 'native-tools-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use the approved native tools.',
                      'max_turns': 10, 'max_tool_iterations': 2, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter',
                         'base_url': 'http://127.0.0.1:' + str(server.server_port),
                         'api_key': gateway_secret, 'model': 'fixture'},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token,
                     'persist': True, 'persist_path': '../state/sessions.sqlite3'},
        }))
        env = dict(os.environ)
        for name in ['JIACLAW_API_KEY', 'JIACLAW_API_TOKEN', 'JIACLAW_LOG_FORMAT',
                     'JIACLAW_MAX_TOOL_ITERATIONS',
                     'JIACLAW_WEBHOOK_SECRET', 'JIACLAW_TELEGRAM_SECRET',
                     'JIACLAW_SLACK_SIGNING_SECRET', 'JIACLAW_DISCORD_PUBLIC_KEY']:
            env.pop(name, None)
        env['JIACLAW_LOG_LEVEL'] = 'info'
        log = root / 'server.log'
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
                                       env=env, stdout=output, stderr=output)
        deadline = time.monotonic() + 20
        base = None
        while time.monotonic() < deadline:
            assert process.poll() is None, log.read_text()
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
            if match:
                base = match.group(1)
                break
            time.sleep(.05)
        assert base, log.read_text()

        def chat(case, enabled_tools=None):
            body = {'session_id': 'native-' + case,
                    'messages': [{'role': 'user', 'content': case}],
                    'enabled_tools': ['file_write'] if enabled_tools is None else enabled_tools}
            req = urllib.request.Request(base + '/api/chat', data=json.dumps(body).encode(),
                                         headers={'Authorization': 'Bearer ' + api_token,
                                                  'Content-Type': 'application/json'})
            try:
                with urllib.request.urlopen(req, timeout=20) as response:
                    return response.status, json.load(response)
            except urllib.error.HTTPError as error:
                return error.code, json.load(error)

        status, result = chat('success')
        assert status == 200, (status, result, fixture_errors)
        assert result['status'] == 'completed'
        assert result['message']['content'] == 'Native roundtrip completed.'
        assert [tool['tool_name'] for tool in result['tool_calls']] == ['file_write', 'file_write']
        assert (workspace / 'alpha.txt').read_text() == 'alpha'
        assert (workspace / 'beta.txt').read_text() == 'beta'

        status, result = chat('text_only')
        assert status == 200, (status, result)
        assert result['message']['content'] == text_instruction
        assert result['tool_calls'] == []
        assert not (workspace / 'text-must-not-execute.txt').exists()

        for case in ['unauthorized', 'unknown_tool', 'duplicate_id', 'empty_id',
                     'malformed_arguments', 'non_object_arguments', 'invalid_schema',
                     'too_many_calls', 'truncated', 'missing_finish_reason', 'wrong_role']:
            status, result = chat(case)
            assert status >= 400, (case, status, result, fixture_errors)
            assert not (workspace / 'must-not-execute.txt').exists(), case
            assert not (workspace / 'invalid.txt').exists(), case
            assert len(requests_by_case[case]) == 1, case

        # Invalid caller authority is rejected before sending a model request.
        count = sum(map(len, requests_by_case.values()))
        status, result = chat('unknown_enabled_tool', ['not_registered'])
        assert status >= 400, (status, result)
        assert sum(map(len, requests_by_case.values())) == count

        status, result = chat('http_error')
        assert status >= 400, (status, result)
        assert upstream_private_body not in json.dumps(result)
        assert gateway_secret not in json.dumps(result)
        for case in ['error_after_write', 'invalid_after_write', 'duplicate_across_rounds', 'iteration_budget']:
            status, result = chat(case)
            assert status == 200, (case, status, result, fixture_errors)
            assert result['status'] == 'requireshumaninput', (case, result)
            assert len(result['tool_calls']) == 1, (case, result)
            assert result['tool_calls'][0]['tool_name'] == 'file_write'
            assert result['tool_calls'][0]['arguments']['path'] == case + '.txt'
            assert (workspace / (case + '.txt')).read_text() == 'committed'
            assert not (workspace / 'must-not-execute.txt').exists(), case
            assert len(requests_by_case[case]) == 2, case
            assert upstream_private_body not in json.dumps(result)
            assert gateway_secret not in json.dumps(result)
            req = urllib.request.Request(base + '/api/sessions/native-' + case,
                                         headers={'Authorization': 'Bearer ' + api_token})
            with urllib.request.urlopen(req, timeout=10) as response:
                history = json.load(response)['messages']
            assert len(history) == 2, (case, history)
            assert history[-1] == result['message'], (case, history, result)

        assert gateway_secret not in log.read_text()
        assert upstream_private_body not in log.read_text()
        assert len(requests_by_case['http_error']) == 1
        assert not fixture_errors, fixture_errors
        print('PASS: native tool transcript/IDs, catalog authority, atomic batch admission, malformed responses, text non-execution, sanitized errors, post-effect interruption/history and iteration budget')
finally:
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)
