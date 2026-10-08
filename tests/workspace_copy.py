#!/usr/bin/env python3
"""Actual serve/native copy acceptance with a localhost model and fake keys.

This verifies completed copies and deterministic rejection before mutation.
A timeout/cancellation is not a durable receipt and does not prove no effect;
no automatic retry is exercised. Shared eight-slot admission, retained permits
on cancellation, growth during read and parent substitution are deterministic
Rust I/O tests, rather than process tests based on scheduler timing.
"""
from contextlib import contextmanager
import fcntl
import hashlib
import json
import stat
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
secret = 'copy-model-fixture-' + uuid.uuid4().hex
api_token = 'copy-host-fixture-' + uuid.uuid4().hex
copy_names = {'copy', 'file_copy'}
cases, observations, fixture_errors = {}, {}, []
lock = threading.Lock()


def tool_call(name, arguments, index):
    return {'id': 'copy-call-' + str(index), 'type': 'function',
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
                assert not copy_names & catalog.keys(), 'disabled copy tool in native catalog'
                assert 'str_replace' in catalog
            else:
                assert set(catalog) == set(spec['enabled']), 'request authority changed'
            if spec.get('catalog'):
                assert catalog['copy'] == catalog['file_copy'], 'native alias schema differs'
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
                        'copy-call-' + str(index) for index in range(len(spec['calls']))]
                message = {'role': 'assistant', 'content': 'local copy fixture completed'}
                finish = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except Exception as error:
            with lock:
                fixture_errors.append(type(error).__name__ + ': ' + str(error))
            self.reply(500, {'error': 'local copy fixture assertion failed'})


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
            'agent': {'name': 'workspace-copy-fixture', 'description': 'Fixture',
                      'system_instructions': 'Use only authorized native tools.',
                      'workspace_path': str(self.workspace), 'max_turns': 10,
                      'max_tool_iterations': 2},
            'provider': {'provider_type': 'brokerrouter', 'api_key': secret,
                         'base_url': 'http://127.0.0.1:' + str(model.server_port), 'model': 'fixture'},
            'tools': {'copy': {'enabled': not disabled}},
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
            raise AssertionError('copy fixture host startup timed out')
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


def put(workspace, name, contents):
    path = workspace / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents.encode() if isinstance(contents, str) else contents)
    return path


def identity(path):
    metadata = path.lstat()
    return metadata.st_dev, metadata.st_ino


COPY_LIMIT = 64 * 1024 * 1024


def sha256(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def copy_args(source, destination, overwrite=False):
    return {'from': source, 'to': destination, 'overwrite': overwrite}


@contextmanager
def relative_parent(workspace, path, create=False):
    # Walk one component per syscall. The legal relative 1024-byte contract
    # plus a temporary-directory prefix exceeds macOS ambient PATH_MAX.
    # Both the accepted and rejected paths really exist; no limit is skipped.
    descriptors = [os.open(workspace, os.O_RDONLY | os.O_DIRECTORY)]
    try:
        parts = path.split('/')
        assert all(part and part not in ('.', '..') for part in parts)
        for part in parts[:-1]:
            if create:
                try:
                    os.mkdir(part, dir_fd=descriptors[-1])
                except FileExistsError:
                    pass
            descriptors.append(os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                       dir_fd=descriptors[-1]))
        yield descriptors[-1], parts[-1]
    finally:
        for descriptor in reversed(descriptors):
            os.close(descriptor)


def relative_put(workspace, path, contents):
    with relative_parent(workspace, path, create=True) as (parent, leaf):
        descriptor = os.open(leaf, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW,
                             0o600, dir_fd=parent)
        with os.fdopen(descriptor, 'wb') as output:
            output.write(contents)


def relative_read(workspace, path):
    with relative_parent(workspace, path) as (parent, leaf):
        descriptor = os.open(leaf, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=parent)
        with os.fdopen(descriptor, 'rb') as source:
            return source.read()


def relative_identity(workspace, path):
    with relative_parent(workspace, path) as (parent, leaf):
        metadata = os.stat(leaf, dir_fd=parent, follow_symlinks=False)
        return metadata.st_dev, metadata.st_ino


def assert_no_staging(workspace):
    # Descriptor traversal also inspects the long-path parents and avoids
    # following any fixture symlinks, including internal directory links.
    def inspect(descriptor):
        with os.scandir(descriptor) as entries:
            for entry in entries:
                assert not entry.name.startswith('.jiaclaw-copy-'), 'uncommitted copy staging remained'
                if entry.is_dir(follow_symlinks=False):
                    child = os.open(entry.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                    dir_fd=descriptor)
                    try:
                        inspect(child)
                    finally:
                        os.close(child)
    descriptor = os.open(workspace, os.O_RDONLY | os.O_DIRECTORY)
    try:
        inspect(descriptor)
    finally:
        os.close(descriptor)


def assert_copy(result, source, destination, size, overwritten=False):
    assert result == {'from': source, 'to': destination, 'bytes': size,
                      'overwritten': overwritten}, result


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-copy-') as temporary:
        root = Path(temporary)
        with Host(root / 'enabled') as host:
            workspace = host.workspace
            root_inode = identity(workspace)
            code, listing = host.request('/api/tools')
            assert code == 200
            catalog = {tool['name']: tool['description'] for tool in listing['tools']}
            assert copy_names <= catalog.keys(), 'copy alias missing from HTTP catalog'
            assert catalog['copy'] == catalog['file_copy'], 'HTTP alias description differs'
            code, _body, count = host.chat([], enabled=sorted(copy_names), catalog=True)
            assert code == 200 and count == 1
            original = b'\x00\xff\x80binary\r\n' + '中文 payload'.encode()
            source = put(workspace, 'source.bin', original)
            source_inode = identity(source)
            combinations = [('from', 'to'), ('source', 'destination'),
                            ('from', 'destination'), ('source', 'to')]
            calls = []
            for name in sorted(copy_names):
                for index, (source_key, destination_key) in enumerate(combinations):
                    destination = name + '-' + str(index) + '.bin'
                    calls.append((name, {source_key: 'source.bin', destination_key: destination}))
            results = host.batch(calls)
            for (_name, arguments), result in zip(calls, results):
                destination = arguments.get('to', arguments.get('destination'))
                assert_copy(result, 'source.bin', destination, len(original))
                copied = workspace / destination
                assert copied.read_bytes() == original and identity(copied) != source_inode
                assert stat.S_IMODE(copied.stat().st_mode) == 0o600
                assert copied.stat().st_nlink == 1
            assert source.read_bytes() == original and identity(source) == source_inode
            assert_no_staging(workspace)
            print('PASS 1: HTTP alias catalogs and native schemas match; both names accept all four argument combinations and exact binary JSON results', flush=True)

            destination = put(workspace, 'overwrite.bin', b'original destination')
            destination_inode = identity(destination)
            for name in sorted(copy_names):
                host.tool(name, copy_args('source.bin', 'overwrite.bin'), rejected=True)
                assert destination.read_bytes() == b'original destination'
                assert identity(destination) == destination_inode
            result = host.tool('copy', copy_args('source.bin', 'overwrite.bin', True))
            assert_copy(result, 'source.bin', 'overwrite.bin', len(original), True)
            assert destination.read_bytes() == original and identity(destination) != destination_inode
            host.tool('file_copy', {'from': 'source.bin', 'source': 'source.bin',
                                   'to': 'equal-alias.bin', 'destination': 'equal-alias.bin'})
            for arguments in (
                    {'from': 'source.bin', 'source': 'different', 'to': 'invalid-alias.bin'},
                    {'from': 'source.bin', 'to': 'invalid-alias.bin', 'destination': 'different'},
                    copy_args('source.bin', 'source.bin', True),
                    copy_args('absent-source', 'absent-target'),
                    copy_args('source.bin', 'absent-parent/nested/target')):
                host.tool('copy', arguments, rejected=True)
                assert source.read_bytes() == original and identity(source) == source_inode
                assert not (workspace / 'invalid-alias.bin').exists()
                assert not (workspace / 'absent-target').exists() and not (workspace / 'absent-parent').exists()
            assert_no_staging(workspace)
            print('PASS 2: no-clobber and explicit atomic overwrite; equal aliases accepted, conflicts/same source/missing source or parent preserve bytes', flush=True)

            large = workspace / 'limit.bin'
            chunk = bytes(range(256)) * 4096
            with large.open('wb') as output:
                for _index in range(COPY_LIMIT // len(chunk)):
                    output.write(chunk)
            assert large.stat().st_size == COPY_LIMIT
            expected_hash = sha256(large)
            result = host.tool('file_copy', copy_args('limit.bin', 'limit-copy.bin'))
            assert_copy(result, 'limit.bin', 'limit-copy.bin', COPY_LIMIT)
            assert (workspace / 'limit-copy.bin').stat().st_size == COPY_LIMIT
            assert sha256(workspace / 'limit-copy.bin') == expected_hash == sha256(large)
            oversized = workspace / 'oversized.bin'
            with oversized.open('wb') as output:
                output.truncate(COPY_LIMIT + 1)
            guard = put(workspace, 'size-guard.bin', b'unchanged size boundary')
            guard_inode = identity(guard)
            for name in sorted(copy_names):
                host.tool(name, copy_args('oversized.bin', 'size-guard.bin', True), rejected=True)
                host.tool(name, copy_args('oversized.bin', 'oversized-copy.bin'), rejected=True)
                assert guard.read_bytes() == b'unchanged size boundary' and identity(guard) == guard_inode
                assert not (workspace / 'oversized-copy.bin').exists()
            assert oversized.stat().st_size == COPY_LIMIT + 1 and sha256(large) == expected_hash
            assert_no_staging(workspace)
            large.unlink()
            (workspace / 'limit-copy.bin').unlink()
            oversized.unlink()
            print('PASS 3: actual 64 MiB binary copy matches SHA-256; 64 MiB+1 source rejects with existing destination inode/bytes and no new target preserved', flush=True)

            outside = root / 'outside'
            outside.mkdir()
            sentinel = put(outside, 'sentinel.bin', b'fixture-owned outside sentinel')
            sentinel_bytes = sentinel.read_bytes()
            sentinel_inode = identity(sentinel)
            put(workspace, 'inside/sentinel.bin', b'fixture-owned inside sentinel')
            inside = workspace / 'inside/sentinel.bin'
            (workspace / 'outside-leaf').symlink_to(sentinel)
            (workspace / 'inside-leaf').symlink_to(inside)
            (workspace / 'dangling-leaf').symlink_to(outside / 'missing-file')
            (workspace / 'outside-parent').symlink_to(outside, target_is_directory=True)
            (workspace / 'inside-parent').symlink_to(workspace / 'inside', target_is_directory=True)
            (workspace / 'dangling-parent').symlink_to(outside / 'missing-dir', target_is_directory=True)
            os.link(sentinel, workspace / 'outside-hardlink')
            os.link(inside, workspace / 'inside-hardlink')
            os.mkfifo(workspace / 'fifo')
            (workspace / 'directory').mkdir()
            guard = put(workspace, 'safety-guard.bin', b'unchanged safety guard')
            guard_inode = identity(guard)
            unsafe = ('outside-leaf', 'inside-leaf', 'dangling-leaf',
                      'outside-parent/sentinel.bin', 'inside-parent/sentinel.bin',
                      'dangling-parent/leaf', 'outside-hardlink', 'inside-hardlink',
                      'fifo', 'directory', '../outside/sentinel.bin', str(sentinel))
            for path in unsafe:
                host.batch([('copy', copy_args(path, 'safety-guard.bin', True)),
                            ('file_copy', copy_args('source.bin', path, True))], rejected=True)
                assert guard.read_bytes() == b'unchanged safety guard' and identity(guard) == guard_inode
                assert sentinel.read_bytes() == sentinel_bytes and identity(sentinel) == sentinel_inode
                assert inside.read_bytes() == b'fixture-owned inside sentinel'
                assert source.read_bytes() == original and identity(source) == source_inode
                assert not (outside / 'missing-file').exists() and not (outside / 'missing-dir').exists()
                assert_no_staging(workspace)
            assert (workspace / 'outside-leaf').is_symlink() and (workspace / 'inside-leaf').is_symlink()
            assert (workspace / 'dangling-leaf').is_symlink() and (workspace / 'dangling-parent').is_symlink()
            assert (workspace / 'directory').is_dir() and stat.S_ISFIFO((workspace / 'fifo').stat().st_mode)
            assert sentinel.stat().st_nlink == inside.stat().st_nlink == 2
            assert host.request('/health')[0] == 200
            print('PASS 4: directory/FIFO, leaf and parent internal/external/dangling symlinks, both hardlink directions, absolute/traversal paths reject without touching sentinels or targets', flush=True)

            parts = ['bytes-limit'] + [letter * 200 for letter in 'abcd']
            prefix = '/'.join(parts)
            at_bytes = prefix + '/' + 'z' * (1024 - len(prefix.encode()) - 1)
            assert len(at_bytes.encode()) == 1024 and all(len(part.encode()) <= 255 for part in at_bytes.split('/'))
            with relative_parent(workspace, at_bytes, create=True):
                pass
            assert_copy(host.tool('copy', copy_args('source.bin', at_bytes)), 'source.bin', at_bytes, len(original))
            assert_copy(host.tool('file_copy', copy_args(at_bytes, 'bytes-roundtrip.bin')), at_bytes, 'bytes-roundtrip.bin', len(original))
            at_depth = '/'.join(['depth-limit'] + ['d'] * 62 + ['leaf'])
            assert len(at_depth.split('/')) == 64
            with relative_parent(workspace, at_depth, create=True):
                pass
            assert_copy(host.tool('file_copy', copy_args('source.bin', at_depth)), 'source.bin', at_depth, len(original))
            assert_copy(host.tool('copy', copy_args(at_depth, 'depth-roundtrip.bin')), at_depth, 'depth-roundtrip.bin', len(original))
            beyond_bytes = at_bytes + 'x'
            beyond_depth = '/'.join(['depth-over'] + ['d'] * 63 + ['leaf'])
            assert len(beyond_bytes.encode()) == 1025 and len(beyond_depth.split('/')) == 65
            for path in (beyond_bytes, beyond_depth):
                relative_put(workspace, path, b'fixture-owned beyond-budget file')
                bounded_inode = relative_identity(workspace, path)
                host.batch([('copy', copy_args(path, 'safety-guard.bin', True)),
                            ('file_copy', copy_args('source.bin', path, True))], rejected=True)
                assert relative_read(workspace, path) == b'fixture-owned beyond-budget file'
                assert relative_identity(workspace, path) == bounded_inode
                assert guard.read_bytes() == b'unchanged safety guard' and identity(guard) == guard_inode
                assert source.read_bytes() == original
            assert relative_read(workspace, at_bytes) == relative_read(workspace, at_depth) == original
            assert identity(workspace) == root_inode
            assert_no_staging(workspace)
            print('PASS 5: existing 1024-byte and 64-component paths accepted in both directions; actual 1025-byte/65-component files reject before any overwrite', flush=True)

            # Hold the real directory inode used by every cooperating writer.
            # This is a deterministic gate, without guessed I/O thread timing.
            descriptor = os.open(workspace, os.O_RDONLY)
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                rejected = host.batch([
                    ('copy', copy_args('source.bin', 'locked-copy.bin')),
                    ('file_copy', copy_args('source.bin', 'safety-guard.bin', True)),
                    ('write_file', {'path': 'locked-parent/new.txt', 'content': 'blocked'}),
                    ('file_write', {'path': 'safety-guard.bin', 'content': 'blocked'}),
                ], rejected=True)
                assert all('workspace write lock busy' in result['error'] for result in rejected)
                assert not (workspace / 'locked-copy.bin').exists() and not (workspace / 'locked-parent').exists()
                assert guard.read_bytes() == b'unchanged safety guard' and identity(guard) == guard_inode
                assert source.read_bytes() == original and identity(source) == source_inode
                assert_no_staging(workspace)
                assert host.request('/health')[0] == 200
            finally:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
                os.close(descriptor)
            # Each action is a new explicit request after the lock release.
            assert_copy(host.tool('copy', copy_args('source.bin', 'locked-copy.bin')),
                        'source.bin', 'locked-copy.bin', len(original))
            assert_copy(host.tool('file_copy', copy_args('source.bin', 'safety-guard.bin', True)),
                        'source.bin', 'safety-guard.bin', len(original), True)
            host.tool('write_file', {'path': 'locked-parent/new.txt', 'content': 'released'})
            host.tool('file_write', {'path': 'written-after-release.txt', 'content': 'released'})
            assert (workspace / 'locked-copy.bin').read_bytes() == guard.read_bytes() == original
            assert (workspace / 'locked-parent/new.txt').read_text() == 'released'
            assert (workspace / 'written-after-release.txt').read_text() == 'released'
            assert_no_staging(workspace)
            print('PASS 6: held actual shared workspace flock rejects copy/file_copy/write_file/file_write before any effect; release permits new explicit requests', flush=True)

        with Host(root / 'disabled', disabled=True) as host:
            guard = put(host.workspace, 'guard.txt', b'original authorized batch guard')
            guard_inode = identity(guard)
            code, listing = host.request('/api/tools')
            assert code == 200 and not copy_names & {tool['name'] for tool in listing['tools']}
            code, _body, count = host.chat([], enabled=[], disabled=True)
            assert code == 200 and count == 1
            for name in sorted(copy_names):
                calls = [('str_replace', {'path': 'guard.txt', 'old_str': 'original', 'new_str': 'changed'}),
                         (name, copy_args('guard.txt', 'forged.txt'))]
                code, _body, count = host.chat(calls, enabled=[], disabled=True)
                assert code >= 400 and count == 1, (name, code, count)
                assert guard.read_bytes() == b'original authorized batch guard' and identity(guard) == guard_inode
                assert not (host.workspace / 'forged.txt').exists()
                code, _body, count = host.chat([], enabled=[name], disabled=True)
                assert code >= 400 and count == 0, (name, code, count)
            code, _body, count = host.chat([], enabled=['unknown-copy-authority'], disabled=True)
            assert code >= 400 and count == 0
            code, metrics = host.request('/metrics', text=True)
            assert code == 200 and not any(line.startswith('jiaclaw_tool_calls_total{') for line in metrics.splitlines())
            assert_no_staging(host.workspace)
            print('PASS 7: disabled switch hides both HTTP/native names, forged batches have zero tool effects, invalid or disabled requested authority sends zero model calls', flush=True)
        with lock:
            assert not fixture_errors, fixture_errors
finally:
    model.shutdown()
    model.server_close()
    thread.join(timeout=5)
    assert not thread.is_alive(), 'copy fixture server did not stop'
