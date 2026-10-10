#!/usr/bin/env python3
"""Real-binary doctor checks for truthful readiness and explicit network effects."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()


class RejectMcp(BaseHTTPRequestHandler):
    requests = 0
    paths = []
    bodies = []

    def do_POST(self):
        type(self).requests += 1
        type(self).paths.append(self.path)
        type(self).bodies.append(self.rfile.read(int(self.headers.get('Content-Length', '0'))))
        self.send_response(503)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def log_message(self, *_args):
        pass


with tempfile.TemporaryDirectory(prefix='jiaclaw-doctor-') as temp:
    root = Path(temp).resolve()
    workspace = root / 'workspace'
    workspace.mkdir()
    (workspace / 'AGENTS.md').write_text('Fixture workspace.\n')
    state = root / 'private-state'
    config_path = root / 'config.json'
    config = {
        'agent': {
            'name': 'doctor-fixture', 'description': 'test',
            'system_instructions': 'test', 'max_turns': 1,
            'workspace_path': str(workspace),
        },
        'provider': {'provider_type': 'brokerrouter', 'model': 'fixture-model', 'base_url': 'http://127.0.0.1:1'},
        'model_calls': {'enabled': True, 'store_path': str(state / 'model-calls.sqlite3')},
        'memory': {'semantic': {
            'enabled': True, 'model': 'fixture-embed', 'space_revision': 'fixture-v1',
            'dimensions': 8, 'index_path': str(state / 'semantic.sqlite3'),
        }},
        'mcp': {'servers': []},
    }
    config_path.write_text(json.dumps(config))
    env = dict(os.environ)
    env.pop('JIACLAW_API_KEY', None)
    env.pop('JIACLAW_BRAVE_API_KEY', None)
    env.pop('JIACLAW_DOCTOR_MCP_TOKEN', None)

    def run(*arguments, environment=None):
        return subprocess.run(
            [str(binary), 'doctor', '--config', str(config_path), *arguments],
            env=environment or env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=20,
        )

    missing_key = run()
    assert missing_key.returncode != 0, missing_key.stdout
    assert '不会自动回退到 stub' in missing_key.stdout, missing_key.stdout
    assert '将使用存根模式' not in missing_key.stdout, missing_key.stdout
    assert '默认只做本地只读检查' in missing_key.stdout, missing_key.stdout
    assert not state.exists(), 'default doctor initialized persistent state'

    configured_key_env = dict(env, JIACLAW_API_KEY='fixture-not-a-real-key')
    config['provider'] = {'provider_type': 'brokerrouter', 'model': 'fixture-model', 'base_url': 'http://gateway.example/v1'}
    config_path.write_text(json.dumps(config))
    invalid_endpoint = run(environment=configured_key_env)
    assert invalid_endpoint.returncode != 0, invalid_endpoint.stdout
    assert '提供商就绪状态: ❌' in invalid_endpoint.stdout, invalid_endpoint.stdout
    assert '配置尚未就绪' in invalid_endpoint.stdout, invalid_endpoint.stdout
    assert 'fixture-not-a-real-key' not in invalid_endpoint.stdout, invalid_endpoint.stdout
    assert 'Base URL: ✅ 满足安全端点格式要求' not in invalid_endpoint.stdout, invalid_endpoint.stdout

    config['provider'] = {'provider_type': 'stub', 'model': 'fixture-model', 'base_url': 'http://127.0.0.1:1'}
    config['model_calls']['enabled'] = False
    config['memory']['semantic']['enabled'] = False
    config_path.write_text(json.dumps(config))
    stub = run()
    assert stub.returncode == 0, stub.stdout
    assert '显式 stub' in stub.stdout, stub.stdout
    assert '当前 provider 需要 API Key' not in stub.stdout, stub.stdout
    assert 'API Key: 不需要（显式 stub）' in stub.stdout, stub.stdout
    assert 'HTTP MCP: 已接线' in stub.stdout, stub.stdout
    assert 'StateKnot durable driver: 未接线' in stub.stdout, stub.stdout
    assert '会话存储: SQLite（配置已启用；doctor 不打开会话库）' in stub.stdout, stub.stdout
    assert '等待稳定 API 发布' not in stub.stdout, stub.stdout
    assert not state.exists(), 'default doctor initialized disabled persistent state'

    config['http'] = {'persist': False}
    config_path.write_text(json.dumps(config))
    memory = run()
    assert memory.returncode == 0, memory.stdout
    assert '会话存储: 内存（重启不保留）' in memory.stdout, memory.stdout

    server = ThreadingHTTPServer(('127.0.0.1', 0), RejectMcp)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    config['provider'] = {
        'provider_type': 'brokerrouter', 'model': 'fixture-model',
        'base_url': f'http://127.0.0.1:{server.server_port}/v1',
    }
    config['model_calls']['enabled'] = True
    config['memory']['semantic']['enabled'] = True
    config['http'] = {'persist': True, 'persist_path': str(state / 'sessions.json')}
    config_path.write_text(json.dumps(config))
    initialized = run('--connect', environment=configured_key_env)
    assert initialized.returncode == 0, initialized.stdout
    assert '主动连接结果: ✅' in initialized.stdout, initialized.stdout
    assert (state / 'model-calls.sqlite3').is_file(), 'opted-in model-call store was not opened'
    assert (state / 'semantic.sqlite3').is_file(), 'opted-in semantic store was not opened'
    assert RejectMcp.requests == 0, 'store initialization submitted an MCP, model, or embedding request'
    assert '会话存储: SQLite（配置已启用；doctor 不打开会话库）' in initialized.stdout, initialized.stdout
    assert not (state / 'sessions.sqlite3').exists(), 'doctor initialized the serve-only session store'

    config['mcp']['servers'] = [{
        'name': 'fixture', 'endpoint': f'http://127.0.0.1:{server.server_port}/mcp/',
        'timeout_secs': 1, 'max_response_bytes': 1024, 'max_concurrent_calls': 1,
        'bearer_token_env': 'JIACLAW_DOCTOR_MCP_TOKEN',
        'tools': [{
            'name': 'lookup', 'alias': 'lookup',
            'descriptor_sha256': 'sha256:' + '0' * 64, 'effect': 'read_only',
        }],
    }]
    config_path.write_text(json.dumps(config))
    try:
        empty_credential = run(environment=dict(configured_key_env, JIACLAW_DOCTOR_MCP_TOKEN=''))
        assert empty_credential.returncode != 0, empty_credential.stdout
        assert 'MCP 凭据: ❌' in empty_credential.stdout, empty_credential.stdout
        assert 'JIACLAW_DOCTOR_MCP_TOKEN' not in empty_credential.stdout, empty_credential.stdout
        assert 'fixture-bearer' not in empty_credential.stdout, empty_credential.stdout

        authenticated_env = dict(configured_key_env, JIACLAW_DOCTOR_MCP_TOKEN='fixture-bearer')
        offline = run(environment=authenticated_env)
        assert offline.returncode == 0, offline.stdout
        assert '远程工具: 未连接' in offline.stdout, offline.stdout
        assert RejectMcp.requests == 0, 'default doctor contacted the MCP server'
        assert 'HTTP MCP: 已接线（配置 1 个服务器；连通性见工具系统检查）' in offline.stdout, offline.stdout
        assert 'Brokerrouter 本地配置检查' in offline.stdout, offline.stdout
        assert 'Brokerrouter 连接测试' not in offline.stdout, offline.stdout

        active = run('--connect', environment=authenticated_env)
        assert active.returncode != 0, active.stdout
        assert '--connect 已启用' in active.stdout, active.stdout
        assert '主动连接结果: ❌' in active.stdout, active.stdout
        assert RejectMcp.requests > 0, '--connect did not contact the configured MCP server'
        assert set(RejectMcp.paths) == {'/mcp/'}, RejectMcp.paths
        assert all(b'tools/call' not in body for body in RejectMcp.bodies), RejectMcp.bodies
    finally:
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=2)

    print('PASS: truthful provider/MCP credential exits, invalid endpoint exit, explicit stub, offline read-only default, opted-in stores/MCP without model, embedding, or tool calls')
    print('PASS: truthful stub/endpoint/MCP/durable/session capability status without initializing the session store')
