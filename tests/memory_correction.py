#!/usr/bin/env python3
"""Operator correction through the actual CLI and subsequent HTTP prompts.

All credentials and the model gateway are disposable local fixtures. This does
not certify model learning quality, semantic index deletion, or a supplier.
"""
import concurrent.futures
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import uuid
from host_log import read_running_log

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
digest = lambda raw: hashlib.sha256(raw).hexdigest()

with tempfile.TemporaryDirectory(prefix='jiaclaw-memory-correction-') as directory:
    root = Path(directory).resolve()
    workspace = root / 'workspace'
    (workspace / 'notes').mkdir(parents=True)
    target = workspace / 'notes/memory.md'
    original = 'Original preference: 茶\r\nOld experience: review source first.\n'.encode()
    target.write_bytes(original)
    (workspace / 'MEMORY.md').write_text('DEFAULT MUST NOT BE USED')
    candidate = workspace / 'reviewed.md'
    revised = 'Corrected preference: 咖啡\nConfirmed experience: inspect source first.\n'.encode()
    candidate.write_bytes(revised)
    secret, token = uuid.uuid4().hex, uuid.uuid4().hex
    config = root / 'config.json'
    settings = {'agent': {'name': 'memory-correction-fixture', 'description': 'Disposable',
        'workspace_path': str(workspace), 'max_turns': 10, 'system_instructions': '',
        'skill_lock_required': True},
        'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:1',
                     'model': 'fixture', 'api_key': secret},
        'memory': {'path': 'notes/memory.md', 'semantic': {'enabled': True, 'model': 'fixture-embed',
            'space_revision': 'correction-v1', 'dimensions': 8, 'index_path': '../state/semantic.sqlite3'}},
        'model_calls': {'enabled': True, 'store_path': '../state/model-calls/index.sqlite3'},
        'mcp': {'servers': [{'name': 'must-not-initialize',
            'endpoint': 'http://127.0.0.1:1/mcp/', 'timeout_secs': 1,
            'tools': [{'name': 'read_private', 'alias': 'read_private',
                       'descriptor_sha256': 'sha256:' + '0' * 64, 'effect': 'read_only'}]}]},
        'http': {'persist': True, 'persist_path': '../state/sessions.sqlite3', 'bind': '127.0.0.1:0',
                 'api_token': token, 'shutdown_timeout_secs': 1},
        'tools': {'memory_write': {'enabled': False}}}
    # CLI maintenance must not initialize any enabled Agent dependency.
    (workspace / 'skills').mkdir()
    (workspace / 'skills.lock.json').write_text('invalid policy must not be opened')
    (workspace / 'skills/trap').mkdir()
    os.mkfifo(workspace / 'skills/trap/SKILL.md')
    config.write_text(json.dumps(settings))

    def cli(*args, **kwargs):
        return subprocess.run([str(binary), 'memory', *args, '--config', str(config)],
                              env=env, capture_output=True, timeout=10, **kwargs)

    def inspect():
        result = cli('inspect')
        assert result.returncode == 0, (result.stdout, result.stderr)
        return json.loads(result.stdout)

    def replace(expected=None, replacement=None, path='reviewed.md', **kwargs):
        return cli('replace', '--from', path,
            '--if-content-sha256', expected or digest(target.read_bytes()),
            '--if-replacement-sha256', replacement or digest(candidate.read_bytes()), **kwargs)

    def unchanged(result, before=None, inode=None):
        assert result.returncode != 0 and not result.stdout.strip(), (result.stdout, result.stderr)
        if before is not None:
            assert target.read_bytes() == before
        if inode is not None:
            assert target.stat().st_ino == inode
        assert not list(workspace.rglob('.jiaclaw-memory-*'))

    if '--expect-parent-rejection' in sys.argv:
        unchanged(cli('inspect'), original, target.stat().st_ino)
        unchanged(replace(), original, target.stat().st_ino)
        assert not (root / 'state').exists()
        print('PASS parent: correction CLI is absent; original bytes and inode preserved')
        sys.exit(0)

    captured = inspect()
    assert captured == {'path': 'notes/memory.md', 'content': original.decode(),
                        'content_sha256': digest(original), 'bytes': len(original)}
    result = replace()
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {'path': 'notes/memory.md', 'previous_sha256': digest(original),
        'content_sha256': digest(revised), 'bytes_written': len(revised)}
    assert revised not in result.stdout and target.read_bytes() == revised
    assert stat.S_IMODE(target.stat().st_mode) == 0o600
    assert not (root / 'state').exists()
    print('PASS memory correction 1: full configured raw-byte snapshot and dual-hash disk receipt, zero dependency initialization')

    inode = target.stat().st_ino
    unchanged(replace(), revised, inode)
    unchanged(replace(digest(original)), revised, inode)
    candidate.write_text('candidate changed after review')
    unchanged(replace(replacement=digest(revised)), revised, inode)
    for args in [('replace',), ('replace', '--from', 'reviewed.md'),
                 ('replace', '--from', 'reviewed.md', '--if-content-sha256', digest(revised))]:
        unchanged(cli(*args), revised, inode)
    for old, new in [('bad', digest(revised)), (digest(revised), 'bad'), (digest(revised).upper(), digest(revised))]:
        unchanged(replace(old, new), revised, inode)
    missing_config = subprocess.run([str(binary), 'memory', 'inspect'], env=env, capture_output=True, timeout=5)
    unchanged(missing_config, revised, inode)
    print('PASS memory correction 2: stale/candidate drift/no-op and missing or malformed explicit conditions preserve bytes and inode')

    candidate.write_bytes(b'')
    assert replace(replacement=digest(b'')).returncode == 0
    assert inspect()['content'] == ''
    exact = ('汉' * 10922 + 'ab').encode()
    assert len(exact) == 32768
    candidate.write_bytes(exact)
    assert replace().returncode == 0
    assert inspect()['content'].encode() == exact
    for bad in [b'x' * 32769, b'\xff']:
        candidate.write_bytes(bad)
        unchanged(replace(), exact, target.stat().st_ino)
    target.write_bytes(b'x' * 32769)
    unchanged(cli('inspect'), b'x' * 32769, target.stat().st_ino)
    candidate.write_bytes(revised)
    unchanged(replace(), b'x' * 32769, target.stat().st_ino)
    target.write_bytes(original)
    print('PASS memory correction 3: explicitly reviewed empty replacement and exact 32KiB UTF8; oversize and invalid UTF8 reject in full')

    foreign = root / 'foreign'
    foreign.write_bytes(revised)
    for leaf in [candidate, target]:
        for kind in ['symlink', 'hardlink', 'fifo', 'directory']:
            leaf.unlink()
            if kind == 'symlink':
                leaf.symlink_to(foreign)
            elif kind == 'hardlink':
                os.link(foreign, leaf)
            elif kind == 'fifo':
                os.mkfifo(leaf)
            else:
                leaf.mkdir()
            unchanged(replace(digest(original), digest(revised)), original if leaf != target else None)
            if leaf == target:
                unchanged(cli('inspect'))
            assert foreign.read_bytes() == revised
            leaf.rmdir() if kind == 'directory' else leaf.unlink()
            leaf.write_bytes(original if leaf == target else revised)
    for raw in ['../foreign', str(foreign), 'missing/candidate']:
        unchanged(replace(path=raw), original, target.stat().st_ino)
    notes = workspace / 'notes'
    notes.rename(workspace / 'notes-saved')
    notes.symlink_to(root, target_is_directory=True)
    try:
        unchanged(cli('inspect'))
        unchanged(replace(digest(original), digest(revised)))
    finally:
        notes.unlink()
        (workspace / 'notes-saved').rename(notes)
    target.unlink()
    unchanged(cli('inspect'))
    unchanged(replace(digest(original), digest(revised)))
    assert not target.exists()
    target.write_bytes(original)
    print('PASS memory correction 4: unsafe targets and candidates, parent links, missing files and path escape refuse without creation')

    import fcntl
    owner = os.open(workspace, os.O_RDONLY)
    try:
        fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        unchanged(replace(), original, target.stat().st_ino)
        assert inspect()['content_sha256'] == digest(original)
    finally:
        os.close(owner)
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(lambda _: replace(digest(original), digest(revised)), range(2)))
    assert sum(r.returncode == 0 for r in results) == 1, [(r.stdout, r.stderr) for r in results]
    assert target.read_bytes() == revised
    assert not list(workspace.rglob('.jiaclaw-memory-*'))
    print('PASS memory correction 5: actual held root writer and two competing original-hash CLI requests admit one byte-changing commit')

    import resource
    def deny_writes():
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
        resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
    candidate.write_bytes(original)
    unchanged(replace(preexec_fn=deny_writes), revised, target.stat().st_ino)
    notes.chmod(0o500)
    try:
        unchanged(replace(), revised, target.stat().st_ino)
    finally:
        notes.chmod(0o700)
    assert not (root / 'state').exists()
    print('PASS memory correction 6: actual OS file-write and directory permission failures preserve original bytes/inode and clean staging')

    observed, errors = [], []
    class Gateway(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_POST(self):
            try:
                assert self.path == '/v1/chat/completions'
                assert self.headers['Authorization'] == 'Bearer ' + secret
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                observed.append(body)
                value = {'choices': [{'message': {'role': 'assistant', 'content': 'fixture completed'},
                                      'finish_reason': 'stop'}]}
                raw = json.dumps(value).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)
            except Exception as error:
                errors.append(repr(error))
                self.send_error(500)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    settings['provider']['base_url'] = 'http://127.0.0.1:' + str(server.server_port)
    settings['memory']['semantic']['enabled'] = False
    settings['model_calls']['enabled'] = False
    settings['mcp']['servers'] = []
    settings['agent']['skill_lock_required'] = False
    (workspace / 'skills/trap/SKILL.md').unlink()
    (workspace / 'skills.lock.json').unlink()
    config.write_text(json.dumps(settings))
    host = None
    log = root / 'serve.log'
    try:
        with log.open('wb') as output:
            host = subprocess.Popen([str(binary), 'serve', '--config', str(config)], env=env, stdout=output, stderr=output)
        end = time.monotonic() + 15
        while time.monotonic() < end:
            assert host.poll() is None, read_running_log(log)
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', read_running_log(log))
            if match:
                base = match.group(1)
                break
            time.sleep(.025)
        else:
            raise AssertionError(read_running_log(log))
        def chat(session):
            before = len(observed)
            req = urllib.request.Request(base + '/api/chat', headers={'Authorization': 'Bearer ' + token,
                'Content-Type': 'application/json'}, data=json.dumps({'session_id': session,
                'messages': [{'role': 'user', 'content': 'Observe configured memory'}],
                'enabled_tools': ['datetime_now'], 'auto_skills': False}).encode())
            with urllib.request.urlopen(req, timeout=10) as response:
                assert json.load(response)['status'] == 'completed'
            assert len(observed) == before + 1 and not errors, errors
            return next(m['content'] for m in observed[-1]['messages'] if m['role'] == 'system')
        assert revised.decode() in chat('same')
        assert replace().returncode == 0
        prompt = chat('same')
        assert original.decode() in prompt and revised.decode() not in prompt
        assert original.decode() in chat('fresh')
        candidate.write_bytes(b'')
        assert replace().returncode == 0
        prompt = chat('cleared')
        assert 'Long-term Memory' not in prompt and original.decode() not in prompt
        assert 'DEFAULT MUST NOT BE USED' not in prompt
        print('PASS memory correction 7: same running service next-turn and fresh-session prompt bytes follow correction/clear, not default path or model quality')
    finally:
        if host and host.poll() is None:
            host.terminate()
            try:
                host.wait(timeout=10)
            except subprocess.TimeoutExpired:
                host.kill()
                host.wait(timeout=5)
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive()
