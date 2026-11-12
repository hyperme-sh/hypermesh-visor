"""Development bundle content and launcher contracts; never opens the desktop."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

APP = 'Hypermesh Visor Development.app/Contents'
PROGRAMS = [f'{APP}/MacOS/hypermesh-native-desktop', 'libexec/hypermesh', 'libexec/hypermesh-desktop-mcp']


def manifest(root):
    rows = []
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError('Bundle symlinks are not supported')
        if path.is_file() and path != root/'checksums.sha256':
            rows.append(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  ./{path.relative_to(root)}\n')
    return ''.join(rows)


def verify(root):
    if (root/'checksums.sha256').read_text() != manifest(root):
        raise ValueError('Bundle checksum mismatch')
    if not re.fullmatch(r'[a-f0-9]{40}\n', (root/'source-revision').read_text()):
        raise ValueError('Invalid source revision')
    info = plistlib.loads((root/APP/'Info.plist').read_bytes())
    assert info['CFBundleIdentifier'] == 'sh.hyperme.visor.development'
    assert info['CFBundleVersion'] == '1'
    assert info['CFBundleShortVersionString'] == '0.1.0-dev'
    assert info['LSMinimumSystemVersion']
    for relative in PROGRAMS:
        path = root/relative
        with path.open('rb') as stream:
            magic, cpu = struct.unpack('<II', stream.read(8))
        if (magic, cpu) != (0xfeedfacf, 0x0100000c) or not os.access(path, os.X_OK):
            raise ValueError('Expected an executable native ARM64 Mach-O program')
    for name in ['hypermesh', 'hypermesh-desktop-mcp']:
        assert os.access(root/'bin'/name, os.X_OK)
    assert (root/'README.md').is_file()


class LauncherContracts(unittest.TestCase):
    def test_paths_arguments_and_runtime_selection(self):
        with tempfile.TemporaryDirectory(prefix="visor's bundle ") as directory:
            root = Path(directory)
            (root/'bin').mkdir()
            (root/'libexec').mkdir()
            for name in ['hypermesh', 'hypermesh-desktop-mcp']:
                launch = root/'bin'/name
                shutil.copyfile(Path(__file__).with_name('launch-tool.sh'), launch)
                launch.chmod(0o755)
                program = root/'libexec'/name
                program.write_text('#!' + sys.executable + '\nimport os,json,sys\nprint(json.dumps([sys.argv[1:],os.environ["HYPERMESH_RUNTIME_DIR"]]))\n')
                program.chmod(0o755)
                for override in [None, str(root/'custom socket')]:
                    env = dict(os.environ, HOME=str(root/'home'))
                    env.pop('HYPERMESH_RUNTIME_DIR', None)
                    if override is not None:
                        env['HYPERMESH_RUNTIME_DIR'] = override
                    args = ['literal;$(no)', 'spaces and quotes\'"', '']
                    result = subprocess.run([str(launch), *args], env=env, check=True, capture_output=True, text=True)
                    self.assertEqual(json.loads(result.stdout), [args, override or str(root/'home/.local/state/hypermesh/native-run')])

    def test_side_by_side_upgrade_and_removal_preserve_user_state(self):
        with tempfile.TemporaryDirectory(prefix="visor upgrade ") as directory:
            root = Path(directory)
            home = root/'home'
            config = home/'.config/hypermesh/config.toml'
            config.parent.mkdir(parents=True)
            config.write_text('tenant_id = "fixture"\n')
            before = config.read_bytes()
            bundles = [root/'previous', root/'replacement']
            for bundle in bundles:
                (bundle/'bin').mkdir(parents=True)
                (bundle/'libexec').mkdir()
                for name in ['hypermesh', 'hypermesh-desktop-mcp']:
                    launcher = bundle/'bin'/name
                    shutil.copyfile(Path(__file__).with_name('launch-tool.sh'), launcher)
                    launcher.chmod(0o755)
                    program = bundle/'libexec'/name
                    program.write_text('#!' + sys.executable + '\nimport json,os\nprint(json.dumps([__file__,os.environ["HYPERMESH_RUNTIME_DIR"]]))\n')
                    program.chmod(0o755)
            env = dict(os.environ, HOME=str(home))
            env.pop('HYPERMESH_RUNTIME_DIR', None)
            for bundle in bundles:
                for name in ['hypermesh', 'hypermesh-desktop-mcp']:
                    result = subprocess.run([str(bundle/'bin'/name)], check=True, capture_output=True, text=True, env=env)
                    self.assertEqual(json.loads(result.stdout), [str(bundle/'libexec'/name), str(home/'.local/state/hypermesh/native-run')])
                self.assertEqual(config.read_bytes(), before)
                # The replacement launcher must continue to resolve its own files
                # after the old extracted distribution has been removed.
                shutil.rmtree(bundle)
            self.assertEqual(config.read_bytes(), before)

    def test_manifest_detects_tampering_and_refuses_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'payload with spaces').write_bytes(b'original')
            (root/'nested').mkdir()
            (root/'nested/checksums.sha256').write_text('also covered')
            original = manifest(root)
            self.assertIn('./nested/checksums.sha256', original)
            (root/'checksums.sha256').write_text(original)
            self.assertEqual(manifest(root), original)
            (root/'payload with spaces').write_bytes(b'changed')
            self.assertNotEqual(manifest(root), original)
            (root/'link').symlink_to(root/'payload with spaces')
            with self.assertRaises(ValueError):
                manifest(root)


if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']:
        unittest.main(argv=[sys.argv[0]])
    elif len(sys.argv) == 3 and sys.argv[1] == '--manifest':
        root = Path(sys.argv[2])
        (root/'checksums.sha256').write_text(manifest(root))
    elif len(sys.argv) == 2:
        verify(Path(sys.argv[1]))
        print('Native bundle content contracts passed')
    else:
        raise SystemExit('Use --self-test, --manifest DIRECTORY or DIRECTORY')
