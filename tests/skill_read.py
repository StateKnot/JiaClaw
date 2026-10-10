#!/usr/bin/env python3
"""Real binary/native HTTP fixture for progressive skill reads; no paid model."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
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
token = 'fixture-host-' + uuid.uuid4().hex
gateway_key = 'fixture-model-' + uuid.uuid4().hex
original = 'REVIEWED_BODY_' + uuid.uuid4().hex + '\nUse only independently authorized tools.'
replacement = 'REPLACEMENT_BODY_' + uuid.uuid4().hex
unrelated = 'UNRELATED_BODY_' + uuid.uuid4().hex
observed = {}
errors = []
state = {}
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'


def request(path, body=None, auth=True):
    headers = {'Content-Type': 'application/json'}
    if auth:
        headers['Authorization'] = 'Bearer ' + token
    data = json.dumps(body).encode() if body is not None else None
    try:
        with urllib.request.urlopen(urllib.request.Request(state['base'] + path, data=data,
                                                          headers=headers), timeout=20) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)


def write_skill(body=original):
    directory = state['workspace'] / 'skills' / 'directory-name'
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'SKILL.md').write_text('---\nname: reviewed-skill\ndescription: choose relevant instructions\ntriggers: [request]\n---\n' + body)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, status, body):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + gateway_key
            data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            case = next(m['content'] for m in data['messages'] if m['role'] == 'user')
            rounds = observed.setdefault(case, [])
            rounds.append(data)
            catalog = {t['function']['name']: t['function'] for t in data['tools']}
            prompt = data['messages'][0]['content']
            if case == 'request-legacy':
                assert original in prompt and 'skill_read' not in catalog
                self.reply(200, {'choices': [{'message': {'role': 'assistant', 'content': 'legacy complete'}, 'finish_reason': 'stop'}]})
                return
            assert unrelated not in prompt
            assert (original in prompt) == (case == 'request-manual')
            if case != 'request-denied':
                assert 'skill_read' in catalog
                schema = catalog['skill_read']['parameters']
                assert schema['additionalProperties'] is False
                assert set(schema['required']) == {'name', 'content_sha256'}
            else:
                assert 'skill_read' not in catalog
            if len(rounds) == 1:
                descriptors = [json.loads(line[2:]) for line in prompt.splitlines()
                               if line.startswith('- {') and 'content_sha256' in line]
                descriptor = next(x for x in descriptors if x['name'] == 'reviewed-skill')
                selected = state.get('expected_body', original)
                assert descriptor['content_sha256'] == hashlib.sha256(selected.encode()).hexdigest()
                arguments = {'name': descriptor['name'], 'content_sha256': descriptor['content_sha256']}
                if case == 'request-reload-race':
                    write_skill(replacement)
                    assert request('/api/skills/reload', {})[0] == 200
                elif case == 'request-remove-race':
                    shutil.rmtree(state['workspace'] / 'skills')
                    assert request('/api/skills/reload', {})[0] == 200
                elif case == 'request-extra':
                    arguments['path'] = '../outside.txt'
                elif case == 'request-path':
                    arguments['name'] = '../outside.txt'
                call = {'id': 'read-' + case, 'type': 'function',
                        'function': {'name': 'skill_read', 'arguments': json.dumps(arguments)}}
                self.reply(200, {'choices': [{'message': {'role': 'assistant', 'content': None, 'tool_calls': [call]},
                                             'finish_reason': 'tool_calls'}]})
                return
            assert len(rounds) == 2
            assert data['messages'][-1]['role'] == 'tool'
            assert data['messages'][-1]['tool_call_id'] == 'read-' + case
            result = json.loads(data['messages'][-1]['content'])
            if case in {'request-reload-race', 'request-remove-race', 'request-path', 'request-escaped-budget'}:
                assert result['effect_status'] == 'no_effect', result
                assert original not in json.dumps(result) and replacement not in json.dumps(result)
            else:
                assert result == state.get('expected_body', original), repr(result)[:1000]
            assert unrelated not in json.dumps(result)
            if case == 'request-escalate':
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': 'forbidden-write', 'type': 'function',
                    'function': {'name': 'file_write', 'arguments': json.dumps({'path': 'forbidden.txt', 'content': 'must not run'})}}]}
                reason = 'tool_calls'
            else:
                message = {'role': 'assistant', 'content': 'progressive complete'}
                reason = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': reason}]})
        except Exception as error:
            errors.append(repr(error))
            self.reply(500, {'error': 'fixture assertion failed'})


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()


def run_host(root, progressive):
    state['workspace'] = root / 'workspace'
    state['workspace'].mkdir()
    write_skill()
    other = state['workspace'] / 'skills' / 'unrelated'
    other.mkdir()
    (other / 'SKILL.md').write_text('---\nname: unrelated\ndescription: unrelated summary\n---\n' + unrelated)
    config = root / 'config.json'
    settings = {
        'agent': {'name': 'skill-read-fixture', 'description': 'Disposable', 'system_instructions': '',
                  'workspace_path': str(state['workspace']), 'max_tool_iterations': 2, 'max_turns': 10},
        'provider': {'provider_type': 'brokerrouter', 'model': 'fixture', 'api_key': gateway_key,
                     'base_url': 'http://127.0.0.1:' + str(server.server_port)},
        'http': {'bind': '127.0.0.1:0', 'api_token': token}
    }
    if progressive:
        settings['tools'] = {'skill_read': {'enabled': True}}
    config.write_text(json.dumps(settings))
    log = root / 'server.log'
    with log.open('wb') as output:
        process = subprocess.Popen([str(binary), 'serve', '--config', str(config)], env=env,
                                   stdout=output, stderr=output)
    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert process.poll() is None, log.read_text()
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
            if match:
                state['base'] = match.group(1)
                break
            time.sleep(.05)
        else:
            raise AssertionError(log.read_text())

        def chat(case, tools=None, skills=None, auto=True, auth=True):
            body = {'messages': [{'role': 'user', 'content': case}],
                    'enabled_tools': tools if tools is not None else ['skill_read'],
                    'enabled_skills': skills or [], 'auto_skills': auto}
            return request('/api/chat', body, auth)

        if not progressive:
            assert chat('request-legacy', ['datetime_now'])[0] == 200
            assert chat('request-disabled')[0] >= 400
            assert 'request-disabled' not in observed
            print('PASS: default legacy keyword injection and disabled read rejection before model')
            return
        assert chat('request-auth', auth=False)[0] == 401
        assert 'request-auth' not in observed
        for case, options in [('request-progressive', {}), ('request-manual', {'skills': ['reviewed-skill']}),
                              ('request-no-auto', {'auto': False})]:
            status, response = chat(case, **options)
            assert status == 200 and response['status'] == 'completed', (case, status, response, errors)
            assert response['tool_calls'][0]['result'] == original
        print('PASS: actual two-round summary-only selection, manual skill compatibility and no-auto authority')
        # CLI uses the same native tool loop and default registered-tool authority.
        cli = subprocess.run([str(binary), 'chat', '--config', str(config), 'request-cli'],
                             env=env, capture_output=True, text=True, timeout=30)
        assert cli.returncode == 0 and 'progressive complete' in cli.stdout, (cli.stdout, cli.stderr, errors)
        assert len(observed['request-cli']) == 2
        print('PASS: real ordinary CLI progressive skill roundtrip')
        write_skill(replacement)
        assert chat('request-disk-only')[1]['tool_calls'][0]['result'] == original
        for case in ['request-denied', 'request-extra']:
            status, _ = chat(case, ['datetime_now'] if case == 'request-denied' else None)
            assert status >= 400 and len(observed[case]) == 1, (case, status, errors)
        status, result = chat('request-escalate')
        assert status == 200 and result['status'] == 'requireshumaninput', (status, result, errors)
        assert not (state['workspace'] / 'forbidden.txt').exists()
        assert chat('request-path')[1]['status'] == 'completed'
        print('PASS: per-request tool denial, strict arguments, name-not-path and skill text cannot grant writes')
        assert chat('request-reload-race')[1]['status'] == 'completed'
        assert observed['request-reload-race'][1]['messages'][-1]['content'].find(replacement) == -1
        # Reestablish original body before the next turn, then revoke during dispatch.
        write_skill()
        assert request('/api/skills/reload', {})[0] == 200
        assert chat('request-remove-race')[1]['status'] == 'completed'
        print('PASS: edited disk remains original; real reload/change/removal between catalog and dispatch rejects stale versions')
        for case, body in [('request-large', 'LARGE_BODY_' + 'x' * (120 * 1024)),
                           ('request-escaped-budget', '\x01' * (120 * 1024))]:
            state['expected_body'] = body
            write_skill(body)
            assert request('/api/skills/reload', {})[0] == 200
            status, response = chat(case)
            assert status == 200 and response['status'] == 'completed', (case, status, response, errors)
            assert len(observed[case]) == 2
        state.pop('expected_body')
        print('PASS: large bounded body delivered; JSON-expanded body rejected as no_effect before successful result loss')
        assert not errors, errors
        assert gateway_key not in log.read_text() and token not in log.read_text()
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-skill-read-') as directory:
        root = Path(directory)
        for mode in [False, True]:
            child = root / str(mode)
            child.mkdir()
            run_host(child, mode)
finally:
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)
