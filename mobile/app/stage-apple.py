"""Stage the same-revision, checksum-verified native bundle for Xcode compilation."""
from pathlib import Path, PurePosixPath
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import zipfile

root = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory() as scratch:
    unpacked = Path(scratch)
    with zipfile.ZipFile(sys.argv[1]) as archive:
        for item in archive.infolist():
            path = PurePosixPath(item.filename)
            if path.is_absolute() or '..' in path.parts or (item.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError('Unsafe native artifact')
        archive.extractall(unpacked)
    build = json.loads((unpacked / 'build.json').read_text())
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    if build['source_revision'] != revision or build['signed'] is not False:
        raise ValueError('Native artifact provenance differs')
    for line in (unpacked / 'checksums.txt').read_text().splitlines():
        digest, name = line.split('  ', 1)
        path = unpacked / name
        if not path.resolve().is_relative_to(unpacked) or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError('Native artifact checksum differs')
    dest = root / 'ios/App/Native'
    dest.mkdir(exist_ok=True)
    shutil.copytree(unpacked / 'HypermeshVisorMobile.xcframework', dest / 'HypermeshVisorMobile.xcframework', dirs_exist_ok=True)
    for name in ('hypermesh_visor_mobile.swift', 'KeychainSessionStore.swift'):
        shutil.copy2(unpacked / 'Sources' / name, dest / name)
