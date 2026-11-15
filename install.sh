#!/usr/bin/env sh
set -eu

REPO="princetonacelopez/ace-term"
BIN_NAME="ace-term"
INSTALL_DIR="${ACE_TERM_INSTALL_DIR:-$HOME/.local/bin}"

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
                *)
                    echo "Unsupported Linux architecture: $arch" >&2
                    exit 1
                    ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                arm64|aarch64) echo "aarch64-apple-darwin" ;;
                x86_64) echo "x86_64-apple-darwin" ;;
                *)
                    echo "Unsupported macOS architecture: $arch" >&2
                    exit 1
                    ;;
            esac
            ;;
        *)
            echo "Unsupported operating system: $os" >&2
            exit 1
            ;;
    esac
}

verify_checksum() {
    file="$1"
    checksums_file="$2"
    filename="$(basename "$file")"
    expected="$(grep " ${filename}\$" "$checksums_file" | awk '{print $1}')"
    if [ -z "$expected" ]; then
        echo "Checksum entry not found for $filename" >&2
        exit 1
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    else
        echo "Warning: sha256sum/shasum not found; skipping checksum verification" >&2
        return 0
    fi

    if [ "$expected" != "$actual" ]; then
        echo "Checksum mismatch for $filename" >&2
        exit 1
    fi
}

main() {
    target="$(detect_target)"
    tag="${ACE_TERM_VERSION:-}"

    if [ -z "$tag" ]; then
        tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
    fi

    if [ -z "$tag" ]; then
        echo "Could not determine latest release tag for ${REPO}." >&2
        exit 1
    fi

    archive="ace-term-${tag}-${target}.tar.gz"
    base_url="https://github.com/${REPO}/releases/download/${tag}"

    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT INT TERM

    echo "Downloading ${archive} (${tag})..."
    curl -fsSL "${base_url}/${archive}" -o "${tmpdir}/${archive}"
    curl -fsSL "${base_url}/checksums.txt" -o "${tmpdir}/checksums.txt"

    verify_checksum "${tmpdir}/${archive}" "${tmpdir}/checksums.txt"

    tar -xzf "${tmpdir}/${archive}" -C "$tmpdir"
    pkg_dir="${tmpdir}/ace-term-${tag}-${target}"

    mkdir -p "$INSTALL_DIR"
    install -m 755 "${pkg_dir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"

    echo "Installed ${BIN_NAME} to ${INSTALL_DIR}/${BIN_NAME}"
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo "Add ${INSTALL_DIR} to your PATH to run '${BIN_NAME}' from anywhere:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac
}

main "$@"
