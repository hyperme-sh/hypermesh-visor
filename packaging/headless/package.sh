#!/bin/sh
set -eu
case "$EXPECTED_ARCH" in
  x86_64) NFPM_ARCH=amd64 ;;
  aarch64) NFPM_ARCH=arm64 ;;
  *) echo 'unsupported native architecture' >&2; exit 1 ;;
esac
case "$SOURCE_REVISION" in
  ''|*[!0-9a-f]*) echo 'source revision must be a full Git SHA' >&2; exit 1 ;;
esac
[ "${#SOURCE_REVISION}" -eq 40 ]
VERSION=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
export NFPM_ARCH VERSION
mkdir -p /output /bundle/root/usr/bin /bundle/root/usr/lib/hypermesh-visor-headless /bundle/root/usr/share/doc/hypermesh-visor-headless
cp target/release/hypermesh-visor /bundle/root/usr/bin/hypermesh-visor-headless
cp /bundle/conductor /bundle/root/usr/lib/hypermesh-visor-headless/conductor
cp /bundle/conductor-LICENSE /bundle/root/usr/share/doc/hypermesh-visor-headless/
cp LICENSE /bundle/root/usr/share/doc/hypermesh-visor-headless/
cp docs/headless.md /bundle/root/usr/share/doc/hypermesh-visor-headless/README.md
for format in apk deb rpm; do
  nfpm package --config packaging/headless/nfpm.yaml --packager "$format" --target "/output/hypermesh-visor-headless-${EXPECTED_ARCH}.${format}"
done
tar czf "/output/hypermesh-visor-headless-${EXPECTED_ARCH}.tar.gz" -C /bundle/root .
printf 'visor=%s\nconductor=%s\narchitecture=%s\n' "$SOURCE_REVISION" 8c381046952863e8ba83f054d467402c959eb59a "$EXPECTED_ARCH" > /output/build.txt
cd /output
sha256sum ./*.apk ./*.deb ./*.rpm ./*.tar.gz build.txt > checksums.txt
