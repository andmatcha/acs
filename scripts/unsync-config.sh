#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

target_dir=$(acs_managed_local_config_dir)

if [ -e "$target_dir" ]; then
    rm -rf "$target_dir"
    info "removed synced local config overlay: $target_dir"
else
    info "no synced local config overlay found at $target_dir"
fi
