#!/usr/bin/env python3
"""Three real workbenches: explicit original receipt comparison, no model rerun."""
import json
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
import http_turns as h
import tenant_http_browser as tenant
import tenant_http_fixture as f

native = app = None
try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-task-result-compare-') as temporary:
        root = Path(temporary)
        for mode in ['standalone', 'tenant_json', 'tenant_preview']:
            if mode == 'standalone':
                native = h.App(root, mode, timeout=15)
                native.settings['provider'].update(base_url=f'http://127.0.0.1:{f.model.server_port}', api_key=f.keys['alice'])
                native.save(); native.start()
                cfg = {'base': f'http://127.0.0.1:{native.port}', 'token': h.api_token}
            else:
                directory = root / mode; directory.mkdir()
                app = tenant.App(directory)
                cfg = {'base': f'http://127.0.0.1:{app.gp}', 'token': app.users['alice']['token'], 'readonly': app.readonly['token'], 'bob': app.users['bob']['token']}
            replies = ['\ufeffbefore\r\n\t<img src=x onerror=alert(1)>\0🦀' + 'a' * (1900 * 1024), '']
            ids = []
            for index, reply in enumerate(replies):
                prompt = 'compare-' + mode + '-' + str(index)
                f.reply_plans[('alice', prompt)] = reply
                identity = str(uuid.uuid4())
                body = {'session_id': 'http:' + str(uuid.uuid4()), 'prompt': prompt, 'enabled_tools': ['file_write'] if native else ['datetime_now']}
                port = native.port if native else app.gp
                code, value, _ = f.request(port, '/api/turns/' + identity, cfg['token'], 'PUT', body)
                assert code == 202, (code, value)
                def done():
                    status, value, _ = f.request(port, '/api/turns/' + identity, cfg['token'])
                    assert status == 200
                    return value if value['receipt']['state'] == 'completed' and not value['active'] else None
                if app:
                    deadline = time.monotonic() + 15
                    f.wait(lambda: not f.sql(app.registry,
                           'SELECT request_id,state FROM write_holds WHERE user_id=?',
                           (app.users['alice']['user_id'],)),
                           'original comparison source gateway settlement',
                           seconds=max(0, deadline - time.monotonic()))
                    assert time.monotonic() < deadline, 'comparison source settlement exceeded original deadline'
                    result = f.wait(done, 'real completed comparison source',
                                    seconds=max(0, deadline - time.monotonic()))
                    elapsed = time.monotonic()
                    assert elapsed < deadline, 'comparison source receipt exceeded original deadline'
                    with f.lock:
                        model_posts = len([p for p in f.posts if p['who'] == 'alice' and p['prompt'] == prompt])
                    assert model_posts == 1
                    print('ACTUAL comparison source settlement ' + json.dumps({
                        'protocol': 1, 'mode': mode, 'index': index, 'id': identity,
                        'gateway_hold_empty': True, 'before_client_receipt': True,
                        'deadline_ms': 15000, 'elapsed_ms': int((elapsed - deadline + 15) * 1000),
                        'model_posts': model_posts}), flush=True)
                else:
                    result = f.wait(done, 'real completed comparison source')
                assert result['receipt']['result']['reply'] == reply
                assert result['receipt']['result']['tool_names'] == []
                ids.append(identity)
            cfg.update(mode=mode, ids=ids, replies=replies)
            path = root / (mode + '.json'); path.write_text(json.dumps(cfg))
            result = subprocess.run(['node', 'tests/task_result_compare_browser.cjs', str(path)], timeout=160)
            assert result.returncode == 0, (mode, result.returncode)
            assert not f.faults, f.faults
            with f.lock:
                posts = [p for p in f.posts if p['prompt'].startswith('compare-' + mode + '-')]
            assert len(posts) == 2 and len({p['operation'] for p in posts}) == 2
            if native: native.stop(); native = None
            if app:
                f.stop(app.process)
                for backend in app.backends.values(): f.stop(backend['process'])
                app = None
        assert not h.faults, h.faults
        print('ALL TASK RESULT COMPARISON BROWSER GROUPS PASS', flush=True)
finally:
    for process in reversed(h.processes + f.processes):
        if process.poll() is None: process.terminate(); process.wait(timeout=8)
    h.server.shutdown(); f.model.shutdown()
