#!/usr/bin/env python3
"""Real CLI/HTTP/native-tool memory I/O acceptance; only a local model fixture.

Runs on the supported Linux/macOS hosts. Files, links, flock contention, tokens
and process state are disposable. Does not certify semantic search or a vendor.
"""
import fcntl
import json
import os
from pathlib import Path
import re
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
env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
secret = 'memory-fixture-' + uuid.uuid4().hex
api_token = 'memory-host-' + uuid.uuid4().hex
specs, observed, errors = {}, [], []
lock = threading.Lock()


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, code, value):
        payload = json.dumps(value).encode()
        self.send_response(code)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers.get('Authorization') == 'Bearer ' + secret
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            user = next(message['content'] for message in reversed(body['messages']) if message['role'] == 'user')
            case = re.search(r'memory-case:([a-z0-9_-]+)', user).group(1)
            with lock:
                observed.append((case, body))
                spec = specs.get(case)
            if spec and body['messages'][-1]['role'] != 'tool':
                name, arguments = spec
                if case == 'disabled_alias':
                    assert not {'memory_write', 'memory_append'} & {
                        tool['function']['name'] for tool in body.get('tools', [])}
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': 'memory-' + case, 'type': 'function',
                    'function': {'name': name, 'arguments': json.dumps(arguments)},
                }]}
                finish = 'tool_calls'
            else:
                message = {'role': 'assistant', 'content': 'fixture completed ' + case}
                finish = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except Exception as error:
            with lock:
                errors.append(repr(error))
            self.reply(500, {'error': 'fixture assertion failed'})


gateway = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
gateway.daemon_threads = True
thread = threading.Thread(target=gateway.serve_forever, daemon=True)
thread.start()


def captured(case=None):
    with lock:
        assert not errors, errors
        return [body for name, body in observed if case is None or name == case]


def run(*args):
    return subprocess.run([str(binary), *map(str, args)], env=env, capture_output=True,
                          text=True, timeout=20)


def config_for(root, **extra):
    workspace = root / 'workspace'
    workspace.mkdir(parents=True)
    config = {
        'agent': {'name': 'memory-io-fixture', 'description': 'Fixture',
                  'system_instructions': 'Use only the approved tools.', 'max_turns': 10,
                  'max_tool_iterations': 2, 'workspace_path': str(workspace)},
        'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:' + str(gateway.server_port),
                     'api_key': secret, 'model': 'fixture-memory'},
        'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
    }
    config.update(extra)
    return config


class Host:
    def __init__(self, root, config):
        self.config = root / 'config.json'
        self.config.write_text(json.dumps(config))
        self.log = root / 'server.log'
        self.process = None

    def __enter__(self):
        with self.log.open('wb') as output:
            self.process = subprocess.Popen([str(binary), 'serve', '--config', str(self.config)],
                                            env=env, stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert self.process.poll() is None, self.log.read_text()
                match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', self.log.read_text())
                if match:
                    self.base = match.group(1)
                    return self
                time.sleep(.05)
            raise AssertionError('host startup timed out: ' + self.log.read_text())
        except Exception:
            self.__exit__()
            raise

    def __exit__(self, *_args):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        assert secret not in self.log.read_text(), 'model secret in host log'

    def request(self, path, body=None):
        headers = {'Authorization': 'Bearer ' + api_token}
        if body is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(self.base + path, headers=headers,
                                     data=json.dumps(body).encode() if body is not None else None)
        try:
            with urllib.request.urlopen(req, timeout=15) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def chat(self, case, enabled_tools):
        return self.request('/api/chat', {'session_id': 'memory-' + case,
                           'messages': [{'role': 'user', 'content': 'memory-case:' + case}],
                           'enabled_tools': enabled_tools, 'auto_skills': False})

    def tool(self, case, name, arguments, reject=False):
        with lock:
            specs[case] = (name, arguments)
        code, response = self.chat(case, [name])
        if reject and code >= 400:
            return response
        assert code == 200, (case, code, response, errors)
        assert len(response['tool_calls']) == 1, (case, response)
        record = response['tool_calls'][0]
        assert record['tool_name'] == name, (case, record)
        result = record['result']
        if reject:
            assert isinstance(result, dict) and result.get('error'), (case, result)
        else:
            assert isinstance(result, str), (case, result)
        return result


def wait_until(predicate, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.05)
    raise AssertionError('fixture condition timed out')


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-memory-io-') as directory:
        root = Path(directory)
        initialized = root / 'initialized'
        assert run('init', '--path', initialized).returncode == 0
        names = ['AGENTS.md', 'MEMORY.md', 'SOUL.md', 'USER.md',
                 'skills/search/SKILL.md', 'skills/calculator/SKILL.md']
        defaults = {name: (initialized / name).read_bytes() for name in names}
        for name in names:
            (initialized / name).write_text('operator edit ' + name)
        before = {name: (initialized / name).read_bytes() for name in names}
        assert run('init', '--path', initialized).returncode == 0
        assert {name: (initialized / name).read_bytes() for name in names} == before
        # Restart initialization after a partial setup without forcing operator edits away.
        for name in ['MEMORY.md', 'skills/search/SKILL.md']:
            (initialized / name).unlink()
        result = run('init', '--path', initialized)
        assert result.returncode == 0, result.stderr
        assert (initialized / 'MEMORY.md').read_bytes() == defaults['MEMORY.md']
        assert (initialized / 'skills/search/SKILL.md').read_bytes() == defaults['skills/search/SKILL.md']
        assert all((initialized / name).read_bytes() == before[name]
                   for name in names if name not in ['MEMORY.md', 'skills/search/SKILL.md'])
        invalid_root = root / 'ordinary-file-root'
        invalid_root.write_text('operator root sentinel')
        assert run('init', '--path', invalid_root).returncode != 0
        assert invalid_root.read_text() == 'operator root sentinel'
        result = run('init', '--path', initialized, '--force')
        assert result.returncode == 0, result.stderr
        assert {name: (initialized / name).read_bytes() for name in names} == defaults
        assert all(stat.S_IMODE((initialized / name).stat().st_mode) == 0o600 for name in names)
        outside = root / 'outside'
        outside.mkdir()
        sentinel = outside / 'sentinel'
        sentinel.write_text('OUTSIDE-PRIVATE-' + uuid.uuid4().hex)
        sentinel_bytes = sentinel.read_bytes()
        (initialized / 'MEMORY.md').unlink()
        (initialized / 'MEMORY.md').symlink_to(sentinel)
        assert run('init', '--path', initialized).returncode != 0
        assert run('init', '--path', initialized, '--force').returncode != 0
        assert sentinel.read_bytes() == sentinel_bytes
        (initialized / 'MEMORY.md').unlink()
        (initialized / 'MEMORY.md').write_bytes(defaults['MEMORY.md'])
        (initialized / 'skills').rename(initialized / 'skills-saved')
        (initialized / 'skills').symlink_to(outside, target_is_directory=True)
        assert run('init', '--path', initialized).returncode != 0
        assert run('init', '--path', initialized, '--force').returncode != 0
        assert sorted(path.name for path in outside.iterdir()) == ['sentinel']
        print('PASS: CLI init resumes missing defaults, preserves operator edits, rejects invalid roots and links; explicit force safely replaces ordinary files')

        active = root / 'active'
        config = config_for(active, memory={'path': 'notes/custom.md'},
                            identity={'soul_path': 'notes/persona.md', 'user_path': 'notes/profile.md'})
        workspace = active / 'workspace'
        notes = workspace / 'notes'
        notes.mkdir()
        target = notes / 'custom.md'
        target.write_text('CONFIGURED-MEMORY')
        (notes / 'persona.md').write_text('CONFIGURED-SOUL')
        (notes / 'profile.md').write_text('CONFIGURED-USER')
        for name in ['MEMORY.md', 'SOUL.md', 'USER.md']:
            (workspace / name).write_text('UNSELECTED-DEFAULT-' + name)
        (notes / 'custom.md.tmp').symlink_to(sentinel)
        with Host(active, config) as host:
            for logical, marker in [('MEMORY.md', 'CONFIGURED-MEMORY'), ('SOUL.md', 'CONFIGURED-SOUL'),
                                    ('USER.md', 'CONFIGURED-USER')]:
                result = host.tool('read-' + logical.split('.')[0].lower(), 'memory_read', {'file': logical})
                assert marker in result and 'UNSELECTED-DEFAULT' not in result, result
            prompt = json.dumps(captured('read-memory')[0]['messages'], ensure_ascii=False)
            assert all(marker in prompt for marker in ['CONFIGURED-MEMORY', 'CONFIGURED-SOUL', 'CONFIGURED-USER'])
            assert 'UNSELECTED-DEFAULT' not in prompt
            for command, marker in [('memory', 'CONFIGURED-MEMORY'), ('soul', 'CONFIGURED-SOUL'), ('user', 'CONFIGURED-USER')]:
                result = run(command, 'show', '--config', host.config)
                assert result.returncode == 0 and marker in result.stdout, (result.stdout, result.stderr)
            host.tool('append', 'memory_append', {'content': 'append marker'})
            host.tool('write', 'memory_write', {'content': 'replacement marker', 'mode': 'overwrite'})
            assert target.read_text() == 'replacement marker'
            assert stat.S_IMODE(target.stat().st_mode) == 0o600
            assert sentinel.read_bytes() == sentinel_bytes and (notes / 'custom.md.tmp').is_symlink()
            assert {path.name for path in notes.iterdir()} == {'custom.md', 'custom.md.tmp', 'persona.md', 'profile.md'}
            # Native tool argument JSON has a separate 16 KiB cap. Use small
            # append arguments to exercise the final-file 32 KiB boundary itself.
            target.write_text('x' * 32766)
            for name, arguments in [('memory_append', {'content': 'x'}),
                                    ('memory_write', {'content': 'x', 'mode': 'append'})]:
                host.tool('oversize-' + name.replace('_', '-'), name, arguments, reject=True)
                assert target.read_bytes() == b'x' * 32766
            # The separator plus this two-byte append produces exactly 32 KiB.
            target.write_text('x' * 32764)
            host.tool('exact-write', 'memory_write', {'content': 'ok', 'mode': 'append'})
            host.tool('full-append', 'memory_append', {'content': 'extra'}, reject=True)
            assert target.read_bytes() == b'x' * 32764 + b'\n\nok'
            target.write_text('a' * 32767 + '\u00e9' + 'BEYOND-READ-BOUNDARY')
            result = host.tool('bounded-read', 'memory_read', {'file': 'MEMORY.md'})
            assert 'BEYOND-READ-BOUNDARY' not in result and '\u00e9' not in result and '\ufffd' not in result
            assert len(result.encode()) <= 32768 + 1024 and ('截断' in result or 'truncat' in result.lower())
            assert 'BEYOND-READ-BOUNDARY' not in json.dumps(captured('bounded-read')[0]['messages'])
            result = run('memory', 'show', '--config', host.config)
            assert result.returncode == 0 and 'BEYOND-READ-BOUNDARY' not in result.stdout
            target.write_text('before contention')
            fd = os.open(workspace, os.O_RDONLY)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                host.tool('busy-append', 'memory_append', {'content': 'must not commit'}, reject=True)
                assert target.read_text() == 'before contention'
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
                os.close(fd)
            host.tool('after-busy', 'memory_append', {'content': 'explicit fresh append'})
            assert target.read_text() == 'before contention\n\nexplicit fresh append'
            saved = workspace / 'notes-saved'
            notes.rename(saved)
            notes.symlink_to(outside, target_is_directory=True)
            host.tool('parent-link-write', 'memory_write', {'content': 'escape', 'mode': 'overwrite'}, reject=True)
            host.tool('parent-link-read', 'memory_read', {'file': 'MEMORY.md'}, reject=True)
            assert not (outside / 'custom.md').exists() and sentinel.read_bytes() == sentinel_bytes
            notes.unlink()
            saved.rename(notes)
            for kind in ['symlink', 'hardlink', 'fifo']:
                target.unlink()
                if kind == 'symlink':
                    target.symlink_to(sentinel)
                elif kind == 'hardlink':
                    os.link(sentinel, target)
                else:
                    os.mkfifo(target)
                host.tool(kind + '-read', 'memory_read', {'file': 'MEMORY.md'}, reject=True)
                host.tool(kind + '-write', 'memory_write', {'content': 'escape', 'mode': 'overwrite'}, reject=True)
                host.tool(kind + '-search', 'memory_search', {'query': 'OUTSIDE', 'paths': ['notes/custom.md']}, reject=True)
                assert sentinel.read_bytes() == sentinel_bytes
            target.unlink()
            target.write_text('searchable')
            searchfile = notes / 'search.md'
            searchfile.write_text('needle ' + '\u754c' * 2000 + '\n' + 'x' * (512 * 1024) + '\nAFTER-SEARCH-BOUNDARY')
            found = json.loads(host.tool('search-excerpt', 'memory_search', {'query': 'needle', 'paths': ['notes/search.md']}))
            assert found['matches'] and all(len(hit['excerpt'].encode()) <= 1024 for hit in found['matches'])
            found = json.loads(host.tool('search-bounded', 'memory_search', {'query': 'AFTER-SEARCH-BOUNDARY', 'paths': ['notes/search.md']}))
            assert found['matches'] == [] and found.get('warnings'), found
            host.tool('search-query-limit', 'memory_search', {'query': '\u754c' * 342}, reject=True)
            host.tool('search-path-limit', 'memory_search', {'query': 'needle', 'paths': ['missing' + str(n) for n in range(17)]}, reject=True)
            assert sentinel_bytes.decode() not in json.dumps(captured())
        print('PASS: configured memory/identity reads, native aliases, bounded UTF-8/read/write/search, guarded atomic replacement, flock contention, symlink/hardlink/FIFO refusal')

        disabled = root / 'disabled'
        config = config_for(disabled, tools={'memory_write': {'enabled': False}})
        target = disabled / 'workspace' / 'MEMORY.md'
        target.write_text('read-only marker')
        with Host(disabled, config) as host:
            names = {tool['name'] for tool in host.request('/api/tools')[1]['tools']}
            assert 'memory_read' in names and not {'memory_write', 'memory_append'} & names
            count = len(captured())
            assert host.chat('disabled-explicit', ['memory_append'])[0] >= 400
            assert len(captured()) == count
            with lock:
                specs['disabled_alias'] = ('memory_append', {'content': 'must not execute'})
            assert host.chat('disabled_alias', ['memory_read'])[0] >= 400
            assert len(captured('disabled_alias')) == 1 and target.read_text() == 'read-only marker'
        print('PASS: memory_write disable removes both write names and rejects a forged native alias before effects')

        heartbeat = root / 'heartbeat'
        config = config_for(heartbeat, heartbeat={'enabled': True, 'interval_secs': 1,
                                                 'path': 'HEARTBEAT.md', 'session_id': 'bounded-heartbeat'})
        target = heartbeat / 'workspace' / 'HEARTBEAT.md'
        target.write_text('memory-case:heartbeat_large\n' + 'x' * 32769)
        with Host(heartbeat, config) as host:
            wait_until(lambda: 'Heartbeat 本轮跳过：无法读取文件' in host.log.read_text())
            assert not captured('heartbeat_large')
            target.write_text('memory-case:heartbeat_ok\nValid bounded instruction.')
            wait_until(lambda: bool(captured('heartbeat_ok')))
            target.write_text('')
            sent = captured('heartbeat_ok')[0]['messages'][-1]['content']
            assert sent == 'memory-case:heartbeat_ok\nValid bounded instruction.', sent
        captured()
        print('PASS: oversized HEARTBEAT performs no model call; a subsequent bounded instruction runs unchanged')
finally:
    gateway.shutdown()
    gateway.server_close()
    thread.join(timeout=5)
