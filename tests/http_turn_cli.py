#!/usr/bin/env python3
"""Actual stopped-server receipt maintenance, lock exclusion and crash boundaries.

Disposable local model only. An offline lock is not evidence that upstream model
charges, container children or effects were reconciled. No credentials are read
by the CLI, and no tool/model resumption is certified here.
"""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import uuid
import http_turns as fixture

FIELDS = {'id', 'session_id', 'created_ms', 'finished_ms', 'state',
          'session_committed', 'cancel_requested', 'result_purged', 'reviewed_ms'}


def command(database, *args, error=None):
    before = time.monotonic()
    result = subprocess.run([str(fixture.binary), 'http-turns', '--database', str(database), *args],
                            env=fixture.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=10)
    assert time.monotonic() - before < 10
    assert fixture.provider_key.encode() not in result.stdout + result.stderr
    assert fixture.api_token.encode() not in result.stdout + result.stderr
    if error:
        assert result.returncode != 0 and error.encode() in result.stderr, result.stderr.decode()
        assert not result.stdout, result.stdout
        return
    assert result.returncode == 0, result.stderr.decode()
    value = json.loads(result.stdout)
    assert value['protocol'] == 1 and value['offline'] is True
    assert 'active' not in value  # Persisted state never asserts actual runtime ownership.
    return value


def snapshot(app):
    return {'turns': app.sql('SELECT * FROM http_turns ORDER BY id'),
            'sessions': app.sql('SELECT * FROM sessions ORDER BY id'),
            'ledger': app.ledger(), 'posts': len(fixture.posts), 'gets': len(fixture.gets),
            'effects': sorted(p.name for p in app.workspace.glob('*.txt'))}


try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-http-maintenance-') as temporary:
        root = Path(temporary)
        app = fixture.App(root, 'maintenance', timeout=15)
        app.start()
        completed, completed_body = app.submit('cli-completed')
        app.terminal(completed)
        gate = fixture.gate('cli-cancel')
        cancelled, _ = app.submit('cli-cancel')
        assert gate['ready'].wait(10)
        fixture.eventually(lambda: app.ledger()[-1]['remote_id'] is not None,
                           'actual gated model identity persisted')
        before = snapshot(app)
        for args in [('list',), ('get', completed),
                     ('review', cancelled, '--confirm-abandon', '--note', 'must not record'),
                     ('purge-result', completed, '--confirm-purge')]:
            command(app.db, *args, error='another JiaClaw process owns this session database')
        after = snapshot(app)
        assert after == before, {'before': before, 'after': after}
        app.http('/api/turns/' + cancelled + '/cancel', 'POST')
        gate['release'].set()
        app.terminal(cancelled, 'needs_review')
        app.stop()
        print('PASS maintenance 1: real serve lock excludes all four CLI actions without changing requests, model/effects or history', flush=True)

        # Neither configuration nor revoked runtime credentials are needed to
        # inspect this already-created database. Remove the actual config file.
        app.config.unlink()
        before = snapshot(app)
        value = command(app.db, 'list')
        assert [r['id'] for r in value['turns']] == [cancelled]
        assert set(value['turns'][0]) == FIELDS and not value['has_more']
        value = command(app.db, 'list', '--state', 'all', '--limit', '1')
        assert value['has_more'] and len(value['turns']) == 1
        next_page = command(app.db, 'list', '--state', 'all', '--limit', '1', '--offset', '1')
        assert not next_page['has_more'] and len(next_page['turns']) == 1
        assert {value['turns'][0]['id'], next_page['turns'][0]['id']} == {completed, cancelled}
        receipt = command(app.db, 'get', completed)['receipt']
        assert receipt['state'] == 'completed' and receipt['session_committed']
        assert receipt['result']['reply'] == '最终🦀回复'
        assert snapshot(app) == before
        for args in [('list', '--limit', '0'), ('list', '--limit', '51'),
                     ('list', '--offset', '10001'), ('list', '--state', 'invalid')]:
            command(app.db, *args, error='invalid value')
        command(app.db, 'get', completed.upper(), error='invalid_http_turn_id')
        command(app.db, 'get', str(uuid.uuid4()), error='http_turn_not_found')
        print('PASS maintenance 2: no config/Agent needed, payload-free finite paging and private original receipt leave rows and model ledger unchanged', flush=True)

        before = snapshot(app)
        for args in [('review', cancelled, '--note', 'missing confirmation'),
                     ('review', cancelled, '--confirm-abandon', '--note', ' '),
                     ('review', cancelled, '--confirm-abandon', '--note', '蟹' * 342)]:
            command(app.db, *args, error='review requires')
        command(app.db, 'purge-result', completed, error='requires --confirm-purge')
        command(app.db, 'review', completed, '--confirm-abandon', '--note', 'already completed', error='http_turn_review_not_required')
        assert snapshot(app) == before
        note = 'Operator separately reconciled the disposable model fixture and effects'
        reviewed = command(app.db, 'review', cancelled, '--confirm-abandon', '--note', note)['receipt']
        assert reviewed['state'] == 'needs_review' and reviewed['review_note'] == note
        assert reviewed['reviewed_ms'] is not None and reviewed['cancel_requested']
        assert command(app.db, 'review', cancelled, '--confirm-abandon', '--note', note)['receipt'] == reviewed
        command(app.db, 'review', cancelled, '--confirm-abandon', '--note', 'replace recorded decision', error='http_turn_review_already_recorded')
        assert command(app.db, 'list')['turns'] == []
        assert app.ledger() == before['ledger'] and len(fixture.posts) == before['posts']
        assert app.sql('SELECT * FROM sessions ORDER BY id') == before['sessions']
        print('PASS maintenance 3: explicit byte-bounded review, idempotent original decision and conflicting-note rejection; no model hold or history cleared', flush=True)

        before = snapshot(app)
        purged = command(app.db, 'purge-result', completed, '--confirm-purge')['receipt']
        assert purged['result'] is None and purged['result_purged'] and purged['state'] == 'completed'
        for key in ('id', 'session_id', 'created_ms', 'finished_ms', 'request_hash', 'context_hash', 'session_committed'):
            assert purged[key] == receipt[key]
        assert command(app.db, 'purge-result', completed, '--confirm-purge')['receipt'] == purged
        assert command(app.db, 'get', completed)['receipt'] == purged
        assert len(app.sql('SELECT id FROM http_turns')) == 2
        assert app.sql('SELECT * FROM sessions ORDER BY id') == before['sessions']
        assert app.ledger() == before['ledger'] and len(fixture.posts) == before['posts']
        print('PASS maintenance 4: actual terminal purge retains original identity, committed history and review without dispatch or replay', flush=True)

        # SIGKILL leaves the original running row. Merely opening/listing it must
        # not synthesize interruption or open/reconcile the private model ledger.
        app.save(); app.start()
        gate = fixture.gate('killed')
        original, body = app.submit('killed')
        assert gate['ready'].wait(10)
        app.stop(kill=True); gate['release'].set()
        before = snapshot(app)
        original_row = next(r for r in before['turns'] if r['id'] == original)
        assert original_row['state'] == 'running' and original_row['finished_ms'] is None
        assert command(app.db, 'list')['turns'][0]['state'] == 'running'
        assert command(app.db, 'get', original)['receipt']['state'] == 'running'
        command(app.db, 'purge-result', original, '--confirm-purge', error='http_turn_active')
        assert snapshot(app) == before
        reviewed = command(app.db, 'review', original, '--confirm-abandon', '--note', 'Explicit fixture-only abandonment after inspecting model and effects')['receipt']
        assert reviewed['state'] == 'needs_review' and reviewed['reviewed_ms'] is not None
        assert reviewed['error'] == 'owner_interrupted' and not reviewed['session_committed']
        assert app.ledger() == before['ledger'] and len(fixture.posts) == before['posts']
        assert not (app.workspace / 'killed.txt').exists()
        app.settings['http']['tracked_turns'] = False; app.save(); app.start()
        assert app.http('/api/turns/' + original)[1]['receipt'] == reviewed
        assert app.http('/api/turns/' + original, 'PUT', body)[0] == 200
        assert len(fixture.posts) == before['posts'] and not app.history(body['session_id'])
        app.stop()
        print('PASS maintenance 5: real SIGKILL preserves running identity until explicit offline review; restart disabled-admission GET/duplicate PUT never resumes model/effects', flush=True)

        before = snapshot(app)
        missing = root / 'not-created' / 'sessions.sqlite3'
        command(missing, 'list', error='existing session database required')
        assert not missing.parent.exists()
        command(Path(':memory:'), 'list', error='existing session database required')
        bad = root / 'wrong.sqlite3'
        bad.write_bytes(b'not a SQLite database at all'); bad.chmod(0o600)
        command(bad, 'list', error='requires an existing SQLite database')
        assert bad.read_bytes() == b'not a SQLite database at all'
        for version in (10, 12):
            app.sql(f'PRAGMA user_version={version}', write=True)
            command(app.db, 'list', error='requires schema 11')
            assert app.sql('PRAGMA user_version')[0]['user_version'] == version
        app.sql('PRAGMA user_version=11', write=True)
        app.sql('PRAGMA journal_mode=DELETE', write=True)
        command(app.db, 'list', error='requires an existing WAL database')
        assert app.sql('PRAGMA journal_mode')[0]['journal_mode'] == 'delete'
        app.sql('PRAGMA journal_mode=WAL', write=True)
        app.sql('CREATE TABLE gateway_fixture_owner(id TEXT)', write=True)
        command(app.db, 'list', error='rejects gateway channel databases')
        app.sql('DROP TABLE gateway_fixture_owner', write=True)
        app.sql("CREATE TRIGGER reject_maintenance BEFORE UPDATE ON http_turns BEGIN SELECT RAISE(ABORT,'unexpected fixture trigger'); END", write=True)
        command(app.db, 'list', error='HTTP turn schema does not match')
        app.sql('DROP TRIGGER reject_maintenance', write=True)
        if os.name == 'posix':
            alias = root / 'alias.sqlite3'
            alias.symlink_to(app.db)
            command(alias, 'list', error='requires a regular file')
            alias.unlink(); os.link(app.db, alias)
            command(app.db, 'list', error='rejects linked files')
            alias.unlink()
            app.db.chmod(0o640)
            command(app.db, 'list', error='requires private file permissions')
            app.db.chmod(0o600)
            lock_path = app.db.with_suffix('.sqlite3.lock')
            retained_lock = lock_path.with_suffix('.retained')
            lock_path.rename(retained_lock); lock_path.symlink_to(retained_lock)
            command(app.db, 'list', error='Too many levels')
            lock_path.unlink(); retained_lock.rename(lock_path)
            pipe = root / 'pipe.sqlite3'; os.mkfifo(pipe, 0o600)
            command(pipe, 'list', error='requires a regular file')
        assert snapshot(app) == before
        command(app.db, 'get', original)
        print('PASS maintenance 6: existing private unlinked v11 WAL only; missing/corrupt/future/old/private-channel/trigger/link/FIFO inputs fail closed without migration or row effects', flush=True)
        assert not fixture.faults, fixture.faults
finally:
    for gate in fixture.gates.values(): gate['release'].set()
    for process in fixture.processes:
        if process.poll() is None: process.kill(); process.wait(timeout=5)
    fixture.server.shutdown()
