#!/usr/bin/env python3
"""Real local check CLI/ownership; synthetic refresh setup is not vendor quality."""
import copy
from contextlib import closing
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from stopped_child import wait_stopped

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
parent_mode = '--frozen-parent' in sys.argv
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'off'
secret = 'check-fixture-' + uuid.uuid4().hex
requests, failures = [], []
mutex = threading.Lock()
behavior = None
post_held = threading.Event()
release_post = threading.Event()


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        with mutex:
            requests.append(('GET', self.path))
        self.send_error(500)

    def do_POST(self):
        global behavior
        try:
            with mutex:
                requests.append(('POST', self.path))
            assert self.path == '/v1/embeddings', self.path
            assert self.headers['Authorization'] == 'Bearer ' + secret
            assert self.headers['Idempotency-Key']
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert body['model'] == 'fixture-embedding' and body['dimensions'] == 2
            assert body['encoding_format'] == 'float' and 1 <= len(body['input']) <= 8
            if behavior == 'hold':
                behavior = None
                post_held.set()
                assert release_post.wait(8), 'original submitted child was not settled'
                self.close_connection = True
                return
            if behavior == 'disconnect':
                behavior = None
                self.close_connection = True
                self.connection.shutdown(socket.SHUT_RDWR)
                self.connection.close()
                return
            payload = json.dumps({'object': 'list', 'model': body['model'],
                'data': [{'object': 'embedding', 'index': i, 'embedding': [1.0, 0.0]}
                         for i in range(len(body['input']))],
                'usage': {'prompt_tokens': len(body['input']), 'total_tokens': len(body['input'])}}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.send_header('x-brokerrouter-request-id', str(uuid.uuid4()))
            self.end_headers()
            self.wfile.write(payload)
        except Exception as error:
            with mutex:
                failures.append(repr(error))
            self.send_error(500)


gateway = ThreadingHTTPServer(('127.0.0.1', 0), Gateway)
gateway.daemon_threads = True
threading.Thread(target=gateway.serve_forever, daemon=True).start()


def count():
    with mutex:
        assert not failures, failures
        return len(requests)


def run(*args, injected=None):
    r = subprocess.run([str(binary), *map(str, args)], env=injected or env,
                       capture_output=True, text=True, timeout=25)
    assert secret not in r.stdout + r.stderr
    return r


def cli(case, action, fail=None):
    r = run('memory', 'semantic', action, '--config', case[2])
    if fail is not None:
        assert r.returncode != 0, (action, r.stdout, r.stderr)
        assert fail in r.stdout + r.stderr, (fail, r.stdout, r.stderr)
        return r
    assert r.returncode == 0, (action, r.stdout, r.stderr)
    return json.loads(r.stdout)


def check(case, freshness=None, ready=None, fail=None):
    before = count()
    result = cli(case, 'check', fail=fail)
    assert count() == before, 'local check made a network request'
    if fail is None:
        assert result['protocol_version'] == 1
        assert result['freshness'] == freshness and result['search_ready_at_capture'] is ready
        assert result['local_only'] is True and result['runtime_applied'] is False
        assert not {'text', 'excerpt', 'vectors', 'credential', 'request_body'} & result.keys()
        assert 'private-source-text' not in json.dumps(result)
    return result


def fixture(root, name):
    d = root / name; d.mkdir(mode=0o700)
    w = d / 'workspace'; w.mkdir(mode=0o700)
    (w / 'MEMORY.md').write_text('private-source-text tea\n')
    config = {'agent': {'name': 'check-fixture', 'description': 'Local protocol fixture',
        'system_instructions': 'Local fixture only.', 'max_turns': 10,
        'workspace_path': str(w), 'skill_lock_required': True},
        'provider': {'provider_type': 'brokerrouter', 'base_url': 'http://127.0.0.1:' + str(gateway.server_port),
            'api_key': secret, 'model': 'must-not-chat'},
        'memory': {'path': 'MEMORY.md', 'semantic': {'enabled': True, 'model': 'fixture-embedding',
            'space_revision': 'fixture-v1', 'dimensions': 2, 'timeout_secs': 2,
            'index_path': '../state/semantic/index.sqlite3'}},
        'tools': {'memory_write': {'enabled': False}},
        'mcp': {'servers': [{'name': 'must-not-initialize',
            'endpoint': 'http://127.0.0.1:' + str(gateway.server_port) + '/mcp/',
            'timeout_secs': 1,
            'tools': [{'name': 'read_private', 'alias': 'read_private',
                'descriptor_sha256': 'sha256:' + '0' * 64, 'effect': 'read_only'}]}]}}
    (w / 'skills').mkdir(); (w / 'skills/trap').mkdir()
    os.mkfifo(w / 'skills/trap/SKILL.md')
    p = d / 'config.json'; p.write_text(json.dumps(config))
    return d, w, p, config


def save(case, config):
    case[2].write_text(json.dumps(config))


def rows(case):
    path = case[0] / 'state/semantic/index.sqlite3'
    # Consume the original semantic_memory.py observer contract: a separate
    # query-only connection with ordinary WAL lifecycle, never immutable mode.
    script = """import json, pathlib, sqlite3, sys
db = sqlite3.connect(pathlib.Path(sys.argv[1]).as_uri() + '?mode=rw', uri=True, timeout=2)
try:
    db.execute('PRAGMA query_only=ON')
    assert db.execute('PRAGMA query_only').fetchone() == (1,)
    rows = [[list(row) for row in db.execute('SELECT * FROM ' + table + ' ORDER BY 1')]
            for table in ('semantic_meta', 'generation', 'operations')]
    print(json.dumps(rows, default=lambda value: {'blob_hex': value.hex()}))
finally:
    db.close()
"""
    result = subprocess.run([sys.executable, '-c', script, str(path)], env=env,
                            capture_output=True, text=True, timeout=5)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


with tempfile.TemporaryDirectory(prefix='jiaclaw-semantic-check-') as directory:
    root = Path(directory).resolve()
    f = fixture(root, 'main')
    if parent_mode:
        r = run('memory', 'semantic', 'check', '--config', f[2])
        assert r.returncode == 2 and 'unrecognized subcommand' in r.stderr
        assert not (f[0] / 'state').exists() and count() == 0
        print('PASS frozen parent semantic check absent: exit2, no state or network')
        gateway.shutdown(); gateway.server_close()
        sys.exit(0)

    r = run('memory', 'semantic', 'check')
    assert r.returncode != 0 and 'requires explicit --config' in r.stderr and count() == 0
    disabled = copy.deepcopy(f[3]); disabled['memory']['semantic']['enabled'] = False
    save(f, disabled); cli(f, 'check', fail='semantic_memory_disabled')
    assert not (f[0] / 'state').exists() and count() == 0
    save(f, f[3]); check(f, 'missing', False)
    assert count() == 0 and not (f[0] / 'state/sessions.sqlite3').exists()
    print('PASS semantic check 1: explicit config, opt-in, Agent initialization traps and no-network missing index')

    cli(f, 'refresh')  # Explicit local synthetic setup, never part of check.
    before = rows(f); check(f, 'current', True); assert rows(f) == before
    source = f[1] / 'MEMORY.md'; old = source.read_bytes(); meta = source.stat()
    source.write_bytes(old.replace(b'tea', b'new')); os.utime(source, ns=(meta.st_atime_ns, meta.st_mtime_ns))
    assert source.stat().st_size == meta.st_size and source.stat().st_mtime_ns == meta.st_mtime_ns
    check(f, 'stale', False); assert rows(f) == before
    original_sources = copy.deepcopy(f[3]); original_sources['memory']['semantic']['sources'] = ['MEMORY.md', 'missing.md']
    save(f, original_sources); check(f, 'stale', False); assert rows(f) == before
    save(f, f[3]); changed_space = copy.deepcopy(f[3]); changed_space['memory']['semantic']['space_revision'] = 'fixture-v2'
    save(f, changed_space); check(f, 'space_mismatch', False); assert rows(f) == before
    save(f, f[3]); source.write_bytes(old); check(f, 'current', True)
    print('PASS semantic check 2: complete raw same-size/mtime changes, allowlist/space changes and generation/receipt preservation')

    source.unlink(); check(f, 'stale', False)
    source.write_bytes(b''); check(f, 'stale', False)
    cli(f, 'refresh'); check(f, 'current', True)
    source.unlink(); check(f, 'stale', False); cli(f, 'refresh'); check(f, 'current', True)
    large = fixture(root, 'large'); large_config = copy.deepcopy(large[3])
    large_config['memory']['semantic']['sources'] = ['MEMORY.md', 'two.md', 'three.md']
    save(large, large_config)
    for name in large_config['memory']['semantic']['sources']:
        (large[1] / name).write_bytes((chr(0x1f9e0) * (128 * 1024 // 4)).encode())
    assert check(large, 'missing', False)['source_chunks'] == 384
    for name in large_config['memory']['semantic']['sources']:
        (large[1] / name).write_bytes((b'x' * 512 + b'\n') * 255)
    check(large, fail='chunk budget exceeded')
    print('PASS semantic check 3: empty versus missing manifests, exact UTF8 128KiB x3 and cumulative chunk limits')

    unsafe = fixture(root, 'unsafe'); check(unsafe, 'missing', False)
    target = unsafe[1] / 'MEMORY.md'; saved = rows(unsafe)
    for kind in ('symlink', 'hardlink', 'fifo', 'directory', 'invalid-utf8', 'oversized'):
        target.unlink()
        external = unsafe[0] / 'external'; external.write_bytes(b'private-source-text\n')
        if kind == 'symlink': target.symlink_to(external)
        elif kind == 'hardlink': os.link(external, target)
        elif kind == 'fifo': os.mkfifo(target)
        elif kind == 'directory': target.mkdir()
        elif kind == 'invalid-utf8': target.write_bytes(b'\xff')
        else: target.write_bytes(b'x' * (128 * 1024 + 1))
        check(unsafe, fail='memory file:')
        assert rows(unsafe) == saved and external.read_bytes() == b'private-source-text\n'
        if kind == 'directory': target.rmdir(); target.write_bytes(b'safe')
    assert cli(unsafe, 'status')['generation'] is None  # Original metadata diagnostic remains available.
    print('PASS semantic check 4: unsafe/link/special/UTF8/budget source errors preserve original DB and status contract')

    held = fixture(root, 'held'); check(held, 'missing', False)
    writer = os.open(held[1], os.O_RDONLY); fcntl.flock(writer, fcntl.LOCK_EX | fcntl.LOCK_NB)
    before = rows(held)
    try: check(held, fail='busy')
    finally: fcntl.flock(writer, fcntl.LOCK_UN); os.close(writer)
    assert rows(held) == before; check(held, 'missing', False)
    library = root / ('check-read.dylib' if sys.platform == 'darwin' else 'check-read.so')
    subprocess.run(['cc', '-dynamiclib' if sys.platform == 'darwin' else '-shared', '-fPIC',
        str(Path(__file__).with_name('semantic_check_read.c')), '-o', str(library),
        *([] if sys.platform == 'darwin' else ['-ldl'])], check=True, capture_output=True)
    stopped_receipts = []
    for terminate in (False, True):
        marker = root / ('check-stop-' + str(terminate))
        injected = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD': str(library),
            'JIACLAW_TEST_CHECK_SOURCE': str(held[1] / 'MEMORY.md'), 'JIACLAW_TEST_CHECK_MARKER': str(marker)}
        process = subprocess.Popen([str(binary), 'memory', 'semantic', 'check', '--config', str(held[2])],
            env=injected, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            wait_stopped(process, time.monotonic() + 10)
            assert marker.read_bytes() == b'read-held\n'
            probe = os.open(held[1], os.O_RDONLY)
            try:
                try: fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError: pass
                else: raise AssertionError('actual capture released original workspace writer')
            finally: os.close(probe)
            check(held, fail='another process owns semantic state')
            assert rows(held) == before
            if terminate: process.kill()
            else: process.send_signal(signal.SIGCONT)
            out, err = process.communicate(timeout=8)
            assert process.returncode == (-signal.SIGKILL if terminate else 0), (out, err)
            if not terminate: assert json.loads(out)['freshness'] == 'missing'
            stopped_receipts.append({'kernel_sigstop': True, 'kill': terminate, 'exit': process.returncode})
        finally:
            if process.poll() is None: process.kill(); process.communicate(timeout=5)
        assert rows(held) == before; check(held, 'missing', False)
    print('ACTUAL semantic check read owner receipts ' + json.dumps(stopped_receipts))
    print('PASS semantic check 5: actual original root/store owners across stopped read, resume/SIGKILL and two-process rejection')

    pending = fixture(root, 'pending'); cli(pending, 'refresh')
    (pending[1] / 'MEMORY.md').write_text('different private-source-text\n')
    behavior = 'disconnect'; cli(pending, 'refresh', fail='needs_review')
    status = cli(pending, 'status'); preserved = rows(pending)
    result = check(pending, 'stale', False)
    assert result['pending'] == status['pending'] and result['next_action'] == 'reconcile_pending'
    assert rows(pending) == preserved
    interrupted = fixture(root, 'interrupted'); cli(interrupted, 'refresh')
    (interrupted[1] / 'MEMORY.md').write_text('changed private-source-text\n')
    hold_config = copy.deepcopy(interrupted[3]); hold_config['memory']['semantic']['timeout_secs'] = 10
    save(interrupted, hold_config)
    behavior = 'hold'
    child = subprocess.Popen([str(binary), 'memory', 'semantic', 'refresh', '--config', str(interrupted[2])],
                             env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        assert post_held.wait(5), 'setup refresh did not actually submit its original POST'
        captured = rows(interrupted)
        assert captured[2][-1][8] == 'pending', captured[2][-1]
        child.kill(); child.communicate(timeout=8); assert child.returncode == -signal.SIGKILL
    finally:
        if child.poll() is None: child.kill(); child.communicate(timeout=5)
        release_post.set()
    assert rows(interrupted) == captured
    result = check(interrupted, 'stale', False)
    assert result['pending']['state'] == 'pending' and result['pending']['id'] == captured[2][-1][1]
    assert result['next_action'] == 'reconcile_pending' and rows(interrupted) == captured
    # An explicit ordinary operational startup retains its existing normalization.
    assert cli(interrupted, 'status')['pending']['state'] == 'unknown'
    print('PASS semantic check interrupted pending: submitted POST SIGKILL original row preserved; ordinary status still normalizes unknown')
    print('PASS semantic check 6: actual submitted unknown hold preserved with original identity, no POST/GET/reconcile')

    corrupt = fixture(root, 'corrupt'); cli(corrupt, 'refresh')
    dbpath = corrupt[0] / 'state/semantic/index.sqlite3'
    with closing(sqlite3.connect(dbpath)) as db:
        blob = bytearray(db.execute('SELECT body FROM generation').fetchone()[0])
        # One 2D vector is the final eight bytes of this existing binary format.
        blob[-8:] = struct.pack('<ff', 0.25, 0.75)
        db.execute('UPDATE generation SET body=?', (bytes(blob),))
        db.commit()
    preserved = rows(corrupt); check(corrupt, fail='index_corrupt'); assert rows(corrupt) == preserved
    print('PASS semantic check 7: matching-manifest invalid vector refuses readiness without mutating evidence or requesting query')
    count()
gateway.shutdown(); gateway.server_close()
print('ALL SEMANTIC CHECK GROUPS PASS')
