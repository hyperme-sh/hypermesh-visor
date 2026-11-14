from pathlib import Path
import plistlib
import tempfile
import unittest

from package import HEADER, LIBRARY, MODULE, SLICES, check_framework


class PackageContract(unittest.TestCase):
    def test_platform_slices_native_bytes_and_ffi_cohort(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bindings = root / "bindings"
            bindings.mkdir()
            (bindings / HEADER).write_bytes(b"fixture header")
            (bindings / MODULE).write_bytes(b"fixture module")
            framework = root / "Mobile.xcframework"
            framework.mkdir()
            entries, originals = [], {}
            for name, (variant, _, _) in SLICES.items():
                directory = framework / name
                (directory / "Headers").mkdir(parents=True)
                data = b"!<arch>\n" + name.encode()
                (directory / LIBRARY).write_bytes(data)
                original = root / (name + ".a")
                original.write_bytes(data)
                originals[name] = original
                (directory / "Headers" / HEADER).write_bytes((bindings / HEADER).read_bytes())
                (directory / "Headers/module.modulemap").write_bytes((bindings / MODULE).read_bytes())
                entry = dict(LibraryIdentifier=name, LibraryPath=LIBRARY, HeadersPath="Headers", SupportedPlatform="ios", SupportedArchitectures=["arm64"])
                if variant:
                    entry["SupportedPlatformVariant"] = variant
                entries.append(entry)
            def write(items):
                (framework / "Info.plist").write_bytes(plistlib.dumps(dict(AvailableLibraries=items)))
            write(entries)
            self.assertEqual(set(check_framework(framework, originals, bindings)), set(SLICES))
            for bad in [entries[:1], [entries[0], entries[0]], [dict(entries[0], SupportedArchitectures=["x86_64"]), entries[1]],
                        [entries[0], dict(entries[1], SupportedPlatformVariant="maccatalyst")],
                        [dict(entries[0], LibraryPath="../outside.a"), entries[1]]]:
                write(bad)
                with self.assertRaises(ValueError):
                    check_framework(framework, originals, bindings)
            write(entries)
            binary = framework / "ios-arm64" / LIBRARY
            binary.write_bytes(b"!<arch>\nstale-build")
            with self.assertRaises(ValueError):
                check_framework(framework, originals, bindings)
            binary.write_bytes(originals["ios-arm64"].read_bytes())
            (framework / "ios-arm64/Headers/module.modulemap").write_bytes(b"different-interface")
            with self.assertRaises(ValueError):
                check_framework(framework, originals, bindings)


if __name__ == "__main__":
    unittest.main()
