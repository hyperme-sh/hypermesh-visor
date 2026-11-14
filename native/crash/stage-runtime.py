"""Stage a matching SDK's runtime files without headers or static archives."""
import argparse
from pathlib import Path
import shutil

parser = argparse.ArgumentParser()
parser.add_argument("source", type=Path)
parser.add_argument("destination", type=Path)
parser.add_argument("--platform", choices=("linux", "macos", "windows"), required=True)
parser.add_argument("--revision", required=True)
args = parser.parse_args()
source = args.source.resolve(strict=True)
if (source / "visor-source-revision").read_text().strip() != args.revision:
    raise SystemExit("Native SDK source does not match the application build")
if args.destination.exists():
    raise SystemExit("Native SDK runtime destination already exists")
files = [Path("source-revision"), Path("visor-source-revision")]
files.append(Path("bin/crashpad_handler.exe" if args.platform == "windows" else "bin/crashpad_handler"))
if args.platform == "windows":
    files.append(Path("bin/sentry.dll"))
elif args.platform == "linux":
    libraries = sorted((source / "lib").glob("libsentry.so*"))
    if not libraries:
        raise SystemExit("Native SDK shared library is missing")
    files.extend(path.relative_to(source) for path in libraries)
licenses = source / "licenses"
if not licenses.is_dir():
    raise SystemExit("Native SDK licenses are missing")
files.extend(path.relative_to(source) for path in sorted(licenses.rglob("*")) if path.is_file())
for relative in files:
    path = source / relative
    if not path.is_file() or not path.resolve(strict=True).is_relative_to(source):
        raise SystemExit("Unsafe or missing native SDK runtime file")
args.destination.mkdir(parents=True)
for relative in files:
    target = args.destination / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / relative, target)
    target.chmod(0o755 if relative.parts[0] in ("bin", "lib") else 0o644)
