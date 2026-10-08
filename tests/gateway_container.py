#!/usr/bin/env python3
"""Linux Docker qualification: real gateway, two private backends, bounded disks.

Requires a local rootful Linux daemon, mkfs.ext4 and noninteractive sudo/losetup.
Each filesystem is formatted ONLY in a newly created regular temporary file;
loop devices are attached afterwards and checked against that exact file. Missing
kernel/filesystem support is a failed qualification, never a successful skip.
Uses the supplied image and explicit stub providers; no real model credentials.
"""
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import time
import urllib.error
import urllib.request
import uuid


image = sys.argv[1]
prefix = 'jiaclaw-gateway-e2e-' + uuid.uuid4().hex
root = Path(tempfile.mkdtemp(prefix=prefix + '-'))
containers = []
networks = []
volumes = []
loops = []
secrets = []
cleanup_errors = []
firewall_rules = []
temporary_accept_rules = []
guard_applied = False
guard_script = Path(__file__).resolve().parent.parent / "deploy/gateway/network_guard.py"
host_sentinel = None
sentinel_thread = None
backend_tokens = {tenant: uuid.uuid4().hex + uuid.uuid4().hex for tenant in ['alice', 'bob']}
secrets.extend(backend_tokens.values())
gateway_name = prefix + '-gateway'
backend_names = {tenant: prefix + '-' + tenant for tenant in backend_tokens}
net_names = {tenant: prefix + '_' + tenant + ('_private' if tenant != 'ingress' else '')
             for tenant in ['alice', 'bob', 'ingress']}
volume_names = {tenant: prefix + '-' + tenant for tenant in ['alice', 'bob', 'gateway']}
base = None


def sanitized(text):
    for value in secrets:
        text = text.replace(value, '<fixture-secret>')
    return text


def run(args, check=True, timeout=45):
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if check and result.returncode:
        # Arguments are not printed; future fixture changes must not leak tokens.
        raise AssertionError('command failed: ' + sanitized(result.stdout + result.stderr))
    return result


def docker(*args, check=True, timeout=45):
    return run(['docker', *args], check=check, timeout=timeout)


def privileged(*args, check=True):
    command = list(args) if os.geteuid() == 0 else ['sudo', '-n', *args]
    return run(command, check=check)


def inspect(name):
    return json.loads(docker('inspect', name).stdout)[0]


def loop_matches(device, backing):
    result = privileged('losetup', '--json', '--list', '--output', 'NAME,BACK-FILE', device)
    entries = json.loads(result.stdout)['loopdevices']
    return (len(entries) == 1 and entries[0]['name'] == device
            and Path(entries[0]['back-file']).resolve() == backing.resolve())


def bounded_volume(tenant):
    backing = root / (tenant + '.ext4')
    flags = os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW
    descriptor = os.open(backing, flags, 0o600)
    with os.fdopen(descriptor, 'wb') as output:
        output.truncate(64 * 1024 * 1024)
        output.flush()
        os.fsync(output.fileno())
    metadata = backing.lstat()
    assert stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
    assert backing.parent == root and metadata.st_size == 64 * 1024 * 1024
    # Formatting a fresh regular file precedes and is separate from loop setup.
    run(['mkfs.ext4', '-q', '-F', '-m', '0', '-E', 'root_owner=10001:10001', str(backing)])
    device = privileged('losetup', '--find', '--show', '--nooverlap', str(backing)).stdout.strip()
    assert re.fullmatch(r'/dev/loop[0-9]+', device), 'unexpected loop device name'
    loops.append((device, backing))
    assert loop_matches(device, backing), 'loop backing file mismatch'
    volume = volume_names[tenant]
    docker('volume', 'create', '--driver', 'local', '--opt', 'type=ext4',
           '--opt', 'device=' + device, '--opt', 'o=nodev,nosuid,noexec', volume)
    volumes.append(volume)
    info = json.loads(docker('volume', 'inspect', volume).stdout)[0]
    assert info['Driver'] == 'local' and info['Options']['device'] == device
    assert info['Options']['type'] == 'ext4'


def config_file(name, value):
    path = root / name
    path.write_text(json.dumps(value))
    path.chmod(0o444)
    return path


def container_args(name, volume, network):
    return ['--name', name, '--init', '--user', '10001:10001', '--read-only',
            '--cap-drop=ALL', '--security-opt=no-new-privileges', '--pids-limit=128',
            '--sysctl', 'net.ipv6.conf.all.disable_ipv6=1', '--sysctl', 'net.ipv6.conf.default.disable_ipv6=1',
            '--memory=512m', '--cpus=1', '--tmpfs', '/tmp:rw,noexec,nosuid,nodev,size=16m',
            '--log-driver=json-file', '--log-opt=max-size=10m', '--log-opt=max-file=3',
            '--network', network, '--mount', 'type=volume,src=' + volume + ',dst=/data,volume-nocopy']


def wait_backend(tenant):
    name = backend_names[tenant]
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        result = docker('exec', name, 'curl', '--fail', '--silent', '--max-time', '2',
                        'http://127.0.0.1:8080/health', check=False)
        if result.returncode == 0:
            assert json.loads(result.stdout)['agent_name'] == tenant
            return
        state = inspect(name)['State']
        assert state['Running'], sanitized(docker('logs', name).stdout)
        time.sleep(0.1)
    raise AssertionError('backend startup timed out: ' + sanitized(docker('logs', name).stdout))


def request(path, token=None, method='GET', data=None):
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(base + path, method=method, headers=headers,
                                 data=json.dumps(data).encode() if data is not None else None)
    try:
        response = urllib.request.urlopen(req, timeout=15)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        payload = response.read()
        try:
            body = json.loads(payload) if payload else None
        except ValueError:
            body = payload.decode()
        return response.status, body


def wait_gateway():
    global base
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        state = inspect(gateway_name)
        assert state['State']['Running'], sanitized(docker('logs', gateway_name).stdout)
        port = state['NetworkSettings']['Ports']['8080/tcp'][0]['HostPort']
        base = 'http://127.0.0.1:' + port
        try:
            if request('/health')[0] == 200:
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise AssertionError('gateway startup timed out: ' + sanitized(docker('logs', gateway_name).stdout))


def admin(command, *args):
    result = docker('exec', gateway_name, '/usr/local/bin/jiaclaw', 'gateway', command,
                    '--config', '/etc/jiaclaw/gateway.json', *args)
    value = json.loads(result.stdout)
    if 'token' in value:
        secrets.append(value['token'])
    return value


def audit_history(user_id, limit=100, include_notes=False):
    events = []
    cursor = '0'
    for _ in range(4097):
        args = ['--user', user_id, '--after-seq', cursor, '--limit', str(limit)]
        if include_notes:
            args.append('--include-notes')
        page = admin('audit-list', *args)
        assert page['user_id'] == user_id and page['notes_included'] is include_notes
        for field in ['next_after_seq', 'latest_seq']:
            assert re.fullmatch(r'0|[1-9][0-9]*', page[field]), field
        assert page['oldest_retained_seq'] is None or re.fullmatch(r'[1-9][0-9]*', page['oldest_retained_seq'])
        assert len(page['events']) <= limit and not page['retention_gap'], page
        for event in page['events']:
            assert event['user_id'] == user_id and re.fullmatch(r'[1-9][0-9]*', event['seq'])
            assert int(cursor) < int(event['seq']) <= int(page['latest_seq'])
            assert include_notes or 'note' not in event
        encoded = json.dumps(page)
        assert all(secret not in encoded for secret in secrets), 'credential exposed by audit query'
        assert 'verifier' not in encoded and 'token' not in encoded
        events.extend(page['events'])
        if not page['has_more']:
            assert page['next_after_seq'] == page['latest_seq']
            assert [int(event['seq']) for event in events] == sorted({int(event['seq']) for event in events})
            return events, page
        assert len(page['events']) == limit and page['next_after_seq'] == page['events'][-1]['seq']
        assert int(page['next_after_seq']) > int(cursor)
        cursor = page['next_after_seq']
    raise AssertionError('audit pagination did not terminate within retained history')


def chat(token, content, session='shared-id'):
    status, body = request('/api/chat', token, 'POST', {
        'session_id': session, 'messages': [{'role': 'user', 'content': content}],
        'enabled_tools': ['datetime_now'],
    })
    assert status == 200, (status, body)
    return body


def messages(token, session='shared-id'):
    status, body = request('/api/sessions/' + session, token)
    assert status == 200, (status, body)
    return body['messages']


def verify_runtime(name, expected_networks, expected_volume, published=False):
    data = inspect(name)
    host = data['HostConfig']
    assert data['Config']['User'] == '10001:10001'
    assert host['ReadonlyRootfs'] and not host['Privileged']
    assert host['CapDrop'] == ['ALL']
    assert any(option.startswith('no-new-privileges') for option in host['SecurityOpt'])
    assert host['Memory'] == 512 * 1024 * 1024 and host['NanoCpus'] == 1_000_000_000
    assert host['PidsLimit'] == 128 and host['PidMode'] != 'host' and host['NetworkMode'] != 'host'
    assert host['IpcMode'] != 'host'
    assert host['Sysctls']['net.ipv6.conf.all.disable_ipv6'] == '1'
    assert 'size=16m' in host['Tmpfs']['/tmp']
    assert host['LogConfig']['Type'] == 'json-file'
    assert host['LogConfig']['Config'] == {'max-size': '10m', 'max-file': '3'}
    assert set(data['NetworkSettings']['Networks']) == set(expected_networks)
    actual_volumes = [mount for mount in data['Mounts'] if mount['Type'] == 'volume']
    assert len(actual_volumes) == 1 and actual_volumes[0]['Name'] == expected_volume
    assert actual_volumes[0]['Destination'] == '/data'
    for mount in data['Mounts']:
        assert mount['Destination'] not in ['/var/run/docker.sock', '/run/docker.sock', '/proc', '/sys', '/dev']
        if mount['Type'] == 'bind':
            assert not mount['RW'], mount
    if published:
        assert host['PortBindings']['8080/tcp'][0]['HostIp'] == '127.0.0.1'
    else:
        assert not host['PortBindings'], host['PortBindings']
    namespace = docker('exec', name, 'readlink', '/proc/1/ns/pid').stdout.strip()
    assert namespace != privileged('readlink', '/proc/1/ns/pid').stdout.strip(), 'container shares host PID namespace'
    return namespace


try:
    assert sys.platform.startswith('linux'), 'bounded-volume qualification requires a native Linux runner'
    assert shutil.which('mkfs.ext4') and shutil.which('losetup'), 'e2fsprogs and util-linux are required'
    endpoint = os.environ.get('DOCKER_HOST')
    if not endpoint:
        endpoint = json.loads(docker('context', 'inspect', '--format', '{{json .Endpoints.docker.Host}}').stdout)
    assert endpoint.startswith('unix://'), 'fixture requires the local Linux Docker daemon'
    info = json.loads(docker('info', '--format', '{{json .}}').stdout)
    assert info['OSType'] == 'linux' and 'rootless' not in str(info.get('SecurityOptions', []))
    for tenant in ['alice', 'bob', 'gateway']:
        bounded_volume(tenant)
    assert len(set(volume_names.values())) == 3 and len({device for device, _ in loops}) == 3
    for tenant in ['alice', 'bob', 'ingress']:
        args = ['network', 'create', '--label', 'com.docker.compose.project=' + prefix]
        if tenant != 'ingress':
            args.append('--internal')
        docker(*args, net_names[tenant])
        networks.append(net_names[tenant])
    # Internal bridge routing alone does not isolate host listeners. Restrict only
    # the freshly created tenant bridges; never change the host's default policy.
    assert shutil.which('iptables'), 'iptables is required for host-service isolation'
    for tenant in ['alice', 'bob']:
        network = json.loads(docker('network', 'inspect', net_names[tenant]).stdout)[0]
        assert network['Driver'] == 'bridge' and network['Internal'] and not network['EnableIPv6']
        bridge = network.get('Options', {}).get('com.docker.network.bridge.name') or 'br-' + network['Id'][:12]
        rule = ['-i', bridge, '-m', 'comment', '--comment', 'jiaclaw-gateway:' + prefix + ':' + tenant, '-j', 'REJECT']
        firewall_rules.append(rule)
    run([sys.executable, str(guard_script), 'apply', '--project', prefix])
    guard_applied = True
    run([sys.executable, str(guard_script), 'apply', '--project', prefix])  # idempotent
    for rule in firewall_rules:
        privileged('iptables', '-w', '5', '-C', 'INPUT', *rule)
    rules = privileged('iptables', '-w', '5', '-S', 'INPUT').stdout.splitlines()
    assert sum('jiaclaw-gateway:' + prefix + ':' in rule for rule in rules) == 2
    # Existing deny rules must not hide behind a subsequently inserted ACCEPT.
    # This exact task-owned fault is injected before any application starts.
    accept = [*firewall_rules[0][:-1], 'ACCEPT']
    accept[accept.index('--comment') + 1] += ':precedence-fixture'
    privileged('iptables', '-w', '5', '-I', 'INPUT', *accept)
    temporary_accept_rules.append(accept)
    rejected = run([sys.executable, str(guard_script), 'apply', '--project', prefix], check=False)
    assert rejected.returncode != 0 and 'precedes the project guard' in rejected.stderr, rejected.stderr
    privileged('iptables', '-w', '5', '-D', 'INPUT', *accept)
    temporary_accept_rules.remove(accept)
    run([sys.executable, str(guard_script), 'apply', '--project', prefix])

    class Sentinel(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Length', '2')
            self.end_headers()
            self.wfile.write(b'ok')

    host_sentinel = ThreadingHTTPServer(('0.0.0.0', 0), Sentinel)
    host_sentinel.daemon_threads = True
    sentinel_thread = threading.Thread(target=host_sentinel.serve_forever, daemon=True)
    sentinel_thread.start()
    with urllib.request.urlopen('http://127.0.0.1:' + str(host_sentinel.server_port), timeout=5) as response:
        assert response.status == 200
    for tenant in backend_tokens:
        config = config_file(tenant + '.json', {
            'agent': {'name': tenant, 'description': 'Isolated image fixture',
                      'system_instructions': 'Test assistant', 'max_turns': 10, 'workspace_path': '/data/workspace'},
            'provider': {'provider_type': 'stub'},
            'http': {'bind': '0.0.0.0:8080', 'api_token': backend_tokens[tenant], 'persist': True,
                     'persist_path': '/data/state/sessions.sqlite3', 'metrics_public': False, 'channels': []},
            'tools': {'exec': {'enabled': False}, 'web_search': {'enabled': False}, 'web_fetch': {'enabled': False}},
            'mcp': {'servers': []}, 'scheduler': {'enabled': True, 'gateway_driven': True}, 'heartbeat': {'enabled': False},
        })
        name = backend_names[tenant]
        args = container_args(name, volume_names[tenant], net_names[tenant])
        args += ['--network-alias', tenant + '-backend', '--mount',
                 'type=bind,src=' + str(config) + ',dst=/etc/jiaclaw/config.json,readonly']
        docker('create', *args, image, 'serve', '--config', '/etc/jiaclaw/config.json')
        containers.append(name)
        docker('start', name)
        wait_backend(tenant)
    gateway_config = config_file('gateway.json', {
        'bind': '0.0.0.0:8080', 'registry_path': '/data/gateway/registry.sqlite3',
        'backends': [{'id': tenant, 'url': 'http://' + tenant + '-backend:8080',
                      'token_file': '/run/secrets/' + tenant + '.token'} for tenant in backend_tokens],
        'request_timeout_seconds': 150, 'max_in_flight': 16, 'scheduled_jobs': True,
        'allow_private_http': True, 'allow_remote_bind': True,
    })
    args = container_args(gateway_name, volume_names['gateway'], net_names['ingress'])
    args += ['-p', '127.0.0.1::8080', '--mount',
             'type=bind,src=' + str(gateway_config) + ',dst=/etc/jiaclaw/gateway.json,readonly']
    for tenant, token in backend_tokens.items():
        path = root / (tenant + '.token')
        path.write_text(token + '\n')
        path.chmod(0o444)
        args += ['--mount', 'type=bind,src=' + str(path) + ',dst=/run/secrets/' + tenant + '.token,readonly']
    docker('create', *args, image, 'gateway', 'serve', '--config', '/etc/jiaclaw/gateway.json')
    containers.append(gateway_name)
    for tenant in backend_tokens:
        docker('network', 'connect', net_names[tenant], gateway_name)
    docker('start', gateway_name)
    wait_gateway()

    namespaces = [verify_runtime(backend_names[tenant], [net_names[tenant]], volume_names[tenant])
                  for tenant in backend_tokens]
    namespaces.append(verify_runtime(gateway_name, list(net_names.values()), volume_names['gateway'], published=True))
    assert len(set(namespaces)) == 3
    for tenant in ['alice', 'bob']:
        network = json.loads(docker('network', 'inspect', net_names[tenant]).stdout)[0]
        assert network['Internal'] is True
    bob_ip = inspect(backend_names['bob'])['NetworkSettings']['Networks'][net_names['bob']]['IPAddress']
    blocked = docker('exec', backend_names['alice'], 'curl', '--noproxy', '*', '--silent', '--max-time', '2',
                     'http://' + bob_ip + ':8080/health', check=False)
    assert blocked.returncode != 0, 'Alice reached Bob across private network boundaries'
    for tenant in backend_tokens:
        network = json.loads(docker('network', 'inspect', net_names[tenant]).stdout)[0]
        host_ip = network['IPAM']['Config'][0]['Gateway']
        blocked = docker('exec', backend_names[tenant], 'curl', '--noproxy', '*', '--silent', '--max-time', '2',
                         'http://' + host_ip + ':' + str(host_sentinel.server_port), check=False)
        assert blocked.returncode != 0, tenant + ' reached the host HTTP sentinel'
        blocked = docker('exec', backend_names[tenant], 'curl', '--noproxy', '*', '--silent', '--max-time', '2',
                         'http://169.254.169.254/', check=False)
        assert blocked.returncode != 0, tenant + ' reached link-local metadata'
        other = 'bob' if tenant == 'alice' else 'alice'
        assert docker('exec', backend_names[tenant], 'test', '-e', '/run/secrets/' + other + '.token',
                      check=False).returncode == 1
    assert request('/api/sessions')[0] == 401
    assert request('/api/sessions', backend_tokens['alice'])[0] == 401
    issued = {tenant: admin('user-add', '--backend', tenant) for tenant in backend_tokens}
    assert len({value['token'] for value in issued.values()}) == 2
    assert len({value['user_id'] for value in issued.values()}) == 2
    assert len({value['key_id'] for value in issued.values()}) == 2
    a, b = issued['alice']['token'], issued['bob']['token']
    chat(a, 'Only Alice should see this unique sentence.')
    chat(b, 'Only Bob should see this different sentence.')
    a_history, b_history = messages(a), messages(b)
    assert len(a_history) == len(b_history) == 2
    assert 'Alice' in json.dumps(a_history) and 'Bob' not in json.dumps(a_history)
    assert 'Bob' in json.dumps(b_history) and 'Alice' not in json.dumps(b_history)
    # The same hardened tenant volumes also persist gateway-admitted cron.
    # No provider call: each backend explicitly uses the deterministic stub.
    assert request('/api/gateway/capabilities', a) == (200, {'scheduled_jobs': True, 'read_only': False})
    status, job = request('/api/jobs', a, 'POST', {
        'name': 'container clock', 'prompt': 'Report current time.',
        'schedule': {'kind': 'interval', 'seconds': 10},
        'enabled_tools': ['datetime_now'], 'timeout_secs': 10,
    })
    assert status == 201, (status, job)
    job_path = '/api/jobs/' + job['id']
    assert request(job_path, b)[0] == 404
    assert request('/internal/scheduler/status', a)[0] == 404
    deadline = time.monotonic() + 25
    while True:
        status, page = request(job_path + '/runs?limit=5', a)
        assert status == 200, (status, page)
        finished = [entry for entry in page['items'] if entry['status'] == 'completed']
        if finished:
            break
        assert time.monotonic() < deadline, page
        time.sleep(0.15)
    assert all(entry['job_id'] == job['id'] for entry in page['items'])
    deadline = time.monotonic() + 5
    while True:
        status, paused_job = request(job_path + '/pause', a, 'POST', {})
        if status != 429:
            break
        assert time.monotonic() < deadline
        time.sleep(0.1)
    assert status == 200 and not paused_job['enabled'], (status, paused_job)
    status, settled_runs = request(job_path + '/runs?limit=5', a)
    assert status == 200 and settled_runs['items']
    settled_ids = {entry['id'] for entry in settled_runs['items']}
    viewer = admin('key-add', '--user', issued['alice']['user_id'], '--read-only')
    assert viewer['read_only'] is True
    view_key = viewer['token']
    assert request('/api/gateway/capabilities', view_key) == (200, {'scheduled_jobs': True, 'read_only': True})
    assert messages(view_key) == a_history
    assert request(job_path, view_key)[1] == paused_job
    for denied_path, denied_method in [('/api/chat', 'POST'), ('/api/sessions', 'POST'),
                                      ('/api/sessions/same', 'DELETE'), ('/api/sessions/import', 'POST'),
                                      ('/api/jobs', 'POST'), (job_path, 'DELETE'),
                                      (job_path + '/pause', 'POST'), (job_path + '/resume', 'POST')]:
        assert request(denied_path, view_key, denied_method, {})[0] == 403, denied_path
    assert messages(view_key) == a_history
    assert not request(job_path, view_key)[1]['enabled']
    assert next(user for user in admin('user-list')['users']
                if user['user_id'] == viewer['user_id'])['hold'] is None
    rotated_viewer = admin('key-rotate', '--key', viewer['key_id'])
    assert rotated_viewer['read_only'] is True
    assert request('/api/sessions', view_key)[0] == 401
    assert messages(rotated_viewer['token']) == a_history
    assert request('/api/chat', rotated_viewer['token'], 'POST', {})[0] == 403
    admin('key-revoke', '--key', rotated_viewer['key_id'])
    assert request('/api/sessions', rotated_viewer['token'])[0] == 401
    audit_events, _ = audit_history(issued['alice']['user_id'], limit=2)
    key_events = [event for event in audit_events if event['key_id'] in [viewer['key_id'], rotated_viewer['key_id']]]
    assert [event['action'] for event in key_events] == ['key_added', 'key_revoked_by_rotation', 'key_rotated', 'key_revoked']
    assert all(event['key_id'] != issued['bob']['key_id'] for event in audit_events)
    for path in ['/api/gateway/audit', '/api/audit']:
        assert request(path, a)[0] == 404, 'administrator history exposed as a tenant route'
    print('PASS: real quota-volume audit query pages canonical metadata, isolates users, omits notes and credentials, and exposes no tenant HTTP route')
    print('PASS: real container read-only key reads its tenant, rejects all exposed mutations without hold, and preserves permission across live rotation/revocation')
    docker('restart', backend_names['alice'])
    wait_backend('alice')
    status, restored = request(job_path + '/runs?limit=5', a)
    assert status == 200 and {entry['id'] for entry in restored['items']} == settled_ids
    assert all(not row['id'].startswith('job:') for row in request('/api/sessions', a)[1]['sessions'])
    assert request(job_path, a, 'DELETE')[0] == 204
    print('PASS: gateway-driven cron executes and persists on the tenant quota volume; jobs/results stay isolated across tenants and backend restart')
    marker = uuid.uuid4().hex + '.txt'
    docker('exec', backend_names['alice'], '/bin/sh', '-c', 'printf isolated > /data/workspace/' + marker)
    assert docker('exec', backend_names['bob'], 'test', '-e', '/data/workspace/' + marker, check=False).returncode == 1
    # Token rotation is effective on the live process; no gateway restart required.
    replacement = admin('key-rotate', '--key', issued['alice']['key_id'])
    assert replacement['user_id'] == issued['alice']['user_id'] and replacement['key_id'] != issued['alice']['key_id']
    assert replacement['token'] != a and request('/api/sessions', a)[0] == 401
    a = replacement['token']
    assert messages(a) == a_history and messages(b) == b_history
    # Backend and gateway restarts retain data and live revocation state.
    docker('restart', backend_names['alice'])
    wait_backend('alice')
    assert messages(a) == a_history and messages(b) == b_history
    docker('restart', gateway_name)
    wait_gateway()
    assert request('/api/sessions', issued['alice']['token'])[0] == 401
    assert messages(a) == a_history and messages(b) == b_history
    # Schema 4 bindings persist on the real bounded gateway volume; runtime stays
    # explicitly off. Platform ingress/provisioning is qualified separately.
    slack_binding = admin('slack-bind', '--user', issued['alice']['user_id'],
                          '--team-id', 'TCONTAINER', '--app-id', 'ACONTAINER',
                          '--bot-user-id', 'WBOTCONTAINER', '--bot-id', 'BCONTAINER',
                          '--sender-id', 'UHUMANCONTAINER', '--conversation-id', 'DCONTAINER')
    assert slack_binding['backend_id'] == 'alice' and slack_binding['enabled']
    assert admin('slack-bindings')['bindings'] == [slack_binding]
    assert request('/hooks/slack/' + slack_binding['id'], method='POST', data={})[0] == 404
    for path in ['/internal/channels/slack/status', '/internal/channels/slack-binding',
                 '/internal/channels/slack/execute', '/api/gateway/slack-bindings']:
        assert request(path, a)[0] == 404, path
    admin('slack-revoke', '--binding', slack_binding['id'])
    docker('restart', gateway_name)
    wait_gateway()
    restored_binding = admin('slack-bindings')['bindings'][0]
    assert restored_binding == dict(slack_binding, enabled=False)
    rejected_binding = docker('exec', gateway_name, '/usr/local/bin/jiaclaw', 'gateway', 'slack-bind',
                              '--config', '/etc/jiaclaw/gateway.json', '--user', issued['bob']['user_id'],
                              '--team-id', 'TCONTAINER', '--app-id', 'ACONTAINER',
                              '--bot-user-id', 'WBOTCONTAINER', '--bot-id', 'BCONTAINER',
                              '--sender-id', 'UBOBCONTAINER', '--conversation-id', 'DBOBCONTAINER', check=False)
    assert rejected_binding.returncode != 0, 'revoked installation was reassigned'
    assert admin('slack-bindings')['bindings'] == [restored_binding]
    assert messages(a) == a_history and messages(b) == b_history
    print('PASS: schema 4 Slack immutable reservation and revocation survive real bounded-volume restart; default-off ingress and private routes stay unavailable')
    # Persisted admission is observed through the real admin API before SIGKILL.
    # Pausing only this fixture backend holds the write without vendor calls.
    docker('pause', backend_names['alice'])
    interrupted_result = []

    def interrupted_chat():
        try:
            interrupted_result.append(request('/api/chat', a, 'POST', {
                'session_id': 'interrupted-session',
                'messages': [{'role': 'user', 'content': 'fixture interrupted write'}],
                'enabled_tools': ['datetime_now'],
            }))
        except (OSError, urllib.error.URLError) as error:
            interrupted_result.append(type(error).__name__)

    interrupted_thread = threading.Thread(target=interrupted_chat, daemon=True)
    interrupted_thread.start()
    deadline = time.monotonic() + 10
    while True:
        user = next(user for user in admin('user-list')['users'] if user['user_id'] == issued['alice']['user_id'])
        if user.get('hold') and user['hold']['state'] == 'in_flight':
            break
        assert time.monotonic() < deadline, 'write admission was not persisted'
        assert not interrupted_result, interrupted_result
        time.sleep(0.05)
    docker('kill', '--signal=KILL', gateway_name)
    docker('unpause', backend_names['alice'])
    # Terminate the old backend process before claiming that it is idle. No
    # active background jobs or real external services remain in this fixture.
    docker('stop', '--time=10', backend_names['alice'])
    docker('start', backend_names['alice'])
    wait_backend('alice')
    interrupted_thread.join(timeout=20)
    assert not interrupted_thread.is_alive()
    docker('start', gateway_name)
    wait_gateway()
    user = next(user for user in admin('user-list')['users'] if user['user_id'] == issued['alice']['user_id'])
    assert user['hold']['state'] == 'needs_review', user
    status, body = request('/api/chat', a, 'POST', {'session_id': 'blocked-after-restart',
                          'messages': [{'role': 'user', 'content': 'must not execute'}]})
    assert status == 409, (status, body)
    chat(b, 'Bob remains available while Alice requires review.', session='during-hold')
    assert messages(a) == a_history
    review_note = 'Fixture backend was stopped and restarted; external work is disabled and reviewed. "核对"'
    # A retained channel binding requires the gateway process lock even when
    # revoked and never enabled. Live maintenance must fail without clearing
    # the recovered HTTP hold; use the documented stopped-volume workflow.
    before_review, _ = audit_history(issued['alice']['user_id'])
    denied = docker('exec', gateway_name, '/usr/local/bin/jiaclaw', 'gateway', 'review-clear',
                    '--config', '/etc/jiaclaw/gateway.json', '--user', issued['alice']['user_id'],
                    '--confirm-backend-idle', '--note', review_note, check=False)
    assert denied.returncode != 0 and 'stop gateway before reviewing channel queues' in denied.stderr
    assert audit_history(issued['alice']['user_id'])[0] == before_review
    assert next(user for user in admin('user-list')['users']
                if user['user_id'] == issued['alice']['user_id'])['hold'] == user['hold']
    docker('stop', '--time=155', gateway_name, timeout=165)
    maintenance_name = prefix + '-maintenance'
    containers.append(maintenance_name)
    maintenance_args = container_args(maintenance_name, volume_names['gateway'], net_names['ingress'])
    maintenance_args += ['--mount', 'type=bind,src=' + str(gateway_config)
                         + ',dst=/etc/jiaclaw/gateway.json,readonly']
    cleared = docker('run', *maintenance_args, image, 'gateway', 'review-clear',
                     '--config', '/etc/jiaclaw/gateway.json', '--user', issued['alice']['user_id'],
                     '--confirm-backend-idle', '--note', review_note)
    assert json.loads(cleared.stdout)['review_cleared'] is True
    docker('rm', maintenance_name)
    containers.remove(maintenance_name)
    docker('start', gateway_name)
    wait_gateway()
    recovery_events, _ = audit_history(issued['alice']['user_id'])
    recovered = next(event for event in reversed(recovery_events) if event['action'] == 'write_recovered_for_review')
    lifecycle = [event for event in recovery_events if event['request_id'] == recovered['request_id']]
    assert [event['action'] for event in lifecycle] == ['write_admitted', 'write_recovered_for_review', 'write_review_cleared']
    private_events, _ = audit_history(issued['alice']['user_id'], include_notes=True)
    assert next(event for event in reversed(private_events) if event['action'] == 'write_review_cleared')['note'] == review_note
    _, bob_tail = audit_history(issued['bob']['user_id'])
    chat(a, 'Explicit new work after review.', session='after-review')
    empty_bob = admin('audit-list', '--user', issued['bob']['user_id'], '--after-seq', bob_tail['next_after_seq'])
    assert empty_bob['events'] == [] and not empty_bob['has_more']
    assert int(empty_bob['next_after_seq']) > int(bob_tail['next_after_seq'])
    assert empty_bob['next_after_seq'] == empty_bob['latest_seq']
    print('PASS: persisted SIGKILL review lifecycle is correlated by request ID, notes require explicit inclusion, and empty filtered pages advance past other users')
    print('PASS: hardened containers, separate mounts/PID/private networks, per-user same-ID session isolation, live key rotation and restart persistence and stale-write review hold')

    # Only Alice's fresh 64 MiB ext4 filesystem is filled, never the host filesystem.
    filled = docker('exec', backend_names['alice'], 'dd', 'if=/dev/zero', 'of=/data/fixture-fill.bin',
                    'bs=4096', 'conv=fsync', check=False, timeout=30)
    assert filled.returncode != 0 and 'No space left on device' in filled.stderr, sanitized(filled.stderr)
    capacity, available = map(int, docker('exec', backend_names['alice'], 'df', '-B1',
                                        '--output=size,avail', '/data').stdout.splitlines()[-1].split())
    written = int(docker('exec', backend_names['alice'], 'stat', '-c', '%s', '/data/fixture-fill.bin').stdout)
    # ext4 may retain internally reserved or fragmented free blocks at ENOSPC.
    # df's available counter is not proof that this UID can allocate those blocks.
    # Qualify the real failed write and the actual hard filesystem capacity instead.
    assert 0 < capacity <= 64 * 1024 * 1024, capacity
    assert 32 * 1024 * 1024 <= written <= 64 * 1024 * 1024, written
    print(f'PASS: real ENOSPC after {written} bytes on bounded {capacity}-byte filesystem (df reports {available} available bytes)')
    assert docker('exec', backend_names['bob'], 'test', '-e', '/data/fixture-fill.bin', check=False).returncode == 1
    chat(b, 'Bob continues while Alice is out of disk.')
    assert len(messages(b)) == 4
    # The separately bounded registry also stays writable while Alice is full.
    extra = admin('key-add', '--user', issued['bob']['user_id'])
    assert request('/api/sessions', extra['token'])[0] == 200
    admin('key-revoke', '--key', extra['key_id'])
    assert request('/api/sessions', extra['token'])[0] == 401
    docker('exec', backend_names['alice'], 'rm', '/data/fixture-fill.bin')
    assert messages(a) == a_history
    admin('user-disable', '--user', issued['bob']['user_id'])
    assert request('/api/sessions', b)[0] == 401 and messages(a) == a_history
    disabled_events, disabled_tail = audit_history(issued['bob']['user_id'])
    assert disabled_events[-1]['action'] == 'user_disabled'
    docker('restart', gateway_name)
    wait_gateway()
    assert request('/api/sessions', b)[0] == 401 and messages(a) == a_history
    restarted_events, restarted_tail = audit_history(issued['bob']['user_id'])
    assert restarted_events == disabled_events and restarted_tail['latest_seq'] == disabled_tail['latest_seq']
    for name in containers:
        logs = docker('logs', name).stdout + docker('logs', name).stderr
        assert all(secret not in logs for secret in secrets), 'a credential appeared in container logs'
    print('PASS: real per-tenant ENOSPC leaves other tenant and registry available, live revoke/disable persist, credentials absent from logs')
except Exception:
    for name in containers:
        result = docker('logs', '--tail=60', name, check=False)
        print(name + ': ' + sanitized(result.stdout + result.stderr), file=sys.stderr)
    raise
finally:
    # Keep sparse files if cleanup fails: do not unlink a still-attached filesystem.
    for name in reversed(containers):
        result = docker('rm', '--force', name, check=False)
        if result.returncode:
            cleanup_errors.append('container ' + name)
    for volume in reversed(volumes):
        result = docker('volume', 'rm', volume, check=False)
        if result.returncode:
            cleanup_errors.append('volume ' + volume)
    for device, backing in reversed(loops):
        try:
            assert loop_matches(device, backing), 'loop backing changed; refusing detach'
            result = privileged('losetup', '--detach', device, check=False)
            if result.returncode:
                cleanup_errors.append('loop ' + device)
        except Exception as error:
            cleanup_errors.append('loop verification ' + device + ': ' + str(error))
    if host_sentinel is not None:
        host_sentinel.shutdown()
        host_sentinel.server_close()
        sentinel_thread.join(timeout=5)
    for rule in reversed(temporary_accept_rules):
        result = privileged('iptables', '-w', '5', '-D', 'INPUT', *rule, check=False)
        if result.returncode:
            cleanup_errors.append('fixture precedence ACCEPT rule')
    if guard_applied:
        result = run([sys.executable, str(guard_script), 'remove', '--project', prefix], check=False)
        if result.returncode:
            cleanup_errors.append('deployment INPUT firewall guard removal')
        else:
            result = run([sys.executable, str(guard_script), 'remove', '--project', prefix], check=False)
            if result.returncode:
                cleanup_errors.append('idempotent firewall guard removal')
            for rule in firewall_rules:
                if privileged('iptables', '-w', '5', '-C', 'INPUT', *rule, check=False).returncode != 1:
                    cleanup_errors.append('fixture INPUT firewall rule remained')
    for name in reversed(networks):
        result = docker('network', 'rm', name, check=False)
        if result.returncode:
            cleanup_errors.append('network ' + name)
    if cleanup_errors:
        raise AssertionError('manual fixture cleanup required; files retained at ' + str(root) + ': ' + ', '.join(cleanup_errors))
    shutil.rmtree(root)
