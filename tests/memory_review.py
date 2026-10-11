#!/usr/bin/env python3
"""Actual standalone HTTP review; disposable credentials, no model calls."""
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from unittest.mock import patch
from host_log import read_running_log

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
with tempfile.TemporaryDirectory(prefix='jiaclaw-memory-review-') as raw_root:
    root = Path(raw_root).resolve()
    workspace = root / 'workspace'
    (workspace / 'notes').mkdir(parents=True)
    target = workspace / 'notes/private.md'
    original = b'Private tea preference\r\n<img src=x onerror=alert(1)>\n'
    target.write_bytes(original)
    (workspace / 'MEMORY.md').write_text('DEFAULT MUST NOT APPEAR')
    token = uuid.uuid4().hex
    config = {'agent': {'name': 'memory-review', 'description': 'Disposable review',
              'workspace_path': str(workspace), 'max_turns': 10, 'system_instructions': ''},
              'provider': {'provider_type': 'stub'}, 'memory': {'path': 'notes/private.md'},
              'http': {'bind': '127.0.0.1:0', 'api_token': token, 'memory_review': True,
                       'persist': False, 'shutdown_timeout_secs': 1}}
    config_path = root / 'config.json'

    def start(settings):
        config_path.write_text(json.dumps(settings))
        log = open(root / ('serve-' + uuid.uuid4().hex + '.log'), 'wb')
        process = subprocess.Popen([str(binary), 'serve', '--config', str(config_path)],
                                   env=env, stdout=log, stderr=log)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            text = read_running_log(Path(log.name))
            import re
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:\d+)', text)
            if match:
                return process, log, match[1]
            if process.poll() is not None:
                raise AssertionError(text)
            time.sleep(.02)
        process.kill(); process.wait(); log.close()
        raise AssertionError('startup deadline')

    def stop(process, log):
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait()
                raise
        log.close()

    def request(base, path='/api/memory', key=token, method='GET'):
        headers = {'Authorization': 'Bearer ' + key} if key is not None else {}
        request = urllib.request.Request(base + path, headers=headers, method=method)
        try:
            response = urllib.request.urlopen(request, timeout=5)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, response.headers, response.read()

    def header_case_regression():
        required = {'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff',
                    'Referrer-Policy': 'no-referrer'}

        def receive(fields, status):
            client, peer = socket.socketpair()
            try:
                client.settimeout(5); peer.settimeout(5)
                wire = 'HTTP/1.1 ' + str(status) + ' Test\r\n'
                wire += ''.join(name + ': ' + value + '\r\n' for name, value in fields.items())
                peer.sendall((wire + 'Content-Length: 2\r\nConnection: close\r\n\r\n{}').encode('ascii'))
                peer.shutdown(socket.SHUT_WR)
                response = http.client.HTTPResponse(client)
                response.begin()
                options = {'return_value': response} if status == 200 else {
                    'side_effect': urllib.error.HTTPError('http://127.0.0.1/api/memory',
                                                         status, 'Test', response.headers, response)}
                with patch.object(urllib.request, 'urlopen', **options):
                    actual_status, headers, body = request('http://127.0.0.1', key=None)
                assert actual_status == status and body == b'{}'
                return headers
            finally:
                client.close(); peer.close()

        for status in [200, 401]:
            for case in [str.lower, str.title, str.swapcase]:
                headers = receive({case(name): value for name, value in required.items()}, status)
                assert all(headers[name] == value for name, value in required.items())
            for name in required:
                for value in [None, 'wrong']:
                    fields = {key.lower(): item for key, item in required.items() if key != name}
                    if value is not None: fields[name.lower()] = value
                    headers = receive(fields, status)
                    try:
                        assert all(headers[key] == item for key, item in required.items())
                    except AssertionError:
                        pass
                    else:
                        raise AssertionError('missing/wrong privacy header accepted')
        print('PASS memory review header casing: actual HTTP parser accepts lower/title/mixed-case 200/401; missing/wrong privacy values reject')

    header_case_regression()
    process, log, base = start(config)
    try:
        status, headers, raw = request(base)
        assert status == 200, (status, raw)
        expected = {'path': 'notes/private.md', 'content': original.decode(),
                    'content_sha256': hashlib.sha256(original).hexdigest(), 'bytes': len(original)}
        assert json.loads(raw) == expected
        assert headers['Cache-Control'] == 'no-store' and headers['X-Content-Type-Options'] == 'nosniff'
        assert json.loads(request(base, '/api/memory/capabilities')[2]) == {'protocol': 1, 'scope': 'standalone', 'read_only': True, 'max_bytes': 32768}
        for key in [None, 'wrong']:
            status, headers, body = request(base, key=key)
            assert status == 401 and original not in body and headers['Cache-Control'] == 'no-store'
        print('PASS memory review 1: complete private configured snapshot, exact digest, explicit authenticated capability and no-store')

        inode = target.stat().st_ino
        for method in ['POST', 'PUT', 'DELETE', 'PATCH']:
            assert request(base, method=method)[0] == 405
        assert target.read_bytes() == original and target.stat().st_ino == inode
        target.write_bytes(b'')
        assert json.loads(request(base)[2])['content'] == ''
        exact = b'\x00' * 32768
        target.write_bytes(exact)
        assert json.loads(request(base)[2])['content'].encode() == exact
        for invalid in [b'x' * 32769, b'\xff']:
            target.write_bytes(invalid)
            assert request(base)[0] == 422
            assert target.read_bytes() == invalid
        print('PASS memory review 2: no write methods; complete empty/exact32KiB/escaped JSON and no truncation or fallback')

        foreign = root / 'foreign'
        foreign.write_bytes(original)
        for kind in ['link', 'hardlink', 'fifo', 'directory', 'missing']:
            target.unlink()
            if kind == 'link': target.symlink_to(foreign)
            elif kind == 'hardlink': os.link(foreign, target)
            elif kind == 'fifo': os.mkfifo(target)
            elif kind == 'directory': target.mkdir()
            assert request(base)[0] == 422
            assert foreign.read_bytes() == original
            if kind == 'directory': target.rmdir()
            elif kind != 'missing': target.unlink()
            target.write_bytes(original)
        assert not list(workspace.rglob('.jiaclaw-memory-*'))
        print('PASS memory review 3: unsafe and missing source refuses completely without writing or opening ambient paths')
    finally:
        stop(process, log)

    disabled = json.loads(json.dumps(config)); disabled['http']['memory_review'] = False; disabled['http'].pop('api_token')
    process, log, base = start(disabled)
    try:
        for path in ['/api/memory', '/api/memory/capabilities']:
            status, headers, raw = request(base, path, key=None)
            assert status == 403 and original not in raw and headers['Cache-Control'] == 'no-store'
    finally:
        stop(process, log)
    for options in [{'api_token': ''}, {'gateway_channel_chat': True}, {}]:
        invalid = json.loads(json.dumps(config)); invalid['http'].update(options)
        if options == {}: invalid['http'].pop('api_token')
        config_path.write_text(json.dumps(invalid))
        result = subprocess.run([str(binary), 'serve', '--config', str(config_path)], env=env, capture_output=True, timeout=8)
        assert result.returncode != 0 and b'memory_review requires' in result.stderr
    print('PASS memory review 4: opt-in/nonempty operator token/standalone gates; legacy anonymous access cannot expose memory')
