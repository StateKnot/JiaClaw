#!/usr/bin/env python3
"""Real native stat/tree metadata acceptance with localhost fake credentials.

Read-only tool contracts, hierarchy, field disclosure and resource bounds are
verified against actual files. No paid service or privileged mount is used.
"""
import fcntl
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
env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
model_key = 'info-model-fixture-' + uuid.uuid4().hex
host_key = 'info-host-fixture-' + uuid.uuid4().hex
result_limit = 64 * 1024
specs, observed, errors = {}, {}, []
lock = threading.Lock()


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, code, value):
        payload = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers.get('Authorization') == 'Bearer ' + model_key
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            case = next(message['content'] for message in reversed(body['messages'])
                        if message['role'] == 'user')
            with lock:
                spec = specs[case]
                requests = observed.setdefault(case, [])
                requests.append(body)
                turn = len(requests)
            names = {tool['function']['name'] for tool in body.get('tools', [])}
            if spec['catalog'] is not None:
                assert names & {'stat', 'tree'} == set(spec['catalog'])
                assert 'str_replace' in names
            else:
                assert names == set(spec['enabled'])
            if turn == 1 and spec['calls']:
                calls = [{'id': 'info-call-' + str(index), 'type': 'function',
                          'function': {'name': name, 'arguments': json.dumps(args, ensure_ascii=False)}}
                         for index, (name, args) in enumerate(spec['calls'])]
                message = {'role': 'assistant', 'content': None, 'tool_calls': calls}
                finish = 'tool_calls'
            else:
                assert turn == (2 if spec['calls'] else 1), 'unexpected model replay'
                if spec['calls']:
                    replies = [message for message in body['messages'] if message['role'] == 'tool']
                    assert [message['tool_call_id'] for message in replies] == [
                        'info-call-' + str(index) for index in range(len(spec['calls']))]
                message = {'role': 'assistant', 'content': 'local filesystem info fixture completed'}
                finish = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except Exception as error:
            with lock:
                errors.append(type(error).__name__ + ': ' + str(error))
            self.reply(500, {'error': 'local filesystem info fixture assertion failed'})


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
model_thread = threading.Thread(target=model.serve_forever, daemon=True)
model_thread.start()


class Host:
    def __init__(self, directory, tools=None):
        self.workspace = directory / 'workspace'
        self.workspace.mkdir(parents=True)
        self.config = directory / 'config.json'
        self.log = directory / 'host.log'
        self.process = None
        self.config.write_text(json.dumps({
            'agent': {'name': 'filesystem-info-fixture', 'description': 'Fixture',
                      'system_instructions': 'Use only approved native tools.',
                      'workspace_path': str(self.workspace), 'max_turns': 10,
                      'max_tool_iterations': 2},
            'provider': {'provider_type': 'brokerrouter', 'api_key': model_key,
                         'base_url': 'http://127.0.0.1:' + str(model.server_port), 'model': 'fixture'},
            'tools': tools or {},
            'http': {'bind': '127.0.0.1:0', 'api_token': host_key, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
        }))

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
            raise AssertionError('filesystem info fixture host startup timed out')
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
        log = self.log.read_text()
        assert model_key not in log and host_key not in log, 'credential in host log'

    def request(self, path, body=None, text=False):
        req = urllib.request.Request(self.base + path,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={'Authorization': 'Bearer ' + host_key,
                                              'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=20) as response:
                raw = response.read().decode()
                return response.status, raw if text else json.loads(raw)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def chat(self, calls, enabled=None, catalog=None):
        case = 'info-' + uuid.uuid4().hex
        if enabled is None:
            enabled = list(dict.fromkeys(name for name, _args in calls))
        with lock:
            specs[case] = {'calls': calls, 'enabled': enabled, 'catalog': catalog}
        code, body = self.request('/api/chat', {
            'session_id': case, 'messages': [{'role': 'user', 'content': case}],
            'enabled_tools': enabled, 'auto_skills': False,
        })
        with lock:
            assert not errors, errors
            count = len(observed.get(case, []))
        assert model_key not in json.dumps(body) and host_key not in json.dumps(body)
        return code, body, count

    def info(self, name, arguments, reject=False, invalid=False):
        code, body, count = self.chat([(name, arguments)])
        if invalid and code >= 400:
            assert count == 1, (name, code, count)
            return body
        assert code == 200 and body['status'] == 'completed', (name, code, body.get('error'), count)
        assert count == 2 and len(body['tool_calls']) == 1
        record = body['tool_calls'][0]
        assert record['tool_name'] == name
        result = record['result']
        if reject or invalid:
            assert isinstance(result, dict) and result.get('error'), (name, result)
            return result
        assert isinstance(result, str), (name, result)
        assert len(result.encode()) <= result_limit, (name, 'serialized output exceeds 64 KiB')
        output = json.loads(result)
        if name == 'tree':
            assert set(output) == {'path', 'max_depth', 'max_entries', 'truncated', 'entries'}
            assert output['path'] == arguments.get('path', '.')
            assert output['max_depth'] == arguments.get('max_depth', 3)
            assert output['max_entries'] == arguments.get('max_entries', 200)
            assert type(output['truncated']) is bool
            assert len(output['entries']) <= output['max_entries']
            for entry in output['entries']:
                assert set(entry) == ({'name', 'type', 'size'} if entry['type'] == 'file' else {'name', 'type'})
                assert entry['type'] in ('file', 'dir', 'symlink', 'unsupported')
                assert not entry['name'].startswith('/') and '..' not in Path(entry['name']).parts
                full = Path(output['path']) / entry['name']
                assert len(str(full).encode()) <= 1024 and len(full.parts) <= 64
        return output

def put(root, relative, contents):
    target = root / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(contents.encode() if isinstance(contents, str) else contents)
    return target


def metadata_expected(path, name, kind):
    metadata = path.stat()
    expected = {'path': name, 'type': kind, 'modified_unix_ms': metadata.st_mtime_ns // 1_000_000}
    if kind == 'file':
        expected['size_bytes'] = metadata.st_size
    return expected


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-filesystem-info-') as temporary:
        root = Path(temporary)
        with Host(root / 'enabled') as host:
            workspace = host.workspace
            code, catalog = host.request('/api/tools')
            assert code == 200 and {'stat', 'tree'} <= {tool['name'] for tool in catalog['tools']}
            assert host.info('tree', {}) == {'path': '.', 'max_depth': 3, 'max_entries': 200,
                                            'truncated': False, 'entries': []}
            assert host.info('stat', {}) == metadata_expected(workspace, '.', 'dir')
            text = put(workspace, 'metadata/文本.txt', 'metadata only\n中文')
            binary_file = put(workspace, 'metadata/binary.dat', b'\x00\xff\xfe')
            large = workspace / 'metadata/large.bin'
            with large.open('wb') as output:
                output.truncate(64 * 1024 * 1024 + 3)
            timestamp_ms = 1_700_000_123_456
            for target in (text, binary_file, large, workspace / 'metadata'):
                os.utime(target, ns=(timestamp_ms * 1_000_000, timestamp_ms * 1_000_000))
            for name in ('metadata/文本.txt', 'metadata/binary.dat', 'metadata/large.bin'):
                actual = host.info('stat', {'path': name})
                assert actual == metadata_expected(workspace / name, name, 'file'), (name, actual)
                assert actual['modified_unix_ms'] == timestamp_ms
            assert host.info('stat', {'path': 'metadata'}) == {
                'path': 'metadata', 'type': 'dir', 'modified_unix_ms': timestamp_ms}
            assert text.read_text() == 'metadata only\n中文' and binary_file.read_bytes() == b'\x00\xff\xfe'
            assert large.stat().st_size == 64 * 1024 * 1024 + 3
            print('PASS: default root stat/tree, exact metadata fields and millisecond mtime; text, binary and sparse large file inspected without content changes', flush=True)

            put(workspace, 'hierarchy/.git/config', 'git')
            put(workspace, 'hierarchy/.hidden', 'h')
            put(workspace, 'hierarchy/a/z.txt', 'z')
            put(workspace, 'hierarchy/a.txt', 'a')
            put(workspace, 'hierarchy/b/deeper/leaf', 'leaf')
            (workspace / 'hierarchy/empty').mkdir()
            expected = [
                {'name': '.git', 'type': 'dir'}, {'name': '.git/config', 'type': 'file', 'size': 3},
                {'name': '.hidden', 'type': 'file', 'size': 1},
                {'name': 'a', 'type': 'dir'}, {'name': 'a/z.txt', 'type': 'file', 'size': 1},
                {'name': 'a.txt', 'type': 'file', 'size': 1}, {'name': 'b', 'type': 'dir'},
                {'name': 'b/deeper', 'type': 'dir'}, {'name': 'b/deeper/leaf', 'type': 'file', 'size': 4},
                {'name': 'empty', 'type': 'dir'},
            ]
            full = host.info('tree', {'path': 'hierarchy'})
            assert full['entries'] == expected and not full['truncated']
            first_level = host.info('tree', {'path': 'hierarchy', 'max_depth': 1})
            assert first_level['entries'] == [entry for entry in expected if '/' not in entry['name']]
            assert first_level['truncated']
            limited = host.info('tree', {'path': 'hierarchy', 'max_entries': 2})
            assert limited['entries'] == expected[:2] and limited['truncated']
            scoped = host.info('tree', {'path': 'hierarchy/b'})
            assert scoped['entries'] == [{'name': 'deeper', 'type': 'dir'},
                                          {'name': 'deeper/leaf', 'type': 'file', 'size': 4}]
            (workspace / 'only-empty/child').mkdir(parents=True)
            conservative = host.info('tree', {'path': 'only-empty', 'max_depth': 1})
            assert conservative['entries'] == [{'name': 'child', 'type': 'dir'}]
            assert conservative['truncated'], 'depth boundary is conservative even for an empty directory'
            print('PASS: selected-directory relative hierarchy, sorted-per-level DFS, hidden/.git entries, depth/entry bounds and conservative empty-directory truncation', flush=True)

            marker = 'outside-private-' + uuid.uuid4().hex
            outside = put(root, 'outside/secret.txt', marker)
            inside = put(workspace, 'unsafe/inside.txt', 'inside')
            unsafe = workspace / 'unsafe'
            (unsafe / 'leaf').symlink_to(outside)
            (unsafe / 'directory').symlink_to(outside.parent, target_is_directory=True)
            (unsafe / 'internal').symlink_to(inside)
            (unsafe / 'dangling').symlink_to(outside.parent / 'missing')
            os.link(outside, unsafe / 'hardlink')
            os.mkfifo(unsafe / 'fifo')
            for name in ('leaf', 'directory', 'internal', 'dangling', 'hardlink', 'fifo'):
                kind = 'unsupported' if name in ('hardlink', 'fifo') else 'symlink'
                assert host.info('stat', {'path': 'unsafe/' + name}) == {'path': 'unsafe/' + name, 'type': kind}
            shown = host.info('tree', {'path': 'unsafe'})
            assert shown['entries'] == [
                {'name': 'dangling', 'type': 'symlink'}, {'name': 'directory', 'type': 'symlink'},
                {'name': 'fifo', 'type': 'unsupported'}, {'name': 'hardlink', 'type': 'unsupported'},
                {'name': 'inside.txt', 'type': 'file', 'size': 6},
                {'name': 'internal', 'type': 'symlink'}, {'name': 'leaf', 'type': 'symlink'},
            ]
            assert not shown['truncated'] and marker not in json.dumps(shown)
            for name in ('stat', 'tree'):
                host.info(name, {'path': 'unsafe/directory/secret.txt'}, reject=True)
                host.info(name, {'path': 'missing'}, reject=True)
                host.info(name, {'path': '../outside/secret.txt'}, reject=True)
                host.info(name, {'path': str(outside)}, reject=True)
            for name in ('unsafe/leaf', 'unsafe/directory', 'unsafe/internal', 'unsafe/dangling',
                         'unsafe/hardlink', 'unsafe/fifo', 'unsafe/inside.txt'):
                host.info('tree', {'path': name}, reject=True)
            assert outside.read_text() == marker and inside.read_text() == 'inside'
            # Metadata operations do not take the mutation lock.
            descriptor = os.open(workspace, os.O_RDONLY)
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                assert host.info('stat', {'path': 'unsafe/inside.txt'})['size_bytes'] == 6
                assert host.info('tree', {'path': 'unsafe'}) == shown
            finally:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
                os.close(descriptor)
            assert host.request('/health')[0] == 200
            print('PASS: leaf links/hardlinks/FIFO report only allowed metadata; tree never traverses them, linked parents and invalid roots fail; read-only tools work during writer lock', flush=True)

            for name in ('stat', 'tree'):
                host.info(name, {'unexpected': True}, invalid=True)
                for path in (None, 3, True, [], {}, '', '   '):
                    host.info(name, {'path': path}, invalid=True)
                for path in ('/'.join(['x' * 204] * 5) + 'x', '/'.join(['d'] * 65)):
                    host.info(name, {'path': path}, invalid=True)
            for key, values in {'max_depth': (0, 33, -1, 1.5, True, '3', None),
                                'max_entries': (0, 1001, -1, 2.5, False, '200', None)}.items():
                for value in values:
                    host.info('tree', {key: value}, invalid=True)
            depth64 = '/'.join(['depth64'] + ['d'] * 63)
            (workspace / depth64).mkdir(parents=True)
            assert host.info('stat', {'path': depth64}) == metadata_expected(workspace / depth64, depth64, 'dir')
            deep = workspace / 'deep'
            cursor = deep
            for _ in range(34):
                cursor = cursor / 'd'
            cursor.mkdir(parents=True)
            bounded = host.info('tree', {'path': 'deep', 'max_depth': 32, 'max_entries': 1000})
            assert bounded['truncated'] and len(bounded['entries']) == 32
            assert max(entry['name'].count('/') + 1 for entry in bounded['entries']) == 32
            print('PASS: strict path/type/range validation, 64-component stat boundary and 32-level tree limit', flush=True)

            wide = workspace / 'wide'
            wide.mkdir()
            for index in range(2005):
                (wide / f'empty-{index:04}').mkdir()
            wide_result = host.info('tree', {'path': 'wide', 'max_depth': 32, 'max_entries': 1000})
            assert wide_result['truncated'] and len(wide_result['entries']) <= 1000
            assert all(entry['type'] == 'dir' for entry in wide_result['entries'])
            assert host.request('/health')[0] == 200
            for index in range(400):
                put(workspace, 'escaped/' + f'{index:03}-' + '"' * 150 + '.txt', '界')
            escaped = host.info('tree', {'path': 'escaped', 'max_depth': 32, 'max_entries': 1000})
            assert escaped['truncated'] and 0 < len(escaped['entries']) < 400
            assert all(entry['type'] == 'file' and entry['size'] == 3 for entry in escaped['entries'])
            assert [entry['name'] for entry in escaped['entries']] == sorted(entry['name'] for entry in escaped['entries'])
            assert host.request('/health')[0] == 200
            print('PASS: 2005 empty directories truncate under scan/entry limits; escaped output is complete pretty JSON within 64 KiB and host remains responsive', flush=True)
            guard = put(workspace, 'allowlist-guard.txt', 'original allowlist guard')
            for allowed, forbidden in [('stat', 'tree'), ('tree', 'stat')]:
                calls = [('str_replace', {'path': 'allowlist-guard.txt', 'old_str': 'original', 'new_str': 'changed'}),
                         (forbidden, {})]
                code, _body, count = host.chat(calls, enabled=['str_replace', allowed])
                assert code >= 400 and count == 1
                assert guard.read_text() == 'original allowlist guard'
            code, metrics = host.request('/metrics', text=True)
            assert code == 200
            assert not any(line.startswith('jiaclaw_tool_calls_total{tool="str_replace"') for line in metrics.splitlines())

        for enabled_name in ('stat', 'tree', None):
            tool_settings = {name: {'enabled': name == enabled_name} for name in ('stat', 'tree')}
            with Host(root / ('switch-' + str(enabled_name)), tools=tool_settings) as host:
                original = b'original batch guard'
                put(host.workspace, 'guard.txt', original)
                enabled = set() if enabled_name is None else {enabled_name}
                code, catalog = host.request('/api/tools')
                assert code == 200
                assert {tool['name'] for tool in catalog['tools']} & {'stat', 'tree'} == enabled
                code, _body, count = host.chat([], enabled=[], catalog=enabled)
                assert code == 200 and count == 1
                if enabled_name:
                    assert host.info(enabled_name, {})['path'] == '.'
                for disabled_name in {'stat', 'tree'} - enabled:
                    calls = [('str_replace', {'path': 'guard.txt', 'old_str': 'original', 'new_str': 'changed'}),
                             (disabled_name, {})]
                    code, _body, count = host.chat(calls, enabled=[], catalog=enabled)
                    assert code >= 400 and count == 1
                    assert (host.workspace / 'guard.txt').read_bytes() == original
                    code, _body, count = host.chat([], enabled=[disabled_name], catalog=enabled)
                    assert code >= 400 and count == 0
                code, metrics = host.request('/metrics', text=True)
                assert code == 200
                assert not any(line.startswith('jiaclaw_tool_calls_total{tool="str_replace"') for line in metrics.splitlines())
                if enabled_name is None:
                    assert not any(line.startswith('jiaclaw_tool_calls_total{') for line in metrics.splitlines())
        print('PASS: stat/tree switches are independent; HTTP/native catalogs agree, forged mixed batches have zero earlier write effects and disabled caller authority sends zero model requests', flush=True)
finally:
    model.shutdown()
    model.server_close()
    model_thread.join(timeout=5)
