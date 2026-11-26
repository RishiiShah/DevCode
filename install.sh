#!/bin/sh
# Installs a DevCode release binary. Usage: sh install.sh [VERSION]
# Env: DEVCODE_INSTALL_DIR (default: $HOME/.local/bin), DEVCODE_REPO (default: RishiiShah/DevCode),
#      DEVCODE_BASE_URL (download directory URL, overrides the GitHub release location)
main() {
  set -eu

  REPO="${DEVCODE_REPO:-RishiiShah/DevCode}"
  DEST="${DEVCODE_INSTALL_DIR:-$HOME/.local/bin}"
  VERSION="${1:-latest}"

  os=$(uname -s)
  arch=$(uname -m)
  case "$os-$arch" in
    Darwin-arm64) target="aarch64-apple-darwin" ;;
    Darwin-x86_64) target="x86_64-apple-darwin" ;;
    Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
    Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
    *) echo "unsupported platform: $os $arch" >&2; exit 1 ;;
  esac

  if [ -n "${DEVCODE_BASE_URL:-}" ]; then
    base="$DEVCODE_BASE_URL"
  elif [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/$VERSION"
  fi
  archive="devcode-$target.tar.gz"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' 0
  echo "downloading $archive"
  curl -fsSL "$base/$archive" -o "$tmp/$archive"
  curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256"

  expected=$(cut -d ' ' -f 1 "$tmp/$archive.sha256")
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$archive" | cut -d ' ' -f 1)
  else
    actual=$(shasum -a 256 "$tmp/$archive" | cut -d ' ' -f 1)
  fi
  if [ "$expected" != "$actual" ]; then
    echo "checksum mismatch for $archive; not installing" >&2
    exit 1
  fi

  tar -xzf "$tmp/$archive" -C "$tmp"
  "$tmp/devcode" --version
  mkdir -p "$DEST"
  install -m 755 "$tmp/devcode" "$DEST/devcode"
  echo "installed $DEST/devcode"
  case ":$PATH:" in
    *":$DEST:"*) ;;
    *) echo "add $DEST to PATH, for example: export PATH=\"$DEST:\$PATH\"" ;;
  esac
}

main "$@"
