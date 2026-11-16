"""Host-loss watchdog for a managed desktop; never grants input authority."""
import json
import os
from pathlib import Path
import time
import uuid

KEYS = ('HYPERMESH_MANAGED_COMPANION', 'HYPERMESH_MANAGED_SESSION', 'HYPERMESH_MANAGED_GENERATION')

def scope(env):
    if not any(key in env for key in KEYS):
        return None
    try:
        ids = [uuid.UUID(env[key]) for key in KEYS]
        if any(value.int == 0 for value in ids):
            raise ValueError()
        return dict(zip(('companion', 'session', 'generation'), map(str, ids)))
    except (KeyError, ValueError, TypeError):
        raise RuntimeError('invalid managed desktop scope') from None

def alive(expected, path=Path('/run/manager/lease.json'), now=None):
    if expected is None:
        return True
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as stream:
            data = stream.read(2049)
        if len(data) > 2048:
            return False
        lease = json.loads(data)
        now = int(time.time() * 1000) if now is None else now
        expires = lease['expires_ms']
        return (lease['version'] == 1 and type(expires) is int
                and 0 < expires - now <= 1000
                and all(lease[key] == value for key, value in expected.items()))
    except (OSError, ValueError, KeyError, TypeError):
        return False
