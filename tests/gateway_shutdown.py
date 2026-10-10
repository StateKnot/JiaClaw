#!/usr/bin/env python3
# Copyright 2026 JiaClaw contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
"""Actual gateway/TCP/SIGTERM/SQLite boundaries with a local protocol backend.

No native model, vendor effect, OS-stuck filesystem or tenant SSE qualification.
The production 10s request timeout + 5s shared grace is never extended.
"""
import argparse
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

parser = argparse.ArgumentParser()
parser.add_argument('binary', nargs='?', default='target/debug/jiaclaw')
parser.add_argument('--only-slow', action='store_true')
args = parser.parse_args()
binary = str(Path(args.binary).resolve())
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
TOKEN = 'fixture-private-token-32-characters-long'


def until(predicate, seconds=5):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError('fixture barrier deadline exceeded')


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value):
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass  # A timed-out transport says nothing about the original effect.

    def do_GET(self):
        if self.headers.get('Authorization') != 'Bearer ' + TOKEN:
            self.server.errors.append('backend credential mismatch')
            self.send_error(403)
            return
        if self.path == '/health':
            self.reply({'agent_name': 'alice'})
        elif self.path == '/api/sessions':
            self.reply([{'id': 'a', 'message_count': 0, 'payload': 'x' * 1900000}])
        else:
            self.server.errors.append('unexpected backend GET')
            self.send_error(404)

    def do_POST(self):
        try:
            assert self.path == '/api/sessions'
            assert self.headers.get('Authorization') == 'Bearer ' + TOKEN
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            self.server.requests.append((self.headers['X-Request-ID'], body))
            self.server.entered.set()
            assert self.server.release.wait(20), 'backend release deadline exceeded'
            self.server.completed.set()
            self.reply({'session_id': 'created'})
        except Exception as error:
            self.server.errors.append(repr(error))
            self.send_error(500)


class Fixture:
    def __enter__(self):
        self.temp = tempfile.TemporaryDirectory(prefix='jiaclaw-gateway-shutdown-')
        self.root = Path(self.temp.name)
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.server.errors, self.server.requests = [], []
        self.server.entered = threading.Event()
        self.server.release = threading.Event()
        self.server.completed = threading.Event()
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            self.port = sock.getsockname()[1]
        token = self.root / 'token'
        token.write_text(TOKEN)
        token.chmod(0o600)
        self.config = self.root / 'config.json'
        self.config.write_text(json.dumps({
            'bind': f'127.0.0.1:{self.port}',
            'registry_path': str(self.root / 'registry' / 'users.sqlite3'),
            'request_timeout_seconds': 10, 'max_in_flight': 4,
            'backends': [{'id': 'alice', 'url': f'http://127.0.0.1:{self.server.server_port}/',
                          'token_file': str(token)}]}))
        self.user = self.cli('user-add', '--backend', 'alice')
        self.sockets, self.process = [], None
        self.log = self.root / 'gateway.log'
        self.output = self.log.open('ab')
        self.start()
        return self

    def cli(self, *words):
        result = subprocess.run([binary, 'gateway', *words, '--config', str(self.config)],
                                env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    def start(self):
        self.process = subprocess.Popen([binary, 'gateway', 'serve', '--config', str(self.config)],
                                        env=env, stdout=self.output, stderr=self.output)
        def ready():
            assert self.process.poll() is None, self.log.read_text()
            try:
                with socket.create_connection(('127.0.0.1', self.port), timeout=.2):
                    return True
            except OSError:
                return False
        until(ready, 15)

    def request(self, method='GET', body=None):
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=13)
        try:
            connection.request(method, '/api/sessions', body=json.dumps(body) if body else None,
                               headers={'Authorization': 'Bearer ' + self.user['token'],
                                        'Content-Type': 'application/json'})
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), json.loads(response.read())
        finally:
            connection.close()

    def socket(self, slow=False):
        sock = socket.socket()
        if slow:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
        sock.settimeout(3)
        sock.connect(('127.0.0.1', self.port))
        self.sockets.append(sock)
        return sock

    def slow_downloads(self):
        for _ in range(4):
            sock = self.socket(slow=True)
            sock.sendall((f'GET /api/sessions HTTP/1.1\r\nHost: localhost\r\n'
                          f'Authorization: Bearer {self.user["token"]}\r\n\r\n').encode())
            received = b''
            while b'\r\n\r\n' not in received:
                chunk = sock.recv(512)
                assert chunk, 'closed before download headers'
                received += chunk
            assert received.split(b'\r\n', 1)[0] == b'HTTP/1.1 200 OK', received[:128]
            # Leave the real response body unread; OS send buffers must drain.

    def stop_signal(self):
        start = time.monotonic()
        self.process.terminate()
        until(lambda: 'shared shutdown grace started' in self.log.read_text())
        return start

    def stopped(self, start, bounded=17):
        self.process.wait(timeout=max(.1, bounded - (time.monotonic() - start)))
        elapsed = time.monotonic() - start
        assert self.process.returncode == 0, self.log.read_text()
        assert elapsed <= bounded, elapsed
        return elapsed

    def hold(self):
        users = self.cli('user-list')['users']
        return next(u['hold'] for u in users if u['user_id'] == self.user['user_id'])

    def __exit__(self, kind, error, _):
        self.server.release.set()
        for sock in self.sockets:
            sock.close()
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.server.shutdown()
        self.server.server_close()
        self.output.close()
        if kind:
            print(self.log.read_text(), flush=True)
        assert not self.server.errors, self.server.errors
        self.temp.cleanup()


with Fixture() as f:
    f.slow_downloads()
    start = time.monotonic()
    # Baseline binary has no new log barrier. The external bound is identical.
    f.process.terminate()
    elapsed = f.stopped(start)
    assert 'shutdown grace exhausted' in f.log.read_text(), f.log.read_text()
    assert f.hold() is None and not f.server.requests
    print(f'1/4 slow TCP downloads share the original 15s grace: {elapsed:.3f}s', flush=True)

if not args.only_slow:
    with Fixture() as f:
        result, errors = [], []
        def post():
            try:
                result.append(f.request('POST', {'mode': 'settle'}))
            except Exception as error:
                errors.append(repr(error))
        worker = threading.Thread(target=post)
        worker.start()
        assert f.server.entered.wait(5)
        original = f.hold()['request_id']
        start = f.stop_signal()
        f.server.release.set()
        worker.join(timeout=5)
        assert not worker.is_alive() and not errors, errors
        assert result[0][0] == 200 and result[0][2] == {'session_id': 'created'}, result
        elapsed = f.stopped(start, 5)
        assert f.server.requests == [(original, {'mode': 'settle'})]
        assert f.hold() is None
        assert 'HTTP and worker drain completed' in f.log.read_text()
        print(f'2/4 admitted effect settles once during drain: {elapsed:.3f}s', flush=True)

    with Fixture() as f:
        result, errors = [], []
        def post_unknown():
            try:
                result.append(f.request('POST', {'mode': 'uncertain'}))
            except Exception as error:
                errors.append(repr(error))
        worker = threading.Thread(target=post_unknown)
        worker.start()
        assert f.server.entered.wait(5)
        original = f.hold()['request_id']
        start = f.stop_signal()
        worker.join(timeout=13)
        assert not worker.is_alive() and not errors, errors
        assert result[0][0] == 502, result
        f.stopped(start)
        hold = f.hold()
        assert hold['state'] == 'needs_review' and hold['request_id'] == original, hold
        f.start()
        assert f.request('POST', {'mode': 'new'})[0] == 409
        assert f.server.requests == [(original, {'mode': 'uncertain'})]
        assert not f.server.completed.is_set(), 'restart must not imply original backend idle'
        assert f.hold()['request_id'] == original
        f.server.release.set()
        assert f.server.completed.wait(5)
        assert f.hold()['state'] == 'needs_review'
        f.stopped(f.stop_signal(), 5)
        print('3/4 uncertain original hold survives stop/restart; no replay or inferred idleness', flush=True)

    with Fixture() as f:
        sock = f.socket()
        sock.sendall((f'POST /api/sessions HTTP/1.1\r\nHost: localhost\r\n'
                      f'Authorization: Bearer {f.user["token"]}\r\n'
                      'Content-Type: application/json\r\nContent-Length: 2\r\n'
                      'Expect: 100-continue\r\n\r\n').encode())
        # Hyper sends 100 only when the handler polls the request body, after
        # authentication and both permits. A competing GET can steal that lane
        # before authentication finishes and is not a valid admission barrier.
        interim = b''
        while b'\r\n\r\n' not in interim:
            chunk = sock.recv(512)
            assert chunk, 'closed before body polling barrier'
            interim += chunk
        assert interim == b'HTTP/1.1 100 Continue\r\n\r\n', interim
        sock.sendall(b'{')
        start = f.stop_signal()
        sock.sendall(b'}')
        response = b''
        while b'\r\n\r\n' not in response:
            chunk = sock.recv(512)
            assert chunk, 'late request closed before explicit rejection'
            response += chunk
        assert response.split(b'\r\n', 1)[0] == b'HTTP/1.1 503 Service Unavailable', response
        sock.close()
        f.stopped(start, 5)
        assert f.hold() is None and not f.server.requests
        print('4/4 pre-signal partial body cannot acquire durable admission after close', flush=True)
