#!/usr/bin/env python3
"""Real serve startup with disposable WeCom token/app HTTP fixtures.

Synthetic credentials and localhost only. This proves application wiring and
fail-closed startup, not a live enterprise installation, member license or
client delivery. Successful first-send token reuse is exercised by wecom.py.
"""
import base64
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.parse
import urllib.request
import uuid


binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
corp, agent_id = 'wwfixturestartup', 1000002
members = ['alice.user', 'conversation.member', 'scheduled.only']
app_secret, access_token = 'fixture-app-' + uuid.uuid4().hex, 'fixture-token-' + uuid.uuid4().hex
callback_token = uuid.uuid4().hex
encoding_key = base64.b64encode(os.urandom(32)).decode().rstrip('=')
api_token, model_key = 'fixture-api-' + uuid.uuid4().hex, 'fixture-model-' + uuid.uuid4().hex
private_error = 'PRIVATE-WECOM-' + uuid.uuid4().hex
secrets = [app_secret, access_token, callback_token, encoding_key, api_token, model_key, private_error]
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def encoded(value):
    return json.dumps(value, separators=(',', ':')).encode()


def token_reply():
    return {'errcode': 0, 'errmsg': 'ok', 'access_token': access_token, 'expires_in': 7200}


def agent_reply():
    # Additional members, departments and tags are valid; explicit configured
    # members must still all be present, independently of those other scopes.
    return {'errcode': 0, 'errmsg': 'ok', 'agentid': agent_id, 'close': 0,
            'allow_userinfos': {'user': [{'userid': member.upper()} for member in members]
                                      + [{'userid': 'other.member'}]},
            'allow_partys': {'partyid': [1]}, 'allow_tags': {'tagid': [2]}}


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def listening(port):
    try:
        with socket.create_connection(('127.0.0.1', port), timeout=.15):
            return True
    except OSError:
        return False


def snapshot(directory):
    return {str(path.relative_to(directory)): (path.stat().st_size,
             hashlib.sha256(path.read_bytes()).hexdigest())
            for path in directory.rglob('*') if path.is_file()}


def inspect_stopped_database(database):
    # The caller has confirmed the actual production process exited. Open only
    # an existing DB; writable VFS access permits absent WAL bookkeeping, while
    # query_only prevents SQL mutations. Never use immutable on a live DB.
    before = hashlib.sha256(database.read_bytes()).hexdigest()
    with closing(sqlite3.connect(database.as_uri() + '?mode=rw', uri=True)) as connection:
        connection.execute('PRAGMA query_only=ON')
        assert connection.execute('PRAGMA integrity_check').fetchone() == ('ok',)
        assert connection.execute("SELECT count(*) FROM sqlite_master WHERE type='table'").fetchone()[0] > 1
    assert hashlib.sha256(database.read_bytes()).hexdigest() == before, 'SQL observer changed stopped database bytes'


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        case = self.server.case
        stage = None
        try:
            parsed = urllib.parse.urlsplit(self.path)
            assert not self.headers.get('Authorization'), 'unexpected authorization header'
            if parsed.path == '/cgi-bin/gettoken':
                stage = 'token'
                assert urllib.parse.parse_qs(parsed.query) == {
                    'corpid': [corp], 'corpsecret': [app_secret]}, 'wrong credential query'
            elif parsed.path == '/cgi-bin/agent/get':
                stage = 'agent'
                assert urllib.parse.parse_qs(parsed.query) == {
                    'access_token': [access_token], 'agentid': [str(agent_id)]}, 'wrong app query'
            else:
                raise AssertionError('unexpected GET endpoint')
            with case.lock:
                case.calls.append(stage)
                case.request_at[stage] = time.monotonic()
            if case.block != stage or not case.block_after_headers:
                case.received[stage].set()
            if case.block == stage and not case.block_after_headers:
                assert case.release.wait(timeout=15), 'fixture gate exceeded finite budget'
            body, status, mime = case.responses[stage]
            self.send_response(status)
            for value in mime:
                self.send_header('Content-Type', value)
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Connection', 'close')
            self.end_headers()
            if case.block == stage and case.block_after_headers:
                # Keep a genuine original response stream incomplete. The
                # production request deadline must include bounded body reads.
                first = max(1, len(body) // 2)
                self.wfile.write(body[:first])
                self.wfile.flush()
                case.received[stage].set()
                assert case.release.wait(timeout=15), 'fixture slow body exceeded finite budget'
                body = body[first:]
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass  # Expected after the real client times out or is terminated.
        except Exception as error:
            case.errors.append(type(error).__name__ + ': ' + str(error))
            self.send_error(500)
        finally:
            if case.block is not None and case.block == stage:
                case.finished.set()

    def do_POST(self):
        case = self.server.case
        path = urllib.parse.urlsplit(self.path).path
        case.posts.append('model' if path == '/v1/chat/completions' else 'platform')
        self.send_error(500)


class Case:
    def __init__(self, root, name, *, wecom=True, database=None, block=None, block_after_headers=False):
        self.root = root / name
        self.root.mkdir()
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        self.state = self.root / 'state'
        self.database = self.state / 'sessions.sqlite3'
        if database is not None:
            self.state.mkdir()
            self.database.write_bytes(database)
        self.block = block
        self.block_after_headers = block_after_headers
        self.release, self.finished = threading.Event(), threading.Event()
        self.received = {stage: threading.Event() for stage in ['token', 'agent']}
        self.calls, self.posts, self.errors = [], [], []
        self.request_at = {}
        self.lock = threading.Lock()
        self.responses = {'token': (encoded(token_reply()), 200, ['application/json']),
                          'agent': (encoded(agent_reply()), 200, ['Application/JSON; charset=utf-8'])}
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.server.case = self
        self.thread = threading.Thread(target=self.server.serve_forever,
                                       kwargs={'poll_interval': .02}, daemon=True)
        self.thread.start()
        self.port = free_port()
        base = 'http://127.0.0.1:' + str(self.server.server_port)
        self.settings = {
            'agent': {'name': 'wecom-startup-fixture', 'description': 'Local startup acceptance',
                      'system_instructions': 'Use the authorized clock.', 'max_turns': 3,
                      'workspace_path': str(self.workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': base,
                         'api_key': model_key, 'model': 'fixture'},
            'heartbeat': {'enabled': False, 'interval_secs': 1, 'path': 'HEARTBEAT.md'},
            'http': {'bind': '127.0.0.1:' + str(self.port), 'api_token': api_token,
                     'persist': True, 'persist_path': '../state/sessions.sqlite3',
                     'shutdown_timeout_secs': 1}}
        if wecom:
            self.settings['http'].update({
                'wecom_app_secret': app_secret, 'wecom_callback_token': callback_token,
                'wecom_encoding_aes_key': encoding_key,
                'channels': [{'channel': 'wecom', 'installation_id': corp + ':' + str(agent_id),
                              'allowed_senders': [members[0]], 'allowed_conversations': [members[1]],
                              'scheduled_destinations': [{'conversation_id': members[2], 'thread_id': None}],
                              'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                              'local_test_api_base': base + '/cgi-bin'}]})
        self.process = None
        self.log = self.root / 'serve.log'

    def begin(self, *, failure=True):
        # Every failing case enables a genuine heartbeat instruction so an
        # incorrectly ordered startup would be observable as a model request.
        self.settings['heartbeat']['enabled'] = failure
        if failure:
            (self.workspace / 'HEARTBEAT.md').write_text('Report startup acceptance status.\n')
        self.before_workspace = snapshot(self.workspace)
        self.before_state = snapshot(self.state)
        config = self.root / 'config.json'
        config.write_text(json.dumps(self.settings))
        self.started = time.monotonic()
        with self.log.open('wb') as output:
            self.process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
                                            env=env, stdout=output, stderr=output)

    def public_log(self):
        content = self.log.read_text()
        assert all(secret not in content for secret in secrets), 'private response/credential escaped'
        return content

    def ready(self):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert self.process.poll() is None, 'valid startup exited before listening'
            if 'HTTP 服务已启动于 ' in self.public_log():
                assert listening(self.port), 'startup log without actual listener'
                request = urllib.request.Request('http://127.0.0.1:' + str(self.port) + '/api/channels/status',
                                                 headers={'Authorization': 'Bearer ' + api_token})
                with opener.open(request, timeout=2) as response:
                    body = json.loads(response.read())
                    assert response.status == 200
                assert body['state'] == ('running' if self.settings['http'].get('channels') else 'disabled')
                return
            time.sleep(.02)
        raise AssertionError('valid startup did not listen within finite fixture budget')

    def failed(self, *, timeout=12):
        deadline = time.monotonic() + timeout
        while self.process.poll() is None and time.monotonic() < deadline:
            assert not listening(self.port), 'unverified app exposed an HTTP listener'
            time.sleep(.02)
        assert self.process.poll() is not None, 'failed startup exceeded finite process budget'
        self.observed_exit_at = time.monotonic()
        assert self.process.returncode != 0, 'invalid app startup succeeded'
        assert 'WeCom installation verification failed' in self.public_log(), 'missing generalized error'
        self.no_effects()

    def no_effects(self):
        assert not listening(self.port), 'listener survived failed/cancelled startup'
        assert snapshot(self.workspace) == self.before_workspace, 'workspace/MEMORY modified before verification'
        assert snapshot(self.state) == self.before_state, 'SQLite state modified before verification'
        assert not self.posts, 'unverified startup submitted a model/message request'
        assert not self.errors, self.errors
        self.public_log()

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            self.process.wait(timeout=5)
        assert not listening(self.port), 'process exit did not remove listener'

    def close(self):
        try:
            if self.process and self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=5)
        finally:
            self.release.set()
            if self.block and self.received[self.block].is_set():
                assert self.finished.wait(timeout=2), 'fixture HTTP worker did not drain'
            self.server.shutdown()
            self.server.server_close()
            self.thread.join(timeout=2)
            assert not self.thread.is_alive(), 'fixture server did not stop'


def malformed_cases():
    cases = []

    def add(name, *, stage='agent', value=None, raw=None, status=200, mime=None):
        cases.append((name, stage, (encoded(value) if raw is None else raw,
                                    status, ['application/json'] if mime is None else mime)))

    for field, value in [('agentid', agent_id + 1), ('close', 1), ('errcode', 40013)]:
        body = agent_reply()
        body[field] = value
        body['errmsg'] = private_error + app_secret + access_token
        add('wrong_' + field, value=body)
    for omitted in members:
        body = agent_reply()
        body['allow_userinfos']['user'] = [{'userid': member} for member in members if member != omitted]
        add('missing_' + omitted.replace('.', '_'), value=body)
    for name, extra in [('party_only', {'allow_partys': {'partyid': [1]}}),
                        ('tag_only', {'allow_tags': {'tagid': [2]}})]:
        body = {'errcode': 0, 'agentid': agent_id, 'close': 0, 'allow_userinfos': {'user': []}}
        body.update(extra)
        add(name, value=body)
    add('agent_top_array', value=[0, agent_id, 0, {'user': [{'userid': member} for member in members]}])
    body = agent_reply()
    body['allow_userinfos'] = [[{'userid': member} for member in members]]
    add('scope_array', value=body)
    body = agent_reply()
    body['allow_userinfos']['user'] = [[member] for member in members]
    add('member_array', value=body)
    body = agent_reply()
    body['allow_userinfos']['user'] += [{'userid': members[0].upper()}]
    add('casefold_duplicate_user', value=body)
    body = agent_reply()
    body['allow_userinfos']['user'].append({'userid': '@all'})
    add('broadcast_user', value=body)
    # Original bytes preserve duplicates that an ordinary json.loads/Value
    # fixture would erase. Every identity-bearing object layer is exercised.
    base = encoded(agent_reply())
    for name, needle, replacement in [
        ('duplicate_agentid', b'"agentid":1000002', b'"agentid":1000002,"agentid":1000002'),
        ('duplicate_close', b'"close":0', b'"close":0,"close":0'),
        ('duplicate_errcode', b'"errcode":0', b'"errcode":0,"errcode":0'),
        ('duplicate_scope', b'"allow_userinfos":', b'"allow_userinfos":{},"allow_userinfos":'),
        ('duplicate_users', b'"user":', b'"user":[],"user":'),
        ('duplicate_userid', b'"userid":"ALICE.USER"', b'"userid":"ALICE.USER","userid":"ALICE.USER"'),
    ]:
        assert base.count(needle) == 1
        add(name, raw=base.replace(needle, replacement, 1))
    add('agent_bad_json', raw=b'{"errcode":' + private_error.encode())
    add('agent_trailing_json', raw=base + b' {}')
    add('agent_http_500', value={'errmsg': private_error + access_token}, status=500)
    add('agent_http_201', value=agent_reply(), status=201)
    add('agent_wrong_mime', value=agent_reply(), mime=['text/plain'])
    add('agent_no_mime', value=agent_reply(), mime=[])
    add('agent_duplicate_mime', value=agent_reply(), mime=['application/json', 'application/json'])
    add('agent_joined_mime', value=agent_reply(), mime=['application/json, application/json'])
    body = agent_reply()
    body['description'] = private_error + 'x' * (64 * 1024)
    add('agent_oversize', value=body)
    add('token_top_array', stage='token', value=[0, access_token, 7200])
    raw = encoded(token_reply()).replace(b'"access_token":', b'"access_token":"' + access_token.encode()
                                        + b'","access_token":', 1)
    add('token_duplicate_value', stage='token', raw=raw)
    add('token_duplicate_mime', stage='token', value=token_reply(), mime=['application/json', 'text/plain'])
    add('token_http_failure', stage='token', value={'errcode': 40013, 'errmsg': private_error + app_secret}, status=403)
    return cases


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-wecom-startup-') as directory:
        root = Path(directory)
        successful = Case(root, 'valid_union')
        try:
            successful.begin(failure=False)
            successful.ready()
            assert successful.calls == ['token', 'agent'] and successful.posts == []
            successful.stop()
            assert successful.database.is_file(), 'valid startup did not open real SQLite'
            inspect_stopped_database(successful.database)
            saved_database = successful.database.read_bytes()
            successful.public_log()
            assert not successful.errors
        finally:
            successful.close()
        print('PASS 1: real token/app GET, active exact app and full member union including scheduled-only')

        no_wecom = Case(root, 'no_wecom', wecom=False)
        try:
            no_wecom.begin(failure=False)
            no_wecom.ready()
            assert no_wecom.calls == [] and no_wecom.posts == [] and not no_wecom.errors
            no_wecom.stop()
            no_wecom.public_log()
        finally:
            no_wecom.close()
        print('PASS 2: no WeCom configuration makes no token/app/platform/model request')

        invalids = malformed_cases()
        for name, stage, response in invalids:
            # Use a byte-for-byte copy of actual valid serve-created SQLite for
            # one app failure. Its tables/outbox remain unopened and unchanged.
            case = Case(root, name, database=saved_database if name == 'wrong_close' else None)
            try:
                case.responses[stage] = response
                case.begin()
                case.failed()
                assert case.calls == (['token'] if stage == 'token' else ['token', 'agent']), name
                if name == 'wrong_close':
                    assert case.database.read_bytes() == saved_database
            finally:
                case.close()
        print('PASS 3: ' + str(len(invalids)) + ' original JSON/MIME/HTTP/identity/visibility/size failures before effects')
        print('PASS 4: failed active-app proof preserves copied real SQLite bytes/hash and workspace')

        for stage in ['token', 'agent']:
            case = Case(root, 'cancel_' + stage, block=stage)
            try:
                case.begin()
                assert case.received[stage].wait(timeout=3), 'real blocked GET was not received'
                assert not case.release.is_set()
                case.no_effects()
                before = time.monotonic()
                case.stop()
                assert time.monotonic() - before < 5, 'SIGTERM did not exit within process budget'
                assert case.process.returncode != 0
                case.no_effects()
                assert case.calls == (['token'] if stage == 'token' else ['token', 'agent'])
            finally:
                case.close()
        print('PASS 5: actual blocked token/app GET SIGTERM exit without listener/SQLite/MEMORY/model/send')

        for stage in ['token', 'agent']:
            case = Case(root, 'deadline_' + stage, block=stage, block_after_headers=stage == 'agent')
            try:
                case.begin()
                assert case.received[stage].wait(timeout=3), 'real timed GET was not received'
                case.failed(timeout=8)
                elapsed = case.observed_exit_at - case.request_at[stage]
                assert 4 <= elapsed < 8, 'startup GET did not enforce actual five-second HTTP budget'
                assert case.observed_exit_at - case.started < 30, 'startup exceeded thirty-second overall budget'
                assert not case.release.is_set(), 'fixture released the timed HTTP request'
                assert case.calls == (['token'] if stage == 'token' else ['token', 'agent'])
                print('Deadline observation: ' + json.dumps({'stage': stage, 'request_seconds': round(elapsed, 3),
                      'startup_seconds': round(case.observed_exit_at - case.started, 3)}), flush=True)
            finally:
                case.close()
        print('PASS 6: token headers/app partial-body five-second deadlines, secret-free failure, no effects')
        print('Local fixtures support these contracts; live enterprise/member license/client delivery remains unqualified.')
except KeyboardInterrupt:
    raise SystemExit(130)
