#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
: "${HYPERMESH_RUNTIME_DIR:=$HOME/.local/state/hypermesh/native-run}"
export HYPERMESH_RUNTIME_DIR
exec "$root/libexec/$(basename -- "$0")" "$@"
