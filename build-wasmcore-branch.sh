#!/bin/bash
# 把 assets/ffmpeg-core-st.wasm 以单文件提交推到 wasm-core 孤儿分支。
# core 32MB 不入库（.gitignore），此分支是它在远端的唯一出处。
set -e
git update-ref -d refs/heads/wasm-core 2>/dev/null || true
blob=$(git hash-object -w assets/ffmpeg-core-st.wasm)
tree=$(printf "100644 blob %s\tffmpeg-core-st.wasm\n" "$blob" | git mktree)
export GIT_AUTHOR_NAME=YoursFunny GIT_AUTHOR_EMAIL=admin@yoursfunny.top GIT_COMMITTER_NAME=YoursFunny GIT_COMMITTER_EMAIL=admin@yoursfunny.top
commit=$(git commit-tree "$tree" -m "ffmpeg core wasm for web deploy (self-built, see docs/W4_CORE_BUILD.md)")
git update-ref refs/heads/wasm-core "$commit"
git log --oneline -1 wasm-core
