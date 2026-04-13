#!/bin/sh
set -eu

ACS_NAME="acs"
ACS_REPO_URL_DEFAULT="https://github.com/andmatcha/acs.git"
ACS_MANAGED_LOCAL_CONFIG_DIR_NAME="zz-local"

info() {
    printf '%s\n' "$*"
}

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

acs_repo_url() {
    printf '%s\n' "${ACS_REPO_URL:-$ACS_REPO_URL_DEFAULT}"
}

acs_os() {
    uname -s 2>/dev/null || printf '%s\n' "unknown"
}

acs_home_dir() {
    [ -n "${HOME:-}" ] || fail "HOME is not set"
    printf '%s\n' "$HOME"
}

acs_config_dir() {
    home_dir=$(acs_home_dir)
    case "$(acs_os)" in
        Darwin)
            printf '%s\n' "$home_dir/Library/Application Support/$ACS_NAME"
            ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            base_dir=${APPDATA:-"$home_dir/AppData/Roaming"}
            printf '%s\n' "$base_dir/$ACS_NAME"
            ;;
        *)
            base_dir=${XDG_CONFIG_HOME:-"$home_dir/.config"}
            printf '%s\n' "$base_dir/$ACS_NAME"
            ;;
    esac
}

acs_managed_local_config_dir() {
    printf '%s\n' "$(acs_config_dir)/$ACS_MANAGED_LOCAL_CONFIG_DIR_NAME"
}

acs_log_dir() {
    home_dir=$(acs_home_dir)
    case "$(acs_os)" in
        Darwin)
            printf '%s\n' "$home_dir/Library/Logs/$ACS_NAME"
            ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            base_dir=${LOCALAPPDATA:-"$home_dir/AppData/Local"}
            printf '%s\n' "$base_dir/$ACS_NAME/logs"
            ;;
        *)
            base_dir=${XDG_STATE_HOME:-"$home_dir/.local/state"}
            printf '%s\n' "$base_dir/$ACS_NAME/logs"
            ;;
    esac
}

acs_cargo_home() {
    if [ -n "${CARGO_HOME:-}" ]; then
        printf '%s\n' "$CARGO_HOME"
    else
        printf '%s\n' "$(acs_home_dir)/.cargo"
    fi
}

acs_cargo_bin_dir() {
    printf '%s\n' "$(acs_cargo_home)/bin"
}

load_cargo_env() {
    cargo_home=$(acs_cargo_home)
    cargo_env="$cargo_home/env"

    if [ -f "$cargo_env" ]; then
        # shellcheck disable=SC1090
        . "$cargo_env"
    else
        export PATH="$cargo_home/bin:$PATH"
    fi
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

ensure_dir() {
    mkdir -p "$1"
}

remove_dir_if_exists() {
    if [ -e "$1" ]; then
        rm -rf "$1"
    fi
}

install_example_configs() {
    example_dir=$1
    config_dir=$2

    if [ ! -d "$example_dir" ]; then
        info "no example config directory found at $example_dir"
        return 0
    fi

    ensure_dir "$config_dir"

    for src in "$example_dir"/*.json; do
        if [ ! -e "$src" ]; then
            continue
        fi

        dest="$config_dir/$(basename "$src")"
        if [ -e "$dest" ]; then
            info "kept $dest"
            continue
        fi

        cp "$src" "$dest"
        info "installed $dest"
    done
}

find_local_config_source() {
    repo_root=$1
    config_dir="$repo_root/config"
    config_file="$repo_root/acs.config.json"

    if [ -d "$config_dir" ]; then
        printf 'dir:%s\n' "$config_dir"
        return 0
    fi

    if [ -f "$config_file" ]; then
        printf 'file:%s\n' "$config_file"
        return 0
    fi

    return 1
}

print_path_summary() {
    info "config dir: $(acs_config_dir)"
    info "config overlay dir: $(acs_managed_local_config_dir)"
    info "log dir: $(acs_log_dir)"
    info "cargo bin: $(acs_cargo_bin_dir)"
}

print_path_hint_if_needed() {
    cargo_bin_dir=$(acs_cargo_bin_dir)
    case ":$PATH:" in
        *":$cargo_bin_dir:"*)
            return 0
            ;;
    esac

    info "add $cargo_bin_dir to PATH if you want to run \`$ACS_NAME\` without a full path"
}
