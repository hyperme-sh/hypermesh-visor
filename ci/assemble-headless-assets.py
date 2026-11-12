#!/usr/bin/env python3
"""Verify native headless bundles before adding them to release assets."""
import argparse
import hashlib
from pathlib import Path
import re
import shutil

ARCHES = ("x86_64", "aarch64")
SUFFIXES = ("", ".apk", ".deb", ".rpm", ".tar.gz")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify(directory, expected):
    entries = {}
    for line in (directory / "checksums.txt").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64}) [ *](?:\./)?([A-Za-z0-9_.-]+)", line)
        if not match or match[2] in entries or match[2] in (".", ".."):
            raise ValueError("invalid or duplicate checksum entry")
        entries[match[2]] = match[1]
    if set(entries) != expected:
        raise ValueError("unexpected asset manifest")
    for name, checksum in entries.items():
        path = directory / name
        if path.is_symlink() or not path.is_file() or digest(path) != checksum:
            raise ValueError("asset checksum mismatch")


def assemble(dist, bundles, revision):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("expected a full source revision")
    desktop = {f"hypermesh-visor-{arch}{suffix}" for arch in ARCHES for suffix in ("", ".deb", ".rpm")}
    verify(dist, desktop)
    copies = []
    conductor = None
    for arch in ARCHES:
        source = bundles / arch
        names = {f"hypermesh-visor-headless-{arch}{suffix}" for suffix in SUFFIXES}
        verify(source, names | {"build.txt"})
        lines = (source / "build.txt").read_text().splitlines()
        metadata = dict(line.split("=", 1) for line in lines)
        if len(lines) != 3 or set(metadata) != {"visor", "conductor", "architecture"}:
            raise ValueError("invalid bundle metadata")
        if metadata["visor"] != revision or metadata["architecture"] != arch:
            raise ValueError("bundle source or architecture mismatch")
        if not re.fullmatch(r"[0-9a-f]{40}", metadata["conductor"]):
            raise ValueError("missing Conductor source revision")
        if conductor is not None and metadata["conductor"] != conductor:
            raise ValueError("Conductor revisions differ between architectures")
        conductor = metadata["conductor"]
        copies.extend((source / name, dist / name) for name in sorted(names))
        copies.append((source / "build.txt", dist / f"hypermesh-visor-headless-{arch}-build.txt"))
    if any(target.exists() for _, target in copies):
        raise ValueError("refusing to overwrite an asset")
    # Validate both architectures and desktop bytes before modifying the output.
    for source, target in copies:
        shutil.copyfile(source, target)
    names = desktop | {target.name for _, target in copies}
    (dist / "checksums.txt").write_text("".join(f"{digest(dist / name)}  {name}\n" for name in sorted(names)))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path)
    parser.add_argument("bundles", type=Path)
    parser.add_argument("revision")
    args = parser.parse_args()
    assemble(args.dist, args.bundles, args.revision)
