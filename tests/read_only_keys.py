#!/usr/bin/env python3
"""Actual gateway and two isolated backend binaries; local model protocol only."""
import http.client
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
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'off'
client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
lock = threading.Lock()
model_calls, model_errors = [], []
model_secrets = {who: 'fixture-model-' + uuid.uuid4().hex for who in ('alice', 'bob')}


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            who = next(who for who, secret in model_secrets.items()
                       if self.headers.get('Authorization') == 'Bearer ' + secret)
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert body['model'] == 'fixture-read-only'
            prompt = next(item['content'] for item in reversed(body['messages']) if item['role'] == 'user')
            with lock:
                model_calls.append((who, prompt))
            data = json.dumps({'choices': [{'message': {'role': 'assistant', 'content': who + ': ' + prompt},
                                           'finish_reason': 'stop'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except Exception as error:
            with lock:
                model_errors.append(type(error).__name__)
            self.send_error(500)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def request(base, path, token=None, method='GET', body=None, headers=None):
    values = dict(headers or {})
    if token is not None:
        values['Authorization'] = 'Bearer ' + token
    if body is not None:
        values['Content-Type'] = 'application/json'
    req = urllib.request.Request(base + path, method=method, headers=values,
                                 data=json.dumps(body).encode() if body is not None else None)
    try:
        response = client.open(req, timeout=10)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read()
        assert len(raw) <= 2 * 1024 * 1024
        assert not response.headers.get('Set-Cookie')
        assert response.headers.get('Cache-Control') == 'no-store' or path == '/health'
        mime = response.headers.get('Content-Type', '')
        return response.status, json.loads(raw) if raw and 'application/json' in mime else raw, response.headers


def wait(check, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with lock:
            assert not model_errors, model_errors
        value = check()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError('read-only fixture deadline exceeded')


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
threading.Thread(target=model.serve_forever, daemon=True).start()
processes = []
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-read-only-') as temp:
        root = Path(temp).resolve()
        config_path = root / 'gateway.json'
        registry = root / 'registry/users.sqlite3'
        config = {'bind': '127.0.0.1:' + str(port()), 'registry_path': str(registry),
                  'scheduled_jobs': True, 'max_in_flight': 4,
                  'request_timeout_seconds': 150, 'backends': []}
        gateway_url = 'http://' + config['bind']
        backends = {}

        def cli(*args, ok=True):
            result = subprocess.run([str(binary), 'gateway', *args, '--config', str(config_path)],
                                    env=env, text=True, capture_output=True, timeout=15)
            if not ok:
                assert result.returncode != 0, 'invalid administrator command succeeded'
                return None
            assert result.returncode == 0, (args, result.stderr)
            return json.loads(result.stdout)

        def launch(args, base, name):
            logfile = root / (name + '.log')
            with logfile.open('ab') as output:
                proc = subprocess.Popen([str(binary), *args], env=env, stdout=output, stderr=output)
            processes.append(proc)
            def ready():
                assert proc.poll() is None, logfile.read_text()
                try:
                    return request(base, '/health')[0] == 200
                except (OSError, urllib.error.URLError):
                    return False
            wait(ready)
            return proc

        def api(key, path, method='GET', body=None, expected=200, headers=None):
            status, data, response_headers = request(gateway_url, path, key['token'], method, body, headers)
            assert status == expected, (method, path, status, data)
            assert response_headers['X-Content-Type-Options'] == 'nosniff'
            assert uuid.UUID(response_headers['X-Request-ID'])
            return data

        def hold_snapshot():
            # Keep both reads in one snapshot. Registry connections are short
            # lived and may remove WAL sidecars between independent statements.
            with sqlite3.connect(registry) as connection:
                connection.execute('BEGIN')
                return (connection.execute('SELECT count(*) FROM write_holds').fetchone()[0],
                        connection.execute("SELECT count(*) FROM audit_events WHERE action='write_admitted'").fetchone()[0])

        def content_snapshot():
            result = []
            for who in ('alice', 'bob'):
                with sqlite3.connect(backends[who]['db'].as_uri() + '?mode=ro', uri=True) as connection:
                    result.append((connection.execute('SELECT id,messages FROM sessions ORDER BY id').fetchall(),
                                   connection.execute('SELECT id,spec,enabled,deleted,next_due_ms FROM jobs ORDER BY id').fetchall()))
            return result

        for who in ('alice', 'bob'):
            directory = root / who
            workspace = directory / 'workspace'
            workspace.mkdir(parents=True)
            token = 'fixture-backend-' + uuid.uuid4().hex
            base = 'http://127.0.0.1:' + str(port())
            settings = {'agent': {'name': who, 'description': 'Read-only acceptance fixture',
                                  'system_instructions': 'Use configured tools.', 'max_turns': 10,
                                  'workspace_path': str(workspace)},
                        'provider': {'provider_type': 'brokerrouter',
                                     'base_url': 'http://127.0.0.1:' + str(model.server_port),
                                     'api_key': model_secrets[who], 'model': 'fixture-read-only'},
                        'http': {'bind': base.removeprefix('http://'), 'api_token': token, 'persist': True,
                                 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1},
                        'scheduler': {'enabled': True, 'gateway_driven': True}}
            cfg = directory / 'config.json'
            cfg.write_text(json.dumps(settings))
            secret = directory / 'backend.secret'
            secret.write_text(token)
            backends[who] = {'db': directory / 'state/sessions.sqlite3'}
            config['backends'].append({'id': who, 'url': base, 'token_file': str(secret)})
            launch(['serve', '--config', str(cfg)], base, who)
        config_path.write_text(json.dumps(config))
        full = {'alice': cli('user-add', '--backend', 'alice')}
        read = {'alice': cli('key-add', '--user', full['alice']['user_id'], '--read-only'),
                'bob': cli('user-add', '--backend', 'bob', '--read-only')}
        full['bob'] = cli('key-add', '--user', read['bob']['user_id'])
        assert all(key['read_only'] is False for key in full.values())
        assert all(key['read_only'] is True for key in read.values())
        gateway = launch(['gateway', 'serve', '--config', str(config_path)], gateway_url, 'gateway')
        sessions, jobs = {}, {}
        spec = {'name': 'owned schedule', 'prompt': 'previously authorized background work',
                'schedule': {'kind': 'interval', 'seconds': 31536000},
                'enabled_tools': ['datetime_now'], 'timeout_secs': 30}
        for who in ('alice', 'bob'):
            assert api(full[who], '/api/gateway/capabilities') == {'scheduled_jobs': True, 'read_only': False}
            assert api(read[who], '/api/gateway/capabilities') == {'scheduled_jobs': True, 'read_only': True}
            sessions[who] = api(full[who], '/api/sessions', 'POST')['session_id']
            response = api(full[who], '/api/chat', 'POST', {'session_id': sessions[who],
                           'messages': [{'role': 'user', 'content': who + ' private history'}],
                           'enabled_tools': []})
            assert response['status'] == 'completed' and response['message']['content'].startswith(who + ':')
            jobs[who] = api(full[who], '/api/jobs', 'POST', spec, 201)
            assert api(read[who], '/api/sessions')['sessions'][0]['id'] == sessions[who]
            assert api(read[who], '/api/sessions/' + sessions[who])['messages'][-1]['content'].startswith(who + ':')
            assert api(read[who], '/api/sessions/' + sessions[who] + '/export?format=json')['id'] == sessions[who]
            exported = api(read[who], '/api/sessions/' + sessions[who] + '/export?format=jsonl')
            assert json.loads(exported.splitlines()[-1])['content'].startswith(who + ':')
            assert api(read[who], '/api/jobs/status')['state'] == 'running'
            assert api(read[who], '/api/jobs?limit=5&offset=0')['items'][0]['id'] == jobs[who]['id']
            assert api(read[who], '/api/jobs/' + jobs[who]['id'])['id'] == jobs[who]['id']
            assert api(read[who], '/api/jobs/' + jobs[who]['id'] + '/runs')['items'] == []
        print('PASS: default full keys, explicit read-only issuance, real own history/exports and bounded job reads')

        before_hold, before_content, before_models = hold_snapshot(), content_snapshot(), len(model_calls)
        for credential in [*full.values(), *read.values()]:
            api(credential, '/api/turns?state=all&limit=5', expected=404)
            api(credential, '/api/turns/capabilities', expected=404)
        for who in ('alice', 'bob'):
            other = 'bob' if who == 'alice' else 'alice'
            for path in ['/api/sessions/' + sessions[other], '/api/sessions/' + sessions[other] + '/export',
                         '/api/jobs/' + jobs[other]['id'], '/api/jobs/' + jobs[other]['id'] + '/runs']:
                api(read[who], path, expected=404)
            forged = {'X-Tenant-ID': other, 'X-User-ID': full[other]['user_id'],
                      'X-JiaClaw-Read-Only': 'false', 'X-JiaClaw-Permissions': 'write'}
            own = api(read[who], '/api/sessions/' + sessions[who], headers=forged)
            assert own['messages'][-1]['content'].startswith(who + ':')
            for method, path, body in [
                ('POST', '/api/chat', {}), ('POST', '/api/sessions', {}),
                ('POST', '/api/sessions/import?overwrite=true', {'id': sessions[who], 'messages': []}),
                ('DELETE', '/api/sessions/' + sessions[who], None),
                ('POST', '/api/jobs', spec), ('POST', '/api/jobs/' + jobs[who]['id'] + '/pause', {}),
                ('POST', '/api/jobs/' + jobs[who]['id'] + '/resume', {}),
                ('DELETE', '/api/jobs/' + jobs[who]['id'], None),
            ]:
                assert api(read[who], path, method, body, 403, forged) == {'error': 'read-only API key'}
            for method, path in [('PUT', '/api/jobs/' + jobs[who]['id']), ('POST', '/api/tools'),
                                 ('GET', '/api/jobs?limit=6'), ('POST', '/internal/scheduler/dispatch')]:
                api(read[who], path, method, expected=404)
        # No body is supplied: rejection must precede body reading and 100 Continue.
        connection = http.client.HTTPConnection(config['bind'], timeout=2)
        connection.putrequest('POST', '/api/chat')
        connection.putheader('Authorization', 'Bearer ' + read['alice']['token'])
        connection.putheader('Content-Length', '524289')
        connection.putheader('Content-Type', 'text/plain')
        connection.putheader('Expect', '100-continue')
        connection.endheaders()
        assert connection.getresponse().status == 403
        connection.close()
        assert hold_snapshot() == before_hold and content_snapshot() == before_content
        assert len(model_calls) == before_models
        print('PASS: all exposed mutations denied before body/model/hold/content changes; tenant and header isolation')

        for who in ('alice', 'bob'):
            old = read[who]
            read[who] = cli('key-rotate', '--key', old['key_id'])
            assert read[who]['read_only'] is True and read[who]['user_id'] == old['user_id']
            api(old, '/api/sessions', expected=401)
            api(read[who], '/api/chat', 'POST', {}, 403)
            api(read[who], '/api/sessions/' + sessions[who])
            metadata = cli('key-list', '--user', old['user_id'])['keys']
            assert all(set(item) == {'key_id', 'user_id', 'read_only', 'created_ms', 'revoked_ms'} for item in metadata)
            assert next(item for item in metadata if item['key_id'] == old['key_id'])['revoked_ms'] is not None
            assert next(item for item in metadata if item['key_id'] == read[who]['key_id'])['read_only'] is True
            paged = cli('key-list', '--user', old['user_id'], '--limit', '1', '--offset', '1')['keys']
            assert len(paged) == 1 and paged[0] == metadata[1]
            for args in [('--limit', '0'), ('--limit', '101'), ('--offset', '1025')]:
                cli('key-list', '--user', old['user_id'], *args, ok=False)
        old_full = full['alice']
        full['alice'] = cli('key-rotate', '--key', old_full['key_id'])
        assert full['alice']['read_only'] is False
        api(old_full, '/api/sessions', expected=401)
        created = api(full['alice'], '/api/sessions', 'POST')['session_id']
        assert api(full['alice'], '/api/sessions/' + created, 'DELETE')['success'] is True
        revoked = cli('key-add', '--user', full['alice']['user_id'], '--read-only')
        cli('key-revoke', '--key', revoked['key_id'])
        api(revoked, '/api/sessions', expected=401)
        cli('key-rotate', '--key', revoked['key_id'], ok=False)
        cli('user-disable', '--user', full['alice']['user_id'])
        for key in (read['alice'], full['alice']):
            api(key, '/api/sessions', expected=401)
        cli('key-add', '--user', full['alice']['user_id'], '--read-only', ok=False)
        cli('user-enable', '--user', full['alice']['user_id'])
        api(read['alice'], '/api/sessions')
        api(revoked, '/api/sessions', expected=401)
        print('PASS: immutable privilege across rotation; bounded secret-free metadata; revocation and disable/enable')

        # A key's HTTP privilege does not revoke the user's prior cron authority.
        cli('key-revoke', '--key', full['bob']['key_id'])
        with sqlite3.connect(backends['bob']['db']) as connection:
            connection.execute('UPDATE jobs SET next_due_ms=? WHERE id=?',
                               (int(time.time() * 1000) + 300, jobs['bob']['id']))
        run = wait(lambda: next((item for item in api(read['bob'], '/api/jobs/' + jobs['bob']['id'] + '/runs')['items']
                                if item['status'] == 'completed'), None))
        assert run['response']['message']['content'].startswith('bob:')
        assert len(model_calls) == before_models + 1
        wait(lambda: hold_snapshot()[0] == 0)
        print('PASS: separately authorized user cron still executes when only read-only HTTP keys remain')

        stop(gateway)
        config['scheduled_jobs'] = False
        config_path.write_text(json.dumps(config))
        gateway = launch(['gateway', 'serve', '--config', str(config_path)], gateway_url, 'gateway')
        assert api(read['alice'], '/api/gateway/capabilities') == {'scheduled_jobs': False, 'read_only': True}
        assert api(full['alice'], '/api/gateway/capabilities') == {'scheduled_jobs': False, 'read_only': False}
        api(read['alice'], '/api/jobs', expected=404)
        api(read['alice'], '/api/jobs', 'POST', spec, 404)
        api(read['alice'], '/api/chat', 'POST', {}, 403)
        api(read['alice'], '/api/sessions/' + sessions['alice'])
        assert hold_snapshot()[0] == 0
        with lock:
            assert not model_errors, model_errors
        print('PASS: restart preserves access; disabled and unknown routes remain unavailable')
finally:
    for process in reversed(processes):
        stop(process)
    model.shutdown()
    model.server_close()
