#!/usr/bin/env python3
"""Actual operator source review, conditional publication and explicit activation."""
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
env['JIACLAW_LOG_LEVEL'] = 'info'
digest = lambda raw: hashlib.sha256(raw).hexdigest()
with tempfile.TemporaryDirectory(prefix='jiaclaw-source-approval-') as temporary:
    root = Path(temporary).resolve()
    workspace = root / 'workspace'
    folder = workspace / 'skills/reviewed'
    other = workspace / 'skills/other'
    folder.mkdir(parents=True)
    other.mkdir()
    raw = b'---\nname: selected\ndescription: old version\n---\nOLD_BODY'
    updated = b'---\nname: selected\ndescription: new version\n---\nNEW_BODY'
    stable = b'---\nname: other\ndescription: unchanged\n---\nOTHER_BODY'
    leaf = folder / 'SKILL.md'
    leaf.write_bytes(raw)
    other_leaf = other / 'SKILL.md'
    other_leaf.write_bytes(stable)
    old_pin = {'repository': 'https://github.com/example/skills', 'revision': 'a' * 40, 'skill_sha256': digest(raw)}
    other_pin = {**old_pin, 'skill_sha256': digest(stable)}
    proposed = {**old_pin, 'revision': 'b' * 40, 'skill_sha256': digest(updated)}
    lock = workspace / 'skills.lock.json'
    lock.write_text(json.dumps({'version': 1, 'skills': [{'directory': 'reviewed', 'source': old_pin}, {'directory': 'other', 'source': other_pin}]}))
    config = root / 'config.json'
    token = uuid.uuid4().hex
    settings = {'agent': {'name': 'source-review', 'description': 'Disposable', 'workspace_path': str(workspace), 'max_turns': 3, 'system_instructions': '', 'skill_lock_required': True}, 'provider': {'provider_type': 'stub'}, 'http': {'bind': '127.0.0.1:0', 'api_token': token}}
    config.write_text(json.dumps(settings))

    def cli(*args, **options):
        return subprocess.run([str(binary), *args, '--config', str(config)], env=env, capture_output=True, text=True, timeout=10, **options)
    def approve(pin=None, expected=None, directory='reviewed', **options):
        pin = pin or proposed
        return cli('skills', 'set-source', directory, '--repository', pin['repository'], '--revision', pin['revision'], '--skill-sha256', pin['skill_sha256'], '--if-manifest-sha256', expected or digest(lock.read_bytes()), **options)
    def enabled(value):
        return cli('skills', 'set-enabled', 'reviewed', '--enabled', str(value).lower(), '--if-manifest-sha256', digest(lock.read_bytes()))
    def selected(policy):
        return next(entry for entry in policy['skills'] if entry['directory'] == 'reviewed')
    def success(result):
        assert result.returncode == 0, (result.stdout, result.stderr)
        value = json.loads(result.stdout)
        assert value['runtime_applied'] is False
        policy = value['disk_policy']
        assert policy['manifest_sha256'] == digest(lock.read_bytes())
        assert next(entry for entry in policy['skills'] if entry['directory'] == 'other')['source'] == other_pin
        return policy
    def rejected(result, before, inode=None):
        assert result.returncode != 0 and not result.stdout.strip(), (result.stdout, result.stderr)
        assert lock.read_bytes() == before
        if inode is not None:
            assert lock.stat().st_ino == inode
        assert not list(workspace.glob('.jiaclaw-memory-*'))

    if '--expect-parent-rejection' in sys.argv:
        result = approve()
        rejected(result, lock.read_bytes())
        assert "unrecognized subcommand 'set-source'" in result.stderr
        print('PASS: frozen parent has no checked source approval command')
        sys.exit(0)

    before = lock.read_bytes()
    rejected(approve(), before, lock.stat().st_ino)
    assert success(enabled(False))['version'] == 2
    leaf.write_bytes(updated)
    before = lock.read_bytes()
    rejected(approve(expected=digest(before[:-1])), before)
    rejected(approve({**proposed, 'skill_sha256': digest(raw)}), before)
    other_leaf.write_bytes(b'drift')
    rejected(approve(), before)
    other_leaf.write_bytes(stable)
    approved = success(approve())
    assert selected(approved) == {'directory': 'reviewed', 'enabled': False, 'source': proposed}
    assert cli('skills', 'policy').returncode == 0
    rejected(approve(), lock.read_bytes(), lock.stat().st_ino)
    print('PASS source approval 1: active/stale/digest/full-catalog rejection; reviewed source retained disabled, other origins preserved and no-op never republishes')

    next_pin = {**proposed, 'revision': 'c' * 40}
    for key, value in [('repository', 'http://example.test/repo'), ('repository', 'https://user:APPROVAL_SECRET@example.test/repo'), ('repository', 'https://example.test/repo?secret=APPROVAL_SECRET'), ('repository', 'https://example.test/repo#fragment'), ('revision', 'main'), ('revision', 'A' * 40), ('skill_sha256', 'F' * 64)]:
        result = approve({**next_pin, key: value})
        rejected(result, lock.read_bytes(), lock.stat().st_ino)
        assert 'APPROVAL_SECRET' not in result.stderr
    rejected(approve(next_pin, directory='../escape'), lock.read_bytes())
    rejected(approve(next_pin, directory='selected'), lock.read_bytes())
    for flag in ['--repository', '--revision', '--skill-sha256', '--if-manifest-sha256']:
        args = ['skills', 'set-source', 'reviewed', '--repository', next_pin['repository'], '--revision', next_pin['revision'], '--skill-sha256', next_pin['skill_sha256'], '--if-manifest-sha256', digest(lock.read_bytes())]
        offset = args.index(flag)
        rejected(cli(*(args[:offset] + args[offset + 2:])), lock.read_bytes())
    settings['agent']['skill_lock_required'] = False
    config.write_text(json.dumps(settings))
    rejected(approve(next_pin), lock.read_bytes())
    settings['agent']['skill_lock_required'] = True
    config.write_text(json.dumps(settings))
    print('PASS source approval 2: all explicit parameters, required policy, declared directory and bounded credential-free immutable source checks')

    foreign = root / 'foreign.md'
    foreign.write_bytes(updated)
    if os.name == 'posix':
        for kind in ['symlink', 'hardlink', 'fifo']:
            leaf.unlink()
            if kind == 'symlink':
                leaf.symlink_to(foreign)
            elif kind == 'hardlink':
                os.link(foreign, leaf)
            else:
                os.mkfifo(leaf)
            rejected(approve(next_pin), lock.read_bytes(), lock.stat().st_ino)
            assert foreign.read_bytes() == updated
            leaf.unlink()
            leaf.write_bytes(updated)
    for invalid in [b'\xff', b'x' * (128 * 1024 + 1), b'---\nname: [PRIVATE_PARSE_SECRET\n---\nbody']:
        leaf.write_bytes(invalid)
        result = approve({**next_pin, 'skill_sha256': digest(invalid)})
        rejected(result, lock.read_bytes())
        assert 'PRIVATE_PARSE_SECRET' not in result.stderr
    leaf.unlink()
    rejected(approve(next_pin), lock.read_bytes())
    leaf.write_bytes(updated)
    print('PASS source approval 3: capability-relative target read rejects linked/special/missing/oversized/invalid body without manifest publication or private parse echo')

    if os.name == 'posix':
        import fcntl
        owner = os.open(workspace, os.O_RDONLY)
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
            rejected(approve(next_pin), lock.read_bytes())
        finally:
            os.close(owner)
    captured = digest(lock.read_bytes())
    choices = [{**next_pin, 'revision': char * 40} for char in ['d', 'e']]
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        outcomes = list(pool.map(lambda pin: approve(pin, captured), choices))
    assert sum(result.returncode == 0 for result in outcomes) == 1, [(r.stdout, r.stderr) for r in outcomes]
    current = selected(json.loads(lock.read_text()))['source']
    assert current in choices
    rejected(approve(current), lock.read_bytes(), lock.stat().st_ino)
    print('PASS source approval 4: actual workspace lock and two different reviewed changes with one captured hash permit one commit, retain disabled state and release ownership')

    if os.name == 'posix':
        import resource
        import signal
        def no_file_writes():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
        rejected(approve(next_pin, preexec_fn=no_file_writes), lock.read_bytes(), lock.stat().st_ino)
        workspace.chmod(0o500)
        try:
            rejected(approve(next_pin), lock.read_bytes(), lock.stat().st_ino)
        finally:
            workspace.chmod(0o700)
    assert lock.stat().st_mode & 0o777 == 0o600
    print('PASS source approval 5: real OS file-limit/permission failures preserve original bytes and inode, clean staging and private publication mode')

    valid = lock.read_bytes()
    foreign_lock = root / 'foreign-lock.json'
    foreign_lock.write_bytes(valid)
    if os.name == 'posix':
        for kind in ['symlink', 'hardlink', 'fifo']:
            lock.unlink()
            if kind == 'symlink':
                lock.symlink_to(foreign_lock)
            elif kind == 'hardlink':
                os.link(foreign_lock, lock)
            else:
                os.mkfifo(lock)
            result = approve(next_pin, digest(valid))
            assert result.returncode != 0 and not result.stdout.strip()
            assert foreign_lock.read_bytes() == valid
            lock.unlink()
            lock.write_bytes(valid)
    lock.unlink()
    result = approve(next_pin, digest(valid))
    assert result.returncode != 0 and not lock.exists()
    lock.write_bytes(valid)
    print('PASS source approval 6: original manifest capability refuses links/special/missing input without foreign writes or lock creation')

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
        assert [entry['name'] for entry in initial['skills']] == ['other']
        success(approve(next_pin))
        assert request('/api/skills') == initial
        success(enabled(True))
        assert request('/api/skills') == initial
        request('/api/skills/reload', {})
        loaded = request('/api/skills')['skills']
        reviewed = next(entry for entry in loaded if entry['name'] == 'selected')
        assert reviewed['description'] == 'new version' and reviewed['source'] == next_pin
        print('PASS source approval 7: actual authenticated runtime changes only after separate explicit activation and successful reload of the reviewed version')
    finally:
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            assert 'panicked at' not in log.read_text()
