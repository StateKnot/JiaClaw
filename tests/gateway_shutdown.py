#!/usr/bin/env python3
# Copyright 2026 JiaClaw contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
"""Actual gateway/TCP/SIGTERM/SQLite boundaries with a local protocol backend.

No native model, vendor effect, OS-stuck filesystem or tenant SSE qualification.
The production 10s request timeout + 5s shared grace is never extended.
"""
import argparse
from datetime import datetime
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from host_log import read_running_log

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
        began = time.monotonic()
        deadline = began + 15
        attempts = 0
        elapsed_ms = 0
        def ready():
            nonlocal attempts, elapsed_ms
            assert self.process.poll() is None, self.log.read_text()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            connection = http.client.HTTPConnection('127.0.0.1', self.port,
                                                    timeout=min(.2, remaining))
            try:
                attempts += 1
                connection.request('GET', '/health')
                response = connection.getresponse()
                body = response.read(65)
                observed = time.monotonic()
                assert observed < deadline, 'gateway HTTP readiness exceeded 15s'
                assert response.status == 200 and body == b'{"status":"ok"}', (response.status, body)
                elapsed_ms = int((observed - began) * 1000)
                return True
            except (OSError, http.client.HTTPException):
                return False
            finally:
                connection.close()
        # A listening socket can queue requests before axum is polled. Keep
        # startup inside its original deadline before spending a header budget.
        until(ready, max(0, deadline - time.monotonic()))
        print('ACTUAL gateway HTTP readiness ' + json.dumps({
            'protocol': 1, 'pid': self.process.pid, 'path': '/health', 'status': 200,
            'body_sha256': hashlib.sha256(b'{"status":"ok"}').hexdigest(),
            'body_bytes': 15, 'deadline_ms': 15000,
            'elapsed_ms': elapsed_ms, 'attempts': attempts,
        }), flush=True)

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
        previous = read_running_log(self.log)
        start = time.monotonic()
        wall_start = time.time()
        self.signal_sent = start
        self.process.terminate()
        # The real shared deadline starts when shutdown() is handled, not when
        # Python sends the signal. Signal observation has its own existing 5s
        # barrier; only the slow-response test measures drain from this phase.
        receipt = []
        def closing_receipt():
            current = read_running_log(self.log)
            assert time.monotonic() - start <= 5, 'shutdown receipt observation deadline exceeded'
            assert current.startswith(previous), 'fixture log changed before shutdown receipt'
            lines = [line for line in current[len(previous):].splitlines()
                     if 'shared shutdown grace started' in line]
            if not lines:
                return False
            assert len(lines) == 1, 'multiple shutdown receipts for this signal'
            receipt.append(lines[0])
            return True
        until(closing_receipt)
        self.grace_observed = time.monotonic()
        # Use the event's UTC time from the same host, paired with the signal's
        # monotonic clock. A delayed read must not grant a fresh drain budget.
        plain = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', receipt[0])
        stamp = datetime.fromisoformat(plain.split()[0].replace('Z', '+00:00'))
        assert stamp.tzinfo is not None, 'shutdown receipt must have an explicit timezone'
        self.grace_started = start + (stamp.timestamp() - wall_start)
        assert start <= self.grace_started <= self.grace_observed, 'inconsistent shutdown clocks'
        return start

    def stopped(self, start, bounded=17):
        try:
            self.process.wait(timeout=max(.1, bounded - (time.monotonic() - start)))
        except subprocess.TimeoutExpired as error:
            raise AssertionError({
                'phase_elapsed_seconds': time.monotonic() - start,
                'phase_bound_seconds': bounded,
                'signal_to_grace_seconds': getattr(self, 'grace_observed', start)
                    - getattr(self, 'signal_sent', start),
                'receipt_read_delay_seconds': getattr(self, 'grace_observed', start)
                    - getattr(self, 'grace_started', start),
                'settled': self.process.poll() is not None,
            }) from error
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
    f.stop_signal()
    elapsed = f.stopped(f.grace_started)
    assert 'shutdown grace exhausted' in f.log.read_text(), f.log.read_text()
    assert f.hold() is None and not f.server.requests
    print(f'1/4 slow TCP downloads share the original 15s grace: {elapsed:.3f}s; '
          f'signal observation: {f.grace_observed - f.signal_sent:.3f}s', flush=True)

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

    with Fixture() as f:
        f.stopped(f.stop_signal(), 5)
        f.start()  # Keep the prior shutdown marker in the append-only log.
        f.slow_downloads()
        # Suspend only this fixture's child before it can observe SIGTERM.
        # This reproduces the old mixed-clock assertion; it does not identify
        # a historical CI scheduling cause or certify physical signal latency.
        os.kill(f.process.pid, signal.SIGSTOP)
        resume = threading.Timer(2.3, lambda: os.kill(f.process.pid, signal.SIGCONT))
        resume.start()
        try:
            f.stop_signal()
            assert 2 <= f.grace_started - f.signal_sent < 5
            elapsed = f.stopped(f.grace_started)
            assert 'shutdown grace exhausted' in f.log.read_text(), f.log.read_text()
            assert f.hold() is None and not f.server.requests
            print(f'5/5 delayed signal observation retains the original 15s shared grace: '
                  f'{elapsed:.3f}s; no admission/effect or new production deadline', flush=True)
        finally:
            resume.join(timeout=5)
            assert not resume.is_alive()

    with Fixture() as f:
        f.slow_downloads()
        original_read = read_running_log
        resume = None
        def delayed_receipt(path):
            global resume
            value = original_read(path)
            if 'shared shutdown grace started' in value and resume is None:
                # Hold only this child across the drain allowance. Reading the
                # receipt late must not let the same event pass as a fresh timer.
                os.kill(f.process.pid, signal.SIGSTOP)
                resume = threading.Timer(18, lambda: os.kill(f.process.pid, signal.SIGCONT))
                resume.start()
                time.sleep(3)
            return value
        read_running_log = delayed_receipt
        try:
            f.stop_signal()
            assert f.grace_observed - f.grace_started >= 3
            try:
                f.stopped(f.grace_started)
            except AssertionError as error:
                assert isinstance(error.__cause__, subprocess.TimeoutExpired), repr(error)
                assert f.process.poll() is None, 'controlled child settled before its timer'
            else:
                raise AssertionError('delayed receipt incorrectly granted a new drain allowance')
            resume.join(timeout=5)
            assert not resume.is_alive()
            assert f.process.wait(timeout=5) == 0, f.log.read_text()
            assert 'shutdown grace exhausted' in f.log.read_text(), f.log.read_text()
            assert f.hold() is None and not f.server.requests
            print('6/6 delayed log receipt cannot reset the original drain allowance; '
                  'controlled child settles without admission/effect', flush=True)
        finally:
            read_running_log = original_read
            if resume is not None:
                resume.cancel()
                if f.process.poll() is None:
                    os.kill(f.process.pid, signal.SIGCONT)
                resume.join(timeout=5)
                assert not resume.is_alive()

    with Fixture() as f:
        original_read = read_running_log
        delayed = False
        def expired_receipt(path):
            global delayed
            value = original_read(path)
            if 'shared shutdown grace started' in value and not delayed:
                delayed = True
                time.sleep(6)  # Return a valid receipt after its original 5s observation window.
            return value
        read_running_log = expired_receipt
        try:
            try:
                f.stop_signal()
            except AssertionError as error:
                assert str(error) == 'shutdown receipt observation deadline exceeded', repr(error)
                assert delayed
            else:
                raise AssertionError('late receipt bypassed the original observation deadline')
            assert f.process.wait(timeout=5) == 0, f.log.read_text()
            assert f.hold() is None and not f.server.requests
            print('7/7 a late log read cannot bypass the original signal observation deadline; '
                  'controlled child settles without admission/effect', flush=True)
        finally:
            read_running_log = original_read
