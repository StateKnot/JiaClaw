#!/usr/bin/env python3
"""Real binary/HTTP/SQLite crash recovery smoke test; no provider credentials."""
import concurrent.futures
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
with tempfile.TemporaryDirectory(prefix='jiaclaw-e2e-') as directory:
    root = Path(directory)
    (root / 'workspace').mkdir()
    name = 'e2e-' + uuid.uuid4().hex
    token = uuid.uuid4().hex
    config = root / 'config.json'
    config.write_text(json.dumps({
        'agent': {'name': name, 'description': 'Disposable E2E fixture', 'system_instructions': 'You are a test assistant.', 'max_turns': 10, 'workspace_path': str(root / 'workspace')},
        'provider': {'provider_type': 'stub'},
        'http': {'bind': '127.0.0.1:0', 'api_token': token, 'persist': True, 'persist_path': '../state/sessions.sqlite3'}
    }))
    process = None
    base = None
    def request(path, method='GET', body=None, authenticated=True, extra_headers=None):
        headers = {'Authorization': 'Bearer ' + token} if authenticated else {}
        headers.update(extra_headers or {})
        if body is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(base + path, method=method, headers=headers, data=json.dumps(body).encode() if body is not None else None)
        with urllib.request.urlopen(req, timeout=15) as response:
            raw = response.read()
            return response.status, response.headers, json.loads(raw) if 'application/json' in response.headers.get('Content-Type', '') else raw
    def start():
        global process, base
        log = root / ('server-' + uuid.uuid4().hex + '.log')
        env = dict(os.environ)
        env.pop('JIACLAW_API_KEY', None)
        env.pop('JIACLAW_API_TOKEN', None)
        env.pop('JIACLAW_LOG_FORMAT', None)
        for key in ['JIACLAW_WEBHOOK_SECRET', 'JIACLAW_TELEGRAM_SECRET', 'JIACLAW_SLACK_SIGNING_SECRET', 'JIACLAW_DISCORD_PUBLIC_KEY']:
            env.pop(key, None)
        env['JIACLAW_LOG_LEVEL'] = 'info'
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config)], stdout=output, stderr=output, env=env)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise AssertionError(log.read_text())
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
            if match:
                base = match.group(1)
                try:
                    assert request('/health')[2]['agent_name'] == name
                    return
                except (urllib.error.URLError, ConnectionError):
                    pass
            time.sleep(.05)
        raise AssertionError('server startup timed out: ' + log.read_text())
    try:
        start()
        status, headers, page = request('/', authenticated=False)
        assert status == 200 and b'JiaClaw' in page
        assert "frame-ancestors 'none'" in headers['Content-Security-Policy']
        for path in ['/ui/app.js', '/ui/app.css']:
            assert request(path, authenticated=False)[0] == 200
        try:
            request('/api/sessions', authenticated=False)
            raise AssertionError('unauthenticated session access succeeded')
        except urllib.error.HTTPError as error:
            assert error.code == 401
        for path in ['/hooks/inbound', '/hooks/telegram', '/hooks/slack', '/hooks/discord', '/hooks/feishu', '/hooks/wecom', '/hooks/dingtalk']:
            try:
                request(path, 'POST', {}, authenticated=False)
                raise AssertionError('unconfigured webhook was reachable: ' + path)
            except urllib.error.HTTPError as error:
                assert error.code == 404
        empty_id = request('/api/sessions', 'POST')[2]['session_id']
        chat_id = request('/api/sessions', 'POST')[2]['session_id']
        def chat(text):
            return request('/api/chat', 'POST', {'session_id': chat_id, 'messages': [{'role': 'user', 'content': text}]})[2]
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
            replies = list(executor.map(chat, ['first simultaneous turn', 'second simultaneous turn']))
        assert all(reply['status'] == 'completed' for reply in replies)
        assert len(request('/api/sessions/' + chat_id)[2]['messages']) == 4
        # SIGKILL intentionally skips graceful shutdown: committed WAL must suffice.
        process.kill(); process.wait(timeout=5)
        start()
        assert request('/api/sessions/' + empty_id)[2]['messages'] == []
        assert len(request('/api/sessions/' + chat_id)[2]['messages']) == 4
        assert request('/api/sessions/' + chat_id, 'DELETE')[2]['success']
        process.kill(); process.wait(timeout=5)
        start()
        try:
            request('/api/sessions/' + chat_id)
            raise AssertionError('deleted session reappeared after restart')
        except urllib.error.HTTPError as error:
            assert error.code == 404
        process.terminate(); process.wait(timeout=5)
        settings = json.loads(config.read_text())
        settings['http']['webhook_secret'] = 'fixture-inbound-secret'
        config.write_text(json.dumps(settings))
        start()
        body = {'chat_id': 'authenticated-hook', 'text': 'hello'}
        try:
            request('/hooks/inbound', 'POST', body, authenticated=False)
            raise AssertionError('configured webhook accepted missing secret')
        except urllib.error.HTTPError as error:
            assert error.code == 401
        assert request('/hooks/inbound', 'POST', body, authenticated=False, extra_headers={'X-Webhook-Secret': 'fixture-inbound-secret'})[2]['ok']
        print('PASS: embedded Web, API/channel auth, concurrent turns, empty sessions, SIGKILL recovery, persistent deletion')

    finally:
        if process and process.poll() is None:
            process.terminate()
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
