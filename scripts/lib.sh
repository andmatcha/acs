#!/bin/sh
set -eu

ACS_NAME="acs"
ACS_REPO_URL_DEFAULT="https://github.com/andmatcha/acs.git"

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

acs_global_binary_path() {
    printf '%s\n' "$(acs_cargo_bin_dir)/$ACS_NAME"
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

require_clean_ref_selection() {
    count=0
    [ -n "${1:-}" ] && count=$((count + 1))
    [ -n "${2:-}" ] && count=$((count + 1))
    [ -n "${3:-}" ] && count=$((count + 1))
    [ "$count" -le 1 ] || fail "specify only one of TAG=..., BRANCH=..., or COMMIT=..."
}

ensure_dir() {
    mkdir -p "$1"
}

acs_is_globally_installed() {
    [ -x "$(acs_global_binary_path)" ]
}

installed_version_line() {
    if acs_is_globally_installed; then
        version_output=$("$(acs_global_binary_path)" --version 2>/dev/null || true)
        if [ -z "$version_output" ]; then
            version_output="version metadata unavailable"
        fi
        printf '%s\n' "$version_output"
    fi
}

git_branch_name() {
    repo_dir=$1
    branch=$(git -C "$repo_dir" rev-parse --abbrev-ref HEAD 2>/dev/null || printf '%s\n' "detached")
    if [ "$branch" = "HEAD" ]; then
        branch="detached"
    fi
    printf '%s\n' "$branch"
}

git_commit_id() {
    git -C "$1" rev-parse HEAD
}

git_dirty_state() {
    repo_dir=$1
    if [ -n "$(git -C "$repo_dir" status --porcelain 2>/dev/null)" ]; then
        printf '%s\n' "dirty"
    else
        printf '%s\n' "clean"
    fi
}

source_ref_for_branch() {
    branch=$1
    commit=$2

    if [ "$branch" = "detached" ]; then
        printf '%s\n' "$commit"
    else
        printf '%s\n' "$branch"
    fi
}

latest_remote_tag() {
    git ls-remote --tags --refs --sort=-v:refname "$(acs_repo_url)" \
        | sed 's#^[^[:space:]]*[[:space:]]*refs/tags/##' \
        | sed -n '1p'
}

clone_local_head_source() {
    repo_dir=$1
    dest_dir=$2
    commit=$(git_commit_id "$repo_dir")

    git clone --local --no-hardlinks "$repo_dir" "$dest_dir" >/dev/null
    git -c advice.detachedHead=false -C "$dest_dir" checkout --detach "$commit" >/dev/null
}

clone_remote_branch_source() {
    branch=$1
    dest_dir=$2
    git clone --depth 1 --branch "$branch" "$(acs_repo_url)" "$dest_dir" >/dev/null
}

clone_remote_tag_source() {
    tag=$1
    dest_dir=$2
    git -c advice.detachedHead=false clone --depth 1 --branch "$tag" "$(acs_repo_url)" "$dest_dir" >/dev/null
}

clone_remote_commit_source() {
    commit=$1
    dest_dir=$2
    git clone "$(acs_repo_url)" "$dest_dir" >/dev/null
    git -c advice.detachedHead=false -C "$dest_dir" checkout --detach "$commit" >/dev/null
}

install_binary_from_source() {
    source_dir=$1
    force_install=$2
    build_branch=$3
    build_commit=$4
    build_dirty=$5
    build_source_kind=$6
    build_source_ref=$7

    if [ "$force_install" -eq 1 ]; then
        ACS_BUILD_BRANCH="$build_branch" \
        ACS_BUILD_COMMIT="$build_commit" \
        ACS_BUILD_DIRTY="$build_dirty" \
        ACS_BUILD_SOURCE_KIND="$build_source_kind" \
        ACS_BUILD_SOURCE_REF="$build_source_ref" \
        cargo install --path "$source_dir" --locked --force
    else
        ACS_BUILD_BRANCH="$build_branch" \
        ACS_BUILD_COMMIT="$build_commit" \
        ACS_BUILD_DIRTY="$build_dirty" \
        ACS_BUILD_SOURCE_KIND="$build_source_kind" \
        ACS_BUILD_SOURCE_REF="$build_source_ref" \
        cargo install --path "$source_dir" --locked
    fi
}

ensure_standard_global_layout() {
    ensure_dir "$(acs_log_dir)"
}

print_global_install_summary() {
    info "binary installed to $(acs_global_binary_path)"
    info "log dir: $(acs_log_dir)"
    if acs_is_globally_installed; then
        info "installed build: $(installed_version_line)"
    fi
    print_path_hint_if_needed
}

print_path_summary() {
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
