#!/usr/bin/env python3
"""Completed native-tool result loss stops dispatch at public HTTP/CLI seams.

Actual binary, filesystem effects and localhost model protocol; no paid keys.
--exec additionally requires the explicitly selected, pre-pulled real Docker
sandbox used by CI. It never falls back to a host command or skips acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import time
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

parser = argparse.ArgumentParser()
parser.add_argument('binary', nargs='?', default='target/debug/jiaclaw')
parser.add_argument('--exec', action='store_true', dest='sandbox')
parser.add_argument('--tool-errors', action='store_true', help='exercise failed attempts and known local pure errors')
parser.add_argument('--legacy', action='store_true', help='exercise the existing development text-tool entry point')
args = parser.parse_args()
assert not args.legacy or args.tool_errors, '--legacy only qualifies failed-attempt behavior'
binary = Path(args.binary).resolve()
docker = os.environ.get('JIACLAW_TEST_DOCKER')
image = os.environ.get('JIACLAW_TEST_EXEC_IMAGE')
if args.sandbox:
    assert docker and Path(docker).is_absolute() and Path(docker).is_file()
    assert image and re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}', image)
    subprocess.run([docker, 'image', 'inspect', image], check=True, stdout=subprocess.DEVNULL)

posts = {}
fixture_errors = []
failures = []
reports = []
secret = 'fixture-model-' + uuid.uuid4().hex


def tool(call_id, name, arguments):
    return {'id': call_id, 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(arguments)}}


def first_calls(case):
    if case == 'pure-error':
        return [tool('read-error', 'file_read', {'path': 'missing.txt'})]
    oversized = case != 'bounded'
    if args.sandbox:
        command = 'printf x >> effect-count; '
        command += ('sleep 20' if args.tool_errors else "head -c 300000 /dev/zero | tr '\\000' a") if oversized else 'printf ok'
        calls = [tool('effect', 'exec', {'command': 'fixture', 'args': ['-c', command]})]
        pending = tool('pending', 'exec', {'command': 'fixture',
                                         'args': ['-c', 'printf unexpected > pending-effect']})
    else:
        calls = [tool('effect', 'file_write', {'path': 'effect-count', 'content': 'x', 'mode': 'append'}),
                 (tool('write-error', 'file_write', {'path': '../outside.txt', 'content': 'must not escape'})
                  if args.tool_errors and oversized else tool('read', 'file_read', {'path': 'output.txt'}))]
        pending = tool('pending', 'file_write', {'path': 'pending-effect', 'content': 'unexpected'})
    if case in ('same-batch', 'compat-sse', 'cli'):
        calls.append(pending)
    return calls


class Model(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        try:
            assert self.path == ('/chat/completions' if args.legacy else '/v1/chat/completions')
            assert self.headers['Authorization'] == 'Bearer ' + secret
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if not args.legacy:
                assert body['stream'] is False
            case = next(m['content'] for m in body['messages'] if m['role'] == 'user')
            observed = posts.setdefault(case, [])
            observed.append(body)
            calls = []
            if len(observed) == 1:
                calls = first_calls(case)
            elif case == 'next-round' and len(observed) == 2:
                # A model can propose the exact effect with a different ID. The
                # application must stop before asking it again after result loss.
                calls = first_calls(case)
                for call in calls:
                    call['id'] += '-again'
            message = {'role': 'assistant', 'content': None if calls else 'Fixture finished.'}
            if calls:
                if args.legacy:
                    message['content'] = '\n'.join('```tool\n' + json.dumps({
                        'tool_name': call['function']['name'],
                        'arguments': json.loads(call['function']['arguments'])}) + '\n```' for call in calls)
                else:
                    message['tool_calls'] = calls
            payload = json.dumps({'choices': [{'message': message,
                                 'finish_reason': 'tool_calls' if calls else 'stop'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)


model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
thread = threading.Thread(target=model.serve_forever, daemon=True)
thread.start()
env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
try:
    cases = ('bounded', 'same-batch', 'next-round', 'compat-sse', 'cli')
    if args.tool_errors:
        cases += ('pure-error',)
    for case in cases:
        process = None
        with tempfile.TemporaryDirectory(prefix='jiaclaw-result-limit-') as temporary:
            root = Path(temporary).resolve()
            workspace = root / 'workspace'
            workspace.mkdir()
            if args.sandbox:
                workspace.chmod(0o777)  # The non-root sandbox can write only this disposable root.
            # Legal UTF-8 below the file tool's 256 KiB input limit. JSON escaping
            # makes its completed result exceed the native wire-result budget.
            (workspace / 'output.txt').write_text('ok' if case == 'bounded' else '\\' * 100000)
            token = 'fixture-host-' + uuid.uuid4().hex
            config = root / 'config.json'
            settings = {
                'agent': {'name': 'result-limit', 'description': 'Fixture',
                          'system_instructions': 'Use authorized tools only.', 'max_turns': 10,
                          'max_tool_iterations': 4, 'workspace_path': str(workspace)},
                'provider': {'provider_type': 'openai_compatible' if args.legacy else 'brokerrouter', 'model': 'fixture',
                             'base_url': 'http://127.0.0.1:' + str(model.server_port), 'api_key': secret},
                'http': {'bind': '127.0.0.1:0', 'api_token': token, 'persist': True,
                         'persist_path': '../state/sessions.sqlite3', 'shutdown_timeout_secs': 1}}
            if args.sandbox:
                settings['tools'] = {'exec': {'enabled': True, 'docker_path': docker,
                    'image': image, 'commands': {'fixture': '/bin/sh'}, 'timeout_secs': 4 if args.tool_errors else 30,
                    'max_output_bytes': 400000, 'workspace_read_only': False, 'user': '65534:65534'}}
            config.write_text(json.dumps(settings))
            log = root / 'host.log'
            try:
                if case == 'cli':
                    cli = subprocess.run([str(binary), 'chat', '--config', str(config), '--no-auto-skill',
                                          '--session', 'result-limit-cli', case],
                                         env=env, capture_output=True, text=True, timeout=90)
                    assert cli.returncode == 0, cli.stderr
                    status = 'requireshumaninput' if '状态: RequiresHumanInput' in cli.stdout else 'completed'
                    count = cli.stdout.count('   • ')
                    error = ('effect_status' in cli.stdout and 'unknown' in cli.stdout) if args.tool_errors else 'do not replay' in cli.stdout
                    prior_posts = len(posts[case])
                    exported = root / 'history.jsonl'
                    subprocess.run([str(binary), 'session', 'export', 'result-limit-cli',
                                    '--config', str(config), '--output', str(exported)],
                                   env=env, check=True, capture_output=True, timeout=20)
                    history = [json.loads(line) for line in exported.read_text().splitlines()]
                    assert len(history) == 2 and history[-1]['role'] == 'assistant'
                    assert history[-1]['content'] in cli.stdout
                    if status == 'requireshumaninput':
                        assert '勿直接重试整个请求' in history[-1]['content']
                    assert len(posts[case]) == prior_posts, 'CLI history export submitted a model request'
                else:
                    with log.open('wb') as output:
                        process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
                                                  env=env, stdout=output, stderr=output)
                    deadline = time.monotonic() + 20
                    base = None
                    while time.monotonic() < deadline:
                        assert process.poll() is None, log.read_text()
                        match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
                        if match:
                            base = match.group(1)
                            break
                        time.sleep(.05)
                    assert base, log.read_text()
                    body = {'session_id': 'result-limit-' + case,
                            'messages': [{'role': 'user', 'content': case}],
                            'enabled_tools': ['file_read'] if case == 'pure-error' else (['exec'] if args.sandbox else ['file_write', 'file_read']),
                            'stream': case == 'compat-sse'}
                    request = urllib.request.Request(base + '/api/chat', data=json.dumps(body).encode(),
                        headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
                    with urllib.request.urlopen(request, timeout=90) as response:
                        assert response.status == 200
                        raw = response.read().decode()
                    if body['stream']:
                        frames = [frame for frame in raw.split('\n\n') if 'event: done\n' in frame]
                        assert len(frames) == 1, raw
                        result = json.loads(next(line[6:] for line in frames[0].splitlines() if line.startswith('data: ')))
                        status = result['status']
                        count = len([frame for frame in raw.split('\n\n') if 'event: tool\n' in frame])
                        # Compatibility SSE deliberately exposes tool summaries,
                        # not the full result object carried by JSON/CLI.
                        error = any('error' in json.loads(line[6:])
                                    for frame in raw.split('\n\n') if 'event: tool\n' in frame
                                    for line in frame.splitlines() if line.startswith('data: ')) if args.tool_errors else 'do not replay' in raw
                    else:
                        result = json.loads(raw)
                        status = result['status']
                        count = len(result['tool_calls'])
                        error = any(isinstance(call.get('result'), dict)
                                    and (call['result'].get('effect_status') == 'unknown' if args.tool_errors
                                         else 'do not replay' in call['result'].get('error', ''))
                                    for call in result['tool_calls'])
                    prior_posts = len(posts[case])
                    history_request = urllib.request.Request(base + '/api/sessions/result-limit-' + case,
                        headers={'Authorization': 'Bearer ' + token})
                    with urllib.request.urlopen(history_request, timeout=10) as response:
                        history = json.load(response)['messages']
                    assert len(history) == 2
                    if not body['stream']:
                        assert history[-1] == result['message']
                    assert len(posts[case]) == prior_posts, 'reading committed history submitted another model request'
                record = {'case': case, 'mode': 'exec' if args.sandbox else 'file', 'status': status,
                          'provider': 'legacy' if args.legacy else 'native', 'failure_mode': args.tool_errors,
                          'effect_count': len((workspace / 'effect-count').read_text()) if (workspace / 'effect-count').exists() else 0,
                          'pending_effect': (workspace / 'pending-effect').exists(),
                          'model_submissions': len(posts[case]), 'tool_records': count}
                reports.append(record)
                expected_records = 1 if args.sandbox or case == 'pure-error' else 2
                expected_status = 'completed' if case in ('bounded', 'pure-error') else 'requireshumaninput'
                if case == 'pure-error':
                    assert result['tool_calls'][0]['result']['effect_status'] == 'no_effect', result
                    assert 'error' in result['tool_calls'][0]['result'], result
                if not (status == expected_status and record['effect_count'] == (0 if case == 'pure-error' else 1)
                        and not record['pending_effect'] and count == expected_records
                        and len(posts[case]) == (2 if case in ('bounded', 'pure-error') else 1)
                        and (case in ('bounded', 'pure-error') or error)):
                    failures.append(record)
                assert not fixture_errors, fixture_errors
                assert secret not in (log.read_text() if log.exists() else '')
            finally:
                if process is not None and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
                if args.sandbox:
                    containers = subprocess.check_output([docker, 'ps', '-aq', '--filter',
                        'label=jiaclaw.workspace=' + str(workspace)], text=True).split()
                    if containers:
                        subprocess.run([docker, 'rm', '-f', *containers], check=True, stdout=subprocess.DEVNULL)
    print(json.dumps({'cases': reports}, ensure_ascii=False), flush=True)
    assert not failures, ('unknown failed attempt continued dispatch: ' if args.tool_errors else 'completed result loss continued dispatch: ') + json.dumps(failures)
    if args.tool_errors:
        print('PASS: unknown failed attempts stop batch/model dispatch in HTTP, compatibility SSE and CLI; trusted local pure errors remain recoverable')
    else:
        print('PASS: bounded output completes; completed result loss stops the batch and next model round in HTTP, compatibility SSE and ordinary CLI; effect/history retained')
finally:
    model.shutdown()
    model.server_close()
    thread.join(timeout=5)
