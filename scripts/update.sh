#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
VERSION=""
USE_LATEST=0

# shellcheck disable=SC1091
. "$SCRIPT_DIR/lib.sh"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail "missing value for --version"
            VERSION=$2
            shift 2
            ;;
        --latest)
            USE_LATEST=1
            shift
            ;;
        *)
            fail "unknown option for update.sh: $1"
            ;;
    esac
done

[ -z "$VERSION" ] || [ "$USE_LATEST" -eq 0 ] || fail "use either --version or --latest"

load_cargo_env
require_command cargo
require_command git

TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/acs-update.XXXXXX")
trap 'rm -rf "$TMP_ROOT"' EXIT INT TERM HUP

TAG_FILE="$TMP_ROOT/tags.txt"
SOURCE_DIR="$TMP_ROOT/source"

info "fetching tags from $(acs_repo_url)"
git ls-remote --tags --refs --sort=-v:refname "$(acs_repo_url)" \
    | sed 's#^[^[:space:]]*[[:space:]]*refs/tags/##' > "$TAG_FILE"

[ -s "$TAG_FILE" ] || fail "no tags were found at $(acs_repo_url)"

select_tag() {
    count=$(wc -l < "$TAG_FILE" | tr -d ' ')
    [ "$count" -gt 0 ] || fail "no tags are available to select"

    printf '%s\n' "available tags:" >&2
    nl -w2 -s'. ' "$TAG_FILE" >&2

    while :; do
        printf 'select a tag [1-%s]: ' "$count" >&2
        IFS= read -r selection

        case "$selection" in
            ''|*[!0-9]*)
                printf '%s\n' "enter a number between 1 and $count" >&2
                ;;
            *)
                if [ "$selection" -lt 1 ] || [ "$selection" -gt "$count" ]; then
                    printf '%s\n' "enter a number between 1 and $count" >&2
                    continue
                fi
                sed -n "${selection}p" "$TAG_FILE"
                return 0
                ;;
        esac
    done
}

if [ -n "$VERSION" ]; then
    grep -Fx "$VERSION" "$TAG_FILE" >/dev/null 2>&1 || fail "unknown tag: $VERSION"
elif [ "$USE_LATEST" -eq 1 ]; then
    VERSION=$(sed -n '1p' "$TAG_FILE")
elif [ -t 0 ]; then
    VERSION=$(select_tag)
else
    fail "stdin is not interactive; use make update VERSION=<tag> or make update-latest"
fi

info "installing $ACS_NAME from tag $VERSION"
git clone --depth 1 --branch "$VERSION" "$(acs_repo_url)" "$SOURCE_DIR"
"$REPO_ROOT/scripts/install.sh" --source-dir "$SOURCE_DIR" --version "$VERSION"
