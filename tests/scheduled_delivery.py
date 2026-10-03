#!/usr/bin/env python3
"""Scheduled Telegram/Slack delivery through a real host and temporary SQLite.

The native model and both platforms are localhost fixtures with disposable
credentials. No real supplier or messaging account is used.
"""
from contextlib import closing
import copy
import html
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
telegram_token = '123456:' + uuid.uuid4().hex
slack_token = 'xoxb-fixture-' + uuid.uuid4().hex
private_error = 'PRIVATE-SCHEDULED-DELIVERY-' + uuid.uuid4().hex
model_calls = {}
platform_calls = {}
fixture_errors = []
fixture_lock = threading.Lock()
gates = {}
process = None
base = None


def gate(case):
    return gates.setdefault(case, threading.Event())


def reply(case):
    return 'scheduled-delivery:' + case + ' completed. <plain text>'


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

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/chat/completions':
                assert self.headers['Authorization'] == 'Bearer ' + gateway_secret
                assert self.headers.get('Idempotency-Key')
                prompt = next(message['content'] for message in reversed(body['messages'])
                              if message['role'] == 'user')
                case = re.search(r'scheduled-delivery:([a-z_]+)', prompt).group(1)
                assert {tool['function']['name'] for tool in body['tools']} == {'datetime_now'}
                with fixture_lock:
                    model_calls.setdefault(case, []).append(body)
                if body['messages'][-1]['role'] == 'tool':
                    assert body['messages'][-1]['tool_call_id'].startswith('clock-')
                    assert 'Unix' in body['messages'][-1]['content']
                    if case == 'pause_delete':
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
                return
            if self.path == '/bot' + telegram_token + '/sendMessage':
                assert body['chat_id'] == '-100123'
                assert body['link_preview_options'] == {'is_disabled': True}
                assert 'parse_mode' not in body
                text = body['text']
                receipt = {'ok': True, 'result': {'message_id': 123, 'chat': {'id': -100123}}}
            else:
                assert self.path == '/chat.postMessage'
                assert self.headers['Authorization'] == 'Bearer ' + slack_token
                assert body['channel'] == 'C123ABC'
                assert body['mrkdwn'] is False and body['parse'] == 'none'
                assert body['link_names'] is False
                assert body['unfurl_links'] is False and body['unfurl_media'] is False
                assert '<plain text>' not in body['text']
                text = html.unescape(body['text'])
                receipt = {'ok': True, 'channel': 'C123ABC', 'ts': '1760000000.123456'}
            case = re.search(r'scheduled-delivery:([a-z_]+)', text).group(1)
            assert text == reply(case)
            with fixture_lock:
                platform_calls.setdefault(case, []).append({'body': body, 'at': time.monotonic()})
                attempt = len(platform_calls[case])
            if case == 'unknown' and attempt == 1:
                self.respond(502, {'error': private_error + ' ' + slack_token})
            elif case == 'disconnect' and attempt == 1:
                self.connection.shutdown(socket.SHUT_RDWR)
                self.connection.close()
                self.close_connection = True
            elif case == 'submitting':
                gate(case).wait(timeout=90)
                self.respond(200, receipt)
            elif case in ['cooldown', 'revoke_send'] and attempt == 1:
                self.respond(429, {'ok': False, 'error': 'ratelimited'}, {'Retry-After': '4'})
            else:
                self.respond(200, receipt)
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)


fixture = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
fixture.daemon_threads = True
threading.Thread(target=fixture.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-scheduled-delivery-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        database = root / 'state' / 'sessions.sqlite3'
        config = root / 'config.json'
        fixture_base = 'http://127.0.0.1:' + str(fixture.server_port)
        telegram_target = {'channel': 'telegram', 'installation_id': '123456',
                           'conversation_id': '-100123', 'thread_id': None}
        slack_target = {'channel': 'slack', 'installation_id': 'T123ABC',
                        'conversation_id': 'C123ABC', 'thread_id': '1760000000.123456'}
        settings = {
            'agent': {'name': 'scheduled-delivery-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter', 'base_url': fixture_base,
                         'api_key': gateway_secret, 'model': 'fixture'},
            'scheduler': {'enabled': True},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token, 'persist': True,
                     'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1,
                     'telegram_secret': 'fixture-webhook-' + uuid.uuid4().hex,
                     'telegram_bot_token': telegram_token,
                     'slack_signing_secret': uuid.uuid4().hex, 'slack_bot_token': slack_token,
                     'channels': [
                         {'channel': 'telegram', 'installation_id': '123456',
                          'allowed_senders': ['987654'], 'allowed_conversations': ['-100123']},
                         {'channel': 'slack', 'installation_id': 'T123ABC', 'app_id': 'A123ABC',
                          'allowed_senders': ['U123ABC'], 'allowed_conversations': ['C123ABC']},
                     ]},
        }
        for binding in settings['http']['channels']:
            binding.update(enabled_tools=['datetime_now'], timeout_secs=30,
                           local_test_api_base=fixture_base)
        env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
        env['JIACLAW_LOG_LEVEL'] = 'info'
        logs = []

        def request(path, method='GET', body=None, authenticated=True):
            headers = {'Authorization': 'Bearer ' + api_token} if authenticated else {}
            if body is not None:
                headers['Content-Type'] = 'application/json'
            req = urllib.request.Request(base + path, method=method, headers=headers,
                                         data=json.dumps(body).encode() if body is not None else None)
            try:
                response = urllib.request.urlopen(req, timeout=10)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                raw = response.read()
                if not raw:
                    return response.status, None
                if 'application/json' in response.headers.get('Content-Type', ''):
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
                    assert request('/health')[0] == 200
                    assert request('/api/channels/status')[1]['state'] == 'running'
                    if settings['scheduler']['enabled']:
                        assert request('/api/jobs/status')[1]['state'] == 'running'
                    return
                time.sleep(.05)
            raise AssertionError('server startup timed out: ' + log.read_text())

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

        def spec(case, target=None, seconds=1):
            return {'name': case, 'prompt': 'scheduled-delivery:' + case,
                    'schedule': {'kind': 'interval', 'seconds': seconds},
                    'enabled_tools': ['datetime_now'], 'timeout_secs': 30,
                    'delivery': copy.deepcopy(target or telegram_target)}

        def create(body):
            status, value = request('/api/jobs', 'POST', body)
            assert status in [200, 201] and value['spec'] == body, (status, value)
            return value

        def job_path(job):
            return '/api/jobs/' + job['id']

        def get(job):
            status, value = request(job_path(job))
            assert status == 200, (status, value)
            return value

        def runs(job):
            status, value = request(job_path(job) + '/runs')
            assert status == 200 and isinstance(value, list), (status, value)
            return value

        def completed_run(job, previous=None):
            return eventually(lambda: next((run for run in runs(job) if run['status'] == 'completed'
                                             and run['id'] != previous), None), 'completed ' + job['spec']['name'])

        def delivery_path(job, run):
            return job_path(job) + '/runs/' + run['id'] + '/deliveries'

        def deliveries(job, run):
            status, value = request(delivery_path(job, run))
            assert status == 200 and isinstance(value, list), (status, value)
            for item in value:
                assert item['event_id'] is None and item['job_run_id'] == run['id']
                assert item['job_id'] == job['id']
            return sorted(value, key=lambda item: item['ordinal'])

        def delivery_state(job, run, state):
            items = deliveries(job, run)
            return items if len(items) == 1 and items[0]['state'] == state else None

        def pause(job):
            assert request(job_path(job) + '/pause', 'POST')[0] == 200
            assert not get(job)['enabled']

        def assert_delivered(job, run, case, model_count=2):
            item = eventually(lambda: delivery_state(job, run, 'delivered'), case + ' delivered')[0]
            assert item['receipt'] and item['text'] == reply(case)
            status, session = request('/api/sessions/' + run['session_id'])
            assert status == 200 and session['messages'][-1] == run['response']['message'], (status, session)
            assert run['response']['message']['content'] == reply(case)
            assert called(case) == model_count
            return item

        def cancel(job, run):
            assert request(delivery_path(job, run) + '/cancel', 'POST')[0] == 204

        # Existing inbound conversation permission must never imply proactive
        # scheduled-delivery permission. The new list defaults to empty.
        start()
        assert request('/api/jobs', 'POST', spec('unauthorized'))[0] == 400
        assert model_calls == {} and platform_calls == {}
        stop()
        settings['http']['channels'][0]['scheduled_destinations'] = [
            {'conversation_id': '-100123', 'thread_id': None},
            {'conversation_id': '-100123', 'thread_id': '321'},
        ]
        settings['http']['channels'][1]['scheduled_destinations'] = [
            {'conversation_id': 'C123ABC', 'thread_id': '1760000000.123456'},
        ]
        for extra in [{'url': fixture_base}, {'token': 'must-not-be-a-destination'}]:
            rejected = copy.deepcopy(settings)
            rejected['http']['channels'][0]['scheduled_destinations'][0].update(extra)
            config.write_text(json.dumps(rejected))
            outcome = subprocess.run([str(binary), 'serve', '--config', str(config)], env=env,
                                     capture_output=True, text=True, timeout=10)
            assert outcome.returncode != 0
            assert gateway_secret not in outcome.stdout + outcome.stderr
        start()
        assert request('/api/jobs', 'POST', spec('unauthorized'), authenticated=False)[0] == 401
        denied = [
            {**telegram_target, 'installation_id': 'OTHER'},
            {**telegram_target, 'conversation_id': '-100999'},
            {**telegram_target, 'thread_id': '999'},
            {**slack_target, 'thread_id': None},
            {**telegram_target, 'channel': 'discord'},
            {**telegram_target, 'url': fixture_base},
            {**telegram_target, 'token': telegram_token},
            {'channel': 'telegram'},
        ]
        for target in denied:
            status, value = request('/api/jobs', 'POST', spec('unauthorized', target))
            assert 400 <= status < 500, (target, status, value)
        assert request('/api/jobs', 'POST', {**spec('unauthorized'), 'enabled_tools': ['json_query']})[0] == 400
        assert model_calls == {} and platform_calls == {}

        telegram = create(spec('telegram'))
        telegram_run = completed_run(telegram)
        pause(telegram)
        assert_delivered(telegram, telegram_run, 'telegram')
        assert 'message_thread_id' not in platform_calls['telegram'][0]['body']
        path = delivery_path(telegram, telegram_run)
        for method, endpoint in [('GET', path), ('POST', path + '/cancel'), ('DELETE', path)]:
            assert request(endpoint, method, authenticated=False)[0] == 401
        assert request(path + '?limit=0')[0] == 400
        assert request(path + '?limit=101')[0] == 400
        assert len(request(path + '?limit=1')[1]) == 1

        slack = create(spec('slack', slack_target))
        slack_run = completed_run(slack)
        pause(slack)
        assert_delivered(slack, slack_run, 'slack')
        assert platform_calls['slack'][0]['body']['thread_ts'] == slack_target['thread_id']
        assert request(delivery_path(telegram, slack_run))[0] == 404
        assert request(delivery_path(telegram, slack_run) + '/cancel', 'POST')[0] == 404
        assert request(delivery_path(telegram, slack_run), 'DELETE')[0] == 404
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            assert db.execute('PRAGMA user_version').fetchone()[0] == 8
            assert db.execute('SELECT count(*) FROM channel_outbox WHERE '
                              '(event_id IS NULL) = (job_run_id IS NULL)').fetchone()[0] == 0

        # Audit cleanup can remove a resolved run's outbox without deleting its
        # session or run result. It must not dereference another job's run.
        assert request(delivery_path(telegram, telegram_run), 'DELETE')[0] == 204
        assert deliveries(telegram, telegram_run) == []
        assert runs(telegram)[0]['id'] == telegram_run['id']
        assert request('/api/sessions/' + telegram_run['session_id'])[0] == 200
        assert request(job_path(telegram), 'DELETE')[0] in [200, 204]
        assert request(job_path(telegram) + '?purge=true', 'DELETE')[0] in [200, 204]
        assert request(job_path(telegram))[0] == 404

        # Pausing/deleting a job stops future occurrences, not its admitted
        # result. The explicit delivery cancel API refuses active work.
        paused = create(spec('pause_delete', {**telegram_target, 'thread_id': '321'}))
        eventually(lambda: called('pause_delete') == 2, 'submitted pausable job')
        active_run = runs(paused)[0]
        assert active_run['status'] == 'running'
        assert request(delivery_path(paused, active_run) + '/cancel', 'POST')[0] == 409
        pause(paused)
        assert request(job_path(paused), 'DELETE')[0] in [200, 204]
        gate('pause_delete').set()
        paused_run = completed_run(paused)
        assert_delivered(paused, paused_run, 'pause_delete')
        assert platform_calls['pause_delete'][0]['body']['message_thread_id'] == 321
        assert request(job_path(paused) + '?purge=true', 'DELETE')[0] in [200, 204]

        # A known 429 keeps the job enabled, but its unresolved output blocks a
        # second occurrence even when another interval becomes due.
        cooldown = create(spec('cooldown', slack_target))
        cooldown_run = completed_run(cooldown)
        eventually(lambda: delivery_state(cooldown, cooldown_run, 'retry_wait'), 'persisted cooldown')
        assert get(cooldown)['enabled']
        time.sleep(1.3)
        assert len(runs(cooldown)) == 1 and called('cooldown') == 2
        pause(cooldown)
        cooldown_item = assert_delivered(cooldown, cooldown_run, 'cooldown')
        assert cooldown_item['attempts'] == 2 and sent('cooldown') == 2

        unknown = create(spec('unknown', slack_target))
        unknown_run = completed_run(unknown)
        uncertain = eventually(lambda: delivery_state(unknown, unknown_run, 'unknown'), 'unknown delivery')[0]
        eventually(lambda: not get(unknown)['enabled'], 'unknown pauses job')
        assert request(job_path(unknown) + '/resume', 'POST')[0] == 409
        assert request(delivery_path(unknown, unknown_run), 'DELETE')[0] == 409
        assert request(job_path(unknown) + '?purge=true', 'DELETE')[0] == 409
        stop(kill=True)
        start()
        time.sleep(1.2)
        assert len(runs(unknown)) == 1 and runs(unknown)[0]['id'] == unknown_run['id']
        assert delivery_state(unknown, unknown_run, 'unknown')
        assert called('unknown') == 2 and sent('unknown') == 1 and not get(unknown)['enabled']
        resolution = '/api/channels/deliveries/' + uncertain['id'] + '/resolve'
        assert request(resolution, 'POST', {'action': 'delivered', 'receipt': ''})[0] == 400
        assert request(resolution, 'POST', {'action': 'delivered', 'receipt': 'Verified platform message.'})[0] == 204
        assert request(job_path(unknown) + '/resume', 'POST')[0] == 200
        new_unknown_run = completed_run(unknown, previous=unknown_run['id'])
        pause(unknown)
        assert new_unknown_run['scheduled_for_ms'] > unknown_run['scheduled_for_ms']
        assert_delivered(unknown, new_unknown_run, 'unknown', model_count=4)
        assert sent('unknown') == 2 and len(runs(unknown)) == 2
        assert delivery_state(unknown, unknown_run, 'delivered')

        disconnected = create(spec('disconnect', {**telegram_target, 'thread_id': '321'}))
        disconnected_run = completed_run(disconnected)
        unknown_disconnect = eventually(lambda: delivery_state(disconnected, disconnected_run, 'unknown'),
                                       'submitted connection failure')[0]
        eventually(lambda: not get(disconnected)['enabled'], 'connection failure pauses job')
        assert request(job_path(disconnected) + '/resume', 'POST')[0] == 409
        assert request('/api/channels/deliveries/' + unknown_disconnect['id'] + '/resolve', 'POST',
                       {'action': 'cancel'})[0] == 204
        assert delivery_state(disconnected, disconnected_run, 'cancelled')
        assert request(job_path(disconnected) + '/resume', 'POST')[0] == 200
        pause(disconnected)
        assert called('disconnect') == 2 and sent('disconnect') == 1

        submitting = create(spec('submitting'))
        submitting_run = completed_run(submitting)
        eventually(lambda: sent('submitting') == 1, 'platform request already submitted')
        assert delivery_state(submitting, submitting_run, 'submitting')
        assert request(delivery_path(submitting, submitting_run) + '/cancel', 'POST')[0] == 409
        stop(kill=True)
        gate('submitting').set()
        start()
        eventually(lambda: delivery_state(submitting, submitting_run, 'unknown'), 'submitted recovery')
        assert not get(submitting)['enabled']
        time.sleep(.4)
        assert called('submitting') == 2 and sent('submitting') == 1
        assert request(job_path(submitting) + '/resume', 'POST')[0] == 409
        assert request(job_path(submitting), 'DELETE')[0] in [200, 204]
        assert request(job_path(submitting) + '?purge=true', 'DELETE')[0] == 409
        cancel(submitting, submitting_run)
        assert request(delivery_path(submitting, submitting_run), 'DELETE')[0] == 204
        assert request(job_path(submitting) + '?purge=true', 'DELETE')[0] in [200, 204]

        # Revoke a destination after admission but before its first occurrence.
        # Startup, execution, and resume must not silently retain old authority.
        revoked = create(spec('revoke_execution', {**telegram_target, 'thread_id': '321'}, seconds=2))
        stop()
        saved_targets = copy.deepcopy(settings['http']['channels'][0]['scheduled_destinations'])
        settings['http']['channels'][0]['scheduled_destinations'] = [saved_targets[0]]
        start()
        eventually(lambda: not get(revoked)['enabled'], 'revoked execution target pauses job')
        assert called('revoke_execution') == 0 and sent('revoke_execution') == 0
        assert 400 <= request(job_path(revoked) + '/resume', 'POST')[0] < 500
        stop()
        settings['http']['channels'][0]['scheduled_destinations'] = saved_targets
        start()

        # Changing installation tools while a known-unsent 429 is pending must
        # stop delivery, pause the job, and also prevent a manual resume.
        revoked_send = create(spec('revoke_send', slack_target))
        revoked_send_run = completed_run(revoked_send)
        eventually(lambda: delivery_state(revoked_send, revoked_send_run, 'retry_wait'), 'revocation cooldown')
        stop()
        settings['http']['channels'][1]['enabled_tools'] = ['json_query']
        start()
        eventually(lambda: delivery_state(revoked_send, revoked_send_run, 'permanent_failed'),
                   'revoked tools prohibit outbound retry')
        assert not get(revoked_send)['enabled']
        assert called('revoke_send') == 2 and sent('revoke_send') == 1
        cancel(revoked_send, revoked_send_run)
        assert request(job_path(revoked_send) + '/resume', 'POST')[0] == 400
        stop()
        settings['http']['channels'][1]['enabled_tools'] = ['datetime_now']
        start()

        # Delivery audit remains operable when scheduling is disabled, so an
        # operator can inspect/cancel output without re-enabling execution.
        stop()
        settings['scheduler']['enabled'] = False
        start()
        assert deliveries(slack, slack_run)[0]['state'] == 'delivered'
        assert request(delivery_path(slack, slack_run), 'DELETE')[0] == 204
        stop()
        settings['scheduler']['enabled'] = True
        start()

        # Insert failure occurs inside the transaction shared by the completed
        # run, session and outbox. No partial success or automatic rerun survives.
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            db.execute("CREATE TRIGGER fixture_scheduled_outbox_fault BEFORE INSERT ON channel_outbox "
                       "WHEN NEW.job_run_id IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture outbox write fault'); END")
            db.commit()
        faulty = create(spec('fault'))
        eventually(lambda: request('/api/jobs/status')[1]['state'] == 'failed', 'scheduler fails closed')
        fault_run = eventually(lambda: next((run for run in runs(faulty) if run['status'] == 'interrupted'), None),
                               'failed completion recovers as interrupted')
        assert not get(faulty)['enabled']
        assert request('/api/sessions/' + fault_run['session_id'])[0] == 404
        assert deliveries(faulty, fault_run) == [] and fault_run['response'] is None
        assert called('fault') == 2 and sent('fault') == 0
        assert request('/api/jobs', 'POST', spec('after_fault'))[0] == 503
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=rw', uri=True, timeout=5)) as db:
            db.execute('DROP TRIGGER fixture_scheduled_outbox_fault')
            db.commit()
        assert request('/api/jobs/status')[1]['state'] == 'failed'
        stop()
        start()
        assert runs(faulty)[0]['id'] == fault_run['id'] and runs(faulty)[0]['status'] == 'interrupted'
        assert not get(faulty)['enabled'] and called('fault') == 2 and called('after_fault') == 0
        public = json.dumps(request('/api/channels/deliveries?limit=100')[1])
        public += json.dumps(request('/api/jobs?include_deleted=true')[1])
        public += ''.join(path.read_text() for path in logs)
        assert all(secret not in public for secret in [gateway_secret, telegram_token, slack_token, private_error])
        stop()
        assert not fixture_errors, fixture_errors
        print('Scheduled delivery acceptance passed: precise target/tool authority, Telegram/Slack native '
              'results and threads, atomic run/session/outbox, durable backpressure, unknown-result pause '
              'and manual recovery, no crash replay, audit ownership/cleanup, policy revocation and fail-closed storage.')
finally:
    for pending in gates.values():
        pending.set()
    if process and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    fixture.shutdown()
    fixture.server_close()
