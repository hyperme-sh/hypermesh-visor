#!/usr/bin/env python3
"""Check both native libraries in the unsigned, build-only APK consumer."""
import argparse
from pathlib import Path
import struct
import zipfile

from package import LIBRARY, check_elf, entries

NATIVE = {"lib/arm64-v8a/libhypermesh_visor_mobile.so", "lib/arm64-v8a/libjnidispatch.so"}


def check_consumer(apk, aar):
    with zipfile.ZipFile(aar) as library, zipfile.ZipFile(apk) as archive:
        names = entries(archive)
        if {name for name in names if name.endswith(".so")} != NATIVE:
            raise ValueError("consumer must contain only ARM64 Visor and JNA libraries")
        if not {"AndroidManifest.xml", "classes.dex"} <= names:
            raise ValueError("consumer manifest or compiled code missing")
        if archive.read("lib/arm64-v8a/libhypermesh_visor_mobile.so") != library.read(LIBRARY):
            raise ValueError("consumer changed the packaged Visor native library")
        with apk.open("rb") as raw:
            for name in sorted(NATIVE):
                check_elf(archive.read(name))
                info = archive.getinfo(name)
                if info.compress_type != zipfile.ZIP_STORED:
                    raise ValueError("consumer native libraries must be uncompressed")
                raw.seek(info.header_offset)
                header = raw.read(30)
                if len(header) != 30 or header[:4] != b"PK\x03\x04":
                    raise ValueError("invalid native library ZIP header")
                filename_length, extra_length = struct.unpack_from("<HH", header, 26)
                if (info.header_offset + 30 + filename_length + extra_length) % 16384:
                    raise ValueError("native library ZIP offset lacks 16 KiB alignment")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("apk", type=Path)
    parser.add_argument("aar", type=Path)
    args = parser.parse_args()
    check_consumer(args.apk, args.aar)
    print("Android AAR consumer compilation and native packaging contracts passed")
