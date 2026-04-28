#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PURGE=0

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --purge)
            PURGE=1
            shift
            ;;
        *)
            fail "unknown option for uninstall.sh: $1"
            ;;
    esac
done

load_cargo_env
require_command cargo

if cargo uninstall "$ACS_NAME"; then
    info "removed the global $ACS_NAME binary"
else
    info "the global $ACS_NAME binary was not installed"
fi

LOG_DIR=$(acs_log_dir)

if [ "$PURGE" -eq 1 ]; then
    rm -rf "$LOG_DIR"
    info "removed $LOG_DIR"
else
    info "kept log dir: $LOG_DIR"
fi
