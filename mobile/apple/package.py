#!/usr/bin/env python3
"""Build an unsigned device/simulator XCFramework and link its Swift consumers."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import plistlib
import shutil
import subprocess
import tempfile
import tomllib
import zipfile

LIBRARY = "libhypermesh_visor_mobile.a"
HEADER = "hypermesh_visor_mobileFFI.h"
MODULE = "hypermesh_visor_mobileFFI.modulemap"
SLICES = {"ios-arm64": (None, "iphoneos", "arm64-apple-ios15.0"),
          "ios-arm64-simulator": ("simulator", "iphonesimulator", "arm64-apple-ios15.0-simulator")}


def check_framework(framework, originals, bindings):
    info = plistlib.loads((framework / "Info.plist").read_bytes())
    libraries = info.get("AvailableLibraries", [])
    if len(libraries) != 2 or {item.get("LibraryIdentifier") for item in libraries} != set(SLICES):
        raise ValueError("expected one ARM64 device and one ARM64 simulator slice")
    hashes = {}
    for item in libraries:
        name = item["LibraryIdentifier"]
        variant = SLICES[name][0]
        if item.get("SupportedArchitectures") != ["arm64"] or item.get("SupportedPlatform") != "ios" or item.get("SupportedPlatformVariant") != variant:
            raise ValueError("incorrect Apple platform or architecture")
        if item.get("LibraryPath") != LIBRARY or item.get("HeadersPath") != "Headers":
            raise ValueError("unexpected library or headers path")
        native = (framework / name / LIBRARY).read_bytes()
        if native != originals[name].read_bytes() or not native.startswith(b"!<arch>\n"):
            raise ValueError("packaged static library differs from the build input")
        hashes[name] = hashlib.sha256(native).hexdigest()
        for packaged, original in [(HEADER, HEADER), ("module.modulemap", MODULE)]:
            if (framework / name / "Headers" / packaged).read_bytes() != (bindings / original).read_bytes():
                raise ValueError("FFI headers differ from generated bindings")
    return hashes


def main():
    parser = argparse.ArgumentParser()
    for name in ["bindings", "device", "simulator", "metadata", "output"]:
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    repo = root.parent
    originals = {"ios-arm64": args.device.resolve(), "ios-arm64-simulator": args.simulator.resolve()}
    with tempfile.TemporaryDirectory(prefix="visor-ios-library-") as temporary:
        work = Path(temporary)
        bundle = work / "bundle"
        bundle.mkdir()
        headers = work / "headers"
        headers.mkdir()
        shutil.copyfile(args.bindings / HEADER, headers / HEADER)
        shutil.copyfile(args.bindings / MODULE, headers / "module.modulemap")
        framework = bundle / "HypermeshVisorMobile.xcframework"
        subprocess.run(["xcodebuild", "-create-xcframework", "-library", str(originals["ios-arm64"]), "-headers", str(headers),
                        "-library", str(originals["ios-arm64-simulator"]), "-headers", str(headers), "-output", str(framework)], check=True)
        hashes = check_framework(framework, originals, args.bindings)
        sources = bundle / "Sources"
        sources.mkdir()
        shutil.copyfile(args.bindings / "hypermesh_visor_mobile.swift", sources / "hypermesh_visor_mobile.swift")
        shutil.copyfile(root / "apple/KeychainSessionStore.swift", sources / "KeychainSessionStore.swift")
        for name, (_, sdk_name, target) in SLICES.items():
            directory = framework / name
            architectures = subprocess.check_output(["xcrun", "lipo", "-archs", str(directory / LIBRARY)], text=True).strip()
            if architectures != "arm64":
                raise ValueError("unexpected static library architecture")
            sdk = subprocess.check_output(["xcrun", "--sdk", sdk_name, "--show-sdk-path"], text=True).strip()
            # Compile and link the packaged files, without running an app or accessing Keychain.
            subprocess.run(["swiftc", "-parse-as-library", "-emit-library", "-module-name", "VisorMobileContract",
                            "-target", target, "-sdk", sdk, "-I", str(directory / "Headers"),
                            str(sources / "hypermesh_visor_mobile.swift"), str(sources / "KeychainSessionStore.swift"),
                            "-L", str(directory), "-lhypermesh_visor_mobile", "-framework", "Security",
                            "-framework", "LocalAuthentication", "-framework", "SystemConfiguration",
                            "-framework", "CoreFoundation", "-lresolv", "-liconv",
                            "-o", str(work / (name + "-consumer.dylib"))], check=True)
        version = tomllib.loads((root / "core/Cargo.toml").read_text())["package"]["version"]
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
        metadata = {"version": version, "source_revision": revision, "minimum_ios": "15.0", "signed": False,
                    "native_sha256": hashes,
                    "binding_sha256": hashlib.sha256((sources / "hypermesh_visor_mobile.swift").read_bytes()).hexdigest()}
        (bundle / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
        shutil.copyfile(root / "apple/LIBRARY.md", bundle / "README.md")
        shutil.copyfile(repo / "LICENSE", bundle / "LICENSE")
        attribution = []
        for package in sorted(json.loads(args.metadata.read_text())["packages"], key=lambda p: (p["name"], p["version"])):
            directory = Path(package["manifest_path"]).resolve().parent
            attribution.append({key: package.get(key) for key in ["name", "version", "license", "repository"]})
            candidates = {path for path in directory.iterdir() if path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE"))}
            if package.get("license_file"):
                candidates.add(directory / package["license_file"])
            for path in sorted(candidates):
                if path.is_file() and path.resolve().is_relative_to(directory):
                    destination = bundle / "licenses" / f"{package['name']}-{package['version']}" / path.name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, destination)
        (bundle / "licenses").mkdir(exist_ok=True)
        (bundle / "licenses/dependencies.json").write_text(json.dumps(attribution, indent=2) + "\n")
        files = sorted(path for path in bundle.rglob("*") if path.is_file())
        if any(path.is_symlink() for path in bundle.rglob("*")):
            raise ValueError("unexpected package symlink")
        (bundle / "checksums.txt").write_text("".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(bundle).as_posix()}\n" for path in files))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(args.output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(path for path in bundle.rglob("*") if path.is_file()):
                name = path.relative_to(bundle).as_posix()
                if ".." in PurePosixPath(name).parts:
                    raise ValueError("unsafe package path")
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.external_attr = 0o100644 << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, path.read_bytes())
        args.output.with_suffix(args.output.suffix + ".sha256").write_text(f"{hashlib.sha256(args.output.read_bytes()).hexdigest()}  {args.output.name}\n")
    print("Apple mobile library package and consumer contracts passed")


if __name__ == "__main__":
    main()
