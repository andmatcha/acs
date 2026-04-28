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

if acs_is_globally_installed; then
    info "$ACS_NAME is already installed at $(acs_global_binary_path)"
    info "installed build: $(installed_version_line)"
    info "use \`make update\` to replace the global build"
    exit 0
fi

ensure_rust_toolchain 0

TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/acs-install.XXXXXX")
trap 'rm -rf "$TMP_ROOT"' EXIT INT TERM HUP
SOURCE_DIR="$TMP_ROOT/source"

if [ -z "$TAG" ] && [ -z "$BRANCH" ] && [ -z "$COMMIT" ]; then
    TAG=$(latest_remote_tag)
    [ -n "$TAG" ] || fail "no remote tags were found at $(acs_repo_url)"
    info "installing $ACS_NAME from latest remote tag $TAG"
elif [ -n "$TAG" ]; then
    info "installing $ACS_NAME from remote tag $TAG"
fi

if [ -n "$TAG" ]; then
    if has_command git; then
        clone_remote_tag_source "$TAG" "$SOURCE_DIR"
    else
        SOURCE_DIR=$(download_github_source_archive tag "$TAG" "$TMP_ROOT")
    fi
    BUILD_BRANCH="detached"
    BUILD_SOURCE_KIND="remote-tag"
    BUILD_SOURCE_REF="$TAG"
elif [ -n "$BRANCH" ]; then
    info "installing $ACS_NAME from remote branch $BRANCH"
    if has_command git; then
        clone_remote_branch_source "$BRANCH" "$SOURCE_DIR"
    else
        SOURCE_DIR=$(download_github_source_archive branch "$BRANCH" "$TMP_ROOT")
    fi
    BUILD_BRANCH="$BRANCH"
    BUILD_SOURCE_KIND="remote-branch"
    BUILD_SOURCE_REF="$BRANCH"
elif [ -n "$COMMIT" ]; then
    info "installing $ACS_NAME from remote commit $COMMIT"
    if has_command git; then
        clone_remote_commit_source "$COMMIT" "$SOURCE_DIR"
    else
        SOURCE_DIR=$(download_github_source_archive commit "$COMMIT" "$TMP_ROOT")
        BUILD_COMMIT="$COMMIT"
    fi
    BUILD_BRANCH="detached"
    BUILD_SOURCE_KIND="remote-commit"
    BUILD_SOURCE_REF="$COMMIT"
fi

if [ -z "${BUILD_COMMIT:-}" ]; then
    if has_command git && git -C "$SOURCE_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        BUILD_COMMIT=$(git_commit_id "$SOURCE_DIR")
    else
        BUILD_COMMIT="unknown"
    fi
fi
if [ -z "${BUILD_DIRTY:-}" ]; then
    BUILD_DIRTY="clean"
fi
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
print_global_install_summary
