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
cp target/release/hypermesh-visor "/output/hypermesh-visor-headless-${EXPECTED_ARCH}"
tar czf "/output/hypermesh-visor-headless-${EXPECTED_ARCH}.tar.gz" -C /bundle/root .
printf 'visor=%s\nconductor=%s\narchitecture=%s\n' "$SOURCE_REVISION" 75ee07d4401232c0a8a5e1946dd5de1043b5d723 "$EXPECTED_ARCH" > /output/build.txt
cd /output
sha256sum "hypermesh-visor-headless-${EXPECTED_ARCH}" ./*.apk ./*.deb ./*.rpm ./*.tar.gz build.txt > checksums.txt
