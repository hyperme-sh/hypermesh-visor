#!/usr/bin/env python3
"""Validate and bundle the Android library; never sign or publish it."""
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import struct
import subprocess
import tomllib
import xml.etree.ElementTree as ET
import zipfile

ANDROID = "{http://schemas.android.com/apk/res/android}"
LIBRARY = "jni/arm64-v8a/libhypermesh_visor_mobile.so"
CLASSES = {
    "sh/hyperme/visor/mobile/AndroidSessionStoreKt.class",
    "sh/hyperme/visor/mobile/EncryptedSessionStore.class",
    "uniffi/hypermesh_visor_mobile/MobileSession.class",
    "uniffi/hypermesh_visor_mobile/NativeSessionStore.class",
}


def entries(archive):
    names = archive.namelist()
    if len(names) != len(set(names)):
        raise ValueError("duplicate archive member")
    for entry in archive.infolist():
        path = PurePosixPath(entry.filename)
        if path.is_absolute() or ".." in path.parts or "\\" in entry.filename or (entry.external_attr >> 16) & 0o170000 == 0o120000:
            raise ValueError("unsafe archive member")
    return set(names)


def check_elf(data):
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<HH", data, 16) != (3, 183):
        raise ValueError("expected ARM64 ELF shared library")
    offset = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    if size != 56 or count == 0 or offset + size * count > len(data):
        raise ValueError("invalid ELF program headers")
    loads = 0
    for index in range(count):
        kind, _, file_offset, virtual, _, file_size, memory_size, alignment = struct.unpack_from("<IIQQQQQQ", data, offset + size * index)
        if kind != 1:
            continue
        loads += 1
        if alignment < 16384 or alignment & (alignment - 1) or file_offset % alignment != virtual % alignment or file_offset + file_size > len(data) or memory_size < file_size:
            raise ValueError("ELF load segment lacks 16 KiB compatibility")
    if loads == 0:
        raise ValueError("ELF has no load segments")


def check_aar(path, native):
    expected = native.read_bytes()
    check_elf(expected)
    with zipfile.ZipFile(path) as archive:
        names = entries(archive)
        if {name for name in names if name.endswith(".so")} != {LIBRARY}:
            raise ValueError("unexpected or missing native ABI")
        if archive.read(LIBRARY) != expected:
            raise ValueError("packaged native library differs from the build input")
        with zipfile.ZipFile(io.BytesIO(archive.read("classes.jar"))) as classes:
            if not CLASSES <= entries(classes):
                raise ValueError("native bindings or secure store adapter missing")
        rules = archive.read("proguard.txt").decode("utf-8")
        if "uniffi.hypermesh_visor_mobile.**" not in rules or "com.sun.jna.**" not in rules:
            raise ValueError("native callback consumer rules missing")
        manifest = ET.fromstring(archive.read("AndroidManifest.xml"))
        sdk = manifest.find("uses-sdk")
        if sdk is None or sdk.get(ANDROID + "minSdkVersion") != "28":
            raise ValueError("secure store requires Android API 28")
        # A library must not silently introduce permissions or application components.
        if any(child.tag != "uses-sdk" for child in manifest):
            raise ValueError("unexpected Android permission or component")
    return hashlib.sha256(expected).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("aar", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("bindings", type=Path)
    parser.add_argument("metadata", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    native_hash = check_aar(args.aar, args.native)
    version = tomllib.loads((root / "core/Cargo.toml").read_text())["package"]["version"]
    repo = root.parent
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    binding = args.bindings / "uniffi/hypermesh_visor_mobile/hypermesh_visor_mobile.kt"
    metadata = {
        "version": version, "source_revision": revision, "abi": "arm64-v8a", "minimum_android_api": 28,
        "native_sha256": native_hash, "binding_sha256": hashlib.sha256(binding.read_bytes()).hexdigest(),
        "dependencies": {"net.java.dev.jna:jna@aar": "5.17.0", "org.jetbrains.kotlin:kotlin-stdlib": "2.1.20"},
        "build": {"gradle": "8.11.1", "android_gradle_plugin": "8.9.1", "compile_sdk": 35},
        "signed": False,
    }
    files = {
        "hypermesh-visor-mobile.aar": args.aar.read_bytes(),
        "build.json": (json.dumps(metadata, indent=2) + "\n").encode(),
        "README.md": (root / "android/LIBRARY.md").read_bytes(),
        "LICENSE": (repo / "LICENSE").read_bytes(),
    }
    attribution = []
    for package in sorted(json.loads(args.metadata.read_text())["packages"], key=lambda p: (p["name"], p["version"])):
        directory = Path(package["manifest_path"]).resolve().parent
        prefix = f"licenses/{package['name']}-{package['version']}"
        attribution.append({key: package.get(key) for key in ["name", "version", "license", "repository"]})
        candidates = {path for path in directory.iterdir() if path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE"))}
        if package.get("license_file"):
            candidates.add(directory / package["license_file"])
        for path in sorted(candidates):
            if path.is_file() and path.resolve().is_relative_to(directory):
                files[f"{prefix}/{path.name}"] = path.read_bytes()
    files["licenses/dependencies.json"] = (json.dumps(attribution, indent=2) + "\n").encode()
    files["checksums.txt"] = "".join(f"{hashlib.sha256(value).hexdigest()}  {name}\n" for name, value in sorted(files.items())).encode()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, value in sorted(files.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, value)
    args.output.with_suffix(args.output.suffix + ".sha256").write_text(f"{hashlib.sha256(args.output.read_bytes()).hexdigest()}  {args.output.name}\n")
    print("Android library package contracts passed")


if __name__ == "__main__":
    main()
