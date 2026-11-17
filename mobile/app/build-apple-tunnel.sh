#!/usr/bin/env bash
# CI/native SDK builds only. Produce linkable archives; no signing or VPN startup.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source_dir="$root/mobile/apple/tunnel/Vendor/wireguard-apple/Sources/WireGuardKitGo"
for platform in iphoneos iphonesimulator; do
  suffix=""
  if [[ "$platform" == iphonesimulator ]]; then suffix="-simulator"; fi
  make -C "$source_dir" ARCHS=arm64 PLATFORM_NAME="$platform" GOOS_iphonesimulator=ios \
    CC="$(xcrun --sdk "$platform" --find clang) -target arm64-apple-ios15.0$suffix" \
    CONFIGURATION_TEMP_DIR="${RUNNER_TEMP:?}/visor-wireguard-$platform" \
    CONFIGURATION_BUILD_DIR="$root/mobile/app/ios/App/Native/WireGuard/$platform" build
 done
