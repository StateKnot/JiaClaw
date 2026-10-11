#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Real local CLI source-lock creation; no supplier or installer certification."""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import concurrent.futures
import copy
import re
import shutil
import time
import urllib.request
import uuid
from host_log import read_running_log

binary = pathlib.Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='jiaclaw-lock-create-') as tmp:
    root = pathlib.Path(tmp)
    workspace = root / 'workspace'
    workspace.mkdir()
    raw = '---\nname: reviewed\ndescription: local reviewed skill\n---\nreviewed body\n'
    target = workspace / 'skills' / 'reviewed'
    target.mkdir(parents=True)
    (target / 'SKILL.md').write_text(raw)
    declaration = {'directory': 'reviewed', 'enabled': False, 'source': {
        'repository': 'https://github.com/example/reviewed', 'revision': 'a' * 40,
        'skill_sha256': hashlib.sha256(raw.encode()).hexdigest()}}
    manifest = json.dumps({'version': 2, 'skills': [declaration]}) + '\n'
    source = workspace / 'reviewed-policy.json'
    source.write_text(manifest)
    config = root / 'config.json'
    # The CLI must not initialize unrelated configured model/state features.
    settings = {'agent': {'name': 'lock-create', 'description': 'Disposable', 'max_turns': 3,
                          'system_instructions': '', 'workspace_path': str(workspace),
                          'skill_lock_required': True}, 'provider': {'provider_type': 'stub'}}
    config.write_text(json.dumps(settings))
    expected = hashlib.sha256(manifest.encode()).hexdigest()
    env = {k: v for k, v in os.environ.items() if k != 'JIACLAW_API_KEY'}

    def run(extra=(), expected_hash=expected):
        return subprocess.run([str(binary), 'skills', 'create-lock', '--from', 'reviewed-policy.json',
                               '--if-source-sha256', expected_hash, '--config', str(config), *extra],
                              capture_output=True, text=True, env=env, timeout=10)

    first = run()
    assert first.returncode == 0, (first.returncode, first.stdout, first.stderr)
    result = json.loads(first.stdout)
    lock = workspace / 'skills.lock.json'
    assert lock.read_bytes() == manifest.encode()
    assert result['runtime_applied'] is False and result['disk_policy']['manifest_sha256'] == expected
    assert result['disk_policy']['skills'][0]['enabled'] is False
    old_inode = lock.stat().st_ino
    second = run()
    assert second.returncode != 0 and lock.read_bytes() == manifest.encode() and lock.stat().st_ino == old_inode
    print('PASS lock create 1: exact reviewed bytes, disabled policy, duplicate rejected without replacement')

    def reject_absent(result):
        assert result.returncode != 0 and not result.stdout.strip(), (result.stdout, result.stderr)
        assert not lock.exists() and not lock.is_symlink()
        assert not list(workspace.glob('.jiaclaw-memory-*'))

    lock.unlink()
    reject_absent(run(expected_hash='0' * 64))
    for altered in ({'version': 1, 'skills': [declaration]},
                    {'version': 2, 'skills': [{**declaration, 'enabled': True}]},
                    {'version': 2, 'skills': [{k: v for k, v in declaration.items() if k != 'enabled'}]},
                    {'version': 2, 'skills': [declaration, declaration]},
                    {'version': 2, 'skills': [declaration], 'extra': True}):
        data = json.dumps(altered)
        source.write_text(data)
        reject_absent(run(expected_hash=hashlib.sha256(data.encode()).hexdigest()))
    source.write_text(manifest)
    settings['agent']['skill_lock_required'] = False
    config.write_text(json.dumps(settings))
    reject_absent(run())
    settings['agent']['skill_lock_required'] = True
    settings['provider'] = {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:1', 'model': 'trap'}
    settings['model_calls'] = {'enabled': True, 'store_path': '../state/calls/index.sqlite3'}
    settings['memory'] = {'semantic': {'enabled': True, 'model': 'trap', 'space_revision': 'test',
                                     'dimensions': 8, 'index_path': '../state/index.sqlite3'}}
    settings['mcp'] = {'servers': [{'name': 'trap', 'endpoint': 'http://127.0.0.1:1/mcp/', 'timeout_secs': 1,
                                   'bearer_token_env': 'JIACLAW_LOCK_CREATE_UNUSED_KEY',
                                   'tools': [{'name': 'read', 'alias': 'read', 'descriptor_sha256': 'sha256:' + '0' * 64,
                                              'effect': 'read_only'}]}]}
    config.write_text(json.dumps(settings))
    assert run().returncode == 0
    assert not (root / 'state').exists()
    lock.unlink()
    print('PASS lock create 2: reviewed hash, explicit disabled schema and opt-in required; configured model/MCP/semantic traps never initialize')

    saved_body = (target / 'SKILL.md').read_bytes()
    for path in (source, target / 'SKILL.md'):
        saved = path.read_bytes()
        path.unlink()
        for kind in ('symlink', 'hardlink', 'fifo'):
            other = root / 'other'
            other.write_bytes(saved)
            if kind == 'symlink':
                path.symlink_to(other)
            elif kind == 'hardlink':
                os.link(other, path)
            else:
                os.mkfifo(path)
            reject_absent(run())
            path.unlink()
            other.unlink()
        path.write_bytes(saved)
    for raw_input in ('../reviewed-policy.json', str(source)):
        result = subprocess.run([str(binary), 'skills', 'create-lock', '--from', raw_input,
                                 '--if-source-sha256', expected, '--config', str(config)], env=env,
                                capture_output=True, text=True, timeout=10)
        reject_absent(result)
    for kind in ('symlink', 'hardlink', 'fifo', 'directory'):
        other = root / 'other'
        other.write_text('retained')
        if kind == 'symlink': lock.symlink_to(other)
        elif kind == 'hardlink': os.link(other, lock)
        elif kind == 'fifo': os.mkfifo(lock)
        else: lock.mkdir()
        result = run()
        assert result.returncode != 0 and other.read_text() == 'retained'
        if kind == 'directory': lock.rmdir()
        else: lock.unlink()
        other.unlink()
    rogue = workspace / 'skills' / 'rogue'
    rogue.mkdir()
    (rogue / 'SKILL.md').write_bytes(saved_body)
    reject_absent(run())
    shutil.rmtree(rogue)
    (target / 'SKILL.md').write_bytes(saved_body + b'drift')
    reject_absent(run())
    (target / 'SKILL.md').write_bytes(saved_body)
    print('PASS lock create 3: linked/special source, body and target, path escapes, unregistered and drifted bodies reject before publication')

    source.write_bytes(b' ' * (128 * 1024 + 1))
    reject_absent(run(expected_hash=hashlib.sha256(source.read_bytes()).hexdigest()))
    source.write_text(manifest)
    (target / 'SKILL.md').write_bytes(saved_body + b'x' * (128 * 1024))
    altered = copy.deepcopy(declaration)
    altered['source']['skill_sha256'] = hashlib.sha256((target / 'SKILL.md').read_bytes()).hexdigest()
    data = json.dumps({'version': 2, 'skills': [altered]})
    source.write_text(data)
    reject_absent(run(expected_hash=hashlib.sha256(data.encode()).hexdigest()))
    (target / 'SKILL.md').write_bytes(saved_body)
    entries = []
    big_body = saved_body + b'x' * (128 * 1024 - len(saved_body))
    for i in range(17):
        directory = 'limit' + str(i)
        folder = workspace / 'skills' / directory
        folder.mkdir()
        (folder / 'SKILL.md').write_bytes(big_body)
        item = copy.deepcopy(declaration)
        item['directory'] = directory
        item['source']['skill_sha256'] = hashlib.sha256(big_body).hexdigest()
        entries.append(item)
    data = json.dumps({'version': 2, 'skills': entries + [declaration]})
    source.write_text(data)
    reject_absent(run(expected_hash=hashlib.sha256(data.encode()).hexdigest()))
    for item in entries: shutil.rmtree(workspace / 'skills' / item['directory'])
    for i in range(256): (workspace / 'skills' / ('entry' + str(i))).write_text('')
    source.write_text(manifest)
    reject_absent(run())
    for path in (workspace / 'skills').glob('entry*'): path.unlink()
    print('PASS lock create 4: raw input/body limits, aggregate two-MiB capture and complete 256-entry directory scan remain bounded')

    if os.name == 'posix':
        import fcntl
        fd = os.open(workspace, os.O_RDONLY)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            reject_absent(run())
        finally:
            os.close(fd)
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        outcomes = list(pool.map(lambda _: run(), range(2)))
    assert sum(outcome.returncode == 0 for outcome in outcomes) == 1
    assert lock.read_bytes() == manifest.encode()
    lock.unlink()
    print('PASS lock create 5: actual held original workspace writer refuses; concurrent identical requests publish exactly once')

    if os.name == 'posix' and os.geteuid() != 0:
        import resource
        import signal
        def no_file_writes():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
        result = subprocess.run([str(binary), 'skills', 'create-lock', '--from', 'reviewed-policy.json',
                                 '--if-source-sha256', expected, '--config', str(config)], env=env,
                                capture_output=True, text=True, timeout=10, preexec_fn=no_file_writes)
        reject_absent(result)
        workspace.chmod(0o500)
        try: reject_absent(run())
        finally: workspace.chmod(0o700)
    assert run().returncode == 0 and lock.stat().st_mode & 0o777 == 0o600
    print('PASS lock create 6: real OS file-size and permissions refuse without staging remnants; successful publication is private')

    settings = {'agent': {'name': 'lock-create', 'description': 'Disposable', 'max_turns': 3,
                          'system_instructions': '', 'workspace_path': str(workspace), 'skill_lock_required': True},
                'provider': {'provider_type': 'stub'},
                'http': {'bind': '127.0.0.1:0', 'api_token': uuid.uuid4().hex}}
    config.write_text(json.dumps(settings))
    warm = copy.deepcopy(declaration)
    warm['enabled'] = True
    lock.write_text(json.dumps({'version': 2, 'skills': [warm]}))
    process = None
    log = root / 'serve.log'
    try:
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config)], env=env, stdout=output, stderr=output)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert process.poll() is None, read_running_log(log)
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', read_running_log(log))
            if match:
                base = match.group(1)
                break
            time.sleep(.05)
        else: raise AssertionError(read_running_log(log))
        def request(path, value=None):
            data = json.dumps(value).encode() if value is not None else None
            with urllib.request.urlopen(urllib.request.Request(base + path, data=data,
                                        headers={'Authorization': 'Bearer ' + settings['http']['api_token'],
                                                 'Content-Type': 'application/json'}), timeout=10) as response:
                return json.load(response)
        original = request('/api/skills')
        assert len(original['skills']) == 1
        lock.unlink()  # Explicit fixture removal; no installer or automatic rollback.
        assert run().returncode == 0 and request('/api/skills') == original
        request('/api/skills/reload', {})
        assert request('/api/skills')['skills'] == []
        print('PASS lock create 7: authenticated original service retains loaded table until explicit reload of the newly created disk policy')
    finally:
        if process is not None:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            assert 'panicked at' not in log.read_text()
