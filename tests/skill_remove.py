#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Actual local removal/original identity/crash interface, not supplier or power-loss certification."""
import copy
import fcntl
import hashlib
import json
import os
import pathlib
import re
import urllib.request
from host_log import read_running_log
from stopped_child import wait_stopped
import signal
import subprocess
import sys
import tempfile
import time
import uuid

binary = pathlib.Path(sys.argv[1]).resolve()
sha = lambda raw: hashlib.sha256(raw.encode()).hexdigest()
env = {k: v for k, v in os.environ.items() if k != 'JIACLAW_API_KEY'}
with tempfile.TemporaryDirectory(prefix='jiaclaw-skill-remove-') as temporary:
    root = pathlib.Path(temporary).resolve()
    interposer = root / ('fsync.dylib' if sys.platform == 'darwin' else 'fsync.so')
    subprocess.run(['cc', '-dynamiclib' if sys.platform == 'darwin' else '-shared', '-fPIC',
        str(pathlib.Path(__file__).with_name('skill_install_fsync.c')), '-o', str(interposer),
        *([] if sys.platform == 'darwin' else ['-ldl'])], check=True, capture_output=True)

    def bundle(body, ref, revision):
        body = ('---\nname: newskill\ndescription: reviewed version\njiaclaw_resources:\n'
                f'  - path: references/nested/ref.md\n    sha256: {sha(ref)}\n---\n{body}\n')
        return {'version': 1, 'directory': 'newskill', 'source': {'repository': 'https://github.com/example/skills',
            'revision': revision * 40, 'skill_sha256': sha(body)}, 'files': [
            {'path': 'SKILL.md', 'text': body}, {'path': 'references/nested/ref.md', 'text': ref}]}

    def run(f, action, *args, extra_env=env):
        return subprocess.run(command(f, action, *args), capture_output=True, text=True, env=extra_env, timeout=15)

    def command(f, action, *args):
        return [str(binary), 'skills', action, *args, '--config', str(f['config'])]

    def fixture(label, equal_bytes=False):
        work = root / label
        work.mkdir()
        (work / 'skills').mkdir()
        empty = json.dumps({'version': 2, 'skills': []}) + '\n'
        (work / 'skills.lock.json').write_text(empty)
        old = bundle('old body', 'old reference\n', 'a')
        raw = json.dumps(old) + '\n'
        (work / 'install.json').write_text(raw)
        config = work / 'config.json'
        config.write_text(json.dumps({'agent': {'name': 'update', 'description': 'disposable', 'max_turns': 3,
            'system_instructions': '', 'workspace_path': str(work), 'skill_lock_required': True},
            'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:1', 'model': 'unused'},
            'model_calls': {'enabled': True, 'store_path': '../state/calls.sqlite3'},
            'memory': {'semantic': {'enabled': True, 'model': 'unused', 'space_revision': 'test', 'dimensions': 8,
                                    'index_path': '../state/index.sqlite3'}}}))
        installed_id = str(uuid.uuid4())
        f = {'work': work, 'config': config, 'old': old, 'id': str(uuid.uuid4()), 'installed_id': installed_id}
        installed = run(f, 'install', '--operation-id', installed_id, '--from', 'install.json',
            '--if-bundle-sha256', sha(raw), '--if-manifest-sha256', sha(empty))
        assert installed.returncode == 0, installed.stderr
        new = copy.deepcopy(old) if equal_bytes else bundle('new body', 'new reference\n', 'b')
        new['source']['revision'] = 'b' * 40
        f.update(new=new, raw=json.dumps(new) + '\n', policy=(work / 'skills.lock.json').read_text(),
                 old_ino=(work / 'skills/newskill').stat().st_ino)
        (work / 'update.json').write_text(f['raw'])
        return f

    def remove_args(f):
        return ['newskill', '--operation-id', f['id'], '--if-manifest-sha256', sha(f['policy']),
                '--if-current-skill-sha256', f['old']['source']['skill_sha256']]

    def remove(f, extra_env=env):
        return run(f, 'uninstall', *remove_args(f), extra_env=extra_env)

    def op(f):
        return f['work'] / '.jiaclaw-skill-removals' / f['id']

    def status(f):
        result = run(f, 'remove-status', f['id'])
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    def finish(f, saved):
        return run(f, 'finish-remove', f['id'], '--if-intent-sha256', saved['intent_sha256'])

    def verify(f, service_started=False):
        saved = status(f)
        assert saved['state'] == 'completed' and saved['runtime_applied'] is False
        assert not (f['work'] / 'skills/newskill').exists()
        assert json.loads((f['work'] / 'skills.lock.json').read_text())['skills'] == []
        retired = op(f) / 'retired'
        assert retired.stat().st_ino == f['old_ino']
        for leaf in f['old']['files']:
            assert (retired / leaf['path']).read_bytes() == leaf['text'].encode()
        if not service_started: assert not (root / 'state').exists()
        assert run(f, 'policy').returncode == 0
        return saved

    def injected(f, phase, mode='stop'):
        child_env = dict(env)
        child_env['DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD'] = str(interposer)
        marker = root / (f['work'].name + '-' + phase + '.marker')
        child_env.update(JIACLAW_TEST_REMOVE_WORKSPACE=str(f['work']), JIACLAW_TEST_REMOVE_ID=f['id'],
                         JIACLAW_TEST_REMOVE_PHASE=phase, JIACLAW_TEST_REMOVE_MODE=mode,
                         JIACLAW_TEST_REMOVE_MARKER=str(marker))
        return child_env, marker

    def held(f, phase):
        child_env, marker = injected(f, phase)
        child = subprocess.Popen(command(f, 'uninstall', *remove_args(f)), stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True, env=child_env)
        try:
            deadline = time.monotonic() + 15
            while not marker.exists() and child.poll() is None and time.monotonic() < deadline: time.sleep(.01)
            assert marker.exists(), (phase, child.poll())
            wait_stopped(child, deadline)
            return child
        except BaseException:
            if child.poll() is None: child.kill()
            child.communicate(timeout=5)
            raise

    f = fixture('success')
    old_cap = os.open(f['work'] / 'skills/newskill', os.O_RDONLY | os.O_DIRECTORY)
    try:
        result = remove(f)
        assert result.returncode == 0, result.stderr
        saved = verify(f)
        fd = os.open('SKILL.md', os.O_RDONLY, dir_fd=old_cap)
        try: assert os.read(fd, 131072).decode() == f['old']['files'][0]['text']
        finally: os.close(fd)
        before = (f['work'] / 'skills.lock.json').read_bytes()
        ino = (f['work'] / 'skills.lock.json').stat().st_ino
        assert remove(f).returncode != 0
        assert finish(f, saved).returncode == 0
        assert (f['work'] / 'skills.lock.json').read_bytes() == before
        assert (f['work'] / 'skills.lock.json').stat().st_ino == ino
        new_path = f['work'] / 'skills/newskill'
        new_path.mkdir(); (new_path / 'foreign').write_text('later administrator files')
        assert finish(f, saved).returncode == 0
        assert (new_path / 'foreign').read_text() == 'later administrator files'
    finally: os.close(old_cap)
    print('PASS skill remove 1: exact retained inode/body/references, old open capability, historical receipt and no repeated removal')

    for kind in ('active', 'stale', 'old-hash', 'drift', 'extra', 'link', 'hardlink', 'fifo', 'root-link', 'mode'):
        f = fixture('reject-' + kind)
        target = f['work'] / 'skills/newskill'
        if kind == 'active':
            p = json.loads(f['policy']); p['skills'][0]['enabled'] = True
            f['policy'] = json.dumps(p); (f['work'] / 'skills.lock.json').write_text(f['policy'])
        elif kind == 'stale': f['policy'] = 'not the reviewed policy'
        elif kind == 'old-hash': f['old']['source']['skill_sha256'] = '0' * 64
        elif kind == 'drift': (target / 'references/nested/ref.md').write_text('drift')
        elif kind == 'extra': (target / 'extra.txt').write_text('unreviewed')
        elif kind == 'mode': target.chmod(0o755)
        elif kind == 'root-link':
            moved = f['work'] / 'foreign-skills'; (f['work'] / 'skills').rename(moved); (f['work'] / 'skills').symlink_to(moved)
        else:
            leaf = target / 'SKILL.md'; leaf.unlink()
            external = root / ('external-' + kind); external.write_text(f['old']['files'][0]['text'])
            if kind == 'link': leaf.symlink_to(external)
            elif kind == 'hardlink': os.link(external, leaf)
            else: os.mkfifo(leaf)
        before = (f['work'] / 'skills.lock.json').read_bytes()
        result = remove(f)
        assert result.returncode != 0 and not result.stdout.strip(), (kind, result)
        assert not (f['work'] / '.jiaclaw-skill-removals').exists()
        assert (f['work'] / 'skills.lock.json').read_bytes() == before
    print('PASS skill remove 2: active/stale/body/reference drift/extra/symlink/hardlink/FIFO/root-link/mode reject before intent')

    for phase in ('snapshot-temp', 'preparing', 'intent', 'retired', 'policy', 'receipt'):
        f = fixture('kill-' + phase)
        child = held(f, phase)
        try:
            assert run(f, 'remove-status', f['id']).returncode != 0
        finally:
            child.kill(); child.communicate(timeout=5)
        saved = status(f)
        if phase in ('snapshot-temp', 'preparing'):
            assert saved['state'] == 'preparation_incomplete' and saved['intent_sha256'] is None
            assert run(f, 'finish-remove', f['id'], '--if-intent-sha256', '0' * 64).returncode != 0
            f2 = dict(f, id=str(uuid.uuid4()))
            assert remove(f2).returncode != 0
            assert (f['work'] / 'skills.lock.json').read_text() == f['policy']
            assert (f['work'] / 'skills/newskill').stat().st_ino == f['old_ino']
        else:
            assert saved['state'] == {'intent':'prepared', 'retired':'files_retired', 'policy':'policy_published', 'receipt':'completed'}[phase]
            assert finish(f, saved).returncode == 0
            verify(f)
    print('PASS skill remove 3: six real sync/SIGKILL phases, owner settlement, incomplete evidence retained and original intent-only finish')

    for kind in ('policy', 'replacement', 'content', 'known-link', 'known-hardlink', 'known-fifo', 'temporary'):
        f = fixture('conflict-' + kind)
        child = held(f, 'retired'); child.kill(); child.communicate(timeout=5)
        saved = status(f)
        policy = f['work'] / 'skills.lock.json'
        if kind == 'policy': policy.write_text(f['policy'] + ' ')
        elif kind == 'replacement':
            target = f['work'] / 'skills/newskill'; target.mkdir(mode=0o700); (target / 'foreign').write_text('conflict')
        elif kind == 'content': (op(f) / 'retired/references/nested/ref.md').write_text('conflict')
        elif kind == 'temporary':
            leaf = op(f) / 'retired' / ('.jiaclaw-memory-' + str(uuid.uuid4())); leaf.write_text('original evidence'); leaf.chmod(0o600)
        else:
            leaf = op(f) / 'bundle.json'; raw = leaf.read_bytes(); leaf.unlink()
            external = root / ('known-' + kind); external.write_bytes(raw); external.chmod(0o600)
            if kind == 'known-link': leaf.symlink_to(external)
            elif kind == 'known-hardlink': os.link(external, leaf)
            else: os.mkfifo(leaf, 0o600)
        before = policy.read_bytes(); inode = policy.stat().st_ino
        result = finish(f, saved)
        assert result.returncode != 0 and run(f, 'remove-status', f['id']).returncode != 0
        assert policy.read_bytes() == before and policy.stat().st_ino == inode
        if kind == 'temporary': assert leaf.read_text() == 'original evidence'
    print('PASS skill remove 4: conflicting policy/namespace/content and unsafe known state reject, preserving original bytes/inodes/temporary evidence')

    for phase in ('retired', 'policy', 'receipt'):
        f = fixture('eio-' + phase)
        child_env, _ = injected(f, phase, 'error')
        result = remove(f, child_env)
        assert result.returncode != 0 and not result.stdout.strip()
        saved = status(f)
        assert saved['state'] == {'retired':'files_retired', 'policy':'policy_published', 'receipt':'completed'}[phase]
        assert finish(f, saved).returncode == 0
        verify(f)
    print('PASS skill remove 5: post-retirement/policy/receipt sync EIO can commit; inspect original identity before explicit finish')

    f = fixture('owner')
    owner = os.open(f['work'], os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        assert remove(f).returncode != 0
        assert not (f['work'] / '.jiaclaw-skill-removals').exists()
    finally: os.close(owner)
    children = [subprocess.Popen(command(f, 'uninstall', *remove_args(f)), stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, env=env) for _ in range(2)]
    for child in children: child.communicate(timeout=15)
    assert sorted(child.returncode == 0 for child in children) == [False, True]
    verify(f)
    print('PASS skill remove 6: original nonqueueing root writer and competing same UUID yield one retirement/receipt')

    f = fixture('cross-identity')
    reused = dict(f, id=f['installed_id'])
    assert remove(reused).returncode != 0
    assert not (f['work'] / '.jiaclaw-skill-removals').exists()
    removed = remove(f); assert removed.returncode == 0, removed.stderr
    saved = verify(f)
    raw = json.dumps(f['old']); (f['work'] / 'another.json').write_text(raw)
    current = (f['work'] / 'skills.lock.json').read_text()
    install_args = ['--operation-id', f['id'], '--from', 'another.json', '--if-bundle-sha256', sha(raw), '--if-manifest-sha256', sha(current)]
    assert run(f, 'install', *install_args).returncode != 0
    fresh_id = str(uuid.uuid4()); install_args[1] = fresh_id
    assert run(f, 'install', *install_args).returncode == 0
    current = (f['work'] / 'skills.lock.json').read_text()
    update_args = ['--operation-id', f['id'], '--from', 'update.json', '--if-bundle-sha256', sha(f['raw']),
                  '--if-manifest-sha256', sha(current), '--if-current-skill-sha256', f['old']['source']['skill_sha256']]
    assert run(f, 'update', *update_args).returncode != 0
    update_id = str(uuid.uuid4()); update_args[1] = update_id
    assert run(f, 'update', *update_args).returncode == 0
    current = (f['work'] / 'skills.lock.json').read_text()
    f2 = dict(f, id=update_id, policy=current, old=f['new'])
    assert remove(f2).returncode != 0
    (op(f) / 'receipt.json').unlink()
    before = (f['work'] / 'skills.lock.json').read_bytes()
    install_args[1] = str(uuid.uuid4())
    assert run(f, 'install', *install_args).returncode != 0
    assert (f['work'] / 'skills.lock.json').read_bytes() == before
    print('PASS skill remove 8: cross install/update/remove UUID reuse and unresolved removal block new effects under one writer')

    for stop_delay in (0, 400000):
        f = fixture('runtime-' + str(stop_delay))
        token = 'disposable-local-remove-bearer'
        settings = json.loads(f['config'].read_text()); settings['provider'] = {'provider_type':'stub'}
        settings.pop('model_calls'); settings.pop('memory')
        settings['http'] = {'bind':'127.0.0.1:0', 'api_token':token}; f['config'].write_text(json.dumps(settings))
        # Load the target, then disable only the disk declaration. Uninstall
        # cannot silently pretend that a running registry has been reloaded.
        enabled = json.loads(f['policy']); enabled['skills'][0]['enabled'] = True
        (f['work'] / 'skills.lock.json').write_text(json.dumps(enabled))
        log = root / ('serve-' + str(stop_delay) + '.log')
        service = None; child = None
        try:
            with log.open('wb') as output:
                service = subprocess.Popen([str(binary), 'serve', '--config', str(f['config'])], env=env, stdout=output, stderr=output)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert service.poll() is None, read_running_log(log)
                match = re.search(r'HTTP .+ (http://127\.0\.0\.1:[1-9][0-9]*)', read_running_log(log))
                if match: base = match.group(1); break
                time.sleep(.05)
            else: raise AssertionError(read_running_log(log))
            def request(path, payload=None):
                data = None if payload is None else json.dumps(payload).encode()
                with urllib.request.urlopen(urllib.request.Request(base+path,data=data,
                    headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'}),timeout=10) as response: return json.load(response)
            loaded = request('/api/skills'); assert len(loaded['skills']) == 1
            (f['work'] / 'skills.lock.json').write_text(f['policy'])
            child_env, marker = injected(f, 'retired'); child_env['JIACLAW_TEST_SYNC_STOP_DELAY_US'] = str(stop_delay)
            child = subprocess.Popen(command(f, 'uninstall', *remove_args(f)),stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env=child_env)
            deadline = time.monotonic() + 15
            while not marker.exists() and child.poll() is None and time.monotonic() < deadline: time.sleep(.01)
            assert marker.exists(); wait_stopped(child, deadline)
            assert request('/api/skills') == loaded
            request('/api/skills/reload', {})
            assert request('/api/skills')['skills'] == []
            child.send_signal(signal.SIGCONT); stdout, stderr = child.communicate(timeout=15)
            assert child.returncode == 0, (stdout,stderr)
            verify(f, service_started=True); assert request('/api/skills')['skills'] == []
        finally:
            if child is not None and child.poll() is None: child.kill(); child.communicate(timeout=5)
            if service is not None:
                service.terminate()
                try: service.wait(timeout=10)
                except subprocess.TimeoutExpired: service.kill(); service.wait(timeout=5)
                assert 'panicked at' not in log.read_text()
        print(f'PASS skill remove stop: actual SIGSTOP before one SIGCONT; marker delay {stop_delay}us')
    print('PASS skill remove 7: authenticated concurrent GET/reload, disabled missing directory is safe and runtime only changes after explicit reload')
