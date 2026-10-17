#!/bin/sh
# Installs the latest pragma release into ~/.pragma/bin (or $PRAGMA_HOME/bin):
#   curl -fsSL https://raw.githubusercontent.com/xiaojia630/pragma/main/install.sh | sh
# Set PRAGMA_NO_MODIFY_PATH=1 to leave your shell startup file alone.
set -eu

repo="xiaojia630/pragma"
home="${PRAGMA_HOME:-$HOME/.pragma}"

case "$(uname -s)-$(uname -m)" in
  Darwin-*) asset="pragma-macos-universal" ;;
  Linux-x86_64 | Linux-amd64) asset="pragma-linux-x86_64" ;;
  Linux-aarch64 | Linux-arm64) asset="pragma-linux-aarch64" ;;
  *)
    echo "No prebuilt pragma for $(uname -s) $(uname -m) yet." >&2
    echo "Build it from source: https://github.com/$repo/blob/main/docs/pragma/README.md#install" >&2
    exit 1
    ;;
esac

url="https://github.com/$repo/releases/latest/download/$asset.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/pragma.tar.gz"
tar -xzf "$tmp/pragma.tar.gz" -C "$tmp"
mkdir -p "$home/bin"
mv "$tmp/pragma" "$home/bin/pragma"
chmod +x "$home/bin/pragma"
echo "Installed $("$home/bin/pragma" version) to $home/bin/pragma"

case ":$PATH:" in
  *":$home/bin:"*) ;;
  *)
    line="export PATH=\"$home/bin:\$PATH\""
    if [ -n "${PRAGMA_NO_MODIFY_PATH:-}" ]; then
      echo "Add pragma to your PATH: $line"
    else
      case "${SHELL:-}" in
        */zsh) rc="$HOME/.zshrc" ;;
        */bash) rc="$HOME/.bashrc" ;;
        *) rc="$HOME/.profile" ;;
      esac
      grep -qsF "$line" "$rc" || printf '\n# pragma\n%s\n' "$line" >>"$rc"
      echo "Added pragma to your PATH in $rc. Open a new terminal, or run: $line"
    fi
    ;;
esac

echo
echo "Next, let pragma check Claude Code's edits:"
echo "  pragma hooks install"
