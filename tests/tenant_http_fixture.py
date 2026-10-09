#!/usr/bin/env python3
"""Shared isolated tenant HTTP process/model fixture.

No paid credentials, SSE delivery, upstream durable resume or supplier claim.
"""
import contextlib
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
keys = {who: 'fixture-model-' + uuid.uuid4().hex for who in ('alice', 'bob')}
lock = threading.Lock()
posts, results, gates, faults, processes = [], {}, {}, [], []


def wait(check, label, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with lock:
            assert not faults, faults
        value = check()
        if value:
            return value
        time.sleep(.03)
    raise AssertionError(label + ' timed out; posts=' + repr([(p['who'], p['prompt']) for p in posts]))


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def sql(path, statement, params=(), write=False):
    with contextlib.closing(sqlite3.connect(path, timeout=6)) as c:
        c.row_factory = sqlite3.Row
        rows = [dict(r) for r in c.execute(statement, params)]
        if write:
            c.commit()
        return rows


def request(number, path, token=None, method='GET', body=None, marker=False):
    c = http.client.HTTPConnection('127.0.0.1', number, timeout=10)
    headers = {'Authorization': 'Bearer ' + token} if token else {}
    if marker:
        headers['x-jiaclaw-gateway-turns'] = '1'
    data = json.dumps(body, ensure_ascii=False).encode() if body is not None else None
    if data is not None:
        headers['Content-Type'] = 'application/json'
    c.request(method, path, data, headers)
    r = c.getresponse()
    raw = r.read()
    status, response_headers = r.status, r.headers
    c.close()
    assert len(raw) <= 2 * 1024 * 1024 + 32 * 1024
    for secret in keys.values():
        assert secret.encode() not in raw
    return status, json.loads(raw) if raw else None, response_headers


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        try:
            who = next(w for w, k in keys.items() if self.headers.get('Authorization') == 'Bearer ' + k)
            remote = self.path.split('/')[3]
            with lock:
                owner, receipt = results[remote]
            assert who == owner
            value = receipt if self.path.endswith('/result') else {'id': remote, 'model': receipt['model'], 'purpose': 'model', 'status': 'succeeded'}
            data = json.dumps(value).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as e:
            faults.append(repr(e))

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            who = next(w for w, k in keys.items() if self.headers.get('Authorization') == 'Bearer ' + k)
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert body['stream'] is True and self.headers['Accept'] == 'text/event-stream'
            operation = self.headers['Idempotency-Key']
            assert str(uuid.UUID(operation)) == operation
            prompt = next(m['content'] for m in reversed(body['messages']) if m['role'] == 'user')
            remote = str(uuid.uuid4())
            answer = who + ': ' + prompt
            receipt = {'id': 'fixture', 'object': 'chat.completion', 'created': 1, 'model': body['model'],
                'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': answer}, 'finish_reason': 'stop'}],
                'usage': {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5}}
            with lock:
                assert not any(p['operation'] == operation for p in posts), 'model dispatch replayed'
                posts.append({'who': who, 'prompt': prompt, 'operation': operation, 'remote': remote})
                results[remote] = (who, receipt)
                gate = gates.get((who, prompt))
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('x-brokerrouter-request-id', remote)
            self.send_header('Connection', 'close')
            self.end_headers()
            self.close_connection = True

            def send(delta, finish=None, usage=None):
                value = {'id': 'fixture', 'object': 'chat.completion.chunk', 'created': 1, 'model': body['model'],
                         'choices': [] if usage else [{'index': 0, 'delta': delta, 'finish_reason': finish}], 'usage': usage}
                self.wfile.write(('data: ' + json.dumps(value) + '\r\n\r\n').encode())
                self.wfile.flush()
            send({'role': 'assistant', 'content': who + ': '})
            if gate:
                gate['ready'].set()
                assert gate['release'].wait(20), 'fixture model gate expired'
            send({'content': prompt}, 'stop')
            send({}, usage={'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 5})
            self.wfile.write(b'data: [DONE]\r\n\r\n')
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as e:
            faults.append(repr(e))


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
threading.Thread(target=model.serve_forever, daemon=True).start()


def stop(p, kill=False):
    if p and p.poll() is None:
        p.kill() if kill else p.terminate()
        code = p.wait(timeout=8)
        if not kill:
            assert code == 0, code


def launch(args, number, log):
    with log.open('ab') as output:
        p = subprocess.Popen([str(binary), *args], env=env, stdout=output, stderr=output)
    processes.append(p)
    def healthy():
        assert p.poll() is None, log.read_text()
        try:
            return request(number, '/health')[0] == 200
        except (OSError, http.client.HTTPException):
            return False
    try:
        wait(healthy, 'actual server startup')
    except Exception as error:
        raise AssertionError(str(error) + '\nActual server log:\n' + log.read_text()) from error
    return p
