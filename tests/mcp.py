#!/usr/bin/env python3
"""Binary + real StateKnot HTTP MCP + gateway fixture + SQLite acceptance.

Fixtures exercise actual network/protocol adapters, without real supplier credentials.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.request
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
descriptor = {
    'name': 'lookup', 'description': 'Look up an approved inventory record.',
    'inputSchema': {'type': 'object', 'properties': {'query': {'type': 'string'}},
                    'required': ['query'], 'additionalProperties': False},
    'outputSchema': {'type': 'object', 'properties': {'found': {'type': 'boolean'}}, 'required': ['found']}
}
mcp_secret = 'fixture-mcp-' + uuid.uuid4().hex
gateway_secret = 'fixture-gateway-' + uuid.uuid4().hex
secret_env = 'JIACLAW_MCP_FIXTURE_' + uuid.uuid4().hex.upper()
calls = []
gateway_messages = []
fixture_errors = []
native_call = {'id': 'mcp-fixture-call', 'type': 'function', 'function': {
    'name': 'mcp_inventory_lookup', 'arguments': json.dumps({'query': 'abc'})}}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path == '/v1/chat/completions':
                assert self.headers['Authorization'] == 'Bearer ' + gateway_secret
                assert self.headers.get('Idempotency-Key')
                messages = request['messages']
                gateway_messages.append(messages)
                prompt = messages[0]['content']
                advertised = {tool['function']['name']: tool for tool in request['tools']}
                assert 'mcp_inventory_lookup' in advertised
                assert advertised['mcp_inventory_lookup']['function']['parameters'] == descriptor['inputSchema']
                assert 'mcp_inventory_unapproved_write' not in advertised
                assert 'unapproved_write' not in json.dumps(request)
                assert 'UNTRUSTED-SERVER-INSTRUCTIONS' not in prompt
                if any(message['role'] == 'tool' for message in messages):
                    assert messages[-2] == {'role': 'assistant', 'content': None, 'tool_calls': [native_call]}
                    assert messages[-1]['role'] == 'tool'
                    assert messages[-1]['tool_call_id'] == native_call['id']
                    assert 'fixture record' in messages[-1]['content']
                    message = {'role': 'assistant', 'content': 'MCP roundtrip completed.'}
                    finish_reason = 'stop'
                else:
                    message = {'role': 'assistant', 'content': None, 'tool_calls': [native_call]}
                    finish_reason = 'tool_calls'
                response = {'choices': [{'message': message, 'finish_reason': finish_reason}]}
            elif self.path == '/mcp/':
                assert self.headers['Authorization'] == 'Bearer ' + mcp_secret
                method = request['method']
                calls.append(method)
                if method == 'server/discover':
                    result = {'resultType': 'complete', 'supportedVersions': ['2026-07-28'],
                              'capabilities': {'tools': {}}, 'ttlMs': 0, 'cacheScope': 'private',
                              'instructions': 'UNTRUSTED-SERVER-INSTRUCTIONS',
                              '_meta': {'io.modelcontextprotocol/serverInfo': {'name': 'fixture', 'version': '1'}}}
                elif method == 'tools/list':
                    result = {'resultType': 'complete', 'tools': [descriptor, {
                        'name': 'unapproved_write', 'inputSchema': {'type': 'object'},
                        'annotations': {'readOnlyHint': True}}]}
                elif method == 'tools/call':
                    assert request['params']['name'] == 'lookup'
                    assert request['params']['arguments'] == {'query': 'abc'}
                    assert self.headers['MCP-Protocol-Version'] == '2026-07-28'
                    result = {'resultType': 'complete', 'content': [{'type': 'text', 'text': 'fixture record'}],
                              'structuredContent': {'found': True}, 'isError': False}
                else:
                    raise AssertionError(method)
                response = {'jsonrpc': '2.0', 'id': request['id'], 'result': result}
            else:
                raise AssertionError(self.path)
            payload = json.dumps(response).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except Exception as error:
            fixture_errors.append(repr(error))
            self.send_error(500)


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
endpoint = 'http://127.0.0.1:' + str(server.server_port)
env = dict(os.environ)
for key in ['JIACLAW_API_KEY', 'JIACLAW_API_TOKEN', 'JIACLAW_LOG_FORMAT']:
    env.pop(key, None)
env[secret_env] = mcp_secret
env['JIACLAW_LOG_LEVEL'] = 'info'
process = None
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-mcp-e2e-') as directory:
        root = Path(directory)
        (root / 'workspace').mkdir()
        inspection = subprocess.run([str(binary), 'mcp-inspect', '--endpoint', endpoint + '/mcp/',
                                    '--bearer-token-env', secret_env], env=env, capture_output=True, text=True, timeout=40)
        assert inspection.returncode == 0, inspection.stderr
        assert mcp_secret not in inspection.stdout + inspection.stderr
        review = json.loads(inspection.stdout)
        reviewed = next(tool for tool in review['tools'] if tool['descriptor']['name'] == 'lookup')
        assert reviewed['descriptor'] == descriptor  # Approve the expected fixture, never arbitrary discovery.
        assert calls == ['server/discover', 'tools/list']
        missing_env = dict(env)
        missing_env.pop(secret_env)
        missing = subprocess.run([str(binary), 'mcp-inspect', '--endpoint', endpoint + '/mcp/',
                                 '--bearer-token-env', secret_env], env=missing_env, capture_output=True, text=True, timeout=10)
        assert missing.returncode != 0
        assert calls == ['server/discover', 'tools/list']  # A missing credential never becomes anonymous.
        api_token = 'fixture-host-' + uuid.uuid4().hex
        config = root / 'config.json'
        policy = {
            'agent': {'name': 'mcp-acceptance', 'description': 'Fixture', 'system_instructions': 'Use reviewed tools.',
                      'max_turns': 10, 'workspace_path': str(root / 'workspace')},
            'provider': {'provider_type': 'brokerrouter', 'base_url': endpoint, 'api_key': gateway_secret, 'model': 'fixture'},
            'http': {'bind': '127.0.0.1:0', 'persist': True, 'persist_path': '../state/sessions.sqlite3'},
            'mcp': {'servers': [{'name': 'inventory', 'endpoint': endpoint + '/mcp/', 'bearer_token_env': secret_env,
                                'tools': [{'name': 'lookup', 'alias': 'lookup', 'effect': 'read_only',
                                           'descriptor_sha256': reviewed['descriptor_sha256']}]}]}
        }
        config.write_text(json.dumps(policy))
        before = len(calls)
        rejected = subprocess.run([str(binary), 'serve', '--config', str(config)], env=env, capture_output=True, text=True, timeout=10)
        assert rejected.returncode != 0 and 'API Token' in rejected.stderr
        assert len(calls) == before  # Local authorization precedes outbound discovery.

        cli = subprocess.run([str(binary), 'chat', '--config', str(config), '--session', 'mcp-cli', 'look up abc'],
                             env=env, capture_output=True, text=True, timeout=30)
        assert cli.returncode == 0, cli.stderr
        assert 'MCP roundtrip completed.' in cli.stdout
        assert calls.count('tools/call') == 1
        assert mcp_secret not in cli.stdout + cli.stderr

        policy['http']['api_token'] = api_token
        config.write_text(json.dumps(policy))
        log = root / 'server.log'
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config)], env=env, stdout=output, stderr=output)
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

        def request(path, data=None):
            headers = {'Authorization': 'Bearer ' + api_token, 'Content-Type': 'application/json'}
            req = urllib.request.Request(base + path, data=json.dumps(data).encode() if data is not None else None, headers=headers)
            with urllib.request.urlopen(req, timeout=20) as response:
                return json.load(response)

        assert len(request('/api/sessions/mcp-cli')['messages']) == 2
        result = request('/api/chat', {'session_id': 'mcp-http', 'messages': [{'role': 'user', 'content': 'look up abc'}]})
        assert result['message']['content'] == 'MCP roundtrip completed.'
        assert result['tool_calls'][0]['tool_name'] == 'mcp_inventory_lookup'
        assert calls.count('tools/call') == 2
        assert len(request('/api/sessions/mcp-http')['messages']) == 2
        assert mcp_secret not in log.read_text()
        assert not fixture_errors, fixture_errors
        assert len(gateway_messages) == 4
        print('MCP binary/gateway/HTTP/SQLite acceptance passed')
finally:
    if process is not None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)
