#!/usr/bin/env bash
# pi-rust installer: latest (or PI_RUST_VERSION=) release from GitHub.
# Installs to ~/.local/share/pi-rust, links ~/.local/bin/pi-rust.
# Requires: bash, curl, tar, node >= 22.19 (the extension sidecar).
set -euo pipefail

repo="ripmav/pi-rust"
install_root="${PI_RUST_INSTALL_ROOT:-$HOME/.local/share/pi-rust}"
bin_dir="${PI_RUST_BIN_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="linux" ;;
  Darwin) os="darwin" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac
case "$os-$arch" in
  linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  linux-aarch64) target="aarch64-unknown-linux-musl" ;;
  darwin-x86_64) target="x86_64-apple-darwin" ;;
  darwin-aarch64) target="aarch64-apple-darwin" ;;
esac

# Latest non-prerelease release (or the pinned version).
if [ -n "${PI_RUST_VERSION:-}" ]; then
  tag="$PI_RUST_VERSION"
else
  tag="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
  [ -n "$tag" ] || { echo "could not resolve latest release" >&2; exit 1; }
fi

asset="pi-rust-${tag#v}-${target}.tar.gz"
url="https://github.com/$repo/releases/download/$tag/$asset"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "installing $tag ($target) -> $install_root"
curl -fsSL "$url" -o "$tmp/pkg.tar.gz"
tar -xzf "$tmp/pkg.tar.gz" -C "$tmp"
rm -rf "$install_root"
mkdir -p "$install_root" "$bin_dir"
cp -r "$tmp/pi-rust-${tag#v}-${target}/." "$install_root/"
ln -sf "$install_root/pi-rust" "$bin_dir/pi-rust"

"$install_root/pi-rust" --provider fake --prompt hi --agent-dir "$(mktemp -d)" >/dev/null
echo "done: $bin_dir/pi-rust (needs node >= 22.19 on PATH for extensions)"
