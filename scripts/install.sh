#!/bin/sh

set -eu

REPOSITORY="${CODEX_NOTIFY_REPOSITORY:-JunieXD/codex-notify}"
VERSION="${CODEX_NOTIFY_VERSION:-latest}"
INSTALL_DIR="${CODEX_NOTIFY_INSTALL_DIR:-$HOME/.local/bin}"
DOWNLOAD_BASE_OVERRIDE="${CODEX_NOTIFY_DOWNLOAD_BASE:-}"
FORCE_UPDATE="${CODEX_NOTIFY_FORCE_UPDATE:-0}"
TARGET_PATH="${INSTALL_DIR}/codex-notify"

SYSTEM="$(uname -s)"
ARCHITECTURE="$(uname -m)"
case "${SYSTEM}:${ARCHITECTURE}" in
    Darwin:arm64)
        TARGET="aarch64-apple-darwin"
        ;;
    Darwin:x86_64)
        TARGET="x86_64-apple-darwin"
        ;;
    Linux:aarch64 | Linux:arm64)
        TARGET="aarch64-unknown-linux-gnu"
        ;;
    Linux:x86_64 | Linux:amd64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
    Darwin:* | Linux:*)
        echo "Unsupported ${SYSTEM} architecture: ${ARCHITECTURE}" >&2
        exit 2
        ;;
    *)
        echo "codex-notify currently supports macOS, Windows, and Linux only." >&2
        exit 2
        ;;
esac

# Newer installations own the entire update transaction. This avoids stopping
# the watcher or touching configuration until the release has been verified.
if [ -x "$TARGET_PATH" ] && "$TARGET_PATH" update --help >/dev/null 2>&1; then
    echo "Existing codex-notify installation found; starting a safe update..."
    set -- update --yes --repository "$REPOSITORY"
    if [ "$VERSION" != "latest" ]; then
        set -- "$@" --version "$VERSION"
    fi
    if [ -n "$DOWNLOAD_BASE_OVERRIDE" ]; then
        set -- "$@" --download-base "$DOWNLOAD_BASE_OVERRIDE"
    fi
    if [ "$FORCE_UPDATE" = "1" ]; then
        set -- "$@" --force
    fi
    "$TARGET_PATH" "$@"
    exit 0
fi

ASSET="codex-notify-${TARGET}.tar.gz"
if [ -n "$DOWNLOAD_BASE_OVERRIDE" ]; then
    DOWNLOAD_BASE="${DOWNLOAD_BASE_OVERRIDE%/}"
elif [ "$VERSION" = "latest" ]; then
    DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/latest/download"
else
    case "$VERSION" in
        v*) RELEASE_TAG="$VERSION" ;;
        *) RELEASE_TAG="v$VERSION" ;;
    esac
    DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/download/${RELEASE_TAG}"
fi

mkdir -p "$INSTALL_DIR"
TEMP_DIR="$(mktemp -d "${INSTALL_DIR}/.codex-notify-install.XXXXXX")"
cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT HUP INT TERM

echo "Downloading codex-notify for ${TARGET}..."
curl -fsSL --retry 3 --retry-delay 1 \
    "${DOWNLOAD_BASE}/${ASSET}" -o "${TEMP_DIR}/${ASSET}"
curl -fsSL --retry 3 --retry-delay 1 \
    "${DOWNLOAD_BASE}/SHA256SUMS" -o "${TEMP_DIR}/SHA256SUMS"

CHECKSUMS_SIZE="$(wc -c < "${TEMP_DIR}/SHA256SUMS")"
ARCHIVE_SIZE="$(wc -c < "${TEMP_DIR}/${ASSET}")"
if [ "$CHECKSUMS_SIZE" -gt 1048576 ] || [ "$ARCHIVE_SIZE" -gt 134217728 ]; then
    echo "The downloaded release exceeds the allowed size." >&2
    exit 2
fi

EXPECTED_HASH="$(awk -v asset="$ASSET" '
    NF == 2 {
        name = $2
        sub(/^\*/, "", name)
        if (name == asset) print tolower($1)
    }
' "${TEMP_DIR}/SHA256SUMS")"
case "$EXPECTED_HASH" in
    *"
"*)
        echo "SHA256SUMS contains more than one checksum for ${ASSET}." >&2
        exit 2
        ;;
    *[!0-9a-f]* | "")
        echo "SHA256SUMS does not contain a valid checksum for ${ASSET}." >&2
        exit 2
        ;;
esac
if [ "${#EXPECTED_HASH}" -ne 64 ]; then
    echo "SHA256SUMS does not contain a valid checksum for ${ASSET}." >&2
    exit 2
fi
printf '%s  %s\n' "$EXPECTED_HASH" "$ASSET" > "${TEMP_DIR}/${ASSET}.sha256"
(
    cd "$TEMP_DIR"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "${ASSET}.sha256"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c "${ASSET}.sha256"
    else
        echo "A SHA-256 checksum tool is required (sha256sum or shasum)." >&2
        exit 2
    fi
)

tar -xzf "${TEMP_DIR}/${ASSET}" -C "$TEMP_DIR"
if [ ! -f "${TEMP_DIR}/codex-notify" ]; then
    echo "The release archive does not contain codex-notify." >&2
    exit 2
fi
chmod 755 "${TEMP_DIR}/codex-notify"

if "${TEMP_DIR}/codex-notify" install-prepared --help >/dev/null 2>&1; then
    set -- install-prepared --target "$TARGET_PATH"
    if [ "$VERSION" != "latest" ]; then
        set -- "$@" --expected-version "$VERSION"
    fi
    if [ "$FORCE_UPDATE" = "1" ]; then
        set -- "$@" --force
    fi
    "${TEMP_DIR}/codex-notify" "$@"
elif [ ! -e "$TARGET_PATH" ]; then
    # Compatibility for a first install while the latest GitHub Release still
    # predates the built-in updater.
    mkdir -p "$INSTALL_DIR"
    install -m 755 "${TEMP_DIR}/codex-notify" "$TARGET_PATH"
    echo "Installed codex-notify to ${TARGET_PATH}"
else
    INSTALLED_VERSION="$("$TARGET_PATH" --version 2>/dev/null || true)"
    DOWNLOADED_VERSION="$("${TEMP_DIR}/codex-notify" --version 2>/dev/null || true)"
    if [ -n "$INSTALLED_VERSION" ] && [ "$INSTALLED_VERSION" = "$DOWNLOADED_VERSION" ]; then
        echo "codex-notify is already up to date (${INSTALLED_VERSION#codex-notify })."
    else
        echo "This older release cannot safely upgrade an existing installation." >&2
        echo "Install a newer codex-notify release explicitly, then retry." >&2
        exit 1
    fi
fi

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo "Add this directory to PATH before running codex-notify:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac
