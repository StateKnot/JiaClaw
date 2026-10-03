#!/usr/bin/env python3
"""Tenant cron acceptance: real gateway + two real backends + local model.

Only temporary data and synthetic credentials are used. No paid provider calls.
"""
import json
import os
from pathlib import Path
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
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'off'
client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
lock = threading.Lock()
observed, errors, model_keys = [], [], set()
model_secrets = {name: 'fixture-model-' + uuid.uuid4().hex for name in ('alice', 'bob')}
model_submitted, model_release = threading.Event(), threading.Event()
held_identity = None


def request_id():
    # Canonical UUIDv7 using the current millisecond timestamp, no extra package.
    value = (int(time.time() * 1000) << 80) | (0x7 << 76) | (int.from_bytes(os.urandom(2), 'big') & 0xFFF) << 64
    value |= (0b10 << 62) | (int.from_bytes(os.urandom(8), 'big') & ((1 << 62) - 1))
    return str(uuid.UUID(int=value))


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait(check, description, seconds=15, diagnostics=None):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with lock:
            assert not errors, errors
        value = check()
        if value:
            return value
        time.sleep(.05)
    detail = json.dumps(diagnostics(), sort_keys=True) if diagnostics else 'unavailable'
    raise AssertionError('Timed out: ' + description + '; diagnostics=' + detail)


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            who = next(name for name, secret in model_secrets.items()
                       if self.headers.get('Authorization') == 'Bearer ' + secret)
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            key = self.headers.get('Idempotency-Key')
            assert key and body['model'] == 'fixture-scheduled', body
            prompt = next(item['content'] for item in reversed(body['messages']) if item['role'] == 'user')
            with lock:
                assert key not in model_keys, 'model POST was replayed'
                model_keys.add(key)
                observed.append((who, prompt, key))
                held = held_identity == who
            if held:
                model_submitted.set()
                assert model_release.wait(30), 'held fixture not released'
            data = json.dumps({'choices': [{'message': {'role': 'assistant',
                                'content': who + ': <img src=x onerror="alert(1)"> ' + prompt[-128:]},
                                'finish_reason': 'stop'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass  # Expected when a submitted backend request is killed.
        except Exception as error:
            with lock:
                errors.append(repr(error))
            self.send_error(500)


def count(who):
    with lock:
        assert not errors, errors
        return sum(item[0] == who for item in observed)


def request(base, path, token=None, method='GET', body=None):
    headers = {'Authorization': 'Bearer ' + token} if token else {}
    if body is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(base + path, method=method, headers=headers,
                                 data=json.dumps(body).encode() if body is not None else None)
    try:
        response = client.open(req, timeout=10)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read()
        assert len(raw) <= (1024 * 1024 if path.startswith('/api/jobs') else 2 * 1024 * 1024), 'unbounded HTTP result'
        return response.status, json.loads(raw) if raw else None, response.headers


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
threading.Thread(target=model.serve_forever, daemon=True).start()
processes = []


def stop(process, kill=False):
    if process is not None and process.poll() is None:
        process.kill() if kill else process.terminate()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-cron-') as temp:
        root = Path(temp).resolve()
        backends = {}
        gateway_config = {'bind': '127.0.0.1:' + str(port()),
                          'registry_path': str(root / 'registry/users.sqlite3'),
                          'request_timeout_seconds': 150, 'max_in_flight': 4,
                          'scheduled_jobs': True, 'backends': []}
        config_path = root / 'gateway.json'
        gateway_url = 'http://' + gateway_config['bind']

        def cli(*arguments):
            result = subprocess.run([str(binary), 'gateway', *arguments, '--config', str(config_path)],
                                    env=env, text=True, capture_output=True, timeout=15)
            assert result.returncode == 0, (arguments, result.stdout, result.stderr)
            return json.loads(result.stdout)

        def launch(arguments, base, logfile):
            with logfile.open('ab') as output:
                process = subprocess.Popen([str(binary), *arguments], env=env, stdout=output, stderr=output)
            processes.append(process)
            def healthy():
                assert process.poll() is None, logfile.read_text()
                try:
                    return request(base, '/health')[0] == 200
                except (OSError, urllib.error.URLError):
                    return False
            wait(healthy, 'server startup')
            return process

        def start_backend(who):
            backend = backends[who]
            backend['process'] = launch(['serve', '--config', str(backend['config_path'])],
                                        backend['url'], root / (who + '.log'))

        def start_gateway():
            return launch(['gateway', 'serve', '--config', str(config_path)], gateway_url, root / 'gateway.log')

        def api(who, path, method='GET', body=None, expected=200):
            status, data, headers = request(gateway_url, path, users[who]['token'], method, body)
            assert status == expected, (who, method, path, status, data)
            return data

        def spec(label, **changes):
            value = {'name': label, 'prompt': 'tenant-cron:' + label,
                     'schedule': {'kind': 'interval', 'seconds': 60},
                     'enabled_tools': ['datetime_now'], 'timeout_secs': 120}
            value.update(changes)
            return value

        def create(who, name, **changes):
            return api(who, '/api/jobs', 'POST', spec(name, **changes), 201)

        def due(who, job_id, after_ms=300):
            # Fixture-only clock setup; no production endpoint can choose next_due.
            with sqlite3.connect(backends[who]['db']) as connection:
                connection.execute('UPDATE jobs SET next_due_ms=? WHERE id=?',
                                   (int(time.time() * 1000) + after_ms, job_id))

        def runs(who, job_id):
            return api(who, '/api/jobs/' + job_id + '/runs?limit=5&offset=0')['items']

        def completed(who, job_id, previous=0):
            return wait(lambda: next((run for run in runs(who, job_id)
                                      if run['status'] == 'completed' and run['started_ms'] > previous), None),
                        who + ' completed run', diagnostics=lambda: scheduler_summary(who))

        def hold(who):
            return next(user for user in cli('user-list')['users'] if user['user_id'] == users[who]['user_id'])['hold']

        def scheduler_summary(who):
            # Only identifiers, states and timestamps: no prompt, result, key or audit note.
            summary = {'backend': who, 'backend_returncode': backends[who]['process'].poll(),
                       'gateway_returncode': gateway_process.poll()}
            with lock:
                summary['model_requests'] = sum(item[0] == who for item in observed)
            try:
                pending = hold(who)
                summary['hold'] = None if pending is None else {
                    key: pending.get(key) for key in
                    ('request_id', 'state', 'reason', 'admitted_ms', 'updated_ms')}
                with sqlite3.connect(backends[who]['db'].as_uri() + '?mode=ro', uri=True, timeout=1) as connection:
                    connection.row_factory = sqlite3.Row
                    for label, query in [
                        ('jobs', 'SELECT id,enabled,deleted,next_due_ms FROM jobs WHERE deleted=0 ORDER BY next_due_ms LIMIT 5'),
                        ('runs', 'SELECT id,job_id,scheduled_for_ms,started_ms,finished_ms,status FROM job_runs ORDER BY started_ms DESC LIMIT 5'),
                        ('dispatches', 'SELECT request_id,run_id,job_id,status,issued_ms,created_ms FROM scheduler_dispatches ORDER BY created_ms DESC LIMIT 5')]:
                        summary[label] = [dict(row) for row in connection.execute(query)]
            except Exception as error:
                summary['diagnostic_error'] = type(error).__name__
            return summary

        for who in ('alice', 'bob'):
            directory = root / who
            workspace = directory / 'workspace'
            workspace.mkdir(parents=True)
            token = 'fixture-backend-' + uuid.uuid4().hex
            base = 'http://127.0.0.1:' + str(port())
            settings = {'agent': {'name': who, 'description': 'Tenant cron fixture',
                                  'system_instructions': 'Use only configured tools.', 'max_turns': 10,
                                  'workspace_path': str(workspace)},
                        'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:' + str(model.server_port),
                                     'api_key': model_secrets[who], 'model': 'fixture-scheduled'},
                        'http': {'bind': base.removeprefix('http://'), 'api_token': token, 'persist': True,
                                 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
                        'scheduler': {'enabled': True, 'gateway_driven': True}}
            cfg = directory / 'config.json'
            cfg.write_text(json.dumps(settings))
            secret = directory / 'backend.secret'
            secret.write_text(token)
            backends[who] = {'url': base, 'token': token, 'config_path': cfg,
                             'db': directory / 'state/sessions.sqlite3'}
            gateway_config['backends'].append({'id': who, 'url': base, 'token_file': str(secret)})
            start_backend(who)
        config_path.write_text(json.dumps(gateway_config))
        users = {who: cli('user-add', '--backend', who) for who in ('alice', 'bob')}
        gateway_process = start_gateway()

        assert request(gateway_url, '/api/gateway/capabilities')[0] == 401
        assert api('alice', '/api/gateway/capabilities')['scheduled_jobs'] is True
        idle_id = request_id()
        alice_backend = backends['alice']
        idle = request(alice_backend['url'], '/internal/scheduler/dispatch', alice_backend['token'],
                       'POST', {'request_id': idle_id})
        assert idle[0] == 200 and idle[1]['request_id'] == idle_id and idle[1]['run'] is None, idle
        assert request(gateway_url, '/internal/scheduler/dispatch', users['alice']['token'],
                       'POST', {'request_id': request_id()})[0] == 404
        for path in ['/api/internal/scheduler/dispatch', '/internal/scheduler/dispatch', '/internal/scheduler/status',
                     '/api/jobs/dispatch', '/api/jobs/%2e%2e', '/api/jobs?limit=6',
                     '/api/jobs?offset=10001', '/api/jobs?limit=5&limit=1', '/api/jobs?tenant=bob']:
            assert request(gateway_url, path, users['alice']['token'])[0] in (400, 404), path
        jobs = {who: create(who, 'same-name') for who in ('alice', 'bob')}
        assert jobs['alice']['id'] != jobs['bob']['id']
        repeated = request(alice_backend['url'], '/internal/scheduler/dispatch', alice_backend['token'],
                           'POST', {'request_id': idle_id})
        assert repeated[0] == 200 and repeated[1]['run'] is None, repeated
        api('bob', '/api/jobs/' + jobs['alice']['id'], expected=404)
        for who in ('alice', 'bob'):
            page = api(who, '/api/jobs?limit=5&offset=0&include_deleted=false')
            assert [job['id'] for job in page['items']] == [jobs[who]['id']] and page['next_offset'] is None
            assert api(who, '/api/jobs/' + jobs[who]['id'])['session_id'].startswith('job:')
        assert not api('alice', '/api/sessions')['sessions'], 'job sessions exposed as ordinary chat sessions'
        for changes in [{'timeout_secs': 121}, {'enabled_tools': ['exec']}, {'delivery': {}},
                        {'name': 'x' * 129}, {'prompt': 'x' * (32768 + 1)}]:
            api('alice', '/api/jobs', 'POST', spec('rejected', **changes), 400)
        # Six maximum-size prompts exercise bounded responses and row pagination.
        maximum = []
        for index in range(6):
            maximum.append(create('alice', 'x' * 128, prompt='\n' * 32767 + 'x',
                                  schedule={'kind': 'interval', 'seconds': 31536000}))
        page = api('alice', '/api/jobs?limit=5&offset=0')
        assert 1 <= len(page['items']) <= 5 and page['next_offset'] is not None
        second = api('alice', '/api/jobs?limit=5&offset=' + str(page['next_offset']))
        assert not {item['id'] for item in page['items']} & {item['id'] for item in second['items']}
        for job in maximum:
            api('alice', '/api/jobs/' + job['id'], 'DELETE', expected=204)
        paused = api('alice', '/api/jobs/' + jobs['alice']['id'] + '/pause', 'POST')
        assert not paused['enabled']
        resumed = api('alice', '/api/jobs/' + jobs['alice']['id'] + '/resume', 'POST')
        assert resumed['enabled']
        print('PASS: authenticated capabilities, tenant CRUD/isolation, bounded large-input pagination, validation and DELETE 204')

        for who in ('alice', 'bob'):
            due(who, jobs[who]['id'])
        first = {who: completed(who, jobs[who]['id']) for who in ('alice', 'bob')}
        for who in ('alice', 'bob'):
            assert first[who]['response']['message']['content'].startswith(who + ':')
            with sqlite3.connect(backends[who]['db']) as connection:
                binding = connection.execute('SELECT request_id FROM scheduler_dispatches WHERE run_id=?',
                                             (first[who]['id'],)).fetchone()
                assert binding and str(uuid.UUID(binding[0])) == binding[0]
            receipt = request(backends[who]['url'], '/internal/scheduler/operations/' + binding[0], backends[who]['token'])
            assert receipt[0] == 200 and receipt[1]['request_id'] == binding[0], receipt
            assert receipt[1]['run']['id'] == first[who]['id'] and receipt[1]['run']['status'] == 'completed', receipt
        assert not api('alice', '/api/sessions')['sessions']
        previous_key = users['alice']
        users['alice'] = cli('key-rotate', '--key', previous_key['key_id'])
        assert request(gateway_url, '/api/jobs', previous_key['token'])[0] == 401
        due('alice', jobs['alice']['id'])
        first['alice'] = completed('alice', jobs['alice']['id'], first['alice']['started_ms'])
        cli('user-disable', '--user', users['alice']['user_id'])
        before = count('alice')
        due('alice', jobs['alice']['id']); due('bob', jobs['bob']['id'])
        second = completed('bob', jobs['bob']['id'], first['bob']['started_ms'])
        assert count('alice') == before
        assert request(gateway_url, '/api/jobs', users['alice']['token'])[0] == 401
        cli('user-enable', '--user', users['alice']['user_id'])
        # Stop the gateway: both live backends must remain passive.
        stop(gateway_process)
        before = (count('alice'), count('bob'))
        due('alice', jobs['alice']['id']); due('bob', jobs['bob']['id'])
        time.sleep(1.5)
        assert (count('alice'), count('bob')) == before, 'backend ran autonomous scheduler without gateway'
        # An expired occurrence still causes an admitted idle dispatch to advance
        # its schedule. Park both tenants before restarting, then make only Alice
        # due below, so the injected crash cannot also interrupt Bob's idle write.
        for who in ('alice', 'bob'):
            due(who, jobs[who]['id'], after_ms=3_600_000)
        gateway_process = start_gateway()
        wait(lambda: hold('bob') is None, 'non-target Bob settled before crash',
             diagnostics=lambda: scheduler_summary('bob'))
        print('PASS: result/dispatch identity persisted per tenant; disabled user does not dispatch, other user continues, gateway stop prevents autonomous backend work')

        # Claim and submit Alice's model request, then kill its backend and gateway.
        with lock:
            held_identity = 'alice'
        due('alice', jobs['alice']['id'])
        assert model_submitted.wait(10), 'scheduled model submission not observed'
        in_flight = runs('alice', jobs['alice']['id'])[0]
        assert in_flight['status'] == 'running'
        assert api('alice', '/api/jobs/status') is not None, 'control reads blocked by execution'
        status, _, _ = request(gateway_url, '/api/chat', users['alice']['token'], 'POST',
                               {'session_id': 'same', 'messages': [{'role': 'user', 'content': 'must not overlap'}]})
        assert status in (409, 429), status
        assert hold('bob') is None, scheduler_summary('bob')
        stop(backends['alice']['process'], kill=True)
        stop(gateway_process, kill=True)
        with lock:
            held_identity = None
        model_release.set()
        submitted_count = count('alice')
        start_backend('alice')
        gateway_process = start_gateway()
        assert hold('bob') is None, scheduler_summary('bob')
        pending = hold('alice')
        assert pending is not None and pending['state'] == 'needs_review', pending
        recovered = runs('alice', jobs['alice']['id'])[0]
        assert recovered['id'] == in_flight['id'] and recovered['status'] == 'interrupted', recovered
        assert not api('alice', '/api/jobs/' + jobs['alice']['id'])['enabled']
        old_key = users['alice']
        users['alice'] = cli('key-rotate', '--key', old_key['key_id'])
        assert users['alice']['user_id'] == old_key['user_id']
        assert request(gateway_url, '/api/jobs', old_key['token'])[0] == 401
        api('alice', '/api/jobs/' + jobs['alice']['id'] + '/resume', 'POST', expected=409)
        due('bob', jobs['bob']['id'])
        completed('bob', jobs['bob']['id'], second['started_ms'])
        assert count('alice') == submitted_count, 'unknown scheduled operation was automatically replayed'
        assert hold('alice')['request_id'] == pending['request_id']
        with sqlite3.connect(backends['alice']['db']) as connection:
            assert connection.execute('SELECT run_id FROM scheduler_dispatches WHERE request_id=?',
                                      (pending['request_id'],)).fetchone() == (in_flight['id'],)
        print('PASS: claimed/submitted run survives SIGKILL as interrupted with matching durable hold; key rotation cannot bypass it, Bob continues and no model POST repeats')

        # Explicit reconciliation permits a new future occurrence, never the interrupted run.
        cli('review-clear', '--user', users['alice']['user_id'], '--confirm-backend-idle',
            '--note', 'Fixture backend restarted idle; interrupted run and synthetic model receipt reviewed.')
        api('alice', '/api/jobs/' + jobs['alice']['id'] + '/resume', 'POST')
        due('alice', jobs['alice']['id'])
        latest = completed('alice', jobs['alice']['id'], in_flight['started_ms'])
        assert latest['id'] != in_flight['id'] and count('alice') == submitted_count + 1
        print('PASS: only explicit reviewed clearance and resume admit a new occurrence; prior run and dispatch identity remain unchanged')
        for path in root.glob('*.log'):
            text = path.read_text()
            for secret in list(model_secrets.values()) + [user['token'] for user in users.values()] + [item['token'] for item in backends.values()]:
                assert secret not in text, 'credential exposed in process logs'
        with lock:
            assert not errors, errors
finally:
    model_release.set()
    for process in reversed(processes):
        stop(process)
    model.shutdown()
    model.server_close()
