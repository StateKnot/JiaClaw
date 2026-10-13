#!/usr/bin/env python3
"""Real workbench correction drafts through existing HTTP/SQLite/model fixtures."""
import json
import subprocess
import sys
import tempfile
import threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http_turns as h
import tenant_http_browser as tenant
import tenant_http_fixture as f

native = app = control = None
mode = None

class Control(BaseHTTPRequestHandler):
    def log_message(self, *_args): pass
    def do_POST(self):
        value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert value['action'] in ('status', 'purge')
        prompt = value.get('prompt', '')
        if value['action'] == 'purge':
            path = '/api/turns/' + value['id'] + '/result'
            if mode == 'standalone': result = native.http(path, 'DELETE')[0]
            else:
                backend = app.backends['alice']
                result = f.request(backend['port'], path, backend['token'], 'DELETE', marker=True)[0]
            assert result == 200, result
        elif mode == 'standalone':
            with h.lock: posts = [p['body'] for p in h.posts if p['case'] == prompt]
            result = {'count': len(posts), 'posts': posts, 'faults': h.faults}
        else:
            result = app.status('alice', prompt)
        raw = json.dumps(result).encode()
        self.send_response(200); self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw))); self.end_headers(); self.wfile.write(raw)

try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-task-revision-') as temporary:
        root = Path(temporary)
        control = ThreadingHTTPServer(('127.0.0.1', 0), Control)
        control.daemon_threads = True
        threading.Thread(target=control.serve_forever, daemon=True).start()
        for mode in ['standalone', 'tenant_json', 'tenant_preview']:
            if mode == 'standalone':
                native = h.App(root, mode, timeout=15); native.start()
                cfg = {'base': f'http://127.0.0.1:{native.port}', 'token': h.api_token}
            else:
                directory = root / mode; directory.mkdir()
                app = tenant.App(directory)
                cfg = {'base': f'http://127.0.0.1:{app.gp}', 'token': app.users['alice']['token'], 'readonly': app.readonly['token'], 'bob': app.users['bob']['token']}
            cfg.update({'mode': mode, 'control': f'http://127.0.0.1:{control.server_port}'})
            path = root / (mode + '.json'); path.write_text(json.dumps(cfg))
            result = subprocess.run(['node', 'tests/task_revision_browser.cjs', str(path)], timeout=150)
            assert result.returncode == 0, (mode, result.returncode)
            if native: native.stop(); native = None
            if app:
                f.stop(app.process)
                for backend in app.backends.values(): f.stop(backend['process'])
                app = None
        assert not h.faults, h.faults
        assert not f.faults, f.faults
        print('ALL TASK REVISION DRAFT BROWSER GROUPS PASS', flush=True)
finally:
    if control: control.shutdown()
    for gate in h.gates.values(): gate['release'].set()
    for gate in f.gates.values(): gate['release'].set()
    for process in reversed(h.processes + f.processes):
        if process.poll() is None: process.terminate(); process.wait(timeout=8)
    h.server.shutdown(); f.model.shutdown()
