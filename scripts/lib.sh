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
    case "$(acs_os)" in
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            printf '%s\n' "$(acs_cargo_bin_dir)/$ACS_NAME.exe"
            ;;
        *)
            printf '%s\n' "$(acs_cargo_bin_dir)/$ACS_NAME"
            ;;
    esac
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

has_command() {
    command -v "$1" >/dev/null 2>&1
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

run_privileged() {
    if [ "$(id -u 2>/dev/null || printf '%s\n' 1)" -eq 0 ]; then
        "$@"
    elif has_command sudo; then
        sudo "$@"
    else
        fail "sudo is required to install system packages; install the missing build tools manually, then rerun this script"
    fi
}

ensure_unix_build_tools() {
    case "$(acs_os)" in
        Darwin)
            if { has_command xcrun && xcrun --find clang >/dev/null 2>&1; } || has_command cc || has_command clang; then
                return 0
            fi

            if has_command xcode-select; then
                info "installing Apple Command Line Tools"
                xcode-select --install >/dev/null 2>&1 || true
                fail "Apple Command Line Tools installation was requested. Rerun this script after the installer finishes."
            fi

            fail "Apple Command Line Tools are required; install them, then rerun this script"
            ;;
        Linux)
            if has_command cc && has_command pkg-config && pkg-config --exists libudev >/dev/null 2>&1; then
                return 0
            fi

            if has_command apt-get; then
                info "installing Linux build dependencies with apt"
                run_privileged apt-get update
                run_privileged apt-get install -y build-essential pkg-config libudev-dev curl ca-certificates
            elif has_command dnf; then
                info "installing Linux build dependencies with dnf"
                run_privileged dnf install -y gcc make pkgconf-pkg-config systemd-devel curl ca-certificates
            elif has_command yum; then
                info "installing Linux build dependencies with yum"
                run_privileged yum install -y gcc make pkgconfig systemd-devel curl ca-certificates
            elif has_command zypper; then
                info "installing Linux build dependencies with zypper"
                run_privileged zypper --non-interactive install gcc make pkg-config libudev-devel curl ca-certificates
            elif has_command pacman; then
                info "installing Linux build dependencies with pacman"
                run_privileged pacman -Sy --needed --noconfirm base-devel pkgconf systemd curl ca-certificates
            elif has_command apk; then
                info "installing Linux build dependencies with apk"
                run_privileged apk add build-base pkgconf eudev-dev curl ca-certificates
            else
                fail "could not find a supported package manager; install C build tools, pkg-config, and libudev development headers, then rerun this script"
            fi
            ;;
    esac
}

download_to_stdout() {
    url=$1

    if has_command curl; then
        curl --proto '=https' --tlsv1.2 -fsSL "$url"
    elif has_command wget; then
        wget -qO- "$url"
    else
        fail "required command not found: curl or wget"
    fi
}

download_to_file() {
    url=$1
    dest=$2

    if has_command curl; then
        curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$dest"
    elif has_command wget; then
        wget -qO "$dest" "$url"
    else
        fail "required command not found: curl or wget"
    fi
}

install_rustup_if_needed() {
    load_cargo_env

    if ! has_command rustup; then
        info "installing rustup with the stable toolchain"
        download_to_stdout https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
        load_cargo_env
    fi

    require_command rustup
    require_command cargo
}

ensure_rust_toolchain() {
    with_components=$1

    ensure_unix_build_tools
    install_rustup_if_needed

    info "ensuring the stable Rust toolchain is available"
    rustup toolchain install stable --profile minimal

    if [ "$with_components" -eq 1 ]; then
        rustup component add clippy rustfmt
    fi
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
    if has_command git; then
        tag=$(git ls-remote --tags --refs --sort=-v:refname "$(acs_repo_url)" 2>/dev/null \
            | sed 's#^[^[:space:]]*[[:space:]]*refs/tags/##' \
            | sed -n '1p' || true)
        if [ -n "$tag" ]; then
            printf '%s\n' "$tag"
            return 0
        fi
    fi

    slug=$(github_archive_slug)
    api_output=$(download_to_stdout "https://api.github.com/repos/$slug/tags?per_page=100") || return 1
    printf '%s\n' "$api_output" \
        | sed -n 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | latest_tag_from_stdin
}

latest_tag_from_stdin() {
    awk '
        {
            tag = $0
            value = tag
            sub(/^[vV]/, "", value)
            n = split(value, parts, /[^0-9]+/)
            key = ""
            for (i = 1; i <= 4; i++) {
                number = 0
                if (i <= n && parts[i] != "") {
                    number = parts[i] + 0
                }
                key = key sprintf("%09d.", number)
            }
            print key "|" tag
        }
    ' | sort -r | sed -n '1s/^[^|]*|//p'
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

copy_source_tree() {
    source_dir=$1
    dest_dir=$2
    archive_path=$dest_dir/source-copy.tar

    ensure_dir "$dest_dir"
    tar \
        --exclude ./.git \
        --exclude ./target \
        --exclude ./logs \
        -C "$source_dir" \
        -cf "$archive_path" .
    tar -C "$dest_dir" -xf "$archive_path"
    rm -f "$archive_path"
}

github_archive_slug() {
    repo_url=$(acs_repo_url)

    case "$repo_url" in
        https://github.com/*)
            slug=${repo_url#https://github.com/}
            ;;
        git@github.com:*)
            slug=${repo_url#git@github.com:}
            ;;
        ssh://git@github.com/*)
            slug=${repo_url#ssh://git@github.com/}
            ;;
        *)
            fail "Git is required for ACS_REPO_URL=$repo_url; without Git, only GitHub repository URLs can be downloaded as archives"
            ;;
    esac

    slug=${slug%.git}
    slug=${slug%/}
    printf '%s\n' "$slug"
}

github_archive_url() {
    kind=$1
    ref=$2
    slug=$(github_archive_slug)

    case "$kind" in
        tag)
            printf '%s\n' "https://github.com/$slug/archive/refs/tags/$ref.tar.gz"
            ;;
        branch)
            printf '%s\n' "https://github.com/$slug/archive/refs/heads/$ref.tar.gz"
            ;;
        commit)
            printf '%s\n' "https://github.com/$slug/archive/$ref.tar.gz"
            ;;
        *)
            fail "unknown GitHub archive kind: $kind"
            ;;
    esac
}

download_github_source_archive() {
    kind=$1
    ref=$2
    dest_dir=$3
    archive_url=$(github_archive_url "$kind" "$ref")
    archive_path=$dest_dir/source.tar.gz
    extract_root=$dest_dir/archive

    info "Git was not found; downloading $archive_url" >&2
    download_to_file "$archive_url" "$archive_path"
    ensure_dir "$extract_root"
    tar -xzf "$archive_path" -C "$extract_root"

    source_root=$(find "$extract_root" -type f -name Cargo.toml 2>/dev/null | sed -n '1p')
    [ -n "$source_root" ] || fail "downloaded archive does not contain Cargo.toml"
    dirname "$source_root"
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
