#!/usr/bin/env python3
"""Package one native binary without host ownership, xattrs or AppleDouble files."""
import argparse
import gzip
from pathlib import Path
import tarfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('archive', type=Path)
args = parser.parse_args()
if args.binary.is_symlink() or not args.binary.is_file():
    parser.error('binary must be a regular file')
size = args.binary.stat().st_size
if not 0 < size <= 256 * 1024 * 1024:
    parser.error('binary must be 1 byte..256 MiB')
# Exclusive creation preserves an existing candidate. On failure remove only
# the output created by this invocation on handled failure. Upload follows
# successful packaging and installation; process death cannot certify an asset.
output = args.archive.open('xb')
try:
    with output, args.binary.open('rb') as binary:
        with gzip.GzipFile(filename='', fileobj=output, mode='wb', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as archive:
                entry = tarfile.TarInfo('jiaclaw')
                entry.size = size
                entry.mode = 0o755
                archive.addfile(entry, binary)
except BaseException:
    args.archive.unlink()
    raise
