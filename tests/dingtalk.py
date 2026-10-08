#!/usr/bin/env python3
"""Real binary + disposable localhost DingTalk app and native-model fixtures.

Python 3 standard library only. No live DingTalk credentials, accounts or messages.
This proves standalone wire and recovery contracts, not a tenant gateway or a
live enterprise installation, public TLS ingress, license or client delivery.
"""
import base64
import copy
from contextlib import closing
import hashlib
import hmac
import http.client
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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
import urllib.parse
import urllib.request
import uuid


binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
gateway_secret, api_token = 'fixture-gateway-' + uuid.uuid4().hex, 'fixture-api-' + uuid.uuid4().hex
app_secret = 'fixture-app-' + uuid.uuid4().hex
callback_secret = 'fixture-webhook-' + uuid.uuid4().hex
access_token, private_error = 'fixture-token-' + uuid.uuid4().hex, 'PRIVATE-DINGTALK-' + uuid.uuid4().hex
corp, robot_code, client_id = 'dingFixtureCorp', 'dingFixtureRobot', 'dingDistinctClientId'
installation = robot_code + ':' + corp
model_calls, platform_calls, token_calls, gates = {}, {}, [], {}
fixture_errors, all_sends, callback_signatures = [], [], []
fixture_lock = threading.Lock()
process, base = None, None
token_response_case = 'normal'


def gate(case):
    return gates.setdefault(case, threading.Event())


def reply(case):
    if case == 'long':
        return 'dingtalk-fixture:long ' + '中文😀<>&' * 900
    return 'dingtalk-fixture:' + case + ' completed. <a href="invalid">plain</a> & 中文😀'


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, body=None, *, raw=None, content_types=('application/json',)):
        if raw is None:
            raw = json.dumps(body).encode()
        self.send_response(status)
        for content_type in content_types:
            self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        fixture_errors.append('Unexpected GET request; callback URLs must never be used')
        self.send_error(500)

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/chat/completions':
                return self.model(body)
            if self.path == '/v1.0/oauth2/accessToken':
                assert body == {'appKey': client_id, 'appSecret': app_secret}
                assert not self.headers.get('Authorization')
                token_calls.append(time.monotonic())
                token = {'accessToken': access_token, 'expireIn': 7200}
                with fixture_lock:
                    token_case = token_response_case
                if token_case == 'token_array':
                    return self.respond(200, [access_token, 7200, None])
                if token_case == 'token_duplicate':
                    raw = ('{"accessToken":' + json.dumps(access_token)
                           + ',"accessToken":' + json.dumps(access_token) + ',"expireIn":7200}').encode()
                    return self.respond(200, raw=raw)
                if token_case == 'token_duplicate_expiry':
                    raw = ('{"accessToken":' + json.dumps(access_token)
                           + ',"expireIn":7200,"expireIn":7200}').encode()
                    return self.respond(200, raw=raw)
                if token_case == 'token_mime_text':
                    return self.respond(200, token, content_types=('application/json', 'text/plain'))
                if token_case == 'token_mime_json':
                    return self.respond(200, token, content_types=('application/json', 'application/json'))
                if token_case == 'token_code_conflict':
                    return self.respond(200, {**token, 'code': 'invalidClientIdOrSecret',
                                              'message': private_error + access_token})
                assert token_case == 'normal', 'unknown token fixture mode'
                return self.respond(200, token, content_types=('application/json; charset=utf-8',))
            assert self.path == '/v1.0/robot/oToMessages/batchSend'
            assert self.headers['x-acs-dingtalk-access-token'] == access_token
            assert not self.headers.get('Authorization')
            assert body['robotCode'] == robot_code and body['robotCode'] != client_id
            assert body['msgKey'] == 'sampleText'
            assert set(body) == {'robotCode', 'userIds', 'msgKey', 'msgParam'}, body
            assert body['userIds'] in [['Alice@Example.COM'], ['Long.User']]
            assert isinstance(body['msgParam'], str)
            params = json.loads(body['msgParam'])
            assert set(params) == {'content'}
            text = params['content']
            assert 0 < len(text.encode('utf-16-le')) // 2 <= 2000
            case = ('long' if body['userIds'] == ['Long.User'] and 'dingtalk-fixture:' not in text else
                    re.search(r'dingtalk-fixture:([a-z_]+)', text).group(1))
            with fixture_lock:
                record = {'body': body, 'text': text, 'at': time.monotonic()}
                platform_calls.setdefault(case, []).append(record)
                all_sends.append(record)
            if case == 'unknown':
                return self.respond(502, {'code': 'UnknownFailure', 'message': private_error + access_token})
            if case == 'unknown_client':
                return self.respond(400, {'code': 'UnknownParameter', 'message': private_error})
            if case == 'rate_unknown':
                return self.respond(429, {'code': 'Throttling', 'message': private_error})
            receipt = {'processQueryKey': 'fixture-' + uuid.uuid4().hex,
                       'invalidStaffIdList': [], 'flowControlledStaffIdList': [], 'filteredStaffIdList': []}
            if case == 'send_array':
                return self.respond(200, [receipt['processQueryKey'], [], [], [], None])
            if case == 'send_duplicate_receipt':
                raw = ('{"processQueryKey":null,"processQueryKey":'
                       + json.dumps(receipt['processQueryKey']) + ',"filteredStaffIdList":[]}').encode()
                return self.respond(200, raw=raw)
            if case == 'send_duplicate_filter':
                raw = ('{"processQueryKey":' + json.dumps(receipt['processQueryKey'])
                       + ',"filteredStaffIdList":null,"filteredStaffIdList":[]}').encode()
                return self.respond(200, raw=raw)
            if case == 'send_mime_text':
                return self.respond(200, receipt, content_types=('application/json', 'text/plain'))
            if case == 'send_mime_json':
                return self.respond(200, receipt, content_types=('application/json', 'application/json'))
            if case in ['send_code_receipt', 'send_code_empty_receipt']:
                if case == 'send_code_empty_receipt':
                    receipt['processQueryKey'] = ''
                return self.respond(400, {**receipt, 'code': 'invalidParameter.msgParam.invalid',
                                         'message': private_error + access_token})
            if case == 'filter_omitted':
                del receipt['filteredStaffIdList']
            elif case == 'filter_null':
                receipt['filteredStaffIdList'] = None
            if case == 'invalid_user':
                receipt['invalidStaffIdList'] = body['userIds']
            if case == 'flow_controlled':
                receipt['flowControlledStaffIdList'] = body['userIds']
            if case == 'filtered_user':
                receipt['filteredStaffIdList'] = body['userIds']
            if case == 'missing_receipt':
                del receipt['processQueryKey']
            if case == 'submitting':
                gate(case).wait(timeout=90)
            if case == 'same' and len(platform_calls[case]) == 1:
                time.sleep(.5)  # Pacing starts after receipt, not before this delay.
            self.respond(200, receipt, content_types=('application/json; charset=utf-8',))
            record['completed_at'] = time.monotonic()
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)

    def model(self, body):
        assert self.headers['Authorization'] == 'Bearer ' + gateway_secret
        assert self.headers.get('Idempotency-Key')
        prompt = next(message['content'] for message in reversed(body['messages'])
                      if message['role'] == 'user')
        case = re.search(r'dingtalk-fixture:([a-z_]+)', prompt).group(1)
        assert {tool['function']['name'] for tool in body['tools']} == {'datetime_now'}
        model_calls.setdefault(case, []).append(body)
        if body['messages'][-1]['role'] == 'tool':
            assert body['messages'][-1]['tool_call_id'].startswith('clock-')
            assert 'Unix' in body['messages'][-1]['content']
            if case in ['ack', 'processing']:
                gate(case).wait(timeout=90)
            message, finish = {'role': 'assistant', 'content': reply(case)}, 'stop'
        else:
            message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                'id': 'clock-' + uuid.uuid4().hex, 'type': 'function',
                'function': {'name': 'datetime_now', 'arguments': '{}'},
            }]}
            finish = 'tool_calls'
        self.respond(200, {'choices': [{'message': message, 'finish_reason': finish}]})


fixture = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
fixture.daemon_threads = True
threading.Thread(target=fixture.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-dingtalk-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        config = root / 'config.json'
        fixture_base = 'http://127.0.0.1:' + str(fixture.server_port)
        settings = {
            'agent': {'name': 'dingtalk-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': fixture_base,
                         'api_key': gateway_secret, 'model': 'fixture'},
            'scheduler': {'enabled': True},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1,
                     'dingtalk_app_secret': app_secret,
                     'channels': [{'channel': 'dingtalk', 'installation_id': installation, 'app_id': client_id,
                                   'allowed_senders': ['Alice@Example.COM', 'Long.User'],
                                   'allowed_conversations': ['Alice@Example.COM', 'Long.User'],
                                   'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                                   'local_test_api_base': fixture_base}]},
        }
        env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
        env['JIACLAW_LOG_LEVEL'] = 'info'
        logs = []

        def request(path, method='GET', body=None, authenticated=True, extra_headers=None):
            headers = {'Authorization': 'Bearer ' + api_token} if authenticated else {}
            headers.update(extra_headers or {})
            data = None
            if body is not None:
                data = json.dumps(body).encode()
                headers['Content-Type'] = 'application/json'
            req = urllib.request.Request(base + path, method=method, headers=headers, data=data)
            try:
                response = urllib.request.urlopen(req, timeout=10)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                raw = response.read()
                if 'application/json' in response.headers.get('Content-Type', '') and raw:
                    return response.status, json.loads(raw)
                return response.status, raw.decode()

        def start(expected_channel_state='running', jobs_enabled=True):
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
                    assert request('/api/channels/status')[1]['state'] == expected_channel_state
                    jobs_status, jobs = request('/api/jobs/status')
                    if jobs_enabled:
                        assert jobs_status == 200 and jobs['state'] == 'running'
                    else:
                        assert jobs_status == 404
                    return
                time.sleep(.05)
            raise AssertionError('startup timeout: ' + log.read_text())

        def stop(kill=False):
            if process and process.poll() is None:
                process.kill() if kill else process.terminate()
                process.wait(timeout=15)

        def eventually(check, label, timeout=30):
            deadline = time.monotonic() + timeout
            last = None
            while time.monotonic() < deadline:
                assert process.poll() is None, log.read_text()
                assert not fixture_errors, fixture_errors
                last = check()
                if last:
                    return last
                time.sleep(.05)
            raise AssertionError(label + ' timed out: ' + repr(last) + '\n' + log.read_text())

        def called(case):
            return len(model_calls.get(case, []))

        def sent(case):
            return len(platform_calls.get(case, []))

        def payload(case, sender='Alice@Example.COM', message_id=None, content=None):
            return {'robotCode': robot_code, 'chatbotCorpId': corp, 'senderCorpId': corp,
                    'conversationType': '1', 'msgtype': 'text',
                    'msgId': message_id if message_id is not None else base64.b64encode(uuid.uuid4().bytes).decode(),
                    'senderStaffId': sender, 'conversationId': 'raw-conversation-is-not-userid',
                    'text': {'content': 'dingtalk-fixture:' + case if content is None else content},
                    'sessionWebhook': fixture_base + '/forbidden-callback?token=' + callback_secret,
                    'sessionWebhookExpiredTime': int(time.time() * 1000) + 600000}

        def webhook(body, bad_signature=False, stale=False, missing_header=False, *,
                    raw=None, content_types=('application/json',)):
            timestamp = str(int(time.time() * 1000) - (7200000 if stale else 0))
            signature = base64.b64encode(hmac.new(app_secret.encode(),
                (timestamp + '\n' + app_secret).encode(), hashlib.sha256).digest()).decode()
            callback_signatures.append(signature)
            headers = {'timestamp': timestamp, 'sign': 'invalid' if bad_signature else signature}
            if missing_header:
                del headers['sign']
            # http.client preserves both original JSON bytes and repeated MIME
            # headers; dict/json or urllib's header map would erase that input.
            if raw is None:
                raw = json.dumps(body).encode()
            url = urllib.parse.urlsplit(base)
            connection = http.client.HTTPConnection(url.hostname, url.port, timeout=10)
            try:
                connection.putrequest('POST', '/hooks/dingtalk')
                for name, value in headers.items():
                    connection.putheader(name, value)
                for content_type in content_types:
                    connection.putheader('Content-Type', content_type)
                connection.putheader('Content-Length', str(len(raw)))
                connection.endheaders(raw)
                response = connection.getresponse()
                received = response.read()
                if 'application/json' in response.headers.get('Content-Type', '') and received:
                    return response.status, json.loads(received)
                return response.status, received.decode()
            finally:
                connection.close()

        def events():
            status, value = request('/api/channels/events?limit=100')
            assert status == 200, (status, value)
            return value

        def admit(body, **wire):
            ids = {event['id'] for event in events()}
            before = time.monotonic()
            status, value = webhook(body, **wire)
            assert status == 200 and value == '', (status, value)
            # Local regression target, not an undocumented platform deadline.
            assert time.monotonic() - before < 2
            return eventually(lambda: next((event for event in events() if event['id'] not in ids), None),
                              'durable admission')

        def deliveries(event_id=None, run_id=None):
            status, value = request('/api/channels/deliveries?limit=100')
            assert status == 200, (status, value)
            return sorted((item for item in value if (item.get('event_id') == event_id if event_id
                           else item.get('job_run_id') == run_id)), key=lambda item: item['ordinal'])

        def state(event_id, expected):
            items = deliveries(event_id=event_id)
            return items if items and all(item['state'] == expected for item in items) else None

        def delivered(event_id, case, expected_calls=2):
            items = eventually(lambda: state(event_id, 'delivered'), case + ' delivered')
            assert all(item['receipt'] for item in items)
            assert ''.join(item['text'] for item in items) == reply(case)
            assert called(case) == expected_calls
            return items

        def counters():
            with fixture_lock:
                return {'token': len(token_calls), 'send': len(all_sends),
                        'model': sum(len(calls) for calls in model_calls.values())}

        def offline(event_id):
            assert process.poll() is not None, 'SQLite observer requires actual process exit'
            database = root / 'state' / 'sessions.sqlite3'
            assert database.is_file(), 'actual persistent database missing'
            # The process has exited. Noncreating rw allows SQLite's VFS to
            # open WAL facilities on supported builds; SQL remains read-only.
            # Do not use immutable: recovery evidence includes committed WAL.
            with closing(sqlite3.connect(database.as_uri() + '?mode=rw', uri=True)) as conn:
                conn.execute('PRAGMA query_only=ON')
                conn.row_factory = sqlite3.Row
                records = [dict(row) for row in conn.execute(
                    'SELECT id,event_id,state,attempts,receipt,started_ms,finished_ms '
                    'FROM channel_outbox WHERE event_id=? ORDER BY ordinal', (event_id,))]
                cooldown = conn.execute(
                    "SELECT until_ms FROM channel_cooldowns WHERE channel='dingtalk' AND installation_id=?",
                    (installation,)).fetchone()
                return {'deliveries': records, 'cooldown_until_ms': cooldown[0] if cooldown else None}

        def original_delivery(items, expected):
            assert len(items) == 1, 'expected one original standalone delivery'
            item = items[0]
            assert str(uuid.UUID(item['id'])) == item['id'] and uuid.UUID(item['id']).version == 4
            assert item['state'] == expected and item['attempts'] == 1
            return item['id']

        # Fail closed before making an auth/message request. Environment from
        # the developer machine cannot supply a real credential fallback.
        for kind in ['missing_secret', 'bad_installation', 'bad_user', 'thread', 'missing_app_id']:
            rejected = copy.deepcopy(settings)
            binding = rejected['http']['channels'][0]
            if kind == 'missing_secret':
                del rejected['http']['dingtalk_app_secret']
            elif kind == 'bad_installation':
                binding['installation_id'] = robot_code
            elif kind == 'bad_user':
                binding['allowed_senders'] = ['alice|bob']
            elif kind == 'thread':
                binding['scheduled_destinations'] = [{'conversation_id': 'Alice@Example.COM', 'thread_id': 'x'}]
            else:
                del binding['app_id']
            config.write_text(json.dumps(rejected))
            outcome = subprocess.run([str(binary), 'serve', '--config', str(config)], env=env,
                                     capture_output=True, text=True, timeout=10)
            assert outcome.returncode != 0, kind
            assert app_secret not in outcome.stdout + outcome.stderr
        assert token_calls == [] and platform_calls == {}
        start()
        assert request('/api/channels/events', authenticated=False)[0] == 401
        invalid = payload('rejected')
        assert webhook({**invalid, 'padding': 'x' * (128 * 1024)})[0] == 413
        assert 400 <= webhook({**invalid, 'text': {'content': 'x' * (32 * 1024 + 1)}})[0] < 500
        for kwargs in [{'bad_signature': True}, {'stale': True}, {'missing_header': True}]:
            assert 400 <= webhook(invalid, **kwargs)[0] < 500, kwargs
        for field, value in [('robotCode', 'otherRobot'), ('chatbotCorpId', 'otherCorp'),
                             ('senderCorpId', 'otherCorp'), ('senderStaffId', 'someone_else'),
                             ('senderStaffId', 'alice@example.com'), ('senderStaffId', '@all'),
                             ('senderStaffId', 'alice|bob'), ('msgId', '')]:
            assert 400 <= webhook({**invalid, field: value})[0] < 500, (field, value)
        # Unsupported group/non-text events cannot invoke a model or tool.
        for altered in [{**invalid, 'conversationType': '2'}, {**invalid, 'msgtype': 'picture'}]:
            assert webhook(altered)[0] in [200, 400, 401, 403]
        assert model_calls == {} and platform_calls == {} and token_calls == []

        encoded = json.dumps(invalid, separators=(',', ':')).encode()
        # Supply every positional field in the old typed DTO's exact order,
        # with valid content/identities. Rejection must come from wire shape,
        # rather than a missing field or an invalid MAC.
        positional = [invalid['robotCode'], invalid['chatbotCorpId'], invalid['senderCorpId'],
                      invalid['senderStaffId'], invalid['conversationType'], invalid['conversationId'],
                      invalid['msgId'], invalid['msgtype'], invalid['text'], None]
        wire_cases = [
            ('root_array', json.dumps(positional).encode(), ('application/json',)),
            ('text_array', json.dumps({**invalid, 'text': [invalid['text']['content']]}).encode(),
             ('application/json',)),
            ('duplicate_robot', encoded.replace(b'"robotCode":',
             b'"robotCode":' + json.dumps(robot_code).encode() + b',"robotCode":', 1),
             ('application/json',)),
            ('duplicate_msgid', encoded.replace(b'"msgId":', b'"msgId":null,"msgId":', 1),
             ('application/json',)),
            ('duplicate_content', encoded.replace(b'"content":',
             b'"content":"dingtalk-fixture:rejected","content":', 1), ('application/json',)),
            ('mime_text', encoded, ('application/json', 'text/plain')),
            ('mime_json', encoded, ('application/json', 'application/json')),
            ('non_json_mime', encoded, ('text/plain',)),
            ('missing_mime', encoded, ()),
        ]
        before_events, before_effects = events(), counters()
        for name, raw, content_types in wire_cases:
            status, _ = webhook(invalid, raw=raw, content_types=content_types)
            assert status == 401, ('callback wire rejection', name, status)
            assert events() == before_events and counters() == before_effects, ('callback effects', name)
        print('DingTalk original signed root/text objects, duplicates and unique JSON MIME: passed', flush=True)

        token_cases = ['token_array', 'token_duplicate', 'token_duplicate_expiry',
                       'token_mime_text', 'token_mime_json', 'token_code_conflict']
        stop()
        for case in token_cases:
            with fixture_lock:
                token_response_case = case
            start()  # A real new sender has no cached token; no short expiry/clock injection.
            before_effects = counters()
            body = payload(case)
            item_event = admit(body)
            items = eventually(lambda: state(item_event['id'], 'permanent_failed'), case + ' token rejection')
            delivery_id = original_delivery(items, 'permanent_failed')
            assert items[0]['error'] == 'credential_unavailable' and items[0]['receipt'] is None
            assert called(case) == 2 and sent(case) == 0
            assert counters() == {**before_effects, 'token': before_effects['token'] + 1,
                                  'model': before_effects['model'] + 2}
            before_events = events()
            assert webhook(body) == (200, '')
            assert events() == before_events and deliveries(event_id=item_event['id']) == items
            assert len(token_calls) == before_effects['token'] + 1 and sent(case) == 0
            stop()
            persisted_failure = offline(item_event['id'])
            assert persisted_failure['deliveries'][0]['id'] == delivery_id
            assert persisted_failure['deliveries'][0]['state'] == 'permanent_failed'
            assert persisted_failure['deliveries'][0]['attempts'] == 1
            assert persisted_failure['cooldown_until_ms'] >= persisted_failure['deliveries'][0]['finished_ms'] + 4000
            before_restart = counters()
            start()
            assert state(item_event['id'], 'permanent_failed') and counters() == before_restart
            assert request('/api/channels/deliveries/' + delivery_id + '/resolve', 'POST',
                           {'action': 'cancel'})[0] == 204
            stop()
        with fixture_lock:
            token_response_case = 'normal'
        start()
        token_before_ack = len(token_calls)
        print('DingTalk six original token object/duplicate/MIME failures: no message POST or retry; durable UUID/cooldown: passed', flush=True)

        first_body = payload('ack', content='dingtalk-fixture:ack <literal> & plain text')
        first = admit(first_body, content_types=('application/json; charset=utf-8',))
        assert first['spec']['sender_id'] == first['spec']['destination']['conversation_id'] == 'Alice@Example.COM'
        assert first['spec']['destination']['thread_id'] is None
        assert callback_secret not in json.dumps(first)
        assert 'raw-conversation-is-not-userid' not in json.dumps(first)
        eventually(lambda: called('ack') == 2, 'native clock completed, final model blocked')
        count = len(events())
        assert webhook(first_body) == (200, '')
        assert len(events()) == count and called('ack') == 2 and sent('ack') == 0
        changed = {**first_body, 'text': {'content': 'changed-prompt'}}
        assert webhook(changed)[0] == 409
        gate('ack').set()
        delivered(first['id'], 'ack')
        assert len(token_calls) == token_before_ack + 1

        # Independent event IDs with identical text remain distinct local sends.
        same_a = admit(payload('same'))
        delivered(same_a['id'], 'same')
        same_b = admit(payload('same'))
        delivered(same_b['id'], 'same', expected_calls=4)
        assert same_a['id'] != same_b['id'] and sent('same') == 2
        assert platform_calls['same'][0]['body'] == platform_calls['same'][1]['body']
        assert len(token_calls) == token_before_ack + 1

        for case in ['filter_omitted', 'filter_null', 'filter_empty']:
            item_event = admit(payload(case))
            items = delivered(item_event['id'], case)
            original_delivery(items, 'delivered')
            assert sent(case) == 1 and len(token_calls) == token_before_ack + 1

        long_event = admit(payload('long', sender='Long.User'))
        parts = delivered(long_event['id'], 'long')
        assert len(parts) >= 3
        sent_text = ''.join(call['text'] for call in platform_calls['long'])
        assert sent_text == reply('long')
        assert sent('long') == len(parts)
        assert all(b['at'] - a['completed_at'] >= 3.8 for a, b in zip(all_sends, all_sends[1:]))

        send_wire_cases = ['send_array', 'send_duplicate_receipt', 'send_duplicate_filter',
                           'send_mime_text', 'send_mime_json', 'send_code_receipt', 'send_code_empty_receipt']
        for case in ['unknown', 'unknown_client', 'rate_unknown', 'invalid_user',
                     'flow_controlled', 'filtered_user', 'missing_receipt'] + send_wire_cases:
            body = payload(case)
            item_event = admit(body)
            items = eventually(lambda: state(item_event['id'], 'unknown'), case + ' unknown')
            delivery_id = original_delivery(items, 'unknown')
            assert items[0]['receipt'] is None
            if case in send_wire_cases:
                before_events, before_effects = events(), counters()
                assert webhook(body) == (200, '')
                assert events() == before_events and counters() == before_effects
                assert deliveries(event_id=item_event['id']) == items
            if case == 'unknown':
                stop(kill=True)
                start()
                assert state(item_event['id'], 'unknown')
                waiting_event = admit(payload('review_wait'))
                eventually(lambda: state(waiting_event['id'], 'pending'), 'same recipient waits for review')
                other_event = admit(payload('other_user', sender='Long.User'))
                delivered(other_event['id'], 'other_user')
                assert sent('review_wait') == 0
            if case == 'send_array':
                stop()
                stopped = offline(item_event['id'])
                assert stopped['deliveries'][0]['id'] == delivery_id
                assert stopped['deliveries'][0]['state'] == 'unknown'
                assert stopped['deliveries'][0]['attempts'] == 1
                assert stopped['cooldown_until_ms'] >= stopped['deliveries'][0]['finished_ms'] + 4000
                saved_settings = copy.deepcopy(settings)
                saved_env = env.copy()
                settings['http']['channels'] = []
                settings['http'].pop('dingtalk_app_secret', None)
                settings['scheduler']['enabled'] = False
                settings['heartbeat'] = {'enabled': False}
                env.pop('JIACLAW_DINGTALK_APP_SECRET', None)
                before_maintenance = counters()
                start(expected_channel_state='disabled', jobs_enabled=False)
                assert app_secret not in config.read_text() and access_token not in config.read_text()
                status, reviewed = request('/api/channels/deliveries/' + delivery_id)
                assert status == 200 and reviewed == items[0]
                assert request('/hooks/dingtalk', 'POST', {}, authenticated=False)[0] == 404
                time.sleep(.2)
                assert counters() == before_maintenance
                stop()
                assert offline(item_event['id']) == stopped
                settings.clear()
                settings.update(saved_settings)
                env.clear()
                env.update(saved_env)
                start()
                assert state(item_event['id'], 'unknown') and counters() == before_maintenance
                wire_wait = admit(payload('wire_wait'))
                eventually(lambda: state(wire_wait['id'], 'pending'), 'wire unknown blocks same recipient')
                assert called('wire_wait') == 2 and sent('wire_wait') == 0
            time.sleep(.2)
            assert called(case) == 2 and sent(case) == 1
            assert request('/api/channels/deliveries/' + delivery_id + '/resolve', 'POST',
                           {'action': 'cancel'})[0] == 204
            if case == 'unknown':
                delivered(waiting_event['id'], 'review_wait')
            if case == 'send_array':
                delivered(wire_wait['id'], 'wire_wait')
            if case in send_wire_cases:
                cancelled = deliveries(event_id=item_event['id'])
                assert original_delivery(cancelled, 'cancelled') == delivery_id
                assert called(case) == 2 and sent(case) == 1
        print('DingTalk seven original send object/duplicate/MIME/conflicting receipts: one POST and unknown hold; original UUID/cooldown and credential-free stopped maintenance: passed', flush=True)

        submitting = admit(payload('submitting'))
        eventually(lambda: sent('submitting') == 1, 'request received by platform')
        assert state(submitting['id'], 'submitting')
        stop(kill=True)
        gate('submitting').set()
        start()
        items = eventually(lambda: state(submitting['id'], 'unknown'), 'submit recovery')
        assert sent('submitting') == 1 and called('submitting') == 2
        assert request('/api/channels/deliveries/' + items[0]['id'] + '/resolve', 'POST',
                       {'action': 'delivered', 'receipt': 'Fixture operator verified processQueryKey acceptance.'})[0] == 204
        processing = admit(payload('processing'))
        eventually(lambda: called('processing') == 2, 'tool ran before crash')
        stop(kill=True)
        gate('processing').set()
        start()
        recovered = request('/api/channels/events/' + processing['id'])[1]
        assert recovered['status'] == 'needs_review' and deliveries(event_id=processing['id']) == []
        assert called('processing') == 2 and sent('processing') == 0

        destination = {'channel': 'dingtalk', 'installation_id': installation,
                       'conversation_id': 'Alice@Example.COM', 'thread_id': None}

        def spec(case, target=None):
            return {'name': case, 'prompt': 'dingtalk-fixture:' + case,
                    'schedule': {'kind': 'interval', 'seconds': 1},
                    'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                    'delivery': copy.deepcopy(target or destination)}

        assert request('/api/jobs', 'POST', spec('denied'))[0] == 400
        stop()
        settings['http']['channels'][0]['scheduled_destinations'] = [
            {'conversation_id': 'Alice@Example.COM', 'thread_id': None}]
        del settings['http']['dingtalk_app_secret']
        env['JIACLAW_DINGTALK_APP_SECRET'] = app_secret
        start()
        for denied in [{**destination, 'conversation_id': '@all'}, {**destination, 'thread_id': 'thread'},
                       {**destination, 'conversation_id': 'alice|bob'},
                       {**destination, 'conversation_id': 'Long.User'},
                       {**destination, 'installation_id': robot_code + ':otherCorp'},
                       {**destination, 'url': fixture_base}]:
            assert 400 <= request('/api/jobs', 'POST', spec('denied', denied))[0] < 500
        assert request('/api/jobs', 'POST', spec('denied'), authenticated=False)[0] == 401
        assert request('/api/jobs', 'POST', {**spec('denied'), 'enabled_tools': ['json_query']})[0] == 400
        status, job = request('/api/jobs', 'POST', spec('schedule'))
        assert status in [200, 201], (status, job)
        path = '/api/jobs/' + job['id']
        run = eventually(lambda: next((item for item in request(path + '/runs')[1]
                                        if item['status'] == 'completed'), None), 'scheduled completion')
        assert request(path + '/pause', 'POST')[0] == 200
        output = eventually(lambda: next((item for item in deliveries(run_id=run['id'])
                                          if item['state'] == 'delivered'), None), 'scheduled acceptance')
        assert output['job_id'] == job['id'] and output['event_id'] is None
        assert called('schedule') == 2 and sent('schedule') == 1 and output['receipt']
        assert called('denied') == 0 and sent('denied') == 0
        assert request(path + '/runs/' + run['id'] + '/deliveries', 'DELETE')[0] == 204
        public = json.dumps(events()) + json.dumps(request('/api/channels/deliveries?limit=100')[1])
        public += ''.join(path.read_text() for path in logs)
        stop()
        persisted = b''.join(path.read_bytes() for path in (root / 'state').glob('*') if path.is_file())
        for secret in [app_secret, callback_secret, access_token,
                       private_error, gateway_secret, api_token] + callback_signatures:
            assert secret not in public, 'secret or private platform body escaped'
            assert secret.encode() not in persisted, 'credential or callback URL persisted'
        assert not fixture_errors, fixture_errors
        print('DingTalk signed headers, case-preserving identity, durable ACK/dedup and native tools: passed')
        print('DingTalk fixed endpoints/token reuse, UTF-16 split, spacing and unknown/crash recovery: passed')
        print('DingTalk exact scheduled recipients, ignored callback URLs, environment credentials and secret handling: passed')
        print('DingTalk filtered recipient lists omitted/null/empty accepted; nonempty fails closed: passed')
finally:
    if process and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    for pending in gates.values():
        pending.set()
    fixture.shutdown()
    fixture.server_close()
