#!/usr/bin/env python3
"""Production-binary two-tenant WeCom acceptance; disposable localhost only.

Uses real private backends, native tool calls, SQLite and independent OpenSSL
AES/32-byte padding. No paid model, enterprise account or live message is used.
SQLite observations use a non-creating connection only after gateway exit.
Unknown effects are reviewed against original requests, never replayed.
"""
import base64
from contextlib import closing
import hashlib
import html
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request
import uuid

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
ENV = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
ENV.update(JIACLAW_LOG_LEVEL='off,jiaclaw::gateway::wecom=error', JIACLAW_LOG_FORMAT='json',
           HTTP_PROXY='http://127.0.0.1:1', HTTPS_PROXY='http://127.0.0.1:1',
           ALL_PROXY='http://127.0.0.1:1', NO_PROXY='')
CLIENT = urllib.request.build_opener(urllib.request.ProxyHandler({}))
LOCK = threading.Lock()
PEOPLE = ('alice', 'bob')
# Same enterprise, different dedicated applications must remain independent.
IDENTITIES = {who: {'corp': 'wwlocalfixturecorp', 'agent': 1000002 + index,
                    'human': who + '.member@example.com'} for index, who in enumerate(PEOPLE)}
APP_SECRETS = {who: 'local-app-' + uuid.uuid4().hex for who in PEOPLE}
CALLBACK_TOKENS = {'alice': 'QDG6eK', 'bob': 'X'}  # Official-compatible short tokens.
AES_KEYS = {who: os.urandom(32) for who in PEOPLE}
ENCODING_KEYS = {who: base64.b64encode(AES_KEYS[who]).decode().rstrip('=') for who in PEOPLE}
ACCESS_TOKENS = {who: 'local-token-' + uuid.uuid4().hex for who in PEOPLE}
MODEL_SECRETS = {who: 'local-model-' + uuid.uuid4().hex for who in PEOPLE}
PRIVATE_ERROR = 'PRIVATE-PLATFORM-' + uuid.uuid4().hex
ERRORS, AUTH_CALLS, APP_CALLS, MODEL_REQUESTS, SENDS, ISSUED_TOKENS = [], [], [], [], [], []
MODEL_KEYS = set()
MODES = {who: 'ok' for who in PEOPLE}
STARTUP_FAULT = {'who': None, 'case': None}
AUTH_GATE = (threading.Event(), threading.Event())
AUTH_GATED = {who: False for who in PEOPLE}
MODEL_GATES = {marker: (threading.Event(), threading.Event())
               for marker in ('GATE_DISABLE', 'GATE_CRASH', 'GATE_REVOKE')}
SEND_GATE = (threading.Event(), threading.Event())
PROCESSES = []


def check_errors():
    with LOCK:
        assert not ERRORS, ERRORS


def wait(predicate, label, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        check_errors()
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError('Timeout: ' + label)


def quiet(predicate, label, seconds=4.2):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        check_errors()
        assert predicate(), label
        time.sleep(.05)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def http(base, path, method='GET', body=None, headers=None, raw=None):
    supplied = dict(headers or {})
    if body is not None:
        supplied.setdefault('Content-Type', 'application/json')
        raw = json.dumps(body).encode()
    request = urllib.request.Request(base + path, method=method, data=raw, headers=supplied)
    try:
        response = CLIENT.open(request, timeout=10)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        data = response.read(2 * 1024 * 1024 + 1)
        assert len(data) <= 2 * 1024 * 1024, 'unbounded response'
        return response.status, json.loads(data) if data else None


def stop(process, kill=False):
    if process is None or process.poll() is not None:
        return
    process.kill() if kill else process.terminate()
    try:
        process.wait(timeout=12)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
        raise AssertionError('gateway/backend did not stop within lifecycle budget')


def count(records, who, marker=None):
    with LOCK:
        return sum(item['who'] == who and (marker is None or item.get('marker') == marker)
                   for item in records)


def model_reply(who, marker):
    text = who + ' CASE:' + marker + ' verified reply literal <@fake> & 中文😀'
    if marker in ('UNKNOWN', 'LONG'):
        text += '界😀<>&' * 500
    return text


def encrypted(who, message, receive_id=None):
    data = message.encode()
    data = os.urandom(16) + struct.pack('!I', len(data)) + data + (
        IDENTITIES[who]['corp'] if receive_id is None else receive_id).encode()
    padding = 32 - len(data) % 32
    data += bytes([padding]) * padding
    key = AES_KEYS[who]
    result = subprocess.run(['openssl', 'enc', '-aes-256-cbc', '-nopad',
                             '-K', key.hex(), '-iv', key[:16].hex()],
                            input=data, capture_output=True, timeout=5)
    assert result.returncode == 0, 'independent OpenSSL encryption failed'
    return base64.b64encode(result.stdout).decode()


def message_id(update):
    return str(100000000000000000 + update)


def signed_query(who, cipher, timestamp=None, nonce=None):
    timestamp = str(int(time.time())) if timestamp is None else str(timestamp)
    nonce = uuid.uuid4().hex if nonce is None else nonce
    signature = hashlib.sha1(''.join(sorted([CALLBACK_TOKENS[who], timestamp, nonce, cipher])).encode()).hexdigest()
    return {'msg_signature': signature, 'timestamp': timestamp, 'nonce': nonce}


def callback(who, update, marker, sender=None, inner_corp=None, inner_agent=None,
             outer_corp=None, outer_agent=None, receive_id=None, inner=None):
    identity = IDENTITIES[who]
    if inner is None:
        inner = ('<xml><ToUserName>' + html.escape(inner_corp or identity['corp']) + '</ToUserName>'
                 '<FromUserName>' + html.escape(sender or identity['human']) + '</FromUserName>'
                 '<CreateTime>' + str(int(time.time())) + '</CreateTime><MsgType>text</MsgType>'
                 '<Content>' + html.escape('CASE:' + marker) + '</Content><MsgId>' + message_id(update) + '</MsgId>'
                 '<AgentID>' + str(identity['agent'] if inner_agent is None else inner_agent) + '</AgentID></xml>')
    cipher = encrypted(who, inner, receive_id)
    outer = ('<xml><ToUserName>' + html.escape(outer_corp or identity['corp']) + '</ToUserName>'
             '<AgentID>' + str(identity['agent'] if outer_agent is None else outer_agent) + '</AgentID>'
             '<Encrypt><![CDATA[' + cipher + ']]></Encrypt></xml>').encode()
    return outer, signed_query(who, cipher)


class LocalHandler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self.dispatch('GET')

    def do_POST(self):
        self.dispatch('POST')

    def dispatch(self, method):
        try:
            url = urllib.parse.urlsplit(self.path)
            if url.path == '/v1/chat/completions':
                assert method == 'POST'
                length = int(self.headers['Content-Length'])
                assert length <= 2 * 1024 * 1024
                return self.model(json.loads(self.rfile.read(length)))
            query = urllib.parse.parse_qs(url.query)
            assert not self.headers.get_all('Authorization', [])
            if url.path == '/cgi-bin/gettoken':
                assert method == 'GET'
                who = next(who for who in PEOPLE if query.get('corpsecret') == [APP_SECRETS[who]])
                assert query == {'corpid': [IDENTITIES[who]['corp']], 'corpsecret': [APP_SECRETS[who]]}
                with LOCK:
                    AUTH_CALLS.append({'who': who, 'at': time.monotonic()})
                    gated = AUTH_GATED[who]
                if gated:
                    AUTH_GATE[0].set()
                    assert AUTH_GATE[1].wait(60), 'auth gate expired'
                return self.reply(200, {'errcode': 0, 'errmsg': 'ok', 'access_token': ACCESS_TOKENS[who], 'expires_in': 7200})
            who = next(who for who in PEOPLE if query.get('access_token') == [ACCESS_TOKENS[who]])
            identity = IDENTITIES[who]
            if url.path == '/cgi-bin/agent/get':
                assert method == 'GET' and query == {'access_token': [ACCESS_TOKENS[who]], 'agentid': [str(identity['agent'])]}
                with LOCK:
                    APP_CALLS.append({'who': who, 'at': time.monotonic()})
                    fault = STARTUP_FAULT['case'] if STARTUP_FAULT['who'] == who else None
                result = {'errcode': 0, 'errmsg': 'ok', 'agentid': identity['agent'], 'close': 0,
                          'allow_userinfos': {'user': [{'userid': identity['human'].upper()}]},
                          'allow_partys': {'partyid': [1]}, 'allow_tags': {'tagid': [2]}}
                if fault == 'agent': result['agentid'] += 1
                if fault == 'close': result['close'] = 1
                if fault == 'visibility': result['allow_userinfos']['user'] = []
                return self.reply(200, result)
            assert method == 'POST' and url.path == '/cgi-bin/message/send'
            assert query == {'access_token': [ACCESS_TOKENS[who]]}
            length = int(self.headers['Content-Length'])
            assert length <= 8192
            return self.platform(who, json.loads(self.rfile.read(length)))
        except (BrokenPipeError, ConnectionResetError):
            pass  # A terminated caller may lose the response after a local effect.
        except Exception as error:
            frame = traceback.extract_tb(error.__traceback__)[-1]
            with LOCK:
                ERRORS.append(type(error).__name__ + ':' + str(frame.lineno))
            try:
                self.reply(500, {'error': 'local fixture failure'})
            except (BrokenPipeError, ConnectionResetError):
                pass

    def model(self, body):
        who = next(who for who in PEOPLE if self.headers.get('Authorization') == 'Bearer ' + MODEL_SECRETS[who])
        assert body['model'] == 'fixture-channel'
        assert sorted(tool['function']['name'] for tool in body['tools']) == ['datetime_now', 'json_query']
        assert body['parallel_tool_calls'] is False
        prompt = next(item['content'] for item in reversed(body['messages']) if item['role'] == 'user')
        marker = re.search(r'CASE:([A-Z_]+)', prompt).group(1)
        last = body['messages'][-1]
        key = self.headers.get('Idempotency-Key')
        with LOCK:
            assert key and key not in MODEL_KEYS, 'model identity replayed'
            MODEL_KEYS.add(key)
            MODEL_REQUESTS.append({'who': who, 'marker': marker, 'last_role': last['role']})
        if last['role'] != 'tool':
            if marker in MODEL_GATES:
                MODEL_GATES[marker][0].set()
                assert MODEL_GATES[marker][1].wait(60), 'model gate expired'
            call = {'id': 'call_' + uuid.uuid4().hex, 'type': 'function',
                    'function': {'name': 'datetime_now', 'arguments': '{}'}}
            message, reason = {'role': 'assistant', 'content': None, 'tool_calls': [call]}, 'tool_calls'
        else:
            previous = body['messages'][-2]
            assert previous['role'] == 'assistant' and previous['tool_calls'][0]['id'] == last['tool_call_id']
            assert 'error' not in json.loads(last['content']), 'native tool failed'
            message, reason = {'role': 'assistant', 'content': model_reply(who, marker)}, 'stop'
        self.reply(200, {'choices': [{'message': message, 'finish_reason': reason}]})

    def platform(self, who, body):
        identity = IDENTITIES[who]
        assert set(body) == {'touser', 'agentid', 'msgtype', 'text', 'safe', 'enable_duplicate_check', 'enable_id_trans'}
        assert body['touser'] == identity['human'] and type(body['agentid']) is int and body['agentid'] == identity['agent']
        assert body['msgtype'] == 'text' and body['safe'] == body['enable_duplicate_check'] == body['enable_id_trans'] == 0
        text = body['text']['content']
        assert text and len(text.encode()) <= 2048 and '<' not in text and '>' not in text
        with LOCK:
            mode = MODES[who]
            receipt = 'local-' + uuid.uuid4().hex
            SENDS.append({'who': who, 'at': time.monotonic(), 'at_ms': int(time.time() * 1000),
                          'text': text, 'receipt': receipt, 'mode': mode})
        if mode in ('unknown', 'rate'):
            return self.reply(503 if mode == 'unknown' else 429, {'errcode': -1, 'errmsg': PRIVATE_ERROR})
        if mode == 'send_crash':
            SEND_GATE[0].set()
            assert SEND_GATE[1].wait(60), 'send gate expired'
        if mode == 'invalid_token':
            return self.reply(400, {'errcode': 40014, 'errmsg': PRIVATE_ERROR})
        result = {'errcode': 0, 'errmsg': 'ok', 'msgid': receipt, 'invaliduser': '',
                  'invalidparty': '', 'invalidtag': '', 'unlicenseduser': ''}
        if mode == 'partial': result['invaliduser'] = identity['human']
        self.reply(200, result)


def main():
    binary_digest = hashlib.sha256(BINARY.read_bytes()).hexdigest()
    fixture = ThreadingHTTPServer(('127.0.0.1', 0), LocalHandler)
    fixture.daemon_threads = True
    server_thread = threading.Thread(target=fixture.serve_forever, daemon=True)
    server_thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-wecom-') as temp:
            root = Path(temp).resolve()
            base = 'http://127.0.0.1:' + str(fixture.server_port)
            settings = {'bind': '127.0.0.1:' + str(port()),
                        'registry_path': str(root / 'registry/users.sqlite3'),
                        'request_timeout_seconds': 150, 'max_in_flight': 4, 'backends': []}
            cfg = root / 'gateway.json'
            gateway_url = 'http://' + settings['bind']
            backend, users, bindings = {}, {}, {}
            gateway = None

            def secret(name, value):
                path = root / name
                path.write_text(value)
                path.chmod(0o600)
                return str(path)

            def save():
                cfg.write_text(json.dumps(settings))

            def cli(*arguments, ok=True):
                result = subprocess.run([str(BINARY), 'gateway', *arguments, '--config', str(cfg)],
                                        env=ENV, text=True, capture_output=True, timeout=15)
                private = [*APP_SECRETS.values(), *ACCESS_TOKENS.values(), *ENCODING_KEYS.values(),
                           *MODEL_SECRETS.values(), PRIVATE_ERROR,
                           *(item['token'] for item in backend.values())]
                assert all(value not in result.stderr for value in private), 'secret in CLI diagnostics'
                if not ok:
                    assert result.returncode != 0, (arguments[0], 'unexpected CLI success')
                    assert all(value not in result.stdout for value in private), 'secret in failed CLI output'
                    return result
                assert result.returncode == 0, (arguments[0], 'CLI failed', result.returncode)
                parsed = json.loads(result.stdout)
                if isinstance(parsed, dict) and 'token' in parsed:
                    ISSUED_TOKENS.append(parsed['token'])
                return parsed

            def launch(arguments, url, name):
                with (root / (name + '.log')).open('ab') as output:
                    process = subprocess.Popen([str(BINARY), *arguments], env=ENV, stdout=output, stderr=output)
                PROCESSES.append(process)
                def healthy():
                    assert process.poll() is None, name + ' startup failed; inspect private local log'
                    try:
                        return http(url, '/health')[0] == 200
                    except (OSError, urllib.error.URLError):
                        return False
                wait(healthy, name + ' startup', 40)
                return process

            def resume():
                nonlocal gateway
                assert gateway is None or gateway.poll() is not None
                gateway = launch(['gateway', 'serve', '--config', str(cfg)], gateway_url, 'gateway')

            def pause(kill=False):
                nonlocal gateway
                previous = gateway
                stop(previous, kill=kill)
                assert previous is None or previous.poll() is not None
                gateway = None
                return previous

            def rows(path, query, parameters=()):
                assert gateway is None, 'SQLite observation requires stopped gateway'
                # Non-creating WAL-aware VFS, closed immediately; never immutable.
                with closing(sqlite3.connect(path.as_uri() + '?mode=rw', uri=True, timeout=2)) as connection:
                    connection.execute('PRAGMA query_only=ON')
                    connection.row_factory = sqlite3.Row
                    return [dict(row) for row in connection.execute(query, parameters)]

            def channel_db(who):
                return root / 'registry/wecom' / (bindings[who]['id'] + '.sqlite3')

            def inspect(who, kind, *arguments):
                assert gateway is None, 'private maintenance requires stopped gateway'
                return cli('wecom-inspect', '--binding', bindings[who]['id'], '--kind', kind, *arguments)['result'][kind]

            def all_items(who, kind):
                result = []
                for offset in range(0, 16001 if kind != 'reservations' else 10001, 100):
                    page = inspect(who, kind, '--limit', '100', '--offset', str(offset))
                    assert len(page) <= 100
                    result.extend(page)
                    if len(page) < 100:
                        return result
                raise AssertionError('unbounded private inspection')

            def event(who, update):
                return next((item for item in all_items(who, 'events') if item['spec']['event_id'] == message_id(update)), None)

            def deliveries(who, update):
                item = event(who, update)
                return [] if item is None else inspect(who, 'deliveries', '--event', item['id'], '--limit', '100')

            def hold(who):
                found = rows(Path(settings['registry_path']), 'SELECT * FROM write_holds WHERE user_id=?',
                             (users[who]['user_id'],))
                return found[0] if found else None

            def clear(who, ok=True):
                return cli('review-clear', '--user', users[who]['user_id'], '--confirm-backend-idle',
                           '--note', 'Local original request and backend completion checked; no replay', ok=ok)

            def bearer(who):
                return {'Authorization': 'Bearer ' + users[who]['token']}

            def private_request(who, request_id, update, state):
                code, value = http(backend[who]['url'], '/internal/channels/wecom/requests/' + request_id,
                                   headers={'Authorization': 'Bearer ' + backend[who]['token']})
                assert code == 200 and value == {
                    'protocol': 5, 'backend_id': who, 'binding_id': bindings[who]['id'],
                    'request_id': request_id, 'event_id': message_id(update),
                    'session_id': 'wecom:' + bindings[who]['id'], 'status': state}
                return value

            def hook(who, update=None, marker=None, expected=200, prepared=None, extra='', return_status=False):
                raw, query = callback(who, update, marker) if prepared is None else prepared
                url = gateway_url + '/hooks/wecom/' + bindings[who]['id'] + '?' + urllib.parse.urlencode(query) + extra
                request = urllib.request.Request(url, method='POST', data=raw, headers={'Content-Type': 'application/xml'})
                started = time.monotonic()
                try:
                    response = CLIENT.open(request, timeout=4)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    data = response.read(64 * 1024 + 1)
                    assert len(data) <= 64 * 1024
                    parsed = json.loads(data) if data and response.status != 200 else None
                    code = parsed.get('status') if isinstance(parsed, dict) else None
                    diagnostic = {'http_status': response.status, 'code': code if code in {
                        None, 'busy', 'queue_full', 'admission_failed', 'ingress_deadline', 'invalid_callback',
                        'body_timeout', 'unknown_binding', 'binding_disabled', 'disabled', 'fingerprint_conflict',
                        'event_not_authorized', 'invalid_query', 'invalid_body'} else 'unrecognized',
                        'elapsed_ms': round((time.monotonic() - started) * 1000)}
                    allowed = (expected,) if isinstance(expected, int) else expected
                    assert response.status in allowed, diagnostic
                    assert time.monotonic() - started < 1, diagnostic
                    if response.status == 200:
                        assert data == b'', 'ACK must be empty, never JSON'
                    return (response.status, code) if return_status else None

            def challenge(who, expected=200):
                plaintext = 'bounded-local-challenge-' + who + '+/&='
                cipher = encrypted(who, plaintext)
                query = signed_query(who, cipher) | {'echostr': cipher}
                url = gateway_url + '/hooks/wecom/' + bindings[who]['id'] + '?' + urllib.parse.urlencode(query)
                started = time.monotonic()
                try:
                    response = CLIENT.open(url, timeout=4)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    raw = response.read(64 * 1024 + 1)
                    assert response.status == expected and time.monotonic() - started < 1
                    if expected == 200:
                        assert raw == plaintext.encode(), 'challenge was quoted, changed or double decoded'

            def expect_start_failure():
                result = subprocess.run([str(BINARY), 'gateway', 'serve', '--config', str(cfg)],
                                        env=ENV, capture_output=True, text=True, timeout=40)
                assert result.returncode != 0, 'invalid startup succeeded'
                secrets = [*APP_SECRETS.values(), *ACCESS_TOKENS.values(), *ENCODING_KEYS.values(),
                           *MODEL_SECRETS.values(), *ISSUED_TOKENS, PRIVATE_ERROR,
                           *(item['token'] for item in backend.values())]
                assert all(secret not in result.stdout + result.stderr for secret in secrets), 'secret in startup failure'

            def complete(who, update, marker, expected_parts=1):
                wait(lambda: count(MODEL_REQUESTS, who, marker) == 2, 'native tool completion ' + marker)
                wait(lambda: sum(item['who'] == who and 'CASE:' + marker + ' ' in item['text'] for item in SENDS) == expected_parts,
                     'platform accepted ' + marker, max(25, expected_parts * 5))
                # Graceful drain proves the local sender/settlement has ended;
                # it does not prove a previously ambiguous external result.
                pause()
                found = deliveries(who, update)
                assert len(found) == expected_parts and all(item['state'] == 'delivered' for item in found)
                assert hold(who) is None, 'known local completion retained write hold'
                return found

            for who in PEOPLE:
                directory = root / who
                workspace = directory / 'workspace'
                workspace.mkdir(parents=True)
                token = 'private-backend-' + uuid.uuid4().hex
                url = 'http://127.0.0.1:' + str(port())
                config = {'agent': {'name': who, 'description': 'Tenant WeCom fixture', 'workspace_path': str(workspace),
                                    'system_instructions': 'Use fixed read-only tools.', 'max_turns': 10},
                          'provider': {'provider_type': 'brokerrouter', 'base_url': base,
                                       'api_key': MODEL_SECRETS[who], 'model': 'wrong-default-route'},
                          'routing': {'channel': {'model': 'fixture-channel'}},
                          'http': {'bind': url.removeprefix('http://'), 'api_token': token, 'persist': True,
                                   'persist_path': '../state/sessions.sqlite3', 'gateway_channel_chat': who == 'alice',
                                   'shutdown_timeout_secs': 2}, 'heartbeat': {'enabled': False}}
                path = directory / 'config.json'
                path.write_text(json.dumps(config))
                backend[who] = {'config': config, 'path': path, 'url': url, 'token': token,
                                'db': directory / 'state/sessions.sqlite3'}
                settings['backends'].append({'id': who, 'url': url, 'token_file': secret(who + '-backend.secret', token)})
                backend[who]['process'] = launch(['serve', '--config', str(path)], url, who)
            save()
            for who in PEOPLE:
                users[who] = cli('user-add', '--backend', who)
                identity = IDENTITIES[who]
                if who == 'bob':
                    cli('wecom-bind', '--user', users[who]['user_id'], '--corp-id', identity['corp'],
                        '--agent-id', str(IDENTITIES['alice']['agent']), '--human-user-id', identity['human'], ok=False)
                bindings[who] = cli('wecom-bind', '--user', users[who]['user_id'], '--corp-id', identity['corp'],
                                    '--agent-id', str(identity['agent']), '--human-user-id', identity['human'])
                assert bindings[who]['backend_id'] == who and type(bindings[who]['agent_id']) is int
            resume()
            hook('alice', 1, 'DEFAULT_OFF', expected=404)
            for path in ('/internal/channels/wecom/status', '/internal/channels/wecom-binding', '/internal/channels/wecom/execute',
                         '/api/channels/events', '/hooks/inbound'):
                assert http(gateway_url, path, headers=bearer('alice'))[0] == 404
            assert http(backend['bob']['url'], '/internal/channels/wecom/status',
                        headers={'Authorization': 'Bearer ' + backend['bob']['token']})[0] == 404
            pause()
            for who in PEOPLE:
                assert inspect(who, 'events') == inspect(who, 'operations') == inspect(who, 'reservations') == []
            assert not (root / 'registry/wecom').exists(), 'offline inspection created an unowned queue'
            settings['wecom'] = [{'binding_id': bindings[who]['id'],
                                  'app_secret_file': secret(who + '-app.secret', APP_SECRETS[who]),
                                  'callback_token_file': secret(who + '-callback.secret', CALLBACK_TOKENS[who]),
                                  'encoding_aes_key_file': secret(who + '-aes.secret', ENCODING_KEYS[who]),
                                  'api_base': base + '/cgi-bin', 'allow_loopback': True} for who in PEOPLE]
            save()
            expect_start_failure()  # Real Bob backend has not enabled protocol5.
            stop(backend['bob']['process'])
            backend['bob']['config']['http']['gateway_channel_chat'] = True
            backend['bob']['path'].write_text(json.dumps(backend['bob']['config']))
            backend['bob']['process'] = launch(['serve', '--config', str(backend['bob']['path'])], backend['bob']['url'], 'bob')
            for fault in ('agent', 'close', 'visibility'):
                with LOCK:
                    STARTUP_FAULT.update(who='alice', case=fault)
                before = len(MODEL_REQUESTS), len(SENDS)
                expect_start_failure()
                assert (len(MODEL_REQUESTS), len(SENDS)) == before
                with LOCK:
                    STARTUP_FAULT.update(who=None, case=None)
            private_file = Path(settings['wecom'][0]['app_secret_file'])
            private_file.chmod(0o640)
            expect_start_failure()
            private_file.chmod(0o600)
            actual = private_file.with_suffix('.actual')
            private_file.rename(actual)
            private_file.symlink_to(actual)
            expect_start_failure()
            private_file.unlink()
            actual.rename(private_file)
            alias = private_file.with_suffix('.alias')
            os.link(private_file, alias)
            expect_start_failure()
            alias.unlink()
            for cancel in (True, False):
                AUTH_GATE[0].clear(); AUTH_GATE[1].clear()
                with LOCK:
                    AUTH_GATED['alice'] = True
                before_effects = len(MODEL_REQUESTS), len(SENDS)
                with (root / 'gateway-auth-blocked.log').open('ab') as output:
                    pending = subprocess.Popen([str(BINARY), 'gateway', 'serve', '--config', str(cfg)],
                                               env=ENV, stdout=output, stderr=output)
                PROCESSES.append(pending)
                try:
                    wait(AUTH_GATE[0].is_set, 'actual blocked tenant token query', 5)
                    observed = time.monotonic()
                    if cancel:
                        stop(pending)
                    else:
                        pending.wait(timeout=12)
                        assert time.monotonic() - observed < 8, 'tenant startup token request was unbounded'
                    assert pending.returncode != 0 and not AUTH_GATE[1].is_set()
                    assert (len(MODEL_REQUESTS), len(SENDS)) == before_effects
                    with socket.socket() as probe:
                        probe.settimeout(.2)
                        host, port_text = settings['bind'].rsplit(':', 1)
                        assert probe.connect_ex((host, int(port_text))) != 0
                finally:
                    stop(pending)
                    with LOCK:
                        AUTH_GATED['alice'] = False
                    AUTH_GATE[1].set()
            resume()
            for who in PEOPLE:
                challenge(who)
                raw, query = callback(who, 2, 'REJECTED')
                hook(who, expected=401, prepared=(raw, query | {'msg_signature': '0' * 40}))
                hook(who, expected=401, prepared=(raw, query), extra='&nonce=duplicate')
                for timestamp in (int(time.time()) - 360, int(time.time()) + 360):
                    cipher = re.search(b'<!\[CDATA\[(.*?)\]\]>', raw).group(1).decode()
                    hook(who, expected=401, prepared=(raw, signed_query(who, cipher, timestamp)))
                hook(who, expected=403, prepared=callback(who, 2, 'REJECTED', sender='foreign.member'))
                for kwargs in ({'inner_agent': IDENTITIES[who]['agent'] + 10}, {'outer_agent': IDENTITIES[who]['agent'] + 10},
                               {'inner_corp': 'wwforeign'}, {'receive_id': 'wwforeign'}):
                    hook(who, expected=401, prepared=callback(who, 2, 'REJECTED', **kwargs))
                for size in (16385, 32768):
                    content = 'CASE:OVERSIZED ' + 'x' * (size - len('CASE:OVERSIZED '))
                    inner = ('<xml><ToUserName>' + IDENTITIES[who]['corp'] + '</ToUserName><FromUserName>'
                             + IDENTITIES[who]['human'] + '</FromUserName><CreateTime>' + str(int(time.time()))
                             + '</CreateTime><MsgType>text</MsgType><Content>' + content + '</Content><MsgId>'
                             + message_id(2) + '</MsgId><AgentID>' + str(IDENTITIES[who]['agent']) + '</AgentID></xml>')
                    hook(who, expected=413, prepared=callback(who, 2, 'OVERSIZED', inner=inner))
                hook(who, expected=401, prepared=(b'<!DOCTYPE xml [<!ENTITY x SYSTEM "file:///etc/passwd">]><xml>&x;</xml>', query))
                # Occupy the real body-reader slot with an unfinished signed
                # request; a separate installation remains independently live.
                partial_raw, partial_query = callback(who, 2, 'PARTIAL_BODY')
                connection = HTTPConnection(settings['bind'], timeout=4)
                target = '/hooks/wecom/' + bindings[who]['id'] + '?' + urllib.parse.urlencode(partial_query)
                connection.putrequest('POST', target)
                connection.putheader('Content-Type', 'application/xml')
                connection.putheader('Content-Length', str(len(partial_raw)))
                started = time.monotonic()
                connection.endheaders(partial_raw[:1])
                try:
                    time.sleep(.05)
                    status, code = hook(who, expected=429, prepared=(partial_raw, partial_query), return_status=True)
                    assert code == 'busy', 'unfinished body did not retain its installation slot'
                    challenge('bob' if who == 'alice' else 'alice')
                    response = connection.getresponse()
                    assert response.status == 408 and time.monotonic() - started < 1, 'body-reader deadline was unbounded'
                    assert json.loads(response.read(64 * 1024 + 1)) == {'status': 'body_timeout'}
                finally:
                    connection.close()
                challenge(who)  # The actual completed timeout released its slot.
                owner = {'protocol': 5, 'binding_id': bindings[who]['id'], 'user_id': users[who]['user_id'],
                         'backend_id': who, 'corp_id': IDENTITIES[who]['corp'],
                         'agent_id': str(IDENTITIES[who]['agent']), 'human_user_id': IDENTITIES[who]['human']}
                owner_raw = json.dumps(owner, separators=(',', ':')).encode()
                # Valid, already-owned identity makes MIME the only changing
                # condition; malformed DTOs would hide a first-header bug.
                for extra_mime in (None, 'text/plain', 'application/json'):
                    direct = HTTPConnection(backend[who]['url'].removeprefix('http://'), timeout=4)
                    try:
                        direct.putrequest('POST', '/internal/channels/wecom-binding')
                        direct.putheader('Authorization', 'Bearer ' + backend[who]['token'])
                        direct.putheader('Content-Type', 'application/json')
                        if extra_mime is not None:
                            direct.putheader('Content-Type', extra_mime)
                        direct.putheader('Content-Length', str(len(owner_raw)))
                        direct.endheaders(owner_raw)
                        response = direct.getresponse()
                        assert response.status == (200 if extra_mime is None else 400), 'private MIME uniqueness was not enforced'
                        actual = response.read(64 * 1024 + 1)
                        assert len(actual) <= 64 * 1024
                        if extra_mime is None:
                            assert json.loads(actual) == owner, 'same private owner control changed'
                    finally:
                        direct.close()
                execute = {'protocol': 5, 'binding_id': bindings[who]['id'],
                           'request_id': '019292de-2345-7123-8123-123456789abc',
                           'session_id': 'wecom:' + bindings[who]['id'], 'event_id': message_id(3),
                           'prompt': 'CASE:REJECTED typed private DTO'}
                for endpoint, payload in (('/internal/channels/wecom-binding', owner),
                                          ('/internal/channels/wecom/execute', execute)):
                    encoded = json.dumps(payload, separators=(',', ':')).encode()
                    rejected = (json.dumps(list(payload.values())).encode(),
                                encoded[:-1] + b',"protocol":5}',
                                encoded[:-1] + b',"unknown":true}')
                    for body in rejected:
                        direct = HTTPConnection(backend[who]['url'].removeprefix('http://'), timeout=4)
                        try:
                            direct.request('POST', endpoint, body=body,
                                           headers={'Authorization': 'Bearer ' + backend[who]['token'],
                                                    'Content-Type': 'application/json'})
                            response = direct.getresponse()
                            assert response.status == 400, 'malformed original private DTO was admitted'
                            assert len(response.read(64 * 1024 + 1)) <= 64 * 1024
                        finally:
                            direct.close()
            pause()
            assert all(all_items(who, 'events') == [] and hold(who) is None for who in PEOPLE)
            assert len(MODEL_REQUESTS) == len(SENDS) == 0
            print('PASS 1: default-off/protocol5, dedicated app proofs, secure short-token files, original XML/query/identity and pre-ACK bounds', flush=True)

            resume()
            for who in PEOPLE:
                hook(who, 100, 'BASIC_' + who.upper())
                hook(who, expected=200, prepared=callback(who, 100, 'BASIC_' + who.upper(), sender=IDENTITIES[who]['human'].upper()))
            wait(lambda: count(SENDS, 'alice') == count(SENDS, 'bob') == 1, 'two private replies')
            pause()
            for who in PEOPLE:
                done = deliveries(who, 100)
                assert len(done) == 1 and done[0]['state'] == 'delivered' and count(MODEL_REQUESTS, who) == 2
                assert len(all_items(who, 'events')) == 1 and hold(who) is None
                sent = next(item for item in SENDS if item['who'] == who)
                assert done[0]['receipt'] == sent['receipt'] and 'literal ＜@fake＞ & 中文😀' in sent['text']
                owner = rows(channel_db(who), 'SELECT * FROM gateway_wecom_owner')[0]
                assert owner['protocol'] == 5 and owner['binding_id'] == bindings[who]['id'] and owner['backend_id'] == who
                assert owner['user_id'] == users[who]['user_id'] and str(owner['agent_id']) == str(IDENTITIES[who]['agent'])
                assert not rows(channel_db(who), 'SELECT * FROM sessions'), 'private queue contains backend session history'
                operations = all_items(who, 'operations')
                original = next(item for item in operations if item['kind'] == 'event')
                assert uuid.UUID(original['request_id']).version == 7
                private_request(who, original['request_id'], 100, 'completed')
            # Only complete, stopped gateway-owned files are exchanged; no SQL
            # owner or quota is fabricated to trigger this rejection.
            alice_db, bob_db = channel_db('alice'), channel_db('bob')
            for path in (alice_db, bob_db):
                assert not Path(str(path) + '-wal').exists()
            temporary = alice_db.with_suffix('.swap')
            alice_db.rename(temporary); bob_db.rename(alice_db); temporary.rename(bob_db)
            try:
                expect_start_failure()
                cli('wecom-inspect', '--binding', bindings['alice']['id'], '--kind', 'events', ok=False)
            finally:
                alice_db.rename(temporary); bob_db.rename(alice_db); temporary.rename(bob_db)
            read_key = cli('key-add', '--user', users['alice']['user_id'], '--read-only')
            cli('key-revoke', '--key', users['alice']['key_id'])
            users['alice'].update(read_key)
            resume()
            assert http(gateway_url, '/api/sessions', 'POST', {}, bearer('alice'))[0] == 403
            hook('alice', 101, 'READ_KEY')
            complete('alice', 101, 'READ_KEY')
            users['alice'].update(cli('key-add', '--user', users['alice']['user_id']))
            resume()
            hook('alice', 100, 'FINGERPRINT_CHANGED', expected=409)
            pause()
            assert count(MODEL_REQUESTS, 'alice', 'BASIC_ALICE') == 2
            print('PASS 2: two real native backends, reencrypted/casefold MsgId dedup, immutable private owners, receipts and independent read-only Key', flush=True)

            resume()
            before = count(SENDS, 'alice')
            hook('alice', 102, 'GATE_DISABLE')
            wait(MODEL_GATES['GATE_DISABLE'][0].is_set, 'actual model request before disable')
            assert http(gateway_url, '/api/sessions', 'POST', {}, bearer('alice'))[0] in (409, 429)
            hook('alice', 103, 'QUEUED_UNDER_HOLD')
            cli('user-disable', '--user', users['alice']['user_id'])
            hook('alice', 104, 'DISABLED', expected=403)
            MODEL_GATES['GATE_DISABLE'][1].set()
            wait(lambda: count(MODEL_REQUESTS, 'alice', 'GATE_DISABLE') == 2, 'disabled admitted model completes')
            quiet(lambda: count(SENDS, 'alice') == before and count(MODEL_REQUESTS, 'alice', 'QUEUED_UNDER_HOLD') == 0,
                  'disabled user or shared hold submitted later work', 1.2)
            pause()
            assert event('alice', 102)['status'] == 'completed' and event('alice', 103)['status'] == 'received'
            cli('user-enable', '--user', users['alice']['user_id'])
            resume()
            wait(lambda: count(SENDS, 'alice') == before + 2, 'independently authorized queued replies', 25)
            pause()
            assert all(item['state'] == 'delivered' for update in (102, 103) for item in deliveries('alice', update))
            assert hold('alice') is None
            print('PASS 3: shared foreground/channel admission, user-disable between model and send, explicit reauthorization', flush=True)

            with LOCK:
                MODES['alice'] = 'unknown'
            resume()
            before = count(SENDS, 'alice')
            hook('alice', 105, 'UNKNOWN')
            wait(lambda: count(SENDS, 'alice') == before + 1, 'actual first ambiguous fragment')
            quiet(lambda: count(SENDS, 'alice') == before + 1, 'unknown fragment retried or later fragment sent')
            assert http(gateway_url, '/api/sessions', 'POST', {}, bearer('alice'))[0] == 409
            pause()
            original = deliveries('alice', 105)
            assert len(original) > 1 and original[0]['state'] == 'unknown'
            assert all(item['state'] == 'pending' for item in original[1:])
            held = hold('alice')
            assert held and held['state'] == 'needs_review'
            operations = all_items('alice', 'operations')
            operation = next(item for item in operations if item['request_id'] == held['request_id'])
            assert operation['kind'] == 'delivery' and operation['delivery_id'] == original[0]['id'] and operation['attempt'] == 1
            quota = next(item for item in all_items('alice', 'reservations') if item['delivery_id'] == original[0]['id'])
            assert quota['settled_ms'] is None and quota['attempt'] == 1 and quota['reserved_ms'] == operation['claimed_ms']
            clear('alice', ok=False)
            cli('wecom-purge', '--binding', bindings['alice']['id'], '--event', event('alice', 105)['id'], ok=False)
            model_calls = count(MODEL_REQUESTS, 'alice')
            resume()
            quiet(lambda: count(SENDS, 'alice') == before + 1 and count(MODEL_REQUESTS, 'alice') == model_calls,
                  'unknown NULL reservation replayed after restart')
            pause()
            assert next(item for item in all_items('alice', 'reservations') if item['delivery_id'] == quota['delivery_id']) == quota
            actual_receipt = next(item['receipt'] for item in reversed(SENDS) if item['who'] == 'alice')
            cli('wecom-resolve', '--binding', bindings['alice']['id'], '--delivery', original[0]['id'],
                '--receipt', actual_receipt + ' local platform record independently checked')
            cli('wecom-cancel', '--binding', bindings['alice']['id'], '--event', event('alice', 105)['id'])
            after_review = all_items('alice', 'operations')
            after_quota = all_items('alice', 'reservations')
            assert next(item for item in after_quota if item['delivery_id'] == quota['delivery_id'])['settled_ms'] is not None
            event_id = event('alice', 105)['id']
            cli('wecom-purge', '--binding', bindings['alice']['id'], '--event', event_id)
            assert event('alice', 105) is None
            assert all_items('alice', 'reservations') == after_quota, 'purge changed platform reservation history'
            assert all_items('alice', 'operations') == after_review, 'purge lost original request/delivery/attempt metadata'
            clear('alice')
            with LOCK:
                MODES['alice'] = 'ok'
            resume()
            hook('alice', 105, 'UNKNOWN')
            quiet(lambda: count(MODEL_REQUESTS, 'alice') == model_calls and count(SENDS, 'alice') == before + 1,
                  'purged MsgId tombstone was executed again', 1.3)
            pause()
            print('PASS 4: NULL quota and original send UUID survive restart; offline review/purge preserves permanent metadata without replay', flush=True)

            for update in (106, 107):
                resume()
                hook('alice', update, 'SAME')
                expected = 2 if update == 107 else 1
                wait(lambda: count(MODEL_REQUESTS, 'alice', 'SAME') == expected * 2, 'independent identical model response')
                wait(lambda: sum(item['who'] == 'alice' and 'CASE:SAME ' in item['text'] for item in SENDS) == expected,
                     'independent identical message delivered')
                pause()
                assert deliveries('alice', update)[0]['state'] == 'delivered' and hold('alice') is None
            matching = [item for item in SENDS if item['who'] == 'alice' and 'CASE:SAME ' in item['text']]
            assert len(matching) == 2 and matching[0]['text'] == matching[1]['text'] and matching[0]['receipt'] != matching[1]['receipt']
            assert matching[1]['at'] - matching[0]['at'] >= 3.8, 'persistent app pacing skipped known completion'
            native = model_reply('alice', 'LONG').replace('<', '＜').replace('>', '＞')
            parts = (len(native.encode()) + 2047) // 2048
            before = count(SENDS, 'alice')
            resume()
            hook('alice', 108, 'LONG')
            wait(lambda: count(SENDS, 'alice') == before + parts, 'real Unicode fragments', parts * 5 + 15)
            pause()
            sent = [item for item in SENDS if item['who'] == 'alice'][-parts:]
            assert ''.join(item['text'] for item in sent) == native and all(len(item['text'].encode()) <= 2048 for item in sent)
            assert all(right['at'] - left['at'] >= 3.8 for left, right in zip(sent, sent[1:]))
            assert len(deliveries('alice', 108)) == parts and all(item['state'] == 'delivered' for item in deliveries('alice', 108))
            print('PASS 5: independent identical messages remain distinct; complete rendered UTF-8 fragments and persistent app spacing', flush=True)

            resume()
            before = count(SENDS, 'alice')
            hook('alice', 109, 'GATE_CRASH')
            wait(MODEL_GATES['GATE_CRASH'][0].is_set, 'actual first model request before gateway SIGKILL')
            killed = pause(kill=True)
            assert killed.returncode < 0 and backend['alice']['process'].poll() is None
            active = hold('alice')
            assert active and active['state'] == 'in_flight'
            request_id = active['request_id']
            private_request('alice', request_id, 109, 'admitted')
            resume()
            quiet(lambda: count(MODEL_REQUESTS, 'alice', 'GATE_CRASH') == 1 and count(SENDS, 'alice') == before,
                  'gateway restart repeated submitted model', 1.3)
            hook('bob', 110, 'BOB_DURING_HOLD')
            complete('bob', 110, 'BOB_DURING_HOLD')
            assert hold('alice')['request_id'] == request_id and hold('alice')['state'] == 'needs_review'
            assert event('alice', 109)['status'] == 'needs_review' and deliveries('alice', 109) == []
            MODEL_GATES['GATE_CRASH'][1].set()
            wait(lambda: count(MODEL_REQUESTS, 'alice', 'GATE_CRASH') == 2, 'real detached backend native loop completion')
            wait(lambda: http(backend['alice']['url'], '/internal/channels/wecom/requests/' + request_id,
                              headers={'Authorization': 'Bearer ' + backend['alice']['token']})[1].get('status') == 'completed',
                 'original backend receipt committed')
            private_request('alice', request_id, 109, 'completed')
            stop(backend['alice']['process'])  # Actual idle evidence, not elapsed timeout.
            clear('alice', ok=False)
            cli('wecom-cancel', '--binding', bindings['alice']['id'], '--event', event('alice', 109)['id'])
            clear('alice')
            backend['alice']['process'] = launch(['serve', '--config', str(backend['alice']['path'])], backend['alice']['url'], 'alice')
            resume()
            quiet(lambda: count(MODEL_REQUESTS, 'alice', 'GATE_CRASH') == 2 and count(SENDS, 'alice') == before,
                  'review of original request synthesized a reply', 1.3)
            pause()
            print('PASS 6: actual submitted model SIGKILL, independent tenant progress, original completed metadata and stopped-backend review without replay', flush=True)

            with LOCK:
                MODES['alice'] = 'send_crash'
            resume()
            before = count(SENDS, 'alice')
            hook('alice', 111, 'SEND_CRASH')
            wait(SEND_GATE[0].is_set, 'actual platform POST before gateway SIGKILL')
            killed = pause(kill=True)
            assert killed.returncode < 0
            send_hold = hold('alice')
            # Capture the actual stopped in-flight state before maintenance
            # performs its documented submitting -> unknown recovery.
            submitted = rows(channel_db('alice'),
                             "SELECT o.* FROM channel_outbox o JOIN channel_events e ON e.id=o.event_id "
                             "WHERE json_extract(e.spec,'$.event_id')=? ORDER BY o.ordinal", (message_id(111),))
            assert len(submitted) == 1
            submitting = submitted[0]
            assert submitting['state'] == 'submitting' and send_hold['state'] == 'in_flight'
            null_quota = rows(channel_db('alice'),
                              'SELECT installation_id,delivery_id,attempt,reserved_ms,settled_ms FROM wecom_send_reservations WHERE delivery_id=?',
                              (submitting['id'],))[0]
            assert null_quota['settled_ms'] is None
            assert deliveries('alice', 111)[0]['state'] == 'unknown'
            assert hold('alice')['request_id'] == send_hold['request_id'] and hold('alice')['state'] == 'needs_review'
            assert next(item for item in all_items('alice', 'reservations') if item['delivery_id'] == submitting['id']) == null_quota
            SEND_GATE[1].set()
            with LOCK:
                MODES['alice'] = 'ok'
            resume()
            quiet(lambda: count(SENDS, 'alice') == before + 1 and count(MODEL_REQUESTS, 'alice', 'SEND_CRASH') == 2,
                  'recovered POST or native model was replayed')
            pause()
            assert hold('alice')['request_id'] == send_hold['request_id']
            assert deliveries('alice', 111)[0]['state'] == 'unknown'
            assert next(item for item in all_items('alice', 'reservations') if item['delivery_id'] == submitting['id']) == null_quota
            runtime_settings = settings.pop('wecom')
            save()
            renamed = []
            for item in runtime_settings:
                for key in ('app_secret_file', 'callback_token_file', 'encoding_aes_key_file'):
                    path = Path(item[key]); destination = path.with_suffix('.offline')
                    path.rename(destination); renamed.append((path, destination))
            try:
                assert inspect('alice', 'deliveries', '--event', submitting['event_id'])[0]['state'] == 'unknown'
                assert next(item for item in all_items('alice', 'reservations') if item['delivery_id'] == submitting['id']) == null_quota
                assert any(item['request_id'] == send_hold['request_id'] and item['delivery_id'] == submitting['id']
                           for item in all_items('alice', 'operations'))
                clear('alice', ok=False)
                receipt = next(item['receipt'] for item in reversed(SENDS) if item['who'] == 'alice')
                cli('wecom-resolve', '--binding', bindings['alice']['id'], '--delivery', submitting['id'],
                    '--receipt', receipt + ' local original POST result independently checked')
                clear('alice')
            finally:
                for path, destination in renamed: destination.rename(path)
                settings['wecom'] = runtime_settings
                save()
            assert next(item for item in all_items('alice', 'reservations') if item['delivery_id'] == submitting['id'])['settled_ms'] is not None
            print('PASS 7: actual in-flight POST SIGKILL, NULL quota/original UUID recovery and no-credential offline reconciliation without resend', flush=True)

            for marker, mode in (('RATE', 'rate'), ('TOKEN_REJECTED', 'invalid_token')):
                with LOCK:
                    MODES['alice'] = mode
                resume()
                before = count(SENDS, 'alice')
                token_before = count(AUTH_CALLS, 'alice')
                update = 112 if marker == 'RATE' else 113
                hook('alice', update, marker)
                wait(lambda: count(SENDS, 'alice') == before + 1, 'single rejected/ambiguous POST')
                quiet(lambda: count(SENDS, 'alice') == before + 1 and count(AUTH_CALLS, 'alice') == token_before,
                      'rate or token failure retried POST/refreshed current message')
                pause()
                item = deliveries('alice', update)[0]
                assert item['attempts'] == 1 and item['state'] == ('unknown' if marker == 'RATE' else 'permanent_failed')
                assert hold('alice')['state'] == 'needs_review'
                clear('alice', ok=False)
                cli('wecom-cancel', '--binding', bindings['alice']['id'], '--event', event('alice', update)['id'])
                clear('alice')
            with LOCK:
                MODES['alice'] = 'ok'
            print('PASS 8: 429 remains unknown with one POST; documented credential rejection is terminal, never refresh-and-replay', flush=True)

            resume()
            before = count(SENDS, 'alice')
            hook('alice', 114, 'GATE_REVOKE')
            wait(MODEL_GATES['GATE_REVOKE'][0].is_set, 'actual model request before revoke')
            cli('wecom-revoke', '--binding', bindings['alice']['id'])
            hook('alice', 115, 'REVOKED', expected=403)
            MODEL_GATES['GATE_REVOKE'][1].set()
            wait(lambda: count(MODEL_REQUESTS, 'alice', 'GATE_REVOKE') == 2, 'already admitted revoked model completion')
            quiet(lambda: count(SENDS, 'alice') == before, 'revoked binding submitted reply', 1.3)
            pause()
            assert event('alice', 114)['status'] == 'completed'
            assert all(item['state'] == 'pending' for item in deliveries('alice', 114))
            assert next(item for item in cli('wecom-bindings')['bindings'] if item['id'] == bindings['alice']['id'])['enabled'] is False
            cli('wecom-bind', '--user', users['alice']['user_id'], '--corp-id', IDENTITIES['alice']['corp'],
                '--agent-id', str(IDENTITIES['alice']['agent'] + 100), '--human-user-id', IDENTITIES['alice']['human'], ok=False)
            cli('wecom-bind', '--user', users['bob']['user_id'], '--corp-id', IDENTITIES['alice']['corp'],
                '--agent-id', str(IDENTITIES['alice']['agent']), '--human-user-id', IDENTITIES['bob']['human'], ok=False)
            cli('wecom-cancel', '--binding', bindings['alice']['id'], '--event', event('alice', 114)['id'])
            print('PASS 9: irreversible application/user reservation and current authorization prevent revoked future ingress/send', flush=True)

            # Fill real ledger headroom under an unknown-send hold; do not edit
            # clocks, hold rows, quota timestamps or production limits.
            with LOCK:
                MODES['bob'] = 'rate'
            settings['wecom'] = [item for item in settings['wecom'] if item['binding_id'] == bindings['bob']['id']]
            save()
            resume()
            before = count(SENDS, 'bob')
            hook('bob', 120, 'CAPACITY_HOLD')
            wait(lambda: count(SENDS, 'bob') == before + 1, 'real hold before capacity intake')
            quiet(lambda: count(SENDS, 'bob') == before + 1, 'capacity hold retried', 1.3)
            pause()
            baseline_events = all_items('bob', 'events')
            baseline_ids = {item['spec']['event_id'] for item in baseline_events}
            baseline_hold, baseline_quota = hold('bob'), all_items('bob', 'reservations')
            baseline_operations = all_items('bob', 'operations')
            baseline_deliveries = all_items('bob', 'deliveries')
            reserved_slots = (len(baseline_operations)
                              + sum(item['status'] == 'received' for item in baseline_events) * 17
                              + sum(item['status'] == 'processing' for item in baseline_events) * 16
                              + sum(item['state'] == 'pending' for item in baseline_deliveries))
            available = min(1000 - len(baseline_events), (16000 - reserved_slots) // 17)
            assert 0 < available <= 941, 'unexpected permanent ledger headroom'
            assert baseline_hold['state'] == 'needs_review' and any(item['settled_ms'] is None for item in baseline_quota)
            model_before, send_before = count(MODEL_REQUESTS, 'bob'), count(SENDS, 'bob')
            resume()
            connection = sqlite3.connect(channel_db('bob').as_uri() + '?mode=rw', uri=True, timeout=2)
            try:
                connection.execute('BEGIN IMMEDIATE')
                status, code = hook('bob', 4001, 'LOCKED', expected=503, return_status=True)
                assert code in ('admission_failed', 'ingress_deadline'), 'unexpected SQL-lock response code'
                pause()  # Keep the writer lock until process exit/drain.
                assert hold('bob') == baseline_hold and count(MODEL_REQUESTS, 'bob') == model_before and count(SENDS, 'bob') == send_before
            finally:
                connection.rollback(); connection.close()
            assert {item['spec']['event_id'] for item in all_items('bob', 'events')} == baseline_ids
            assert all_items('bob', 'reservations') == baseline_quota and event('bob', 4001) is None
            resume()
            admitted_ids, busy_total = set(), 0
            for index in range(available):
                update = 10000 + index
                prepared = callback('bob', update, 'QUEUED_CAPACITY')
                started = time.monotonic()
                for attempt in range(8):
                    status, code = hook('bob', expected=(200, 429), prepared=prepared, return_status=True)
                    if status == 200:
                        assert time.monotonic() - started < 2.5, 'same-message admission exceeded finite retry budget'
                        break
                    assert code == 'busy', 'queue_full or unknown status cannot be treated as busy'
                    busy_total += 1
                    assert busy_total <= 32 and time.monotonic() - started < 2.5
                    time.sleep(.03)
                else:
                    raise AssertionError('bounded same-message busy retry exhausted')
                admitted_ids.add(message_id(update))
                assert count(MODEL_REQUESTS, 'bob') == model_before and count(SENDS, 'bob') == send_before
            status, code = hook('bob', 20000, 'QUEUE_FULL', expected=429, return_status=True)
            assert code == 'queue_full', 'full permanent headroom must not be mistaken for transient busy'
            hook('bob', 120, 'CAPACITY_HOLD')  # Dedup remains ACK-able at full headroom.
            pause()
            full = all_items('bob', 'events')
            assert len(full) == len(baseline_events) + available and {item['spec']['event_id'] for item in full} == baseline_ids | admitted_ids
            assert event('bob', 20000) is None and hold('bob') == baseline_hold and all_items('bob', 'reservations') == baseline_quota
            assert all_items('bob', 'operations') == baseline_operations
            cli('wecom-revoke', '--binding', bindings['bob']['id'])
            settings.pop('wecom'); save()
            assert len(all_items('bob', 'events')) == len(full) and all_items('bob', 'reservations') == baseline_quota
            clear('bob', ok=False)
            cli('wecom-purge', '--binding', bindings['bob']['id'], '--event', event('bob', 120)['id'], ok=False)
            cli('wecom-inspect', '--binding', bindings['bob']['id'], '--kind', 'reservations', '--limit', '101', ok=False)
            cli('wecom-inspect', '--binding', bindings['bob']['id'], '--kind', 'reservations', '--offset', '10001', ok=False)
            resume()
            quiet(lambda: count(SENDS, 'bob') == send_before and count(MODEL_REQUESTS, 'bob') == model_before,
                  'revoked runtime-off queue or NULL quota was discarded/replayed', 1.3)
            pause()
            assert len(all_items('bob', 'events')) == len(full) and all_items('bob', 'reservations') == baseline_quota
            print('PASS 10: permanent 16000-operation headroom admits ' + str(available)
                  + ' actual new identities, next is queue_full; lock drain and revoked/runtime-off NULL quota preserve history', flush=True)

            # Shutdown all real backends before any direct backend DB snapshot.
            for who in PEOPLE:
                stop(backend[who]['process'])
                saved = rows(backend[who]['db'], 'SELECT id,messages FROM sessions')
                assert len(saved) == 1 and saved[0]['id'] == 'wecom:' + bindings[who]['id']
                assert 'BASIC_' + who.upper() in saved[0]['messages']
                assert 'BASIC_' + ('BOB' if who == 'alice' else 'ALICE') not in saved[0]['messages']
            secrets = [*APP_SECRETS.values(), *ACCESS_TOKENS.values(), *ENCODING_KEYS.values(),
                       *MODEL_SECRETS.values(), *ISSUED_TOKENS, PRIVATE_ERROR,
                       *(item['token'] for item in backend.values())]
            for path in root.glob('*.log'):
                text = path.read_text(errors='replace')
                assert all(value not in text for value in secrets), 'credential/platform error in logs'
            check_errors()
            assert hashlib.sha256(BINARY.read_bytes()).hexdigest() == binary_digest, 'acceptance binary changed'
            print('Local process contracts pass; live enterprise licensing/TLS/client/expiry refresh and container runtime remain unqualified.', flush=True)
    finally:
        AUTH_GATE[1].set(); SEND_GATE[1].set()
        for received, release in MODEL_GATES.values(): release.set()
        for process in reversed(PROCESSES): stop(process)
        fixture.shutdown(); fixture.server_close(); server_thread.join(timeout=3)
        assert not server_thread.is_alive(), 'fixture server did not drain'


if __name__ == '__main__':
    main()
