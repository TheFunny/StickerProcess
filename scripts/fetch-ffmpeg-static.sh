#!/usr/bin/env bash
# 取回 CI 构建的静态 ffmpeg（x64-windows-static），解包到 ffmpeg-dist/ 当 FFMPEG_DIR。
#
# 由 https://github.com/TheFunny/ffmpeg-static-win 的 "Build static ffmpeg"
# workflow 构建（vcpkg 树按 commit 钉死，ffmpeg 9.0.1 + libvpx 1.16.0[highbitdepth]，
# 10-bit VP9 冒烟通过才发 Release）。**这个脚本是静态包版本的唯一出处**：
# 升级 = 改下面的 TAG 与 SHA256（新 Release 的 SHA256SUMS.txt 里抄），
# release-desktop.yml / ci.yml 与本地开发共用。
#
# 用法：在仓库根跑 `bash scripts/fetch-ffmpeg-static.sh`（默认写 ffmpeg-dist/），
# 或 `bash scripts/fetch-ffmpeg-static.sh <目录>`。
set -euo pipefail

TAG="ffmpeg-static-v2"
SHA256="48ee024fc5fc393f00c71b334811688ea6d7d91da5b0770d380cb884d9683b25"

dest="${1:-ffmpeg-dist}"
tarball="ffmpeg-static-x64-win.tar.xz"
url="https://github.com/TheFunny/ffmpeg-static-win/releases/download/$TAG/$tarball"

# 让 shell 负责写文件（重定向），curl 只写 stdout：Windows 下 curl 是原生程序，
# 认不出 /mnt/d/... 或 /d/... 这类 MSYS/WSL 路径，直接 -o 到那种路径会失败。
if ! curl -fsSL --retry 3 --retry-delay 2 "$url" > "$tarball"; then
  rm -f "$tarball"
  echo "download failed: $url" >&2
  exit 1
fi

got=$(sha256sum "$tarball" | cut -d' ' -f1)
if [ "$got" != "$SHA256" ]; then
  rm -f "$tarball"
  echo "sha256 mismatch for $tarball" >&2
  echo "  got      $got" >&2
  echo "  expected $SHA256" >&2
  exit 1
fi

mkdir -p "$dest"
tar -xJf "$tarball" -C "$dest"
rm -f "$tarball"

if [ ! -f "$dest/lib/avcodec.lib" ]; then
  echo "unexpected package layout: $dest/lib/avcodec.lib is missing" >&2
  exit 1
fi
echo "ffmpeg static $TAG ok: $(du -sh "$dest" | cut -f1) in $dest (set FFMPEG_DIR to it)"
