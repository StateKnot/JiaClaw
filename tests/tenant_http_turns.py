#!/usr/bin/env python3
"""Two real private backends/gateway; no paid or supplier certification."""
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import uuid
from tenant_http_fixture import (binary, env, keys, lock, posts, gates, faults, processes,
                                 wait, port, sql, request, model, stop, launch)

try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-http-') as temporary:
        root = Path(temporary)
        registry = root / 'registry/users.sqlite3'
        gateway = {'bind': '127.0.0.1:' + str(port()), 'registry_path': str(registry),
                   'request_timeout_seconds': 15, 'max_in_flight': 4, 'tracked_turns': True, 'backends': []}
        config_path = root / 'gateway.json'
        gp = int(gateway['bind'].split(':')[1])
        backends = {}
        for who in ('alice', 'bob'):
            directory = root / who
            directory.mkdir(mode=0o700)
            workspace = directory / 'workspace'
            workspace.mkdir(mode=0o700)
            token = 'fixture-backend-' + uuid.uuid4().hex
            token_file = directory / 'token'
            token_file.write_text(token)
            token_file.chmod(0o600)
            number = port()
            config = {'agent': {'name': who, 'description': 'Disposable tenant HTTP acceptance',
                'system_instructions': 'Use only explicitly authorized tools.', 'max_turns': 10,
                'workspace_path': str(workspace), 'tool_timeout_secs': 2},
                'provider': {'provider_type': 'brokerrouter', 'base_url': f'http://127.0.0.1:{model.server_port}', 'api_key': keys[who], 'model': 'fixture-tenant-http'},
                'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
                'http': {'bind': f'127.0.0.1:{number}', 'persist': True, 'persist_path': '../state/sessions.sqlite3',
                         'api_token': token, 'tracked_turns': True, 'tracked_turn_timeout_secs': 10,
                         'shutdown_timeout_secs': 2, 'gateway_channel_chat': True}}
            path = directory / 'config.json'
            path.write_text(json.dumps(config))
            backends[who] = {'path': path, 'config': config, 'port': number, 'token': token,
                             'db': directory / 'state/sessions.sqlite3', 'ledger': directory / 'state/model-calls/index.sqlite3', 'log': directory / 'serve.log'}
            gateway['backends'].append({'id': who, 'url': f'http://127.0.0.1:{number}/', 'token_file': str(token_file)})
            backends[who]['process'] = launch(['serve', '--config', str(path)], number, backends[who]['log'])
        config_path.write_text(json.dumps(gateway))

        def cli(*args):
            result = subprocess.run([str(binary), 'gateway', *args, '--config', str(config_path)], env=env,
                                    capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, (args, result.stdout, result.stderr)
            return json.loads(result.stdout)
        users = {who: cli('user-add', '--backend', who) for who in ('alice', 'bob')}
        readonly = cli('key-add', '--user', users['alice']['user_id'], '--read-only')
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')

        def api(who, path, method='GET', body=None, expected=200, token=None):
            status, value, headers = request(gp, path, token or users[who]['token'], method, body)
            assert status == expected, (who, method, path, status, value)
            assert headers['Cache-Control'] == 'no-store' and headers['X-Content-Type-Options'] == 'nosniff'
            return value
        def hold(who):
            return sql(registry, 'SELECT * FROM write_holds WHERE user_id=?', (users[who]['user_id'],))
        def body(prompt, session=None):
            return {'session_id': session or 'http:' + str(uuid.uuid4()), 'prompt': prompt, 'enabled_tools': ['datetime_now']}
        def complete(who, identity):
            def done():
                value = api(who, '/api/turns/' + identity)
                return value if value['receipt']['state'] != 'running' and not value['active'] else None
            return wait(done, 'terminal original receipt')
        def gate(who, prompt):
            v = {'ready': threading.Event(), 'release': threading.Event()}
            gates[(who, prompt)] = v
            return v
        def count(who, prompt):
            with lock:
                return sum(p['who'] == who and p['prompt'] == prompt for p in posts)

        assert request(gp, '/api/turns/capabilities')[0] == 401
        assert api('alice', '/api/turns/capabilities')['streaming'] is False
        a = backends['alice']
        assert request(a['port'], '/api/turns/capabilities', a['token'])[0] == 503
        assert request(a['port'], '/api/turns/capabilities', a['token'], marker=True)[1]['gateway_protocol'] == 1
        wrong = str(uuid.uuid4())
        assert request(a['port'], '/api/turns/' + wrong + '/stream', a['token'], 'PUT', body('forbidden-stream'), marker=True)[0] == 503
        for suffix in ('/stream', '/review', '/purge-result', '?backend=bob'):
            api('alice', '/api/turns/' + wrong + suffix, expected=404)
        api('alice', '/api/turns?limit=1&limit=2', expected=404)
        for fields in ({'enabled_tools': []}, {'enabled_tools': ['file_write']}, {'enabled_skills': ['unexpected']}, {'session_id': 'legacy'}):
            api('alice', '/api/turns/' + wrong, 'PUT', dict(body('forbidden'), **fields), expected=400)
        api('alice', '/api/turns/' + wrong, 'PUT', body('readonly'), expected=403, token=readonly['token'])
        api('alice', '/api/turns/' + wrong, 'PUT', {'padding': 'x' * 65536}, expected=413)
        # Reject a read-only partial body before waiting for its missing bytes.
        with socket.create_connection(('127.0.0.1', gp), timeout=2) as s:
            s.sendall((f'PUT /api/turns/{wrong} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {readonly["token"]}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{').encode())
            assert b'403' in s.recv(4096).split(b'\r\n', 1)[0]
        with socket.create_connection(('127.0.0.1', gp), timeout=7) as s:
            s.sendall((f'PUT /api/turns/{wrong} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {users["alice"]["token"]}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{').encode())
            started = time.monotonic()
            assert b'408' in s.recv(4096).split(b'\r\n', 1)[0]
            assert 4.5 <= time.monotonic() - started < 7
        assert not posts and not hold('alice') and not sql(registry, 'SELECT * FROM http_turn_requests')
        print('PASS: tenant mode/authentication/readonly/strict routes and tools before body or effects', flush=True)

        identity = str(uuid.uuid4())
        value = body('same-original')
        g = gate('alice', 'same-original')
        initial = api('alice', '/api/turns/' + identity, 'PUT', value, expected=202)
        assert initial['receipt']['id'] == identity and g['ready'].wait(10)
        wait(lambda: sql(a['ledger'], 'SELECT remote_id FROM model_calls WHERE turn_id=?', (identity,))[0]['remote_id'], 'persisted model remote ID')
        assert hold('alice')[0]['request_id'] == identity
        assert sql(a['db'], 'SELECT id FROM http_turns')[0]['id'] == identity
        normalized = dict(value, enabled_skills=[])
        assert initial['receipt']['request_hash'] == hashlib.sha256(json.dumps(normalized, ensure_ascii=False, separators=(',', ':')).encode()).hexdigest()
        assert api('alice', '/api/turns/' + identity, 'PUT', normalized)['active']
        api('alice', '/api/turns/' + identity, 'PUT', dict(value, prompt='changed'), expected=409)
        api('bob', '/api/turns/' + identity, expected=404)
        assert api('alice', '/api/turns/' + identity, token=readonly['token'])['active']
        api('alice', '/api/turns/' + identity + '/cancel', 'POST', expected=403, token=readonly['token'])
        assert api('alice', '/api/turns?limit=1')['requests'][0]['id'] == identity
        assert api('bob', '/api/turns')['requests'] == []
        api('alice', '/api/chat', 'POST', {'message': 'overlap'}, expected=429)
        bvalue = body('bob-isolated')
        api('bob', '/api/turns/' + identity, 'PUT', bvalue, expected=202)
        assert complete('bob', identity)['receipt']['state'] == 'completed'
        g['release'].set()
        assert complete('alice', identity)['receipt']['state'] == 'completed'
        wait(lambda: not hold('alice') and not hold('bob'), 'shared hold settlement')
        assert count('alice', 'same-original') == 1 and count('bob', 'bob-isolated') == 1
        assert json.loads(sql(a['db'], 'SELECT messages FROM sessions WHERE id=?', (value['session_id'],))[0]['messages'])[-1]['content'] == 'alice: same-original'
        api('alice', '/api/turns/' + identity, 'PUT', value)
        assert count('alice', 'same-original') == 1
        print('PASS: same original IDs across registry/backend/model ledger, isolated keys/catalog and lookup-only duplicates', flush=True)

        cancel_id = str(uuid.uuid4())
        g = gate('alice', 'cancel-original')
        api('alice', '/api/turns/' + cancel_id, 'PUT', body('cancel-original'), expected=202)
        assert g['ready'].wait(10)
        assert api('alice', '/api/turns/' + cancel_id + '/cancel', 'POST')['receipt']['cancel_requested']
        g['release'].set()
        assert complete('alice', cancel_id)['receipt']['state'] == 'needs_review'
        wait(lambda: hold('alice') and hold('alice')[0]['state'] == 'needs_review', 'canceled shared hold')
        api('alice', '/api/turns/' + str(uuid.uuid4()), 'PUT', body('blocked-after-cancel'), expected=409)
        cli('review-clear', '--user', users['alice']['user_id'], '--confirm-backend-idle', '--note', 'Fixture original cancellation inspected; no replay')
        assert count('alice', 'blocked-after-cancel') == 0
        print('PASS: original durable cancellation and uncertain user hold block further writes until explicit review', flush=True)

        restart_id = str(uuid.uuid4())
        rvalue = body('gateway-kill')
        g = gate('alice', 'gateway-kill')
        api('alice', '/api/turns/' + restart_id, 'PUT', rvalue, expected=202)
        assert g['ready'].wait(10)
        stop(process, kill=True)
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        assert hold('alice')[0]['state'] == 'needs_review'
        assert api('alice', '/api/turns/' + restart_id)['active']
        api('alice', '/api/turns/' + restart_id, 'PUT', rvalue)
        g['release'].set()
        assert complete('alice', restart_id)['receipt']['state'] == 'completed'
        assert hold('alice')[0]['state'] == 'needs_review', 'GET must not settle a restarted owner'
        cli('review-clear', '--user', users['alice']['user_id'], '--confirm-backend-idle', '--note', 'Fixture original completed receipt and ledger checked; no replay')
        api('alice', '/api/turns/' + restart_id, 'PUT', rvalue)
        assert count('alice', 'gateway-kill') == 1 and not hold('alice')
        stop(process)
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        print('PASS: real gateway SIGKILL preserves original identity and review hold without polling owner or redispatch', flush=True)

        lost_id = str(uuid.uuid4())
        lost_body = body('backend-kill')
        g = gate('alice', 'backend-kill')
        api('alice', '/api/turns/' + lost_id, 'PUT', lost_body, expected=202)
        assert g['ready'].wait(10)
        stop(a['process'], kill=True)
        g['release'].set()
        wait(lambda: hold('alice') and hold('alice')[0]['state'] == 'needs_review', 'unknown backend hold')
        a['process'] = launch(['serve', '--config', str(a['path'])], a['port'], a['log'])
        assert complete('alice', lost_id)['receipt']['state'] == 'needs_review'
        api('alice', '/api/turns/' + lost_id, 'PUT', lost_body)
        cli('review-clear', '--user', users['alice']['user_id'], '--confirm-backend-idle', '--note', 'Fixture killed backend has no active owner; original retained and never replayed')
        api('alice', '/api/turns/' + lost_id, 'PUT', lost_body)
        assert count('alice', 'backend-kill') == 1
        stop(process)
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        # A lost receipt must retain gateway admission identity after manual review.
        stop(a['process'])
        old = a['db']
        old.rename(old.with_name('retained-original.sqlite3'))
        a['process'] = launch(['serve', '--config', str(a['path'])], a['port'], a['log'])
        api('alice', '/api/turns/' + lost_id, 'PUT', lost_body, expected=409)
        assert count('alice', 'backend-kill') == 1 and not hold('alice')
        assert any(r['id'] == lost_id for r in api('alice', '/api/turns')['requests'])
        print('PASS: actual backend restart/lost database and manual hold clear never remove identity or resend PUT', flush=True)

        before = len(posts)
        sql(registry, "CREATE TRIGGER fixture_fail_admission BEFORE INSERT ON audit_events WHEN NEW.action='http_turn_admitted' BEGIN SELECT RAISE(ABORT,'fixture failure'); END", write=True)
        failed_id = str(uuid.uuid4())
        api('alice', '/api/turns/' + failed_id, 'PUT', body('audit-rollback'), expected=503)
        assert not hold('alice') and not sql(registry, 'SELECT id FROM http_turn_requests WHERE id=?', (failed_id,))
        sql(registry, 'DROP TRIGGER fixture_fail_admission', write=True)
        stop(a['process'])
        a['config']['http']['tracked_turns'] = False
        a['path'].write_text(json.dumps(a['config']))
        a['process'] = launch(['serve', '--config', str(a['path'])], a['port'], a['log'])
        api('alice', '/api/turns/' + str(uuid.uuid4()), 'PUT', body('disabled-admission'), expected=503)
        assert not hold('alice') and len(posts) == before
        stop(process)
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        api('alice', '/api/turns/' + lost_id, expected=409)
        # Stale gateway capability cannot admit to a standalone backend.
        stop(a['process'])
        a['config']['http']['gateway_channel_chat'] = False
        a['path'].write_text(json.dumps(a['config']))
        a['process'] = launch(['serve', '--config', str(a['path'])], a['port'], a['log'])
        api('alice', '/api/turns/' + str(uuid.uuid4()), 'PUT', body('mode-changed'), expected=503)
        assert not hold('alice') and len(posts) == before
        stop(process)
        failed = subprocess.run([str(binary), 'gateway', 'serve', '--config', str(config_path)], env=env, capture_output=True, timeout=15)
        assert failed.returncode != 0
        assert sql(registry, 'PRAGMA user_version')[0]['user_version'] == 8
        assert not faults, faults
        print('PASS: atomic admission rollback, disabled-backend reads and startup/live mode handshake fail before model dispatch', flush=True)

        # The gateway's observation deadline is shorter than the actual backend owner.
        # Unknown work still consumes global/user capacity; a GET cannot reclaim it.
        a['config']['http']['gateway_channel_chat'] = True
        a['path'].write_text(json.dumps(a['config']))
        stop(a['process'])
        a['process'] = launch(['serve', '--config', str(a['path'])], a['port'], a['log'])
        b = backends['bob']
        stop(b['process'])
        b['config']['http']['tracked_turn_timeout_secs'] = 30
        b['path'].write_text(json.dumps(b['config']))
        b['process'] = launch(['serve', '--config', str(b['path'])], b['port'], b['log'])
        gateway['max_in_flight'] = 1
        gateway['request_timeout_seconds'] = 10
        config_path.write_text(json.dumps(gateway))
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        parked_id = str(uuid.uuid4())
        parked_body = body('observation-deadline')
        g = gate('bob', 'observation-deadline')
        api('bob', '/api/turns/' + parked_id, 'PUT', parked_body, expected=202)
        assert g['ready'].wait(10)
        wait(lambda: hold('bob') and hold('bob')[0]['state'] == 'needs_review', 'finite observation deadline', seconds=13)
        assert api('bob', '/api/turns/' + parked_id)['active'] is True
        api('alice', '/api/turns/' + str(uuid.uuid4()), 'PUT', body('blocked-global-reservation'), expected=429)
        stop(process)
        gateway['tracked_turns'] = False
        config_path.write_text(json.dumps(gateway))
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        api('alice', '/api/chat', 'POST', {'message': 'disabled-feature-capacity-bypass'}, expected=429)
        stop(process)
        gateway['tracked_turns'] = True
        config_path.write_text(json.dumps(gateway))
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        api('alice', '/api/turns/' + str(uuid.uuid4()), 'PUT', body('blocked-restarted-reservation'), expected=429)
        g['release'].set()
        assert complete('bob', parked_id)['receipt']['state'] == 'completed'
        cli('review-clear', '--user', users['bob']['user_id'], '--confirm-backend-idle', '--note', 'Fixture original receipt and stopped backend owner inspected; reclaim only by restart')
        api('bob', '/api/turns/' + str(uuid.uuid4()), 'PUT', body('still-reserved-before-restart'), expected=429)
        stop(process)
        process = launch(['gateway', 'serve', '--config', str(config_path)], gp, root / 'gateway.log')
        after_id = str(uuid.uuid4())
        api('bob', '/api/turns/' + after_id, 'PUT', body('after-reviewed-restart'), expected=202)
        assert complete('bob', after_id)['receipt']['state'] == 'completed'
        wait(lambda: not hold('bob'), 'reviewed capacity restored')
        assert count('bob', 'observation-deadline') == 1
        assert all(count(w, p) == 0 for w, p in [('alice', 'blocked-global-reservation'), ('alice', 'blocked-restarted-reservation'), ('bob', 'still-reserved-before-restart')])
        print('PASS: original deadline retains unknown global/user capacity, reconstructs on restart and reclaims only after idle review/restart', flush=True)
finally:
    for g in gates.values():
        g['release'].set()
    for p in reversed(processes):
        stop(p)
    model.shutdown()
