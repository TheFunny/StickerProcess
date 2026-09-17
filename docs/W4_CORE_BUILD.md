# W4：自建 ffmpeg.wasm ST core 配方（highbitdepth）

`assets/ffmpeg-core-st.{js,wasm}` 是**自建 core**，不是 npm 预构建件。29.5MB wasm
（wasm-opt -Oz 后）不入库（.gitignore）。

**产物由 CI 构建并发布**：[`TheFunny/ffmpeg-core-st`](https://github.com/TheFunny/ffmpeg-core-st)
（= 上游 ffmpegwasm/ffmpeg.wasm 的 fork，分支 `st-core`，唯一源码改动是 libvpx 开
highbitdepth）。workflow `build-core.yml`（Actions → *Build ST core* → 填 tag 跑）做五件事：
`docker buildx build`（`FFMPEG_ST=yes`、`EXTRA_CFLAGS="-O3 -msimd128"`，带 buildx 层缓存）
→ `wasm-opt -Oz --strip-debug`（binaryen 132，版本与 sha256 都钉死）→ 产物形状断言 →
headless Chromium 冒烟（GIF alpha + MP4 10-bit + 同实例二次 exec）→ 发 Release
（`ffmpeg-core-st.js`、`ffmpeg-core-st.wasm`、`SHA256SUMS.txt`）。

取件：`scripts/fetch-ffmpeg-core.sh`（**core 版本 tag 与两份 sha256 的唯一出处**，
CI 与本地共用；发新 core 只改这三处）。本文件下面的 Docker/WSL 配方保留作**本地重建
应急路径**与历史记录（CI 挂了、或要在改动上游前的试验分支上迭代时用）。

预构建 `@ffmpeg/core@0.12.10` 有两缺陷，均已在本 core 修复/验证：

| 缺陷 | 预构建 | 本 core 实测 |
|---|---|---|
| libvpx 无 `--enable-vp9-highbitdepth` → MP4 `-pix_fmt yuv420p10` 静默回退 8-bit | 回退 | **真 10-bit**：`vp9 (Profile 2), yuv420p10le` |
| h264→VP9 编码 OOB 崩溃（任意 pix_fmt；崩溃实例后续 exec 永久挂死） | 必崩 | code=0 正常出文件；同实例二次 exec 正常 |

实测数据（无头 Chromium，host CPU 竞争下偏慢）：GIF 2.4s → 35.51KB/10s（与预构建
字节一致，无退化）；MP4 6s 10-bit → 213KB（首跑 490s 属 CPU 饱和，正常量级 ~60s）；
MP4 兜底路由端到端 → Done 208.44KB ≤256KB。

## 本地重建（应急路径；CI 是主路径）

CI（`build-core.yml`）已经把下面 1–6 步全做了；只有在 CI 出问题、或要在改动上游前的
试验分支上快速迭代时，才需要在本机跑一遍。

### 构建环境

- WSL2 Ubuntu + Docker Desktop（Linux 引擎）。构建容器需外网：Docker Desktop
  Settings → Resources → Proxies 配 `http://127.0.0.1:10808`（HTTP+HTTPS，本机
  clash）；`docker pull emscripten/emsdk:3.1.40` 若被墙，走镜像站
  `docker pull docker.1ms.run/emscripten/emsdk:3.1.40 && docker tag … emscripten/emsdk:3.1.40`。
- 内存：Docker Desktop settings `MemoryMiB ≥ 8192`（默认 2048 会拖死编译期 IO）。

### 配方

```bash
# 1) 取源码：直接用 fork 的 st-core 分支（= 上游 f876f907 + libvpx 补丁，
#    即 @ffmpeg/core 0.12.10 的发布源，ABI 与现有 wrapper
#    （assets/ffmpeg.js 0.12.x + 814.ffmpeg.js）完全兼容）
wsl -d Ubuntu
git clone -b st-core https://github.com/TheFunny/ffmpeg-core-st.git ~/ffmpeg-wasm-build
cd ~/ffmpeg-wasm-build
git log --oneline -1   # c0000d6 build the ST core in CI and publish it as a release asset

# 2) 改动（如果要改）：libvpx 开 10-bit 已在 build/libvpx.sh 里
#    （--target=generic-gnu 行后的 --enable-vp9-highbitdepth）；别的改动照常编辑
#    后 docker buildx build -o dist . ——不提交就直接跑，别 push 到 st-core 污染tag

# 3) 网络不稳规避（可选，构建期 git fetch 反复断流时才需要）：
#    宿主侧（带代理+重试）预取 16 个源仓库到 ~/ffmpeg-wasm-build/src-cache/，
#    然后把 Dockerfile 所有 `ADD https://github.com/… /src` 换成
#    `COPY src-cache/<repo> /src`、删掉首行 `# syntax=…master-labs`
#    （补丁后 Dockerfile 备份形态：COPY 16 处、零 ADD https、无 git clone；
#    x264=4-cores x265=3.4 libvpx=v1.13.1 lame=master Ogg=v1.3.4 theora=v1.1.1
#    opus=v1.3.1 vorbis=v1.3.3 zlib=v1.2.11 libwebp=v1.3.2 freetype2=VER-2-10-4
#    fribidi=v1.0.9 harfbuzz=5.2.0 libass=0.15.0 zimg=release-3.0.5 --recursive
#    FFmpeg=n5.1.4，全部 --depth 1）

# 4) 构建 ST 生产 core（~11 min @ 18 核）
make prd        # = FFMPEG_ST=yes + EXTRA_CFLAGS="-O3 -msimd128"

# 5) 产物换入本仓库（UMD only——ESM 变体与本仓库 wrapper 组合不兼容，勿拷）
#    仓库根（Windows git-bash）：
cp assets/ffmpeg-core-st.wasm wasm-demo/ffmpeg-core-st.wasm.pre-w4   # 先备份可回滚
cp ~/ffmpeg-wasm-build/dist/umd/ffmpeg-core.js   assets/ffmpeg-core-st.js
cp ~/ffmpeg-wasm-build/dist/umd/ffmpeg-core.wasm assets/ffmpeg-core-st.wasm
#    部署到构建目录（dx 不拷 assets/，且 python http.server 无缓存头，
#    测前浏览器硬刷新/禁缓存）：
cp assets/ffmpeg-core-st.{js,wasm} target/dx/StickerProcess/debug/web/public/

# 6) wasm-opt 收尾（binaryen，实测 -11%）：core 的 code 段占 84%，上游 make prd
#    的 -O3 是**速度**向，binaryen -Oz 能把 code 段再压掉 13%（data 段不动、
#    15 个 export 与 memory 声明完全一致）。实测（binaryen 132）：
wasm-opt -Oz --strip-debug assets/ffmpeg-core-st.wasm -o assets/ffmpeg-core-st.wasm.opt
mv assets/ffmpeg-core-st.wasm.opt assets/ffmpeg-core-st.wasm
#    raw 33,197,190 → 29,545,000 (-11.0%)  gzip 10,272,653 → 9,904,806 (-3.6%)
#    sha256 3c604655…238fa8 → 512d94ee…3d4e
#    换入后必须跑一遍下面的验证清单（GIF 转码 Done 41.30KB，与未优化 core 字节
#    数一致；耗时同量级 —— 4s vs 6s，无头 Chromium 冷缓存）。
#    注意：-O3 只 -0.3%，-O4 反而更大；要省体积就用 -Oz。
```

## 版本钉与依赖面

- FFmpeg **n5.1.4**（上游有意钉死，Dockerfile 注释：n6 起 CLI 只剩 MT 多线程；
  升 6/7 不在本配方内）；emsdk 3.1.40；libvpx fork **v1.13.1** + highbitdepth。
- ST 链接 flags 里 `-sINITIAL_MEMORY=32MB -sALLOW_MEMORY_GROWTH` 上游自带，无需改。
- 回滚：core 版本就是 `scripts/fetch-ffmpeg-core.sh` 里的 tag + 两个 sha256——改回旧
  tag/哈希再跑一次脚本即可（旧 Release 不删）。引擎矩阵（`Engine::for_web`）不感知
  core 版本，回滚无需改代码。

## 换 core 后的验证清单

1. GIF 回归：http://127.0.0.1:8123（或静态 8199）拖 `2.gif` → Run → Done ~35KB。
2. 10-bit 判据：页面 `FFmpeg.exec` 跑 `-i in.mp4 … -pix_fmt yuv420p10 … out.webm`
   → code=0；产物桌面 `ffmpeg -i out.webm -f null -` 报 `yuv420p10le (Profile 2)`。
3. 兜底路由：预 stub `stickerWebcodecsProbeSupport=()=>false` → 拖 mp4 → Run →
   Done ≤256KB（走 ffmpeg-wasm）。真机 Firefox 为自然复现。
