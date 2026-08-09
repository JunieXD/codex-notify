#!/bin/sh

set -eu

REPOSITORY="${CODEX_NOTIFY_REPOSITORY:-JunieXD/codex-notify}"
VERSION="${CODEX_NOTIFY_VERSION:-latest}"
INSTALL_DIR="${CODEX_NOTIFY_INSTALL_DIR:-$HOME/.local/bin}"

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

ASSET="codex-notify-${TARGET}.tar.gz"
if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/latest/download"
else
    DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/download/${VERSION}"
fi

TEMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT HUP INT TERM

echo "Downloading codex-notify for ${TARGET}..."
curl -fsSL --retry 3 --retry-delay 1 \
    "${DOWNLOAD_BASE}/${ASSET}" -o "${TEMP_DIR}/${ASSET}"
curl -fsSL --retry 3 --retry-delay 1 \
    "${DOWNLOAD_BASE}/SHA256SUMS" -o "${TEMP_DIR}/SHA256SUMS"

(
    cd "$TEMP_DIR"
    grep -F "  ${ASSET}" SHA256SUMS > "${ASSET}.sha256"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "${ASSET}.sha256"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c "${ASSET}.sha256"
    else
        echo "A SHA-256 checksum tool is required (sha256sum or shasum)." >&2
        exit 2
    fi
)

mkdir -p "$INSTALL_DIR"
tar -xzf "${TEMP_DIR}/${ASSET}" -C "$TEMP_DIR"
install -m 755 "${TEMP_DIR}/codex-notify" "${INSTALL_DIR}/codex-notify"

echo "Installed codex-notify to ${INSTALL_DIR}/codex-notify"
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo "Add this directory to PATH before running codex-notify:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac
