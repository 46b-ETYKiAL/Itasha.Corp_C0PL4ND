#!/bin/sh
# install.sh - one-line installer for C0PL4ND.
#
# Detects OS/arch, downloads the latest release tarball from GitHub, verifies
# its SHA256 checksum, installs the binary to ~/.local/bin, and prints next
# steps. POSIX sh, no bashisms. shellcheck-clean.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/itasha-corp/c0pl4nd/main/packaging/linux/install.sh | sh
#
# Environment overrides:
#   C0PL4ND_VERSION   pin a version tag (default: latest)
#   C0PL4ND_BIN_DIR   install directory (default: $HOME/.local/bin)
set -eu

REPO="itasha-corp/c0pl4nd"
BIN="c0pl4nd"
BIN_DIR="${C0PL4ND_BIN_DIR:-${HOME}/.local/bin}"

err() {
	printf 'error: %s\n' "$1" >&2
	exit 1
}

info() {
	printf '%s\n' "$1"
}

need() {
	command -v "$1" >/dev/null 2>&1 || err "required tool not found: $1"
}

# --- detect a usable downloader -------------------------------------------
DOWNLOADER=""
if command -v curl >/dev/null 2>&1; then
	DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
	DOWNLOADER="wget"
else
	err "neither curl nor wget is available"
fi

download() {
	# download <url> <output-path>
	if [ "${DOWNLOADER}" = "curl" ]; then
		curl -fsSL -o "$2" "$1"
	else
		wget -qO "$2" "$1"
	fi
}

fetch_stdout() {
	# fetch_stdout <url>
	if [ "${DOWNLOADER}" = "curl" ]; then
		curl -fsSL "$1"
	else
		wget -qO- "$1"
	fi
}

# --- detect OS ------------------------------------------------------------
os="$(uname -s)"
case "${os}" in
	Linux) os_id="unknown-linux-gnu" ;;
	Darwin) os_id="apple-darwin" ;;
	*) err "unsupported operating system: ${os}" ;;
esac

# --- detect arch ----------------------------------------------------------
arch="$(uname -m)"
case "${arch}" in
	x86_64 | amd64) arch_id="x86_64" ;;
	aarch64 | arm64) arch_id="aarch64" ;;
	*) err "unsupported architecture: ${arch}" ;;
esac

target="${arch_id}-${os_id}"

# --- resolve version ------------------------------------------------------
version="${C0PL4ND_VERSION:-}"
if [ -z "${version}" ]; then
	info "Resolving latest release..."
	api_url="https://api.github.com/repos/${REPO}/releases/latest"
	# Parse the tag_name without requiring jq: grep the JSON field.
	version="$(fetch_stdout "${api_url}" \
		| grep '"tag_name"' \
		| head -n 1 \
		| sed -e 's/.*"tag_name"[[:space:]]*:[[:space:]]*"//' -e 's/".*//')"
	[ -n "${version}" ] || err "could not determine latest version"
fi
info "Installing C0PL4ND ${version} (${target})"

# --- build download URLs --------------------------------------------------
stage="${BIN}-${version}-${target}"
archive="${stage}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/${version}"
archive_url="${base_url}/${archive}"
checksum_url="${archive_url}.sha256"

# --- work in a temp dir ---------------------------------------------------
tmp="$(mktemp -d 2>/dev/null || mktemp -d -t c0pl4nd)"
[ -n "${tmp}" ] || err "could not create temp directory"
# shellcheck disable=SC2064
trap "rm -rf \"${tmp}\"" EXIT INT TERM

info "Downloading ${archive_url}"
download "${archive_url}" "${tmp}/${archive}"

info "Downloading checksum"
download "${checksum_url}" "${tmp}/${archive}.sha256"

# --- verify checksum ------------------------------------------------------
expected="$(awk '{print $1}' "${tmp}/${archive}.sha256")"
[ -n "${expected}" ] || err "checksum file is empty or malformed"

actual=""
if command -v sha256sum >/dev/null 2>&1; then
	actual="$(sha256sum "${tmp}/${archive}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
	actual="$(shasum -a 256 "${tmp}/${archive}" | awk '{print $1}')"
else
	err "no sha256 tool found (need sha256sum or shasum)"
fi

if [ "${expected}" != "${actual}" ]; then
	err "checksum mismatch: expected ${expected}, got ${actual}"
fi
info "Checksum verified."

# --- extract --------------------------------------------------------------
need tar
( cd "${tmp}" && tar -xzf "${archive}" )

src_bin="${tmp}/${stage}/${BIN}"
[ -f "${src_bin}" ] || err "binary not found in archive: ${src_bin}"

# --- install --------------------------------------------------------------
mkdir -p "${BIN_DIR}"
install -m 0755 "${src_bin}" "${BIN_DIR}/${BIN}" 2>/dev/null \
	|| { cp "${src_bin}" "${BIN_DIR}/${BIN}" && chmod 0755 "${BIN_DIR}/${BIN}"; }

info ""
info "C0PL4ND installed to ${BIN_DIR}/${BIN}"

# --- next steps -----------------------------------------------------------
case ":${PATH}:" in
	*:"${BIN_DIR}":*)
		info "Run it with: ${BIN}"
		;;
	*)
		info ""
		info "${BIN_DIR} is not on your PATH. Add it by appending this line to"
		info "your shell profile (~/.profile, ~/.bashrc, or ~/.zshrc):"
		info ""
		info "    export PATH=\"${BIN_DIR}:\$PATH\""
		info ""
		info "Then restart your shell and run: ${BIN}"
		;;
esac
