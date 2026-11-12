#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
app="$HOME/Applications/Hypermesh Visor Development.app"
if [[ -e "$app" ]]; then
  echo "Development app already exists: $app. Quit and remove that development bundle explicitly before reinstalling." >&2
  exit 1
fi
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
bundle="$stage/Hypermesh Visor Development.app"
bash native/macos/build-app.sh "$bundle"
mkdir -p "$HOME/Applications"
mv "$bundle" "$app"
open "$app" --args --setup
printf 'Installed development app: %s\n' "$app"
