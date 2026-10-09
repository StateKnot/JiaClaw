#!/usr/bin/env python3
"""Authenticated, payload-free discovery on real SQLite/HTTP process boundaries.

Disposable local model contract only. Catalog discovery does not certify durable
resumption, tenant authority, model recovery or vendor services.
"""
import http.client
import json
from pathlib import Path
import tempfile
import uuid
import http_turns as fixture

FIELDS = {'id', 'session_id', 'created_ms', 'finished_ms', 'state',
          'session_committed', 'cancel_requested', 'result_purged', 'reviewed_ms'}


def page(app, query=''):
    status, value = app.http('/api/turns' + query)
    assert status == 200, value
    assert set(value) == {'protocol', 'state', 'limit', 'offset', 'has_more', 'turns'}
    assert value['protocol'] == 1
    assert len(value['turns']) <= value['limit'] <= 50
    assert all(set(row) == FIELDS for row in value['turns'])
    pairs = [(row['created_ms'], row['id']) for row in value['turns']]
    assert pairs == sorted(set(pairs), reverse=True)
    return value


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-http-catalog-') as temporary:
        app = fixture.App(Path(temporary), 'catalog', timeout=15)
        app.start()
        assert app.http('/api/turns/capabilities')[1]['listing'] is True
        invalid = ['?limit=0', '?limit=51', '?offset=10001', '?offset=-1',
                   '?limit=01', '?limit=%31', '?state=all&state=running',
                   '?limit=1&limit=1', '?state=ALL', '?unknown=1', '?state=all&',
                   '?offset=18446744073709551616', '?' + 'x' * 129]
        for query in invalid:
            assert app.http('/api/turns' + query, auth=False)[0] == 401
            assert app.http('/api/turns' + query)[0] == 400
        assert page(app)['turns'] == []
        connection = http.client.HTTPConnection('127.0.0.1', app.port, timeout=5)
        connection.request('GET', '/api/turns', headers={'Authorization': 'Bearer ' + fixture.api_token})
        response = connection.getresponse()
        assert response.status == 200 and response.getheader('Cache-Control') == 'no-store'
        response.read(); connection.close()
        assert not fixture.posts and not app.sql('SELECT id FROM http_turns')
        print('PASS catalog 1: auth precedes finite strict query validation, no-store discovery performs no admission/model/effects', flush=True)

        identities = []
        for index in range(6):
            identity, _ = app.submit('private-catalog-prompt-' + str(index))
            app.terminal(identity)
            identities.append(identity)
        app.http('/api/turns/' + identities[0] + '/result', 'DELETE')
        app.sql('UPDATE http_turns SET created_ms=100', write=True)
        # Exercise backwards wall time and a large retained body without reading
        # it through metadata. Production schema constraints stay enabled.
        app.sql("UPDATE http_turns SET finished_ms=1,result=? WHERE id=?", (json.dumps({'reply': 'private-result-' + 'x' * (1900 * 1024)}), identities[1]), write=True)
        seen = []
        for offset in (0, 2, 4):
            value = page(app, '?state=all&limit=2&offset=' + str(offset))
            seen.extend(row['id'] for row in value['turns'])
            assert value['has_more'] == (offset < 4)
            assert 'private-' not in json.dumps(value)
        assert seen == sorted(identities, reverse=True)
        assert page(app)['turns'] == []
        assert len(page(app, '?state=completed&limit=50')['turns']) == 6
        assert page(app, '?state=all&offset=10000')['turns'] == []
        count = len(fixture.posts)
        assert app.http('/api/turns/' + identities[0])[1]['receipt']['result_purged']
        assert len(fixture.posts) == count
        assert app.http('/api/sessions/' + app.http('/api/turns/' + identities[0])[1]['receipt']['session_id'], 'DELETE')[1]['success'] is True
        assert any(r['id'] == identities[0] and r['result_purged'] and r['session_committed'] for r in page(app, '?state=all&limit=50')['turns'])
        assert app.http('/api/turns/' + identities[0])[1]['receipt']['state'] == 'completed'
        assert len(fixture.posts) == count
        print('PASS catalog 2: real committed and purged identities, stable tie paging, large private result omitted without replay', flush=True)

        gate = fixture.gate('catalog-cancel')
        identity, body = app.submit('catalog-cancel')
        assert gate['ready'].wait(10)
        value = page(app)
        assert value['turns'][0]['id'] == identity and value['turns'][0]['state'] == 'running'
        assert 'active' not in value['turns'][0]
        assert app.http('/api/turns/' + identity + '/cancel', 'POST')[1]['receipt']['cancel_requested']
        gate['release'].set(); app.terminal(identity, 'needs_review')
        assert page(app)['turns'][0]['cancel_requested']
        count = len(fixture.posts)
        assert app.http('/api/turns/' + identity + '/review', 'POST', {'decision': 'abandon', 'note': 'private verified fixture note'})[0] == 200
        assert page(app)['turns'] == []
        reviewed = page(app, '?state=needs_review')['turns'][0]
        assert reviewed['id'] == identity and reviewed['reviewed_ms'] is not None
        assert len(fixture.posts) == count and not list(app.workspace.glob('*.txt'))
        print('PASS catalog 3: running/terminal/cancel/review filters use persisted metadata without asserting live resource ownership', flush=True)

        gate = fixture.gate('catalog-killed')
        original, body = app.submit('catalog-killed')
        assert gate['ready'].wait(10)
        app.stop(kill=True); gate['release'].set()
        app.settings['http']['tracked_turns'] = False; app.save(); app.start()
        cap = app.http('/api/turns/capabilities')[1]
        assert cap['listing'] and not cap['enabled'] and not cap['streaming']
        count = len(fixture.posts)
        restored = page(app)['turns']
        assert len(restored) == 1 and restored[0]['id'] == original and restored[0]['state'] == 'needs_review'
        receipt = app.http('/api/turns/' + original)[1]
        assert not receipt['active'] and receipt['receipt']['error'] == 'process_interrupted'
        assert app.http('/api/turns/' + str(uuid.uuid4()), 'PUT', body)[0] == 503
        assert len(fixture.posts) == count and not app.history(body['session_id'])
        assert app.ledger()[-1]['state'] == 'unknown'
        app.stop()
        print('PASS catalog 4: SIGKILL then disabled admission still discovers original unknown identity, no automatic execution or fake model recovery', flush=True)
        assert not fixture.faults, fixture.faults
finally:
    for gate in fixture.gates.values(): gate['release'].set()
    for process in fixture.processes:
        if process.poll() is None: process.kill(); process.wait(timeout=5)
    fixture.server.shutdown()
