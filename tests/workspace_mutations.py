#!/usr/bin/env python3
"""Real host/native mkdir+move acceptance; localhost model and fake keys only.

Checks same-volume namespace changes, preserved bytes/inodes and fail-closed
boundaries. Cross-device EXDEV and deterministic rename races are Rust tests;
this fixture neither mounts filesystems nor claims cross-volume coverage.
"""
import fcntl
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

from tool_batches import assert_tool_batch


binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
secret = 'mutations-model-fixture-' + uuid.uuid4().hex
api_token = 'mutations-host-fixture-' + uuid.uuid4().hex
mutation_names = {'mkdir', 'move'}
cases, observations, fixture_errors = {}, {}, []
lock = threading.Lock()


def tool_call(name, arguments, index):
    return {'id': 'mutation-call-' + str(index), 'type': 'function',
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
                assert not mutation_names & catalog.keys(), 'disabled mutation tool in native catalog'
                assert 'str_replace' in catalog
            else:
                assert set(catalog) == set(spec['enabled']), 'request authority changed'
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
                        'mutation-call-' + str(index) for index in range(len(spec['calls']))]
                message = {'role': 'assistant', 'content': 'local mutation fixture completed'}
                finish = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except Exception as error:
            with lock:
                fixture_errors.append(type(error).__name__ + ': ' + str(error))
            self.reply(500, {'error': 'local mutation fixture assertion failed'})


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
            'agent': {'name': 'workspace-mutations-fixture', 'description': 'Fixture',
                      'system_instructions': 'Use only authorized native tools.',
                      'workspace_path': str(self.workspace), 'max_turns': 10,
                      'max_tool_iterations': 2},
            'provider': {'provider_type': 'brokerrouter', 'api_key': secret,
                         'base_url': 'http://127.0.0.1:' + str(model.server_port), 'model': 'fixture'},
            'tools': {name: {'enabled': False} for name in mutation_names} if disabled else {},
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
            raise AssertionError('mutation fixture host startup timed out')
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

    def chat(self, calls, enabled=None, disabled=False):
        case = 'workspace-' + uuid.uuid4().hex
        if enabled is None:
            enabled = list(dict.fromkeys(name for name, _args in calls))
        with lock:
            cases[case] = {'calls': calls, 'enabled': enabled, 'disabled': disabled}
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
        return assert_tool_batch(self.chat, calls, rejected)

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


def move_args(source, destination, overwrite=False):
    return {'from': source, 'to': destination, 'overwrite': overwrite}


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-workspace-mutations-') as temporary:
        root = Path(temporary)
        with Host(root / 'enabled') as host:
            workspace = host.workspace
            code, catalog = host.request('/api/tools')
            assert code == 200 and mutation_names <= {tool['name'] for tool in catalog['tools']}
            root_inode = identity(workspace)
            for recursive in (True, False):
                root_result = host.tool('mkdir', {'path': '.', 'recursive': recursive})
                assert root_result == {'path': '.', 'created': False, 'existed': True, 'recursive': recursive}
                assert identity(workspace) == root_inode
            created = host.tool('mkdir', {'path': 'notes/深层'})
            assert created == {'path': 'notes/深层', 'created': True, 'existed': False, 'recursive': True}
            inode = identity(workspace / 'notes/深层')
            existed = host.tool('mkdir', {'path': 'notes/深层', 'recursive': False})
            assert existed == {'path': 'notes/深层', 'created': False, 'existed': True, 'recursive': False}
            assert identity(workspace / 'notes/深层') == inode
            child = host.tool('mkdir', {'path': 'notes/one', 'parents': False})
            assert child['created'] and not child['recursive']
            host.tool('mkdir', {'path': 'absent/child', 'recursive': False}, rejected=True)
            assert not (workspace / 'absent').exists()
            host.tool('mkdir', {'path': 'conflicting', 'recursive': True, 'parents': False}, rejected=True)
            assert not (workspace / 'conflicting').exists()
            regular = put(workspace, 'regular.txt', 'original')
            os.mkfifo(workspace / 'pipe')
            for path in ('regular.txt', 'regular.txt/child', 'pipe', 'pipe/child'):
                host.tool('mkdir', {'path': path}, rejected=True)
            assert regular.read_text() == 'original' and stat.S_ISFIFO((workspace / 'pipe').lstat().st_mode)
            print('PASS: mkdir recursive creation and parents alias, idempotent inode, missing-parent/non-directory/FIFO/conflicting options reject without effects', flush=True)

            payload = b'\x00\xffbinary\n' + bytes(range(256)) * 4096
            source = put(workspace, 'binary-source.bin', payload)
            inode = identity(source)
            moved = host.tool('move', {'from': 'binary-source.bin', 'source': 'binary-source.bin',
                                       'to': 'binary-final.bin', 'destination': 'binary-final.bin'})
            assert moved == {'from': 'binary-source.bin', 'to': 'binary-final.bin',
                             'overwrite': False, 'overwritten': False, 'kind': 'file'}
            assert not source.exists()
            assert identity(workspace / 'binary-final.bin') == inode
            assert (workspace / 'binary-final.bin').read_bytes() == payload
            for source_key, destination_key in [('source', 'destination'), ('source', 'to'), ('from', 'destination')]:
                source_name = source_key + '-' + destination_key + '-source'
                destination_name = source_key + '-' + destination_key + '-target'
                put(workspace, source_name, 'alias bytes')
                aliased = host.tool('move', {source_key: source_name, destination_key: destination_name})
                assert aliased == {'from': source_name, 'to': destination_name, 'overwrite': False,
                                   'overwritten': False, 'kind': 'file'}
                assert not (workspace / source_name).exists()
                assert (workspace / destination_name).read_text() == 'alias bytes'
            outside = root / 'outside'
            sentinel = put(outside, 'sentinel.txt', b'outside must remain unchanged')
            outside_bytes = sentinel.read_bytes()
            tree = workspace / 'bundle'
            put(tree, 'nested/inside.txt', '完整目录 payload')
            (tree / 'external-link').symlink_to(sentinel)
            os.mkfifo(tree / 'nested-pipe')
            inode = identity(tree)
            link_inode = identity(tree / 'external-link')
            directory = host.tool('move', move_args('bundle', 'bundle-moved'))
            assert directory == {'from': 'bundle', 'to': 'bundle-moved', 'overwrite': False,
                                 'overwritten': False, 'kind': 'dir'}
            assert not tree.exists() and identity(workspace / 'bundle-moved') == inode
            assert (workspace / 'bundle-moved/nested/inside.txt').read_text() == '完整目录 payload'
            assert identity(workspace / 'bundle-moved/external-link') == link_inode
            assert os.readlink(workspace / 'bundle-moved/external-link') == str(sentinel)
            assert stat.S_ISFIFO((workspace / 'bundle-moved/nested-pipe').lstat().st_mode)
            assert sentinel.read_bytes() == outside_bytes
            print('PASS: same-volume binary file and nonempty directory moves preserve inode/bytes; contained links and FIFO move untouched without traversal', flush=True)

            old = put(workspace, 'replace-source', b'new exact bytes')
            target = put(workspace, 'replace-target', b'old exact bytes')
            old_inode, target_inode = identity(old), identity(target)
            host.tool('move', move_args('replace-source', 'replace-target'), rejected=True)
            assert old.read_bytes() == b'new exact bytes' and target.read_bytes() == b'old exact bytes'
            assert identity(old) == old_inode and identity(target) == target_inode
            replaced = host.tool('move', move_args('replace-source', 'replace-target', True))
            assert replaced['overwrite'] and replaced['overwritten'] and replaced['kind'] == 'file'
            assert not old.exists() and identity(target) == old_inode and target.read_bytes() == b'new exact bytes'
            put(workspace, 'source-dir/nested/keep', b'directory replacement')
            (workspace / 'target-dir').mkdir()
            source_inode, target_inode = identity(workspace / 'source-dir'), identity(workspace / 'target-dir')
            host.tool('move', move_args('source-dir', 'target-dir'), rejected=True)
            assert identity(workspace / 'source-dir') == source_inode and identity(workspace / 'target-dir') == target_inode
            replaced = host.tool('move', move_args('source-dir', 'target-dir', True))
            assert replaced['overwritten'] and replaced['kind'] == 'dir'
            assert not (workspace / 'source-dir').exists() and identity(workspace / 'target-dir') == source_inode
            assert (workspace / 'target-dir/nested/keep').read_bytes() == b'directory replacement'
            put(workspace, 'full-source/a', b'source')
            put(workspace, 'full-target/b', b'target')
            for args in (move_args('full-source', 'full-target', True),
                         move_args('replace-target', 'full-target', True),
                         move_args('full-source', 'replace-target', True),
                         move_args('full-source', 'full-source', True),
                         move_args('full-source', 'full-source/new-child', True),
                         move_args('replace-target', 'replace-target', True),
                         move_args('replace-target', 'missing-parent/target'),
                         move_args('absent-source', 'new-target'),
                         {'from': 'replace-target', 'source': 'different', 'to': 'new-target'}):
                host.tool('move', args, rejected=True)
                assert (workspace / 'full-source/a').read_bytes() == b'source'
                assert (workspace / 'full-target/b').read_bytes() == b'target'
                assert (workspace / 'replace-target').read_bytes() == b'new exact bytes'
                assert not (workspace / 'full-source/new-child').exists()
                assert not (workspace / 'missing-parent').exists() and not (workspace / 'new-target').exists()
            print('PASS: default no-overwrite and explicit same-type file/empty-directory replacement; nonempty/mixed/same/descendant/missing-parent errors preserve both sides', flush=True)

            safe = put(workspace, 'safe-source', b'safe source unchanged')
            for args in (move_args('.', 'root-moved'), move_args('safe-source', '.', True)):
                host.tool('move', args, rejected=True)
                assert identity(workspace) == root_inode and safe.read_bytes() == b'safe source unchanged'
                assert not (workspace / 'root-moved').exists()
            (workspace / 'leaf-link').symlink_to(sentinel)
            (workspace / 'parent-link').symlink_to(outside, target_is_directory=True)
            (workspace / 'inside-link').symlink_to(workspace / 'notes', target_is_directory=True)
            (workspace / 'dangling-link').symlink_to(outside / 'missing')
            os.link(sentinel, workspace / 'hardlink')
            unsafe = ('leaf-link', 'parent-link/sentinel.txt', 'parent-link',
                      'inside-link', 'dangling-link', 'hardlink', 'pipe')
            for path in unsafe:
                host.tool('mkdir', {'path': path}, rejected=True)
                host.tool('move', move_args(path, 'should-not-exist'), rejected=True)
                host.tool('move', move_args('safe-source', path, True), rejected=True)
                assert safe.read_bytes() == b'safe source unchanged'
                assert sentinel.read_bytes() == outside_bytes
                assert not (workspace / 'should-not-exist').exists()
            for path in ('parent-link/new/deeper', 'inside-link/new', 'dangling-link/new'):
                host.tool('mkdir', {'path': path}, rejected=True)
                host.tool('move', move_args('safe-source', path), rejected=True)
            for path in ('../outside/sentinel.txt', str(sentinel)):
                host.tool('mkdir', {'path': path}, rejected=True)
                host.tool('move', move_args('safe-source', path, True), rejected=True)
                host.tool('move', move_args(path, 'should-not-exist'), rejected=True)
            assert not (outside / 'new').exists() and not (outside / 'missing').exists()
            assert not (workspace / 'notes/new').exists()
            assert (workspace / 'leaf-link').is_symlink() and (workspace / 'dangling-link').is_symlink()
            assert (workspace / 'hardlink').read_bytes() == outside_bytes
            assert sentinel.read_bytes() == outside_bytes and safe.read_bytes() == b'safe source unchanged'
            print('PASS: leaf/parent/internal/dangling symlinks, hardlinks, FIFO and escaping move/mkdir paths fail closed; source and outside sentinel unchanged', flush=True)

            depth64 = '/'.join(['depth64'] + ['d'] * 63)
            assert host.tool('mkdir', {'path': depth64})['created']
            assert (workspace / depth64).is_dir()
            too_deep = '/'.join(['too-deep'] + ['d'] * 64)
            too_long = '/'.join(['x' * 204] * 5) + 'x'
            assert len(too_long.encode()) == 1025
            for path in (too_deep, too_long):
                host.tool('mkdir', {'path': path}, rejected=True)
                host.tool('move', move_args('safe-source', path), rejected=True)
                host.tool('move', move_args(path, 'should-not-exist'), rejected=True)
                assert not (workspace / path.split('/')[0]).exists()
            assert safe.read_bytes() == b'safe source unchanged'
            # Exercise the actual workspace inode lock rather than relying on
            # scheduler timing or inducing a nondeterministic mutation race.
            descriptor = os.open(workspace, os.O_RDONLY)
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                host.tool('mkdir', {'path': 'lock-created'}, rejected=True)
                host.tool('move', move_args('safe-source', 'lock-moved'), rejected=True)
                assert safe.exists() and not (workspace / 'lock-created').exists()
                assert not (workspace / 'lock-moved').exists()
                assert host.request('/health')[0] == 200
            finally:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
                os.close(descriptor)
            assert host.tool('mkdir', {'path': 'lock-created'})['created']
            assert host.tool('move', move_args('safe-source', 'lock-moved'))['kind'] == 'file'
            assert not safe.exists() and (workspace / 'lock-moved').read_bytes() == b'safe source unchanged'
            print('PASS: 64 components accepted, over-64/1024-byte paths reject before changes; shared workspace lock prevents both mutations and release restores operation', flush=True)

        with Host(root / 'disabled', disabled=True) as host:
            original = b'original batch guard'
            put(host.workspace, 'guard.txt', original)
            code, catalog = host.request('/api/tools')
            assert code == 200 and not mutation_names & {tool['name'] for tool in catalog['tools']}
            code, _body, count = host.chat([], enabled=[], disabled=True)
            assert code == 200 and count == 1
            for name, arguments in [('mkdir', {'path': 'forged'}),
                                    ('move', move_args('guard.txt', 'forged'))]:
                calls = [('str_replace', {'path': 'guard.txt', 'old_str': 'original', 'new_str': 'changed'}),
                         (name, arguments)]
                code, _body, count = host.chat(calls, enabled=[], disabled=True)
                assert code >= 400 and count == 1
                assert (host.workspace / 'guard.txt').read_bytes() == original
                assert not (host.workspace / 'forged').exists()
                code, _body, count = host.chat([], enabled=[name], disabled=True)
                assert code >= 400 and count == 0
            code, metrics = host.request('/metrics', text=True)
            assert code == 200
            assert not any(line.startswith('jiaclaw_tool_calls_total{') for line in metrics.splitlines())
            print('PASS: disabled mkdir/move absent from HTTP/native catalogs; forged mixed batches have zero tool effects, explicit disabled authority sends no model request', flush=True)
finally:
    model.shutdown()
    model.server_close()
    thread.join(timeout=5)
