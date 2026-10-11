#!/usr/bin/env python3
"""Actual operator HTTP writes to disposable memory, never a provider call."""
import concurrent.futures
import fcntl
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from host_log import read_running_log
from stopped_child import wait_stopped

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
env = {k: v for k, v in os.environ.items() if not k.startswith('JIACLAW_')}
env['JIACLAW_LOG_LEVEL'] = 'info'
digest = lambda raw: hashlib.sha256(raw).hexdigest()
with tempfile.TemporaryDirectory(prefix='jiaclaw-memory-edit-') as directory:
    root = Path(directory).resolve()
    workspace = root / 'workspace'
    (workspace / 'notes').mkdir(parents=True)
    target = workspace / 'notes/private.md'
    original = 'Private 茶\r\n<img src=x onerror=alert(1)>\n'.encode()
    target.write_bytes(original)
    (workspace / 'MEMORY.md').write_text('DEFAULT MUST NOT BE USED')
    token = uuid.uuid4().hex
    config = root / 'config.json'
    settings = {'agent': {'name': 'memory-edit', 'description': 'Disposable',
        'workspace_path': str(workspace), 'max_turns': 10, 'system_instructions': ''},
        'provider': {'provider_type': 'stub'}, 'memory': {'path': 'notes/private.md'},
        'http': {'bind': '127.0.0.1:0', 'memory_review': True, 'memory_edit': True,
                 'api_token': token, 'persist': False, 'shutdown_timeout_secs': 1}}

    def start(options=None, injected=None):
        config.write_text(json.dumps(options or settings))
        log = open(root / ('serve-' + uuid.uuid4().hex + '.log'), 'wb')
        process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
            env=injected or env, stdout=log, stderr=log)
        end = time.monotonic() + 15
        while time.monotonic() < end:
            text = read_running_log(Path(log.name))
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:\d+)', text)
            if match: return process, log, match[1]
            if process.poll() is not None: raise AssertionError(text)
            time.sleep(.02)
        process.kill(); process.wait(); log.close()
        raise AssertionError('startup deadline')

    def stop(process, log):
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try: process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(); raise
        assert process.returncode == 0, read_running_log(Path(log.name))
        log.close()

    def request(base, route='/api/memory/replace', data=None, key=token, method='POST', raw=None):
        headers = {'Content-Type': 'application/json'}
        if key is not None: headers['Authorization'] = 'Bearer ' + key
        payload = raw if raw is not None else (json.dumps(data).encode() if data is not None else None)
        req = urllib.request.Request(base + route, headers=headers, data=payload, method=method)
        try: response = urllib.request.urlopen(req, timeout=5)
        except urllib.error.HTTPError as error: response = error
        with response: return response.status, response.headers, response.read()

    def body(content, old=None):
        return {'path': 'notes/private.md', 'content': content.decode(),
            'if_content_sha256': old or digest(target.read_bytes()),
            'if_replacement_sha256': digest(content)}

    def preserved(base, data=None, raw=None, key=token, code=None):
        before, inode = target.read_bytes(), target.stat().st_ino
        status, headers, result = request(base, data=data, raw=raw, key=key)
        assert status != 200 and (code is None or status == code), (status, result)
        assert headers['Cache-Control'] == 'no-store'
        assert headers['X-Content-Type-Options'] == 'nosniff'
        assert not before or before not in result
        assert target.read_bytes() == before and target.stat().st_ino == inode
        assert not list(workspace.rglob('.jiaclaw-memory-*'))

    process, log, base = start()
    if '--expect-parent-absence' in sys.argv:
        try:
            assert request(base, '/api/memory/edit-capabilities', method='GET')[0] == 404
            assert request(base, data=body(b'new'))[0] == 404
            assert target.read_bytes() == original
            print('PASS parent: operator HTTP replacement contract absent; original memory preserved')
        finally: stop(process, log)
        sys.exit(0)
    try:
        expected = {'protocol': 1, 'scope': 'standalone', 'operation': 'replace',
                    'max_bytes': 32768, 'conditions': 'path-and-two-raw-sha256'}
        assert json.loads(request(base, '/api/memory/edit-capabilities', method='GET')[2]) == expected
        for key in [None, 'wrong']:
            preserved(base, data=body(b'new'), key=key, code=401)
        corrected = 'Corrected 咖啡\r\n'.encode()
        status, headers, raw = request(base, data=body(corrected))
        assert status == 200 and json.loads(raw) == {'path':'notes/private.md',
            'previous_sha256':digest(original), 'content_sha256':digest(corrected), 'bytes_written':len(corrected)}
        assert corrected not in raw and headers['Referrer-Policy'] == 'no-referrer'
        assert target.read_bytes() == corrected and stat.S_IMODE(target.stat().st_mode) == 0o600
        print('PASS memory edit 1: independent authenticated capability and actual full dual-hash raw UTF8/CRLF publication, private receipt')

        for change in [body(corrected), body(b'new', digest(original)),
            {**body(b'new'), 'path':'MEMORY.md'}, {**body(b'new'),'if_replacement_sha256':'0'*64},
            {**body(b'new'),'if_content_sha256':'Z'*64}, {**body(b'new'),'extra':True},
            {**body(b'new'),'content':None}, body(b'x'*32769)]:
            preserved(base, data=change)
        for raw in [b'\xff', b'{"content":"one","content":"two"}', b'x'*(256*1024+1)]:
            preserved(base, raw=raw)
        fd = os.open(workspace, os.O_RDONLY)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            preserved(base, data=body(b'new'), code=409)
        finally: os.close(fd)
        print('PASS memory edit 2: stale/noop/path/hash/schema/UTF8/body-budget and actual held workspace writer reject with bytes/inode preserved')

        foreign = root / 'foreign'; foreign.write_bytes(corrected)
        for kind in ['link','hardlink','fifo','directory','missing']:
            target.unlink()
            if kind == 'link': target.symlink_to(foreign)
            elif kind == 'hardlink': os.link(foreign, target)
            elif kind == 'fifo': os.mkfifo(target)
            elif kind == 'directory': target.mkdir()
            status, headers, raw = request(base, data=body(b'new', digest(corrected)))
            assert status == 409 and headers['X-Jiaclaw-Write-Review'] == 'required'
            assert foreign.read_bytes() == corrected and not list(workspace.rglob('.jiaclaw-memory-*'))
            if kind == 'directory': target.rmdir()
            elif kind != 'missing': target.unlink()
            target.write_bytes(corrected)
        print('PASS memory edit 3: unsafe/missing configured targets never create, open special files, follow links or write ambient paths')

        shared_hash = digest(corrected)
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda text: request(base, data=body(text, shared_hash))[0], [b'first',b'second']))
        assert sorted(results) == [200,409], results
        assert target.read_bytes() in [b'first',b'second'] and not list(workspace.rglob('.jiaclaw-memory-*'))
        for content in [b'\x00'*32768, b'']:
            assert request(base, data=body(content))[0] == 200
            snapshot = json.loads(request(base, '/api/memory', method='GET')[2])
            assert snapshot['content'].encode() == content and target.read_bytes() == content
        print('PASS memory edit 4: competing reviewed original yields one publication; exact32KiB escaped text and explicit empty replacement complete')
    finally: stop(process, log)

    for configured, override in [(512, {}), (1048576, {'JIACLAW_MAX_BODY_BYTES':'512'})]:
        small = json.loads(json.dumps(settings)); small['http']['max_body_bytes'] = configured
        process, log, base = start(small, injected={**env, **override})
        try:
            payload = json.dumps(body(b'x' * 2048)).encode()
            before, inode = target.read_bytes(), target.stat().st_ino
            assert request(base, raw=payload)[0] == 413
            connection = http.client.HTTPConnection('127.0.0.1', int(base.rsplit(':', 1)[1]), timeout=5)
            try:
                connection.request('POST', '/api/memory/replace', iter([payload[:100], payload[100:]]),
                    {'Content-Type':'application/json', 'Authorization':'Bearer '+token}, encode_chunked=True)
                response = connection.getresponse()
                assert response.status == 413, (response.status, response.read())
                assert response.headers['Cache-Control'] == 'no-store'
                response.read()
            finally: connection.close()
            assert target.read_bytes() == before and target.stat().st_ino == inode
            assert not list(workspace.rglob('.jiaclaw-memory-*'))
        finally: stop(process, log)
    fallback = json.loads(json.dumps(settings)); fallback['http']['max_body_bytes'] = 0
    process, log, base = start(fallback)
    try:
        assert request(base, data=body(b'zero configured limit uses original fallback'))[0] == 200
    finally: stop(process, log)
    print('PASS memory edit configured body budget: identical oversized Content-Length and chunked requests reject before publication')
    print('PASS memory edit resolved body budget: configuration and environment limits share startup budget; zero uses original fallback')

    disabled = json.loads(json.dumps(settings)); disabled['http']['memory_edit'] = False
    process, log, base = start(disabled)
    try:
        preserved(base, data=body(b'new'), code=403)
        assert request(base, '/api/memory/edit-capabilities', method='GET')[0] == 403
        assert request(base, '/api/memory', method='GET')[0] == 200
    finally: stop(process, log)
    for change in [{'memory_review':False}, {'api_token':''}, {'gateway_channel_chat':True}]:
        bad = json.loads(json.dumps(settings)); bad['http'].update(change)
        config.write_text(json.dumps(bad))
        result = subprocess.run([str(binary),'serve','--config',str(config)],env=env,capture_output=True,timeout=8)
        assert result.returncode != 0
    print('PASS memory edit 5: read capability never grants writes, default edit gate and startup administrator/standalone/review requirements')

    library = root / ('memory-fsync.dylib' if sys.platform == 'darwin' else 'memory-fsync.so')
    subprocess.run(['cc','-dynamiclib' if sys.platform == 'darwin' else '-shared','-fPIC',
        str(Path(__file__).with_name('skill_install_fsync.c')), '-o',str(library),
        *([] if sys.platform == 'darwin' else ['-ldl'])],check=True,capture_output=True)
    inject = {**env, 'DYLD_INSERT_LIBRARIES' if sys.platform == 'darwin' else 'LD_PRELOAD':str(library),
        'JIACLAW_TEST_MEMORY_DIR':str(target.parent), 'JIACLAW_TEST_MEMORY_PHASE':'temporary',
        'JIACLAW_TEST_MEMORY_MARKER':str(root/'held-marker')}
    target.write_bytes(original)
    process, log, base = start(injected=inject)
    connection = http.client.HTTPConnection('127.0.0.1', int(base.rsplit(':',1)[1]), timeout=5)
    try:
        connection.request('POST','/api/memory/replace',json.dumps(body(b'after disconnected waiter')),
            {'Content-Type':'application/json','Authorization':'Bearer '+token})
        end = time.monotonic()+10
        while not (root/'held-marker').exists() and time.monotonic()<end: time.sleep(.01)
        assert (root/'held-marker').exists()
        wait_stopped(process, end)
        connection.close()
        config.write_text(json.dumps(settings))
        (workspace/'candidate.md').write_bytes(b'competing CLI')
        result = subprocess.run([str(binary),'memory','replace','--from','candidate.md',
            '--if-content-sha256',digest(original),'--if-replacement-sha256',digest(b'competing CLI'),
            '--config',str(config)],env=env,capture_output=True,timeout=10)
        assert result.returncode != 0 and b'busy' in result.stderr.lower(), result.stderr
        assert target.read_bytes() == original
        process.send_signal(signal.SIGCONT)
        end = time.monotonic()+8
        while target.read_bytes() == original and time.monotonic()<end: time.sleep(.01)
        assert target.read_bytes() == b'after disconnected waiter'
        assert request(base,'/api/memory',method='GET')[0] == 200
        assert not list(workspace.rglob('.jiaclaw-memory-*'))
    finally:
        connection.close()
        process.send_signal(signal.SIGCONT)
        stop(process,log)
    print('PASS memory edit 6: actual synced temporary/stop receipt and disconnected HTTP waiter retain real writer against CLI until publication settles')

    inject.update({'JIACLAW_TEST_MEMORY_PHASE':'published','JIACLAW_TEST_MEMORY_MODE':'error'})
    target.write_bytes(original)
    process, log, base = start(injected=inject)
    try:
        status, headers, raw = request(base,data=body(b'committed despite dirsync EIO'))
        assert status == 409 and headers['X-Jiaclaw-Write-Review'] == 'required'
        assert target.read_bytes() == b'committed despite dirsync EIO'
        assert json.loads(request(base,'/api/memory',method='GET')[2])['content_sha256'] == digest(target.read_bytes())
        assert not list(workspace.rglob('.jiaclaw-memory-*'))
    finally: stop(process,log)
    print('PASS memory edit 7: actual post-rename fsync EIO has unconfirmed receipt and committed bytes; explicit inspection determines current content')
