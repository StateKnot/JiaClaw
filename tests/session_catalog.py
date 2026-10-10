#!/usr/bin/env python3
"""Real HTTP/SQLite/gateway discovery; no external models or credentials."""
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env.update(JIACLAW_LOG_LEVEL='info', JIACLAW_LOG_FORMAT='text')
processes = []


def request(base, token, path, method='GET', body=None):
    headers = {'Authorization': 'Bearer ' + token} if token else {}
    if body is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(base + path, method=method, headers=headers,
                                 data=json.dumps(body).encode() if body is not None else None)
    try:
        response = urllib.request.urlopen(req, timeout=15)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read()
        return response.status, json.loads(raw) if raw else None, response.headers


def stop(p):
    if p.poll() is None:
        p.terminate()
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait(timeout=5)


class App:
    def __init__(self, root, persist=True, tenant=False, ttl=None):
        root.mkdir(); self.root = root; self.token = uuid.uuid4().hex
        workspace = root / 'workspace'; workspace.mkdir()
        self.db = root / 'state/sessions.sqlite3'; self.config = root / 'config.json'
        settings = {'agent': {'name': root.name, 'description': 'Catalog acceptance',
            'system_instructions': 'Test assistant', 'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'stub'}, 'http': {'bind': '127.0.0.1:0',
            'api_token': self.token, 'persist': persist, 'persist_path': '../state/sessions.sqlite3',
            'session_ttl_secs': ttl, 'shutdown_timeout_secs': 1}}
        if tenant:
            settings['scheduler'] = {'enabled': True, 'gateway_driven': True}
        self.config.write_text(json.dumps(settings)); self.start()

    def start(self):
        log = self.root / ('host-' + uuid.uuid4().hex + '.log')
        with log.open('wb') as output:
            self.p = subprocess.Popen([str(binary), 'serve', '--config', str(self.config)],
                                      env=env, stdout=output, stderr=output)
        processes.append(self.p)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert self.p.poll() is None, log.read_text()
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
            if match:
                self.base = match.group(1); return
            time.sleep(.03)
        raise AssertionError(log.read_text())

    def sql(self, statement, params=()):
        with sqlite3.connect(self.db, timeout=5) as conn:
            return conn.execute(statement, params).fetchall()

    def http(self, path, method='GET', body=None, auth=True):
        return request(self.base, self.token if auth else None, path, method, body)


def check_page(base, token, query='', limit=50):
    status, page, headers = request(base, token, '/api/sessions' + query)
    assert status == 200, page
    assert set(page) == {'sessions', 'limit', 'has_more', 'next_cursor'}
    assert page['limit'] == limit and len(page['sessions']) <= limit
    assert headers['Cache-Control'] == 'no-store'
    assert all(set(s) == {'id', 'message_count'} for s in page['sessions'])
    ids = [s['id'] for s in page['sessions']]
    assert ids == sorted(set(ids), key=lambda s: s.encode())
    assert page['next_cursor'] == (ids[-1].encode().hex() if page['has_more'] else None)
    return page


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-session-catalog-') as temporary:
        root = Path(temporary)
        invalid = ['?limit=0', '?limit=51', '?limit=01', '?limit=%31', '?limit=1&limit=2',
            '?offset=0', '?after=', '?after=abc', '?after=AA', '?after=ff', '?after=%61%61',
            '?after=61&after=62', '?limit=1&', '?unknown=1', '?after=' + '61' * 1025]
        for persist in (False, True):
            app = App(root / ('sqlite' if persist else 'memory'), persist=persist)
            ids = []
            for _ in range(53):
                status, value, _ = app.http('/api/sessions', 'POST')
                assert status == 200; ids.append(value['session_id'])
            actual = app.http('/api/sessions')[1]
            print(json.dumps({'backend': 'sqlite' if persist else 'memory', 'default_count': len(actual['sessions'])}), flush=True)
            assert len(actual['sessions']) <= 50, 'unbounded default session catalog'
            for query in invalid:
                assert app.http('/api/sessions' + query, auth=False)[0] == 401
                assert app.http('/api/sessions' + query)[0] == 400, query
            first = check_page(app.base, app.token)
            assert len(first['sessions']) == 50 and first['has_more']
            second = check_page(app.base, app.token, '?after=' + first['next_cursor'])
            assert len(second['sessions']) == 3 and not second['has_more']
            assert [s['id'] for s in first['sessions'] + second['sessions']] == sorted(ids)
            # A deleted anchor still advances by its value; insertion before it
            # cannot duplicate rows on the next page. Cursor is not a snapshot.
            anchor = first['sessions'][-1]['id']
            assert app.http('/api/sessions/' + anchor, 'DELETE')[0] == 200
            assert check_page(app.base, app.token, '?after=' + first['next_cursor']) == second
            assert len(check_page(app.base, app.token, '?limit=1', 1)['sessions']) == 1
            stop(app.p)
            print('PASS sessions ' + ('2' if persist else '1') + ': real ' + ('SQLite' if persist else 'memory') + ' authentication, finite pages, deleted anchor and no duplicate discovery', flush=True)

        app = App(root / 'projection', ttl=60)
        # Metadata deliberately has no ChatMessage shape. Listing must not
        # deserialize it, and must not leak its 2 MiB content or alter its TTL.
        private = json.dumps([{'private': 'catalog-secret-' + 'x' * (2 * 1024 * 1024)}])
        planted = int(time.time() * 1000) - 1000
        app.sql('INSERT INTO sessions VALUES(?,?,?)', ('aa-summary', private, planted))
        page = check_page(app.base, app.token)
        assert page['sessions'] == [{'id': 'aa-summary', 'message_count': 1}]
        assert app.sql('SELECT accessed_ms FROM sessions WHERE id=?', ('aa-summary',)) == [(planted,)]
        # Only selected histories are parsed. A large later row is rejected at
        # its own page before JSON parsing, never loaded into Rust memory.
        app.sql('INSERT INTO sessions VALUES(?,?,?)', ('zz-budget', json.dumps(['x' * (33 * 1024 * 1024)]), planted))
        assert check_page(app.base, app.token, '?limit=1', 1)['has_more'] is True
        assert app.http('/api/sessions?after=' + 'aa-summary'.encode().hex())[0] == 500
        app.sql('DELETE FROM sessions WHERE id=?', ('zz-budget',))
        # TTL cleanup exceeds one internal deletion batch. List access doesn't
        # keep these expired sessions alive. A direct GET still touches its ID.
        for i in range(300):
            app.sql('INSERT INTO sessions VALUES(?,?,?)', (f'expired-{i:03}', '[]', planted - 120000))
        app.sql('INSERT INTO sessions VALUES(?,?,?)', ('fresh-get', '[]', planted))
        assert app.http('/api/sessions/fresh-get')[0] == 200
        assert app.sql('SELECT accessed_ms FROM sessions WHERE id=?', ('fresh-get',))[0][0] > planted
        check_page(app.base, app.token)
        assert app.sql("SELECT count(*) FROM sessions WHERE id LIKE 'expired-%'") == [(0,)]
        assert app.sql('SELECT accessed_ms FROM sessions WHERE id=?', ('aa-summary',)) == [(planted,)]
        stop(app.p)
        print('PASS sessions 3: storage metadata excludes bodies, bounded selected-row JSON, unchanged list TTL, explicit GET touch and multi-batch expiry', flush=True)

        alice = App(root / 'alice', tenant=True); bob = App(root / 'bob', tenant=True)
        for i in range(53):
            alice.sql('INSERT INTO sessions VALUES(?,?,?)', (f'alice-{i:03}', '[]', planted))
            alice.sql('INSERT INTO sessions VALUES(?,?,?)', (f'job:{i:03}', '[]', planted))
        bob.sql('INSERT INTO sessions VALUES(?,?,?)', ('bob-only', '[]', planted))
        sock = socket.socket(); sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]; sock.close()
        config = root / 'gateway.json'; backends = []
        for app, name in ((alice, 'alice'), (bob, 'bob')):
            secret = app.root / 'token'; secret.write_text(app.token); secret.chmod(0o600)
            backends.append({'id': name, 'url': app.base + '/', 'token_file': str(secret)})
        config.write_text(json.dumps({'bind': f'127.0.0.1:{port}', 'registry_path': str(root / 'registry/users.sqlite3'),
            'scheduled_jobs': True, 'request_timeout_seconds': 150, 'max_in_flight': 4, 'backends': backends}))
        def cli(*args):
            p = subprocess.run([str(binary), 'gateway', *args, '--config', str(config)], env=env,
                capture_output=True, text=True, timeout=20)
            assert p.returncode == 0, p.stderr
            return json.loads(p.stdout)
        full = cli('user-add', '--backend', 'alice'); other = cli('user-add', '--backend', 'bob')
        readonly = cli('key-add', '--user', full['user_id'], '--read-only')
        audit_before = cli('audit-list', '--user', full['user_id'], '--limit', '50')
        gateway = subprocess.Popen([str(binary), 'gateway', 'serve', '--config', str(config)], env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); processes.append(gateway)
        base = f'http://127.0.0.1:{port}'; deadline = time.monotonic() + 20
        while True:
            assert gateway.poll() is None
            try:
                if request(base, full['token'], '/api/sessions')[0] == 200: break
            except urllib.error.URLError: pass
            assert time.monotonic() < deadline; time.sleep(.03)
        first = check_page(base, readonly['token']); second = check_page(base, readonly['token'], '?after=' + first['next_cursor'])
        assert len(first['sessions']) == 50 and len(second['sessions']) == 3
        assert all(s['id'].startswith('alice-') for s in first['sessions'] + second['sessions'])
        assert check_page(base, other['token'])['sessions'] == [{'id': 'bob-only', 'message_count': 0}]
        for query in invalid:
            assert request(base, None, '/api/sessions' + query)[0] == 401
            assert request(base, readonly['token'], '/api/sessions' + query)[0] == 403
        assert request(base, readonly['token'], '/api/sessions?limit=1', 'POST')[0] == 403
        assert alice.sql('SELECT accessed_ms FROM sessions WHERE id=?', ('alice-000',)) == [(planted,)]
        assert cli('audit-list', '--user', full['user_id'], '--limit', '50') == audit_before
        print('PASS sessions 4: actual two-tenant gateway readonly pagination, job filter before LIMIT, no admission/audit/model/TTL write', flush=True)
finally:
    for p in reversed(processes): stop(p)
