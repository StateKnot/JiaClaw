#!/usr/bin/env python3
"""Real CLI and authenticated HTTP skill discovery/reload boundaries; synthetic local files only."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/jiaclaw').resolve()
with tempfile.TemporaryDirectory(prefix='jiaclaw-skills-') as temporary:
    root = Path(temporary)
    workspace = root / 'workspace'
    skills = workspace / 'skills'
    alpha = skills / 'alpha'
    alpha.mkdir(parents=True)
    sentinel = 'SYNTHETIC_OUTSIDE_SKILL_SENTINEL'
    original = '---\nname: alpha\ndescription: original\ntriggers: [hello]\n---\nNORMAL_SKILL_BODY\n'
    (alpha / 'SKILL.md').write_text(original)
    external = root / 'external'
    external.mkdir()
    (external / 'SKILL.md').write_text('---\nname: escaped\ndescription: fixture\n---\n' + sentinel)
    token = uuid.uuid4().hex
    config = root / 'config.json'
    config.write_text(json.dumps({
        'agent': {'name': 'skill-fixture', 'description': 'Disposable local fixture',
                  'system_instructions': '', 'max_turns': 1, 'workspace_path': str(workspace)},
        'provider': {'provider_type': 'stub'},
        'http': {'bind': '127.0.0.1:0', 'api_token': token}
    }))
    env = {key: value for key, value in os.environ.items() if not key.startswith('JIACLAW_')}
    env['JIACLAW_LOG_LEVEL'] = 'info'
    process = None
    base = None

    def cli(reload=False):
        return subprocess.run([str(binary), 'skills', *(['reload'] if reload else []),
                               '--config', str(config), *([] if reload else ['--verbose'])],
                              capture_output=True, text=True, timeout=5, env=env)

    def request(path, method='GET', authenticated=True):
        headers = {'Authorization': 'Bearer ' + token} if authenticated else {}
        try:
            with urllib.request.urlopen(urllib.request.Request(base + path, method=method,
                                                               headers=headers), timeout=5) as response:
                return response.status, json.loads(response.read())
        except urllib.error.HTTPError as error:
            return error.code, json.loads(error.read())

    try:
        control = cli()
        assert control.returncode == 0 and 'NORMAL_SKILL_BODY' in control.stdout, control.stderr
        log = root / 'server.log'
        with log.open('wb') as output:
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config)],
                                       stdout=output, stderr=output, env=env)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert process.poll() is None, log.read_text()
            match = re.search(r'HTTP 服务已启动于 (http://127\.0\.0\.1:[1-9][0-9]*)', log.read_text())
            if match:
                base = match.group(1)
                break
            time.sleep(.05)
        assert base, log.read_text()
        assert request('/api/skills', authenticated=False)[0] == 401
        assert request('/api/skills/reload', 'POST', authenticated=False)[0] == 401
        initial = request('/api/skills')[1]
        assert initial['skills'][0]['name'] == 'alpha'

        # Ordinary folders without a SKILL.md are not skill metadata candidates.
        for name in [' empty', 'x' * 129]:
            empty = skills / name
            empty.mkdir()
            assert cli(reload=True).returncode == 0, name
            assert request('/api/skills/reload', 'POST')[0] == 200, name
            empty.rmdir()

        # Changes beside a rejected skill must not be partially published.
        (alpha / 'SKILL.md').write_text(original.replace('original', 'updated'))
        bad = skills / 'bad'
        cases = ['oversize', 'invalid-utf8', 'empty-trigger', 'large-description', 'large-name']
        if os.name == 'posix':
            cases += ['leaf-link', 'directory-link', 'dangling-link', 'hardlink', 'fifo']
        for case in cases:
            if case == 'directory-link':
                bad.symlink_to(external, target_is_directory=True)
            else:
                bad.mkdir()
                leaf = bad / 'SKILL.md'
                if case == 'leaf-link':
                    leaf.symlink_to(external / 'SKILL.md')
                elif case == 'dangling-link':
                    leaf.symlink_to(root / 'absent')
                elif case == 'hardlink':
                    os.link(external / 'SKILL.md', leaf)
                elif case == 'fifo':
                    os.mkfifo(leaf)
                elif case == 'oversize':
                    leaf.write_bytes(b'x' * (128 * 1024 + 1))
                elif case == 'invalid-utf8':
                    leaf.write_bytes(b'\xff')
                elif case == 'empty-trigger':
                    leaf.write_text('---\nname: bad\ntriggers: [""]\n---\nbody')
                elif case == 'large-name':
                    leaf.write_text('---\nname: ' + 'x' * 129 + '\n---\nbody')
                else:
                    leaf.write_text('---\nname: bad\ndescription: ' + 'x' * 2049 + '\n---\nbody')
            loose = cli()
            assert loose.returncode == 0 and sentinel not in loose.stdout, (case, loose.stdout, loose.stderr)
            assert '✅ 发现 1 个技能' in loose.stdout, (case, loose.stdout[:1000])
            strict = cli(reload=True)
            assert strict.returncode != 0 and sentinel not in strict.stdout, (case, strict.stdout)
            status, body = request('/api/skills/reload', 'POST')
            assert status == 400 and '已保留旧表' in body['error'], (case, status, body)
            assert request('/api/skills')[1] == initial, case
            if bad.is_symlink():
                bad.unlink()
            else:
                (bad / 'SKILL.md').unlink()
                bad.rmdir()
            print('PASS: unsafe skill rejected without partial reload: ' + case)

        # Duplicate declared names are a catalog error, not ambiguous prompt selection.
        bad.mkdir()
        (bad / 'SKILL.md').write_text(original)
        assert cli().returncode != 0
        assert cli(reload=True).returncode != 0
        assert request('/api/skills/reload', 'POST')[0] == 400
        assert request('/api/skills')[1] == initial
        (bad / 'SKILL.md').unlink()
        bad.rmdir()

        if os.name == 'posix':
            retained = workspace / 'retained-skills'
            skills.rename(retained)
            for target in [external, root / 'missing-skills']:
                skills.symlink_to(target, target_is_directory=True)
                assert cli().returncode != 0
                assert cli(reload=True).returncode != 0
                assert request('/api/skills/reload', 'POST')[0] == 400
                assert request('/api/skills')[1] == initial
                skills.unlink()
            retained.rename(skills)
            print('PASS: linked and dangling skill roots rejected')

        assert request('/api/skills/reload', 'POST')[0] == 200
        assert request('/api/skills')[1]['skills'][0]['description'] == 'updated'
        (alpha / 'SKILL.md').unlink()
        alpha.rmdir()
        skills.rmdir()
        assert cli(reload=True).returncode == 0
        assert request('/api/skills/reload', 'POST')[1]['reloaded'] == 0
        assert request('/api/skills')[1]['skills'] == []
        print('PASS: authenticated atomic reload, successful update and missing-root clearing')
    finally:
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
