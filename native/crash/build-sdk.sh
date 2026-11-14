#!/usr/bin/env bash
set -euo pipefail

# Source-only dependency pin; this script never creates a tag or release.
revision=af9e9367913edf07f54e4f2f9655ce96af0c1681
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
output=${1:?Usage: build-sdk.sh OUTPUT_DIRECTORY}
[[ ! -e "$output" ]] || { echo 'SDK output already exists' >&2; exit 1; }
mkdir -p "$output"
output=$(cd "$output" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git init -q "$work/source"
git -C "$work/source" remote add origin https://github.com/getsentry/sentry-native.git
git -C "$work/source" fetch --no-tags --depth=1 origin "$revision"
git -C "$work/source" checkout -q --detach FETCH_HEAD
[[ $(git -C "$work/source" rev-parse HEAD) == "$revision" ]]
git -C "$work/source" submodule update --init --recursive --depth=1
python3 "$repo/native/crash/prepare-sdk.py" "$work/source"
cargo build --locked --release --manifest-path "$repo/Cargo.toml" \
  -p hypermesh-crash-guard --target-dir "$work/guard-build"
guard="$work/guard-build/release/libhypermesh_crash_guard.a"
shared=ON
if [[ $(uname -s) == Darwin ]]; then shared=OFF; fi
if [[ "${RUNNER_OS:-}" == Windows || "${OS:-}" == Windows_NT ]]; then
  guard=$(cygpath -m "$work/guard-build/release/hypermesh_crash_guard.lib")
fi

# Crashpad has its own sender. Runtime must also use an empty native DSN and
# permanently deny native SDK upload consent; only sanitized JSON uses GlitchTip.
cmake -S "$work/source" -B "$work/build" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$output" \
  -DVISOR_CAPTURE_GUARD="$guard" \
  -DSENTRY_BUILD_SHARED_LIBS="$shared" -DSENTRY_BACKEND=crashpad \
  -DSENTRY_TRANSPORT=none -DSENTRY_BUILD_TESTS=OFF \
  -DSENTRY_BUILD_EXAMPLES=OFF -DSENTRY_BUILD_BENCHMARKS=OFF \
  -DSENTRY_ENABLE_INSTALL=ON -DSENTRY_INTEGRATION_PLATFORM=OFF \
  -DSENTRY_THREAD_STACK_GUARANTEE_AUTO_INIT=OFF
cmake --build "$work/build" --config Release --parallel 2
cmake --install "$work/build" --config Release
if [[ $(uname -s) == Darwin ]]; then
  python3 - "$output/lib" <<'PY'
from pathlib import Path
import subprocess, sys
root = Path(sys.argv[1])
archives = sorted(root.rglob('*.a'))
if not archives:
    raise SystemExit('No static SDK archives installed')
subprocess.run(['libtool', '-static', '-o', str(root / 'libvisor_crash_sdk.a'), *map(str, archives)], check=True)
PY
fi
printf '%s\n' "$revision" > "$output/source-revision"
if git -C "$repo" rev-parse HEAD > "$output/visor-source-revision" 2>/dev/null; then
  :
else
  # Archive/container contexts omit Git metadata. Record the exact source tree
  # bytes instead; the SDK and application compile in the same build context.
  python3 - "$repo" > "$output/visor-source-revision" <<'PYHASH'
import hashlib, pathlib, sys
root = pathlib.Path(sys.argv[1])
hash = hashlib.sha256()
paths = [root / 'Cargo.toml', root / 'Cargo.lock']
for directory in ('src', 'native/crash', 'local-storage'):
    paths.extend(p for p in (root / directory).rglob('*') if p.is_file())
for path in sorted(paths):
    relative = path.relative_to(root).as_posix().encode()
    data = path.read_bytes()
    hash.update(len(relative).to_bytes(8, 'big') + relative)
    hash.update(len(data).to_bytes(8, 'big') + data)
print('source-tree-sha256:' + hash.hexdigest())
PYHASH
fi
git -C "$work/source" submodule status --recursive > "$output/submodule-revisions"
while IFS= read -r file; do
  case "${file##*/}" in
    LICENSE*|COPYING*|NOTICE*|COPYRIGHT*)
      [[ -f "$work/source/$file" && ! -L "$work/source/$file" ]] || continue
      mkdir -p "$output/licenses/$(dirname "$file")"
      cp "$work/source/$file" "$output/licenses/$file"
      ;;
  esac
done < <(git -C "$work/source" ls-files --recurse-submodules)

mkdir -p "$output/licenses/visor"
cp "$repo/LICENSE" "$output/licenses/visor/LICENSE"
