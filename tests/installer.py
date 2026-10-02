#!/usr/bin/env python3
"""Installer acceptance with a local release fixture; never downloads real assets."""
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

installer = Path(__file__).resolve().parents[1] / 'scripts/install.sh'
with tempfile.TemporaryDirectory(prefix='jiaclaw-install-') as directory:
    root = Path(directory)
    tools = root / 'tools'; tools.mkdir()
    release = root / 'release'; release.mkdir()
    install = root / 'bin'
    target = {('Linux', 'x86_64'): 'x86_64-unknown-linux-gnu', ('Linux', 'aarch64'): 'aarch64-unknown-linux-gnu', ('Darwin', 'x86_64'): 'x86_64-apple-darwin', ('Darwin', 'arm64'): 'aarch64-apple-darwin'}[(os.uname().sysname, os.uname().machine)]
    binary = release / 'jiaclaw'
    binary.write_text('#!/bin/sh\nprintf "JiaClaw fixture\\n"\n'); binary.chmod(0o755)
    asset = release / ('jiaclaw-v0.1.0-' + target + '.tar.gz')
    with tarfile.open(asset, 'w:gz') as archive: archive.add(binary, arcname='jiaclaw')
    checksum = hashlib.sha256(asset.read_bytes()).hexdigest()
    sums = release / 'SHA256SUMS'; sums.write_text(checksum + '  ' + asset.name + '\n')
    curl = tools / 'curl'
    curl.write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args=sys.argv[1:]
url=next(arg for arg in args if arg.startswith('https://'))
assert url.startswith('https://github.com/jiawenyao401/JiaClaw/releases/download/v0.1.0/')
source=pathlib.Path(os.environ['INSTALL_FIXTURE']) / url.rsplit('/',1)[1]
if not source.exists(): sys.exit(22)
shutil.copyfile(source, args[args.index('--output')+1])
'''); curl.chmod(0o755)
    env = dict(os.environ, PATH=str(tools) + ':' + os.environ['PATH'], JIACLAW_INSTALL_DIR=str(install), INSTALL_FIXTURE=str(release))
    def run(version='v0.1.0'):
        return subprocess.run(['sh', str(installer), version], env=env, capture_output=True, text=True)
    result = run(); assert result.returncode == 0, result.stderr
    assert (install / 'jiaclaw').read_bytes() == binary.read_bytes()
    before = (install / 'jiaclaw').read_bytes()
    sums.write_text('0' * 64 + '  ' + asset.name + '\n')
    result = run(); assert result.returncode != 0 and 'checksum mismatch' in result.stderr
    assert (install / 'jiaclaw').read_bytes() == before
    sums.unlink()
    assert run().returncode != 0
    assert (install / 'jiaclaw').read_bytes() == before
    assert run('../../bad').returncode != 0
    assert not list(install.glob('.jiaclaw-install.*'))
    print('PASS: verified atomic install; invalid version, missing checksum and corruption preserve existing binary')
