#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
output=${1:?Usage: build-app.sh OUTPUT_APP_PATH}
if [[ -e "$output" ]]; then echo "Output already exists" >&2; exit 1; fi
sdk=${HYPERMESH_CRASH_SDK_PATH:?Matching native crash SDK artifact is required}
[[ -f "$sdk/lib/libvisor_crash_sdk.a" ]] || { echo 'Static native crash SDK is missing' >&2; exit 1; }
[[ $(cat "$sdk/visor-source-revision") == "$(git rev-parse HEAD)" ]] || { echo 'Native crash SDK source mismatch' >&2; exit 1; }
deployment=$(/usr/bin/sw_vers -productVersion)
MACOSX_DEPLOYMENT_TARGET="$deployment" cargo rustc --locked --features native-crash-static --lib --crate-type staticlib
mkdir -p "$output/Contents/MacOS" "$output/Contents/Resources/companion"
for state in idle automation disconnected; do
  count=8
  if [[ "$state" == disconnected ]]; then count=6; fi
  for ((frame=0; frame<count; frame++)); do
    cp "companion/assets/states/${state}_${frame}_128.png" "$output/Contents/Resources/companion/"
  done
done
swiftc -parse-as-library -D EMBEDDED_ENGINE -O -target "$(uname -m)-apple-macos$deployment" -import-objc-header native/macos/Engine.h native/macos/Desktop.swift native/macos/Engine.swift \
  -L target/debug -lhypermesh_visor "$sdk/lib/libvisor_crash_sdk.a" -lc++ -lbsm -framework IOKit -framework Foundation -framework Security -framework SystemConfiguration \
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
python3 native/crash/stage-runtime.py "$sdk" "$output/Contents/Resources/crash-sdk" --platform macos --revision "$(git rev-parse HEAD)"
mv "$output/Contents/Resources/crash-sdk/bin/crashpad_handler" "$output/Contents/MacOS/crashpad_handler"
rmdir "$output/Contents/Resources/crash-sdk/bin"
codesign --force --sign - --options runtime --identifier sh.hyperme.visor.crash-handler "$output/Contents/MacOS/crashpad_handler"
# Hardened local ad-hoc build. No debug, library-validation, or DYLD exceptions.
codesign --force --sign - --options runtime --identifier sh.hyperme.visor.development "$output"
codesign --verify --strict "$output"
