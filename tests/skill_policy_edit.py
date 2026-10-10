#!/usr/bin/env python3
"""Actual operator CAS policy writes; no provider request or model write authority."""
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
env.update(JIACLAW_LOG_LEVEL='info')
digest = lambda raw: hashlib.sha256(raw).hexdigest()
token = uuid.uuid4().hex
with tempfile.TemporaryDirectory(prefix='jiaclaw-policy-edit-') as temporary:
    root = Path(temporary).resolve()
    workspace = root / 'workspace'
    folder = workspace / 'skills/reviewed'
    folder.mkdir(parents=True)
    raw = b'---\nname: selected\ndescription: operator policy\n---\nPOLICY_BODY'
    leaf = folder / 'SKILL.md'
    leaf.write_bytes(raw)
    source = {'repository': 'https://github.com/example/skills', 'revision': 'a' * 40, 'skill_sha256': digest(raw)}
    lock = workspace / 'skills.lock.json'
    original = json.dumps({'version': 1, 'skills': [{'directory': 'reviewed', 'source': source}]}).encode()
    lock.write_bytes(original)
    config = root / 'config.json'
    settings = {'agent': {'name': 'policy', 'description': 'Disposable', 'workspace_path': str(workspace), 'max_turns': 3, 'system_instructions': '', 'skill_lock_required': True},
                'provider': {'provider_type': 'stub'},
                'http': {'bind': '127.0.0.1:0', 'api_token': token}}
    config.write_text(json.dumps(settings))
    def cli(*args, **options):
        return subprocess.run([str(binary), *args, '--config', str(config)], env=env, capture_output=True, text=True, timeout=10, **options)
    def edit(enabled, expected=None, directory='reviewed'):
        return cli('skills', 'set-enabled', directory, '--enabled', str(enabled).lower(), '--if-manifest-sha256', expected or digest(lock.read_bytes()))
    def success(result):
        assert result.returncode == 0, (result.stdout, result.stderr)
        value = json.loads(result.stdout)
        assert value['runtime_applied'] is False
        assert value['disk_policy']['manifest_sha256'] == digest(lock.read_bytes())
        assert value['disk_policy']['skills'][0]['source'] == source
        return value['disk_policy']
    def rejected(result, before):
        assert result.returncode != 0 and not result.stdout.strip(), (result.stdout, result.stderr)
        assert lock.read_bytes() == before
    if '--expect-parent-rejection' in sys.argv:
        rejected(edit(False), original)
        print('PASS: parent has no checked policy edit command (capability gap)')
        sys.exit(0)

    current = success(edit(False))
    assert current['version'] == 2 and current['skills'][0]['enabled'] is False
    assert cli('skills', 'policy').returncode == 0
    before_inode = lock.stat().st_ino
    rejected(edit(False), lock.read_bytes())
    assert lock.stat().st_ino == before_inode
    rejected(edit(True, digest(original)), lock.read_bytes())
    assert success(edit(True))['skills'][0]['enabled'] is True
    before_inode = lock.stat().st_ino
    rejected(edit(True), lock.read_bytes())
    assert lock.stat().st_ino == before_inode
    print('PASS policy edit 1: v1 conversion, retained source, exact content hash, no-op rejects without publication and disk-only receipt')

    for args in [('skills', 'set-enabled', 'reviewed'),
                 ('skills', 'set-enabled', 'reviewed', '--enabled', 'false'),
                 ('skills', 'set-enabled', 'reviewed', '--enabled', 'null', '--if-manifest-sha256', digest(lock.read_bytes()))]:
        rejected(cli(*args), lock.read_bytes())
    for directory, expected in [('selected', digest(lock.read_bytes())), ('../escape', digest(lock.read_bytes())), ('reviewed', 'bad')]:
        rejected(edit(False, expected, directory), lock.read_bytes())
    settings['agent']['skill_lock_required'] = False
    config.write_text(json.dumps(settings))
    rejected(edit(False), lock.read_bytes())
    settings['agent']['skill_lock_required'] = True
    config.write_text(json.dumps(settings))
    print('PASS policy edit 2: explicit parameters, declared directory and required opt-in reject without manifest changes')

    leaf.write_bytes(b'drift')
    assert cli('skills', 'policy').returncode != 0
    assert success(edit(False))['skills'][0]['enabled'] is False
    rejected(edit(True), lock.read_bytes())
    leaf.write_bytes(raw)
    assert success(edit(True))['skills'][0]['enabled'] is True
    print('PASS policy edit 3: disabling drifted skill validates proposed table; invalid re-enable preserves exact bytes')

    if os.name == 'posix':
        import fcntl
        owner = os.open(workspace, os.O_RDONLY)
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
            rejected(edit(False), lock.read_bytes())
        finally:
            os.close(owner)

    expected = digest(lock.read_bytes())
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(lambda _: edit(False, expected), range(2)))
    assert sum(result.returncode == 0 for result in results) == 1, [(r.stdout, r.stderr) for r in results]
    assert json.loads(lock.read_text())['skills'][0]['enabled'] is False
    assert len(list(workspace.glob('.jiaclaw-memory-*'))) == 0
    print('PASS policy edit 4: two actual byte-changing CLI writers with one captured hash admit only one commit')

    valid = lock.read_bytes()
    foreign = root / 'foreign.json'
    foreign.write_bytes(valid)
    if os.name == 'posix':
        for kind in ['symlink', 'hardlink', 'fifo']:
            lock.unlink()
            if kind == 'symlink':
                lock.symlink_to(foreign)
            elif kind == 'hardlink':
                os.link(foreign, lock)
            else:
                os.mkfifo(lock)
            result = edit(True, digest(valid))
            assert result.returncode != 0 and not result.stdout.strip(), (kind, result.stdout, result.stderr)
            assert foreign.read_bytes() == valid
            lock.unlink()
            lock.write_bytes(valid)
    oversized = valid + b' ' * (128 * 1024)
    lock.write_bytes(oversized)
    rejected(edit(True, digest(oversized)), oversized)
    lock.unlink()
    result = edit(True, digest(valid))
    assert result.returncode != 0 and not lock.exists()
    lock.write_bytes(valid)
    assert not list(workspace.glob('.jiaclaw-memory-*'))
    print('PASS policy edit 5: linked/special/oversized/missing manifests reject without changing foreign bytes or creating locks')

    if os.name == 'posix':
        import resource
        import signal
        def deny_file_writes():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
        result = cli('skills', 'set-enabled', 'reviewed', '--enabled', 'true',
                     '--if-manifest-sha256', digest(valid), preexec_fn=deny_file_writes)
        rejected(result, valid)
        assert not list(workspace.glob('.jiaclaw-memory-*'))
        workspace.chmod(0o500)
        try:
            rejected(edit(True), valid)
        finally:
            workspace.chmod(0o700)
    print('PASS policy edit 6: real OS write-limit and directory permission failures preserve old bytes and clean temporary files')

    success(edit(True))
    log = root / 'serve.log'
    process = None
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
        loaded = request('/api/skills')
        assert loaded['skills'][0]['name'] == 'selected'
        success(edit(False))
        assert request('/api/skills') == loaded
        request('/api/skills/reload', {})
        assert request('/api/skills')['skills'] == []
        print('PASS policy edit 7: actual authenticated running registry changes only after explicit successful reload')
    finally:
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            assert 'panicked at' not in log.read_text()
