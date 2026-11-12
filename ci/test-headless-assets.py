import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("assets", Path(__file__).with_name("assemble-headless-assets.py"))
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)
REVISION = "a" * 40


class AssetContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.dist = self.root / "dist"
        self.bundles = self.root / "bundles"
        self.dist.mkdir()
        for arch in assets.ARCHES:
            for suffix in ("", ".deb", ".rpm"):
                (self.dist / f"hypermesh-visor-{arch}{suffix}").write_bytes(b"desktop")
            source = self.bundles / arch
            source.mkdir(parents=True)
            for suffix in assets.SUFFIXES:
                (source / f"hypermesh-visor-headless-{arch}{suffix}").write_bytes(arch.encode())
            (source / "build.txt").write_text(f"visor={REVISION}\nconductor={'b' * 40}\narchitecture={arch}\n")
            self.manifest(source)
        self.manifest(self.dist)
        self.original = {p.name: p.read_bytes() for p in self.dist.iterdir()}

    def manifest(self, directory):
        paths = sorted(p for p in directory.iterdir() if p.name != "checksums.txt")
        (directory / "checksums.txt").write_text("".join(f"{assets.digest(p)}  ./{p.name}\n" for p in paths))

    def rejects_without_changing_desktop(self):
        with self.assertRaises((ValueError, FileNotFoundError)):
            assets.assemble(self.dist, self.bundles, REVISION)
        self.assertEqual({p.name: p.read_bytes() for p in self.dist.iterdir()}, self.original)

    def test_both_native_bundles_keep_separate_metadata_and_desktop_bytes(self):
        assets.assemble(self.dist, self.bundles, REVISION)
        for name, data in self.original.items():
            if name != "checksums.txt":
                self.assertEqual((self.dist / name).read_bytes(), data)
        self.assertEqual(len(list(self.dist.iterdir())), 19)
        assets.verify(self.dist, {p.name for p in self.dist.iterdir()} - {"checksums.txt"})
        for arch in assets.ARCHES:
            self.assertIn(f"architecture={arch}", (self.dist / f"hypermesh-visor-headless-{arch}-build.txt").read_text())

    def test_tampered_package_is_rejected(self):
        (self.bundles / "aarch64" / "hypermesh-visor-headless-aarch64.apk").write_bytes(b"tampered")
        self.rejects_without_changing_desktop()

    def test_wrong_source_is_rejected_even_with_valid_hashes(self):
        source = self.bundles / "aarch64"
        path = source / "build.txt"
        path.write_text(path.read_text().replace(REVISION, "c" * 40))
        self.manifest(source)
        self.rejects_without_changing_desktop()

    def test_wrong_architecture_or_conductor_mismatch_is_rejected(self):
        source = self.bundles / "aarch64"
        path = source / "build.txt"
        original = path.read_text()
        for invalid in [original.replace("architecture=aarch64", "architecture=x86_64"), original.replace("b" * 40, "c" * 40)]:
            path.write_text(invalid)
            self.manifest(source)
            self.rejects_without_changing_desktop()

    def test_missing_architecture_is_rejected(self):
        (self.bundles / "aarch64" / "checksums.txt").unlink()
        self.rejects_without_changing_desktop()

    def test_parent_paths_and_duplicate_entries_are_rejected(self):
        path = self.bundles / "x86_64" / "checksums.txt"
        original = path.read_text()
        for invalid in [original + original.splitlines()[0] + "\n", original + f"{'a' * 64}  ../private\n"]:
            path.write_text(invalid)
            self.rejects_without_changing_desktop()


if __name__ == "__main__":
    unittest.main()
