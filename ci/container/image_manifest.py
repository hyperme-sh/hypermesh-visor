"""Validate a native container build before recording its downloadable archive."""
import hashlib
import json
import re
import sys
from pathlib import Path


def validate(image, architecture, revision):
    if architecture not in {"amd64", "arm64"}:
        raise ValueError("unsupported architecture")
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("source revision must be a full commit")
    if image.get("Os") != "linux" or image.get("Architecture") != architecture:
        raise ValueError("image platform does not match the native runner")
    image_id = image.get("Id", "")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", image_id):
        raise ValueError("invalid image ID")
    config = image.get("Config") or {}
    if config.get("User") != "1000:1000":
        raise ValueError("image must run as the dedicated unprivileged user")
    if (config.get("Labels") or {}).get("org.opencontainers.image.revision") != revision:
        raise ValueError("image source revision mismatch")
    if config.get("Entrypoint") != ["/usr/bin/tini", "--", "/usr/bin/dbus-run-session", "--", "/usr/local/bin/visor-container"]:
        raise ValueError("image entrypoint mismatch")
    if config.get("ExposedPorts") or config.get("Volumes"):
        raise ValueError("image must not introduce implicit ports or volumes")
    if (config.get("Healthcheck") or {}).get("Test") != ["CMD", "visor-health"]:
        raise ValueError("image readiness command missing")
    return {"schema_version": 1, "platform": f"linux/{architecture}", "source_revision": revision, "image_id": image_id}


def main():
    inspect_path, archive_path, architecture, revision, output = sys.argv[1:]
    images = json.loads(Path(inspect_path).read_text())
    if not isinstance(images, list) or len(images) != 1:
        raise ValueError("expected exactly one image")
    manifest = validate(images[0], architecture, revision)
    archive = Path(archive_path)
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    manifest["archive"] = {"name": archive.name, "sha256": digest, "bytes": archive.stat().st_size}
    Path(output).write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
