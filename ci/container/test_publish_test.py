import copy
import hashlib
import json
from pathlib import Path
import tempfile
import subprocess
import io
from unittest import mock
import unittest

from publish_test import publish, index_digest, main, push_platform, run
from test_image_manifest import IMAGE, REVISION

REPOSITORY = 'ghcr.io/hyperme-sh/hypermesh-visor'
DIGESTS = {'amd64': 'sha256:' + 'c' * 64, 'arm64': 'sha256:' + 'd' * 64}
INDEX = {'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.index.v1+json',
         'digest': 'sha256:' + 'e' * 64,
         'manifests': [{'digest': digest, 'platform': {'os': 'linux', 'architecture': arch}}
                       for arch, digest in DIGESTS.items()]}


class PublicationContracts(unittest.TestCase):
    def test_blob_retry_keeps_platform_and_target(self):
        command = mock.Mock(side_effect=[subprocess.CalledProcessError(
            1, ['docker'], stderr='unknown blob\n'), 'ok'])
        sleep = mock.Mock()
        push_platform(command, 'fixture:source-amd64', 'amd64', sleep)
        expected = mock.call('image', 'push', '--platform', 'linux/amd64', 'fixture:source-amd64')
        self.assertEqual(command.call_args_list, [expected, expected])
        sleep.assert_called_once_with(5)

    def test_retries_are_bounded_and_other_errors_fail_immediately(self):
        for message, count in [('unknown blob\n', 3), ('unauthorized', 1),
                               ('denied: unknown blob scope', 1)]:
            with self.subTest(message=message):
                command = mock.Mock(side_effect=subprocess.CalledProcessError(
                    1, ['docker'], stderr=message))
                sleep = mock.Mock()
                with self.assertRaises(subprocess.CalledProcessError):
                    push_platform(command, 'fixture:source-arm64', 'arm64', sleep)
                self.assertEqual(command.call_count, count)
                self.assertEqual(sleep.call_count, count - 1)

    def test_command_failure_reports_bounded_output_and_preserves_error(self):
        error = subprocess.CalledProcessError(1, ['docker'],
                                             output='x' * 20000, stderr='unknown blob\n')
        with mock.patch('publish_test.subprocess.check_output', side_effect=error), \
                mock.patch('sys.stderr', new_callable=io.StringIO) as output:
            with self.assertRaises(subprocess.CalledProcessError) as caught:
                run('image', 'push', 'fixture:tag')
            self.assertIs(caught.exception, error)
            self.assertIn('docker: unknown blob', output.getvalue())
            self.assertLess(len(output.getvalue()), 8300)

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
            self.assertEqual([call for call in calls if call[:2] == ('image', 'push')],
                             [('image', 'push', '--platform', f'linux/{arch}',
                               f'{REPOSITORY}:test-{REVISION}-{arch}') for arch in DIGESTS])

    def test_corrupt_second_archive_causes_no_registry_or_docker_calls(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            images, calls = self.fixture(root), []
            (root/'visor-container-arm64/visor-container-arm64.tar.gz').write_bytes(b'wrong')
            with self.assertRaises(ValueError): publish(root, REPOSITORY, REVISION, self.command(images, calls))
            self.assertEqual(calls, [])

    def test_exhausted_blob_failure_never_moves_test(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            images, calls = self.fixture(root), []
            base = self.command(images, calls)

            def command(*args):
                result = base(*args)
                if args[:2] == ('image', 'push'):
                    raise subprocess.CalledProcessError(1, ['docker'], stderr='unknown blob\n')
                return result

            with mock.patch('publish_test.push_platform', wraps=lambda command, tag, arch:
                            push_platform(command, tag, arch, mock.Mock())):
                with self.assertRaises(subprocess.CalledProcessError):
                    publish(root, REPOSITORY, REVISION, command)
            self.assertEqual(sum(call[:2] == ('image', 'push') for call in calls), 3)
            self.assertFalse(any(REPOSITORY + ':test' in call for call in calls))

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
