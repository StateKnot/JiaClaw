"""Test-only loopback receipt relay; competing private GETs refuse admission.

This controls one possible observer-interference mechanism. It does not identify
the unique cause of a historical runner failure or change the production host.
"""
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import time


class ReceiptRelay:
    def __init__(self, native_port):
        owner = self
        self.guard = threading.Lock()
        self.active = 0
        self.receipts = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def forward(self):
                guarded = (self.command == 'GET'
                           and self.path.startswith('/api/turns/')
                           and self.path != '/api/turns/capabilities')
                held = False
                try:
                    if guarded:
                        with owner.guard:
                            owner.active += 1
                            held = True
                            overlap = owner.active > 1
                        if overlap:
                            data = b'{"error":"controlled_receipt_admission_busy"}'
                            self.send_response(503)
                            self.send_header('Content-Type', 'application/json')
                            self.send_header('Content-Length', str(len(data)))
                            self.end_headers()
                            self.wfile.write(data)
                            with owner.guard:
                                owner.receipts.append({'status':503, 'overlap':True})
                            return
                        # Within the original five-second private I/O budget.
                        time.sleep(.150)
                    size = int(self.headers.get('Content-Length', '0'))
                    assert 0 <= size <= 64 * 1024
                    body = self.rfile.read(size) if size else None
                    headers = {k:v for k,v in self.headers.items()
                               if k.lower() not in ('host', 'connection', 'content-length')}
                    connection = http.client.HTTPConnection('127.0.0.1', native_port, timeout=5)
                    try:
                        connection.request(self.command, self.path, body, headers)
                        response = connection.getresponse()
                        data = response.read(2 * 1024 * 1024 + 16_384 + 1)
                        assert len(data) <= 2 * 1024 * 1024 + 16_384
                        self.send_response(response.status)
                        for key,value in response.headers.items():
                            if key.lower() not in ('connection', 'content-length',
                                                   'transfer-encoding', 'server', 'date'):
                                self.send_header(key, value)
                        self.send_header('Content-Length', str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                        if guarded:
                            with owner.guard:
                                owner.receipts.append({'status':response.status, 'overlap':False})
                    finally:
                        connection.close()
                finally:
                    if held:
                        with owner.guard:
                            owner.active -= 1

            do_GET = forward
            do_PUT = forward
            do_POST = forward

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f'http://127.0.0.1:{self.server.server_port}/'

    def snapshot(self):
        with self.guard:
            return list(self.receipts)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        assert not self.thread.is_alive()
        with self.guard:
            assert self.active == 0, 'actual receipt relay handlers still active'
