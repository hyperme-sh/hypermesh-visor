import copy
import hashlib
import json
from pathlib import Path
import tempfile
from unittest import mock
import unittest

from publish_test import publish, index_digest, main
from test_image_manifest import IMAGE, REVISION

REPOSITORY = 'ghcr.io/hyperme-sh/hypermesh-visor'
DIGESTS = {'amd64': 'sha256:' + 'c' * 64, 'arm64': 'sha256:' + 'd' * 64}
INDEX = {'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.index.v1+json',
         'digest': 'sha256:' + 'e' * 64,
         'manifests': [{'digest': digest, 'platform': {'os': 'linux', 'architecture': arch}}
                       for arch, digest in DIGESTS.items()]}


class PublicationContracts(unittest.TestCase):
    def fixture(self, root):
        images = {}
        for number, arch in enumerate(DIGESTS):
            folder = root/f'visor-container-{arch}'
            folder.mkdir()
            archive = folder/f'visor-container-{arch}.tar.gz'
            archive.write_bytes(arch.encode())
            image = copy.deepcopy(IMAGE)
            image['Architecture'] = arch
            image['Id'] = 'sha256:' + str(number + 1) * 64
            images[image['Id']] = image
            manifest = {'schema_version': 1, 'platform': f'linux/{arch}', 'source_revision': REVISION,
                        'image_id': image['Id'], 'archive': {'name': archive.name, 'bytes': archive.stat().st_size,
                        'sha256': hashlib.sha256(archive.read_bytes()).hexdigest()}}
            (folder/'manifest.json').write_text(json.dumps(manifest))
        return images

    def command(self, images, calls, bad_index=False, fail_arm=False, bad_config=False):
        def call(*args):
            calls.append(args)
            if args[:2] == ('image', 'inspect'):
                return json.dumps([images[args[2]]])
            if args[:2] == ('image', 'push') and args[-1].endswith('-arm64') and fail_arm:
                raise RuntimeError('fixture registry failure')
            if args[:3] == ('buildx', 'imagetools', 'inspect'):
                if '--raw' in args:
                    arch = next(arch for arch, digest in DIGESTS.items() if args[-1].endswith(digest))
                    image_id = next(key for key, value in images.items() if value['Architecture'] == arch)
                    return json.dumps({'schemaVersion': 2, 'config': {'digest': 'wrong' if bad_config else image_id}})
                for arch, digest in DIGESTS.items():
                    if args[-1].endswith('-' + arch): return json.dumps({'digest': digest})
                value = copy.deepcopy(INDEX)
                if bad_index: value['manifests'].pop()
                return json.dumps(value)
            return ''
        return call

    def test_only_verified_two_architecture_index_moves_test(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            images, calls = self.fixture(root), []
            result = publish(root, REPOSITORY, REVISION, self.command(images, calls))
            self.assertEqual(result['registry_digest'], INDEX['digest'])
            self.assertEqual(result['images']['arm64']['registry_digest'], DIGESTS['arm64'])
            promotion = ('buildx', 'imagetools', 'create', '--tag', REPOSITORY + ':test', REPOSITORY + '@' + INDEX['digest'])
            self.assertEqual(calls[-2], promotion)
            self.assertFalse(any('run' in call for call in calls))

    def test_corrupt_second_archive_causes_no_registry_or_docker_calls(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            images, calls = self.fixture(root), []
            (root/'visor-container-arm64/visor-container-arm64.tar.gz').write_bytes(b'wrong')
            with self.assertRaises(ValueError): publish(root, REPOSITORY, REVISION, self.command(images, calls))
            self.assertEqual(calls, [])

    def test_partial_push_or_bad_index_never_moves_test(self):
        for options in ({'bad_index': True}, {'fail_arm': True}, {'bad_config': True}):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                images, calls = self.fixture(root), []
                with self.assertRaises((ValueError, RuntimeError)):
                    publish(root, REPOSITORY, REVISION, self.command(images, calls, **options))
                self.assertFalse(any(REPOSITORY + ':test' in call for call in calls))

    def test_non_main_or_manual_events_cannot_publish(self):
        for event, ref in [('pull_request', 'refs/pull/1/merge'),
                           ('workflow_dispatch', 'refs/heads/main'),
                           ('push', 'refs/heads/feature')]:
            with self.subTest(event=event, ref=ref), mock.patch.dict('os.environ',
                    {'GITHUB_EVENT_NAME': event, 'GITHUB_REF': ref}), mock.patch('publish_test.publish') as operation:
                with self.assertRaisesRegex(SystemExit, 'main push workflow'):
                    main()
                operation.assert_not_called()

    def test_wrong_platform_or_duplicate_descriptor_is_refused(self):
        for change in ('windows', 'duplicate', 'digest'):
            value = copy.deepcopy(INDEX)
            if change == 'windows': value['manifests'][0]['platform']['os'] = 'windows'
            if change == 'duplicate': value['manifests'].append(value['manifests'][0])
            if change == 'digest': value['manifests'][0]['digest'] = DIGESTS['arm64']
            with self.assertRaises(ValueError): index_digest(value, DIGESTS)


if __name__ == '__main__':
    unittest.main()
