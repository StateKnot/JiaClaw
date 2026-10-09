#!/usr/bin/env python3
"""Real embedded browser against the existing gated model/SQLite contract fixture.

Test-only local control service owns fixture gates and fault injection. No vendor
credentials, no UI/backend replay, no tenant or reverse-proxy certification.
"""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import tempfile
import threading
import http_turns as fixture

app = None


class Control(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            action = data['action']
            if action == 'gate':
                fixture.gate(data['case'])
                value = True
            elif action == 'release':
                fixture.gates[data['case']]['release'].set()
                value = True
            elif action == 'status':
                value = {'count': fixture.count(data['case']), 'ready': fixture.gates.get(data['case'], {}).get('ready', threading.Event()).is_set(),
                         'faults': fixture.faults, 'posts': [{'case': p['case'], 'body': p['body']} for p in fixture.posts],
                         'rows': app.sql('SELECT id,session_id,state,session_committed,cancel_requested,reviewed_ms FROM http_turns'),
                         'ledger': app.ledger(), 'effects': [p.name for p in app.workspace.glob('*.txt')],
                         'delivery_stopped': any('HTTP event delivery stopped' in line and any(row['turn_id'] in line for row in app.ledger() if any(p['case'] == data['case'] and p['operation'] == row['id'] for p in fixture.posts)) for line in app.log.read_text().splitlines())}
            elif action == 'fail-commit':
                app.sql("CREATE TRIGGER browser_fail BEFORE UPDATE OF state ON http_turns WHEN NEW.state <> 'running' BEGIN SELECT RAISE(ABORT,'browser receipt failure'); END", write=True)
                value = True
            elif action == 'restore-commit':
                app.sql('DROP TRIGGER browser_fail', write=True)
                value = True
            else:
                raise AssertionError('unknown fixture action')
            raw = json.dumps({'value': value}).encode()
            self.send_response(200)
        except Exception as error:
            raw = json.dumps({'error': repr(error)}).encode()
            self.send_response(500)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-web-stream-') as temporary:
        root = Path(temporary)
        app = fixture.App(root, 'browser', timeout=15)
        app.start()
        control = ThreadingHTTPServer(('127.0.0.1', 0), Control)
        control.daemon_threads = True
        threading.Thread(target=control.serve_forever, daemon=True).start()
        config = root / 'browser.json'
        config.write_text(json.dumps({'base': f'http://127.0.0.1:{app.port}', 'token': fixture.api_token,
                                      'control': f'http://127.0.0.1:{control.server_port}'}))
        result = subprocess.run(['node', 'tests/turn_stream_browser.cjs', str(config)], timeout=240)
        assert result.returncode == 0, result.returncode
        assert not fixture.faults, fixture.faults
        print('ALL WEB STREAM BROWSER GROUPS PASS', flush=True)
finally:
    for gate in fixture.gates.values():
        gate['release'].set()
    if app:
        app.stop()
    fixture.server.shutdown()
