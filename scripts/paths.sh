#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

print_path_summary
