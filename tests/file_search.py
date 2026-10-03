#!/usr/bin/env python3
"""Real host/native grep+glob acceptance with a localhost model and fake keys.

No upstream model is called. Files, links and process state are disposable;
mutation races and blocking-worker cancellation use the Rust unit-test seams.
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
env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
model_key = 'search-model-fixture-' + uuid.uuid4().hex
host_key = 'search-host-fixture-' + uuid.uuid4().hex
file_limit = 256 * 1024
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
            if spec['disabled']:
                assert not names & {'grep', 'glob'}
                assert 'str_replace' in names
            else:
                assert names == set(spec['enabled'])
            if turn == 1 and spec['calls']:
                calls = [{'id': 'search-call-' + str(index), 'type': 'function',
                          'function': {'name': name, 'arguments': json.dumps(args, ensure_ascii=False)}}
                         for index, (name, args) in enumerate(spec['calls'])]
                message = {'role': 'assistant', 'content': None, 'tool_calls': calls}
                finish = 'tool_calls'
            else:
                assert turn == (2 if spec['calls'] else 1), 'unexpected model replay'
                if spec['calls']:
                    replies = [message for message in body['messages'] if message['role'] == 'tool']
                    assert [message['tool_call_id'] for message in replies] == [
                        'search-call-' + str(index) for index in range(len(spec['calls']))]
                message = {'role': 'assistant', 'content': 'local search fixture completed'}
                finish = 'stop'
            self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
        except Exception as error:
            with lock:
                errors.append(type(error).__name__ + ': ' + str(error))
            self.reply(500, {'error': 'local search fixture assertion failed'})


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
model_thread = threading.Thread(target=model.serve_forever, daemon=True)
model_thread.start()


class Host:
    def __init__(self, directory, disabled=False):
        self.workspace = directory / 'workspace'
        self.workspace.mkdir(parents=True)
        self.config = directory / 'config.json'
        self.log = directory / 'host.log'
        self.process = None
        self.config.write_text(json.dumps({
            'agent': {'name': 'file-search-fixture', 'description': 'Fixture',
                      'system_instructions': 'Use only approved native tools.',
                      'workspace_path': str(self.workspace), 'max_turns': 10,
                      'max_tool_iterations': 2},
            'provider': {'provider_type': 'brokerrouter', 'api_key': model_key,
                         'base_url': 'http://127.0.0.1:' + str(model.server_port), 'model': 'fixture'},
            'tools': {'grep': {'enabled': False}, 'glob': {'enabled': False}} if disabled else {},
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
            raise AssertionError('search fixture host startup timed out')
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

    def chat(self, calls, enabled=None, disabled=False):
        case = 'search-' + uuid.uuid4().hex
        if enabled is None:
            enabled = list(dict.fromkeys(name for name, _args in calls))
        with lock:
            specs[case] = {'calls': calls, 'enabled': enabled, 'disabled': disabled}
        code, body = self.request('/api/chat', {
            'session_id': case, 'messages': [{'role': 'user', 'content': case}],
            'enabled_tools': enabled, 'auto_skills': False,
        })
        with lock:
            assert not errors, errors
            count = len(observed.get(case, []))
        assert model_key not in json.dumps(body) and host_key not in json.dumps(body)
        return code, body, count

    def search(self, name, arguments, reject=False):
        code, body, count = self.chat([(name, arguments)])
        assert code == 200 and body['status'] == 'completed', (name, code, body.get('error'), count)
        assert count == 2 and len(body['tool_calls']) == 1
        record = body['tool_calls'][0]
        assert record['tool_name'] == name
        result = record['result']
        if reject:
            assert isinstance(result, dict) and result.get('error'), (name, result)
            return result
        assert isinstance(result, str), (name, result)
        assert len(result.encode()) <= result_limit, (name, 'serialized output exceeds 64 KiB')
        output = json.loads(result)
        assert output['match_count'] == len(output['matches'])
        if name == 'glob':
            assert output['matches'] == sorted(output['matches'])
        return output


def put(root, relative, contents):
    target = root / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(contents.encode() if isinstance(contents, str) else contents)
    return target


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-file-search-') as temporary:
        root = Path(temporary)
        with Host(root / 'enabled') as host:
            workspace = host.workspace
            put(workspace, 'normal/src/a.rs', 'first\nÄLPHA literal [x].+\nneedle needle\n')
            put(workspace, 'normal/src/deep/b.rs', 'needle\n')
            put(workspace, 'normal/readme.md', 'needle\n')
            put(workspace, 'normal/.hidden.md', 'needle\n')
            put(workspace, 'normal/.git/secret.rs', 'needle private-git-content\n')
            put(workspace, 'normal/src/.git/nested.rs', 'needle private-git-content\n')
            literal = host.search('grep', {'pattern': '[x].+', 'path': 'normal'})
            assert literal['matches'] == [{'path': 'normal/src/a.rs', 'line': 2,
                                           'snippet': 'ÄLPHA literal [x].+'}]
            folded = host.search('grep', {'pattern': 'älpha', 'case_insensitive': True, 'path': 'normal'})
            assert folded['matches'] == literal['matches']
            assert not host.search('grep', {'pattern': 'älpha', 'path': 'normal'})['matches']
            found = host.search('grep', {'pattern': 'needle', 'path': 'normal', 'glob': '*.rs'})
            assert [(item['path'], item['line']) for item in found['matches']] == [
                ('normal/src/a.rs', 3), ('normal/src/deep/b.rs', 1)]
            assert not found['truncated']
            scoped = host.search('glob', {'pattern': 'normal/src/**/*.rs', 'path': 'normal/src'})
            assert scoped['matches'] == ['normal/src/a.rs', 'normal/src/deep/b.rs']
            assert host.search('glob', {'pattern': '*.rs', 'path': 'normal/src'})['matches'] == scoped['matches']
            assert host.search('glob', {'pattern': 'normal/src/?.rs', 'path': 'normal/src'})['matches'] == ['normal/src/a.rs']
            all_paths = host.search('glob', {'pattern': '**/*', 'path': 'normal'})
            assert all_paths['matches'] == ['normal/.hidden.md', 'normal/readme.md',
                                           'normal/src/a.rs', 'normal/src/deep/b.rs']
            assert not all_paths['truncated']
            limited = host.search('glob', {'pattern': '**/*', 'path': 'normal', 'max_results': 1})
            assert limited['truncated'] and limited['matches'] == all_paths['matches'][:1]
            limited = host.search('grep', {'pattern': 'needle', 'path': 'normal', 'max_matches': 1})
            assert limited['truncated'] and limited['match_count'] == 1
            assert host.search('glob', {'pattern': '*.rs', 'path': 'normal/readme.md'})['match_count'] == 0
            assert host.search('glob', {'pattern': '*.md', 'path': 'normal/readme.md'})['matches'] == ['normal/readme.md']
            print('PASS: literal and Unicode lowercase matching, exact line IDs, filename/path globs, workspace-relative scope, sorted results, limits and .git exclusion', flush=True)

            put(workspace, 'sized/exact.txt', b'x' * (file_limit - 8) + b'\nneedle\n')
            put(workspace, 'sized/large.txt', b'x' * file_limit + b'!')
            put(workspace, 'sized/nul.txt', b'needle\x00')
            put(workspace, 'sized/invalid.txt', b'needle\xff')
            exact = host.search('grep', {'pattern': 'needle', 'path': 'sized/exact.txt'})
            assert exact['matches'] == [{'path': 'sized/exact.txt', 'line': 2, 'snippet': 'needle'}]
            directory = host.search('grep', {'pattern': 'needle', 'path': 'sized'})
            assert directory['matches'] == exact['matches'] and not directory['truncated']
            for path in ('sized/large.txt', 'sized/nul.txt', 'sized/invalid.txt'):
                host.search('grep', {'pattern': 'needle', 'path': path}, reject=True)
            assert host.search('glob', {'pattern': '*', 'path': 'sized'})['matches'] == [
                'sized/exact.txt', 'sized/invalid.txt', 'sized/large.txt', 'sized/nul.txt']
            for name in ('grep', 'glob'):
                for path in ('missing', '../outside', str(root)):
                    host.search(name, {'pattern': 'needle', 'path': path}, reject=True)
            print('PASS: 256 KiB exact text accepted, oversized/NUL/invalid UTF-8 skipped in trees and rejected directly; glob remains metadata-only; missing/escaping targets fail', flush=True)

            outside = root / 'outside'
            marker = 'OUTSIDE-SECRET-' + uuid.uuid4().hex
            sentinel = put(outside, 'secret.txt', marker)
            put(workspace, 'unsafe/inside.txt', 'needle inside')
            unsafe = workspace / 'unsafe'
            (unsafe / 'leaf').symlink_to(sentinel)
            (unsafe / 'parent').symlink_to(outside, target_is_directory=True)
            (unsafe / 'internal').symlink_to(unsafe / 'inside.txt')
            (unsafe / 'dangling').symlink_to(outside / 'missing')
            os.link(sentinel, unsafe / 'hardlink')
            os.mkfifo(unsafe / 'fifo')
            for name in ('grep', 'glob'):
                for path in ('leaf', 'parent/secret.txt', 'parent', 'internal', 'dangling', 'hardlink', 'fifo'):
                    result = host.search(name, {'pattern': '*' if name == 'glob' else marker,
                                               'path': 'unsafe/' + path}, reject=True)
                    assert marker not in json.dumps(result)
            safe_grep = host.search('grep', {'pattern': 'needle', 'path': 'unsafe'})
            assert [item['path'] for item in safe_grep['matches']] == ['unsafe/inside.txt']
            safe_glob = host.search('glob', {'pattern': '*', 'path': 'unsafe'})
            assert safe_glob['matches'] == ['unsafe/inside.txt']
            assert not host.search('grep', {'pattern': marker, 'path': 'unsafe'})['matches']
            assert sentinel.read_text() == marker and host.request('/health')[0] == 200
            print('PASS: direct leaf/parent/internal/dangling links, hardlinks and FIFO refused; recursive search skips unsafe entries without returning outside content', flush=True)

            wide = workspace / 'wide'
            wide.mkdir()
            for index in range(2005):
                (wide / ('empty-' + str(index))).mkdir()
            deep = workspace / 'deep'
            cursor = deep
            for _ in range(34):
                cursor = cursor / 'd'
            put(cursor, 'too-deep.txt', 'needle')
            for name in ('grep', 'glob'):
                for path in ('wide', 'deep'):
                    result = host.search(name, {'pattern': 'needle' if name == 'grep' else '*', 'path': path})
                    assert result['truncated'] and not result['matches'], (name, path, result)
                    assert host.request('/health')[0] == 200
            # Actual-byte budget: 65 individually admissible full-size files
            # cannot all be inspected under the 16 MiB aggregate read limit.
            for index in range(65):
                put(workspace, 'budget/' + f'{index:03}.txt', b'x' * file_limit)
            result = host.search('grep', {'pattern': 'not-present', 'path': 'budget'})
            assert result['truncated'] and not result['matches']
            filtered = host.search('grep', {'pattern': 'not-present', 'path': 'budget', 'glob': '*.rs'})
            assert not filtered['truncated'] and not filtered['matches']
            # Small files must not be charged as 256 KiB reservations.
            for index in range(80):
                put(workspace, 'small/' + f'{index:03}.txt', 'small needle')
            small = host.search('grep', {'pattern': 'needle', 'path': 'small', 'max_matches': 200})
            assert small['match_count'] == 80 and not small['truncated']
            print('PASS: all-entry/depth and actual aggregate-byte budgets truncate, filtered bodies cost no read budget, 80 small files remain searchable and host stays responsive', flush=True)

            # Each path and snippet is individually valid; their escaped pretty
            # JSON together exceeds 64 KiB. The tool must return bounded JSON.
            for index in range(400):
                name = f'{index:03}-' + '"' * 150 + '.txt'
                put(workspace, 'output/' + name, 'needle ' + '界' * 220 + '   \n')
            for name in ('grep', 'glob'):
                arguments = {'pattern': 'needle' if name == 'grep' else '*', 'path': 'output',
                             'max_matches' if name == 'grep' else 'max_results': 200 if name == 'grep' else 500}
                bounded = host.search(name, arguments)
                assert bounded['truncated'] and bounded['matches']
                assert bounded['match_count'] < (200 if name == 'grep' else 400)
                if name == 'grep':
                    assert all(len(item['snippet']) <= 201 and item['snippet'].endswith('…')
                               for item in bounded['matches'])
            # A nonmatching many-** pattern previously caused combinatorial
            # recursion. Read a single target so scan limits cannot mask it.
            deep_file = 'matcher/' + '/'.join('a' for _ in range(22)) + '/leaf.txt'
            put(workspace, deep_file, 'needle')
            adversarial = '/'.join(['**'] * 20 + ['missing'])
            assert host.search('glob', {'path': deep_file, 'pattern': adversarial})['match_count'] == 0
            assert host.request('/health')[0] == 200
            print('PASS: escaped paths/Unicode snippets keep complete pretty JSON within 64 KiB; adversarial many-** pattern completes without recursive explosion', flush=True)

        with Host(root / 'disabled', disabled=True) as host:
            original = b'original must remain unchanged'
            put(host.workspace, 'guard.txt', original)
            code, catalog = host.request('/api/tools')
            assert code == 200 and not {'grep', 'glob'} & {tool['name'] for tool in catalog['tools']}
            code, _body, count = host.chat([], enabled=[], disabled=True)
            assert code == 200 and count == 1
            for name in ('grep', 'glob'):
                calls = [('str_replace', {'path': 'guard.txt', 'old_str': 'original', 'new_str': 'changed'}),
                         (name, {'pattern': '*', 'path': '.'})]
                code, _body, count = host.chat(calls, enabled=[], disabled=True)
                assert code >= 400 and count == 1
                assert (host.workspace / 'guard.txt').read_bytes() == original
                code, _body, count = host.chat([], enabled=[name], disabled=True)
                assert code >= 400 and count == 0
            code, metrics = host.request('/metrics', text=True)
            assert code == 200
            assert not any(line.startswith('jiaclaw_tool_calls_total{') for line in metrics.splitlines())
            print('PASS: disabled search tools disappear from HTTP/native catalogs; forged batches execute no earlier authorized tool; invalid caller authority sends no model request', flush=True)
finally:
    model.shutdown()
    model.server_close()
    model_thread.join(timeout=5)
