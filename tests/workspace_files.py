#!/usr/bin/env python3
"""Real host/native-tool file boundary acceptance, using only a localhost model.

The native protocol limits each tool argument to 16 KiB. File-size boundaries
therefore use bounded appends and short read windows, without weakening that
wire limit. Deterministic mutation-during-read races belong to Rust I/O tests.
"""
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
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
secret = 'files-model-fixture-' + uuid.uuid4().hex
api_token = 'files-host-fixture-' + uuid.uuid4().hex
pairs = [('read_file', 'file_read'), ('write_file', 'file_write'),
         ('delete_file', 'file_delete'), ('list_dir', 'file_list')]
file_names = {name for pair in pairs for name in pair}
limit = 256 * 1024
cases, observations, fixture_errors = {}, {}, []
lock = threading.Lock()


def tool_call(name, arguments, index):
    return {'id': 'file-call-' + str(index), 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(arguments, ensure_ascii=False)}}


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
            assert self.headers.get('Authorization') == 'Bearer ' + secret
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            case = next(message['content'] for message in reversed(body['messages'])
                        if message['role'] == 'user')
            with lock:
                spec = cases[case]
                seen = observations.setdefault(case, [])
                seen.append(body)
                turn = len(seen)
            catalog = {item['function']['name']: item['function']['parameters']
                       for item in body.get('tools', [])}
            if spec.get('disabled'):
                assert not file_names & catalog.keys(), 'disabled file tool in native catalog'
                assert 'str_replace' in catalog
            else:
                assert set(catalog) == set(spec['enabled']), 'request authority changed'
            if spec.get('catalog'):
                for canonical, alias in pairs:
                    assert catalog[canonical] == catalog[alias], (canonical, alias, 'schema differs')
            if turn == 1 and spec['calls']:
                message = {'role': 'assistant', 'content': None,
                           'tool_calls': [tool_call(name, args, index)
                                          for index, (name, args) in enumerate(spec['calls'])]}
                finish = 'tool_calls'
            else:
                assert turn == (2 if spec['calls'] else 1), 'unexpected model replay'
                if spec['calls']:
                    replies = [message for message in body['messages'] if message['role'] == 'tool']
                    assert [message['tool_call_id'] for message in replies] == [
                        'file-call-' + str(index) for index in range(len(spec['calls']))]
                message = {'role': 'assistant', 'content': 'local file fixture completed'}
                finish = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except Exception as error:
            with lock:
                fixture_errors.append(type(error).__name__ + ': ' + str(error))
            self.reply(500, {'error': 'local file fixture assertion failed'})


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
thread = threading.Thread(target=model.serve_forever, daemon=True)
thread.start()


class Host:
    def __init__(self, directory, disabled=False):
        self.root = directory
        self.workspace = directory / 'workspace'
        self.workspace.mkdir(parents=True)
        self.log = directory / 'host.log'
        self.config = directory / 'config.json'
        self.config.write_text(json.dumps({
            'agent': {'name': 'workspace-files-fixture', 'description': 'Fixture',
                      'system_instructions': 'Use only authorized native tools.',
                      'workspace_path': str(self.workspace), 'max_turns': 10,
                      'max_tool_iterations': 2},
            'provider': {'provider_type': 'brokerrouter', 'api_key': secret,
                         'base_url': 'http://127.0.0.1:' + str(model.server_port), 'model': 'fixture'},
            'tools': {canonical: {'enabled': False} for canonical, _alias in pairs} if disabled else {},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
        }))
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
            raise AssertionError('file fixture host startup timed out')
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
        assert secret not in self.log.read_text(), 'model credential leaked to host log'
        assert api_token not in self.log.read_text(), 'host credential leaked to host log'

    def request(self, path, body=None, text=False):
        req = urllib.request.Request(self.base + path,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={'Authorization': 'Bearer ' + api_token,
                                              'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=20) as response:
                payload = response.read().decode()
                return response.status, payload if text else json.loads(payload)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def chat(self, calls, enabled=None, disabled=False, catalog=False):
        case = 'workspace-' + uuid.uuid4().hex
        if enabled is None:
            enabled = list(dict.fromkeys(name for name, _args in calls))
        with lock:
            cases[case] = {'calls': calls, 'enabled': enabled, 'disabled': disabled, 'catalog': catalog}
        code, body = self.request('/api/chat', {
            'session_id': case, 'messages': [{'role': 'user', 'content': case}],
            'enabled_tools': enabled, 'auto_skills': False,
        })
        with lock:
            assert not fixture_errors, fixture_errors
            count = len(observations.get(case, []))
        assert secret not in json.dumps(body) and api_token not in json.dumps(body)
        return code, body, count

    def batch(self, calls, rejected=False):
        code, body, count = self.chat(calls)
        assert code == 200 and body['status'] == 'completed', (code, body.get('error'), count)
        records = body['tool_calls']
        assert len(records) == len(calls) and count == 2, (len(records), len(calls), count)
        outputs = []
        for (name, _args), record in zip(calls, records):
            assert record['tool_name'] == name
            result = record['result']
            if rejected:
                assert isinstance(result, dict) and result.get('error'), (name, result)
            else:
                assert isinstance(result, str), (name, result)
                result = json.loads(result)
                assert isinstance(result, dict)
            outputs.append(result)
        return outputs

    def tool(self, name, arguments, rejected=False):
        return self.batch([(name, arguments)], rejected)[0]


def write_args(path, content='replacement', mode='overwrite'):
    return {'path': path, 'content': content, 'mode': mode}


def unsafe_calls(path):
    return [(name, {'path': path, 'limit': 1}) for name in ('read_file', 'file_read')] + [
        (name, write_args(path)) for name in ('write_file', 'file_write')] + [
        (name, {'path': path}) for name in ('delete_file', 'file_delete')] + [
        ('str_replace', {'path': path, 'old_str': 'original', 'new_str': 'replacement'})]


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-workspace-files-') as temporary:
        root = Path(temporary)
        with Host(root / 'enabled') as host:
            workspace = host.workspace
            code, listing = host.request('/api/tools')
            assert code == 200 and file_names <= {tool['name'] for tool in listing['tools']}
            code, _body, count = host.chat([], sorted(file_names), catalog=True)
            assert code == 200 and count == 1
            original = '原文 alpha\nsecond line\n'
            appended = '\n追加 beta\n'
            for writer, reader, deleter, lister in [
                    ('write_file', 'file_read', 'delete_file', 'file_list'),
                    ('file_write', 'read_file', 'file_delete', 'list_dir')]:
                path = 'nested/' + writer + '.txt'
                output = host.tool(writer, write_args(path, original))
                assert output == {'path': path, 'mode': 'overwrite', 'bytes_written': len(original.encode())}
                host.tool(writer, write_args(path, appended, 'append'))
                expected = (original + appended).encode()
                assert (workspace / path).read_bytes() == expected
                read = host.tool(reader, {'path': path})
                assert read['path'] == path and read['size_bytes'] == len(expected)
                assert read['content'].splitlines() == expected.decode().splitlines()
                assert not read['truncated']
                replacement = host.tool('str_replace', {'path': path, 'old_str': 'alpha', 'new_str': 'ALPHA'})
                assert replacement['replacements'] == 1
                expected = expected.replace(b'alpha', b'ALPHA')
                assert (workspace / path).read_bytes() == expected
                entries = host.tool(lister, {'path': '.', 'recursive': True, 'max_entries': 20})
                entry = next(item for item in entries['entries'] if item['name'] == path)
                assert entry['type'] == 'file' and entry['size'] == len(expected)
                deleted = host.tool(deleter, {'path': path})
                assert deleted == {'path': path, 'deleted': True, 'size_bytes': len(expected)}
                assert not (workspace / path).exists()
            print('PASS: canonical/alias catalogs and schemas, real create/append/read/replace/list/delete, exact UTF-8 disk bytes', flush=True)

            # 32 bounded native arguments reach exactly the on-disk file limit.
            block = 'first\n' + 'x' * (8192 - 6)
            calls = [(('write_file' if index % 2 == 0 else 'file_write'),
                      write_args('boundary.txt', block, 'overwrite' if index == 0 else 'append'))
                     for index in range(32)]
            outputs = host.batch(calls)
            expected = (block * 32).encode()
            assert len(expected) == limit and outputs[-1]['bytes_written'] == limit
            assert (workspace / 'boundary.txt').read_bytes() == expected
            for reader in ('read_file', 'file_read'):
                result = host.tool(reader, {'path': 'boundary.txt', 'limit': 1})
                assert result['size_bytes'] == limit and result['content'] == 'first' and result['truncated']
            for writer in ('write_file', 'file_write'):
                host.tool(writer, write_args('boundary.txt', '!', 'append'), rejected=True)
                assert (workspace / 'boundary.txt').read_bytes() == expected
            host.tool('str_replace', {'path': 'boundary.txt', 'old_str': 'x', 'new_str': 'xx',
                                     'replace_all': True}, rejected=True)
            assert (workspace / 'boundary.txt').read_bytes() == expected
            with (workspace / 'boundary.txt').open('ab') as output:
                output.write(b'!')
            grown = expected + b'!'
            for reader in ('read_file', 'file_read'):
                host.tool(reader, {'path': 'boundary.txt', 'limit': 1}, rejected=True)
            for writer in ('write_file', 'file_write'):
                host.tool(writer, write_args('boundary.txt', '!', 'append'), rejected=True)
            assert (workspace / 'boundary.txt').read_bytes() == grown
            for reader in ('read_file', 'file_read'):
                (workspace / 'binary.dat').write_bytes(b'\xff\x00original')
                host.tool(reader, {'path': 'binary.dat'}, rejected=True)
            print('PASS: exact 256 KiB through bounded native appends, read windows, append/replace overflow and grown-file rejection preserve bytes', flush=True)

            outside = root / 'outside'
            outside.mkdir()
            sentinel = outside / 'sentinel.txt'
            sentinel_bytes = b'original outside secret'
            sentinel.write_bytes(sentinel_bytes)
            (workspace / 'leaf').symlink_to(sentinel)
            (workspace / 'parent').symlink_to(outside, target_is_directory=True)
            (workspace / 'dangling').symlink_to(outside / 'not-created.txt')
            os.link(sentinel, workspace / 'hardlink')
            os.mkfifo(workspace / 'fifo')
            (workspace / 'inside.txt').write_text('original inside')
            (workspace / 'inside-link').symlink_to(workspace / 'inside.txt')
            for path in ('leaf', 'parent/sentinel.txt', 'dangling', 'hardlink', 'fifo', 'inside-link'):
                host.batch(unsafe_calls(path), rejected=True)
                assert sentinel.read_bytes() == sentinel_bytes, path
                assert (workspace / 'inside.txt').read_text() == 'original inside', path
                assert not (outside / 'not-created.txt').exists(), path
                assert (workspace / 'leaf').is_symlink() and (workspace / 'dangling').is_symlink()
            for writer in ('write_file', 'file_write'):
                host.tool(writer, write_args('parent/missing/deeper.txt'), rejected=True)
                assert not (outside / 'missing').exists()
            for lister in ('list_dir', 'file_list'):
                for path in ('parent', 'parent/missing', 'leaf', 'hardlink', 'fifo'):
                    host.tool(lister, {'path': path}, rejected=True)
            for path in ('../outside/sentinel.txt', str(sentinel)):
                host.batch(unsafe_calls(path), rejected=True)
            assert sentinel.read_bytes() == sentinel_bytes
            assert host.request('/health')[0] == 200
            print('PASS: leaf/parent/dangling/internal symlinks, hardlinks, FIFO, traversal and missing-parent escape rejected; outside sentinel unchanged', flush=True)

        with Host(root / 'disabled', disabled=True) as host:
            original = b'original authorized tool must not execute'
            (host.workspace / 'guard.txt').write_bytes(original)
            code, listing = host.request('/api/tools')
            assert code == 200 and not file_names & {tool['name'] for tool in listing['tools']}
            code, _body, count = host.chat([], enabled=[], disabled=True)
            assert code == 200 and count == 1
            for _canonical, alias in pairs:
                arguments = write_args('guard.txt') if alias == 'file_write' else {'path': 'guard.txt'}
                calls = [('str_replace', {'path': 'guard.txt', 'old_str': 'original', 'new_str': 'changed'}),
                         (alias, arguments)]
                code, _body, count = host.chat(calls, enabled=[], disabled=True)
                assert code >= 400 and count == 1, (alias, code, count)
                assert (host.workspace / 'guard.txt').read_bytes() == original
            for name in sorted(file_names):
                code, _body, count = host.chat([], enabled=[name], disabled=True)
                assert code >= 400 and count == 0, (name, code, count)
            code, metrics = host.request('/metrics', text=True)
            assert code == 200
            assert not any(line.startswith('jiaclaw_tool_calls_total{') for line in metrics.splitlines())
            print('PASS: four disabled switches remove all eight names from HTTP/native catalogs; caller and forged aliases rejected before any batch effect or model retry', flush=True)
        with lock:
            assert not fixture_errors, fixture_errors
finally:
    model.shutdown()
    model.server_close()
    thread.join(timeout=5)
