#!/usr/bin/env python3
"""Installer acceptance with a local release fixture; never downloads real assets."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import resource
import signal
import struct
import subprocess
import tarfile
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('candidate', nargs='?', type=Path)
parser.add_argument('--archive', type=Path, help='Install these exact packaged bytes')
parser.add_argument('--evidence', type=Path, help='Save verified source/artifact evidence')
args = parser.parse_args()
if (args.archive or args.evidence) and not args.candidate:
    parser.error('archive/evidence require an actual candidate binary')
if args.evidence and not args.archive:
    parser.error('evidence requires the exact release archive')
repository = Path(__file__).resolve().parents[1]
installer = repository / 'scripts/install.sh'
candidate = args.candidate.resolve() if args.candidate else None
target = {('Linux', 'x86_64'): 'x86_64-unknown-linux-gnu', ('Linux', 'aarch64'): 'aarch64-unknown-linux-gnu', ('Darwin', 'x86_64'): 'x86_64-apple-darwin', ('Darwin', 'arm64'): 'aarch64-apple-darwin'}[(os.uname().sysname, os.uname().machine)]


def verify_native_header(header, native_target):
    # A runnable binary under emulation is insufficient evidence of a native build.
    if native_target.endswith('-linux-gnu'):
        assert header[:7] == b'\x7fELF\x02\x01\x01', 'expected little-endian ELF64'
        kind, machine = struct.unpack_from('<HH', header, 16)
        assert kind in (2, 3) and machine == (62 if native_target.startswith('x86_64-') else 183), 'ELF target mismatch'
    else:
        assert header[:4] == b'\xcf\xfa\xed\xfe', 'expected thin little-endian Mach-O64'
        cpu, _, kind = struct.unpack_from('<III', header, 4)
        assert kind == 2 and cpu == (0x1000007 if native_target.startswith('x86_64-') else 0x100000c), 'Mach-O target mismatch'


def verify_archive(path, expected_name, candidate_bytes):
    assert path.name == expected_name, 'release archive filename mismatch'
    assert 0 < path.stat().st_size <= 256 * 1024 * 1024, 'archive size limit'
    assert 0 < len(candidate_bytes) <= 256 * 1024 * 1024, 'binary size limit'
    with tarfile.open(path, 'r:gz') as archive:
        member = archive.next()
        assert member is not None and member.name == 'jiaclaw' and member.isfile(), 'expected a regular jiaclaw member'
        assert member.size == len(candidate_bytes) and member.mode == 0o755, 'binary size/mode mismatch'
        assert member.uid == member.gid == member.mtime == 0 and not member.uname and not member.gname and not member.pax_headers, 'host metadata in package'
        with archive.extractfile(member) as stream:
            assert stream.read(member.size + 1) == candidate_bytes, 'packaged binary differs from tested candidate'
        assert archive.next() is None, 'unexpected or duplicate archive member'


def source_identity(checkout):
    def git(*arguments):
        return subprocess.check_output(['git', *arguments], cwd=checkout, text=True).strip()
    # Graph-walking commands hide parents at shallow boundaries. The commit
    # object retains every real parent even when those objects are not fetched.
    headers = git('cat-file', '-p', 'HEAD').split('\n\n', 1)[0].splitlines()
    return dict(source_commit=git('rev-parse', 'HEAD'), source_tree=git('rev-parse', 'HEAD^{tree}'), source_parents=[line.split(' ', 1)[1] for line in headers if line.startswith('parent ')], tracked_source_dirty=bool(git('status', '--porcelain', '--untracked-files=no')))


version = 'v0.1.0'
if candidate:
    output = subprocess.check_output([str(candidate), 'version'], text=True)
    match = re.fullmatch(r'JiaClaw (v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)', output.splitlines()[0])
    assert match, output
    version = match.group(1)
with tempfile.TemporaryDirectory(prefix='jiaclaw-install-') as directory:
    root = Path(directory)
    tools = root / 'tools'; tools.mkdir()
    release = root / 'release'; release.mkdir()
    install = root / 'bin'
    binary = release / 'jiaclaw'
    if candidate:
        binary.write_bytes(candidate.read_bytes())
    else:
        binary.write_text('#!/bin/sh\nprintf "JiaClaw v0.1.0\\n"\n')
    binary.chmod(0o755)
    asset = release / ('jiaclaw-' + version + '-' + target + '.tar.gz')
    evidence = None
    if args.archive:
        candidate_bytes = binary.read_bytes()
        verify_native_header(candidate_bytes[:64], target)
        verify_archive(args.archive, asset.name, candidate_bytes)
        asset.write_bytes(args.archive.read_bytes())
        packager = repository / 'scripts/package_release.py'
        def package(source, destination, **options):
            return subprocess.run(['python3', str(packager), str(source), str(destination)], capture_output=True, **options)
        assert package(candidate, args.archive).returncode != 0
        assert args.archive.read_bytes() == asset.read_bytes(), 'packager replaced an existing artifact'
        link = root / 'linked-binary'; link.symlink_to(binary)
        failed = root / 'failed.tar.gz'
        assert package(link, failed).returncode != 0 and not failed.exists()
        # A real write failure must remove this invocation's incomplete output.
        def file_size_limit():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (1024, 1024))
        assert package(candidate, failed, preexec_fn=file_size_limit).returncode != 0
        assert not failed.exists(), 'packager retained a partial artifact after write failure'
        # Exercise a real shallow boundary without changing the source checkout
        # or fetching from the network. Ignore user hooks/signing in this fixture.
        source = root / 'source'; shallow = root / 'shallow'; template = root / 'git-template'; template.mkdir()
        def fixture_git(*arguments, checkout=None):
            return subprocess.check_output(['git', '-c', 'user.name=Installer fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', *arguments], cwd=checkout, text=True, stderr=subprocess.PIPE).strip()
        fixture_git('init', '--quiet', '--initial-branch=fixture', '--template=' + str(template), str(source))
        fixture_git('commit', '--quiet', '--allow-empty', '-m', 'Parent fixture', checkout=source)
        parent = fixture_git('rev-parse', 'HEAD', checkout=source)
        fixture_git('commit', '--quiet', '--allow-empty', '-m', 'Shallow child fixture', checkout=source)
        child = fixture_git('rev-parse', 'HEAD', checkout=source)
        fixture_git('clone', '--quiet', '--template=' + str(template), '--depth', '1', source.as_uri(), str(shallow))
        assert fixture_git('rev-parse', '--is-shallow-repository', checkout=shallow) == 'true'
        assert fixture_git('show', '-s', '--format=%P', 'HEAD', checkout=shallow) == ''
        identity = source_identity(shallow)
        assert identity['source_commit'] == child and identity['source_parents'] == [parent]
        assert identity['source_tree'] == fixture_git('rev-parse', 'HEAD^{tree}', checkout=source) and identity['tracked_source_dirty'] is False
        # Record the actual checkout, including the merge tree tested on PRs.
        evidence = dict(version=version, target=target, **source_identity(repository), binary_sha256=hashlib.sha256(candidate_bytes).hexdigest(), archive=asset.name, archive_sha256=hashlib.sha256(asset.read_bytes()).hexdigest())
        # Physical negative packages must fail before installation. Never extract
        # them into the filesystem, including a link or a path outside the root.
        bad = root / 'negative' / asset.name; bad.parent.mkdir()
        for name, kind, content in [('jiaclaw', tarfile.SYMTYPE, b''), ('../jiaclaw', tarfile.REGTYPE, candidate_bytes), ('jiaclaw', tarfile.REGTYPE, b'wrong binary')]:
            with tarfile.open(bad, 'w:gz') as archive:
                entry = tarfile.TarInfo(name); entry.type = kind; entry.mode = 0o755; entry.size = len(content)
                if kind == tarfile.SYMTYPE: entry.linkname = '/outside/jiaclaw'
                archive.addfile(entry, io.BytesIO(content))
            try:
                verify_archive(bad, asset.name, candidate_bytes)
            except AssertionError:
                pass
            else:
                raise AssertionError('negative package accepted')
        with tarfile.open(bad, 'w:gz') as archive:
            for _ in range(2):
                entry = tarfile.TarInfo('jiaclaw'); entry.mode = 0o755; entry.size = len(candidate_bytes)
                archive.addfile(entry, io.BytesIO(candidate_bytes))
        try:
            verify_archive(bad, asset.name, candidate_bytes)
        except AssertionError:
            pass
        else:
            raise AssertionError('duplicate package member accepted')
        wrong_target = ('aarch64-' if target.startswith('x86_64-') else 'x86_64-') + target.split('-', 1)[1]
        try:
            verify_native_header(candidate_bytes[:64], wrong_target)
        except AssertionError:
            pass
        else:
            raise AssertionError('wrong native architecture accepted')
    else:
        with tarfile.open(asset, 'w:gz') as archive: archive.add(binary, arcname='jiaclaw')
    checksum = hashlib.sha256(asset.read_bytes()).hexdigest()
    sums = release / 'SHA256SUMS'; sums.write_text(checksum + '  ' + asset.name + '\n')
    curl = tools / 'curl'
    curl.write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args=sys.argv[1:]
url=next(arg for arg in args if arg.startswith('https://'))
assert url.startswith('https://github.com/StateKnot/JiaClaw/releases/download/' + os.environ['INSTALL_FIXTURE_VERSION'] + '/')
source=pathlib.Path(os.environ['INSTALL_FIXTURE']) / url.rsplit('/',1)[1]
if not source.exists(): sys.exit(22)
shutil.copyfile(source, args[args.index('--output')+1])
'''); curl.chmod(0o755)
    env = dict(os.environ, PATH=str(tools) + ':' + os.environ['PATH'], JIACLAW_INSTALL_DIR=str(install), INSTALL_FIXTURE=str(release), INSTALL_FIXTURE_VERSION=version)
    def run(requested_version=version):
        return subprocess.run(['sh', str(installer), requested_version], env=env, capture_output=True, text=True)
    result = run(); assert result.returncode == 0, result.stderr
    assert (install / 'jiaclaw').read_bytes() == binary.read_bytes()
    assert subprocess.check_output([str(install / 'jiaclaw'), 'version'], text=True).splitlines()[0] == 'JiaClaw ' + version
    before = (install / 'jiaclaw').read_bytes()
    sums.write_text('0' * 64 + '  ' + asset.name + '\n')
    result = run(); assert result.returncode != 0 and 'checksum mismatch' in result.stderr
    assert (install / 'jiaclaw').read_bytes() == before
    # A checksummed archive can still contain the wrong release. Do not replace
    # an existing installation merely because the downloaded binary can run.
    wrong_version = 'v9.9.9' if version != 'v9.9.9' else 'v0.0.0'
    binary.write_text('#!/bin/sh\nprintf "JiaClaw ' + wrong_version + '\\n"\n'); binary.chmod(0o755)
    with tarfile.open(asset, 'w:gz') as archive: archive.add(binary, arcname='jiaclaw')
    checksum = hashlib.sha256(asset.read_bytes()).hexdigest()
    sums.write_text(checksum + '  ' + asset.name + '\n')
    result = run()
    assert result.returncode != 0 and 'binary version does not match' in result.stderr
    assert (install / 'jiaclaw').read_bytes() == before
    sums.unlink()
    assert run().returncode != 0
    assert (install / 'jiaclaw').read_bytes() == before
    assert run('../../bad').returncode != 0
    assert not list(install.glob('.jiaclaw-install.*'))
    if evidence:
        evidence['acceptance'] = ['native_header', 'exact_archive', 'atomic_install', 'installed_version', 'checksum_retains_old', 'version_retains_old', 'missing_checksum_retains_old', 'invalid_tag_retains_old', 'package_negatives', 'packager_exclusive_create', 'packager_link_rejection', 'packager_write_failure_cleanup', 'shallow_source_parents']
        if args.evidence:
            assert not evidence['tracked_source_dirty'], 'cannot certify modified tracked source'
            args.evidence.write_text(json.dumps(evidence, indent=2) + '\n')
        print(json.dumps(evidence, sort_keys=True))
    print('PASS: canonical download and verified atomic install; wrong binary version, invalid version, missing checksum and corruption preserve existing binary')
