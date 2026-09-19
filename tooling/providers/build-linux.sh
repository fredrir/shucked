#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || { echo 'Linux target host required' >&2; exit 1; }
exec "$(dirname -- "$0")/build-unix.sh" "$@"
