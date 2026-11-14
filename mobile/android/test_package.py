import io
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile

from package import CLASSES, LIBRARY, check_aar, check_elf


def elf(alignment=16384, machine=183):
    data = bytearray(128)
    data[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<HH", data, 16, 3, machine)
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<HH", data, 54, 56, 1)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0, 0, 128, 128, alignment)
    return bytes(data)


def aar(path, native, minimum=28, extra="", missing=False, other_abi=False):
    classes = io.BytesIO()
    with zipfile.ZipFile(classes, "w") as jar:
        for name in sorted(CLASSES - ({next(iter(CLASSES))} if missing else set())):
            jar.writestr(name, b"fixture")
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("classes.jar", classes.getvalue())
        archive.writestr(LIBRARY, native)
        archive.writestr("proguard.txt", "-keep class uniffi.hypermesh_visor_mobile.** { *; }\n-keep class com.sun.jna.** { *; }")
        archive.writestr("AndroidManifest.xml", f'<manifest xmlns:android="http://schemas.android.com/apk/res/android"><uses-sdk android:minSdkVersion="{minimum}"/>{extra}</manifest>')
        if other_abi:
            archive.writestr("jni/x86_64/extra.so", native)


class PackageContract(unittest.TestCase):
    def test_native_architecture_and_page_alignment(self):
        check_elf(elf())
        for bad in [b"", elf(4096), elf(16385), elf(machine=62), elf()[:100]]:
            with self.assertRaises(ValueError):
                check_elf(bad)

    def test_binding_adapter_and_manifest_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            native, archive = Path(directory) / "library.so", Path(directory) / "library.aar"
            native.write_bytes(elf())
            aar(archive, elf())
            self.assertEqual(len(check_aar(archive, native)), 64)
            for kwargs in [dict(minimum=26), dict(missing=True), dict(other_abi=True),
                           dict(extra='<uses-permission android:name="android.permission.INTERNET"/>'),
                           dict(extra='<application><service android:name="HiddenService"/></application>')]:
                aar(archive, elf(), **kwargs)
                with self.assertRaises(ValueError):
                    check_aar(archive, native)
            aar(archive, elf() + b"different-build")
            with self.assertRaises(ValueError):
                check_aar(archive, native)
            aar(archive, elf())
            with zipfile.ZipFile(archive, "a") as output:
                output.writestr("../escape", b"bad")
            with self.assertRaises(ValueError):
                check_aar(archive, native)


if __name__ == "__main__":
    unittest.main()
