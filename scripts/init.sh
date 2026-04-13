#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

load_cargo_env

if ! command -v rustup >/dev/null 2>&1; then
    require_command curl
    info "installing rustup with the stable toolchain"
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
    load_cargo_env
fi

require_command rustup
require_command cargo

info "ensuring the stable Rust toolchain is available"
rustup toolchain install stable --profile minimal
(cd "$REPO_ROOT" && rustup override set stable)
rustup component add clippy rustfmt

info "building a local release binary"
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" --release

info "local launcher is ready at $REPO_ROOT/acs"
info "run it from this directory with: ./acs --help"
