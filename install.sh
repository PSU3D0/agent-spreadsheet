#!/bin/sh

set -eu

repo="PSU3D0/agent-spreadsheet"
install_mcp=${ASP_INSTALL_MCP:-0}

usage() {
    cat <<'EOF'
Usage: install.sh [--mcp]

Environment:
  ASP_VERSION      Release version to install (for example, 0.12.0 or v0.12.0)
  ASP_INSTALL_DIR  Destination directory (default: $HOME/.local/bin)
  ASP_INSTALL_MCP  Set to 1 to also install agent-spreadsheet-mcp
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --mcp)
            install_mcp=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

os_name=$(uname -s)
case "$os_name" in
    Linux)
        os=linux
        ;;
    Darwin)
        os=macos
        ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
        echo "Native agent-spreadsheet 0.16 releases support Linux and macOS; Windows binaries are not provided." >&2
        exit 1
        ;;
    *)
        echo "Unsupported operating system: $os_name" >&2
        exit 1
        ;;
esac

arch_name=$(uname -m)
case "$arch_name" in
    x86_64|amd64)
        arch=x86_64
        ;;
    aarch64|arm64)
        arch=aarch64
        ;;
    *)
        echo "Unsupported architecture: $arch_name" >&2
        exit 1
        ;;
esac

if command -v curl >/dev/null 2>&1; then
    downloader=curl
elif command -v wget >/dev/null 2>&1; then
    downloader=wget
else
    echo "Installation requires curl or wget." >&2
    exit 1
fi

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/asp-install.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

download() {
    download_url=$1
    download_dest=$2
    if [ "$downloader" = curl ]; then
        curl -fsSL --retry 2 --output "$download_dest" "$download_url"
    else
        wget -q --tries=3 --output-document="$download_dest" "$download_url"
    fi
}

latest_from_redirect() {
    latest_url="https://github.com/$repo/releases/latest"
    if [ "$downloader" = curl ]; then
        curl -fsSL --output /dev/null --write-out '%{url_effective}' "$latest_url"
    else
        wget --spider --server-response --max-redirect=20 "$latest_url" 2>&1 |
            awk '/^[[:space:]]*Location:/ { url=$2 } END { sub(/\r$/, "", url); print url }'
    fi
}

if [ -n "${ASP_VERSION:-}" ]; then
    version=${ASP_VERSION#v}
else
    api_url="https://api.github.com/repos/$repo/releases/latest"
    version=""
    if download "$api_url" "$tmp_dir/latest.json" 2>/dev/null; then
        version=$(sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' "$tmp_dir/latest.json" | head -n 1)
        version=${version#v}
    fi
    if [ -z "$version" ]; then
        echo "GitHub API lookup failed; resolving the latest release redirect instead." >&2
        redirect_url=$(latest_from_redirect)
        version=${redirect_url##*/}
        version=${version#v}
    fi
fi

if [ -z "$version" ]; then
    echo "Could not determine the release version." >&2
    exit 1
fi

release_base="https://github.com/$repo/releases/download/v$version"
asset_suffix="$os-$arch"
checksums_file="$tmp_dir/SHA256SUMS"
checksums_available=0
if download "$release_base/SHA256SUMS" "$checksums_file" 2>/dev/null; then
    checksums_available=1
else
    echo "Notice: SHA256SUMS is not available for v$version; continuing without checksum verification." >&2
fi

verify_asset() {
    verify_file=$1
    verify_name=$2
    if [ "$checksums_available" -ne 1 ]; then
        return 0
    fi

    expected=$(awk -v name="$verify_name" '$2 == name { print $1; exit }' "$checksums_file")
    if [ -z "$expected" ]; then
        echo "SHA256SUMS does not contain $verify_name." >&2
        return 1
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$verify_file" | awk '{ print $1 }')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$verify_file" | awk '{ print $1 }')
    else
        echo "SHA256SUMS is present, but neither sha256sum nor shasum is available." >&2
        return 1
    fi

    if [ "$actual" != "$expected" ]; then
        echo "Checksum verification failed for $verify_name." >&2
        return 1
    fi
    echo "Verified $verify_name"
}

fetch_component() {
    component=$1
    output_file=$2
    archive_name="$component-$asset_suffix.tar.gz"
    raw_name="$component-$asset_suffix"
    archive_file="$tmp_dir/$archive_name"

    echo "Downloading $component v$version for $os/$arch"
    if download "$release_base/$archive_name" "$archive_file" 2>/dev/null; then
        verify_asset "$archive_file" "$archive_name"
        extract_dir="$tmp_dir/extract-$component"
        mkdir -p "$extract_dir"
        tar -xzf "$archive_file" -C "$extract_dir"
        if [ ! -f "$extract_dir/$component" ]; then
            echo "$archive_name does not contain $component." >&2
            return 1
        fi
        cp "$extract_dir/$component" "$output_file"
    else
        echo "Archive not found; falling back to the raw binary asset." >&2
        raw_file="$tmp_dir/$raw_name"
        if ! download "$release_base/$raw_name" "$raw_file"; then
            echo "Could not download $raw_name from release v$version." >&2
            return 1
        fi
        verify_asset "$raw_file" "$raw_name"
        cp "$raw_file" "$output_file"
    fi
    chmod 755 "$output_file"
}

install_dir=${ASP_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$install_dir"
fetch_component agent-spreadsheet "$install_dir/agent-spreadsheet"
ln -sf agent-spreadsheet "$install_dir/asp"

if [ "$install_mcp" = 1 ]; then
    fetch_component agent-spreadsheet-mcp "$install_dir/agent-spreadsheet-mcp"
fi

installed_version=$("$install_dir/agent-spreadsheet" --version)
echo "Installed $installed_version in $install_dir"

case ":${PATH:-}:" in
    *:"$install_dir":*)
        ;;
    *)
        echo "Add $install_dir to PATH to run 'asp' from any directory."
        ;;
esac
