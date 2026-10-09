#!/usr/bin/env python3
"""Actual HTTP body delivery with original identities and local model settlement.

No paid credentials, Web/tenant streaming, proxy or supplier certification.
"""
import http.client
import json
from pathlib import Path
import socket
import tempfile
import time
import uuid

import http_turns as f


class Stream:
    def __init__(self, app, case, identity=None, body=None):
        self.id = identity or str(uuid.uuid4())
        self.body = body or {'session_id': 'http:' + str(uuid.uuid4()), 'prompt': case,
                             'enabled_tools': ['file_write']}
        self.connection = http.client.HTTPConnection('127.0.0.1', app.port, timeout=15)
        self.connection.request('PUT', '/api/turns/' + self.id + '/stream',
                                json.dumps(self.body).encode(),
                                {'Authorization': 'Bearer ' + f.api_token, 'Content-Type': 'application/json'})
        self.response = self.connection.getresponse()
        assert self.response.status == 202, self.response.read()
        assert self.response.getheader('Content-Type') == 'text/event-stream; charset=utf-8'
        assert self.response.getheader('Cache-Control') == 'no-store'
        assert self.response.getheader('X-Accel-Buffering') == 'no'
        self.wire = 0

    def next(self):
        name, data = None, None
        while True:
            line = self.response.readline(2 * 1024 * 1024 + 20 * 1024)
            self.wire += len(line)
            assert self.wire <= 12 * 1024 * 1024
            assert f.api_token.encode() not in line and f.provider_key.encode() not in line
            if not line:
                assert name is None and data is None, 'truncated frame requires original lookup'
                return None
            if line.startswith(b':'):
                continue
            if line in (b'\r\n', b'\n'):
                if name is None:
                    continue
                assert data is not None
                value = json.loads(data)
                assert value['event'] == name
                return value
            if line.startswith(b'event: '):
                assert name is None
                name = line[7:].strip().decode('ascii')
            elif line.startswith(b'data: '):
                assert data is None
                data = line[6:].strip().decode('utf-8')
            else:
                raise AssertionError('unexpected SSE field')

    def early(self, app, gate):
        events = []
        while not events or events[-1]['event'] != 'preview':
            value = self.next()
            assert value and value['event'] not in ('error', 'done')
            events.append(value)
        assert gate['ready'].wait(10)
        assert events[0]['event'] == 'admitted' and events[0]['receipt']['id'] == self.id
        assert events[0]['receipt']['state'] == 'running'
        started = next(e for e in events if e['event'] == 'model_started')
        assert started['turn_id'] == self.id and started['round'] == 0
        assert app.ledger()[0]['turn_id'] == self.id and app.ledger()[0]['state'] == 'submitting'
        assert app.http('/api/turns/' + self.id)[1]['active']
        assert not app.history(self.body['session_id'])
        assert not (app.workspace / (self.body['prompt'] + '.txt')).exists()
        return events

    def finish(self):
        events = []
        while (value := self.next()) is not None:
            events.append(value)
        self.close()
        return events

    def close(self):
        self.response.close()
        self.connection.close()


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-http-stream-') as temporary:
        root = Path(temporary)
        app = f.App(root, 'stream-auth'); app.start()
        capabilities = app.http('/api/turns/capabilities')[1]
        assert capabilities['streaming'] is True and capabilities['stream_suffix'] == '/stream'
        assert capabilities['max_stream_wire_bytes'] == 12 * 1024 * 1024
        identity = str(uuid.uuid4())
        sock = socket.create_connection(('127.0.0.1', app.port)); sock.settimeout(2)
        sock.sendall((f'PUT /api/turns/{identity}/stream HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer wrong-secret\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n' + '{').encode())
        assert b'401' in sock.recv(4096).split(b'\r\n', 1)[0]; sock.close()
        assert app.http('/api/turns/' + identity + '/stream', 'PUT', {'padding': 'x' * 65536})[0] == 413
        assert app.http('/api/turns/' + identity + '/stream', 'PUT', {})[0] == 400
        assert not app.sql('SELECT * FROM http_turns') and not f.posts
        app.stop()
        print('PASS: streaming route authenticates before body and rejects malformed/oversized input before effects', flush=True)

        app = f.App(root, 'stream-tools'); app.start(); gate = f.gate('stream-tools')
        stream = Stream(app, 'stream-tools'); events = stream.early(app, gate)
        assert app.http('/api/turns/' + stream.id + '/stream', 'PUT', stream.body)[0] == 200
        assert app.http('/api/turns/' + stream.id + '/stream', 'PUT', dict(stream.body, prompt='conflict'))[0] == 409
        assert f.count('stream-tools') == 1
        assert app.http('/api/turns/' + stream.id, 'PUT', stream.body)[0] == 200
        gate['release'].set(); events += stream.finish()
        assert events[-1]['event'] == 'done'
        receipt = app.terminal(stream.id)
        assert events[-1]['receipt'] == receipt and receipt['session_committed']
        assert receipt['result']['reply'] == '最终🦀回复' and f.count('stream-tools') == 2
        assert (app.workspace / 'stream-tools.txt').read_text() == 'exactly one authorized effect'
        assert [e['round'] for e in events if e['event'] == 'model_started'] == [0, 1]
        tool = next(i for i, e in enumerate(events) if e['event'] == 'tool_completed')
        assert events[tool]['tool_name'] == 'file_write' and events[tool]['tool_call_id'] == 'call-0'
        assert events[tool - 1]['event'] == 'model_completed'
        assert 'exactly one authorized effect' not in json.dumps(events)
        assert len(json.loads(app.history(stream.body['session_id'])[0]['messages'])) == 2
        failed = Stream(app, 'tool-failure'); failed_events = failed.finish()
        failed_receipt = app.terminal(failed.id, 'needs_review')
        assert failed_events[-1]['event'] == 'done' and failed_events[-1]['receipt'] == failed_receipt
        assert failed_receipt['session_committed'] and failed_receipt['result']['status'] != 'completed'
        assert len(json.loads(app.history(failed.body['session_id'])[0]['messages'])) == 2
        assert f.count('tool-failure') == 1 and not (app.workspace / 'must-not-run.txt').exists()
        app.stop()
        print('PASS: actual preview before gated settlement, native two-round delivery and done only after atomic session commit', flush=True)

        app = f.App(root, 'stream-cancel'); app.start(); gate = f.gate('stream-cancel')
        stream = Stream(app, 'stream-cancel'); stream.early(app, gate)
        assert app.http('/api/turns/' + stream.id + '/cancel', 'POST')[1]['receipt']['cancel_requested']
        assert app.http('/api/turns/' + stream.id)[1]['active'] and app.ledger()[0]['state'] == 'submitting'
        gate['release'].set(); events = stream.finish(); receipt = app.terminal(stream.id, 'needs_review')
        assert events[-1]['event'] == 'done' and events[-1]['receipt'] == receipt
        assert receipt['cancel_requested'] and not receipt['session_committed']
        assert app.ledger()[0]['state'] == 'completed' and f.count('stream-cancel') == 1
        assert not (app.workspace / 'stream-cancel.txt').exists() and not app.history(stream.body['session_id'])
        app.stop()
        print('PASS: explicit stream cancellation retains original current-model settlement without future tool/model dispatch', flush=True)

        app = f.App(root, 'stream-drop'); app.start(); gate = f.gate('stream-drop')
        stream = Stream(app, 'stream-drop'); stream.early(app, gate); stream.close()
        # Observe the actual server transport consumer ending, not just a client FIN.
        f.eventually(lambda: 'HTTP event delivery stopped' in app.log.read_text()
                     and stream.id in app.log.read_text(), 'actual server body consumer cancellation', 14)
        assert app.http('/api/turns/' + stream.id)[1]['active'] and app.ledger()[0]['state'] == 'submitting'
        gate['release'].set(); receipt = app.terminal(stream.id, 'needs_review')
        assert not receipt['session_committed'] and app.ledger()[0]['state'] == 'completed'
        assert f.count('stream-drop') == 1 and not (app.workspace / 'stream-drop.txt').exists()
        assert app.http('/api/turns/' + stream.id + '/stream', 'PUT', stream.body)[0] == 200
        app.stop()
        print('PASS: actual HTTP consumer close cancels dispatch while submitted model and original durable owner remain', flush=True)

        app = f.App(root, 'stream-sql'); app.start(); gate = f.gate('stream-sql')
        stream = Stream(app, 'stream-sql'); stream.early(app, gate)
        app.sql("CREATE TRIGGER fixture_stream_finish BEFORE UPDATE OF state ON http_turns BEGIN SELECT RAISE(ABORT,'fixture terminal fault'); END", write=True)
        gate['release'].set(); events = stream.finish()
        assert events[-1]['event'] == 'error' and all(e['event'] != 'done' for e in events)
        orphan = f.eventually(lambda: (v if not v['active'] else None) if (v := app.http('/api/turns/' + stream.id)[1]) else None, 'failed atomic terminal owner ended')
        assert orphan['receipt']['state'] == 'running' and not app.history(stream.body['session_id'])
        assert app.ledger()[0]['state'] == 'completed' and f.count('stream-sql') == 1
        assert app.http('/api/turns/' + stream.id + '/stream', 'PUT', stream.body)[0] == 200
        app.sql('DROP TRIGGER fixture_stream_finish', write=True); app.stop()
        print('PASS: rejected terminal SQL emits no done and cannot partially commit history or replay original stream', flush=True)

        app = f.App(root, 'stream-slow', timeout=30); app.start(); gate = f.gate('stream-slow')
        identity = str(uuid.uuid4()); body = {'session_id': 'http:' + str(uuid.uuid4()), 'prompt': 'stream-slow', 'enabled_tools': ['file_write']}
        sock = socket.socket(); sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024); sock.settimeout(15)
        sock.connect(('127.0.0.1', app.port)); raw = json.dumps(body).encode()
        sock.sendall((f'PUT /api/turns/{identity}/stream HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {f.api_token}\r\nContent-Type: application/json\r\nContent-Length: {len(raw)}\r\nConnection: close\r\n\r\n').encode() + raw)
        reader = sock.makefile('rb'); assert b'202' in reader.readline()
        for _ in range(64):
            line = reader.readline(8192)
            assert line, 'stream headers truncated'
            if line in (b'\r\n', b'\n'): break
        else: raise AssertionError('stream header budget')
        # Stop reading the real TCP body; the receiver's tiny advertised window
        # fills the server socket/Hyper/finite queues, not a mocked send future.
        assert gate['ready'].wait(15)
        assert app.ledger()[0]['state'] == 'submitting'
        gate['release'].set()
        f.eventually(lambda: identity in app.log.read_text() and 'HTTP delivery backpressure' in app.log.read_text(), 'actual TCP backpressure reaches finite writer deadline', 15)
        terminal = f.eventually(lambda: (v if not v['active'] and v['receipt']['state'] != 'running' else None) if (v := app.http('/api/turns/' + identity)[1]) else None, 'actual slow stream owners ended')['receipt']
        assert terminal['state'] in ('completed', 'needs_review') and f.count('stream-slow') == 1
        assert app.ledger()[0]['state'] == 'completed'
        assert bool(app.history(body['session_id'])) == terminal['session_committed']
        assert terminal['session_committed'] == (terminal['state'] == 'completed')
        print(json.dumps({'actual_slow_receipt_state':terminal['state'],'session_committed':terminal['session_committed'],'actual_backpressure_logged':True}),flush=True)
        reader.close(); sock.close()
        assert app.http('/api/turns/' + identity + '/stream', 'PUT', body)[0] == 200
        # Actual capacity remains usable after the old consumer and model settle.
        stream = Stream(app, 'after-slow'); events = stream.finish(); assert events[-1]['receipt']['state'] == 'completed'
        app.stop()
        print('PASS: real TCP slow reader hits bounded writer deadline; original committed/review receipt remains authoritative; capacity recovers', flush=True)

        app = f.App(root, 'stream-stop'); app.start(); gate = f.gate('stream-stop')
        stream = Stream(app, 'stream-stop'); stream.early(app, gate)
        began = time.monotonic(); app.stop(); assert time.monotonic() - began < 6
        stream.close(); gate['release'].set(); app.settings['http']['tracked_turns'] = False; app.save(); app.start()
        assert app.http('/api/turns/capabilities')[1]['streaming'] is False
        original = app.http('/api/turns/' + stream.id + '/stream', 'PUT', stream.body)[1]
        assert original['receipt']['state'] == 'needs_review' and original['receipt']['error'] == 'process_interrupted'
        assert f.count('stream-stop') == 1 and not (app.workspace / 'stream-stop.txt').exists()
        assert not app.history(stream.body['session_id']); app.stop()
        print('PASS: live stream shares original shutdown grace; restart/feature-off duplicate is JSON lookup without effects', flush=True)
        assert not f.faults, f.faults
        print('PASS: seven HTTP streaming integration groups; no paid credentials or Web/tenant/proxy/supplier certification', flush=True)
finally:
    for gate in f.gates.values(): gate['release'].set()
    for process in f.processes:
        if process.poll() is None: process.kill(); process.wait(timeout=5)
    f.server.shutdown(); f.server.server_close()
