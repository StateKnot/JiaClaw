#!/usr/bin/env python3
"""Real administrator audit CLI, two backend binaries and localhost model protocol.

Normal history is produced only by actual administrator commands and HTTP writes.
The final, offline section edits SQLite solely to exercise retained-history gaps,
large exact cursors and malformed historical data, never to simulate normal work.
"""
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
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
ENV = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
ENV['JIACLAW_LOG_LEVEL'] = 'off'
CLIENT = urllib.request.build_opener(urllib.request.ProxyHandler({}))
LOCK = threading.Lock()
MODEL_CALLS, ERRORS = [], []
MODEL_SECRETS = {who: 'fixture-model-' + uuid.uuid4().hex for who in ('alice', 'bob')}
STARTED, RELEASE = threading.Event(), threading.Event()
PROCESSES = []


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            who = next(name for name, secret in MODEL_SECRETS.items()
                       if self.headers.get('Authorization') == 'Bearer ' + secret)
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert body['model'] == 'fixture-audit'
            prompt = next(item['content'] for item in reversed(body['messages']) if item['role'] == 'user')
            with LOCK:
                MODEL_CALLS.append((who, prompt))
            if prompt == 'held successful turn':
                STARTED.set()
                assert RELEASE.wait(15), 'local fixture was not released'
            if prompt == 'known local provider failure':
                self.reply(503, {'error': {'message': 'local fixture unavailable'}})
            else:
                self.reply(200, {'choices': [{'message': {'role': 'assistant', 'content': who + ': ' + prompt},
                                             'finish_reason': 'stop'}]})
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as error:
            with LOCK:
                ERRORS.append(type(error).__name__)
            self.send_error(500)

    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def http(base, path, token=None, method='GET', body=None):
    headers = {'Authorization': 'Bearer ' + token} if token else {}
    if body is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(base + path, method=method, headers=headers,
                                 data=json.dumps(body).encode() if body is not None else None)
    try:
        response = CLIENT.open(req, timeout=20)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(2 * 1024 * 1024 + 1)
        assert len(raw) <= 2 * 1024 * 1024
        return response.status, json.loads(raw) if raw else None, response.headers


def wait(check, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with LOCK:
            assert not ERRORS, ERRORS
        value = check()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError('gateway audit fixture deadline exceeded')


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def canonical_decimal(value):
    return isinstance(value, str) and value.isascii() and value.isdecimal() and str(int(value)) == value


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
model.daemon_threads = True
threading.Thread(target=model.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-audit-') as temp:
        root = Path(temp).resolve()
        config_path, registry = root / 'gateway.json', root / 'registry/users.sqlite3'
        config = {'bind': '127.0.0.1:' + str(port()), 'registry_path': str(registry),
                  'max_in_flight': 4, 'request_timeout_seconds': 30, 'backends': []}
        gateway_url = 'http://' + config['bind']
        issued = []

        def cli(*args, ok=True, raw=False):
            result = subprocess.run([str(BINARY), 'gateway', *args, '--config', str(config_path)],
                                    env=ENV, text=True, capture_output=True, timeout=15)
            if not ok:
                assert result.returncode != 0 and not result.stdout, 'invalid query emitted output'
                return result
            assert result.returncode == 0, (args[0], result.stderr)
            assert len(result.stdout.encode()) <= 512 * 1024, 'unbounded administrator result'
            data = json.loads(result.stdout)
            if 'token' in data:
                issued.append(data['token'])
            return (data, result.stdout) if raw else data

        def launch(args, base, who):
            log = root / (who + '.log')
            with log.open('ab') as output:
                proc = subprocess.Popen([str(BINARY), *args], env=ENV, stdout=output, stderr=output)
            PROCESSES.append(proc)
            def healthy():
                assert proc.poll() is None, log.read_text()
                try:
                    return http(base, '/health')[0] == 200
                except (OSError, urllib.error.URLError):
                    return False
            wait(healthy)
            return proc

        for who in ('alice', 'bob'):
            directory = root / who
            workspace = directory / 'workspace'
            workspace.mkdir(parents=True)
            token, base = 'fixture-backend-' + uuid.uuid4().hex, 'http://127.0.0.1:' + str(port())
            settings = {'agent': {'name': who, 'description': 'Audit acceptance fixture',
                                  'system_instructions': 'Use configured tools.', 'max_turns': 10,
                                  'workspace_path': str(workspace)},
                        'provider': {'provider_type': 'brokerrouter',
                                     'base_url': 'http://127.0.0.1:' + str(model.server_port),
                                     'api_key': MODEL_SECRETS[who], 'model': 'fixture-audit'},
                        'http': {'bind': base.removeprefix('http://'), 'api_token': token, 'persist': True,
                                 'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1}}
            cfg = directory / 'config.json'
            cfg.write_text(json.dumps(settings))
            secret = directory / 'backend.secret'
            secret.write_text(token)
            config['backends'].append({'id': who, 'url': base, 'token_file': str(secret)})
            launch(['serve', '--config', str(cfg)], base, who)
        config_path.write_text(json.dumps(config))
        users = {who: cli('user-add', '--backend', who) for who in ('alice', 'bob')}
        original = dict(users)
        readonly = cli('key-add', '--user', users['alice']['user_id'], '--read-only')
        gateway = launch(['gateway', 'serve', '--config', str(config_path)], gateway_url, 'gateway')

        def api(who, path, method='GET', body=None, expected=200, key=None):
            result = http(gateway_url, path, (key or users[who])['token'], method, body)
            assert result[0] == expected, (method, path, result[0], result[1])
            return result

        def audit(who, after=0, limit=100, notes=False):
            args = ['audit-list', '--user', users[who]['user_id'], '--after-seq', str(after), '--limit', str(limit)]
            if notes:
                args.append('--include-notes')
            page, raw = cli(*args, raw=True)
            assert set(page) == {'user_id', 'events', 'next_after_seq', 'has_more', 'oldest_retained_seq',
                                 'latest_seq', 'retention_gap', 'notes_included'}
            assert page['user_id'] == users[who]['user_id'] and page['notes_included'] is notes
            assert canonical_decimal(page['next_after_seq']) and canonical_decimal(page['latest_seq'])
            assert page['oldest_retained_seq'] is None or canonical_decimal(page['oldest_retained_seq'])
            seqs = [int(event['seq']) for event in page['events']]
            assert seqs == sorted(set(seqs)) and all(seq > int(after) for seq in seqs)
            for event in page['events']:
                assert event['user_id'] == users[who]['user_id'] and canonical_decimal(event['seq'])
                assert set(event) <= {'seq', 'user_id', 'key_id', 'request_id', 'action', 'created_ms', 'note'}
                assert event['created_ms'] >= 0
                for name in ('key_id', 'request_id'):
                    assert event[name] is None or str(uuid.UUID(event[name])) == event[name]
            if not notes:
                assert '"note"' not in raw and all('note' not in event for event in page['events'])
            assert 'verifier' not in raw and all(secret not in raw for secret in issued + list(MODEL_SECRETS.values()))
            return page

        sessions = {who: api(who, '/api/sessions', 'POST')[1]['session_id'] for who in users}
        chat_result, chat_errors = [], []
        def held_chat():
            try:
                chat_result.append(api('alice', '/api/chat', 'POST', {'session_id': sessions['alice'],
                                   'messages': [{'role': 'user', 'content': 'held successful turn'}]}))
            except Exception as error:
                chat_errors.append(type(error).__name__)
        thread = threading.Thread(target=held_chat)
        thread.start()
        try:
            assert STARTED.wait(10), 'model did not begin'
            during = audit('alice')
            admission = next(event for event in reversed(during['events']) if event['action'] == 'write_admitted')
            assert admission['key_id'] == original['alice']['key_id']
            assert admission['request_id'] is not None
            users['alice'] = cli('key-rotate', '--key', original['alice']['key_id'])
            api('alice', '/api/sessions', expected=401, key=original['alice'])
            # Administrator queries remain available while execution capacity is held.
            after_rotation = audit('alice', during['next_after_seq'])
            assert [event['action'] for event in after_rotation['events']] == ['key_revoked_by_rotation', 'key_rotated']
        finally:
            RELEASE.set()
            thread.join(timeout=10)
        assert not thread.is_alive() and not chat_errors and len(chat_result) == 1
        assert chat_result[0][1]['status'] == 'completed'
        receipt_id = chat_result[0][2]['X-Request-ID']
        assert receipt_id == admission['request_id']
        completed = next(event for event in audit('alice')['events']
                         if event['action'] == 'write_completed' and event['request_id'] == receipt_id)
        assert completed['key_id'] is None
        print('PASS: actual in-flight write audit, rotation association and committed completion; no credential output')

        unknown = api('alice', '/api/chat', 'POST', {'session_id': sessions['alice'],
                      'messages': [{'role': 'user', 'content': 'known local provider failure'}]}, 502)
        needs = next(event for event in audit('alice')['events']
                     if event['action'] == 'write_needs_review' and event['request_id'] == unknown[2]['X-Request-ID'])
        assert needs['key_id'] is None
        note = 'Checked "actual backend idle" and C:\\records\\review; private 中文 evidence <b>literal</b>'
        cli('review-clear', '--user', users['alice']['user_id'], '--confirm-backend-idle', '--note', note)
        default = audit('alice')
        included = audit('alice', notes=True)
        assert included['latest_seq'] == default['latest_seq'] and not included['retention_gap']
        review = next(event for event in included['events'] if event['action'] == 'write_review_cleared')
        assert review['note'] == note and review['request_id'] == needs['request_id']
        for key in (users['alice'], readonly):
            for path in ['/api/gateway/audit', '/api/gateway/audit-list', '/api/audit?user=' + users['bob']['user_id']]:
                api('alice', path, expected=404, key=key)
        cli('key-revoke', '--key', readonly['key_id'])
        cli('user-disable', '--user', users['alice']['user_id'])
        disabled = audit('alice')
        assert disabled['events'][-1]['action'] == 'user_disabled'
        api('alice', '/api/sessions', expected=401)
        print('PASS: unknown outcome/manual review linkage; explicit JSON-escaped notes; disabled-user admin query and HTTP denial')

        # No writes during pagination: compare every page with the retained snapshot.
        all_events = audit('alice')['events']
        cursor, paged = 0, []
        for _ in range(len(all_events) + 1):
            page = audit('alice', cursor, limit=1)
            paged.extend(page['events'])
            assert int(page['next_after_seq']) > cursor
            cursor = int(page['next_after_seq'])
            if not page['has_more']:
                break
        assert paged == all_events
        bob_before = audit('bob')
        extra = cli('key-add', '--user', users['bob']['user_id'])
        cli('key-revoke', '--key', extra['key_id'])
        bob_after = audit('bob', bob_before['next_after_seq'])
        assert [event['action'] for event in bob_after['events']] == ['key_added', 'key_revoked']
        # Alice has no own new rows, but the cursor must advance over Bob's rows.
        empty = audit('alice', cursor)
        assert not empty['events'] and not empty['has_more'] and int(empty['next_after_seq']) > cursor
        assert empty['next_after_seq'] == empty['latest_seq']
        assert not audit('alice', empty['next_after_seq'])['events']
        cli('audit-list', '--user', str(uuid.uuid4()), ok=False)
        for args in [('--limit', '0'), ('--limit', '101'), ('--after-seq', '-1'), ('--after-seq', '+1'),
                     ('--after-seq', '0x1'), ('--after-seq', ' 1'), ('--after-seq', '1.0'),
                     ('--after-seq', '9223372036854775808'), ('--after-seq', str(int(empty['latest_seq']) + 1))]:
            cli('audit-list', '--user', users['alice']['user_id'], *args, ok=False)
        print('PASS: exact ascending cursor pages, per-user filtering, empty global watermark and fail-closed input bounds')

        stop(gateway)
        # Offline fault/retention fixture only. Existing production history above
        # is already verified; direct SQL is not evidence of generated events.
        with sqlite3.connect(registry) as connection:
            connection.execute('DELETE FROM audit_events WHERE seq<=?', (int(empty['latest_seq']) - 1,))
        gap = audit('alice')
        assert gap['retention_gap'] is True and not gap['events']
        assert gap['oldest_retained_seq'] == empty['latest_seq'] and gap['next_after_seq'] == empty['latest_seq']
        large = 9007199254740993
        with sqlite3.connect(registry) as connection:
            connection.execute('UPDATE sqlite_sequence SET seq=? WHERE name=?', (large, 'audit_events'))
        precise = audit('alice', empty['latest_seq'])
        assert precise['latest_seq'] == str(large) and precise['next_after_seq'] == str(large)
        with sqlite3.connect(registry) as connection:
            connection.execute('DELETE FROM audit_events')
        removed = audit('alice', large - 1)
        assert removed['oldest_retained_seq'] is None and removed['retention_gap'] and removed['latest_seq'] == str(large)
        with sqlite3.connect(registry) as connection:
            # Deliberately corrupt a historical row after gateway shutdown;
            # ordinary writers reject this oversized note at the DB boundary.
            connection.execute('PRAGMA ignore_check_constraints=ON')
            connection.execute('INSERT INTO audit_events(user_id,action,note,created_ms) VALUES(?,?,?,?)',
                               (users['alice']['user_id'], 'fixture_metadata', 'x' * 513, 0))
        assert audit('alice', large)['events'][0]['action'] == 'fixture_metadata'
        failure = cli('audit-list', '--user', users['alice']['user_id'], '--after-seq', str(large), '--include-notes', ok=False)
        assert 'x' * 64 not in failure.stderr
        with sqlite3.connect(registry) as connection:
            connection.execute('UPDATE audit_events SET note=NULL,action=?', ('invalid\naction',))
        failure = cli('audit-list', '--user', users['alice']['user_id'], '--after-seq', str(large), ok=False)
        assert 'invalid\naction' not in failure.stderr
        print('PASS: offline retention gap/removed history, exact >2^53 cursor, malformed-note opt-in and metadata rejection')
        with LOCK:
            assert not ERRORS and len(MODEL_CALLS) == 2, ERRORS
finally:
    RELEASE.set()
    for process in reversed(PROCESSES):
        stop(process)
    model.shutdown()
    model.server_close()
