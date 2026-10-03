#!/usr/bin/env python3
"""Real host + disposable SQLite + localhost Discord Bot/model protocol fixtures.

No real Discord account, external HTTP, paid model or third-party Python package.
Only job due timestamps are advanced for deterministic scheduling; all delivery
state transitions run through the production binary and authenticated APIs.
"""
from contextlib import closing
import base64
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
APP, GUILD = '123456789012345678', '223456789012345678'
CHANNEL, OTHER = '323456789012345678', '423456789012345678'
PUBLIC_KEY = 'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'


class Scenario:
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory(prefix='jiaclaw-discord-scheduled-')
        self.root = Path(self.temp.name)
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        self.database = self.root / 'state/sessions.sqlite3'
        self.config = self.root / 'config.json'
        self.api_token = 'fixture-host-' + uuid.uuid4().hex
        self.model_token = 'fixture-model-' + uuid.uuid4().hex
        self.bot_token = 'fixture.bot.' + uuid.uuid4().hex
        self.tokens = [self.api_token, self.model_token, self.bot_token]
        self.private_error = 'PRIVATE-DISCORD-' + uuid.uuid4().hex
        self.requests, self.models, self.errors, self.logs = [], [], [], []
        self.mode = 'success'
        self.text = 'Discord fixture: plain <text> @everyone <@123456789012345678> 🙂'
        self.gate = threading.Event()
        self.process = None
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def respond(self, status, body, headers=None):
                raw = json.dumps(body).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(raw)))
                for key, value in (headers or {}).items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(raw)

            def do_GET(self):
                try:
                    assert self.headers['Authorization'] == 'Bot ' + owner.bot_token
                    owner.requests.append({'method': 'GET', 'path': self.path, 'at': time.time()})
                    if self.path == '/applications/@me':
                        self.respond(200, {'id': OTHER if owner.mode == 'bad_app' else APP})
                    else:
                        match = re.fullmatch(r'/channels/([0-9]+)', self.path)
                        assert match and match[1] in [CHANNEL, OTHER]
                        self.respond(200, {'id': match[1],
                                          'guild_id': OTHER if owner.mode == 'bad_guild' else GUILD,
                                          'type': 11 if owner.mode == 'bad_type' else 0})
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as error:
                    owner.errors.append(type(error).__name__)
                    self.send_error(500)

            def do_POST(self):
                try:
                    body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    if self.path == '/v1/chat/completions':
                        assert self.headers['Authorization'] == 'Bearer ' + owner.model_token
                        assert self.headers.get('Idempotency-Key')
                        assert {tool['function']['name'] for tool in body['tools']} == {'datetime_now'}
                        owner.models.append(body)
                        if body['messages'][-1]['role'] == 'tool':
                            assert 'Unix' in body['messages'][-1]['content']
                            message = {'role': 'assistant', 'content': owner.text}
                            finish = 'stop'
                        else:
                            message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                                'id': 'clock-' + uuid.uuid4().hex, 'type': 'function',
                                'function': {'name': 'datetime_now', 'arguments': '{}'}}]}
                            finish = 'tool_calls'
                        self.respond(200, {'choices': [{'message': message, 'finish_reason': finish}]})
                        return
                    match = re.fullmatch(r'/channels/([0-9]+)/messages', self.path)
                    assert match and match[1] in [CHANNEL, OTHER]
                    assert self.headers['Authorization'] == 'Bot ' + owner.bot_token
                    assert self.headers['Content-Type'].startswith('application/json')
                    assert body['allowed_mentions'] == {'parse': [], 'users': [], 'roles': [], 'replied_user': False}
                    assert body['tts'] is False and body['flags'] == 4
                    assert body['enforce_nonce'] is True and re.fullmatch(r'[A-Za-z0-9_-]{22}', body['nonce'])
                    assert 0 < len(body['content'].encode('utf-16-le')) // 2 <= 2000
                    owner.requests.append({'method': 'POST', 'path': self.path, 'body': body, 'at': time.time()})
                    attempt = len(owner.posts())
                    receipt = {'id': str(523456789012345678 + attempt), 'channel_id': match[1], 'nonce': body['nonce']}
                    if owner.mode == 'rate' and attempt == 1:
                        self.respond(429, {'retry_after': 5, 'global': False},
                                     {'Retry-After': '6', 'X-RateLimit-Reset-After': '5.5'})
                    elif owner.mode == 'exhausted' and attempt == 1:
                        self.respond(200, receipt, {'X-RateLimit-Remaining': '0', 'X-RateLimit-Reset-After': '6'})
                    elif owner.mode == 'unauthorized':
                        self.respond(401, {'error': owner.private_error + owner.bot_token})
                    elif owner.mode == 'wrong_receipt':
                        self.respond(200, {**receipt, 'channel_id': GUILD})
                    elif owner.mode == 'kill':
                        owner.gate.wait(timeout=60)
                        self.respond(200, receipt)
                    else:
                        self.respond(200, receipt)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as error:
                    owner.errors.append(type(error).__name__)
                    self.send_error(500)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        fixture_base = 'http://127.0.0.1:' + str(self.server.server_port)
        self.settings = {
            'agent': {'name': 'discord-scheduled-fixture', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(self.workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': fixture_base,
                         'api_key': self.model_token, 'model': 'fixture'},
            'scheduler': {'enabled': True},
            'http': {'bind': '127.0.0.1:0', 'api_token': self.api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1,
                     'discord_public_key': PUBLIC_KEY, 'discord_bot_token': self.bot_token,
                     'channels': [{'channel': 'discord', 'installation_id': APP,
                                   'discord_guild_id': GUILD, 'allowed_senders': ['623456789012345678'],
                                   'allowed_conversations': [CHANNEL, OTHER],
                                   'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                                   'local_test_api_base': fixture_base,
                                   'scheduled_destinations': [{'conversation_id': channel} for channel in [CHANNEL, OTHER]]}]}}
        self.env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
        self.env['JIACLAW_LOG_LEVEL'] = 'info'
        self.env['JIACLAW_CHANNEL_STATE_KEY'] = uuid.uuid4().hex + uuid.uuid4().hex
        self.tokens.append(self.env['JIACLAW_CHANNEL_STATE_KEY'])

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.gate.set()
        self.stop()
        self.server.shutdown()
        self.server.server_close()
        try:
            self.assert_private()
        finally:
            self.temp.cleanup()

    def assert_private(self):
        text = '\n'.join(path.read_text() for path in self.logs)
        assert all(secret not in text for secret in self.tokens + [self.private_error]), 'secret in host logs'
        for path in self.database.parent.glob('sessions.sqlite3*'):
            if path.is_file():
                raw = path.read_bytes()
                assert all(secret.encode() not in raw for secret in self.tokens + [self.private_error]), 'secret in SQLite state'
        assert not self.errors, self.errors

    def sql(self, statement, parameters=(), write=False):
        with closing(sqlite3.connect(self.database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            db.row_factory = sqlite3.Row
            if not write:
                db.execute('PRAGMA query_only=ON')
            rows = [dict(row) for row in db.execute(statement, parameters).fetchall()]
            if write:
                db.commit()
            return rows

    def request(self, path, method='GET', body=None, authenticated=True):
        headers = {'Authorization': 'Bearer ' + self.api_token} if authenticated else {}
        if body is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(self.base + path, method=method, headers=headers,
                                     data=json.dumps(body).encode() if body is not None else None)
        try:
            response = urllib.request.urlopen(req, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read()
            assert all(secret.encode() not in raw for secret in self.tokens + [self.private_error]), 'secret in API response'
            return response.status, (json.loads(raw) if 'application/json' in response.headers.get('Content-Type', '') else raw.decode()) if raw else None

    def start(self):
        self.config.write_text(json.dumps(self.settings))
        log = self.root / ('server-' + uuid.uuid4().hex + '.log')
        self.logs.append(log)
        with log.open('wb') as output:
            self.process = subprocess.Popen([str(BINARY), 'serve', '--config', str(self.config)],
                                            env=self.env, stdout=output, stderr=output)
        def ready():
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
            if match:
                self.base = match[1]
                return self.request('/health')[0] == 200
        self.eventually(ready, 'startup')
        assert self.request('/api/channels/status')[1]['state'] == 'running'
        assert self.request('/api/jobs/status')[1]['state'] == 'running'

    def reject_config(self, settings):
        self.config.write_text(json.dumps(settings))
        result = subprocess.run([str(BINARY), 'serve', '--config', str(self.config)],
                                env=self.env, capture_output=True, timeout=10)
        assert result.returncode != 0, 'invalid configuration accepted'
        assert all(secret.encode() not in result.stdout + result.stderr for secret in self.tokens)
        assert not self.models and not self.requests

    def stop(self, kill=False):
        if self.process and self.process.poll() is None:
            self.process.kill() if kill else self.process.terminate()
            self.process.wait(timeout=15)

    def diagnostics(self):
        # Never report payloads, token values or raw upstream/host error text.
        rows = self.sql('SELECT state,attempts,ordinal FROM channel_outbox') if self.database.exists() else []
        return {'process_exit': self.process.poll() if self.process else None,
                'models': len(self.models), 'http': [(row['method'], row['path']) for row in self.requests],
                'deliveries': rows, 'fixture_errors': self.errors}

    def eventually(self, check, label, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert self.process is None or self.process.poll() is None, label + ': host exited'
            assert not self.errors, self.errors
            result = check()
            if result:
                return result
            time.sleep(.05)
        raise AssertionError(label + ': ' + json.dumps(self.diagnostics()))

    def posts(self):
        return [row for row in self.requests if row['method'] == 'POST']

    def spec(self, channel=CHANNEL):
        return {'name': 'fixture-' + uuid.uuid4().hex, 'prompt': 'Report the current time.',
                'schedule': {'kind': 'interval', 'seconds': 3600}, 'enabled_tools': ['datetime_now'],
                'timeout_secs': 30, 'delivery': {'channel': 'discord', 'installation_id': APP,
                                               'conversation_id': channel, 'thread_id': None}}

    def create(self, channel=CHANNEL, due=True):
        body = self.spec(channel)
        status, job = self.request('/api/jobs', 'POST', body)
        assert status in [200, 201] and job['spec'] == body, (status, job)
        if due:
            self.sql('UPDATE jobs SET next_due_ms=? WHERE id=?', (int(time.time() * 1000) + 250, job['id']), write=True)
        return job

    def run(self, job):
        return self.eventually(lambda: next((run for run in self.request('/api/jobs/' + job['id'] + '/runs')[1]
                                            if run['status'] == 'completed'), None), 'completed run')

    def rows(self, job, run):
        status, rows = self.request('/api/jobs/' + job['id'] + '/runs/' + run['id'] + '/deliveries')
        assert status == 200
        for row in rows:
            assert row['job_id'] == job['id'] and row['job_run_id'] == run['id'] and row['event_id'] is None
        return sorted(rows, key=lambda row: row['ordinal'])

    def state(self, job, run, state):
        rows = self.rows(job, run)
        return rows if rows and all(row['state'] == state for row in rows) else None

    def wait_state(self, job, run, state):
        return self.eventually(lambda: self.state(job, run, state), 'delivery ' + state)

    def disabled(self, job):
        return not self.request('/api/jobs/' + job['id'])[1]['enabled']

    def cooldown(self):
        rows = self.sql("SELECT until_ms FROM channel_cooldowns WHERE channel='discord' AND installation_id=?", (APP,))
        return rows[0]['until_ms'] if rows else 0

    def quiet(self, seconds=1.1):
        count = len(self.requests)
        time.sleep(seconds)
        assert len(self.requests) == count, 'blocked installation made HTTP requests'


def admission():
    with Scenario() as s:
        for mutation in ['missing_bot', 'missing_guild', 'guild_only', 'foreign_guild', 'thread', 'invalid_id', 'url']:
            settings = copy.deepcopy(s.settings)
            binding = settings['http']['channels'][0]
            if mutation == 'missing_bot':
                del settings['http']['discord_bot_token']
            elif mutation == 'missing_guild':
                del binding['discord_guild_id']
            elif mutation == 'guild_only':
                binding['scheduled_destinations'] = []
            elif mutation == 'foreign_guild':
                binding['channel'] = 'telegram'
            elif mutation == 'thread':
                binding['scheduled_destinations'][0]['thread_id'] = OTHER
            elif mutation == 'invalid_id':
                binding['scheduled_destinations'][0]['conversation_id'] = '00123'
            elif mutation == 'url':
                binding['scheduled_destinations'][0]['url'] = 'https://example.invalid'
            s.reject_config(settings)
        # Existing interaction-only installation starts without guild/Bot access.
        saved = copy.deepcopy(s.settings)
        del s.settings['http']['discord_bot_token']
        del s.settings['http']['channels'][0]['discord_guild_id']
        s.settings['http']['channels'][0]['scheduled_destinations'] = []
        s.start()
        assert s.request('/api/jobs', 'POST', s.spec())[0] == 400
        s.stop()
        s.settings = saved
        s.start()
        assert s.request('/api/jobs', 'POST', s.spec(), authenticated=False)[0] == 401
        for fields in [{'conversation_id': GUILD}, {'thread_id': OTHER}, {'installation_id': OTHER},
                       {'token': 'forbidden'}, {'url': 'https://example.invalid'}]:
            spec = s.spec()
            spec['delivery'].update(fields)
            assert 400 <= s.request('/api/jobs', 'POST', spec)[0] < 500
        assert s.request('/api/jobs', 'POST', {**s.spec(), 'enabled_tools': ['json_query']})[0] == 400
        assert not s.models and not s.requests


def verification_and_parts():
    for mode in ['bad_app', 'bad_guild', 'bad_type']:
        with Scenario() as s:
            s.mode = mode
            s.start()
            job = s.create()
            run = s.run(job)
            s.wait_state(job, run, 'permanent_failed')
            assert not s.posts() and s.disabled(job)
            assert len(s.models) == 2
    with Scenario() as s:
        s.text += (('a' * 1800 + ' Unicode🙂 @everyone ') * 7)
        s.start()
        job = s.create()
        run = s.run(job)
        rows = s.wait_state(job, run, 'delivered')
        assert 6 < len(rows) <= 16
        assert ''.join(row['text'] for row in rows) == s.text
        assert ''.join(row['body']['content'] for row in s.posts()) == s.text
        assert len(rows) == len(s.posts()) and len(s.models) == 2
        for row, post in zip(rows, s.posts()):
            nonce = base64.urlsafe_b64encode(uuid.UUID(row['id']).bytes).decode().rstrip('=')
            assert post['body']['nonce'] == nonce and row['receipt']
        assert [row['method'] for row in s.requests] == ['GET', 'GET', 'POST'] * len(rows)
        assert s.sql('PRAGMA user_version')[0]['user_version'] == 9


def persisted_cooldowns():
    for mode in ['rate', 'exhausted']:
        with Scenario() as s:
            s.mode = mode
            s.start()
            job = s.create()
            run = s.run(job)
            state = 'retry_wait' if mode == 'rate' else 'delivered'
            original = s.wait_state(job, run, state)[0]
            until = s.cooldown()
            assert until >= int(s.posts()[0]['at'] * 1000) + 5900
            if mode == 'exhausted':
                other = s.create(OTHER)
                other_run = s.run(other)
            s.stop()
            s.start()
            assert s.cooldown() >= until and time.time() * 1000 < until - 500
            s.quiet(.4)
            if mode == 'rate':
                result = s.wait_state(job, run, 'delivered')[0]
                assert result['id'] == original['id'] and result['attempts'] == 2
                assert s.posts()[0]['body'] == s.posts()[1]['body'] and len(s.models) == 2
            else:
                s.wait_state(other, other_run, 'delivered')
                assert s.posts()[1]['path'] == '/channels/' + OTHER + '/messages'
                assert len(s.models) == 4
            assert len(s.posts()) == 2 and s.posts()[1]['at'] * 1000 >= until - 50


def credential_block():
    with Scenario() as s:
        s.mode = 'unauthorized'
        s.start()
        first = s.create()
        run = s.run(first)
        s.wait_state(first, run, 'permanent_failed')
        assert s.disabled(first) and len(s.posts()) == 1
        auth = s.sql('SELECT credential_hash,blocked FROM discord_bot_auth')[0]
        assert auth == {'credential_hash': hashlib.sha256(s.bot_token.encode()).hexdigest(), 'blocked': 1}
        s.stop()
        s.start()
        second = s.create(OTHER)
        second_run = s.run(second)
        s.wait_state(second, second_run, 'permanent_failed')
        s.quiet()
        assert len(s.posts()) == 1
        # Existing destination FIFO retains failed plans across credential rotation.
        cancel_path = '/api/jobs/' + second['id'] + '/runs/' + second_run['id'] + '/deliveries/cancel'
        assert s.request(cancel_path, 'POST')[0] == 204
        assert s.state(second, second_run, 'cancelled')
        s.stop()
        s.bot_token = 'fixture.rotated.' + uuid.uuid4().hex
        s.tokens.append(s.bot_token)
        # Environment takes priority over the old configured credential.
        s.env['JIACLAW_DISCORD_BOT_TOKEN'] = s.bot_token
        s.mode = 'success'
        s.start()
        third = s.create(OTHER)
        s.wait_state(third, s.run(third), 'delivered')
        assert len(s.posts()) == 2
        assert s.state(first, run, 'permanent_failed') and s.state(second, second_run, 'cancelled')
        assert s.disabled(first) and s.disabled(second)
        assert s.sql('SELECT credential_hash,blocked FROM discord_bot_auth')[0] == {
            'credential_hash': hashlib.sha256(s.bot_token.encode()).hexdigest(), 'blocked': 0}


def unknown_blocks_installation():
    with Scenario() as s:
        s.mode = 'wrong_receipt'
        s.start()
        job = s.create()
        run = s.run(job)
        unknown = s.wait_state(job, run, 'unknown')[0]
        assert s.disabled(job)
        second = s.create(OTHER)
        second_run = s.run(second)
        s.wait_state(second, second_run, 'pending')
        s.stop()
        s.mode = 'success'
        s.start()
        s.quiet()
        assert len(s.posts()) == 1 and s.state(job, run, 'unknown')
        assert s.request('/api/jobs/' + job['id'] + '/resume', 'POST')[0] == 409
        assert s.request('/api/jobs/' + job['id'] + '/runs/' + run['id'] + '/deliveries', 'DELETE')[0] == 409
        path = '/api/channels/deliveries/' + unknown['id'] + '/resolve'
        assert s.request(path, 'POST', {'action': 'cancel'})[0] == 204
        s.wait_state(second, second_run, 'delivered')
        assert s.state(job, run, 'cancelled') and s.disabled(job)
        assert len(s.posts()) == 2 and s.posts()[1]['body']['nonce'] != s.posts()[0]['body']['nonce']


def killed_submission():
    with Scenario() as s:
        s.mode = 'kill'
        s.start()
        job = s.create()
        run = s.run(job)
        s.eventually(lambda: len(s.posts()) == 1, 'actual Bot POST before SIGKILL')
        original = s.wait_state(job, run, 'submitting')[0]
        old_nonce = s.posts()[0]['body']['nonce']
        s.stop(kill=True)
        s.gate.set()
        s.mode = 'success'
        s.start()
        recovered = s.wait_state(job, run, 'unknown')[0]
        assert recovered['id'] == original['id'] and s.disabled(job)
        s.quiet()
        assert len(s.posts()) == 1 and len(s.models) == 2
        other = s.create(OTHER)
        other_run = s.run(other)
        s.wait_state(other, other_run, 'pending')
        s.quiet(.4)
        path = '/api/channels/deliveries/' + original['id'] + '/resolve'
        assert s.request(path, 'POST', {'action': 'delivered', 'receipt': ''})[0] == 400
        assert s.request(path, 'POST', {'action': 'delivered', 'receipt': 'Fixture observed accepted message ID 523456789012345679'})[0] == 204
        s.wait_state(other, other_run, 'delivered')
        assert s.state(job, run, 'delivered') and s.disabled(job)
        assert len(s.posts()) == 2 and sum(post['body']['nonce'] == old_nonce for post in s.posts()) == 1
        assert len(s.models) == 4


for number, (label, test) in enumerate([
    ('strict configuration and independent destination authorization', admission),
    ('application/guild/type preflight, native tools and complete Unicode parts', verification_and_parts),
    ('429 and successful exhaustion persist installation cooldown across restart', persisted_cooldowns),
    ('401 survives restart; rotated credential permits future delivery only', credential_block),
    ('invalid receipt blocks all Bot targets until explicit cancellation', unknown_blocks_installation),
    ('SIGKILL after POST keeps identity, never replays, manual future-only recovery', killed_submission),
], 1):
    test()
    print(f'{number}/6 PASS {label}', flush=True)
