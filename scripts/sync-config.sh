#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

source_spec=$(find_local_config_source "$REPO_ROOT") || fail "no local config found. Create ./config/ or ./acs.config.json first."
source_type=${source_spec%%:*}
source_path=${source_spec#*:}
target_dir=$(acs_managed_local_config_dir)

ensure_dir "$(acs_config_dir)"
remove_dir_if_exists "$target_dir"
ensure_dir "$target_dir"

case "$source_type" in
    dir)
        cp -R "$source_path/." "$target_dir/"
        info "synced local config directory $source_path"
        ;;
    file)
        cp "$source_path" "$target_dir/$(basename "$source_path")"
        info "synced local config file $source_path"
        ;;
    *)
        fail "unsupported local config source type: $source_type"
        ;;
esac

info "global overlay dir: $target_dir"
info "this overlay is loaded after the base global config and overrides matching entries"
