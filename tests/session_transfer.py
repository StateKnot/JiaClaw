#!/usr/bin/env python3
"""Original native Store/import/export and private gateway; no model calls."""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import http_turns as f
import tenant_http_browser as tenant

app = None


class Control(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if value['action'] == 'restart':
                app.stop(kill=True)
                app.start()
            assert value['action'] in ('restart', 'status')
            result = {'model_posts': len(f.posts) + len(tenant.f.posts), 'faults': f.faults + tenant.f.faults}
            raw = json.dumps(result).encode()
            self.send_response(200)
        except Exception as error:
            raw = json.dumps({'error': repr(error)}).encode()
            self.send_response(500)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-session-transfer-') as directory:
        root = Path(directory)
        app = f.App(root, 'native', timeout=15)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            app.settings['http']['bind'] = '127.0.0.1:' + str(listener.getsockname()[1])
        app.save()
        app.start()
        source = [{'role': 'system', 'content': '\ufeffPrivate source\r\n\t\0🦀 '}, {'role': 'user', 'content': '<img src=x onerror=TRANSFER_XSS> ' + 'x' * 70000}, {'role': 'assistant', 'content': 'tail agreement\r\n '}]
        code, imported = app.http('/api/sessions/import?format=json&id=transfer-source', 'POST', {'messages': source})
        assert code == 200 and imported['messages'] == source
        gateway_root = root / 'gateway'
        gateway_root.mkdir()
        gateway = tenant.App(gateway_root)
        control = ThreadingHTTPServer(('127.0.0.1', 0), Control)
        control.daemon_threads = True
        threading.Thread(target=control.serve_forever, daemon=True).start()
        config = root / 'browser.json'
        config.write_text(json.dumps({'base': 'http://127.0.0.1:' + str(app.port), 'token': f.api_token,
            'control': 'http://127.0.0.1:' + str(control.server_port), 'source': source,
            'gateway': 'http://127.0.0.1:' + str(gateway.gp), 'alice': gateway.users['alice']['token'], 'readonly': gateway.readonly['token']}))
        result = subprocess.run(['node', 'tests/session_transfer.cjs', str(config)], timeout=180)
        assert result.returncode == 0
        assert not f.posts and not tenant.f.posts and not f.faults and not tenant.f.faults
        print('ALL SESSION TRANSFER BROWSER GROUPS PASS', flush=True)
finally:
    if app:
        app.stop()
    for process in reversed(tenant.f.processes):
        tenant.f.stop(process)
    f.server.shutdown()
    tenant.f.model.shutdown()
