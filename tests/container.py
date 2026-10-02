#!/usr/bin/env python3
"""Real image acceptance; isolated state volume, explicit stub, no Docker socket."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.error
import time
import urllib.request
import uuid

image = sys.argv[1]
name = 'jiaclaw-image-e2e-' + uuid.uuid4().hex
volume = name + '-data'
process = None

def docker(*args, check=True):
    return subprocess.run(['docker', *args], check=check, capture_output=True, text=True, timeout=30)
with tempfile.TemporaryDirectory(prefix='jiaclaw-image-') as directory:
    config = Path(directory) / 'config.json'
    config.write_text(json.dumps({'agent': {'name': name, 'description': 'Image acceptance', 'system_instructions': 'Test assistant', 'max_turns': 10, 'workspace_path': '/data/workspace'}, 'provider': {'provider_type': 'stub'}, 'http': {'bind': '0.0.0.0:8080', 'persist': True, 'persist_path': '/data/state/sessions.sqlite3'}}))
    config.chmod(0o644)
    token = uuid.uuid4().hex
    try:
        docker('volume', 'create', volume)
        docker('run', '--detach', '--name', name, '--init', '--read-only', '--cap-drop=ALL', '--security-opt=no-new-privileges', '--pids-limit=128', '--memory=512m', '--cpus=1', '--tmpfs', '/tmp:rw,noexec,nosuid,nodev,size=16m', '-p', '127.0.0.1::8080', '-e', 'JIACLAW_API_TOKEN=' + token, '--mount', 'type=volume,src=' + volume + ',dst=/data', '--mount', 'type=bind,src=' + str(config) + ',dst=/etc/jiaclaw/config.json,readonly', image, 'serve', '--config', '/etc/jiaclaw/config.json')
        inspect = json.loads(docker('inspect', name).stdout)[0]
        assert inspect['Config']['User'] == '10001:10001'
        assert all(m['Destination'] != '/var/run/docker.sock' for m in inspect['Mounts'])
        port = inspect['NetworkSettings']['Ports']['8080/tcp'][0]['HostPort']
        base = 'http://127.0.0.1:' + port
        def request(path, method='GET', data=None):
            req = urllib.request.Request(base + path, method=method, headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}, data=json.dumps(data).encode() if data is not None else None)
            with urllib.request.urlopen(req, timeout=5) as r: return json.load(r)
        deadline = time.monotonic() + 20
        while True:
            try:
                assert request('/health')['agent_name'] == name
                break
            except (ConnectionError, OSError):
                if time.monotonic() >= deadline: raise AssertionError(docker('logs', name).stdout)
                time.sleep(.1)
        session = request('/api/sessions', 'POST')['session_id']
        request('/api/chat', 'POST', {'session_id': session, 'messages': [{'role': 'user', 'content': 'hello'}]})
        docker('restart', name)
        # Docker may reassign an ephemeral host port when restarting a container.
        port = json.loads(docker('inspect', name).stdout)[0]['NetworkSettings']['Ports']['8080/tcp'][0]['HostPort']
        base = 'http://127.0.0.1:' + port
        deadline = time.monotonic() + 20
        while True:
            try:
                assert len(request('/api/sessions/' + session)['messages']) == 2
                break
            except (ConnectionError, OSError):
                if time.monotonic() >= deadline: raise
                time.sleep(.1)
        print('PASS: non-root image, read-only rootfs, HTTP conversation, named-volume recovery')
    finally:
        docker('rm', '--force', name, check=False)
        docker('volume', 'rm', volume, check=False)
