"""Explicit one-start consent for the dedicated private container desktop."""
import http.client
import json
import os
from pathlib import Path
import socket
import stat
import tempfile
import uuid


def private_config(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            raise ValueError('Bot config must be an owner-only regular file')
        raw = stream.read(8193)
    if len(raw) > 8192:
        raise ValueError('Bot config exceeds 8 KiB')
    value = json.loads(raw)
    if not isinstance(value, dict) or value.get('mode') != 'container':
        raise ValueError('Unattended startup requires container enrollment')
    return value


def write_config(path, config):
    path = Path(path)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            os.fchmod(stream.fileno(), 0o600)
            json.dump(config, stream)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


def prepare(environ, runtime):
    flag = environ.get('HYPERMESH_CONTAINER_UNATTENDED', '0')
    if flag == '0':
        return None
    if flag != '1':
        raise ValueError('HYPERMESH_CONTAINER_UNATTENDED must be 0 or 1')
    source = environ.get('HYPERMESH_BOT_CONFIG')
    if not source:
        raise ValueError('Unattended startup requires a bot config')
    config = private_config(source)
    config['session_id'] = str(uuid.UUID(int=0))
    target = Path(runtime) / 'bot-runtime.json'
    write_config(target, config)
    environ['HYPERMESH_BOT_CONFIG'] = str(target)
    return target


class Connection(http.client.HTTPConnection):
    def __init__(self, companion):
        super().__init__('visor', timeout=2)
        self.path = '/run/visor/hypermesh/' + ('companion.sock' if companion else 'control.sock')
    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(2)
        self.sock.connect(self.path)


def local_request(companion, method, path, body=None):
    connection = Connection(companion)
    try:
        connection.request(method, path, body=None if body is None else json.dumps(body),
                           headers={'Content-Type': 'application/json'})
        response = connection.getresponse()
        data = response.read(65537)
        if not 200 <= response.status < 300 or len(data) > 65536:
            raise RuntimeError('Local unattended setup was refused')
        return json.loads(data) if data else None
    finally:
        connection.close()


def activate(config_path, request=local_request):
    config = private_config(config_path)
    opened = request(False, 'POST', '/session', {
        'purpose': 'Dedicated container bot', 'recipes': [],
        'agent': {'name': 'Container companion', 'instructions': ''},
        'skills': [], 'secrets': [],
    })
    session = str(uuid.UUID(opened['id']))
    config['session_id'] = session
    write_config(config_path, config)
    # This local process, not an incoming task, grants the single startup consent.
    # Normal companion monitoring continues; this helper never retries start/resume.
    request(True, 'GET', '/companion')
    request(True, 'POST', '/companion/automation', {'action': 'start'})
    return session
