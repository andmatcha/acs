#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

load_cargo_env
require_command cargo
require_command git

BUILD_BRANCH=$(git_branch_name "$REPO_ROOT")
BUILD_COMMIT=$(git_commit_id "$REPO_ROOT")
BUILD_DIRTY=$(git_dirty_state "$REPO_ROOT")
BUILD_SOURCE_REF=$(source_ref_for_branch "$BUILD_BRANCH" "$BUILD_COMMIT")

info "syncing the current working tree to the global $ACS_NAME binary"

install_binary_from_source \
    "$REPO_ROOT" \
    1 \
    "$BUILD_BRANCH" \
    "$BUILD_COMMIT" \
    "$BUILD_DIRTY" \
    "working-tree" \
    "$BUILD_SOURCE_REF"

ensure_standard_global_layout
info "left existing global config files untouched"
print_global_install_summary
