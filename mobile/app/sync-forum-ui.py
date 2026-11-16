#!/usr/bin/env python3
"""Copy the shared source package from an explicit s3r.ch checkout revision."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

source = Path(sys.argv[1]).resolve()
def git(*args):
    return subprocess.check_output(['git', '-C', str(source), *args])
revision = git('rev-parse', 'HEAD').decode().strip()
version = json.loads(git('show', f'{revision}:package.json'))['version']
prefix = 'packages/forum-ui/'
files = git('ls-tree', '-r', '--name-only', revision, '--', prefix).decode().splitlines()
target = Path(__file__).resolve().parent / 'vendor/s3rch-forum-ui'
expected = {name[len(prefix):] for name in files}
if not expected or 'src/MessageComposer.tsx' not in expected:
    raise SystemExit('Shared composer package is missing')
# Never remove unrelated vendor content or read uncommitted source bytes.
for path in target.rglob('*') if target.exists() else []:
    if path.is_file() and path.relative_to(target).as_posix() not in expected | {'UPSTREAM.json'}:
        raise SystemExit(f'Unexpected vendor file: {path.name}')
digests = {}
for name in files:
    relative = name[len(prefix):]
    data = git('show', f'{revision}:{name}')
    path = target / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    digests[relative] = hashlib.sha256(data).hexdigest()
(target / 'UPSTREAM.json').write_text(json.dumps({
    'repository': 'FyberLabs/s3r.ch', 'source_version': version,
    'revision': revision, 'package': '@s3rch/forum-ui', 'files': digests,
}, indent=2) + '\n')
