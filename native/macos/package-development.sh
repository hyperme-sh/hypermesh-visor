#!/bin/bash
# CI development archive only; this does not create a tag or release.
set -euo pipefail
cd "$(dirname "$0")/../.."
output=${1:?Usage: package-development.sh OUTPUT_DIRECTORY}
[[ ! -e "$output" ]] || { echo 'Output already exists' >&2; exit 1; }
[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || { echo 'The development distribution currently supports Apple Silicon only' >&2; exit 1; }
revision=$(git rev-parse HEAD)
mkdir -p "$output"
bundle="$output/hypermesh-visor-macos-arm64"
mkdir -p "$bundle/bin" "$bundle/libexec"
bash native/macos/build-app.sh "$bundle/Hypermesh Visor Development.app"
cargo build --locked --bin hypermesh-desktop-mcp
cp target/debug/hypermesh-desktop-mcp "$bundle/libexec/"
# Resolve the destination before entering the vendored CLI source directory.
bundle=$(cd "$bundle" && pwd)
(cd third_party/hypermesh-cli && CGO_ENABLED=0 go build -mod=readonly -buildvcs=false -trimpath -o "$bundle/libexec/hypermesh" ./cmd/hypermesh)
for program in hypermesh hypermesh-desktop-mcp; do
  cp native/macos/launch-tool.sh "$bundle/bin/$program"
  chmod 755 "$bundle/bin/$program"
done
cp native/macos/DISTRIBUTION.md "$bundle/README.md"
printf '%s\n' "$revision" > "$bundle/source-revision"
python3 native/macos/test-package.py --manifest "$bundle"
python3 native/macos/test-package.py "$bundle"
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$output/hypermesh-visor-macos-arm64.zip"
(cd "$output" && shasum -a 256 hypermesh-visor-macos-arm64.zip > hypermesh-visor-macos-arm64.zip.sha256)
