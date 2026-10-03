#!/usr/bin/env python3
"""Real semantic CLI/native-tool acceptance against a disposable local gateway.

No real model or paid credential is used. This verifies protocol, freshness,
private state and persistent uncertainty; synthetic vectors do not certify
retrieval quality or a real Brokerrouter deployment.
"""
import copy
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
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
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'off'
secret = 'semantic-fixture-' + uuid.uuid4().hex
rotated_secret = 'semantic-rotated-' + uuid.uuid4().hex
api_token = 'semantic-host-' + uuid.uuid4().hex
model = 'fixture-embedding-v1'
lock = threading.Lock()
posts, gets, chats, errors, behaviors = [], [], [], [], []
receipts, specs = {}, {}


def vector(text):
    lowered = text.lower()
    if 'tea' in lowered or '热饮' in text:
        return [1.0, 0.0, 0.0, 0.0]
    if 'rust' in lowered or '编程' in text:
        return [0.0, 1.0, 0.0, 0.0]
    return [0.0, 0.0, 1.0, 0.0]


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, status, value, request_id=None):
        payload = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        if request_id is not None:
            self.send_header('x-brokerrouter-request-id', request_id)
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        try:
            assert self.headers.get('Authorization') in {'Bearer ' + secret, 'Bearer ' + rotated_secret}
            key = self.headers.get('Idempotency-Key')
            assert key, 'missing durable request identity'
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/embeddings':
                texts = body['input']
                assert body['model'] == model, body
                assert body.get('encoding_format') == 'float', body
                assert body.get('dimensions') == 4, body
                assert isinstance(texts, list) and 1 <= len(texts) <= 8, body
                assert all(isinstance(text, str) and text.strip() for text in texts), body
                request_id = str(uuid.uuid4())
                response = {'object': 'list', 'model': model,
                            'data': [{'object': 'embedding', 'index': index, 'embedding': vector(text)}
                                     for index, text in enumerate(texts)],
                            'usage': {'prompt_tokens': 3 * len(texts), 'total_tokens': 3 * len(texts)}}
                with lock:
                    assert key not in {entry['key'] for entry in posts}, 'embedding POST replayed'
                    posts.append({'key': key, 'body': body, 'remote_id': request_id})
                    receipts[request_id] = response
                    behavior = behaviors.pop(0) if behaviors else None
                if behavior == 'disconnect':
                    self.close_connection = True
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                elif isinstance(behavior, tuple):
                    submitted, released = behavior
                    submitted.set()
                    assert released.wait(10), 'kill fixture was not released'
                    # The caller was killed after submission. No response or
                    # remote request ID was delivered before process death.
                    self.close_connection = True
                elif behavior == 'bad-result':
                    self.reply(200, {'object': 'list', 'model': model, 'data': []}, request_id)
                else:
                    if callable(behavior):
                        behavior()
                    self.reply(200, response, request_id)
            elif self.path == '/v1/chat/completions':
                user = next(item['content'] for item in reversed(body['messages']) if item['role'] == 'user')
                case = re.search(r'semantic-case:([a-z0-9_-]+)', user).group(1)
                with lock:
                    chats.append((case, body))
                    arguments = specs[case]
                if body['messages'][-1]['role'] != 'tool':
                    assert {tool['function']['name'] for tool in body.get('tools', [])} == {'memory_search'}
                    message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                        'id': 'semantic-' + case, 'type': 'function',
                        'function': {'name': 'memory_search', 'arguments': json.dumps(arguments)},
                    }]}
                    finish = 'tool_calls'
                else:
                    message = {'role': 'assistant', 'content': 'Local fixture completed ' + case}
                    finish = 'stop'
                self.reply(200, {'choices': [{'message': message, 'finish_reason': finish}]})
            else:
                raise AssertionError('unexpected POST endpoint: ' + self.path)
        except Exception as error:
            with lock:
                errors.append(repr(error))
            try:
                self.reply(500, {'error': 'fixture assertion failed'})
            except OSError:
                pass

    def do_GET(self):
        try:
            assert self.headers.get('Authorization') in {'Bearer ' + secret, 'Bearer ' + rotated_secret}
            match = re.fullmatch(r'/v1/requests/([0-9a-f-]{36})/result', self.path)
            assert match, self.path
            request_id = match.group(1)
            with lock:
                gets.append(request_id)
                response = receipts[request_id]
            # The real GET result contract does not repeat the POST request-id header.
            self.reply(200, response)
        except Exception as error:
            with lock:
                errors.append(repr(error))
            self.reply(500, {'error': 'fixture assertion failed'})


gateway = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
gateway.daemon_threads = True
threading.Thread(target=gateway.serve_forever, daemon=True).start()


def counts():
    with lock:
        assert not errors, errors
        return len(posts), len(gets)


def arm(*next_behaviors):
    with lock:
        assert not behaviors, behaviors
        behaviors.extend(next_behaviors)


def run(*arguments):
    result = subprocess.run([str(binary), *map(str, arguments)], env=env, capture_output=True,
                            text=True, timeout=25)
    for token in (secret, rotated_secret, api_token):
        assert token not in result.stdout + result.stderr, 'credential leaked by CLI'
    return result


def cli(config_path, *arguments, fail=None):
    result = run('memory', 'semantic', *arguments, '--config', config_path)
    counts()
    if fail is not None:
        assert result.returncode != 0, (arguments, result.stdout, result.stderr)
        if fail:
            assert fail in result.stdout + result.stderr, (arguments, fail, result.stdout, result.stderr)
        return result
    assert result.returncode == 0, (arguments, result.stdout, result.stderr)
    return json.loads(result.stdout)


def write_config(path, config):
    path.write_text(json.dumps(config))


def new_case(root, name, enabled=True, two_sources=False):
    directory = root / name
    directory.mkdir(mode=0o700)
    workspace = directory / 'workspace'
    workspace.mkdir(mode=0o700)
    (workspace / 'MEMORY.md').write_text('The user prefers tea.\n')
    sources = []
    if two_sources:
        (workspace / 'notes').mkdir()
        (workspace / 'notes/work.md').write_text('The user writes Rust.\n')
        (workspace / 'notes/private.md').write_text('Not an authorized embedding source.\n')
        sources = ['MEMORY.md', 'notes/work.md']
    config = {
        'agent': {'name': 'semantic-fixture', 'description': 'Offline protocol fixture',
                  'system_instructions': 'Use only approved tools.', 'max_turns': 10,
                  'max_tool_iterations': 2, 'workspace_path': str(workspace)},
        'provider': {'provider_type': 'brokerrouter',
                     'base_url': 'http://127.0.0.1:' + str(gateway.server_port),
                     'api_key': secret, 'model': 'fixture-chat'},
        'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
        'memory': {'path': 'MEMORY.md', 'semantic': {
            'enabled': enabled, 'model': model, 'space_revision': 'fixture-revision-1',
            'dimensions': 4, 'sources': sources,
            'index_path': '../state/semantic/index.sqlite3', 'timeout_secs': 2}},
    }
    path = directory / 'config.json'
    write_config(path, config)
    return directory, workspace, path, config


class Host:
    def __init__(self, directory, config_path):
        self.config_path = config_path
        self.log = directory / 'host.log'
        self.process = None

    def __enter__(self):
        host_env = dict(env, JIACLAW_LOG_LEVEL='info')
        with self.log.open('wb') as output:
            self.process = subprocess.Popen([str(binary), 'serve', '--config', str(self.config_path)],
                                            env=host_env, stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert self.process.poll() is None, self.log.read_text()
                match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', self.log.read_text())
                if match:
                    self.base = match.group(1)
                    return self
                time.sleep(.05)
            raise AssertionError('host startup timeout: ' + self.log.read_text())
        except Exception:
            self.__exit__()
            raise

    def __exit__(self, *_arguments):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        for token in (secret, rotated_secret, api_token):
            assert token not in self.log.read_text(), 'credential leaked by host'

    def tool(self, case, arguments, reject=False, schema_reject=False):
        with lock:
            specs[case] = arguments
        body = {'session_id': 'semantic-' + case,
                'messages': [{'role': 'user', 'content': 'semantic-case:' + case}],
                'enabled_tools': ['memory_search'], 'auto_skills': False}
        request = urllib.request.Request(self.base + '/api/chat', data=json.dumps(body).encode(),
                                         headers={'Authorization': 'Bearer ' + api_token,
                                                  'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                assert response.status == 200
                result = json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode()
            counts()
            if schema_reject:
                assert error.code == 500 and 'no tools dispatched' in detail, (case, error.code, detail)
                return detail
            raise AssertionError((case, error.code, detail, self.log.read_text())) from error
        assert not schema_reject, (case, result)
        counts()
        assert len(result['tool_calls']) == 1, result
        record = result['tool_calls'][0]
        assert record['tool_name'] == 'memory_search', record
        if reject:
            assert isinstance(record['result'], dict) and record['result'].get('error'), record
            return record['result']['error']
        assert isinstance(record['result'], str), record
        return json.loads(record['result'])


def observe_semantic_state(db):
    # A separate process must open AND close SQLite while serve still owns it.
    # mode=rw refuses to create a missing DB; query_only permits only reads while
    # retaining ordinary SQLite WAL lifecycle behavior (unlike immutable mode).
    script = """import json, pathlib, sqlite3, sys
connection = sqlite3.connect(pathlib.Path(sys.argv[1]).as_uri() + '?mode=rw', uri=True, timeout=2)
try:
    connection.execute('PRAGMA query_only=ON')
    assert connection.execute('PRAGMA query_only').fetchone() == (1,)
    assert connection.execute('PRAGMA journal_mode').fetchone() == ('wal',)
    rows = connection.execute(
        'SELECT id, kind, remote_id, state, receipt IS NOT NULL FROM operations ORDER BY seq'
    ).fetchall()
    generation = connection.execute('SELECT chunk_count FROM generation WHERE id=1').fetchone()
    print(json.dumps({'operations': rows, 'chunks': generation[0] if generation else None}))
finally:
    connection.close()
"""
    result = subprocess.run([sys.executable, '-c', script, str(db)], env=env,
                            capture_output=True, text=True, timeout=5)
    assert result.returncode == 0, ('independent SQLite observer failed', result.stderr)
    return json.loads(result.stdout)


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-semantic-') as temporary:
        root = Path(temporary).resolve()
        directory, workspace, config_path, config = new_case(root, 'disabled', enabled=False)
        before = counts()
        cli(config_path, 'status', fail='semantic_memory_disabled')
        with Host(directory, config_path) as host:
            keyword = host.tool('disabled-keyword', {'query': 'tea'})
            assert keyword['matches'] and 'tea' in keyword['matches'][0]['excerpt']
            host.tool('disabled-semantic', {'query': 'tea', 'mode': 'semantic'}, schema_reject=True)
        assert counts() == before
        assert not (directory / 'state/semantic/index.sqlite3').exists()
        print('PASS: disabled semantic memory creates no index and sends no embedding requests; keyword search remains available')

        directory, workspace, config_path, config = new_case(root, 'ready', two_sources=True)
        before = counts()
        cli(config_path, 'search', 'hot drink before indexing', fail='stale_index')
        assert counts() == before
        refresh = cli(config_path, 'refresh')
        assert refresh['status'] == 'ready' and refresh['sources'] == 2 and refresh['chunks'] == 2, refresh
        indexed = counts()
        assert indexed[0] > before[0]
        cli(config_path, 'refresh')
        assert counts() == indexed, 'unchanged source bodies were billed again'
        result = cli(config_path, 'search', '热饮偏好', '--max-results', '1')
        assert result['mode'] == 'semantic' and len(result['matches']) == 1, result
        assert result['matches'][0]['path'] == 'MEMORY.md' and 'tea' in result['matches'][0]['excerpt'], result
        for hit in result['matches']:
            assert hit['line'] >= 1 and isinstance(hit['score'], (float, int)), hit
        # Every CLI invocation is a fresh process; both generation and receipts survive.
        assert cli(config_path, 'status')['pending'] is None
        again = cli(config_path, 'search', '编程偏好', '--max-results', '1')
        assert again['matches'][0]['path'] == 'notes/work.md', again
        db = directory / 'state/semantic/index.sqlite3'
        assert stat.S_IMODE(db.stat().st_mode) & 0o077 == 0
        assert stat.S_IMODE(db.parent.stat().st_mode) & 0o077 == 0
        with Host(directory, config_path) as host:
            before = counts()
            cli(config_path, 'status', fail='')  # Lifetime owner is serve, including read-only status.
            assert counts() == before
            semantic = host.tool('native-semantic', {'query': 'tea details', 'mode': 'semantic',
                                                    'max_results': 1, 'paths': ['MEMORY.md']})
            assert semantic['mode'] == 'semantic' and semantic['matches'], semantic
            assert all(hit['path'] == 'MEMORY.md' for hit in semantic['matches']), semantic
            before = counts()
            keyword = host.tool('native-keyword', {'query': 'tea'})
            assert keyword['matches'] and all('tea' in hit['excerpt'] for hit in keyword['matches']), keyword
            assert counts() == before
            host.tool('unauthorized-path', {'query': 'private', 'mode': 'semantic',
                                           'paths': ['notes/private.md']}, reject=True)
            host.tool('escaping-path', {'query': 'private', 'mode': 'semantic',
                                       'paths': ['../outside.md']}, reject=True)
            assert counts() == before, 'path denial must precede billing'
        print('PASS: explicit refresh/query persist across processes; native mode, source subset, keyword default and exclusive ownership work')

        memory = workspace / 'MEMORY.md'
        original = memory.read_bytes()
        stamp = memory.stat()
        memory.write_bytes(original.replace(b'tea', b'ale'))
        os.utime(memory, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        before = counts()
        cli(config_path, 'search', 'tea after edit', fail='stale_index')
        assert counts() == before, 'same-size, same-mtime edits must be caught before query billing'
        memory.write_bytes(original)
        cli(config_path, 'refresh')
        memory.unlink()
        before = counts()
        cli(config_path, 'search', 'tea after delete', fail='stale_index')
        assert counts() == before, 'deleted source must not be searched or billed'
        memory.write_bytes(original)
        cli(config_path, 'refresh')
        arm(lambda: memory.write_bytes(original.replace(b'tea', b'ale')))
        cli(config_path, 'search', 'tea changed during response', fail='stale_index')
        memory.write_bytes(original)
        cli(config_path, 'refresh')
        before = counts()
        assert cli(config_path, 'rebuild')['status'] == 'cleared'
        cli(config_path, 'search', 'tea after rebuild', fail='stale_index')
        assert counts() == before
        assert cli(config_path, 'refresh')['status'] == 'ready'
        assert counts() == before, 'rebuild must retain validated receipts, not rebill the same batches'
        print('PASS: content hashes detect edits/deletion; in-flight changes reject results; rebuild clears only derived index')

        directory, workspace, config_path, config = new_case(root, 'observer-lifetime')
        assert cli(config_path, 'refresh')['status'] == 'ready'
        db = directory / 'state/semantic/index.sqlite3'
        before = counts()
        query_before = 'tea before independent SQLite observer'
        query_after = 'tea after independent SQLite observer'
        with Host(directory, config_path) as host:
            first = host.tool('observer-before', {'query': query_before, 'mode': 'semantic'})
            assert first['matches'] and first['matches'][0]['path'] == 'MEMORY.md', first
            assert counts() == (before[0] + 1, before[1])
            sidecars = [db.with_name(db.name + suffix) for suffix in ('-wal', '-shm')]
            identities = [(sidecar.stat().st_dev, sidecar.stat().st_ino) for sidecar in sidecars]
            snapshot = observe_semantic_state(db)
            assert snapshot['chunks'] == 1 and len(snapshot['operations']) == 2, snapshot
            assert all(row[3:] == ['completed', 1] for row in snapshot['operations']), snapshot
            assert host.process.poll() is None, host.log.read_text()
            assert all(sidecar.exists() for sidecar in sidecars), (
                'closing an independent observer removed the live semantic WAL/SHM', snapshot)
            assert [(sidecar.stat().st_dev, sidecar.stat().st_ino) for sidecar in sidecars] == identities
            with lock:
                posted_before = dict(posts[before[0]])
            assert snapshot['operations'][-1][:4] == [
                posted_before['key'], 'query', posted_before['remote_id'], 'completed'], snapshot

            # A fresh embedding must persist after the observer has closed. An
            # unlinked WAL can appear successful in serve while new readers see
            # only the old snapshot, so the tool result alone is insufficient.
            second = host.tool('observer-after', {'query': query_after, 'mode': 'semantic'})
            assert second['matches'] and second['matches'][0]['path'] == 'MEMORY.md', second
            assert counts() == (before[0] + 2, before[1])
            snapshot = observe_semantic_state(db)
            assert snapshot['chunks'] == 1 and len(snapshot['operations']) == 3, snapshot
            assert all(row[3:] == ['completed', 1] for row in snapshot['operations']), snapshot
            with lock:
                posted_after = dict(posts[before[0] + 1])
            assert snapshot['operations'][-1][:4] == [
                posted_after['key'], 'query', posted_after['remote_id'], 'completed'], snapshot
            assert host.process.poll() is None, host.log.read_text()
            assert all(sidecar.exists() for sidecar in sidecars), (
                'the second observer removed the live semantic WAL/SHM', snapshot)
            assert [(sidecar.stat().st_dev, sidecar.stat().st_ino) for sidecar in sidecars] == identities
        persisted = counts()
        assert cli(config_path, 'status')['pending'] is None
        cached = cli(config_path, 'search', query_after)
        assert cached['matches'] and cached['matches'][0]['path'] == 'MEMORY.md', cached
        assert counts() == persisted, 'a receipt lost after observer close was billed again after restart'
        print('PASS: independent SQLite observer close preserves live WAL/SHM; later semantic receipts are externally visible and reused after restart')

        directory, workspace, config_path, config = new_case(root, 'recover')
        (workspace / 'MEMORY.md').write_text(''.join(f'tea {index}' + 'x' * 1019 for index in range(9)))
        batch_start = counts()[0]
        arm(None, 'bad-result')
        cli(config_path, 'refresh', fail='')
        before = counts()
        status = cli(config_path, 'status')
        operation = status['pending']['id']
        assert status['pending']['remote_id'] and status['generation'] is None, status
        with lock:
            assert [len(item['body']['input']) for item in posts[batch_start:]] == [8, 1]
        for command in [('refresh',), ('search', 'tea held'), ('rebuild',)]:
            cli(config_path, *command, fail='')
        assert counts() == before, 'persistent hold must survive CLI process exit and stop every new POST'
        changed = copy.deepcopy(config)
        changed['provider']['api_key'] = rotated_secret
        changed_path = directory / 'changed-key.json'
        write_config(changed_path, changed)
        cli(changed_path, 'refresh', fail='')
        changed['memory']['semantic']['space_revision'] = 'fixture-revision-2'
        write_config(changed_path, changed)
        cli(changed_path, 'refresh', fail='')
        assert counts() == before, 'key/revision change must not bypass a same-database hold'
        recovered = cli(config_path, 'recover', operation)
        assert recovered['status'] == 'recovered' and recovered['operation_id'] == operation, recovered
        assert counts() == (before[0], before[1] + 1), 'recovery must GET only'
        assert cli(config_path, 'status')['pending'] is None
        assert cli(config_path, 'refresh')['status'] == 'ready'
        assert counts()[0] == before[0], 'recovered response should be reused without POST'
        print('PASS: malformed submitted result persists hold; restart/rebuild/key/revision changes cannot bypass it; GET recovery reuses the receipt')

        directory, workspace, config_path, config = new_case(root, 'disconnect')
        arm('disconnect')
        cli(config_path, 'refresh', fail='')
        before = counts()
        operation = cli(config_path, 'status')['pending']['id']
        cli(config_path, 'refresh', fail='')
        cli(config_path, 'rebuild', fail='')
        cli(config_path, 'review-clear', operation, '--note', 'fixture audit', fail='')
        cli(config_path, 'review-clear', operation, '--note', '', '--confirm-reconciled', fail='')
        assert counts() == before
        cli(config_path, 'review-clear', operation, '--note',
            'Verified the local fixture receipt and charge ledger; operator authorizes resuming.',
            '--confirm-reconciled')
        assert cli(config_path, 'status')['pending'] is None
        assert cli(config_path, 'refresh')['status'] == 'ready'
        assert counts()[0] == before[0] + 1, 'only explicit reviewed clearance may allow a new attempt'
        print('PASS: connection loss is held across restart; explicit audited CLI clearance is required before a new attempt')

        directory, workspace, config_path, config = new_case(root, 'query-disconnect')
        assert cli(config_path, 'refresh')['status'] == 'ready'
        arm('disconnect')
        cli(config_path, 'search', 'a query with uncertain submission', fail='')
        before = counts()
        status = cli(config_path, 'status')
        assert status['pending'] is not None and status['generation'] is not None, status
        for command in [('search', 'a different query cannot bypass hold'), ('refresh',), ('rebuild',)]:
            cli(config_path, *command, fail='')
        assert counts() == before
        print('PASS: query embeddings also persist unknown holds without returning cached source results or submitting another query')

        directory, workspace, config_path, config = new_case(root, 'killed-submission')
        submitted, released = threading.Event(), threading.Event()
        arm((submitted, released))
        process = subprocess.Popen([str(binary), 'memory', 'semantic', 'refresh', '--config', str(config_path)],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            assert submitted.wait(8), 'embedding POST was not observed before kill'
            process.kill()
            stdout, stderr = process.communicate(timeout=5)
            assert process.returncode != 0
            assert secret not in stdout + stderr
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            released.set()
        before = counts()
        status = cli(config_path, 'status')
        assert status['pending']['state'] == 'unknown' and status['pending']['remote_id'] is None, status
        for command in [('refresh',), ('search', 'must not resubmit'), ('rebuild',),
                        ('recover', status['pending']['id'])]:
            cli(config_path, *command, fail='')
        assert counts() == before, 'SIGKILL recovery must never infer not-sent or create a new POST'
        print('PASS: SIGKILL after observed embedding submission persists unknown state; restart, search, rebuild and recovery never replay POST')

        before = counts()
        for index, bad_source in enumerate([' MEMORY.md', 'MEMORY.md ', 'notes\n.md']):
            directory, workspace, config_path, config = new_case(root, 'invalid-default-' + str(index))
            config['memory']['path'] = bad_source
            assert config['memory']['semantic']['sources'] == []
            write_config(config_path, config)
            result = run('serve', '--config', config_path)
            assert result.returncode != 0, (bad_source, result.stdout, result.stderr)
            assert not (directory / 'state/semantic').exists(), 'invalid fallback source created semantic state'
        assert counts() == before
        print('PASS: inherited memory.path controls or boundary whitespace abort startup before index creation or embedding requests')

        before = counts()
        directory, workspace, config_path, config = new_case(root, 'inside-workspace')
        config['memory']['semantic']['index_path'] = 'semantic.sqlite3'
        write_config(config_path, config)
        cli(config_path, 'status', fail='')
        assert not (workspace / 'semantic.sqlite3').exists()
        directory, workspace, config_path, config = new_case(root, 'wrong-owner')
        config['memory']['semantic']['index_path'] = str(root / 'ready/state/semantic/index.sqlite3')
        write_config(config_path, config)
        cli(config_path, 'status', fail='')
        directory, workspace, config_path, config = new_case(root, 'foreign-db')
        (directory / 'state').mkdir(mode=0o700)
        state = directory / 'state/semantic'
        state.mkdir(mode=0o700)
        foreign = state / 'index.sqlite3'
        with sqlite3.connect(foreign) as connection:
            connection.execute('CREATE TABLE owner_record (value TEXT NOT NULL)')
            connection.execute("INSERT INTO owner_record VALUES ('must survive')")
        foreign.chmod(0o600)
        cli(config_path, 'status', fail='')
        with sqlite3.connect(foreign) as connection:
            assert connection.execute('SELECT value FROM owner_record').fetchone() == ('must survive',)
        directory, workspace, config_path, config = new_case(root, 'linked-db')
        (directory / 'state').mkdir(mode=0o700)
        state = directory / 'state/semantic'
        state.mkdir(mode=0o700)
        target = state / 'index.sqlite3'
        target.symlink_to(foreign)
        cli(config_path, 'status', fail='')
        target.unlink()
        os.link(foreign, target)
        cli(config_path, 'status', fail='')
        assert counts() == before
        print('PASS: private state rejects workspace placement, another workspace, foreign databases and symlink/hardlink aliases before network access')
        counts()
finally:
    gateway.shutdown()
    gateway.server_close()
