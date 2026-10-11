#!/usr/bin/env python3
"""Actual conditional registration of reviewed local skills, never installation."""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
from host_log import read_running_log

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
digest = lambda raw: hashlib.sha256(raw).hexdigest()
with tempfile.TemporaryDirectory(prefix='jiaclaw-registration-') as temporary:
    root = Path(temporary).resolve()
    workspace = root / 'workspace'
    skills = workspace / 'skills'
    stable = b'---\nname: stable\ndescription: retained\n---\nOLD_BODY'
    proposed = b'---\nname: new-model-name\ndescription: reviewed\n---\nNEW_BODY'
    for name, raw in [('stable', stable), ('new', proposed)]:
        folder = skills / name
        folder.mkdir(parents=True)
        (folder / 'SKILL.md').write_bytes(raw)
    source = {'repository': 'https://github.com/example/skills', 'revision': 'a' * 40, 'skill_sha256': digest(proposed)}
    old_source = {**source, 'skill_sha256': digest(stable)}
    lock = workspace / 'skills.lock.json'
    lock.write_text(json.dumps({'version': 1, 'skills': [{'directory': 'stable', 'source': old_source}]}))
    config = root / 'config.json'
    token = uuid.uuid4().hex
    settings = {'agent': {'name': 'registration', 'description': 'Disposable', 'workspace_path': str(workspace), 'max_turns': 3, 'system_instructions': '', 'skill_lock_required': True}, 'provider': {'provider_type': 'stub'}, 'http': {'bind': '127.0.0.1:0', 'api_token': token}}
    config.write_text(json.dumps(settings))

    def cli(*args, **options):
        return subprocess.run([str(binary), *args, '--config', str(config)], env=env, capture_output=True, text=True, timeout=10, **options)
    def register(name='new', pin=None, expected=None, **options):
        pin = pin or source
        return cli('skills', 'register', name, '--repository', pin['repository'], '--revision', pin['revision'], '--skill-sha256', pin['skill_sha256'], '--if-manifest-sha256', expected or digest(lock.read_bytes()), **options)
    def rejected(result, before, inode=None):
        assert result.returncode != 0 and not result.stdout.strip(), (result.stdout, result.stderr)
        assert lock.read_bytes() == before
        if inode is not None:
            assert lock.stat().st_ino == inode
        assert not list(workspace.glob('.jiaclaw-memory-*'))
    def success(result):
        assert result.returncode == 0, (result.stdout, result.stderr)
        value = json.loads(result.stdout)
        assert value['runtime_applied'] is False
        policy = value['disk_policy']
        assert policy['version'] == 2 and policy['manifest_sha256'] == digest(lock.read_bytes())
        assert next(e for e in policy['skills'] if e['directory'] == 'stable') == {'directory': 'stable', 'enabled': True, 'source': old_source}
        assert [e['directory'] for e in policy['skills']] == sorted(e['directory'] for e in policy['skills'])
        return policy

    # Existing frozen parent must fail this same real call, not a mutated fixture.
    before = lock.read_bytes()
    rejected(register(expected='0' * 64), before, lock.stat().st_ino)
    rejected(register(pin={**source, 'skill_sha256': digest(stable)}), before)
    (skills / 'stable/SKILL.md').write_bytes(b'drift')
    rejected(register(), before)
    (skills / 'stable/SKILL.md').write_bytes(stable)
    body_before = (skills / 'new/SKILL.md').stat().st_ino
    registered = success(register())
    assert next(e for e in registered['skills'] if e['directory'] == 'new') == {'directory': 'new', 'enabled': False, 'source': source}
    assert (skills / 'new/SKILL.md').read_bytes() == proposed and (skills / 'new/SKILL.md').stat().st_ino == body_before
    rejected(register(), lock.read_bytes(), lock.stat().st_ino)
    assert cli('skills', 'policy').returncode == 0
    print('PASS registration 1: reviewed local body registered disabled, v1 origins preserved, stale/digest/full-catalog failures and duplicate never publish')

    pending = skills / 'pending'
    pending.mkdir()
    leaf = pending / 'SKILL.md'
    leaf.write_bytes(proposed)
    for key, value in [('repository', 'http://example.test/repo'), ('repository', 'https://user:REGISTRATION_SECRET@example.test/repo'), ('repository', 'https://example.test/repo?secret=REGISTRATION_SECRET'), ('repository', 'https://example.test/repo#fragment'), ('revision', 'main'), ('revision', 'A' * 40), ('skill_sha256', 'F' * 64)]:
        result = register('pending', {**source, key: value})
        rejected(result, lock.read_bytes(), lock.stat().st_ino)
        assert 'REGISTRATION_SECRET' not in result.stderr
    for name in ['../escape', '/escape', 'new-model-name', 'two/components']:
        rejected(register(name), lock.read_bytes())
    for flag in ['--repository', '--revision', '--skill-sha256', '--if-manifest-sha256']:
        args = ['skills', 'register', 'pending', '--repository', source['repository'], '--revision', source['revision'], '--skill-sha256', source['skill_sha256'], '--if-manifest-sha256', digest(lock.read_bytes())]
        offset = args.index(flag)
        rejected(cli(*(args[:offset] + args[offset + 2:])), lock.read_bytes())
    settings['agent']['skill_lock_required'] = False
    config.write_text(json.dumps(settings))
    rejected(register('pending'), lock.read_bytes())
    settings['agent']['skill_lock_required'] = True
    config.write_text(json.dumps(settings))
    print('PASS registration 2: all explicit arguments, required policy, bounded credential-free immutable origin and directory checks')

    foreign = root / 'foreign.md'
    foreign.write_bytes(proposed)
    if os.name == 'posix':
        for kind in ['symlink', 'hardlink', 'fifo']:
            leaf.unlink()
            if kind == 'symlink':
                leaf.symlink_to(foreign)
            elif kind == 'hardlink':
                os.link(foreign, leaf)
            else:
                os.mkfifo(leaf)
            rejected(register('pending'), lock.read_bytes(), lock.stat().st_ino)
            assert foreign.read_bytes() == proposed
            leaf.unlink()
            leaf.write_bytes(proposed)
    for raw in [b'\xff', b'x' * (128 * 1024 + 1), b'---\nname: [REGISTRATION_PRIVATE\n---\nbody']:
        leaf.write_bytes(raw)
        result = register('pending', {**source, 'skill_sha256': digest(raw)})
        rejected(result, lock.read_bytes())
        assert 'REGISTRATION_PRIVATE' not in result.stderr
    leaf.unlink()
    rejected(register('pending'), lock.read_bytes())
    leaf.write_bytes(proposed)
    print('PASS registration 3: actual target links, hardlinks, FIFO, invalid UTF8/schema, oversized and missing body cannot publish')

    if os.name == 'posix':
        import fcntl
        owner = os.open(workspace, os.O_RDONLY)
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
            rejected(register('pending'), lock.read_bytes())
        finally:
            os.close(owner)
    captured = digest(lock.read_bytes())
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        outcomes = list(pool.map(lambda _: register('pending', expected=captured), range(2)))
    assert sum(r.returncode == 0 for r in outcomes) == 1, [(r.stdout, r.stderr) for r in outcomes]
    success(next(r for r in outcomes if r.returncode == 0))
    rejected(register('pending'), lock.read_bytes(), lock.stat().st_ino)
    print('PASS registration 4: actual held workspace writer and same-hash concurrent registrations permit one disabled declaration and release ownership')

    valid = lock.read_bytes()
    entries = [{'directory': 'entry-' + str(i), 'enabled': False, 'source': source} for i in range(64)]
    lock.write_text(json.dumps({'version': 2, 'skills': entries}))
    rejected(register('extra'), lock.read_bytes(), lock.stat().st_ino)
    lock.write_bytes(valid)
    foreign_lock = root / 'foreign-lock.json'
    foreign_lock.write_bytes(valid)
    if os.name == 'posix':
        for kind in ['symlink', 'hardlink', 'fifo', 'directory']:
            lock.unlink()
            if kind == 'symlink':
                lock.symlink_to(foreign_lock)
            elif kind == 'hardlink':
                os.link(foreign_lock, lock)
            elif kind == 'fifo':
                os.mkfifo(lock)
            else:
                lock.mkdir()
            result = register('extra', expected=digest(valid))
            assert result.returncode != 0 and not result.stdout.strip()
            assert foreign_lock.read_bytes() == valid
            if kind == 'directory':
                lock.rmdir()
            else:
                lock.unlink()
            lock.write_bytes(valid)
    lock.unlink()
    result = register('extra', expected=digest(valid))
    assert result.returncode != 0 and not lock.exists()
    lock.write_bytes(valid)
    print('PASS registration 5: full 64-record capacity, unsafe original manifests and missing lock reject without creation or foreign writes')

    extra = skills / 'extra'
    extra.mkdir()
    (extra / 'SKILL.md').write_bytes(proposed)
    if os.name == 'posix':
        import resource
        import signal
        def no_file_writes():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
        rejected(register('extra', preexec_fn=no_file_writes), lock.read_bytes(), lock.stat().st_ino)
        workspace.chmod(0o500)
        try:
            rejected(register('extra'), lock.read_bytes(), lock.stat().st_ino)
        finally:
            workspace.chmod(0o700)
    success(register('extra'))
    assert lock.stat().st_mode & 0o777 == 0o600
    print('PASS registration 6: actual OS file-limit and permission failures leave byte/inode identity, clean staging, then private publication succeeds')

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
        else:
            raise AssertionError(read_running_log(log))
        def request(path, value=None):
            data = json.dumps(value).encode() if value is not None else None
            with urllib.request.urlopen(urllib.request.Request(base + path, data=data, headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}), timeout=10) as response:
                return json.load(response)
        initial = request('/api/skills')
        assert [e['name'] for e in initial['skills']] == ['stable']
        runtime = skills / 'runtime'
        runtime.mkdir()
        (runtime / 'SKILL.md').write_bytes(proposed)
        success(register('runtime'))
        assert request('/api/skills') == initial
        success(cli('skills', 'set-enabled', 'runtime', '--enabled', 'true', '--if-manifest-sha256', digest(lock.read_bytes())))
        assert request('/api/skills') == initial
        request('/api/skills/reload', {})
        loaded = request('/api/skills')['skills']
        selected = next(e for e in loaded if e['name'] == 'new-model-name')
        assert len(loaded) == 2 and selected['source'] == source
        print('PASS registration 7: actual authenticated service keeps original table after registration and activation until explicit successful reload')
    finally:
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            assert 'panicked at' not in log.read_text()
