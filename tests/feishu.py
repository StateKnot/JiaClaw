#!/usr/bin/env python3
"""Feishu webhook, native agent and outbox acceptance against the real binary.

Only disposable localhost credentials and HTTP fixtures are used. Requires
Python 3 and OpenSSL AES support; no Python packages or Feishu account.
"""
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import socket
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
api_token = 'fixture-api-' + uuid.uuid4().hex
app_secret = 'fixture-app-' + uuid.uuid4().hex
encrypt_key = 'fixture-encrypt-' + uuid.uuid4().hex
verification_token = 'fixture-verification-' + uuid.uuid4().hex
access_token = 't-fixture-' + uuid.uuid4().hex
private_error = 'PRIVATE-FEISHU-RESPONSE-' + uuid.uuid4().hex
app_id, tenant = 'cli_fixture', 'tenant_fixture'
installation = app_id + ':' + tenant
model_calls, platform_calls, token_calls = {}, {}, []
fixture_errors, gates = [], {}
fixture_lock = threading.Lock()
process, base = None, None


def gate(case):
    return gates.setdefault(case, threading.Event())


def reply(case):
    return 'feishu-fixture:' + case + ' completed. <at user_id="all">everyone</at>'


def encrypted(value, key=encrypt_key):
    iv = os.urandom(16)
    raw = json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()
    ciphertext = subprocess.run(['openssl', 'enc', '-aes-256-cbc', '-K',
                                 hashlib.sha256(key.encode()).hexdigest(), '-iv', iv.hex()],
                                input=raw, capture_output=True, check=True).stdout
    return {'encrypt': base64.b64encode(iv + ciphertext).decode()}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, value, headers=None):
        raw = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/chat/completions':
                return self.model(body)
            if self.path == '/open-apis/auth/v3/tenant_access_token/internal':
                assert body == {'app_id': app_id, 'app_secret': app_secret}, body
                assert not self.headers.get('Authorization')
                token_calls.append(body)
                return self.respond(200, {'code': 0, 'msg': 'ok',
                                          'tenant_access_token': access_token, 'expire': 7200})
            assert self.headers['Authorization'] == 'Bearer ' + access_token
            assert body['msg_type'] == 'text'
            text = json.loads(body['content'])['text']
            assert '<at' not in text and '<' not in text and '>' not in text
            case = re.search(r'feishu-fixture:([a-z_]+)', text).group(1)
            assert text == reply(case).replace('<', '＜').replace('>', '＞'), text
            assert str(uuid.UUID(body['uuid'])) == body['uuid']
            match = re.fullmatch(r'/open-apis/im/v1/messages/(om_[A-Za-z0-9_]+)/reply', self.path)
            if match:
                assert 'receive_id' not in body
                receipt = {'message_id': 'om_receipt_' + uuid.uuid4().hex,
                           'chat_id': 'oc_group', 'msg_type': 'text',
                           'root_id': match.group(1), 'parent_id': match.group(1),
                           'thread_id': 'omt_fixture'}
            else:
                assert self.path == '/open-apis/im/v1/messages?receive_id_type=chat_id', self.path
                assert body['receive_id'] == 'oc_private', body
                receipt = {'message_id': 'om_receipt_' + uuid.uuid4().hex,
                           'chat_id': 'oc_private', 'msg_type': 'text'}
            with fixture_lock:
                platform_calls.setdefault(case, []).append({'body': body, 'path': self.path,
                                                            'at': time.monotonic()})
                attempt = len(platform_calls[case])
            if case in ['rate_standard', 'rate_legacy', 'rate_message'] and attempt == 1:
                status = 429 if case == 'rate_standard' else 400
                code = 230020 if case == 'rate_message' else 99991400
                error = {'code': code, 'msg': private_error}
                if case == 'rate_message':
                    error['data'] = {}
                return self.respond(status, error,
                                    {'x-ogw-ratelimit-reset': '3'})
            if case == 'invalid_retry':
                return self.respond(429, {'code': 99991400, 'msg': private_error},
                                    {'x-ogw-ratelimit-reset': 'tomorrow'})
            if case in ['inflight', 'inflight_legacy']:
                return self.respond(400, {'code': 230049 if case == 'inflight' else 18121,
                                          'msg': private_error, 'data': {}})
            if case == 'unknown':
                return self.respond(502, {'code': 99999999, 'msg': private_error + access_token})
            if case == 'unknown_client':
                return self.respond(400, {'code': 99999999, 'msg': private_error, 'data': {}})
            if case == 'token_expired':
                return self.respond(400, {'code': 99991663, 'msg': private_error, 'data': {}})
            if case == 'bad_receipt':
                receipt['chat_id'] = 'oc_wrong'
            if case == 'conflicting_root':
                receipt['parent_id'] = 'om_wrong_parent'
            if case == 'submitting':
                gate(case).wait(timeout=90)
            self.respond(200, {'code': 0, 'msg': 'success', 'data': receipt})
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
        case = re.search(r'feishu-fixture:([a-z_]+)', prompt).group(1)
        assert {tool['function']['name'] for tool in body['tools']} == {'datetime_now'}
        with fixture_lock:
            model_calls.setdefault(case, []).append(body)
        if body['messages'][-1]['role'] == 'tool':
            assert body['messages'][-1]['tool_call_id'].startswith('clock-')
            assert 'Unix' in body['messages'][-1]['content']
            if case in ['ack', 'processing']:
                gate(case).wait(timeout=90)
            message = {'role': 'assistant', 'content': reply(case)}
            finish = 'stop'
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
    with tempfile.TemporaryDirectory(prefix='jiaclaw-feishu-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        config = root / 'config.json'
        fixture_base = 'http://127.0.0.1:' + str(fixture.server_port)
        settings = {
            'agent': {'name': 'feishu-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': fixture_base,
                         'api_key': gateway_secret, 'model': 'fixture'},
            'scheduler': {'enabled': True},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1,
                     'feishu_app_secret': app_secret, 'feishu_encrypt_key': encrypt_key,
                     'feishu_verification_token': verification_token,
                     'channels': [{'channel': 'feishu', 'installation_id': installation,
                                   'allowed_senders': ['ou_fixture'],
                                   'allowed_conversations': ['oc_private', 'oc_group'],
                                   'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                                   'local_test_api_base': fixture_base + '/open-apis'}]},
        }
        env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
        env['JIACLAW_LOG_LEVEL'] = 'info'
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
                    assert request('/api/channels/status')[1]['state'] == 'running'
                    assert request('/api/jobs/status')[1]['state'] == 'running'
                    return
                time.sleep(.05)
            raise AssertionError('host startup timed out: ' + log.read_text())

        def stop(kill=False):
            if process and process.poll() is None:
                process.kill() if kill else process.terminate()
                process.wait(timeout=15)

        def eventually(check, description, timeout=20):
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

        def called(case):
            return len(model_calls.get(case, []))

        def sent(case):
            return len(platform_calls.get(case, []))

        def payload(case, group=False, parent=None):
            message = {'message_id': 'om_' + uuid.uuid4().hex,
                       'create_time': str(int(time.time() * 1000)),
                       'chat_id': 'oc_group' if group else 'oc_private',
                       'chat_type': 'group' if group else 'p2p', 'message_type': 'text',
                       'content': json.dumps({'text': 'feishu-fixture:' + case})}
            if parent:
                message.update(root_id=parent, parent_id=parent, thread_id='omt_fixture')
            return {'schema': '2.0', 'header': {
                'event_id': 'ev_' + uuid.uuid4().hex, 'event_type': 'im.message.receive_v1',
                'app_id': app_id, 'tenant_key': tenant, 'token': verification_token,
                'create_time': str(int(time.time() * 1000)),
            }, 'event': {'sender': {'sender_id': {'open_id': 'ou_fixture'},
                                    'sender_type': 'user', 'tenant_key': tenant},
                         'message': message}}

        def webhook(body, bad_signature=False, stale=False, plaintext=False):
            envelope = body if plaintext else encrypted(body)
            raw = json.dumps(envelope, separators=(',', ':')).encode()
            timestamp = str(int(time.time()) - (601 if stale else 0))
            nonce = uuid.uuid4().hex
            digest = hashlib.sha256((timestamp + nonce + encrypt_key).encode() + raw).hexdigest()
            return request('/hooks/feishu', 'POST', authenticated=False, raw=raw, headers={
                'X-Lark-Request-Timestamp': timestamp, 'X-Lark-Request-Nonce': nonce,
                'X-Lark-Signature': '0' * 64 if bad_signature else digest,
            })

        def admit(body):
            status, value = webhook(body)
            assert status == 200 and value['ok'] and value['duplicate'] is False, (status, value)
            return value['event_id']

        def event(event_id):
            status, value = request('/api/channels/events/' + event_id)
            assert status == 200, (status, value)
            return value

        def deliveries(event_id=None, run_id=None):
            status, value = request('/api/channels/deliveries?limit=100')
            assert status == 200, (status, value)
            return [item for item in value if (item.get('event_id') == event_id if event_id
                                               else item.get('job_run_id') == run_id)]

        def delivery_state(event_id, state):
            items = deliveries(event_id=event_id)
            return items[0] if len(items) == 1 and items[0]['state'] == state else None

        def delivered(event_id, case):
            item = eventually(lambda: delivery_state(event_id, 'delivered'), case + ' delivered')
            assert item['receipt'] and item['text'] == reply(case)
            assert platform_calls[case][-1]['body']['uuid'] == item['id']
            assert event(event_id)['status'] == 'completed' and called(case) == 2
            status, session = request('/api/sessions/' + event(event_id)['spec']['session_id'])
            assert status == 200 and session['messages'][-1]['content'] == reply(case)
            return item

        # Independent official AES example, not generated by our fixture or the
        # host: Feishu's event-decryption guide publishes this known plaintext.
        vector = base64.b64decode('P37w+VZImNgPEO1RBhJ6RtKl7n6zymIbEG1pReEzghk=')
        decoded = subprocess.run(['openssl', 'enc', '-d', '-aes-256-cbc', '-K',
                                  hashlib.sha256(b'test key').hexdigest(), '-iv', vector[:16].hex()],
                                 input=vector[16:], capture_output=True, check=True).stdout
        assert decoded == b'hello world'
        for kind in ['missing_secret', 'slack_app_id', 'missing_tenant', 'remote_override']:
            rejected = copy.deepcopy(settings)
            binding = rejected['http']['channels'][0]
            if kind == 'missing_secret':
                del rejected['http']['feishu_app_secret']
            elif kind == 'slack_app_id':
                binding['app_id'] = app_id
            elif kind == 'missing_tenant':
                binding['installation_id'] = app_id
            else:
                binding['local_test_api_base'] = 'https://example.invalid/open-apis'
            config.write_text(json.dumps(rejected))
            outcome = subprocess.run([str(binary), 'serve', '--config', str(config)], env=env,
                                     capture_output=True, text=True, timeout=10)
            assert outcome.returncode != 0, kind
            assert app_secret not in outcome.stdout + outcome.stderr
        assert token_calls == [] and model_calls == {} and platform_calls == {}
        start()
        challenge = {'type': 'url_verification', 'token': verification_token,
                     'challenge': 'fixture-' + uuid.uuid4().hex}
        before = time.monotonic()
        status, value = request('/hooks/feishu', 'POST', encrypted(challenge), authenticated=False)
        assert status == 200 and value == {'challenge': challenge['challenge']}, (status, value)
        assert time.monotonic() - before < 1.0
        bad_challenge = {**challenge, 'token': 'wrong'}
        assert 400 <= request('/hooks/feishu', 'POST', encrypted(bad_challenge), authenticated=False)[0] < 500
        assert request('/api/channels/events', authenticated=False)[0] == 401

        invalid = payload('rejected')
        for flags in [{'bad_signature': True}, {'stale': True}]:
            assert 400 <= webhook(invalid, **flags)[0] < 500, flags
        assert 400 <= request('/hooks/feishu', 'POST', encrypted(invalid), authenticated=False)[0] < 500
        assert 400 <= webhook({'encrypt': 'not-base64'}, plaintext=True)[0] < 500
        for path, value in [(('header', 'tenant_key'), 'other_tenant'),
                            (('header', 'app_id'), 'cli_other'),
                            (('header', 'token'), 'wrong'),
                            (('event', 'sender', 'tenant_key'), 'other_tenant'),
                            (('event', 'sender', 'sender_id', 'open_id'), 'ou_other'),
                            (('event', 'message', 'chat_id'), 'oc_other')]:
            rejected = copy.deepcopy(invalid)
            node = rejected
            for part in path[:-1]:
                node = node[part]
            node[path[-1]] = value
            status, value = webhook(rejected)
            assert 400 <= status < 500, (path, status, value)
        assert model_calls == {} and platform_calls == {} and token_calls == []

        for kind in ['bot', 'image']:
            ignored = payload('ignored')
            if kind == 'bot':
                ignored['event']['sender']['sender_type'] = 'app'
            else:
                ignored['event']['message']['message_type'] = 'image'
            assert webhook(ignored)[0] == 200
        assert model_calls == {} and platform_calls == {}

        # ACK must not wait for either model turn. A changed delivery event_id
        # cannot execute the same Feishu message a second time.
        ack = payload('ack')
        ack['event']['message'].update(root_id='', thread_id='omt_ignored_private')
        before = time.monotonic()
        ack_id = admit(ack)
        assert time.monotonic() - before < 3
        eventually(lambda: called('ack') == 2, 'native tool roundtrip waiting')
        duplicate = copy.deepcopy(ack)
        duplicate['header']['event_id'] = 'ev_' + uuid.uuid4().hex
        status, value = webhook(duplicate)
        assert status == 200 and value['duplicate'] and value['event_id'] == ack_id, (status, value)
        assert sent('ack') == 0 and called('ack') == 2
        gate('ack').set()
        delivered(ack_id, 'ack')
        assert event(ack_id)['spec']['destination']['thread_id'] is None
        assert len(token_calls) == 1

        # Encrypt Key also authenticates signed plaintext callbacks; encryption
        # is optional, but a correct signature and token are never optional.
        status, plain = webhook(payload('plaintext'), plaintext=True)
        assert status == 200 and plain['duplicate'] is False, (status, plain)
        delivered(plain['event_id'], 'plaintext')

        group = payload('group', group=True)
        group['event']['message'].update(root_id='', thread_id='omt_existing_topic')
        group_id = admit(group)
        delivered(group_id, 'group')
        target = group['event']['message']['message_id']
        assert event(group_id)['spec']['destination']['thread_id'] == target
        assert platform_calls['group'][0]['path'].endswith('/' + target + '/reply')
        child = payload('group_child', group=True, parent=target)
        child_id = admit(child)
        delivered(child_id, 'group_child')
        assert platform_calls['group_child'][0]['path'].endswith('/' + target + '/reply')
        assert event(child_id)['spec']['session_id'] == event(group_id)['spec']['session_id']
        assert len(token_calls) == 1, 'valid tenant token must be reused across sends'

        # Persist the official relative reset deadline and stable delivery UUID.
        rate_id = admit(payload('rate_standard'))
        waiting = eventually(lambda: delivery_state(rate_id, 'retry_wait'), '429 persisted')
        assert waiting['attempts'] == 1 and waiting['next_attempt_ms'] is not None
        first_at = platform_calls['rate_standard'][0]['at']
        stop(kill=True)
        start()
        item = delivered(rate_id, 'rate_standard')
        assert item['attempts'] == 2 and sent('rate_standard') == 2
        assert platform_calls['rate_standard'][1]['at'] - first_at >= 2.9
        assert {call['body']['uuid'] for call in platform_calls['rate_standard']} == {item['id']}
        for case in ['rate_legacy', 'rate_message']:
            event_id = admit(payload(case))
            item = delivered(event_id, case)
            assert item['attempts'] == 2 and sent(case) == 2
            assert platform_calls[case][1]['at'] - platform_calls[case][0]['at'] >= 2.9

        # A verified token rejection may invalidate the cache for a future
        # independent message, but must not refresh/resend this message.
        token_count = len(token_calls)
        expired_id = admit(payload('token_expired'))
        item = eventually(lambda: delivery_state(expired_id, 'permanent_failed'), 'token rejected')
        retry_ready = time.monotonic() + 30.2
        assert called('token_expired') == 2 and sent('token_expired') == 1
        assert len(token_calls) == token_count
        assert request('/api/channels/deliveries/' + item['id'] + '/resolve', 'POST',
                       {'action': 'cancel'})[0] == 204
        backoff_id = admit(payload('token_backoff'))
        backoff = eventually(lambda: delivery_state(backoff_id, 'permanent_failed'), 'credential backoff')
        assert called('token_backoff') == 2 and sent('token_backoff') == 0
        assert len(token_calls) == token_count
        assert request('/api/channels/deliveries/' + backoff['id'] + '/resolve', 'POST',
                       {'action': 'cancel'})[0] == 204
        # This is the configured auth-failure cooldown, not a timing shortcut or
        # a fixture-only clock/database override in production.
        time.sleep(max(0, retry_ready - time.monotonic()))
        fresh_id = admit(payload('after_token'))
        delivered(fresh_id, 'after_token')
        assert len(token_calls) == token_count + 1 and sent('token_expired') == 1

        # The one-hour platform UUID promise does not justify replaying unknown
        # effects. Bad reset headers, in-flight business responses, 5xx and
        # mismatched receipts all require explicit resolution.
        for case in ['unknown', 'unknown_client', 'inflight', 'inflight_legacy',
                     'invalid_retry', 'bad_receipt', 'conflicting_root']:
            event_id = admit(payload(case, group=case == 'conflicting_root'))
            item = eventually(lambda: delivery_state(event_id, 'unknown'), case + ' unknown')
            if case == 'unknown':
                stop(kill=True)
                start()
            time.sleep(.4)
            assert called(case) == 2 and sent(case) == 1
            assert request('/api/channels/deliveries/' + item['id'] + '/resolve', 'POST',
                           {'action': 'cancel'})[0] == 204
            assert delivery_state(event_id, 'cancelled')

        # A process crash while the platform has received a request is unknown,
        # even if its eventual response would have contained a valid receipt.
        submitting_id = admit(payload('submitting'))
        eventually(lambda: sent('submitting') == 1, 'platform received submit')
        assert delivery_state(submitting_id, 'submitting')
        stop(kill=True)
        gate('submitting').set()
        start()
        item = eventually(lambda: delivery_state(submitting_id, 'unknown'), 'submit recovered')
        assert called('submitting') == 2 and sent('submitting') == 1
        assert request('/api/channels/deliveries/' + item['id'] + '/resolve', 'POST',
                       {'action': 'delivered', 'receipt': 'Fixture operator verified message.'})[0] == 204

        # A crashed agent must not replay a potentially effectful tool turn.
        processing_id = admit(payload('processing', group=True))
        eventually(lambda: called('processing') == 2, 'processing native roundtrip')
        stop(kill=True)
        gate('processing').set()
        start()
        eventually(lambda: event(processing_id)['status'] == 'needs_review', 'processing recovered')
        assert deliveries(event_id=processing_id) == []
        assert called('processing') == 2 and sent('processing') == 0

        destination = {'channel': 'feishu', 'installation_id': installation,
                       'conversation_id': 'oc_private', 'thread_id': None}

        def spec(case, target=None, cron=False):
            return {'name': case, 'prompt': 'feishu-fixture:' + case,
                    'schedule': ({'kind': 'cron', 'expression': '0 0 1 1 *', 'timezone': 'Asia/Shanghai'}
                                 if cron else {'kind': 'interval', 'seconds': 1}),
                    'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                    'delivery': copy.deepcopy(target or destination)}

        assert request('/api/jobs', 'POST', spec('denied'))[0] == 400
        stop()
        settings['http']['channels'][0]['scheduled_destinations'] = [
            {'conversation_id': 'oc_private', 'thread_id': None},
            {'conversation_id': 'oc_group', 'thread_id': target},
        ]
        # Exercise the documented environment-secret path with the same
        # disposable installation instead of ever printing secret values.
        for key, value in [('APP_SECRET', app_secret), ('ENCRYPT_KEY', encrypt_key),
                           ('VERIFICATION_TOKEN', verification_token)]:
            del settings['http']['feishu_' + key.lower()]
            env['JIACLAW_FEISHU_' + key] = value
        start()
        group_destination = {**destination, 'conversation_id': 'oc_group', 'thread_id': target}
        for denied in [{**destination, 'installation_id': 'cli_other:tenant_fixture'},
                       {**destination, 'conversation_id': 'oc_other'},
                       {**group_destination, 'thread_id': 'omt_fixture'},
                       {**group_destination, 'thread_id': 'om_not_allowed'},
                       {**group_destination, 'thread_id': None},
                       {**destination, 'token': access_token}]:
            assert 400 <= request('/api/jobs', 'POST', spec('denied', denied))[0] < 500
        assert request('/api/jobs', 'POST', {**spec('denied'), 'enabled_tools': ['json_query']})[0] == 400
        assert request('/api/jobs', 'POST', spec('denied'), authenticated=False)[0] == 401
        status, cron = request('/api/jobs', 'POST', spec('cron', group_destination, cron=True))
        assert status in [200, 201] and cron['spec']['delivery'] == group_destination, (status, cron)
        assert request('/api/jobs/' + cron['id'] + '/pause', 'POST')[0] == 200
        for case, dest in [('schedule', destination), ('schedule_thread', group_destination)]:
            status, job = request('/api/jobs', 'POST', spec(case, dest))
            assert status in [200, 201], (status, job)
            path = '/api/jobs/' + job['id']
            run = eventually(lambda: next((run for run in request(path + '/runs')[1]
                                            if run['status'] == 'completed'), None), case + ' completed')
            assert request(path + '/pause', 'POST')[0] == 200
            item = eventually(lambda: next((item for item in deliveries(run_id=run['id'])
                                             if item['state'] == 'delivered'), None), case + ' delivered')
            assert item['event_id'] is None and item['job_id'] == job['id']
            assert called(case) == 2 and sent(case) == 1 and item['receipt']
            assert platform_calls[case][0]['body']['uuid'] == item['id']
        assert called('denied') == 0 and sent('denied') == 0 and called('cron') == 0
        public = json.dumps(request('/api/channels/events?limit=100')[1])
        public += json.dumps(request('/api/channels/deliveries?limit=100')[1])
        public += ''.join(path.read_text() for path in logs)
        for secret in [app_secret, encrypt_key, verification_token, access_token, private_error,
                       gateway_secret, api_token]:
            assert secret not in public, 'secret or private platform response escaped'
        assert not fixture_errors, fixture_errors
        stop()
        print('Feishu encrypted challenge/signatures/identity/dedup/native tools/thread/token reuse: passed')
        print('Feishu rate headers/persistent cooldown/UUID/unknown/crash recovery: passed')
        print('Feishu scheduled exact destinations/cron admission/environment secrets: passed')
finally:
    if process and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    for pending in gates.values():
        pending.set()
    fixture.shutdown()
    fixture.server_close()
