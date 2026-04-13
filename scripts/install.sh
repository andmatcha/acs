#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
SOURCE_DIR=$REPO_ROOT
VERSION_LABEL=""
SKIP_EXAMPLE_CONFIGS=0

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --source-dir)
            [ "$#" -ge 2 ] || fail "missing value for --source-dir"
            SOURCE_DIR=$2
            shift 2
            ;;
        --version)
            [ "$#" -ge 2 ] || fail "missing value for --version"
            VERSION_LABEL=$2
            shift 2
            ;;
        --skip-example-configs)
            SKIP_EXAMPLE_CONFIGS=1
            shift
            ;;
        *)
            fail "unknown option for install.sh: $1"
            ;;
    esac
done

load_cargo_env
require_command cargo

if [ -n "$VERSION_LABEL" ]; then
    info "installing $ACS_NAME $VERSION_LABEL globally"
else
    info "installing $ACS_NAME globally from the current checkout"
fi

cargo install --path "$SOURCE_DIR" --locked --force

CONFIG_DIR=$(acs_config_dir)
LOG_DIR=$(acs_log_dir)
ensure_dir "$CONFIG_DIR"
ensure_dir "$LOG_DIR"
if [ "$SKIP_EXAMPLE_CONFIGS" -eq 0 ]; then
    install_example_configs "$SOURCE_DIR/config.example" "$CONFIG_DIR"
fi

info "binary installed to $(acs_cargo_bin_dir)/$ACS_NAME"
info "config dir: $CONFIG_DIR"
info "log dir: $LOG_DIR"
if [ "$SKIP_EXAMPLE_CONFIGS" -eq 1 ]; then
    info "left existing global config files untouched"
fi
print_path_hint_if_needed
