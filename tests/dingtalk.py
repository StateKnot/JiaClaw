#!/usr/bin/env python3
"""Real binary + disposable localhost DingTalk app and native-model fixtures.

Python 3 standard library only. No live DingTalk credentials, accounts or messages.
"""
import base64
import copy
import hashlib
import hmac
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


def gate(case):
    return gates.setdefault(case, threading.Event())


def reply(case):
    if case == 'long':
        return 'dingtalk-fixture:long ' + '中文😀<>&' * 900
    return 'dingtalk-fixture:' + case + ' completed. <a href="invalid">plain</a> & 中文😀'


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, body):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
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
                return self.respond(200, {'accessToken': access_token, 'expireIn': 7200})
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
            self.respond(200, receipt)
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
                    assert request('/api/channels/status')[1]['state'] == 'running'
                    assert request('/api/jobs/status')[1]['state'] == 'running'
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

        def webhook(body, bad_signature=False, stale=False, missing_header=False):
            timestamp = str(int(time.time() * 1000) - (7200000 if stale else 0))
            signature = base64.b64encode(hmac.new(app_secret.encode(),
                (timestamp + '\n' + app_secret).encode(), hashlib.sha256).digest()).decode()
            callback_signatures.append(signature)
            headers = {'timestamp': timestamp, 'sign': 'invalid' if bad_signature else signature}
            if missing_header:
                del headers['sign']
            return request('/hooks/dingtalk', 'POST', body, authenticated=False, extra_headers=headers)

        def events():
            status, value = request('/api/channels/events?limit=100')
            assert status == 200, (status, value)
            return value

        def admit(body):
            ids = {event['id'] for event in events()}
            before = time.monotonic()
            status, value = webhook(body)
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

        first_body = payload('ack', content='dingtalk-fixture:ack <literal> & plain text')
        first = admit(first_body)
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
        assert len(token_calls) == 1

        # Independent event IDs with identical text remain distinct local sends.
        same_a = admit(payload('same'))
        delivered(same_a['id'], 'same')
        same_b = admit(payload('same'))
        delivered(same_b['id'], 'same', expected_calls=4)
        assert same_a['id'] != same_b['id'] and sent('same') == 2
        assert platform_calls['same'][0]['body'] == platform_calls['same'][1]['body']
        assert len(token_calls) == 1

        long_event = admit(payload('long', sender='Long.User'))
        parts = delivered(long_event['id'], 'long')
        assert len(parts) >= 3
        sent_text = ''.join(call['text'] for call in platform_calls['long'])
        assert sent_text == reply('long')
        assert sent('long') == len(parts)
        assert all(b['at'] - a['completed_at'] >= 3.8 for a, b in zip(all_sends, all_sends[1:]))

        for case in ['unknown', 'unknown_client', 'rate_unknown', 'invalid_user',
                     'flow_controlled', 'filtered_user', 'missing_receipt']:
            item_event = admit(payload(case))
            items = eventually(lambda: state(item_event['id'], 'unknown'), case + ' unknown')
            if case == 'unknown':
                stop(kill=True)
                start()
                assert state(item_event['id'], 'unknown')
                waiting_event = admit(payload('review_wait'))
                eventually(lambda: state(waiting_event['id'], 'pending'), 'same recipient waits for review')
                other_event = admit(payload('other_user', sender='Long.User'))
                delivered(other_event['id'], 'other_user')
                assert sent('review_wait') == 0
            time.sleep(.2)
            assert called(case) == 2 and sent(case) == 1
            assert request('/api/channels/deliveries/' + items[0]['id'] + '/resolve', 'POST',
                           {'action': 'cancel'})[0] == 204
            if case == 'unknown':
                delivered(waiting_event['id'], 'review_wait')

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
finally:
    if process and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    for pending in gates.values():
        pending.set()
    fixture.shutdown()
    fixture.server_close()
