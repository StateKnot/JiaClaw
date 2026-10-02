#!/usr/bin/env python3
"""Real binary/HTTP/SQLite scheduler acceptance without supplier credentials.

The gateway fixture holds submitted model calls across timeout and SIGKILL. This
proves persisted interruption and no automatic redispatch, not exactly-once
supplier execution or StateKnot durable recovery.
"""
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
import urllib.request
import uuid


binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
gateway_secret = 'fixture-gateway-' + uuid.uuid4().hex
api_token = 'fixture-host-' + uuid.uuid4().hex
fixture_errors = []
gateway_requests = {}
gateway_lock = threading.Lock()
release_requests = threading.Event()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == '/v1/chat/completions'
            assert self.headers['Authorization'] == 'Bearer ' + gateway_secret
            assert self.headers.get('Idempotency-Key')
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            prompt = next(message['content'] for message in reversed(request['messages'])
                          if message['role'] == 'user')
            case = next(name for name in ['complete', 'timeout', 'crash', 'skip', 'graceful', 'fault']
                        if 'scheduler-fixture:' + name in prompt)
            with gateway_lock:
                gateway_requests[case] = gateway_requests.get(case, 0) + 1
            assert {tool['function']['name'] for tool in request['tools']} == {'datetime_now'}
            if request['messages'][-1]['role'] == 'tool':
                assert request['messages'][-1]['tool_call_id'].startswith('clock-')
                assert 'Unix' in request['messages'][-1]['content']
                if case in ['timeout', 'crash', 'graceful']:
                    # Remain submitted until the test releases the fixture. A
                    # cancelled client must not create a fresh model request.
                    release_requests.wait(timeout=90)
                message = {'role': 'assistant', 'content': 'Scheduled ' + case + ' completed.'}
                finish_reason = 'stop'
            else:
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': 'clock-' + uuid.uuid4().hex, 'type': 'function',
                    'function': {'name': 'datetime_now', 'arguments': '{}'},
                }]}
                finish_reason = 'tool_calls'
            payload = json.dumps({'choices': [{'message': message, 'finish_reason': finish_reason}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError):
            # Expected when timeout/SIGKILL closes a submitted request.
            pass
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)


gateway = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
gateway.daemon_threads = True
gateway_thread = threading.Thread(target=gateway.serve_forever, daemon=True)
gateway_thread.start()
process = None
base = None
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-scheduler-') as directory:
        root = Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        config = root / 'config.json'
        settings = {
            'agent': {'name': 'scheduler-acceptance', 'description': 'Fixture',
                      'system_instructions': 'Use only the authorized clock tool.',
                      'max_turns': 10, 'workspace_path': str(workspace)},
            'provider': {'provider_type': 'brokerrouter',
                         'base_url': 'http://127.0.0.1:' + str(gateway.server_port),
                         'api_key': gateway_secret, 'model': 'fixture'},
            'http': {'bind': '127.0.0.1:0', 'api_token': api_token,
                     'persist': True, 'persist_path': '../state/sessions.sqlite3',
                     'shutdown_timeout_secs': 1},
            'scheduler': {'enabled': True},
        }
        env = {name: value for name, value in os.environ.items() if not name.startswith('JIACLAW_')}
        env['JIACLAW_LOG_LEVEL'] = 'info'

        def request(path, method='GET', body=None, authenticated=True):
            headers = {'Authorization': 'Bearer ' + api_token} if authenticated else {}
            if body is not None:
                headers['Content-Type'] = 'application/json'
            req = urllib.request.Request(base + path, method=method, headers=headers,
                                         data=json.dumps(body).encode() if body is not None else None)
            try:
                response = urllib.request.urlopen(req, timeout=10)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                raw = response.read()
                if not raw:
                    return response.status, None
                if 'application/json' in response.headers.get('Content-Type', ''):
                    return response.status, json.loads(raw)
                return response.status, raw.decode()

        def eventually(check, description, timeout=15):
            deadline = time.monotonic() + timeout
            last = None
            while time.monotonic() < deadline:
                assert process.poll() is None, log.read_text()
                assert not fixture_errors, fixture_errors
                last = check()
                if last:
                    return last
                time.sleep(.05)
            raise AssertionError(description + ' timed out; last=' + repr(last) + '\n' + log.read_text())

        def start():
            global process, base, log
            config.write_text(json.dumps(settings))
            log = root / ('server-' + uuid.uuid4().hex + '.log')
            with log.open('wb') as output:
                process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
                                           env=env, stdout=output, stderr=output)
            base = None
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert process.poll() is None, log.read_text()
                match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
                if match:
                    base = match.group(1)
                    assert request('/health')[0] == 200
                    return
                time.sleep(.05)
            raise AssertionError('server startup timed out: ' + log.read_text())

        def spec(name, seconds=1, timeout=20):
            return {'name': name, 'prompt': 'scheduler-fixture:' + name,
                    'schedule': {'kind': 'interval', 'seconds': seconds},
                    'enabled_tools': ['datetime_now'], 'timeout_secs': timeout}

        def create(body):
            status, job = request('/api/jobs', 'POST', body)
            assert status in [200, 201], (status, job)
            assert job['spec'] == body, job
            assert job['enabled'] and not job['deleted']
            assert job['session_id'] == 'job:' + job['id']
            assert job['next_due_ms'] > job['created_ms']
            return job

        def get(job):
            status, result = request('/api/jobs/' + job['id'])
            assert status == 200, (status, result)
            return result

        def runs(job):
            status, result = request('/api/jobs/' + job['id'] + '/runs')
            assert status == 200 and isinstance(result, list), (status, result)
            return result

        # Invalid scheduling setup must fail before any model request.
        for invalid in ['memory_store', 'no_api_token', 'legacy_provider', 'legacy_heartbeat']:
            rejected = json.loads(json.dumps(settings))
            if invalid == 'memory_store':
                rejected['http']['persist'] = False
            elif invalid == 'no_api_token':
                rejected['http'].pop('api_token')
            elif invalid == 'legacy_provider':
                rejected['provider']['provider_type'] = 'openai_compatible'
            else:
                rejected['heartbeat'] = {'enabled': True}
            config.write_text(json.dumps(rejected))
            outcome = subprocess.run([str(binary), 'serve', '--config', str(config)],
                                     env=env, capture_output=True, text=True, timeout=10)
            assert outcome.returncode != 0, invalid
            assert gateway_secret not in outcome.stdout + outcome.stderr
        assert gateway_requests == {}
        start()
        assert request('/api/jobs', authenticated=False)[0] == 401
        assert request('/api/jobs/status', authenticated=False)[0] == 401
        assert request('/api/jobs', 'POST', spec('complete'), authenticated=False)[0] == 401
        assert request('/api/jobs') == (200, [])
        status, health = request('/api/jobs/status')
        assert status == 200 and health['state'] == 'running' and health['max_concurrent_runs'] == 4, (status, health)
        for body in [
            {key: value for key, value in spec('complete').items() if key != 'enabled_tools'},
            {**spec('complete'), 'enabled_tools': []},
            {**spec('complete'), 'enabled_tools': ['unknown_tool']},
            {**spec('complete'), 'enabled_tools': ['http_get']},
            {**spec('complete'), 'schedule': {'kind': 'interval', 'seconds': 0}},
            {**spec('complete'), 'schedule': {'kind': 'cron', 'expression': '* *', 'timezone': 'UTC'}},
            {**spec('complete'), 'schedule': {'kind': 'cron', 'expression': '* * * * *', 'timezone': 'not/a/timezone'}},
            {**spec('complete'), 'timeout_secs': 0},
            {**spec('complete'), 'timeout_secs': 601},
        ]:
            status, result = request('/api/jobs', 'POST', body)
            assert 400 <= status < 500, (body, status, result)
        assert request('/api/jobs?limit=101')[0] == 400
        assert request('/api/jobs?limit=0')[0] == 400

        cron = create({**spec('complete'), 'name': 'daily-cron', 'schedule': {
            'kind': 'cron', 'expression': '0 0 * * *', 'timezone': 'Asia/Shanghai'}})
        assert request('/api/jobs/' + cron['id'], authenticated=False)[0] == 401
        assert request('/api/jobs/' + cron['id'] + '/runs', authenticated=False)[0] == 401
        assert request('/api/jobs/' + cron['id'] + '/pause', 'POST')[0] == 200
        assert not get(cron)['enabled']
        assert request('/api/jobs/' + cron['id'] + '/resume', 'POST')[0] == 200
        assert get(cron)['enabled']
        assert request('/api/jobs/' + cron['id'], 'DELETE')[0] in [200, 204]
        assert get(cron)['deleted'] and not get(cron)['enabled']
        assert request('/api/jobs/' + cron['id'] + '/resume', 'POST')[0] >= 400
        assert request('/api/jobs/' + cron['id'] + '?purge=true', 'DELETE')[0] in [200, 204]
        assert request('/api/jobs/' + cron['id'])[0] == 404

        completed = create(spec('complete'))
        completed_run = eventually(lambda: next((run for run in runs(completed)
                                                  if run['status'] == 'completed'), None), 'completed job')
        assert request('/api/jobs/' + completed['id'] + '/pause', 'POST')[0] == 200
        assert not get(completed)['enabled']
        assert completed_run['spec'] == completed['spec']
        assert completed_run['response']['message']['content'] == 'Scheduled complete completed.'
        assert completed_run['finished_ms'] >= completed_run['started_ms']
        assert completed_run['started_ms'] >= completed_run['scheduled_for_ms']
        status, history = request('/api/sessions/' + completed_run['session_id'])
        assert status == 200 and len(history['messages']) >= 2, (status, history)
        assert history['messages'][-1] == completed_run['response']['message']
        assert len(request('/api/jobs/' + completed['id'] + '/runs?limit=1')[1]) == 1
        assert request('/api/jobs/' + completed['id'] + '?purge=true', 'DELETE')[0] >= 400

        timed_out = create(spec('timeout', timeout=3))
        timeout_run = eventually(lambda: next((run for run in runs(timed_out)
                                                if run['status'] == 'interrupted'), None), 'job timeout')
        assert timeout_run['finished_ms'] is not None
        assert not get(timed_out)['enabled']
        assert gateway_requests.get('timeout') == 2

        crashing = create(spec('crash', timeout=60))
        eventually(lambda: gateway_requests.get('crash') == 2, 'submitted crash fixture')
        running = runs(crashing)
        assert len(running) == 1 and running[0]['status'] == 'running', running
        assert request('/api/jobs/' + crashing['id'] + '?purge=true', 'DELETE')[0] >= 400
        skipped = create(spec('skip'))
        # Kill before the new job's first one-second due time; downtime exceeds
        # the five-second lateness window, so restart must not replay it.
        process.kill()
        process.wait(timeout=5)
        counts_before_restart = dict(gateway_requests)
        time.sleep(6)
        start()
        assert request('/api/jobs/' + skipped['id'] + '/pause', 'POST')[0] == 200
        assert not get(crashing)['enabled']
        recovered = runs(crashing)
        assert len(recovered) == 1 and recovered[0]['status'] == 'interrupted', recovered
        assert recovered[0]['id'] == running[0]['id']
        assert recovered[0]['finished_ms'] is not None
        assert runs(timed_out)[0]['status'] == 'interrupted'
        assert not get(timed_out)['enabled'] and not get(completed)['enabled']
        assert runs(skipped) == []
        assert gateway_requests.get('skip', 0) == 0
        time.sleep(1.2)
        assert gateway_requests == counts_before_restart, (counts_before_restart, gateway_requests)
        assert len(request('/api/jobs?limit=1&offset=1')[1]) == 1

        assert request('/api/jobs/' + completed['id'], 'DELETE')[0] in [200, 204]
        assert get(completed)['deleted']
        assert not any(job['id'] == completed['id'] for job in request('/api/jobs')[1])
        assert any(job['id'] == completed['id'] and job['deleted'] for job in request('/api/jobs?include_deleted=true')[1])
        assert any(run['id'] == completed_run['id'] for run in runs(completed))
        assert request('/api/jobs/' + completed['id'] + '?purge=true', 'DELETE')[0] in [200, 204]
        assert request('/api/jobs/' + completed['id'])[0] == 404

        graceful = create(spec('graceful', timeout=60))
        eventually(lambda: gateway_requests.get('graceful') == 2, 'submitted graceful-shutdown fixture')
        graceful_run = runs(graceful)[0]
        assert graceful_run['status'] == 'running'
        process.terminate()
        process.wait(timeout=8)
        # Check the stopped database before startup recovery can mask a missing
        # shutdown transaction. This is a read-only check of disposable fixtures.
        database_path = (root / 'state' / 'sessions.sqlite3').resolve()
        assert database_path.is_file(), list(root.rglob('*'))
        wal = Path(str(database_path) + '-wal')
        assert not wal.exists() or wal.stat().st_size == 0, 'shutdown left an uncheckpointed WAL'
        # The process has exited and checkpointed the WAL. immutable avoids
        # system SQLite attempting to recreate WAL state for this read-only
        # snapshot (notably on macOS); it cannot hide uncommitted WAL changes.
        with sqlite3.connect(database_path.as_uri() + '?mode=ro&immutable=1', uri=True) as database:
            status, finished_ms = database.execute(
                'SELECT status,finished_ms FROM job_runs WHERE id=?', (graceful_run['id'],)).fetchone()
            assert status == 'interrupted' and finished_ms is not None, (status, finished_ms)
        graceful_count = gateway_requests['graceful']
        start()
        assert not get(graceful)['enabled']
        assert runs(graceful)[0]['id'] == graceful_run['id']
        assert runs(graceful)[0]['status'] == 'interrupted'
        time.sleep(1.2)
        assert gateway_requests['graceful'] == graceful_count

        # Inject a commit fault into this disposable database only. The trigger
        # affects completed run writes; interrupted recovery remains available.
        # A live SQLite transaction exercises the real worker/supervisor path,
        # without adding a fault-injection endpoint to the product.
        with sqlite3.connect(database_path.as_uri() + '?mode=rw', uri=True, timeout=5) as database:
            database.execute("""
                CREATE TRIGGER fixture_completed_write_fault
                BEFORE UPDATE ON job_runs WHEN NEW.status='completed'
                BEGIN SELECT RAISE(ABORT, 'fixture write fault'); END
            """)
        faulted = create(spec('fault'))
        eventually(lambda: request('/api/jobs/status')[1]['state'] == 'failed', 'worker commit failure')
        failed_run = eventually(lambda: next((run for run in runs(faulted)
                                               if run['status'] == 'interrupted'), None), 'failed worker recovery')
        assert not get(faulted)['enabled']
        assert gateway_requests.get('fault') == 2
        # The failed transaction must roll back the first session along with the
        # completed result, rather than publish a conversation without its run.
        assert request('/api/sessions/' + faulted['session_id'])[0] == 404
        assert request('/api/jobs', 'POST', spec('complete'))[0] == 503
        assert request('/api/jobs/' + faulted['id'] + '/resume', 'POST')[0] == 503
        with sqlite3.connect(database_path.as_uri() + '?mode=rw', uri=True, timeout=5) as database:
            database.execute('DROP TRIGGER fixture_completed_write_fault')
        assert request('/api/jobs/status')[1]['state'] == 'failed'
        process.terminate()
        process.wait(timeout=8)
        fault_count = gateway_requests['fault']
        start()
        assert request('/api/jobs/status')[1]['state'] == 'running'
        assert not get(faulted)['enabled']
        assert runs(faulted)[0]['id'] == failed_run['id']
        assert runs(faulted)[0]['status'] == 'interrupted'
        assert not get(crashing)['enabled'] and not get(timed_out)['enabled']
        assert not get(graceful)['enabled'] and not get(skipped)['enabled']
        time.sleep(1.2)
        assert gateway_requests['fault'] == fault_count
        assert gateway_secret not in log.read_text()
        assert api_token not in log.read_text()
        assert not fixture_errors, fixture_errors
        print('PASS: scheduler auth/config/CRUD/cron, scoped tools, completed session, timeout pause, SIGKILL and graceful interruption, skipped downtime, audit retention and transaction-fault admission shutdown/recovery')
finally:
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    release_requests.set()
    gateway.shutdown()
    gateway.server_close()
    gateway_thread.join(timeout=5)
