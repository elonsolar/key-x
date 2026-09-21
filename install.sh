#!/bin/bash
# Key-X 一键安装（macOS / Linux）：从 GitHub Releases 下载对应架构二进制
# 用法: curl -fsSL <安装脚本URL> | bash
set -e

REPO="${KEYX_REPO:-elonsolar/key-x}"                # TODO: 发布时替换为实际 owner/repo
VERSION="${KEYX_VERSION:-latest}"
INSTALL_DIR="${KEYX_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) echo "不支持的架构: $arch"; exit 1 ;;
esac
case "$os" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *) echo "不支持的系统: $os（Windows 请用 install.ps1）"; exit 1 ;;
esac
target="${arch}-${os}"
asset="keyx-${target}.tar.gz"
url="https://github.com/${REPO}/releases/${VERSION}/download/${asset}"

echo "下载 ${url}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fL "$url" -o "$tmp/keyx.tar.gz"
tar -xzf "$tmp/keyx.tar.gz" -C "$tmp"

mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/keyx" "$INSTALL_DIR/keyx"
"$INSTALL_DIR/keyx" --version

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "注意: $INSTALL_DIR 不在 PATH。请在 shell 配置加入后重开终端:"
     echo "  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
echo "完成。下一步：对 AI 说『帮我用 keyx 管理密码』，或在终端运行 keyx set <名称>"
