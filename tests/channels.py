#!/usr/bin/env python3
"""Real binary + local gateway/platform fixtures for durable channels.

All tokens and Ed25519 keys are disposable fixture values. No provider or
Telegram/Slack/Discord account is contacted. Requires Python 3 and OpenSSL
with Ed25519 support; no Python packages are needed.
"""
from contextlib import closing
import copy
import hashlib
import html
import hmac
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
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
gateway_secret = 'fixture-gateway-' + uuid.uuid4().hex
api_token = 'fixture-host-' + uuid.uuid4().hex
telegram_secret = 'fixture-webhook-' + uuid.uuid4().hex
telegram_token = '123456:' + uuid.uuid4().hex
slack_secret = uuid.uuid4().hex
slack_token = 'xoxb-fixture-' + uuid.uuid4().hex
private_error = 'PRIVATE-PLATFORM-BODY-' + uuid.uuid4().hex
requests_by_case = {}
sends_by_case = {}
destination_cases = {}
gates = {}
fixture_errors = []
fixture_lock = threading.Lock()
expected_texts = {}
process = None
base = None
counter = 100


def gate(name):
    return gates.setdefault(name, threading.Event())


def result_text(case):
    return expected_texts.get(case, 'channel-fixture:' + case + ' completed.')


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, body, headers=None):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(raw)

    def handle_request(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/chat/completions':
                self.model(body)
            else:
                self.platform(body)
        except (BrokenPipeError, ConnectionResetError):
            # Expected when a local request is cancelled or the host is killed.
            pass
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)

    do_POST = handle_request
    do_PATCH = handle_request

    def model(self, body):
        assert self.headers['Authorization'] == 'Bearer ' + gateway_secret
        assert self.headers.get('Idempotency-Key')
        prompt = next(message['content'] for message in reversed(body['messages'])
                      if message['role'] == 'user')
        case = re.search(r'channel-fixture:([a-z_]+)', prompt).group(1)
        assert {tool['function']['name'] for tool in body['tools']} == {'datetime_now'}
        with fixture_lock:
            requests_by_case.setdefault(case, []).append(body)
        if body['messages'][-1]['role'] == 'tool':
            assert body['messages'][-1]['tool_call_id'].startswith('clock-')
            assert 'Unix' in body['messages'][-1]['content']
            if case in ['ack', 'discord', 'processing_crash', 'graceful', 'timeout']:
                gate(case).wait(timeout=90)
            message = {'role': 'assistant', 'content': result_text(case)}
            finish_reason = 'stop'
        else:
            message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                'id': 'clock-' + uuid.uuid4().hex, 'type': 'function',
                'function': {'name': 'datetime_now', 'arguments': '{}'},
            }]}
            finish_reason = 'tool_calls'
        self.respond(200, {'choices': [{'message': message, 'finish_reason': finish_reason}]})

    def platform(self, body):
        if self.path.startswith('/bot'):
            assert self.path == '/bot' + telegram_token + '/sendMessage'
            assert self.command == 'POST'
            assert body['chat_id'] == '-100123'
            assert body['link_preview_options'] == {'is_disabled': True}
            assert 'parse_mode' not in body
            assert isinstance(body['message_thread_id'], int)
            case = destination_cases[('telegram', str(body['message_thread_id']))]
            text = body['text']
            receipt = {'ok': True, 'result': {'message_id': 123,
                                            'chat': {'id': -100123}}}
        elif self.path == '/chat.postMessage':
            assert self.command == 'POST'
            assert self.headers['Authorization'] == 'Bearer ' + slack_token
            assert body['channel'] == 'C123ABC'
            assert body['mrkdwn'] is False and body['parse'] == 'none'
            assert body['link_names'] is False
            assert body['unfurl_links'] is False and body['unfurl_media'] is False
            case = destination_cases[('slack', body['thread_ts'])]
            assert '<@' not in body['text']
            text = html.unescape(body['text'])
            receipt = {'ok': True, 'channel': 'C123ABC', 'ts': '1760000000.123456'}
        else:
            match = re.fullmatch(r'/webhooks/234567/([^/?]+)(/messages/@original|\?wait=true)', self.path)
            assert match, self.path
            assert not self.headers.get('Authorization')
            case = destination_cases[('discord', match.group(1))]
            assert self.command == ('PATCH' if match.group(2).startswith('/') else 'POST')
            assert body['allowed_mentions'] == {'parse': [], 'replied_user': False}
            text = body['content']
            receipt = {'id': '345678', 'channel_id': '456789'}
        assert len(text.encode('utf-16-le')) // 2 <= 2000
        with fixture_lock:
            sends_by_case.setdefault(case, []).append({'body': body, 'text': text,
                                                       'at': time.monotonic(), 'method': self.command})
            attempt = len(sends_by_case[case])
        if case == 'rate_limit' and attempt == 1:
            self.respond(429, {'ok': False, 'error_code': 429,
                               'parameters': {'retry_after': 4}})
        elif case == 'short_retry' and attempt == 1:
            gate(case).wait(timeout=90)
            self.respond(429, {'retry_after': .001, 'global': False})
        elif case == 'rate_exhausted':
            self.respond(429, {'ok': False, 'error': 'ratelimited'}, {'Retry-After': '1'})
        elif case == 'server_error' or (case == 'blocked' and attempt == 1):
            self.respond(502, {'error': private_error + ' ' + slack_token})
        elif case == 'bad_receipt':
            self.respond(200, {'ok': True, 'channel': 'WRONG', 'ts': '1760000000.123456',
                               'private': private_error})
        elif case == 'disconnect':
            # Platform received the full body; its effect is deliberately unknown.
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            self.close_connection = True
        elif case == 'submitting_crash':
            gate(case).wait(timeout=90)
            self.respond(200, receipt)
        else:
            self.respond(200, receipt)


fixture = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
fixture.daemon_threads = True
threading.Thread(target=fixture.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-channels-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        config = root / 'config.json'
        database = root / 'state' / 'sessions.sqlite3'
        key = root / 'fixture-ed25519.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(key)],
                       check=True, capture_output=True)
        public_der = subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout',
                                     '-outform', 'DER'], check=True, capture_output=True).stdout
        assert public_der.startswith(bytes.fromhex('302a300506032b6570032100'))
        public_key = public_der[-32:].hex()
        fixture_base = 'http://127.0.0.1:' + str(fixture.server_port)
        settings = {
            'agent': {'name': 'channels-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': fixture_base,
                         'api_key': gateway_secret, 'model': 'fixture'},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1,
                     'telegram_secret': telegram_secret, 'telegram_bot_token': telegram_token,
                     'slack_signing_secret': slack_secret, 'slack_bot_token': slack_token,
                     'discord_public_key': public_key,
                     'channels': [
                         {'channel': 'telegram', 'installation_id': '123456',
                          'allowed_senders': ['987654'], 'allowed_conversations': ['-100123']},
                         {'channel': 'slack', 'installation_id': 'T123ABC', 'app_id': 'A123ABC',
                          'allowed_senders': ['U123ABC'], 'allowed_conversations': ['C123ABC']},
                         {'channel': 'discord', 'installation_id': '234567',
                          'allowed_senders': ['567890'], 'allowed_conversations': ['456789']},
                     ]},
        }
        for binding in settings['http']['channels']:
            binding.update(enabled_tools=['datetime_now'], timeout_secs=30,
                           local_test_api_base=fixture_base)
        env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
        env['JIACLAW_LOG_LEVEL'] = 'info'
        env['JIACLAW_CHANNEL_STATE_KEY'] = uuid.uuid4().hex + uuid.uuid4().hex
        logs = []

        def request(path, method='GET', body=None, authenticated=True, headers=None, raw=None):
            supplied = {'Authorization': 'Bearer ' + api_token} if authenticated else {}
            supplied.update(headers or {})
            if body is not None or raw is not None:
                supplied['Content-Type'] = 'application/json'
            data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
            req = urllib.request.Request(base + path, method=method, headers=supplied, data=data)
            try:
                response = urllib.request.urlopen(req, timeout=10)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                result = response.read()
                if not result:
                    return response.status, None
                if 'application/json' in response.headers.get('Content-Type', ''):
                    return response.status, json.loads(result)
                return response.status, result.decode()

        def start():
            global process, base, log
            config.write_text(json.dumps(settings))
            log = root / ('server-' + uuid.uuid4().hex + '.log')
            logs.append(log)
            with log.open('wb') as output:
                process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
                                           env=env, stdout=output, stderr=output)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert process.poll() is None, log.read_text()
                match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
                if match:
                    base = match.group(1)
                    eventually(lambda: channel_health() == 'running',
                               'channel worker running')
                    return
                time.sleep(.05)
            raise AssertionError('host startup timeout: ' + log.read_text())

        def stop(kill=False):
            if process and process.poll() is None:
                process.kill() if kill else process.terminate()
                process.wait(timeout=15)

        def eventually(check, description, timeout=15):
            deadline = time.monotonic() + timeout
            last = None
            while time.monotonic() < deadline:
                assert process.poll() is None, log.read_text()
                assert not fixture_errors, fixture_errors
                last = check()
                if last:
                    return last
                time.sleep(.05)
            raise AssertionError(description + ' timed out; last=' + repr(last) + '\n' + log.read_text())

        def observed(case):
            return len(requests_by_case.get(case, []))

        def channel_health():
            status, value = request('/api/channels/status')
            assert status == 200 and value['max_processing'] == 4, (status, value)
            return value['state']

        def sent(case):
            return len(sends_by_case.get(case, []))

        def events():
            status, value = request('/api/channels/events?limit=100')
            assert status == 200 and isinstance(value, list), (status, value)
            return value

        def event(event_id):
            status, value = request('/api/channels/events/' + event_id)
            assert status == 200, (status, value)
            return value

        def deliveries(event_id):
            status, value = request('/api/channels/deliveries?limit=100')
            assert status == 200 and isinstance(value, list), (status, value)
            return sorted((item for item in value if item['event_id'] == event_id),
                          key=lambda item: item['ordinal'])

        def delivery_state(event_id, state, count=1):
            items = deliveries(event_id)
            return items if len(items) == count and all(item['state'] == state for item in items) else None

        def payload(channel, case):
            global counter
            counter += 1
            prompt = 'channel-fixture:' + case
            if channel == 'telegram':
                destination_cases[(channel, str(counter))] = case
                return {'update_id': counter, 'message': {'message_id': counter,
                        'from': {'id': 987654, 'is_bot': False}, 'chat': {'id': -100123},
                        'message_thread_id': counter, 'text': prompt}}
            if channel == 'slack':
                thread = '1760000000.' + str(counter).zfill(6)
                destination_cases[(channel, thread)] = case
                return {'type': 'event_callback', 'event_id': 'Ev' + str(counter),
                        'team_id': 'T123ABC', 'api_app_id': 'A123ABC',
                        'event': {'type': 'message', 'user': 'U123ABC', 'channel': 'C123ABC',
                                  'text': prompt, 'ts': '1760000001.000001', 'thread_ts': thread}}
            token = 'fixture-interaction-' + uuid.uuid4().hex
            destination_cases[(channel, token)] = case
            interaction_id = ((int(time.time() * 1000) - 1420070400000) << 22) | counter
            return {'id': str(interaction_id), 'application_id': '234567', 'type': 2,
                    'token': token, 'channel_id': '456789', 'member': {'user': {'id': '567890'}},
                    'data': {'name': 'ask', 'type': 1,
                             'options': [{'name': 'prompt', 'type': 3, 'value': prompt}]}}

        def webhook(channel, body, stale=False, bad_signature=False):
            raw = json.dumps(body, separators=(',', ':')).encode()
            timestamp = str(int(time.time()) - (601 if stale else 0))
            if channel == 'telegram':
                headers = {'X-Telegram-Bot-Api-Secret-Token':
                           'invalid' if bad_signature else telegram_secret}
            elif channel == 'slack':
                signature = hmac.new(slack_secret.encode(), b'v0:' + timestamp.encode() + b':' + raw,
                                     hashlib.sha256).hexdigest()
                headers = {'X-Slack-Request-Timestamp': timestamp,
                           'X-Slack-Signature': 'v0=' + ('0' * 64 if bad_signature else signature)}
            else:
                signed = root / 'signature-input.bin'
                signed.write_bytes(timestamp.encode() + raw)
                signature = subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(key),
                                            '-rawin', '-in', str(signed)],
                                           check=True, capture_output=True).stdout.hex()
                headers = {'X-Signature-Timestamp': timestamp,
                           'X-Signature-Ed25519': '0' * 128 if bad_signature else signature}
            return request('/hooks/' + channel, 'POST', authenticated=False, headers=headers, raw=raw)

        def admit(channel, body, duplicate=False):
            started = time.monotonic()
            status, value = webhook(channel, body)
            assert time.monotonic() - started < 3, 'webhook acknowledgement exceeded 3 seconds'
            assert status == 200, (status, value)
            if channel == 'discord':
                assert value == {'type': 5}, value
                matches = [item for item in events() if item['spec']['event_id'] == body['id']
                           and item['spec']['destination']['channel'] == 'discord']
                assert len(matches) == 1
                return matches[0]['id']
            assert value['ok'] is True and value['duplicate'] is duplicate, value
            return value['event_id']

        def completed(event_id, case, parts=1):
            rows = eventually(lambda: delivery_state(event_id, 'delivered', parts), case + ' delivered')
            state = event(event_id)
            assert state['status'] == 'completed', state
            assert all(item['receipt'] and item['attempts'] >= 1 for item in rows), rows
            assert ''.join(item['text'] for item in rows) == result_text(case)
            status, history = request('/api/sessions/' + state['spec']['session_id'])
            assert status == 200, (status, history)
            assert history['messages'][-1]['content'] == result_text(case), history
            assert observed(case) == 2, (case, observed(case))
            return rows

        def resolve(item, action):
            status, value = request('/api/channels/deliveries/' + item['id'] + '/resolve', 'POST',
                                    {'action': action, 'receipt': 'Fixture administrator verified platform evidence.'})
            assert status in [200, 204], (status, value)

        # Admission fails before starting any model call if durability or explicit
        # unattended permissions are missing. Production endpoints stay disabled.
        for invalid in ['memory_store', 'no_api_token', 'legacy_provider', 'no_policy',
                        'no_senders', 'no_tools', 'unsafe_tool', 'unknown_tool',
                        'wrong_installation', 'remote_fixture', 'discord_no_key']:
            rejected = copy.deepcopy(settings)
            rejected_env = env.copy()
            if invalid == 'memory_store':
                rejected['http']['persist'] = False
            elif invalid == 'no_api_token':
                rejected['http'].pop('api_token')
            elif invalid == 'legacy_provider':
                rejected['provider']['provider_type'] = 'openai_compatible'
            elif invalid == 'no_policy':
                rejected['http']['channels'] = []
            elif invalid == 'no_senders':
                rejected['http']['channels'][0]['allowed_senders'] = []
            elif invalid == 'no_tools':
                rejected['http']['channels'][0]['enabled_tools'] = []
            elif invalid in ['unsafe_tool', 'unknown_tool']:
                rejected['http']['channels'][0]['enabled_tools'] = [
                    'http_get' if invalid == 'unsafe_tool' else 'not_registered']
            elif invalid == 'wrong_installation':
                rejected['http']['channels'][0]['installation_id'] = 'wrong'
            elif invalid == 'remote_fixture':
                rejected['http']['channels'][0]['local_test_api_base'] = 'http://example.com'
            else:
                rejected_env.pop('JIACLAW_CHANNEL_STATE_KEY')
            config.write_text(json.dumps(rejected))
            outcome = subprocess.run([str(binary), 'serve', '--config', str(config)], env=rejected_env,
                                     capture_output=True, text=True, timeout=10)
            assert outcome.returncode != 0, invalid
            assert all(secret not in outcome.stdout + outcome.stderr
                       for secret in [gateway_secret, telegram_token, slack_token])
        assert requests_by_case == {} and sends_by_case == {}
        start()
        for path in ['/api/channels/status', '/api/channels/events', '/api/channels/deliveries']:
            assert request(path, authenticated=False)[0] == 401
        assert events() == []
        assert request('/api/channels/events?limit=0')[0] == 400
        assert request('/api/channels/deliveries?limit=101')[0] == 400

        # Platform authentication alone cannot authorize a sender, destination,
        # workspace, or application. Reject before persisting/model execution.
        for channel in ['telegram', 'slack', 'discord']:
            body = payload(channel, 'rejected')
            assert webhook(channel, body, bad_signature=True)[0] == 401
            if channel != 'telegram':
                assert webhook(channel, body, stale=True)[0] == 401
            mutations = []
            if channel == 'telegram':
                mutations = [('sender', lambda b: b['message']['from'].update(id=111)),
                             ('chat', lambda b: b['message']['chat'].update(id=222)),
                             ('no_id', lambda b: b.pop('update_id'))]
            elif channel == 'slack':
                mutations = [('sender', lambda b: b['event'].update(user='UOTHER')),
                             ('chat', lambda b: b['event'].update(channel='COTHER')),
                             ('team', lambda b: b.update(team_id='TOTHER')),
                             ('app', lambda b: b.update(api_app_id='AOTHER')),
                             ('no_id', lambda b: b.pop('event_id'))]
            else:
                mutations = [('sender', lambda b: b['member']['user'].update(id='111')),
                             ('chat', lambda b: b.update(channel_id='222')),
                             ('app', lambda b: b.update(application_id='333')),
                             ('expired', lambda b: b.update(id=str(
                                 (int(time.time() * 1000) - 15 * 60 * 1000 - 1420070400000) << 22))),
                             ('future', lambda b: b.update(id=str(
                                 (int(time.time() * 1000) + 120000 - 1420070400000) << 22))),
                             ('no_id', lambda b: b.pop('id'))]
            for label, mutate in mutations:
                changed = copy.deepcopy(body)
                mutate(changed)
                status, value = webhook(channel, changed)
                assert 400 <= status < 500, (channel, label, status, value)
        assert events() == [] and observed('rejected') == 0

        # The slow Agent is already submitted, yet webhook ACK and every retry
        # return promptly and refer to one durable event and one native turn.
        ack_body = payload('telegram', 'ack')
        ack_id = admit('telegram', ack_body)
        eventually(lambda: observed('ack') == 2, 'slow admitted Agent')
        assert event(ack_id)['status'] == 'processing'
        for _ in range(3):
            assert admit('telegram', ack_body, duplicate=True) == ack_id
        assert observed('ack') == 2 and sent('ack') == 0
        changed = copy.deepcopy(ack_body)
        changed['message']['text'] += ' changed'
        assert 400 <= webhook('telegram', changed)[0] < 500
        gate('ack').set()
        completed(ack_id, 'ack')
        assert sent('ack') == 1
        assert request('/api/channels/events/' + ack_id, authenticated=False)[0] == 401
        assert len(request('/api/channels/events?limit=1')[1]) == 1

        slack_body = payload('slack', 'slack')
        slack_id = admit('slack', slack_body)
        assert admit('slack', slack_body, duplicate=True) == slack_id
        completed(slack_id, 'slack')
        assert sent('slack') == 1
        assert event(slack_id)['spec']['session_id'] != event(ack_id)['spec']['session_id']

        # Long Unicode output survives deterministic ordered splitting; neither
        # truncation nor accidental mentions/embeds are acceptable substitutes.
        expected_texts['long'] = 'channel-fixture:long ' + '🙂' * 1300 + ' @everyone <@U123ABC>'
        long_id = admit('slack', payload('slack', 'long'))
        completed(long_id, 'long', parts=2)
        assert ''.join(item['text'] for item in sends_by_case['long']) == result_text('long'), [
            (len(item['text']), item['text'][:25], item['text'][-35:]) for item in sends_by_case['long']]

        discord_body = payload('discord', 'discord')
        discord_id = admit('discord', discord_body)
        assert admit('discord', discord_body, duplicate=True) == discord_id
        eventually(lambda: observed('discord') == 2, 'submitted Discord turn')
        public_event = event(discord_id)
        assert 'sealed_token' not in public_event['spec']
        assert discord_body['token'] not in json.dumps(public_event)
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            sealed, saved = db.execute('SELECT sealed_token,spec FROM channel_events WHERE id=?',
                                      (discord_id,)).fetchone()
            assert sealed and re.fullmatch('[0-9a-f]+', sealed)
            assert discord_body['token'] not in sealed + saved
            assert json.loads(saved)['sealed_token'] is None
        gate('discord').set()
        completed(discord_id, 'discord')
        assert sent('discord') == 1

        expected_texts['discord_long'] = 'channel-fixture:discord_long ' + '🙂' * 1300
        discord_long_id = admit('discord', payload('discord', 'discord_long'))
        completed(discord_long_id, 'discord_long', parts=2)
        assert [item['method'] for item in sends_by_case['discord_long']] == ['PATCH', 'POST']
        assert ''.join(item['text'] for item in sends_by_case['discord_long']) == result_text('discord_long')

        expected_texts['discord_overlong'] = 'channel-fixture:discord_overlong ' + 'd' * 12000
        discord_overlong_id = admit('discord', payload('discord', 'discord_overlong'))
        eventually(lambda: event(discord_overlong_id)['status'] == 'needs_review',
                   'Discord reply requiring more than five follow-ups refused')
        assert deliveries(discord_overlong_id) == [] and sent('discord_overlong') == 0

        # The relative 1ms cooldown is computed only after the completion write
        # obtains the database lock. Contention must not turn valid 429 into a
        # fatal worker error or forget the later safe retry.
        short_id = admit('discord', payload('discord', 'short_retry'))
        eventually(lambda: sent('short_retry') == 1, 'Discord rate-limit response held')
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            db.execute('BEGIN IMMEDIATE')
            gate('short_retry').set()
            time.sleep(.15)
            db.commit()
        short_rows = completed(short_id, 'short_retry')
        assert short_rows[0]['attempts'] == 2 and sent('short_retry') == 2
        assert channel_health() == 'running'

        expected_texts['oversized'] = 'channel-fixture:oversized ' + 'a' * 16384
        oversized_id = admit('slack', payload('slack', 'oversized'))
        eventually(lambda: event(oversized_id)['status'] == 'needs_review', 'oversized reply refused')
        assert deliveries(oversized_id) == [] and observed('oversized') == 2 and sent('oversized') == 0

        # Only a documented 429 permits automatic retry, and the persisted
        # installation cooldown survives a restart without a premature send.
        rate_id = admit('telegram', payload('telegram', 'rate_limit'))
        waiting = eventually(lambda: delivery_state(rate_id, 'retry_wait'), '429 cooldown')[0]
        assert waiting['attempts'] == 1 and waiting['next_attempt_ms'] > int(time.time() * 1000)
        first_sent = sends_by_case['rate_limit'][0]['at']
        stop(kill=True)
        start()
        if time.monotonic() - first_sent < 3:
            assert sent('rate_limit') == 1
        rate_rows = completed(rate_id, 'rate_limit')
        assert rate_rows[0]['attempts'] == 2 and sent('rate_limit') == 2
        assert sends_by_case['rate_limit'][1]['at'] - first_sent >= 3

        exhausted_id = admit('slack', payload('slack', 'rate_exhausted'))
        exhausted = eventually(lambda: delivery_state(exhausted_id, 'permanent_failed'),
                               'bounded rate-limit retries', timeout=35)[0]
        assert exhausted['attempts'] == 5 and sent('rate_exhausted') == 5
        resolve(exhausted, 'cancel')

        # Ambiguous transport/HTTP/receipt errors never replay automatically;
        # explicit administrative resolution records evidence without sending.
        unknowns = []
        for channel, case in [('slack', 'server_error'), ('slack', 'bad_receipt'),
                              ('telegram', 'disconnect')]:
            event_id = admit(channel, payload(channel, case))
            item = eventually(lambda: delivery_state(event_id, 'unknown'), case + ' unknown')[0]
            assert item['attempts'] == 1 and sent(case) == 1
            assert private_error not in json.dumps(item)
            unknowns.append((case, event_id, item))
        expected_texts['blocked'] = 'channel-fixture:blocked ' + 'a' * 2500
        blocked_id = admit('slack', payload('slack', 'blocked'))
        def blocked_parts():
            parts = deliveries(blocked_id)
            return parts if [item['state'] for item in parts] == ['unknown', 'pending'] else None
        blocked = eventually(blocked_parts, 'uncertain first part blocks remaining parts')
        assert sent('blocked') == 1
        stop(kill=True)
        start()
        time.sleep(.5)
        assert blocked_parts() and sent('blocked') == 1
        assert request('/api/channels/events/' + blocked_id, 'DELETE')[0] == 409
        resolve(blocked[0], 'delivered')
        completed(blocked_id, 'blocked', parts=2)
        assert sent('blocked') == 2
        for case, event_id, item in unknowns:
            assert delivery_state(event_id, 'unknown') and sent(case) == 1
            endpoint = '/api/channels/deliveries/' + item['id'] + '/resolve'
            assert request(endpoint, 'POST', {'action': 'delivered', 'receipt': ''})[0] >= 400
            assert request(endpoint, 'POST', {'action': 'cancel', 'receipt': 'checked'},
                           authenticated=False)[0] == 401
            action = 'delivered' if case == 'server_error' else 'cancel'
            resolve(item, action)
            assert delivery_state(event_id, 'delivered' if action == 'delivered' else 'cancelled')
            assert sent(case) == 1

        crashing_id = admit('telegram', payload('telegram', 'processing_crash'))
        eventually(lambda: observed('processing_crash') == 2, 'processing request submitted')
        assert event(crashing_id)['status'] == 'processing'
        stop(kill=True)
        gate('processing_crash').set()
        start()
        assert event(crashing_id)['status'] == 'needs_review'
        assert deliveries(crashing_id) == []
        assert request('/api/sessions/' + event(crashing_id)['spec']['session_id'])[0] == 404
        time.sleep(.3)
        assert observed('processing_crash') == 2 and sent('processing_crash') == 0

        submitting_id = admit('telegram', payload('telegram', 'submitting_crash'))
        eventually(lambda: sent('submitting_crash') == 1, 'platform request submitted')
        assert delivery_state(submitting_id, 'submitting')
        stop(kill=True)
        gate('submitting_crash').set()
        start()
        submitting = eventually(lambda: delivery_state(submitting_id, 'unknown'),
                                'orphan submitting is unknown')[0]
        time.sleep(.3)
        assert sent('submitting_crash') == 1
        resolve(submitting, 'cancel')

        # Purging explicitly reviewed history retains a seven-day dedup entry.
        assert request('/api/channels/events/' + ack_id, 'DELETE')[0] in [200, 204]
        assert request('/api/channels/events/' + ack_id)[0] == 404
        assert admit('telegram', ack_body, duplicate=True) == ack_id
        assert observed('ack') == 2 and sent('ack') == 1
        assert request('/api/channels/events/' + crashing_id, 'DELETE')[0] >= 400
        status, value = request('/api/channels/events/' + crashing_id + '/cancel', 'POST',
                                {'receipt': 'Fixture verified no safe automatic recovery.'})
        assert status in [200, 204], (status, value)
        assert request('/api/channels/events/' + crashing_id, 'DELETE')[0] in [200, 204]

        graceful_id = admit('telegram', payload('telegram', 'graceful'))
        eventually(lambda: observed('graceful') == 2, 'graceful request submitted')
        stop()
        gate('graceful').set()
        start()
        assert event(graceful_id)['status'] == 'needs_review'
        assert observed('graceful') == 2 and sent('graceful') == 0

        stop()
        settings['http']['channels'][0]['timeout_secs'] = 1
        start()
        timeout_id = admit('telegram', payload('telegram', 'timeout'))
        eventually(lambda: event(timeout_id)['status'] == 'needs_review', 'bounded channel Agent timeout')
        assert observed('timeout') == 2 and deliveries(timeout_id) == [] and sent('timeout') == 0
        gate('timeout').set()
        stop()
        settings['http']['channels'][0]['timeout_secs'] = 30
        start()
        assert event(timeout_id)['status'] == 'needs_review' and observed('timeout') == 2

        # One fixture-only SQLite trigger makes the final event update fail
        # after session/outbox writes. Their transaction must roll back together.
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            db.execute("CREATE TRIGGER fixture_channel_commit_fault BEFORE UPDATE OF status ON channel_events "
                       "WHEN NEW.status='completed' BEGIN SELECT RAISE(ABORT,'fixture channel write fault'); END")
            db.commit()
        fault_id = admit('telegram', payload('telegram', 'fault'))
        eventually(lambda: channel_health() == 'failed',
                   'failed worker rejects new admissions')
        faulty = eventually(lambda: event(fault_id) if event(fault_id)['status'] == 'needs_review' else None,
                            'failed completion interrupted')
        assert request('/api/sessions/' + faulty['spec']['session_id'])[0] == 404
        assert deliveries(fault_id) == [] and observed('fault') == 2 and sent('fault') == 0
        rejected = payload('telegram', 'after_fault')
        assert webhook('telegram', rejected)[0] == 503
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            db.execute('DROP TRIGGER fixture_channel_commit_fault')
            db.commit()
        assert channel_health() == 'failed'
        assert webhook('telegram', rejected)[0] == 503
        stop()
        start()
        assert event(fault_id)['status'] == 'needs_review' and observed('fault') == 2
        assert observed('after_fault') == 0

        public = json.dumps(events()) + json.dumps(request('/api/channels/deliveries')[1])
        public += ''.join(path.read_text() for path in logs)
        for secret in [gateway_secret, telegram_token, slack_token, discord_body['token'],
                       env['JIACLAW_CHANNEL_STATE_KEY'], private_error]:
            assert secret not in public, 'credential or untrusted platform error leaked'
        assert 'sealed_token' not in json.dumps(events())
        stop()
        for path in [database, database.with_name(database.name + '-wal')]:
            if path.exists():
                assert discord_body['token'].encode() not in path.read_bytes()
        assert not fixture_errors, fixture_errors
        print('Channel acceptance passed: authenticated durable ACK/dedup, native tools, ordered Unicode '
              'delivery, receipts, bounded 429 cooldown/recovery, ambiguous-send resolution, processing/'
              'submitting crash recovery, encrypted interaction tokens, transactional failure and fail-closed admission.')
finally:
    for pending_gate in gates.values():
        pending_gate.set()
    if process and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    fixture.shutdown()
    fixture.server_close()
