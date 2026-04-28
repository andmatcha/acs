#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

ensure_rust_toolchain 1

(cd "$REPO_ROOT" && rustup override set stable)

info "building a local release binary"
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" --release

info "local launcher is ready at $REPO_ROOT/acs"
info "run it from this directory with: ./acs --help"
