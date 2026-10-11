#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Actual CLI/OS installation and explicit recovery; no vendor/power-loss certification."""
import copy
import fcntl
import hashlib
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import time
import re
import urllib.request
import uuid
from host_log import read_running_log
from stopped_child import wait_stopped

binary = pathlib.Path(sys.argv[1]).resolve()
sha = lambda value: hashlib.sha256(value.encode()).hexdigest()
with tempfile.TemporaryDirectory(prefix='jiaclaw-skill-install-') as temporary:
    root = pathlib.Path(temporary).resolve()
    interposer = root / ('fsync.dylib' if sys.platform == 'darwin' else 'fsync.so')
    source = pathlib.Path(__file__).with_name('skill_install_fsync.c')
    subprocess.run(['cc', '-dynamiclib' if sys.platform == 'darwin' else '-shared', '-fPIC', str(source),
                    '-o', str(interposer), *([] if sys.platform == 'darwin' else ['-ldl'])], check=True, capture_output=True)
    env = {key: value for key, value in os.environ.items() if key != 'JIACLAW_API_KEY'}

    def fixture(label):
        workspace = root / label
        workspace.mkdir()
        (workspace / 'skills').mkdir()
        original = json.dumps({'version': 2, 'skills': []}) + '\n'
        (workspace / 'skills.lock.json').write_text(original)
        resource = 'reviewed reference\n'
        body = ('---\nname: newskill\ndescription: local reviewed skill\njiaclaw_resources:\n'
                f'  - path: references/nested/ref.md\n    sha256: {sha(resource)}\n---\nbody\n')
        bundle = {'version': 1, 'directory': 'newskill', 'source': {
            'repository': 'https://github.com/example/skills', 'revision': 'a' * 40, 'skill_sha256': sha(body)},
            'files': [{'path': 'SKILL.md', 'text': body}, {'path': 'references/nested/ref.md', 'text': resource}]}
        raw = json.dumps(bundle) + '\n'
        (workspace / 'reviewed.json').write_text(raw)
        config = workspace / 'config.json'
        config.write_text(json.dumps({'agent': {'name': 'install', 'description': 'disposable', 'max_turns': 3,
            'system_instructions': '', 'workspace_path': str(workspace), 'skill_lock_required': True},
            'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:1', 'model': 'unused'},
            'model_calls': {'enabled': True, 'store_path': '../state/calls.sqlite3'},
            'memory': {'semantic': {'enabled': True, 'model': 'unused', 'space_revision': 'test', 'dimensions': 8,
                                    'index_path': '../state/index.sqlite3'}}}))
        operation_id = str(uuid.uuid4())
        return workspace, config, original, raw, bundle, operation_id

    def command(f, action, *args):
        return [str(binary), 'skills', action, *args, '--config', str(f[1])]

    def install(f, extra_env=env):
        return subprocess.run(command(f, 'install', '--operation-id', f[5], '--from', 'reviewed.json',
            '--if-bundle-sha256', sha(f[3]), '--if-manifest-sha256', sha(f[2])),
            capture_output=True, text=True, env=extra_env, timeout=10)

    def status(f):
        result = subprocess.run(command(f, 'install-status', f[5]), capture_output=True, text=True, env=env, timeout=10)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    def finish(f, saved):
        return subprocess.run(command(f, 'finish-install', f[5], '--if-intent-sha256', saved['intent_sha256']),
                              capture_output=True, text=True, env=env, timeout=10)

    def verify(f):
        result = status(f)
        assert result['state'] == 'completed' and result['runtime_applied'] is False
        manifest = json.loads((f[0] / 'skills.lock.json').read_text())
        assert manifest['skills'] == [{'directory': 'newskill', 'enabled': False, 'source': f[4]['source']}]
        for file in f[4]['files']:
            assert (f[0] / 'skills' / 'newskill' / file['path']).read_bytes() == file['text'].encode()
        assert not (root / 'state').exists()
        assert subprocess.run(command(f, 'policy'), capture_output=True, env=env, timeout=10).returncode == 0
        return result

    f = fixture('success')
    result = install(f)
    assert result.returncode == 0, (result.stdout, result.stderr)
    receipt = verify(f)
    before = (f[0] / 'skills.lock.json').stat().st_ino
    assert install(f).returncode != 0
    assert finish(f, receipt).returncode == 0
    assert (f[0] / 'skills.lock.json').stat().st_ino == before
    print('PASS skill install 1: exact local body/resources, disabled publication, historical receipt, no implicit provider initialization or duplicate replay')

    f = fixture('invalid')
    for change in ('hash', 'extra', 'resource', 'body', 'path', 'source', 'duplicate'):
        altered = copy.deepcopy(f[4])
        if change == 'extra': altered['files'].append({'path': 'scripts/run.sh', 'text': 'echo bad'})
        elif change == 'resource': altered['files'][1]['text'] += 'drift'
        elif change == 'body': altered['files'][0]['text'] += 'drift'
        elif change == 'path': altered['directory'] = '../escape'
        elif change == 'source': altered['source']['repository'] = 'https://user:secret@example.com/pkg'
        elif change == 'duplicate': altered['files'].append(altered['files'][0])
        raw = json.dumps(altered) + '\n'
        (f[0] / 'reviewed.json').write_text(raw)
        failed = install((f[0], f[1], f[2], 'unreviewed bytes' if change == 'hash' else raw, altered, f[5]))
        assert failed.returncode != 0 and not failed.stdout.strip(), failed
        assert (f[0] / 'skills.lock.json').read_text() == f[2]
        assert not (f[0] / '.jiaclaw-skill-installs').exists()
    print('PASS skill install 2: reviewed hashes/schema/source/resource coverage reject before any journal or namespace publication')

    for kind in ('symlink', 'hardlink', 'fifo'):
        f = fixture(kind)
        path = f[0] / 'reviewed.json'
        path.unlink()
        external = root / f'{kind}-external'
        external.write_text(f[3])
        if kind == 'symlink': path.symlink_to(external)
        elif kind == 'hardlink': os.link(external, path)
        else: os.mkfifo(path)
        assert install(f).returncode != 0
        assert not (f[0] / '.jiaclaw-skill-installs').exists()
    print('PASS skill install 3: actual symlink/hardlink/FIFO input refusal without blocking or journal creation')

    for phase, expected in (('intent', 'prepared'), ('stage', 'prepared'), ('policy', 'policy_published'),
                            ('files', 'files_published'), ('receipt', 'completed')):
        f = fixture('kill-' + phase)
        marker = root / ('held-' + phase)
        injected = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(interposer),
            'JIACLAW_TEST_INSTALL_WORKSPACE': str(f[0]), 'JIACLAW_TEST_INSTALL_ID': f[5],
            'JIACLAW_TEST_INSTALL_PHASE': phase, 'JIACLAW_TEST_INSTALL_MARKER': str(marker)}
        process = subprocess.Popen(command(f, 'install', '--operation-id', f[5], '--from', 'reviewed.json',
            '--if-bundle-sha256', sha(f[3]), '--if-manifest-sha256', sha(f[2])), stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, env=injected)
        try:
            deadline = time.monotonic() + 10
            while not marker.exists() and time.monotonic() < deadline and process.poll() is None:
                time.sleep(0.01)
            assert marker.exists(), (phase, process.poll(), process.communicate(timeout=1) if process.poll() is not None else 'no selected fsync')
            wait_stopped(process, deadline)
            # The same owner excludes other installers and original workspace writes.
            competitor = list(f)
            competitor[5] = str(uuid.uuid4())
            refused = install(tuple(competitor))
            assert refused.returncode != 0 and 'lock' in refused.stderr, refused.stderr
            init = subprocess.run([str(binary), 'init', '--path', str(f[0])], env=env, capture_output=True, text=True, timeout=5)
            assert init.returncode != 0 and 'lock' in init.stderr, init.stderr
            process.kill()
            process.communicate(timeout=5)
            assert process.returncode == -signal.SIGKILL
        finally:
            if process.poll() is None: process.kill(); process.communicate(timeout=5)
        saved = status(f)
        assert saved['state'] == expected and saved['operation_id'] == f[5], (phase, saved)
        snapshots = {path: (path.read_bytes(), path.stat().st_ino) for path in (f[0] / '.jiaclaw-skill-installs' / f[5]).glob('*.json')}
        assert status(f) == saved
        assert all(path.read_bytes() == data and path.stat().st_ino == inode for path, (data, inode) in snapshots.items())
        (f[0] / 'reviewed.json').write_text('input changed after original operation')
        assert install(f).returncode != 0
        finished = finish(f, saved)
        assert finished.returncode == 0, (phase, finished.stdout, finished.stderr)
        verify(f)
    print('PASS skill install 4: five actual fsync/SIGKILL phases, retained owner, read-only original status and explicit finish from captured bytes without replay')

    f = fixture('sync-error')
    injected = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(interposer),
        'JIACLAW_TEST_INSTALL_WORKSPACE': str(f[0]), 'JIACLAW_TEST_INSTALL_ID': f[5],
        'JIACLAW_TEST_INSTALL_PHASE': 'policy', 'JIACLAW_TEST_INSTALL_MODE': 'error'}
    assert install(f, injected).returncode != 0
    saved = status(f)
    assert saved['state'] == 'policy_published'
    assert finish(f, saved).returncode == 0
    verify(f)
    print('PASS skill install 5: actual post-policy fsync EIO is a possibly committed failure; original identity inspection then explicit finish')

    f = fixture('writer')
    owner = os.open(f[0], os.O_RDONLY)
    try:
        fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = install(f)
        assert result.returncode != 0 and not (f[0] / '.jiaclaw-skill-installs').exists()
    finally: os.close(owner)
    print('PASS skill install 6: actual original held workspace owner refuses before journal/publication')

    f = fixture('conflict')
    injected = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(interposer),
        'JIACLAW_TEST_INSTALL_WORKSPACE': str(f[0]), 'JIACLAW_TEST_INSTALL_ID': f[5],
        'JIACLAW_TEST_INSTALL_PHASE': 'policy', 'JIACLAW_TEST_INSTALL_MODE': 'error'}
    assert install(f, injected).returncode != 0
    saved = status(f)
    policy = f[0] / 'skills.lock.json'
    captured_policy = policy.read_bytes()
    changed = json.loads(captured_policy)
    changed['skills'][0]['source']['revision'] = 'b' * 40
    policy.write_text(json.dumps(changed))
    inode, content = policy.stat().st_ino, policy.read_bytes()
    assert finish(f, saved).returncode != 0
    assert policy.stat().st_ino == inode and policy.read_bytes() == content
    assert not (f[0] / 'skills/newskill').exists()
    policy.write_bytes(captured_policy)
    assert finish(f, saved).returncode == 0
    verify(f)
    print('PASS skill install 7: actual post-crash operator policy edit conflicts without overwrite; restoring reviewed state allows original explicit finish')

    f = fixture('capacity')
    journal = f[0] / '.jiaclaw-skill-installs'
    journal.mkdir(mode=0o700)
    for _ in range(64): (journal / str(uuid.uuid4())).mkdir(mode=0o700)
    assert install(f).returncode != 0 and len(list(journal.iterdir())) == 64
    assert (f[0] / 'skills.lock.json').read_text() == f[2]
    print('PASS skill install 8: actual 64-record journal capacity refuses before creating another operation')

    for kind in ('symlink', 'hardlink', 'fifo', 'oversize', 'unsafe-stage'):
        f = fixture('incomplete-' + kind)
        journal = f[0] / '.jiaclaw-skill-installs'
        journal.mkdir(mode=0o700)
        op = journal / f[5]
        op.mkdir(mode=0o700)
        leaf = op / ('stage' if kind == 'unsafe-stage' else 'bundle.json')
        external = f[0] / 'external'
        external.write_text('original evidence'); external.chmod(0o600)
        if kind in ('symlink', 'unsafe-stage'): leaf.symlink_to(external if kind == 'symlink' else f[0] / 'skills')
        elif kind == 'hardlink': os.link(external, leaf)
        elif kind == 'fifo': os.mkfifo(leaf, 0o600)
        else: leaf.write_bytes(b'x' * (512 * 1024 + 1)); leaf.chmod(0o600)
        before = leaf.lstat().st_ino
        inspected = subprocess.run(command(f, 'install-status', f[5]), capture_output=True, text=True, env=env, timeout=5)
        assert inspected.returncode != 0 and not inspected.stdout.strip(), inspected
        assert leaf.lstat().st_ino == before and external.read_text() == 'original evidence'
        assert (f[0] / 'skills.lock.json').read_text() == f[2]
    print('PASS skill install 10: incomplete original records reject unsafe known snapshots/stage without following links, blocking, or changing evidence')

    f = fixture('original-temporary-conflict')
    marker = root / 'stage-temp-held'
    injected = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(interposer),
        'JIACLAW_TEST_INSTALL_WORKSPACE': str(f[0]), 'JIACLAW_TEST_INSTALL_ID': f[5],
        'JIACLAW_TEST_INSTALL_PHASE': 'stage-temp', 'JIACLAW_TEST_INSTALL_MARKER': str(marker)}
    process = subprocess.Popen(command(f, 'install', '--operation-id', f[5], '--from', 'reviewed.json',
        '--if-bundle-sha256', sha(f[3]), '--if-manifest-sha256', sha(f[2])),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=injected)
    try:
        deadline = time.monotonic() + 10
        while not marker.exists() and time.monotonic() < deadline and process.poll() is None: time.sleep(.01)
        assert marker.exists(), process.poll()
        wait_stopped(process, deadline)
        process.kill(); process.communicate(timeout=5)
        assert process.returncode == -signal.SIGKILL
    finally:
        if process.poll() is None: process.kill(); process.communicate(timeout=5)
    op = f[0] / '.jiaclaw-skill-installs' / f[5]
    stage = op / 'stage'
    temporary_leaf, = stage.glob('.jiaclaw-memory-*')
    original_bytes, original_inode = temporary_leaf.read_bytes(), temporary_leaf.stat().st_ino
    assert original_bytes == f[4]['files'][0]['text'].encode()
    saved = status(f)
    foreign = stage / 'foreign.txt'; foreign.write_text('operator conflict'); foreign.chmod(0o600)
    assert finish(f, saved).returncode != 0
    inspected = subprocess.run(command(f, 'install-status', f[5]), capture_output=True, text=True, env=env, timeout=5)
    assert inspected.returncode != 0
    assert temporary_leaf.read_bytes() == original_bytes and temporary_leaf.stat().st_ino == original_inode
    assert foreign.read_text() == 'operator conflict' and (f[0] / 'skills.lock.json').read_text() == f[2]
    assert not (f[0] / 'skills/newskill').exists()
    foreign.unlink()  # Explicit operator resolution; finish never deletes conflicts.
    assert finish(f, saved).returncode == 0
    verify(f)
    print('PASS skill install 11: actual synced stage leaf survives SIGKILL and rejected finish conflict byte/inode unchanged; explicit operator resolution then original finish succeeds')

    f = fixture('runtime')
    token = 'disposable-install-token'
    settings = json.loads(f[1].read_text())
    settings['provider'] = {'provider_type': 'stub'}
    settings.pop('model_calls'); settings.pop('memory')
    settings['http'] = {'bind': '127.0.0.1:0', 'api_token': token}
    f[1].write_text(json.dumps(settings))
    log = root / 'serve.log'
    process = None
    try:
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(f[1])], env=env, stdout=output, stderr=output)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert process.poll() is None, read_running_log(log)
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', read_running_log(log))
            if match: base = match.group(1); break
            time.sleep(.05)
        else: raise AssertionError(read_running_log(log))
        def request(path, payload=None):
            data = None if payload is None else json.dumps(payload).encode()
            with urllib.request.urlopen(urllib.request.Request(base + path, data=data,
                headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}), timeout=10) as response:
                return json.load(response)
        loaded = request('/api/skills')
        assert loaded['skills'] == []
        marker = root / 'runtime-held'
        injected = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(interposer),
            'JIACLAW_TEST_INSTALL_WORKSPACE': str(f[0]), 'JIACLAW_TEST_INSTALL_ID': f[5],
            'JIACLAW_TEST_INSTALL_PHASE': 'policy', 'JIACLAW_TEST_INSTALL_MARKER': str(marker)}
        owner_process = subprocess.Popen(command(f, 'install', '--operation-id', f[5], '--from', 'reviewed.json',
            '--if-bundle-sha256', sha(f[3]), '--if-manifest-sha256', sha(f[2])),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=injected)
        try:
            deadline = time.monotonic() + 10
            while not marker.exists() and time.monotonic() < deadline and owner_process.poll() is None: time.sleep(.01)
            assert marker.exists()
            wait_stopped(owner_process, deadline)
            assert not (f[0] / 'skills/newskill').exists()
            assert request('/api/skills') == loaded
            request('/api/skills/reload', {})
            assert request('/api/skills') == loaded
            owner_process.send_signal(signal.SIGCONT)
            stdout, stderr = owner_process.communicate(timeout=10)
            assert owner_process.returncode == 0, (stdout, stderr)
        finally:
            if owner_process.poll() is None: owner_process.kill(); owner_process.communicate(timeout=5)
        assert request('/api/skills') == loaded
        request('/api/skills/reload', {})
        assert request('/api/skills') == loaded
        current = sha((f[0] / 'skills.lock.json').read_text())
        enabled = subprocess.run(command(f, 'set-enabled', 'newskill', '--enabled', 'true', '--if-manifest-sha256', current),
                                 env=env, capture_output=True, text=True, timeout=10)
        assert enabled.returncode == 0 and json.loads(enabled.stdout)['runtime_applied'] is False
        assert request('/api/skills') == loaded
        request('/api/skills/reload', {})
        assert request('/api/skills')['skills'][0]['name'] == 'newskill'
    finally:
        if process is not None:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
            assert 'panicked at' not in log.read_text()
    print('PASS skill install 9: authenticated GET/reload during held publication and after install keep old table; separate enable then authenticated reload applies the skill')

