"""Publish already-built native archives to the automatic TEST channel."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from image_manifest import validate

ARCHES = ('amd64', 'arm64')
DIGEST = r'sha256:[0-9a-f]{64}'


def run(*args):
    return subprocess.check_output(['docker', *args], text=True, timeout=900)


def prepare(root, revision):
    if not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise ValueError('A complete source revision is required')
    prepared = []
    for arch in ARCHES:
        folder = root/f'visor-container-{arch}'
        manifest = json.loads((folder/'manifest.json').read_text())
        archive = folder/f'visor-container-{arch}.tar.gz'
        record = manifest.get('archive', {})
        if (manifest.get('schema_version') != 1 or manifest.get('source_revision') != revision
                or manifest.get('platform') != f'linux/{arch}'
                or not re.fullmatch(DIGEST, manifest.get('image_id', ''))
                or record.get('name') != archive.name or archive.is_symlink()
                or record.get('bytes') != archive.stat().st_size):
            raise ValueError('Native archive identity mismatch')
        with archive.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if digest != record.get('sha256'):
            raise ValueError('Native archive checksum mismatch')
        prepared.append((arch, archive, manifest))
    return prepared


def index_digest(value, expected):
    if (value.get('schemaVersion') != 2
            or value.get('mediaType') not in ('application/vnd.oci.image.index.v1+json',
                                              'application/vnd.docker.distribution.manifest.list.v2+json')
            or not re.fullmatch(DIGEST, value.get('digest', ''))):
        raise ValueError('Invalid registry image index')
    manifests = value.get('manifests', [])
    actual = {}
    for entry in manifests:
        platform = entry.get('platform', {})
        arch = platform.get('architecture')
        if platform.get('os') != 'linux' or arch not in ARCHES or arch in actual:
            raise ValueError('Unexpected registry image platform')
        actual[arch] = entry.get('digest')
    if actual != expected:
        raise ValueError('Registry image index does not match native builds')
    return value['digest']


def publish(root, repository, revision, command=run):
    if not re.fullmatch(r'ghcr\.io/[a-z0-9][a-z0-9-]*/[a-z0-9][a-z0-9._-]*', repository):
        raise ValueError('Expected a GitHub container repository')
    prepared = prepare(root, revision)  # Verify both archives before any registry write.
    images = {}
    for arch, archive, manifest in prepared:
        command('image', 'load', '--input', str(archive))
        image_id = manifest['image_id']
        loaded = json.loads(command('image', 'inspect', image_id))
        if len(loaded) != 1 or validate(loaded[0], arch, revision)['image_id'] != image_id:
            raise ValueError('Loaded image does not match its archive manifest')
        tag = f'{repository}:test-{revision}-{arch}'
        command('image', 'tag', image_id, tag)
        command('image', 'push', tag)
        remote = json.loads(command('buildx', 'imagetools', 'inspect', '--format', '{{json .Manifest}}', tag))
        digest = remote.get('digest', '')
        if not re.fullmatch(DIGEST, digest) or remote.get('manifests'):
            raise ValueError('Expected a single-platform registry image')
        stored = json.loads(command('buildx', 'imagetools', 'inspect', '--raw', f'{repository}@{digest}'))
        if stored.get('schemaVersion') != 2 or stored.get('config', {}).get('digest') != image_id:
            raise ValueError('Registry image configuration does not match the tested build')
        images[arch] = dict(manifest, registry_digest=digest)
    expected = {arch: images[arch]['registry_digest'] for arch in ARCHES}
    source_tag = f'{repository}:test-{revision}'
    command('buildx', 'imagetools', 'create', '--tag', source_tag,
            *(f'{repository}@{expected[arch]}' for arch in ARCHES))
    descriptor = json.loads(command('buildx', 'imagetools', 'inspect', '--format', '{{json .Manifest}}', source_tag))
    digest = index_digest(descriptor, expected)
    # Copy the verified index by digest; only this last operation moves TEST.
    command('buildx', 'imagetools', 'create', '--tag', f'{repository}:test', f'{repository}@{digest}')
    current = json.loads(command('buildx', 'imagetools', 'inspect', '--format', '{{json .Manifest}}', f'{repository}:test'))
    if index_digest(current, expected) != digest:
        raise ValueError('TEST channel digest mismatch')
    return {'schema_version': 1, 'source_revision': revision, 'repository': repository,
            'channel': f'{repository}:test', 'source_tag': source_tag,
            'registry_digest': digest, 'images': images}


def main():
    if os.environ.get('GITHUB_EVENT_NAME') != 'push' or os.environ.get('GITHUB_REF') != 'refs/heads/main':
        raise SystemExit('TEST publication requires the main push workflow')
    root, repository, revision, output = sys.argv[1:]
    result = publish(Path(root), repository, revision)
    Path(output).write_text(json.dumps(result, indent=2) + '\n')
    print('Published TEST index ' + result['registry_digest'])


if __name__ == '__main__':
    main()
