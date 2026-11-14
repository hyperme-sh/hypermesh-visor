import struct
import tempfile
from pathlib import Path
import unittest
import zipfile

from check_consumer import NATIVE, check_consumer
from package import LIBRARY
from test_package import elf


def apk_fixture(path, *, aligned=True, jna=None, visor=None, extra=False, missing=False, compressed=False):
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("AndroidManifest.xml", b"fixture")
        if not missing:
            archive.writestr("classes.dex", b"fixture")
        for name in sorted(NATIVE):
            info = zipfile.ZipInfo(name)
            if aligned:
                padding = -(archive.fp.tell() + 30 + len(name)) % 16384
                if padding < 4:
                    padding += 16384
                info.extra = struct.pack("<HH", 0xCAFE, padding - 4) + bytes(padding - 4)
            info.compress_type = zipfile.ZIP_DEFLATED if compressed else zipfile.ZIP_STORED
            data = (jna if "jnidispatch" in name else visor)
            archive.writestr(info, elf() if data is None else data)
        if extra:
            archive.writestr("lib/x86_64/unexpected.so", elf(machine=62))


class ConsumerContract(unittest.TestCase):
    def test_validates_the_dependency_and_final_apk_not_only_the_aar(self):
        with tempfile.TemporaryDirectory() as directory:
            apk, aar = Path(directory) / "consumer.apk", Path(directory) / "library.aar"
            with zipfile.ZipFile(aar, "w") as archive:
                archive.writestr(LIBRARY, elf())
            apk_fixture(apk)
            check_consumer(apk, aar)
            for kwargs in [dict(aligned=False), dict(jna=elf(4096)), dict(jna=elf(machine=62)),
                           dict(visor=elf() + b"changed"), dict(extra=True), dict(missing=True), dict(compressed=True)]:
                with self.subTest(kwargs=kwargs):
                    apk_fixture(apk, **kwargs)
                    with self.assertRaises(ValueError):
                        check_consumer(apk, aar)


if __name__ == "__main__":
    unittest.main()
