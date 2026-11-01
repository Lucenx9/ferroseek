#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Create a local, checksummed native Linux archive. Never uploads/releases."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tomllib

from installer import ARCHIVE_FILES

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--components', choices=['launcher', 'core', 'mcp', 'full'], default='launcher')
    parser.add_argument('--output', type=Path, default=ROOT / 'dist')
    parser.add_argument('--no-build', action='store_true')
    args = parser.parse_args()
    if platform.system() != 'Linux':
        parser.error('native Linux packaging only; cross-built artifacts need a separate validation workflow')
    components = ['core'] + (['launcher', 'mcp'] if args.components == 'full' else [] if args.components == 'core' else [args.components])
    binaries = {'ferroseek': 'ferroseek-cli', 'ferroseekd': 'ferroseek-daemon'}
    if 'mcp' in components:
        binaries['ferroseek-mcp'] = 'ferroseek-mcp'
    if 'launcher' in components:
        binaries['ferroseek-launcher'] = 'ferroseek-launcher'
    if not args.no_build:
        subprocess.run(['cargo', 'build', '--release', '--locked', *[a for p in binaries.values() for a in ['-p', p]]], cwd=ROOT, check=True)
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    name = f'ferroseek-{version}-linux-{platform.machine()}-{args.components}'
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / (name + '.tar.gz')
    checksum = archive.with_name(archive.name + '.sha256')
    if archive.exists() or checksum.exists():
        parser.error('output already exists; refusing to overwrite it')
    content = {}
    for rel in ARCHIVE_FILES:
        content[rel] = ((ROOT / rel).read_bytes(), 0o755 if rel.endswith('.sh') else 0o644)
    for binary in binaries:
        path = ROOT / 'target/release' / binary
        raw = path.read_bytes()
        machines = {'x86_64': 62, 'aarch64': 183, 'armv7l': 40, 'riscv64': 243}
        if raw[:4] != b'\x7fELF' or int.from_bytes(raw[18:20], 'little' if raw[5] == 1 else 'big') != machines.get(platform.machine()):
            parser.error(f'{binary} is not a supported native ELF binary')
        content['bin/' + binary] = (raw, 0o755)
    package = dict(format=1, version=version, os='linux', arch=platform.machine(), components=components,
                   sha256={p: hashlib.sha256(b).hexdigest() for p, (b, _) in content.items()})
    content['package.json'] = ((json.dumps(package, indent=2) + '\n').encode(), 0o644)
    epoch = int(os.environ.get('SOURCE_DATE_EPOCH', '0'))
    dest = archive.open('xb')
    wrote_checksum = False
    try:
        with dest, gzip.GzipFile(filename='', mode='wb', fileobj=dest, mtime=epoch) as gz, tarfile.open(fileobj=gz, mode='w') as tar:
            for path, (raw, mode) in sorted(content.items()):
                info = tarfile.TarInfo(name + '/' + path)
                info.size, info.mode, info.mtime = len(raw), mode, epoch
                info.uid = info.gid = 0
                info.uname = info.gname = ''
                tar.addfile(info, io.BytesIO(raw))
        with checksum.open('x') as checksum_file:
            wrote_checksum = True
            checksum_file.write(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
    except BaseException:
        archive.unlink(missing_ok=True)
        if wrote_checksum:
            checksum.unlink(missing_ok=True)
        raise
    print(archive)
    print(checksum)
    print('Local artifact only. Verify libc/runtime compatibility and authenticate checksums before distribution.')


if __name__ == '__main__':
    main()
