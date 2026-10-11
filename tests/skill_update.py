#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Actual local update/original identity/crash interface, not supplier or power-loss certification."""
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
with tempfile.TemporaryDirectory(prefix='jiaclaw-skill-update-') as temporary:
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
        f = {'work': work, 'config': config, 'old': old, 'id': str(uuid.uuid4())}
        installed = run(f, 'install', '--operation-id', str(uuid.uuid4()), '--from', 'install.json',
            '--if-bundle-sha256', sha(raw), '--if-manifest-sha256', sha(empty))
        assert installed.returncode == 0, installed.stderr
        new = copy.deepcopy(old) if equal_bytes else bundle('new body', 'new reference\n', 'b')
        new['source']['revision'] = 'b' * 40
        f.update(new=new, raw=json.dumps(new) + '\n', policy=(work / 'skills.lock.json').read_text(),
                 old_ino=(work / 'skills/newskill').stat().st_ino)
        (work / 'update.json').write_text(f['raw'])
        return f

    def update_args(f):
        return ['--operation-id', f['id'], '--from', 'update.json', '--if-bundle-sha256', sha(f['raw']),
            '--if-manifest-sha256', sha(f['policy']), '--if-current-skill-sha256', f['old']['source']['skill_sha256']]

    def update(f, extra_env=env):
        return run(f, 'update', *update_args(f), extra_env=extra_env)

    def op(f):
        return f['work'] / '.jiaclaw-skill-updates' / f['id']

    def status(f):
        result = run(f, 'update-status', f['id'])
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    def finish(f, saved):
        return run(f, 'finish-update', f['id'], '--if-intent-sha256', saved['intent_sha256'])

    def verify(f):
        receipt = status(f)
        assert receipt['state'] == 'completed' and not receipt['runtime_applied']
        policy = json.loads((f['work'] / 'skills.lock.json').read_text())
        assert policy['skills'] == [{'directory': 'newskill', 'enabled': False, 'source': f['new']['source']}]
        current = f['work'] / 'skills/newskill'
        assert current.stat().st_ino != f['old_ino'] and (op(f) / 'stage').stat().st_ino == f['old_ino']
        for data, directory in ((f['new'], current), (f['old'], op(f) / 'stage')):
            for file in data['files']:
                assert (directory / file['path']).read_bytes() == file['text'].encode()
        assert not (root / 'state').exists()
        assert run(f, 'policy').returncode == 0
        return receipt

    for equal in (False, True):
        f = fixture('success-' + str(equal), equal)
        old_cap = os.open(f['work'] / 'skills/newskill', os.O_RDONLY | os.O_DIRECTORY)
        try:
            result = update(f)
            assert result.returncode == 0, result.stderr
            receipt = verify(f)
            held = os.open('SKILL.md', os.O_RDONLY, dir_fd=old_cap)
            try: assert os.read(held, 131072).decode() == f['old']['files'][0]['text']
            finally: os.close(held)
            inode = (f['work'] / 'skills/newskill').stat().st_ino
            policy_inode = (f['work'] / 'skills.lock.json').stat().st_ino
            assert update(f).returncode != 0
            assert finish(f, receipt).returncode == 0
            assert (f['work'] / 'skills/newskill').stat().st_ino == inode
            assert (f['work'] / 'skills.lock.json').stat().st_ino == policy_inode
        finally: os.close(old_cap)
    print('PASS skill update 1: real EXCHANGE, exact old backup/new resources, old open capability, equal-byte version, historical receipt and no second exchange')

    for kind in ('active', 'stale', 'old-hash', 'drift', 'extra', 'source', 'no-op', 'no-op-reordered', 'input-link', 'input-hardlink', 'input-fifo'):
        f = fixture('reject-' + kind)
        target = f['work'] / 'skills/newskill'
        if kind == 'active':
            p = json.loads(f['policy']); p['skills'][0]['enabled'] = True
            f['policy'] = json.dumps(p); (f['work'] / 'skills.lock.json').write_text(f['policy'])
        elif kind == 'stale': f['policy'] = 'not the reviewed policy'
        elif kind == 'old-hash': f['old']['source']['skill_sha256'] = '0' * 64
        elif kind == 'drift': (target / 'references/nested/ref.md').write_text('drift')
        elif kind == 'extra': (target / 'extra.txt').write_text('unreviewed')
        elif kind == 'source':
            f['new']['source']['repository'] = 'https://user:secret@example.com/pkg'
            f['raw'] = json.dumps(f['new']); (f['work'] / 'update.json').write_text(f['raw'])
        elif kind in ('no-op', 'no-op-reordered'):
            no_op = copy.deepcopy(f['old'])
            if kind == 'no-op-reordered': no_op['files'].reverse()
            f['raw'] = json.dumps(no_op); (f['work'] / 'update.json').write_text(f['raw'])
        elif kind.startswith('input-'):
            input_path = f['work'] / 'update.json'; input_path.unlink()
            other = root / (kind + '-external'); other.write_text(f['raw'])
            if kind == 'input-link': input_path.symlink_to(other)
            elif kind == 'input-hardlink': os.link(other, input_path)
            else: os.mkfifo(input_path)
        before = (f['work'] / 'skills.lock.json').read_bytes()
        result = update(f)
        assert result.returncode != 0 and not result.stdout.strip(), (kind, result)
        assert not (f['work'] / '.jiaclaw-skill-updates').exists()
        assert target.stat().st_ino == f['old_ino']
        assert (f['work'] / 'skills.lock.json').read_bytes() == before
    print('PASS skill update 2: active/stale/drift/extra/no-op and actual link/hardlink/FIFO refuse before journal or namespace publication')

    def injected(f, phase, error=False):
        marker = root / (f['work'].name + '-held')
        return {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(interposer),
            'JIACLAW_TEST_UPDATE_WORKSPACE': str(f['work']), 'JIACLAW_TEST_UPDATE_ID': f['id'],
            'JIACLAW_TEST_UPDATE_PHASE': phase, 'JIACLAW_TEST_UPDATE_OLD_INO': str(f['old_ino']),
            'JIACLAW_TEST_UPDATE_MARKER': str(marker), **({'JIACLAW_TEST_UPDATE_MODE': 'error'} if error else {})}, marker

    def killed(f, phase):
        child_env, marker = injected(f, phase)
        child = subprocess.Popen(command(f, 'update', *update_args(f)), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 text=True, env=child_env)
        try:
            deadline = time.monotonic() + 15
            while not marker.exists() and child.poll() is None and time.monotonic() < deadline: time.sleep(0.01)
            assert marker.exists(), (phase, child.poll(), child.communicate(timeout=1) if child.poll() is not None else 'no observed sync')
            wait_stopped(child, deadline)
            competing = dict(f, id=str(uuid.uuid4()))
            rejected = update(competing)
            assert rejected.returncode != 0 and 'lock' in rejected.stderr, rejected
            assert run(f, 'update-status', f['id']).returncode != 0
            init = subprocess.run([str(binary), 'init', '--path', str(f['work'])], capture_output=True, text=True, env=env, timeout=5)
            assert init.returncode != 0 and 'lock' in init.stderr
            child.kill(); child.communicate(timeout=5)
            assert child.returncode == -signal.SIGKILL
        finally:
            if child.poll() is None: child.kill(); child.communicate(timeout=5)
        return status(f)

    for phase, expected in (('preparing', 'preparation_incomplete'), ('stage-temp', 'preparation_incomplete'),
                            ('intent', 'prepared'), ('policy', 'policy_published'), ('files', 'files_published'), ('receipt', 'completed')):
        f = fixture('kill-' + phase, phase == 'files')
        saved = killed(f, phase)
        assert saved['state'] == expected, (phase, saved)
        competitor = dict(f, id=str(uuid.uuid4()))
        if expected == 'preparation_incomplete':
            assert update(competitor).returncode != 0
            assert run(f, 'finish-update', f['id'], '--if-intent-sha256', '0' * 64).returncode != 0
            assert (f['work'] / 'skills/newskill').stat().st_ino == f['old_ino']
            assert (f['work'] / 'skills.lock.json').read_text() == f['policy']
        else:
            if expected != 'completed': assert update(competitor).returncode != 0
            (f['work'] / 'update.json').unlink()
            result = finish(f, saved)
            assert result.returncode == 0, (phase, result.stderr)
            verify(f)
    print('PASS skill update 3: six actual sync/SIGKILL stages, held writer, incomplete capture refusal, captured-only finish and equal-byte exchange recovery')

    f = fixture('post-exchange-eio')
    child_env, _ = injected(f, 'files', True)
    result = update(f, child_env)
    assert result.returncode != 0 and not result.stdout.strip(), result
    saved = status(f)
    assert saved['state'] == 'files_published'
    assert finish(f, saved).returncode == 0
    verify(f)
    print('PASS skill update 4: actual post-exchange sync EIO may be committed, original UUID inspection then explicit finish without replay')

    for kind in ('policy', 'directory-identity', 'stage-content', 'known-link'):
        f = fixture('conflict-' + kind)
        saved = killed(f, 'intent')
        evidence = op(f) / ('.jiaclaw-memory-' + str(uuid.uuid4()))
        evidence.write_text('original synced evidence'); evidence.chmod(0o600)
        before = (evidence.read_bytes(), evidence.stat().st_ino)
        if kind == 'policy': (f['work'] / 'skills.lock.json').write_text('{"version":2,"skills":[]}\n')
        elif kind == 'directory-identity':
            original = f['work'] / 'skills/newskill'; held = f['work'] / 'held-original'
            original.rename(held); original.mkdir(mode=0o700)
            for file in f['old']['files']:
                path = original / file['path']; path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                path.write_text(file['text']); path.chmod(0o600)
        elif kind == 'stage-content': (op(f) / 'stage/SKILL.md').write_text('drift')
        else:
            path = op(f) / 'new-bundle.json'; path.unlink(); path.symlink_to(f['work'] / 'update.json')
        failed = finish(f, saved)
        assert failed.returncode != 0 and not failed.stdout.strip()
        assert (evidence.read_bytes(), evidence.stat().st_ino) == before
        assert (f['work'] / 'skills/newskill').stat().st_ino == f['old_ino'] or kind == 'directory-identity'
    print('PASS skill update 5: original policy/directory identity/stage/known-file conflicts refuse and retain original evidence bytes/inode')

    f = fixture('cross-ledger-block')
    saved = killed(f, 'policy')
    another = copy.deepcopy(f['new']); another['directory'] = 'another'
    raw = json.dumps(another); (f['work'] / 'another.json').write_text(raw)
    current_policy = (f['work'] / 'skills.lock.json').read_text()
    refused = run(f, 'install', '--operation-id', str(uuid.uuid4()), '--from', 'another.json',
                  '--if-bundle-sha256', sha(raw), '--if-manifest-sha256', sha(current_policy))
    assert refused.returncode != 0
    assert finish(f, saved).returncode == 0
    result = run(f, 'install', '--operation-id', str(uuid.uuid4()), '--from', 'another.json',
                 '--if-bundle-sha256', sha(raw), '--if-manifest-sha256', sha((f['work'] / 'skills.lock.json').read_text()))
    assert result.returncode == 0, result.stderr
    print('PASS skill update 6: unfinished update blocks original installer under the same writer, explicit settlement permits a separate later operation')

    f = fixture('cross-kind-identity')
    install_ids = list((f['work'] / '.jiaclaw-skill-installs').iterdir())
    assert len(install_ids) == 1
    reused = dict(f, id=install_ids[0].name)
    before_policy = (f['work'] / 'skills.lock.json').read_bytes()
    assert update(reused).returncode != 0
    assert not (f['work'] / '.jiaclaw-skill-updates').exists()
    assert (f['work'] / 'skills.lock.json').read_bytes() == before_policy
    assert update(f).returncode == 0
    another = copy.deepcopy(f['new']); another['directory'] = 'another'
    raw = json.dumps(another); (f['work'] / 'another.json').write_text(raw)
    current_policy = (f['work'] / 'skills.lock.json').read_text()
    rejected = run(f, 'install', '--operation-id', f['id'], '--from', 'another.json',
        '--if-bundle-sha256', sha(raw), '--if-manifest-sha256', sha(current_policy))
    assert rejected.returncode != 0 and not rejected.stdout.strip()
    assert not (f['work'] / 'skills/another').exists()
    assert (f['work'] / 'skills.lock.json').read_text() == current_policy
    print('PASS skill update 8: completed install/update UUIDs cannot start another kind of operation; original policy and namespaces unchanged')

    for stop_delay in (0, 400000):
        f = fixture('runtime-' + str(stop_delay))
        token = 'disposable-local-update-bearer'
        settings = json.loads(f['config'].read_text())
        settings['provider'] = {'provider_type': 'stub'}
        settings.pop('model_calls'); settings.pop('memory')
        settings['http'] = {'bind': '127.0.0.1:0', 'api_token': token}
        f['config'].write_text(json.dumps(settings))
        log = root / 'serve.log'
        service = None
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
                with urllib.request.urlopen(urllib.request.Request(base + path, data=data,
                    headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}), timeout=10) as response:
                    return json.load(response)
            empty = request('/api/skills')
            assert empty['skills'] == []
            child_env, marker = injected(f, 'policy')
            child_env['JIACLAW_TEST_SYNC_STOP_DELAY_US'] = str(stop_delay)
            child = subprocess.Popen(command(f, 'update', *update_args(f)), stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, env=child_env)
            try:
                deadline = time.monotonic() + 15
                while not marker.exists() and child.poll() is None and time.monotonic() < deadline: time.sleep(.01)
                assert marker.exists()
                wait_stopped(child, deadline)
                assert (f['work'] / 'skills/newskill').stat().st_ino == f['old_ino']
                assert request('/api/skills') == empty
                request('/api/skills/reload', {})
                assert request('/api/skills') == empty
                child.send_signal(signal.SIGCONT)
                stdout, stderr = child.communicate(timeout=15)
                assert child.returncode == 0, (stdout, stderr)
            finally:
                if child.poll() is None: child.kill(); child.communicate(timeout=5)
            assert request('/api/skills') == empty
            request('/api/skills/reload', {})
            assert request('/api/skills') == empty
            enabled = run(f, 'set-enabled', 'newskill', '--enabled', 'true',
                          '--if-manifest-sha256', sha((f['work'] / 'skills.lock.json').read_text()))
            assert enabled.returncode == 0 and json.loads(enabled.stdout)['runtime_applied'] is False
            assert request('/api/skills') == empty
            request('/api/skills/reload', {})
            loaded = request('/api/skills')['skills'][0]
            assert loaded['name'] == 'newskill' and loaded['source'] == f['new']['source'], loaded
        finally:
            if service is not None:
                service.terminate()
                try: service.wait(timeout=10)
                except subprocess.TimeoutExpired: service.kill(); service.wait(timeout=5)
                assert 'panicked at' not in log.read_text()
        print(f'PASS skill update stop: actual SIGSTOP observed before one SIGCONT; marker delay {stop_delay}us')
    print('PASS skill update 7: authenticated concurrent GET/reload keeps disabled registry; independent enable/reload applies the captured new source')
