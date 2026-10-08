#!/usr/bin/env python3
"""Real binary + disposable localhost WeCom app and native-model fixtures.

Python 3 and OpenSSL only. No live WeCom credentials, accounts or messages.
"""
import base64
import copy
from contextlib import closing
import hashlib
import html
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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
import urllib.parse
import urllib.request
import uuid


binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
gateway_secret, api_token = 'fixture-gateway-' + uuid.uuid4().hex, 'fixture-api-' + uuid.uuid4().hex
app_secret, callback_token = 'fixture-app-' + uuid.uuid4().hex, uuid.uuid4().hex
aes_key = os.urandom(32)
encoding_aes_key = base64.b64encode(aes_key).decode().rstrip('=')
access_token, private_error = 'fixture-token-' + uuid.uuid4().hex, 'PRIVATE-WECOM-' + uuid.uuid4().hex
corp, agent_id = 'wwfixturecorp', 1000002
installation = corp + ':' + str(agent_id)
model_calls, platform_calls, token_calls, agent_calls, gates = {}, {}, [], [], {}
fixture_errors, all_sends = [], []
fixture_lock = threading.Lock()
process, base, counter = None, None, 100000000000000000


def gate(case):
    return gates.setdefault(case, threading.Event())


def reply(case):
    if case == 'long':
        return 'wecom-fixture:long ' + '中文😀<>&' * 500
    return 'wecom-fixture:' + case + ' completed. <a href="invalid">plain</a> & 中文😀'


def encrypted(message, receive_id=corp):
    raw = message.encode()
    plaintext = os.urandom(16) + struct.pack('!I', len(raw)) + raw + receive_id.encode()
    pad = 32 - len(plaintext) % 32
    plaintext += bytes([pad]) * pad
    ciphertext = subprocess.run(['openssl', 'enc', '-aes-256-cbc', '-nopad',
                                 '-K', aes_key.hex(), '-iv', aes_key[:16].hex()],
                                input=plaintext, capture_output=True, check=True).stdout
    return base64.b64encode(ciphertext).decode()


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
        try:
            parsed = urllib.parse.urlsplit(self.path)
            assert not self.headers.get('Authorization')
            if parsed.path == '/cgi-bin/gettoken':
                assert urllib.parse.parse_qs(parsed.query) == {'corpid': [corp], 'corpsecret': [app_secret]}
                token_calls.append(time.monotonic())
                return self.respond(200, {'errcode': 0, 'errmsg': 'ok', 'access_token': access_token,
                                         'expires_in': 7200})
            assert parsed.path == '/cgi-bin/agent/get'
            assert urllib.parse.parse_qs(parsed.query) == {'access_token': [access_token],
                                                         'agentid': [str(agent_id)]}
            agent_calls.append(time.monotonic())
            self.respond(200, {'errcode': 0, 'errmsg': 'ok', 'agentid': agent_id, 'close': 0,
                              'allow_userinfos': {'user': [{'userid': 'ALIce@Example.COM'},
                                                          {'userid': 'Long.User'}]}})
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/chat/completions':
                return self.model(body)
            parsed = urllib.parse.urlsplit(self.path)
            assert parsed.path == '/cgi-bin/message/send'
            assert urllib.parse.parse_qs(parsed.query) == {'access_token': [access_token]}
            assert not self.headers.get('Authorization')
            assert body['agentid'] == agent_id and isinstance(body['agentid'], int)
            assert body['msgtype'] == 'text'
            assert body['enable_duplicate_check'] == 0 and body['enable_id_trans'] == 0
            assert body['safe'] == 0
            assert set(body) == {'touser', 'agentid', 'msgtype', 'text', 'safe',
                                 'enable_duplicate_check', 'enable_id_trans'}, body
            assert body['touser'] in ['alice@example.com', 'long.user']
            text = body['text']['content']
            assert 0 < len(text.encode()) <= 2048 and '<' not in text and '>' not in text
            case = ('long' if body['touser'] == 'long.user' else
                    re.search(r'wecom-fixture:([a-z_]+)', text).group(1))
            with fixture_lock:
                record = {'body': body, 'text': text, 'at': time.monotonic()}
                platform_calls.setdefault(case, []).append(record)
                all_sends.append(record)
            if case == 'unknown':
                return self.respond(502, {'errcode': -1, 'errmsg': private_error + access_token})
            if case == 'unknown_client':
                return self.respond(400, {'errcode': 99999999, 'errmsg': private_error})
            if case == 'rate_unknown':
                return self.respond(429, {'errcode': 45009, 'errmsg': private_error})
            receipt = {'errcode': 0, 'errmsg': 'ok', 'msgid': 'fixture-' + uuid.uuid4().hex,
                       'invaliduser': '', 'invalidparty': '', 'invalidtag': '', 'unlicenseduser': ''}
            if case == 'invalid_user':
                receipt['invaliduser'] = body['touser']
            if case == 'submitting':
                gate(case).wait(timeout=90)
            self.respond(200, receipt)
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
        case = re.search(r'wecom-fixture:([a-z_]+)', prompt).group(1)
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
    with tempfile.TemporaryDirectory(prefix='jiaclaw-wecom-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        config = root / 'config.json'
        fixture_base = 'http://127.0.0.1:' + str(fixture.server_port)
        settings = {
            'agent': {'name': 'wecom-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': fixture_base,
                         'api_key': gateway_secret, 'model': 'fixture'},
            'scheduler': {'enabled': True},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1,
                     'wecom_app_secret': app_secret, 'wecom_callback_token': callback_token,
                     'wecom_encoding_aes_key': encoding_aes_key,
                     'channels': [{'channel': 'wecom', 'installation_id': installation,
                                   'allowed_senders': ['alice@example.com', 'long.user'],
                                   'allowed_conversations': ['alice@example.com', 'long.user'],
                                   'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                                   'local_test_api_base': fixture_base + '/cgi-bin'}]},
        }
        env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
        env['JIACLAW_LOG_LEVEL'] = 'info'
        logs = []

        def request(path, method='GET', body=None, authenticated=True, xml=None):
            headers = {'Authorization': 'Bearer ' + api_token} if authenticated else {}
            data = None
            if xml is not None:
                data = xml.encode()
                headers['Content-Type'] = 'application/xml'
            elif body is not None:
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

        def start(expected_channel_state='running', jobs_disabled=False):
            global process, base, log
            before_token, before_agent = len(token_calls), len(agent_calls)
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
                    status, jobs = request('/api/jobs/status')
                    if jobs_disabled:
                        # Existing scheduler capability contract: disabled
                        # management routes are absent, not a disabled DTO.
                        assert status == 404, (status, jobs)
                    else:
                        assert status == 200 and jobs['state'] == 'running', (status, jobs)
                    proofs = 0 if expected_channel_state == 'disabled' else 1
                    assert len(token_calls) == before_token + proofs
                    assert len(agent_calls) == before_agent + proofs
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

        def reservations():
            # Inspect the real quota ledger without synthesizing or changing
            # any reservations. NULL settlement retains unknown external work.
            assert process is None or process.poll() is not None, 'raw SQLite observations require a stopped process'
            with closing(sqlite3.connect((root / 'state' / 'sessions.sqlite3').as_uri() + '?mode=rw',
                                         uri=True)) as connection:
                connection.execute('PRAGMA query_only=ON')
                return connection.execute('SELECT * FROM wecom_send_reservations '
                                          'ORDER BY installation_id,delivery_id,attempt').fetchall()

        def external_counts():
            return (len(token_calls), len(agent_calls),
                    {name: len(calls) for name, calls in model_calls.items()},
                    {name: len(calls) for name, calls in platform_calls.items()})

        def payload(case, sender='ALIce@Example.COM', inner_corp=corp, inner_agent=agent_id,
                    message_id=None, content=None):
            global counter
            counter += 1
            msgid = str(counter) if message_id is None else message_id
            prompt = 'wecom-fixture:' + case if content is None else content
            return ('<xml><ToUserName>' + html.escape(inner_corp) + '</ToUserName>'
                    '<FromUserName>' + html.escape(sender) + '</FromUserName>'
                    '<CreateTime>' + str(int(time.time())) + '</CreateTime><MsgType>text</MsgType>'
                    '<Content>' + html.escape(prompt) + '</Content><MsgId>' + msgid + '</MsgId>'
                    '<AgentID>' + str(inner_agent) + '</AgentID></xml>')

        def callback_path(ciphertext, challenge=False, bad_signature=False, stale=False):
            timestamp = str(int(time.time()) - (601 if stale else 0))
            nonce = uuid.uuid4().hex
            signature = hashlib.sha1(''.join(sorted([callback_token, timestamp, nonce, ciphertext])).encode()).hexdigest()
            query = {'msg_signature': '0' * 40 if bad_signature else signature,
                     'timestamp': timestamp, 'nonce': nonce}
            if challenge:
                query['echostr'] = ciphertext
            return '/hooks/wecom?' + urllib.parse.urlencode(query)

        def webhook(plaintext, receive_id=corp, outer_corp=corp, outer_agent=agent_id,
                    bad_signature=False, stale=False, duplicate_query=False):
            ciphertext = encrypted(plaintext, receive_id)
            path = callback_path(ciphertext, bad_signature=bad_signature, stale=stale)
            if duplicate_query:
                path += '&nonce=duplicate'
            outer = ('<xml><ToUserName>' + outer_corp + '</ToUserName><AgentID>' + str(outer_agent) +
                     '</AgentID><Encrypt><![CDATA[' + ciphertext + ']]></Encrypt></xml>')
            return request(path, 'POST', authenticated=False, xml=outer)

        def events():
            status, value = request('/api/channels/events?limit=100')
            assert status == 200, (status, value)
            return value

        def admit(plaintext):
            ids = {event['id'] for event in events()}
            before = time.monotonic()
            status, value = webhook(plaintext)
            assert status == 200 and value == '', (status, value)
            assert time.monotonic() - before < 5
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
        for kind in ['missing_secret', 'bad_agent', 'broadcast_user', 'thread', 'app_id']:
            rejected = copy.deepcopy(settings)
            binding = rejected['http']['channels'][0]
            if kind == 'missing_secret':
                del rejected['http']['wecom_app_secret']
            elif kind == 'bad_agent':
                binding['installation_id'] = corp + ':0'
            elif kind == 'broadcast_user':
                binding['allowed_senders'] = ['@all']
            elif kind == 'thread':
                binding['scheduled_destinations'] = [{'conversation_id': 'alice@example.com', 'thread_id': 'x'}]
            else:
                binding['app_id'] = 'not-wecom'
            config.write_text(json.dumps(rejected))
            outcome = subprocess.run([str(binary), 'serve', '--config', str(config)], env=env,
                                     capture_output=True, text=True, timeout=10)
            assert outcome.returncode != 0, kind
            assert app_secret not in outcome.stdout + outcome.stderr
        start()
        echo = 'fixture-challenge-+&-' + uuid.uuid4().hex
        ciphertext = encrypted(echo)
        before = time.monotonic()
        assert request(callback_path(ciphertext, challenge=True), authenticated=False) == (200, echo)
        assert time.monotonic() - before < 1
        for kwargs in [{'bad_signature': True}, {'stale': True}]:
            assert 400 <= request(callback_path(ciphertext, challenge=True, **kwargs), authenticated=False)[0] < 500
        assert 400 <= request(callback_path(encrypted(echo, 'wwother'), challenge=True), authenticated=False)[0] < 500
        assert request('/api/channels/events', authenticated=False)[0] == 401

        invalid = payload('rejected')
        for kwargs in [{'bad_signature': True}, {'stale': True}, {'receive_id': 'wwother'},
                       {'outer_corp': 'wwother'}, {'outer_agent': agent_id + 1}, {'duplicate_query': True}]:
            assert 400 <= webhook(invalid, **kwargs)[0] < 500, kwargs
        for invalid in [payload('rejected', inner_corp='wwother'), payload('rejected', inner_agent=agent_id + 1),
                        payload('rejected', sender='someone_else'), payload('rejected', sender='@all'),
                        payload('rejected', sender='alice|bob'), payload('rejected', message_id='')]:
            assert 400 <= webhook(invalid)[0] < 500
        entity = '<!DOCTYPE xml [<!ENTITY leak SYSTEM "file:///etc/passwd">]>' + payload('rejected')
        entity = entity.replace('wecom-fixture:rejected', '&leak;')
        assert 400 <= webhook(entity)[0] < 500
        assert model_calls == {} and platform_calls == {}
        assert len(token_calls) == len(agent_calls) == 1

        first_xml = payload('ack', content='wecom-fixture:ack <literal> & normal XML entities')
        first = admit(first_xml)
        assert first['spec']['sender_id'] == first['spec']['destination']['conversation_id'] == 'alice@example.com'
        assert first['spec']['destination']['thread_id'] is None
        eventually(lambda: called('ack') == 2, 'native clock completed, final model blocked')
        count = len(events())
        duplicate = first_xml.replace('ALIce@Example.COM', 'alice@example.com')
        assert webhook(duplicate) == (200, '')
        assert len(events()) == count and called('ack') == 2 and sent('ack') == 0
        assert webhook(duplicate.replace('wecom-fixture:ack', 'changed-prompt'))[0] == 409
        gate('ack').set()
        delivered(first['id'], 'ack')
        assert len(token_calls) == 1

        # Independent messages with the same text must not be collapsed by
        # WeCom's content-based duplicate suppression option.
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
        assert sent_text == reply('long').replace('<', '＜').replace('>', '＞')
        assert sent('long') == len(parts)
        assert all(b['at'] - a['at'] >= 3.8 for a, b in zip(all_sends, all_sends[1:]))

        for case in ['unknown', 'unknown_client', 'rate_unknown', 'invalid_user']:
            item_event = admit(payload(case))
            items = eventually(lambda: state(item_event['id'], 'unknown'), case + ' unknown')
            if case == 'unknown':
                stop(kill=True)
                retained_reservations = reservations()
                unknown_ids = {item['id'] for item in items}
                assert any(row[1] in unknown_ids and row[4] is None for row in retained_reservations)
                saved_settings, saved_env = copy.deepcopy(settings), env.copy()
                before_maintenance = external_counts()
                try:
                    settings['http']['channels'] = []
                    for credential in ['wecom_app_secret', 'wecom_callback_token', 'wecom_encoding_aes_key']:
                        settings['http'].pop(credential, None)
                    for credential in list(env):
                        if credential.startswith('JIACLAW_WECOM_'):
                            del env[credential]
                    settings['scheduler']['enabled'] = False
                    settings['heartbeat'] = {'enabled': False}
                    start(expected_channel_state='disabled', jobs_disabled=True)
                    # This is the actual credential-free maintenance startup
                    # for a now-invalid installation, using its original DB.
                    assert request('/api/channels/events/' + item_event['id'])[0] == 200
                    assert state(item_event['id'], 'unknown') == items
                    time.sleep(1)
                    assert external_counts() == before_maintenance
                    assert state(item_event['id'], 'unknown') == items
                    stop()
                    assert external_counts() == before_maintenance
                    assert reservations() == retained_reservations
                finally:
                    stop()
                    settings, env = saved_settings, saved_env
                start()
                assert state(item_event['id'], 'unknown')
                # A prior request can still complete remotely after a local
                # timeout/crash. Hold the whole installation, including other
                # members, until explicit reconciliation establishes a boundary.
                review_wait = admit(payload('review_wait', sender='Long.User'))
                waiting = eventually(lambda: next((item for item in deliveries(event_id=review_wait['id'])
                    if item['state'] == 'pending' and item['error'] == 'wecom_delivery_review_required'), None),
                    'unknown installation blocks another member')
                assert waiting['attempts'] == 0 and sent('review_wait') == 0
            time.sleep(.2)
            assert called(case) == 2 and sent(case) == 1
            assert request('/api/channels/deliveries/' + items[0]['id'] + '/resolve', 'POST',
                           {'action': 'cancel'})[0] == 204
            if case == 'unknown':
                delivered(review_wait['id'], 'review_wait')

        submitting = admit(payload('submitting'))
        eventually(lambda: sent('submitting') == 1, 'request received by platform')
        assert state(submitting['id'], 'submitting')
        stop(kill=True)
        gate('submitting').set()
        start()
        items = eventually(lambda: state(submitting['id'], 'unknown'), 'submit recovery')
        assert sent('submitting') == 1 and called('submitting') == 2
        assert request('/api/channels/deliveries/' + items[0]['id'] + '/resolve', 'POST',
                       {'action': 'delivered', 'receipt': 'Fixture operator verified acceptance msgid.'})[0] == 204
        processing = admit(payload('processing'))
        eventually(lambda: called('processing') == 2, 'tool ran before crash')
        stop(kill=True)
        gate('processing').set()
        start()
        recovered = request('/api/channels/events/' + processing['id'])[1]
        assert recovered['status'] == 'needs_review' and deliveries(event_id=processing['id']) == []
        assert called('processing') == 2 and sent('processing') == 0

        destination = {'channel': 'wecom', 'installation_id': installation,
                       'conversation_id': 'alice@example.com', 'thread_id': None}

        def spec(case, target=None):
            return {'name': case, 'prompt': 'wecom-fixture:' + case,
                    'schedule': {'kind': 'interval', 'seconds': 1},
                    'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                    'delivery': copy.deepcopy(target or destination)}

        assert request('/api/jobs', 'POST', spec('denied'))[0] == 400
        stop()
        settings['http']['channels'][0]['scheduled_destinations'] = [
            {'conversation_id': 'alice@example.com', 'thread_id': None}]
        for name, value in [('APP_SECRET', app_secret), ('CALLBACK_TOKEN', callback_token),
                            ('ENCODING_AES_KEY', encoding_aes_key)]:
            del settings['http']['wecom_' + name.lower()]
            env['JIACLAW_WECOM_' + name] = value
        start()
        for denied in [{**destination, 'conversation_id': '@all'}, {**destination, 'thread_id': 'thread'},
                       {**destination, 'conversation_id': 'alice|bob'},
                       {**destination, 'conversation_id': 'long.user'},
                       {**destination, 'installation_id': corp + ':1000003'},
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
        for secret in [app_secret, callback_token, encoding_aes_key, access_token,
                       private_error, gateway_secret, api_token]:
            assert secret not in public, 'secret or private platform body escaped'
        assert not fixture_errors, fixture_errors
        stop()
        print('WeCom GET/POST crypto, identity/XML checks, casefold/dedup and native tools: passed')
        print('WeCom verified app/member startup, token reuse, UTF-8 split, spacing and unknown/crash recovery: passed')
        print('WeCom credential-free stopped-channel maintenance preserves unknown audit and quota without replay: passed')
        print('WeCom exact scheduled recipients, environment credentials and secret handling: passed')
finally:
    if process and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    for pending in gates.values():
        pending.set()
    fixture.shutdown()
    fixture.server_close()
