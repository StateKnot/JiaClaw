#!/usr/bin/env python3
"""Real CLI/doctor/startup/HTTP/native lock checks; no supplier credentials or downloads."""
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
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
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
token, model_key = uuid.uuid4().hex, uuid.uuid4().hex
calls, errors = [], []
body, reference = 'REVIEWED_BODY_' + uuid.uuid4().hex, 'REFERENCE_' + uuid.uuid4().hex
raw = ('---\nname: reviewed\ndescription: approved\ntriggers: [activate]\njiaclaw_resources: '
       + json.dumps([{'path': 'references/guide.md', 'sha256': hashlib.sha256(reference.encode()).hexdigest()}])
       + '\n---\n' + body)


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def manifest(text=raw):
    return {'version': 1, 'skills': [{'directory': 'folder', 'source': {
        'repository': 'https://github.com/example/reviewed', 'revision': 'a' * 40,
        'skill_sha256': digest(text)}}]}


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + model_key
            data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            calls.append(data)
            prompt = data['messages'][0]['content']
            assert body not in prompt and reference not in prompt
            assert 'https://github.com/example/reviewed' not in prompt
            names = {t['function']['name'] for t in data['tools']}
            assert names == {'skill_read', 'skill_resource_read'}, names
            descriptor = next(json.loads(line[2:]) for line in prompt.splitlines()
                              if line.startswith('- {') and 'reviewed' in line)
            assert descriptor['content_sha256'] == digest(body)
            if len(calls) <= 2:
                name = 'skill_read' if len(calls) == 1 else 'skill_resource_read'
                if len(calls) == 2:
                    assert json.loads(data['messages'][-1]['content']) == body
                args = {'name': 'reviewed', 'content_sha256': digest(body)}
                if name == 'skill_resource_read':
                    args.update(path='references/guide.md', resource_sha256=digest(reference))
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': 'read-' + str(len(calls)), 'type': 'function',
                    'function': {'name': name, 'arguments': json.dumps(args)}}]}
                reason = 'tool_calls'
            else:
                assert len(calls) == 3
                assert json.loads(data['messages'][-1]['content']) == reference
                message, reason = {'role': 'assistant', 'content': 'LOCKED_READ_COMPLETE'}, 'stop'
            value = {'choices': [{'message': message, 'finish_reason': reason}]}
            status = 200
        except Exception as error:
            errors.append(repr(error))
            value, status = {'error': 'fixture assertion failed'}, 500
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


with tempfile.TemporaryDirectory(prefix='jiaclaw-lock-') as temporary:
    root = Path(temporary)
    workspace = root / 'workspace'
    folder = workspace / 'skills/folder'
    (folder / 'references').mkdir(parents=True)
    leaf = folder / 'SKILL.md'
    leaf.write_text(raw)
    (folder / 'references/guide.md').write_text(reference)
    (workspace / 'AGENTS.md').write_text('Disposable local workspace')
    lock = workspace / 'skills.lock.json'
    config = root / 'config.json'
    settings = {'agent': {'name': 'lock-fixture', 'description': 'Disposable', 'system_instructions': '',
                         'max_turns': 10, 'max_tool_iterations': 3, 'workspace_path': str(workspace),
                         'skill_lock_required': True},
                'provider': {'provider_type': 'stub'}, 'http': {'bind': '127.0.0.1:0', 'api_token': token},
                'tools': {'skill_read': {'enabled': True, 'resources_enabled': True}}}

    def save():
        config.write_text(json.dumps(settings))

    def cli(*args):
        return subprocess.run([str(binary), *args, '--config', str(config)], env=env,
                              capture_output=True, text=True, timeout=10)

    def pin(text=raw):
        lock.write_text(json.dumps(manifest(text)))

    def expect_rejection(startup=False):
        for command in [('skills',), ('skills', 'reload'), ('doctor',)]:
            result = cli(*command)
            assert result.returncode != 0, (command, result.stdout, result.stderr)
            assert 'secret-origin-credential' not in result.stdout + result.stderr
        if startup:
            result = cli('serve')
            assert result.returncode != 0 and 'HTTP 服务已启动于' not in result.stdout + result.stderr

    save()
    if '--expect-unlocked' in sys.argv:
        pin(raw + 'changed')
        result = cli('skills', '--verbose')
        assert result.returncode == 0 and body in result.stdout
        print('PASS: parent accepts skill bytes that disagree with the required lock (capability gap)')
        sys.exit(0)

    # Required startup and diagnostic admission cannot silently fall back.
    expect_rejection(startup=True)
    pin()
    assert cli('doctor').returncode == 0
    assert cli('skills', '--verbose').returncode == 0
    for changed in [raw.replace('approved', 'drifted'), raw.replace('activate', 'changed'), raw + '\n']:
        leaf.write_text(changed)
        expect_rejection(startup=True)
    leaf.write_text(raw)
    bad_manifests = [{'version': 2, 'skills': []}, {'version': 1, 'skills': [], 'extra': True},
                     {'version': 1, 'skills': []}, {'version': 1, 'skills': manifest()['skills'] * 2}]
    for field, value in [('repository', 'https://user:secret-origin-credential@host/repo'),
                         ('repository', 'https://host/repo?k=secret-origin-credential'),
                         ('revision', 'main'), ('revision', 'A' * 40), ('skill_sha256', 'A' * 64)]:
        bad = manifest()
        bad['skills'][0]['source'][field] = value
        bad_manifests.append(bad)
    for bad in bad_manifests:
        lock.write_text(json.dumps(bad))
        expect_rejection()
    lock.write_text('{"version":1,"version":1,"skills":[]}')
    expect_rejection()
    pin()
    print('PASS: required CLI doctor startup reject missing schema origin and raw metadata drift')

    # Capability access and byte budgets are enforced for the lock itself.
    for content in [b'x' * (128 * 1024 + 1), b'\xff']:
        lock.write_bytes(content)
        expect_rejection()
    external = root / 'external-lock.json'
    external.write_text(json.dumps(manifest()))
    if os.name == 'posix':
        for kind in ['symlink', 'hardlink', 'fifo', 'directory']:
            lock.unlink()
            if kind == 'symlink':
                lock.symlink_to(external)
            elif kind == 'hardlink':
                os.link(external, lock)
            elif kind == 'fifo':
                os.mkfifo(lock)
            else:
                lock.mkdir()
            expect_rejection(startup=True)
            if kind == 'directory':
                lock.rmdir()
                pin()
            else:
                lock.unlink()
                pin()
    print('PASS: bounded lock capability rejects UTF8 oversize links hardlinks FIFO and directories')

    # Catalog coverage is exact; irrelevant empty ordinary folders remain legal.
    other = workspace / 'skills/other'
    other.mkdir()
    assert cli('skills', 'reload').returncode == 0
    (other / 'SKILL.md').write_text('---\nname: other\n---\nbody')
    expect_rejection()
    (other / 'SKILL.md').unlink()
    other.rmdir()
    leaf.unlink()
    expect_rejection()
    leaf.write_text(raw)
    print('PASS: exact catalog rejects unregistered and absent skill files')

    process, server, logs = None, None, []

    def stop():
        global process
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            process = None

    def start(label):
        global process, base
        log = root / (label + '.log')
        logs.append(log)
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config)], env=env,
                                       stdout=output, stderr=output)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert process.poll() is None, read_running_log(log)
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', read_running_log(log))
            if match:
                base = match.group(1)
                return
            time.sleep(.05)
        raise AssertionError(read_running_log(log))

    def request(path, value=None, authenticated=True):
        headers = {'Content-Type': 'application/json'}
        if authenticated:
            headers['Authorization'] = 'Bearer ' + token
        try:
            with urllib.request.urlopen(urllib.request.Request(base + path, headers=headers,
                    data=json.dumps(value).encode() if value is not None else None), timeout=20) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    try:
        start('locked')
        assert request('/api/skills', authenticated=False)[0] == 401
        assert request('/api/skills/reload', {}, authenticated=False)[0] == 401
        advertised = request('/api/openapi.json')[1]['components']['schemas']
        assert advertised['SkillInfo']['properties']['source']['$ref'] == '#/components/schemas/SkillSourcePin'
        assert set(advertised['SkillSourcePin']['required']) == {'repository', 'revision', 'skill_sha256'}
        initial = request('/api/skills')[1]
        assert initial['skills'][0]['source'] == manifest()['skills'][0]['source']
        leaf.write_text(raw.replace('approved', 'updated'))
        status, result = request('/api/skills/reload', {})
        assert status == 400 and '已保留旧表' in result['error']
        assert request('/api/skills')[1] == initial
        pin(leaf.read_text())
        assert request('/api/skills/reload', {})[0] == 200
        updated = request('/api/skills')[1]
        assert updated['skills'][0]['description'] == 'updated'
        assert updated['skills'][0]['source']['skill_sha256'] == digest(leaf.read_text())
        lock.unlink()
        assert request('/api/skills/reload', {})[0] == 400
        assert request('/api/skills')[1] == updated
        leaf.write_text(raw)
        pin()
        if os.name == 'posix':
            import signal
            process.send_signal(signal.SIGHUP)
            deadline = time.monotonic() + 10
            while request('/api/skills')[1] != initial and time.monotonic() < deadline:
                time.sleep(.05)
            assert request('/api/skills')[1] == initial
        stop()
        print('PASS: authenticated HTTP and SIGHUP preserve captured lock policy and atomic old table')

        server = ThreadingHTTPServer(('127.0.0.1', 0), Model)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        settings['provider'] = {'provider_type': 'brokerrouter', 'model': 'fixture', 'api_key': model_key,
                                'base_url': 'http://127.0.0.1:' + str(server.server_port)}
        save()
        start('native')
        status, result = request('/api/chat', {'messages': [{'role': 'user', 'content': 'activate locked read'}],
                         'enabled_tools': ['skill_read', 'skill_resource_read']})
        assert status == 200 and result['message']['content'] == 'LOCKED_READ_COMPLETE', (status, result, errors)
        assert len(calls) == 3 and not errors, errors
        assert not (workspace / 'forbidden.txt').exists()
        stop()
        print('PASS: locked native body and reference reads retain original tool allowlist and model catalog')

        settings['provider'] = {'provider_type': 'stub'}
        settings['agent']['skill_lock_required'] = False
        save()
        lock.write_bytes(b'\xff')
        assert cli('skills').returncode == 0
        start('unlocked')
        assert 'source' not in request('/api/skills')[1]['skills'][0]
        stop()
        settings['agent']['skill_lock_required'] = True
        save()
        leaf.unlink()
        lock.write_text('{"version":1,"skills":[]}')
        assert cli('skills', 'reload').returncode == 0
        assert cli('doctor').returncode == 0
        start('empty')
        assert request('/api/skills')[1]['skills'] == []
        stop()
        print('PASS: opt-in compatibility ignores unused lock and accepts explicit locked empty catalog')
    finally:
        stop()
        if server:
            server.shutdown()
            server.server_close()
        # All writers are settled, including shutdown logs, before strict UTF-8 scanning.
        for log in logs:
            text = log.read_text()
            assert not re.search(r'panicked at|fatal runtime error', text), text
