#!/usr/bin/env python3
"""Real gateway binary, local protocol backends; no paid or external writes."""
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
observed, errors = [], []
started, release, completed = threading.Event(), threading.Event(), threading.Event()

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body, **headers):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Set-Cookie', 'backend-private-cookie=value')
        for key, value in headers.items():
            self.send_header(key, value)
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def handle_request(self):
        try:
            assert self.headers.get('Authorization') == 'Bearer ' + self.server.token
            assert not self.headers.get('Cookie')
            assert not self.headers.get('X-Tenant-ID')
            assert not self.headers.get('X-Forwarded-For')
            if self.path == '/health':
                self.reply(200, {'agent_name': self.server.identity})
                return
            assert self.headers.get('X-Request-ID') != 'attacker-chosen-id'
            data = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            body = json.loads(data) if data else {}
            observed.append((self.server.identity, self.command, self.path, body))
            mode = body.get('mode')
            if mode == 'hold':
                started.set()
                assert release.wait(30), 'fixture hold exceeded deadline'
                completed.set()
            if mode == 'oversize':
                self.reply(200, {'payload': 'X' * (2 * 1024 * 1024)})
            elif mode == 'redirect':
                self.reply(307, {'secret': 'do-not-relay'}, Location=self.server.other_url + '/redirect-target')
            elif mode == 'malformed':
                self.reply(200, {})
            elif mode == 'error':
                self.reply(500, {'secret': 'do-not-relay'})
            else:
                self.reply(200, {'tenant': self.server.identity, 'session_id': body.get('session_id', 'created'), 'messages': [], 'id': 'same', 'message': {'role': 'assistant', 'content': 'done'}, 'status': 'requireshumaninput' if mode == 'incomplete' else 'completed', 'tool_calls': []})
        except Exception as error:
            errors.append(repr(error))
            self.reply(500, {'error': 'fixture failed'})
    do_GET = do_POST = do_DELETE = handle_request


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait_until(predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.04)
    raise AssertionError('fixture deadline exceeded')


with tempfile.TemporaryDirectory(prefix='jiaclaw-user-gateway-') as temp:
    root = Path(temp)
    servers = []
    process = None
    config_path = root / 'gateway.json'
    config = {'bind': f'127.0.0.1:{port()}', 'registry_path': str(root / 'registry' / 'users.sqlite3'),
              'request_timeout_seconds': 10, 'max_in_flight': 4, 'backends': []}
    url = 'http://' + config['bind']
    env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
    # Proxy environment must never reroute authenticated tenant traffic.
    env['HTTP_PROXY'] = env['HTTPS_PROXY'] = env['ALL_PROXY'] = 'http://127.0.0.1:1'
    env['NO_PROXY'] = ''
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def cli(*args, ok=True):
        result = subprocess.run([str(binary), 'gateway', *args, '--config', str(config_path)], env=env, text=True, capture_output=True, timeout=15)
        if ok:
            assert result.returncode == 0, result.stderr
            return json.loads(result.stdout)
        assert result.returncode != 0, result.stdout
        return result

    def request(path='/api/sessions', token=None, method='GET', body=None, headers=None):
        headers = dict(headers or {})
        if token:
            headers['Authorization'] = 'Bearer ' + token
        if body is not None:
            headers['Content-Type'] = 'application/json'
            body = json.dumps(body).encode()
        req = urllib.request.Request(url + path, data=body, headers=headers, method=method)
        try:
            response = client.open(req, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read()
            assert not response.headers.get('Set-Cookie')
            assert not response.headers.get('Location')
            return response.status, json.loads(raw), response.headers

    def start():
        log = open(root / 'gateway.log', 'ab')
        proc = subprocess.Popen([str(binary), 'gateway', 'serve', '--config', str(config_path)], env=env, stdout=log, stderr=log)
        log.close()
        def ready():
            if proc.poll() is not None:
                raise AssertionError((root / 'gateway.log').read_text())
            try:
                return request('/health')[0] == 200
            except (OSError, urllib.error.URLError):
                return False
        wait_until(ready)
        return proc

    def chat(key, **extra):
        return request('/api/chat', key, 'POST', {'session_id': 'same', 'messages': [{'role': 'user', 'content': 'hello'}], **extra})

    def clear(user):
        cli('review-clear', '--user', user, '--confirm-backend-idle', '--note', 'Fixture backend completion checked; no replay')

    try:
        for identity in ['alice', 'bob']:
            server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            server.daemon_threads = True
            server.identity, server.token = identity, 'fixture-backend-' + uuid.uuid4().hex
            secret = root / (identity + '.secret')
            secret.write_text(server.token)
            backend_url = f'http://127.0.0.1:{server.server_port}/'
            config['backends'].append({'id': identity, 'url': backend_url, 'token_file': str(secret)})
            servers.append(server)
            threading.Thread(target=server.serve_forever, daemon=True).start()
        servers[0].other_url = config['backends'][1]['url'].rstrip('/')
        config_path.write_text(json.dumps(config))
        alice = cli('user-add', '--backend', 'alice')
        bob = cli('user-add', '--backend', 'bob')
        assert alice['token'] != bob['token']
        cli('user-add', '--backend', 'alice', ok=False)
        process = start()
        cli('serve', ok=False)  # one process owns restart recovery for a registry
        assert request()[0] == 401
        assert request(token='invalid')[0] == 401
        for key, who in [(alice['token'], 'alice'), (bob['token'], 'bob')]:
            status, data, headers = chat(key)
            assert status == 200 and data['tenant'] == who
            assert headers['Cache-Control'] == 'no-store'
        assert request(token=alice['token'], headers={'X-Tenant-ID': 'bob', 'Cookie': 'private=secret', 'X-Forwarded-For': 'forged', 'X-Request-ID': 'attacker-chosen-id'})[1]['tenant'] == 'alice'
        assert request(token=alice['token'], headers={'X-Api-Token': alice['token']})[0] == 401
        connection = http.client.HTTPConnection(config['bind'])
        connection.putrequest('GET', '/api/sessions')
        connection.putheader('Authorization', 'Bearer ' + alice['token'])
        connection.putheader('Authorization', 'Bearer ' + alice['token'])
        connection.endheaders()
        assert connection.getresponse().status == 401
        connection.close()
        count = len(observed)
        for path in ['/api/jobs', '/metrics', '/api/tools', '/api/sessions/%2e%2e', '/api/sessions/a%2fb', '/api/sessions/a/export?format=json&format=json', '/api/sessions?tenant=bob']:
            assert request(path, alice['token'])[0] == 404, path
        assert len(observed) == count
        assert chat(alice['token'], stream=True)[0] == 400
        assert request('/api/chat', alice['token'], 'POST', {'messages': []})[0] == 400
        assert chat(alice['token'], padding='X' * (512 * 1024))[0] == 413
        assert request('/api/sessions/import', alice['token'], 'POST', {'id': 'a/b', 'messages': []})[0] == 400
        assert len(observed) == count
        old = alice
        alice = cli('key-rotate', '--key', old['key_id'])
        assert alice['user_id'] == old['user_id']
        assert request(token=old['token'])[0] == 401
        assert chat(alice['token'])[0] == 200
        # Revocation during a slow body must be checked again at write admission.
        raw = json.dumps({'session_id': 'same', 'messages': [{'role': 'user', 'content': 'hello'}]}).encode()
        connection = http.client.HTTPConnection(config['bind'])
        connection.putrequest('POST', '/api/chat')
        connection.putheader('Authorization', 'Bearer ' + alice['token'])
        connection.putheader('Content-Type', 'application/json')
        connection.putheader('Content-Length', str(len(raw)))
        connection.putheader('Expect', '100-continue')
        connection.endheaders()
        # Hyper sends Continue only when the authenticated handler starts reading
        # the body. Avoid racing a probing GET against the request's admission.
        connection.sock.settimeout(5)
        interim = b''
        while not interim.endswith(b'\r\n\r\n'):
            chunk = connection.sock.recv(1)
            assert chunk, 'gateway closed before body admission'
            interim += chunk
        assert interim.startswith(b'HTTP/1.1 100 Continue'), interim
        wait_until(lambda: request(token=alice['token'])[0] == 429)
        count = len(observed)
        alice = cli('key-rotate', '--key', alice['key_id'])
        connection.send(raw)
        assert connection.getresponse().status == 401
        connection.close()
        assert len(observed) == count
        assert chat(alice['token'])[0] == 200
        additional = cli('key-add', '--user', alice['user_id'])
        assert request(token=additional['token'])[0] == 200
        cli('key-revoke', '--key', additional['key_id'])
        assert request(token=additional['token'])[0] == 401
        cli('user-disable', '--user', alice['user_id'])
        assert request(token=alice['token'])[0] == 401
        assert chat(bob['token'])[0] == 200
        cli('user-enable', '--user', alice['user_id'])
        # Client disconnect does not cancel admitted backend work or release permits.
        raw = json.dumps({'session_id': 'same', 'mode': 'hold', 'messages': [{'role': 'user', 'content': 'hello'}]}).encode()
        connection = http.client.HTTPConnection(config['bind'])
        connection.request('POST', '/api/chat', raw, {'Authorization': 'Bearer ' + alice['token'], 'Content-Type': 'application/json'})
        assert started.wait(5)
        connection.close()
        assert chat(alice['token'])[0] == 429
        assert chat(bob['token'])[0] == 200
        release.set()
        assert completed.wait(5)
        wait_until(lambda: request(token=alice['token'])[0] == 200)
        assert chat(alice['token'])[0] == 200
        # A crash leaves a durable hold; startup never retries it.
        started.clear(); release.clear(); completed.clear()
        connection = http.client.HTTPConnection(config['bind'])
        connection.request('POST', '/api/chat', raw, {'Authorization': 'Bearer ' + alice['token'], 'Content-Type': 'application/json'})
        assert started.wait(5)
        process.kill(); process.wait(timeout=5); connection.close()
        release.set(); assert completed.wait(5)
        count = len(observed)
        process = start()
        assert len(observed) == count
        assert chat(alice['token'])[0] == 409
        assert request(token=alice['token'])[0] == 200
        assert chat(bob['token'])[0] == 200
        cli('review-clear', '--user', alice['user_id'], '--note', 'missing confirmation', ok=False)
        clear(alice['user_id'])
        assert chat(alice['token'])[0] == 200
        for mode in ['redirect', 'oversize', 'error', 'malformed']:
            status, data, _ = chat(alice['token'], mode=mode)
            assert status == 502, (mode, status, data)
            assert 'do-not-relay' not in json.dumps(data)
            assert chat(alice['token'])[0] == 409
            assert chat(bob['token'])[0] == 200
            clear(alice['user_id'])
        started.clear(); release.clear(); completed.clear()
        assert chat(alice['token'], mode='hold')[0] == 502
        assert started.is_set() and not completed.is_set()
        assert chat(alice['token'])[0] == 409
        assert chat(bob['token'])[0] == 200
        release.set(); assert completed.wait(5)
        clear(alice['user_id'])
        assert request('/api/sessions/import?id=same', alice['token'], 'POST', {'id': 'body-other', 'messages': []})[0] == 200
        for path, body in [('/api/sessions/import', {'id': 'other', 'messages': []}),
                           ('/api/sessions/import?id=other', {'id': 'same', 'messages': []})]:
            assert request(path, alice['token'], 'POST', body)[0] == 502
            assert chat(alice['token'])[0] == 409
            clear(alice['user_id'])
        assert chat(alice['token'], mode='incomplete')[2]['x-jiaclaw-write-review'] == 'required'
        assert chat(alice['token'])[0] == 409
        clear(alice['user_id'])
        assert not any(item[2] == '/redirect-target' for item in observed)
        summaries = cli('user-list')
        assert alice['token'] not in json.dumps(summaries)
        for file in (root / 'registry').iterdir():
            if file.is_file():
                data = file.read_bytes()
                assert alice['token'].encode() not in data and bob['token'].encode() not in data
        assert not errors, errors
        print('user gateway: isolation, live key lifecycle, canonical routes, bounded IO, cancellation and crash holds passed')
    finally:
        release.set()
        if process and process.poll() is None:
            process.terminate()
            process.wait(timeout=20)
        for server in servers:
            server.shutdown()
            server.server_close()
