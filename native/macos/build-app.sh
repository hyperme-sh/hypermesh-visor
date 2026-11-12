#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
output=${1:?Usage: build-app.sh OUTPUT_APP_PATH}
if [[ -e "$output" ]]; then echo "Output already exists" >&2; exit 1; fi
deployment=$(/usr/bin/sw_vers -productVersion)
MACOSX_DEPLOYMENT_TARGET="$deployment" cargo build --locked --lib
mkdir -p "$output/Contents/MacOS"
swiftc -parse-as-library -D EMBEDDED_ENGINE -O -target "$(uname -m)-apple-macos$deployment" -import-objc-header native/macos/Engine.h native/macos/Desktop.swift native/macos/Engine.swift \
  -L target/debug -lhypermesh_visor -framework Security -framework SystemConfiguration \
  -framework CoreFoundation -lresolv -liconv -o "$output/Contents/MacOS/hypermesh-native-desktop"
cat > "$output/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>sh.hyperme.visor.development</string>
<key>CFBundleName</key><string>Hypermesh Visor Development</string>
<key>CFBundleDisplayName</key><string>Hypermesh Visor Development</string>
<key>CFBundleExecutable</key><string>hypermesh-native-desktop</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0-dev</string>
<key>LSMinimumSystemVersion</key><string>$deployment</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
# Hardened local ad-hoc build. No debug, library-validation, or DYLD exceptions.
codesign --force --sign - --options runtime --identifier sh.hyperme.visor.development "$output"
codesign --verify --strict "$output"
