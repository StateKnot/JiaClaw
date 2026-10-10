#!/usr/bin/env python3
"""Real native/CLI reads of declared reference bytes; synthetic local files/model only."""
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
from host_log import read_running_log

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
token, model_key = uuid.uuid4().hex, uuid.uuid4().hex
body = 'APPROVED_INSTRUCTIONS_' + uuid.uuid4().hex
original = 'APPROVED_REFERENCE_' + uuid.uuid4().hex + '\nUse only independently authorized tools.\n'
state, observed, errors = {}, {}, []
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()


def request(path, value=None, auth=True):
    headers = {'Content-Type': 'application/json'}
    if auth:
        headers['Authorization'] = 'Bearer ' + token
    try:
        with urllib.request.urlopen(urllib.request.Request(state['base'] + path, headers=headers,
                data=json.dumps(value).encode() if value is not None else None), timeout=20) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)


def write_skill(hash_value=None, content=body, resources=None):
    if resources is None:
        resources = [{'path': state.get('reference_path', 'references/guide.md'), 'sha256': hash_value or digest(state.get('expected', original))}]
    (state['skill'] / 'SKILL.md').write_text('---\nname: declared-skill\ndescription: reviewed catalog\n'
        'jiaclaw_resources: ' + json.dumps(resources) + '\n---\n' + content)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, code, value):
        raw = json.dumps(value).encode()
        self.send_response(code)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + model_key
            data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            case = next(m['content'] for m in data['messages'] if m['role'] == 'user')
            rounds = observed.setdefault(case, [])
            rounds.append(data)
            prompt = data['messages'][0]['content']
            assert original not in prompt and body not in prompt
            tools = {t['function']['name']: t['function'] for t in data['tools']}
            descriptor = next(json.loads(line[2:]) for line in prompt.splitlines()
                              if line.startswith('- {') and 'declared-skill' in line)
            assert descriptor['content_sha256'] == digest(body)
            reference = descriptor['resources'][0]
            assert reference == {'path': state.get('reference_path', 'references/guide.md'),
                                 'sha256': state.get('expected_hash', digest(original))}
            if case != 'resource-denied':
                assert set(tools['skill_resource_read']['parameters']['required']) == {
                    'name', 'content_sha256', 'path', 'resource_sha256'}
                assert tools['skill_resource_read']['parameters']['additionalProperties'] is False
            else:
                assert 'skill_resource_read' not in tools
            call_args = {'name': descriptor['name'], 'content_sha256': descriptor['content_sha256'],
                         'path': reference['path'], 'resource_sha256': reference['sha256']}
            if len(rounds) == 1 and case not in {'resource-denied', 'resource-extra'}:
                name, args = 'skill_read', {k: call_args[k] for k in ['name', 'content_sha256']}
            elif len(rounds) == 2 or case in {'resource-denied', 'resource-extra'}:
                if len(rounds) == 2:
                    assert data['messages'][-1]['role'] == 'tool'
                    assert json.loads(data['messages'][-1]['content']) == body
                name, args = 'skill_resource_read', call_args
                if case == 'resource-changed-declaration':
                    write_skill('a' * 64)
                    assert request('/api/skills/reload', {})[0] == 200
                elif case == 'resource-changed-body':
                    write_skill(content='REPLACED_INSTRUCTIONS')
                    assert request('/api/skills/reload', {})[0] == 200
                elif case == 'resource-revoked':
                    write_skill(resources=[])
                    assert request('/api/skills/reload', {})[0] == 200
                elif case == 'resource-undeclared':
                    args['path'] = 'references/undeclared.md'
                elif case == 'resource-traversal':
                    args['path'] = 'references/../SKILL.md'
                elif case == 'resource-extra':
                    args['url'] = 'https://example.invalid/resource'
            else:
                assert len(rounds) == 3
                assert data['messages'][-1]['role'] == 'tool'
                assert data['messages'][-1]['tool_call_id'] == 'resource-call-' + case
                result = json.loads(data['messages'][-1]['content'])
                if case in state.get('failures', set()):
                    assert result['effect_status'] == 'no_effect', result
                    assert original not in json.dumps(result)
                else:
                    assert result == state.get('expected', original)
                if case == 'resource-poison':
                    name, args = 'file_write', {'path': 'forbidden.txt', 'content': 'do not write'}
                else:
                    self.reply(200, {'choices': [{'message': {'role': 'assistant', 'content': 'resources complete'},
                                                 'finish_reason': 'stop'}]})
                    return
            call = {'id': ('body-call-' if name == 'skill_read' else 'resource-call-') + case,
                    'type': 'function', 'function': {'name': name, 'arguments': json.dumps(args)}}
            self.reply(200, {'choices': [{'message': {'role': 'assistant', 'content': None, 'tool_calls': [call]},
                                         'finish_reason': 'tool_calls'}]})
        except Exception as error:
            errors.append(repr(error))
            self.reply(500, {'error': 'fixture assertion failed'})


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-resource-') as temporary:
        root = Path(temporary)
        state['workspace'] = root / 'workspace'
        state['skill'] = state['workspace'] / 'skills' / 'folder-name'
        references = state['skill'] / 'references'
        references.mkdir(parents=True)
        leaf = references / 'guide.md'
        leaf.write_text(original)
        write_skill()
        config = root / 'config.json'
        settings = {'agent': {'name': 'resource-fixture', 'description': 'Disposable', 'system_instructions': '',
                    'max_turns': 10, 'max_tool_iterations': 3, 'workspace_path': str(state['workspace'])},
            'provider': {'provider_type': 'brokerrouter', 'model': 'fixture', 'api_key': model_key,
                         'base_url': 'http://127.0.0.1:' + str(server.server_port)},
            'http': {'bind': '127.0.0.1:0', 'api_token': token},
            'tools': {'skill_read': {'enabled': False, 'resources_enabled': True}}}
        config.write_text(json.dumps(settings))
        bad = subprocess.run([str(binary), 'chat', '--config', str(config), 'invalid-combination'],
                             env=env, capture_output=True, text=True, timeout=10)
        assert bad.returncode != 0 and 'requires skill_read.enabled' in bad.stderr
        assert 'invalid-combination' not in observed
        settings['tools']['skill_read'] = {'enabled': True}
        config.write_text(json.dumps(settings))

        def stop_host(process):
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

        def start_host(name):
            log = root / (name + '.log')
            with log.open('wb') as output:
                process = subprocess.Popen([str(binary), 'serve', '--config', str(config)], env=env,
                                           stdout=output, stderr=output)
            try:
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    assert process.poll() is None, log.read_text()
                    match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', read_running_log(log))
                    if match:
                        state['base'] = match.group(1)
                        return process, log
                    time.sleep(.05)
                raise AssertionError(log.read_text())
            except BaseException:
                stop_host(process)
                raise

        disabled, _ = start_host('disabled-server')
        try:
            status, _ = request('/api/chat', {'messages':[{'role':'user','content':'resource-disabled'}],
                                            'enabled_tools':['skill_resource_read']})
            assert status >= 400 and 'resource-disabled' not in observed
        finally:
            stop_host(disabled)
        print('PASS: default reference tool disabled before model and invalid opt-in combination rejected')
        settings['tools']['skill_read']['resources_enabled'] = True
        config.write_text(json.dumps(settings))
        process, log = start_host('server')
        try:
            def chat(case, tools=None, auth=True):
                return request('/api/chat', {'messages': [{'role': 'user', 'content': case}],
                    'enabled_tools': tools if tools is not None else ['skill_read', 'skill_resource_read']}, auth)

            assert chat('resource-auth', auth=False)[0] == 401
            assert 'resource-auth' not in observed
            result = chat('resource-valid')
            assert result[0] == 200 and result[1]['status'] == 'completed', (result, errors)
            assert len(observed['resource-valid']) == 3
            assert [t['result'] for t in result[1]['tool_calls']] == [body, original]
            for length in [129, 245]:
                state['reference_path'] = 'references/' + 'a' * length
                (state['skill'] / state['reference_path']).write_text(original)
                write_skill()
                assert request('/api/skills/reload', {})[0] == 200
                case = 'resource-long-' + str(length)
                result = chat(case)
                assert result[0] == 200 and result[1]['status'] == 'completed', (result, errors)
                assert len(observed[case]) == 3
                assert [t['result'] for t in result[1]['tool_calls']] == [body, original]
                (state['skill'] / state['reference_path']).unlink()
            state.pop('reference_path')
            write_skill()
            assert request('/api/skills/reload', {})[0] == 200
            print('PASS: authenticated real body-reference-model roundtrip without eager reference content')
            cli = subprocess.run([str(binary), 'chat', '--config', str(config), 'resource-cli'],
                                 env=env, capture_output=True, text=True, timeout=30)
            assert cli.returncode == 0 and 'resources complete' in cli.stdout, (cli.stdout, cli.stderr, errors)
            assert len(observed['resource-cli']) == 3
            print('PASS: actual ordinary CLI reference roundtrip and invalid flag combination before model')
            for case in ['resource-denied', 'resource-extra']:
                result = chat(case, ['skill_read'] if case == 'resource-denied' else None)
                assert result[0] >= 400 and len(observed[case]) == 1, (case, result, errors)
            poison = chat('resource-poison')
            assert poison[0] == 200 and poison[1]['status'] == 'requireshumaninput', (poison, errors)
            assert not (state['workspace'] / 'forbidden.txt').exists()
            print('PASS: original tool permission, strict resource schema and reference text cannot grant writes')
            failures = ['resource-changed-declaration', 'resource-changed-body', 'resource-revoked',
                        'resource-undeclared', 'resource-traversal', 'resource-disk-edited', 'resource-missing']
            if os.name == 'posix':
                failures += ['resource-leaf-link', 'resource-parent-link', 'resource-hardlink', 'resource-fifo']
            state['failures'] = set(failures) | {'resource-oversize', 'resource-invalid-utf8', 'resource-json-budget'}
            outside = root / 'outside.txt'
            outside.write_text(original)
            for case in failures:
                if references.is_symlink():
                    references.unlink()
                    references.mkdir()
                if leaf.exists() or leaf.is_symlink():
                    leaf.unlink()
                leaf.write_text(original)
                write_skill()
                assert request('/api/skills/reload', {})[0] == 200
                if case == 'resource-disk-edited':
                    leaf.write_text('UNAPPROVED_EDIT')
                elif case == 'resource-missing':
                    leaf.unlink()
                elif case == 'resource-leaf-link':
                    leaf.unlink()
                    leaf.symlink_to(outside)
                elif case == 'resource-hardlink':
                    leaf.unlink()
                    os.link(outside, leaf)
                elif case == 'resource-fifo':
                    leaf.unlink()
                    os.mkfifo(leaf)
                elif case == 'resource-parent-link':
                    leaf.unlink()
                    references.rmdir()
                    external_dir = root / 'external'
                    external_dir.mkdir()
                    (external_dir / 'guide.md').write_text(original)
                    references.symlink_to(external_dir, target_is_directory=True)
                start = time.monotonic()
                result = chat(case)
                assert result[0] == 200 and result[1]['status'] == 'completed', (case, result, errors)
                assert len(observed[case]) == 3
                assert original not in json.dumps(result[1]['tool_calls'][-1])
                if case == 'resource-fifo':
                    assert time.monotonic() - start < 5
            print('PASS: declaration/body revocation, undeclared paths, disk hashes, missing files and link-hardlink-FIFO capability refusal')
            if leaf.exists() or leaf.is_symlink():
                leaf.unlink()
            for case, raw in [('resource-large', b'x' * (128 * 1024)),
                              ('resource-oversize', b'x' * (128 * 1024 + 1)),
                              ('resource-invalid-utf8', b'\xff'),
                              ('resource-json-budget', b'\x01' * (128 * 1024))]:
                if leaf.exists():
                    leaf.unlink()
                leaf.write_bytes(raw)
                state['expected'] = raw.decode('utf-8', errors='replace')
                state['expected_hash'] = hashlib.sha256(raw).hexdigest()
                write_skill(hashlib.sha256(raw).hexdigest())
                assert request('/api/skills/reload', {})[0] == 200
                result = chat(case)
                assert result[0] == 200 and result[1]['status'] == 'completed', (case, result, errors)
                assert len(observed[case]) == 3
            print('PASS: raw file and serialized output budgets, invalid UTF-8 and no_effect refusal before success')
            assert not errors, errors
            stop_host(process)
            assert token not in log.read_text() and model_key not in log.read_text()
        finally:
            stop_host(process)
finally:
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)
