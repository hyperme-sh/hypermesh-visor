"""Runtime file-selection contracts; fixture files are never executed."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("stage-runtime.py")
REVISION = "a" * 40


class StageRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "sdk"
        for directory in ("bin", "lib", "licenses"):
            (self.source / directory).mkdir(parents=True, exist_ok=True)
        for name in ("bin/crashpad_handler", "bin/crashpad_handler.exe", "bin/sentry.dll",
                     "bin/crashpad_wer.dll", "lib/libsentry.so", "lib/libvisor_crash_sdk.a",
                     "licenses/LICENSE", "source-revision"):
            (self.source / name).write_bytes(b"fixture")
        (self.source / "visor-source-revision").write_text(REVISION + "\n")

    def run_stage(self, platform, revision=REVISION):
        target = self.root / platform
        result = subprocess.run([sys.executable, str(SCRIPT), str(self.source), str(target),
                                 "--platform", platform, "--revision", revision],
                                capture_output=True, text=True)
        return result, target

    def test_platform_runtime_excludes_archives_and_wer(self):
        for platform, binary in (("linux", "bin/crashpad_handler"),
                                 ("macos", "bin/crashpad_handler"),
                                 ("windows", "bin/crashpad_handler.exe")):
            result, target = self.run_stage(platform)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((target / binary).is_file())
            self.assertTrue((target / "licenses/LICENSE").is_file())
            self.assertFalse((target / "bin/crashpad_wer.dll").exists())
            self.assertFalse((target / "lib/libvisor_crash_sdk.a").exists())
            if platform == "macos":
                self.assertFalse((target / "lib").exists())

    def test_source_mismatch_refuses_before_staging(self):
        result, target = self.run_stage("linux", "b" * 40)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(target.exists())

    @unittest.skipIf(os.name == "nt", "Unix symlink fixture")
    def test_external_library_link_is_refused(self):
        outside = self.root / "outside"
        outside.write_bytes(b"excluded")
        library = self.source / "lib/libsentry.so"
        library.unlink()
        library.symlink_to(outside)
        result, target = self.run_stage("linux")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(target.exists())


if __name__ == "__main__":
    unittest.main()
