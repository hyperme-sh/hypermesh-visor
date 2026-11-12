import copy
import unittest
from image_manifest import validate

REVISION = "a" * 40
IMAGE = {
    "Id": "sha256:" + "b" * 64,
    "Os": "linux", "Architecture": "arm64",
    "Config": {
        "User": "1000:1000",
        "Labels": {"org.opencontainers.image.revision": REVISION},
        "Entrypoint": ["/usr/bin/tini", "--", "/usr/bin/dbus-run-session", "--", "/usr/local/bin/visor-container"],
        "Healthcheck": {"Test": ["CMD", "visor-health"]},
    },
}


class ManifestContract(unittest.TestCase):
    def test_native_platform_and_source_are_recorded(self):
        for architecture in ("amd64", "arm64"):
            image = copy.deepcopy(IMAGE)
            image["Architecture"] = architecture
            result = validate(image, architecture, REVISION)
            self.assertEqual(result["platform"], f"linux/{architecture}")
            self.assertEqual(result["image_id"], image["Id"])
            self.assertEqual(result["source_revision"], REVISION)

    def test_wrong_platform_or_revision_is_refused(self):
        for image in ({**IMAGE, "Architecture": "amd64"}, {**IMAGE, "Os": "windows"}, {**IMAGE, "Id": "latest"}):
            with self.assertRaises(ValueError):
                validate(image, "arm64", REVISION)
        for revision in ("main", "c" * 40):
            with self.assertRaises(ValueError):
                validate(IMAGE, "arm64", revision)

    def test_weakened_runtime_contract_is_refused(self):
        for key, value in (("User", "root"), ("User", ""), ("Entrypoint", ["sh"]), ("Healthcheck", {}), ("ExposedPorts", {"80/tcp": {}}), ("Volumes", {"/var/run/docker.sock": {}}), ("Labels", {})):
            with self.subTest(key=key, value=value):
                image = copy.deepcopy(IMAGE)
                image["Config"][key] = value
                with self.assertRaises(ValueError):
                    validate(image, "arm64", REVISION)


if __name__ == "__main__":
    unittest.main()
