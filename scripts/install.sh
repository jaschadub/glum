#!/usr/bin/env bash
#
# glum — one-liner installer for macOS and Linux.
#
# Downloads the latest release archive for the current platform from GitHub,
# verifies its SHA-256 against the signed checksums file, extracts the `glum`
# binary into $HOME/.local/bin (or a directory the user picks), and leaves
# LICENSE / NOTICE / README next to it.
#
# Windows users: use `cargo install glum` or grab the .zip from the GitHub
# release page manually.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jaschadub/glum/main/scripts/install.sh | bash
#
# Options (env vars):
#   GLUM_VERSION   pin to a specific tag (e.g. v0.1.0); default = latest
#   GLUM_PREFIX    install prefix; default = $HOME/.local
#   GLUM_VERIFY    set to 0 to skip SHA-256 verification (not recommended)

set -euo pipefail

REPO="jaschadub/glum"
BIN="glum"
PREFIX="${GLUM_PREFIX:-$HOME/.local}"
VERIFY="${GLUM_VERIFY:-1}"
VERSION="${GLUM_VERSION:-}"

err() { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '==> %s\n' "$*"; }

need() {
    command -v "$1" >/dev/null 2>&1 || err "required command not found: $1"
}

need curl
need tar
need uname
need mkdir
# sha256 util is platform-dependent; we'll pick one at verify time.

detect_target() {
    local os arch
    case "$(uname -s)" in
        Linux)  os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *) err "unsupported OS: $(uname -s). Try: cargo install glum" ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64) arch="x86_64" ;;
        arm64|aarch64) arch="aarch64" ;;
        *) err "unsupported architecture: $(uname -m). Try: cargo install glum" ;;
    esac
    printf '%s-%s' "$arch" "$os"
}

resolve_version() {
    if [[ -n "$VERSION" ]]; then
        printf '%s' "$VERSION"
        return
    fi
    local url="https://api.github.com/repos/${REPO}/releases/latest"
    # Pull the `tag_name` field without pulling in jq as a dependency.
    local tag
    tag="$(curl -fsSL "$url" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
    [[ -n "$tag" ]] || err "could not resolve latest version from $url"
    printf '%s' "$tag"
}

sha256_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        err "no sha256 tool available (tried shasum and sha256sum)"
    fi
}

main() {
    local target tag base archive archive_url checksum_url tmp expected got
    target="$(detect_target)"
    tag="$(resolve_version)"
    base="https://github.com/${REPO}/releases/download/${tag}"
    archive="${BIN}-${tag}-${target}.tar.gz"
    archive_url="${base}/${archive}"
    checksum_url="${base}/checksums.txt"

    info "installing ${BIN} ${tag} for ${target}"
    info "source: ${archive_url}"

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    curl -fL --retry 3 -o "${tmp}/${archive}" "$archive_url" \
        || err "download failed: ${archive_url}"

    if [[ "$VERIFY" = "1" ]]; then
        info "verifying SHA-256"
        curl -fsSL -o "${tmp}/checksums.txt" "$checksum_url" \
            || err "could not fetch checksums.txt — re-run with GLUM_VERIFY=0 to skip, but you should investigate first"
        expected="$(grep " ${archive}\$" "${tmp}/checksums.txt" | awk '{print $1}')"
        [[ -n "$expected" ]] || err "no checksum entry for ${archive}"
        got="$(sha256_of "${tmp}/${archive}")"
        if [[ "$expected" != "$got" ]]; then
            err "checksum mismatch: expected ${expected}, got ${got}"
        fi
        info "sha256 ok"
    else
        info "GLUM_VERIFY=0 — skipping checksum verification"
    fi

    mkdir -p "${PREFIX}/bin" "${PREFIX}/share/doc/${BIN}"
    tar -xzf "${tmp}/${archive}" -C "$tmp"
    install -m 0755 "${tmp}/${BIN}" "${PREFIX}/bin/${BIN}"
    for doc in LICENSE NOTICE README.md CHANGELOG.md; do
        [[ -f "${tmp}/${doc}" ]] && install -m 0644 "${tmp}/${doc}" "${PREFIX}/share/doc/${BIN}/${doc}"
    done

    info "installed ${PREFIX}/bin/${BIN}"
    if ! command -v "${BIN}" >/dev/null 2>&1; then
        cat <<EOF

${PREFIX}/bin is not on your PATH. Add it, then re-open your shell:

  echo 'export PATH="${PREFIX}/bin:\$PATH"' >> ~/.bashrc   # or ~/.zshrc
EOF
    fi
    "${PREFIX}/bin/${BIN}" --version || true
}

main "$@"
