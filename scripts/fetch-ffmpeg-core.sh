#!/usr/bin/env bash
# 取回自建 ffmpeg.wasm ST core（桌面/网页端共用），并逐份校验 sha256。
#
# core 由 https://github.com/TheFunny/ffmpeg-core-st 的 "Build ST core" workflow
# 构建（上游 f876f907 + libvpx --enable-vp9-highbitdepth，wasm-opt -Oz，冒烟通过
# 后才发 Release）。**这个脚本是 core 版本的唯一出处**：升级 core = 改下面的
# TAG 与两个 sha256（新 Release 的 SHA256SUMS.txt 里抄），CI 与本地开发共用。
#
# 用法：在仓库根跑 `bash scripts/fetch-ffmpeg-core.sh`（默认写 assets/），
# 或 `bash scripts/fetch-ffmpeg-core.sh <目录>`。刻意用**相对**路径：Windows 下
# bash 与 curl 是两套路径语法（/mnt/d vs /d vs D:），绝对路径会把两者搅在一起。
set -euo pipefail

TAG="st-core-v1"
SHA_JS="74d1c39ed88c195e99efd10ca9fb07588af962e7503c7de5a7f444485cd6f663"
SHA_WASM="4bcc85dd7653ca1c0ac027c4694c78bea516117528f40792dcb92c2d7e3bf0ec"

dest="${1:-assets}"
base="https://github.com/TheFunny/ffmpeg-core-st/releases/download/$TAG"

mkdir -p "$dest"
# 让 shell 负责写文件（重定向），curl 只写 stdout：Windows 下 curl 是原生程序，
# 认不出 /mnt/d/... 或 /d/... 这类 MSYS/WSL 路径，直接 -o 到那种路径会失败。
for name in ffmpeg-core-st.js ffmpeg-core-st.wasm; do
  if ! curl -fsSL --retry 3 --retry-delay 2 "$base/$name" > "$dest/$name"; then
    rm -f "$dest/$name"
    echo "download failed: $base/$name" >&2
    exit 1
  fi
done

verify() { # <file> <expected sha256>
  local got
  got=$(sha256sum "$1" | cut -d' ' -f1)
  if [ "$got" != "$2" ]; then
    echo "sha256 mismatch for $1" >&2
    echo "  got      $got" >&2
    echo "  expected $2" >&2
    exit 1
  fi
}

verify "$dest/ffmpeg-core-st.js" "$SHA_JS"
verify "$dest/ffmpeg-core-st.wasm" "$SHA_WASM"
echo "ffmpeg core $TAG ok: wasm $(stat -c%s "$dest/ffmpeg-core-st.wasm") bytes"
