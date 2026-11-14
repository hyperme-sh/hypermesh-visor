#!/usr/bin/env bash
set -euo pipefail

# Source-only dependency pin; this script never creates a tag or release.
revision=af9e9367913edf07f54e4f2f9655ce96af0c1681
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

# Crashpad has its own sender. Runtime must also use an empty native DSN and
# permanently deny native SDK upload consent; only sanitized JSON uses GlitchTip.
cmake -S "$work/source" -B "$work/build" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$output" \
  -DSENTRY_BUILD_SHARED_LIBS=ON -DSENTRY_BACKEND=crashpad \
  -DSENTRY_TRANSPORT=none -DSENTRY_BUILD_TESTS=OFF \
  -DSENTRY_BUILD_EXAMPLES=OFF -DSENTRY_BUILD_BENCHMARKS=OFF \
  -DSENTRY_ENABLE_INSTALL=ON -DSENTRY_INTEGRATION_PLATFORM=OFF \
  -DSENTRY_THREAD_STACK_GUARANTEE_AUTO_INIT=OFF
cmake --build "$work/build" --config Release --parallel 2
cmake --install "$work/build" --config Release
printf '%s\n' "$revision" > "$output/source-revision"
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
