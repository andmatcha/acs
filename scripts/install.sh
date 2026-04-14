#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
TAG=""
BRANCH=""
COMMIT=""

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --tag)
            [ "$#" -ge 2 ] || fail "missing value for --tag"
            TAG=$2
            shift 2
            ;;
        --branch)
            [ "$#" -ge 2 ] || fail "missing value for --branch"
            BRANCH=$2
            shift 2
            ;;
        --commit)
            [ "$#" -ge 2 ] || fail "missing value for --commit"
            COMMIT=$2
            shift 2
            ;;
        *)
            fail "unknown option for install.sh: $1"
            ;;
    esac
done

require_clean_ref_selection "$TAG" "$BRANCH" "$COMMIT"
load_cargo_env
require_command cargo
require_command git

if acs_is_globally_installed; then
    info "$ACS_NAME is already installed at $(acs_global_binary_path)"
    info "installed build: $(installed_version_line)"
    info "use \`make update\` to replace the global build"
    exit 0
fi

TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/acs-install.XXXXXX")
trap 'rm -rf "$TMP_ROOT"' EXIT INT TERM HUP
SOURCE_DIR="$TMP_ROOT/source"

if [ -n "$TAG" ]; then
    info "installing $ACS_NAME from remote tag $TAG"
    clone_remote_tag_source "$TAG" "$SOURCE_DIR"
    BUILD_BRANCH="detached"
    BUILD_SOURCE_KIND="remote-tag"
    BUILD_SOURCE_REF="$TAG"
elif [ -n "$BRANCH" ]; then
    info "installing $ACS_NAME from remote branch $BRANCH"
    clone_remote_branch_source "$BRANCH" "$SOURCE_DIR"
    BUILD_BRANCH="$BRANCH"
    BUILD_SOURCE_KIND="remote-branch"
    BUILD_SOURCE_REF="$BRANCH"
elif [ -n "$COMMIT" ]; then
    info "installing $ACS_NAME from remote commit $COMMIT"
    clone_remote_commit_source "$COMMIT" "$SOURCE_DIR"
    BUILD_BRANCH="detached"
    BUILD_SOURCE_KIND="remote-commit"
    BUILD_SOURCE_REF="$COMMIT"
else
    BUILD_BRANCH=$(git_branch_name "$REPO_ROOT")
    info "installing $ACS_NAME from local committed source on branch $BUILD_BRANCH"
    clone_local_head_source "$REPO_ROOT" "$SOURCE_DIR"
    BUILD_SOURCE_KIND="local-commit"
fi

BUILD_COMMIT=$(git_commit_id "$SOURCE_DIR")
BUILD_DIRTY="clean"
if [ "${BUILD_SOURCE_KIND:-}" = "local-commit" ]; then
    BUILD_SOURCE_REF=$(source_ref_for_branch "$BUILD_BRANCH" "$BUILD_COMMIT")
fi

install_binary_from_source \
    "$SOURCE_DIR" \
    0 \
    "$BUILD_BRANCH" \
    "$BUILD_COMMIT" \
    "$BUILD_DIRTY" \
    "$BUILD_SOURCE_KIND" \
    "$BUILD_SOURCE_REF"

ensure_standard_global_layout
install_example_configs "$SOURCE_DIR/config.example" "$(acs_config_dir)"
print_global_install_summary
