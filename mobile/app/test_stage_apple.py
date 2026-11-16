import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location('stage_apple', Path(__file__).with_name('stage-apple.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class StagingTests(unittest.TestCase):
    def bundle(self, root, *, bad_checksum=False, unsafe=False):
        entries = {'build.json': json.dumps({'source_revision':'a'*40,'signed':False}).encode(),
                   'Sources/hypermesh_visor_mobile.swift': b'generated bindings',
                   'Sources/KeychainSessionStore.swift': b'native secure store',
                   'HypermeshVisorMobile.xcframework/Info.plist': b'fixture framework'}
        checksums = ''.join(hashlib.sha256(data).hexdigest() + '  ' + name + '\n' for name, data in entries.items())
        if bad_checksum: entries['Sources/KeychainSessionStore.swift'] = b'changed'
        if unsafe: entries['../escape'] = b'unsafe'
        archive = root / 'native.zip'
        with zipfile.ZipFile(archive, 'w') as out:
            for name, data in entries.items(): out.writestr(name, data)
            out.writestr('checksums.txt', checksums)
        return archive

    def test_symlinked_temporary_root_is_canonicalized_like_macos_var(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            real = root / 'real'; real.mkdir()
            alias = root / 'alias'; alias.symlink_to(real, target_is_directory=True)
            archive = self.bundle(root)
            with patch.object(tempfile, 'tempdir', str(alias)):
                module.stage(archive, root / 'staged', 'a'*40)
            self.assertEqual((root / 'staged/KeychainSessionStore.swift').read_bytes(), b'native secure store')

    def test_wrong_revision_checksum_and_traversal_do_not_stage(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for bad_checksum, unsafe, revision in ((True,False,'a'*40),(False,True,'a'*40),(False,False,'b'*40)):
                archive = self.bundle(root, bad_checksum=bad_checksum, unsafe=unsafe)
                with self.assertRaises(ValueError): module.stage(archive, root / 'staged', revision)
                self.assertFalse((root / 'staged').exists())
                self.assertFalse((root / 'escape').exists())
