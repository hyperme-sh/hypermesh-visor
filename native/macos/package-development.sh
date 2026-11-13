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
# Match the verified Conductor source used by the Linux downloads.
conductor_revision=$(sed -n 's/^conductor_revision=//p' packaging/package.sh)
conductor_sha256=$(sed -n 's/^conductor_sha256=//p' packaging/package.sh)
[[ $conductor_revision =~ ^[a-f0-9]{40}$ && $conductor_sha256 =~ ^[a-f0-9]{64}$ ]] || { echo 'Invalid Conductor source pin' >&2; exit 1; }
source_dir=$(mktemp -d)
trap 'rm -rf "$source_dir"' EXIT
curl --fail --silent --show-error --location "https://codeload.github.com/hyperme-sh/hypermesh-conductor/tar.gz/$conductor_revision" -o "$source_dir/source.tar.gz"
printf '%s  %s\n' "$conductor_sha256" "$source_dir/source.tar.gz" | shasum -a 256 -c -
tar -xzf "$source_dir/source.tar.gz" -C "$source_dir" --strip-components=1
(cd "$source_dir" && CGO_ENABLED=0 go build -mod=readonly -buildvcs=false -trimpath -o "$bundle/libexec/conductor" ./cmd/conductor)
printf '%s\n' "$conductor_revision" > "$bundle/conductor-source-revision"
printf '%s\n' "$conductor_sha256" > "$bundle/conductor-source-archive.sha256"
for program in hypermesh hypermesh-desktop-mcp conductor; do
  cp native/macos/launch-tool.sh "$bundle/bin/$program"
  chmod 755 "$bundle/bin/$program"
done
cp native/macos/DISTRIBUTION.md "$bundle/README.md"
printf '%s\n' "$revision" > "$bundle/source-revision"
python3 native/macos/test-package.py --manifest "$bundle"
python3 native/macos/test-package.py "$bundle"
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$output/hypermesh-visor-macos-arm64.zip"
(cd "$output" && shasum -a 256 hypermesh-visor-macos-arm64.zip > hypermesh-visor-macos-arm64.zip.sha256)
