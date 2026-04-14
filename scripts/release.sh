#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
VERSION=""

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail "missing value for --version"
            VERSION=$2
            shift 2
            ;;
        *)
            fail "unknown option for release.sh: $1"
            ;;
    esac
done

[ -n "$VERSION" ] || fail "release version is required. use: make release VERSION=x.y.z"

load_cargo_env
require_command cargo
require_command git

CURRENT_BRANCH=$(git_branch_name "$REPO_ROOT")
[ "$CURRENT_BRANCH" != "detached" ] || fail "release must be created from a branch, not detached HEAD"

if [ -n "$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null)" ]; then
    fail "release requires a clean worktree"
fi

if git -C "$REPO_ROOT" rev-parse "v$VERSION" >/dev/null 2>&1; then
    fail "tag v$VERSION already exists"
fi

CURRENT_VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO_ROOT/Cargo.toml" | sed -n '1p')
[ -n "$CURRENT_VERSION" ] || fail "failed to read package version from Cargo.toml"

if [ "$CURRENT_VERSION" = "$VERSION" ]; then
    fail "Cargo.toml already has version $VERSION"
fi

info "releasing $ACS_NAME $VERSION from branch $CURRENT_BRANCH"

perl -0pi -e 's/\[package\]\nname = "acs"\nversion = "[^"]+"/[package]\nname = "acs"\nversion = "'"$VERSION"'"/' \
    "$REPO_ROOT/Cargo.toml"
perl -0pi -e 's/\[\[package\]\]\nname = "acs"\nversion = "[^"]+"/[[package]]\nname = "acs"\nversion = "'"$VERSION"'"/' \
    "$REPO_ROOT/Cargo.lock"

info "running tests"
cargo test --manifest-path "$REPO_ROOT/Cargo.toml"

info "building release binary"
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" --release

git -C "$REPO_ROOT" add Cargo.toml Cargo.lock
git -C "$REPO_ROOT" commit -m "release: v$VERSION"
git -C "$REPO_ROOT" tag -a "v$VERSION" -m "Release v$VERSION"

RELEASE_COMMIT=$(git_commit_id "$REPO_ROOT")

info "created release commit $(printf '%s' "$RELEASE_COMMIT" | cut -c1-12) and tag v$VERSION"
info "next step: git push origin $CURRENT_BRANCH --follow-tags"
