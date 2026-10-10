#!/usr/bin/env python3
"""Actual locked startup/reload/native selection; disposable local model only."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import signal
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
body, reference = 'ACTIVATION_BODY_' + uuid.uuid4().hex, 'ACTIVATION_REFERENCE_' + uuid.uuid4().hex
digest = lambda text: hashlib.sha256(text.encode()).hexdigest()
raw = ('---\nname: reviewed\ndescription: reviewed activation\ntriggers: [activate]\njiaclaw_resources: '
       + json.dumps([{'path': 'references/guide.md', 'sha256': digest(reference)}]) + '\n---\n' + body)
source = {'repository': 'https://github.com/example/reviewed', 'revision': 'a' * 40, 'skill_sha256': digest(raw)}
observed, errors = {}, []


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + model_key
            data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            case = next(m['content'] for m in data['messages'] if m['role'] == 'user')
            rounds = observed.setdefault(case, [])
            rounds.append(data)
            call_id = 'activation-' + case.replace(' ', '-')
            prompt = data['messages'][0]['content']
            names = {t['function']['name'] for t in data['tools']}
            if case == 'activate after legacy disable':
                assert body not in prompt and reference not in prompt
                assert names == {'datetime_now'}
                message, reason = {'role': 'assistant', 'content': 'NO_DISABLED_INJECTION'}, 'stop'
            elif len(rounds) == 1:
                assert names == {'skill_read', 'skill_resource_read'}
                assert body not in prompt and reference not in prompt
                descriptors = [json.loads(line[2:]) for line in prompt.splitlines()
                               if line.startswith('- {') and 'content_sha256' in line]
                if case.startswith('disabled'):
                    assert descriptors == []
                else:
                    assert descriptors[0]['name'] == 'reviewed'
                    assert descriptors[0]['content_sha256'] == digest(body)
                if case.startswith('race'):
                    write_policy(False)
                    assert request('/api/skills/reload', {})[0] == 200
                    assert request('/api/skills')[1]['skills'] == []
                name = 'skill_resource_read' if case.endswith('resource') else 'skill_read'
                args = {'name': 'reviewed', 'content_sha256': digest(body)}
                if name == 'skill_resource_read':
                    args.update(path='references/guide.md', resource_sha256=digest(reference))
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': call_id, 'type': 'function',
                    'function': {'name': name, 'arguments': json.dumps(args)}}]}
                reason = 'tool_calls'
            else:
                assert len(rounds) == 2
                result = json.loads(data['messages'][-1]['content'])
                assert data['messages'][-1]['tool_call_id'] == call_id
                if case.startswith(('disabled', 'race')):
                    assert result['effect_status'] == 'no_effect', result
                    assert body not in json.dumps(result) and reference not in json.dumps(result)
                else:
                    assert result == body
                message, reason = {'role': 'assistant', 'content': 'ACTIVATION_COMPLETE'}, 'stop'
            value, status = {'choices': [{'message': message, 'finish_reason': reason}]}, 200
        except Exception as error:
            errors.append(repr(error))
            value, status = {'error': 'fixture assertion failed'}, 500
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


with tempfile.TemporaryDirectory(prefix='jiaclaw-activation-') as temporary:
    root = Path(temporary)
    workspace = root / 'workspace'
    folder = workspace / 'skills/folder'
    (folder / 'references').mkdir(parents=True)
    leaf = folder / 'SKILL.md'
    leaf.write_text(raw)
    (folder / 'references/guide.md').write_text(reference)
    (workspace / 'AGENTS.md').write_text('Disposable local activation workspace')
    lock, config = workspace / 'skills.lock.json', root / 'config.json'
    settings = {'agent': {'name': 'activation', 'description': 'Disposable', 'system_instructions': '',
                         'max_turns': 10, 'max_tool_iterations': 3, 'workspace_path': str(workspace),
                         'skill_lock_required': True},
                'provider': {'provider_type': 'stub'}, 'http': {'bind': '127.0.0.1:0', 'api_token': token},
                'tools': {'skill_read': {'enabled': True, 'resources_enabled': True}}}
    process, server, logs = None, None, []

    def save():
        config.write_text(json.dumps(settings))

    def write_policy(enabled, version=2):
        entry = {'directory': 'folder', 'source': source}
        if version == 2:
            entry['enabled'] = enabled
        value = {'version': version, 'skills': [entry]}
        stage = lock.with_suffix('.stage')
        stage.write_text(json.dumps(value))
        stage.replace(lock)
        return value

    def cli(*args):
        return subprocess.run([str(binary), *args, '--config', str(config)], env=env,
                              capture_output=True, text=True, timeout=10)

    def policy():
        result = cli('skills', 'policy')
        assert result.returncode == 0, (result.stdout, result.stderr)
        result = json.loads(result.stdout)
        assert result['manifest_sha256'] == hashlib.sha256(lock.read_bytes()).hexdigest()
        return result

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

    def chat(case, legacy=False):
        value = {'messages': [{'role': 'user', 'content': case}], 'enabled_skills': ['reviewed'] if legacy else [],
                 'enabled_tools': ['datetime_now'] if legacy else ['skill_read', 'skill_resource_read']}
        result = request('/api/chat', value)
        assert result[0] == 200 and result[1]['status'] == 'completed', (result, errors)

    save()
    write_policy(False)
    if '--expect-parent-rejection' in sys.argv:
        result = cli('skills')
        assert result.returncode != 0 and '技能锁' in result.stdout + result.stderr, (result.stdout, result.stderr)
        print('PASS: parent rejects persistent activation version 2 (capability gap)')
        sys.exit(0)

    try:
        assert policy()['skills'][0]['enabled'] is False
        assert cli('skills').returncode == 0 and body not in cli('skills', '--verbose').stdout
        assert cli('doctor').returncode == 0
        write_policy(True, version=1)
        assert policy()['version'] == 1 and policy()['skills'][0]['enabled'] is True
        assert body in cli('skills', '--verbose').stdout
        print('PASS activation 1: actual policy JSON hash, version 1 compatibility and disabled CLI/doctor')

        value = write_policy(True)
        for bad in [None, 'false', 0, [], {}]:
            value['skills'][0]['enabled'] = bad
            lock.write_text(json.dumps(value))
            assert cli('skills').returncode != 0 and cli('skills', 'policy').returncode != 0
        value['skills'][0].pop('enabled')
        lock.write_text(json.dumps(value))
        assert cli('skills', 'reload').returncode != 0
        for encoded in ['{"version":2,"skills":[{"directory":"folder","source":'
                         + json.dumps(source) + ',"enabled":false,"enabled":true}]}',
                        json.dumps({'version': 1, 'skills': [{'directory': 'folder', 'source': source, 'enabled': None}]}),
                        json.dumps({'version': 1, 'skills': [{'directory': 'folder', 'source': source, 'enabled': False}]})]:
            lock.write_text(encoded)
            assert cli('skills', 'policy').returncode != 0
        settings['agent']['skill_lock_required'] = False
        save()
        assert cli('skills', 'policy').returncode != 0
        settings['agent']['skill_lock_required'] = True
        save()
        print('PASS activation 2: explicit boolean, duplicate/ambiguous fields and opt-in policy requirement')

        write_policy(False)
        leaf.write_bytes(b'\xff')
        assert policy()['skills'][0]['enabled'] is False
        leaf.unlink()
        outside = root / 'outside'
        outside.write_text('OUTSIDE_MUST_NOT_READ')
        leaf.symlink_to(outside)
        assert cli('skills', 'reload').returncode == 0
        leaf.unlink()
        os.link(outside, leaf)
        assert cli('skills', 'reload').returncode == 0
        leaf.unlink()
        if os.name == 'posix':
            os.mkfifo(leaf)
            assert cli('skills', 'reload').returncode == 0
            start('disabled-fifo')
            assert request('/api/skills')[1]['skills'] == []
            stop()
            leaf.unlink()
        shutil.rmtree(folder)
        assert cli('doctor').returncode == 0
        folder.symlink_to(root)
        assert cli('skills', 'reload').returncode != 0
        folder.unlink()
        (folder / 'references').mkdir(parents=True)
        leaf.write_text(raw)
        (folder / 'references/guide.md').write_text(reference)
        print('PASS activation 3: disabled corrupt/linked/FIFO/missing leaves never read; linked directory still rejected')

        write_policy(True)
        start('atomic')
        initial = request('/api/skills')[1]
        write_policy(False)
        assert request('/api/skills')[1] == initial
        assert request('/api/skills/reload', {}, authenticated=False)[0] == 401
        assert request('/api/skills')[1] == initial
        assert request('/api/skills/reload', {})[0] == 200
        assert request('/api/skills')[1]['skills'] == []
        write_policy(True)
        leaf.write_text('drift')
        assert request('/api/skills/reload', {})[0] == 400
        assert request('/api/skills')[1]['skills'] == []
        leaf.write_text(raw)
        assert request('/api/skills/reload', {})[0] == 200
        assert request('/api/skills')[1] == initial
        write_policy(False)
        if os.name == 'posix':
            process.send_signal(signal.SIGHUP)
            deadline = time.monotonic() + 10
            while request('/api/skills')[1]['skills'] and time.monotonic() < deadline:
                time.sleep(.05)
            assert request('/api/skills')[1]['skills'] == []
        else:
            assert request('/api/skills/reload', {})[0] == 200
        stop()
        start('disabled-restart')
        assert request('/api/skills')[1]['skills'] == []
        stop()
        print('PASS activation 4: authenticated atomic reload, disk-only edit, failed enable, SIGHUP and restart retention')

        server = ThreadingHTTPServer(('127.0.0.1', 0), Model)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        settings['provider'] = {'provider_type': 'brokerrouter', 'model': 'fixture', 'api_key': model_key,
                                'base_url': 'http://127.0.0.1:' + str(server.server_port)}
        save()
        write_policy(True)
        start('native')
        chat('enabled body')
        for case in ['race body', 'race resource']:
            write_policy(True)
            assert request('/api/skills/reload', {})[0] == 200
            chat(case)
        chat('disabled body')
        chat('disabled resource')
        assert not errors, errors
        assert not (workspace / 'forbidden.txt').exists()
        stop()
        print('PASS activation 5: real native stale body/resource calls rejected no_effect after disable without widening tools')

        settings['tools']['skill_read']['enabled'] = False
        settings['tools']['skill_read']['resources_enabled'] = False
        save()
        start('legacy-disabled')
        chat('activate after legacy disable', legacy=True)
        assert not errors, errors
        stop()
        print('PASS activation 6: disabled keyword and explicit skill do not inject body in legacy mode')
    finally:
        stop()
        if server:
            server.shutdown()
            server.server_close()
        for log in logs:
            text = log.read_text()
            assert not re.search(r'panicked at|fatal runtime error', text), text
