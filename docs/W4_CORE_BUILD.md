# W4：自建 ffmpeg.wasm ST core 配方（highbitdepth）

`assets/ffmpeg-core-st.{js,wasm}` 是**自建 core**，不是 npm 预构建件。32MB wasm 不入库
（.gitignore），本文件是其唯一出处记录。预构建 `@ffmpeg/core@0.12.10` 有两缺陷，
均已在本 core 修复/验证：

| 缺陷 | 预构建 | 本 core 实测 |
|---|---|---|
| libvpx 无 `--enable-vp9-highbitdepth` → MP4 `-pix_fmt yuv420p10` 静默回退 8-bit | 回退 | **真 10-bit**：`vp9 (Profile 2), yuv420p10le` |
| h264→VP9 编码 OOB 崩溃（任意 pix_fmt；崩溃实例后续 exec 永久挂死） | 必崩 | code=0 正常出文件；同实例二次 exec 正常 |

实测数据（无头 Chromium，host CPU 竞争下偏慢）：GIF 2.4s → 35.51KB/10s（与预构建
字节一致，无退化）；MP4 6s 10-bit → 213KB（首跑 490s 属 CPU 饱和，正常量级 ~60s）；
MP4 兜底路由端到端 → Done 208.44KB ≤256KB。

## 构建环境

- WSL2 Ubuntu + Docker Desktop（Linux 引擎）。构建容器需外网：Docker Desktop
  Settings → Resources → Proxies 配 `http://127.0.0.1:10808`（HTTP+HTTPS，本机
  clash）；`docker pull emscripten/emsdk:3.1.40` 若被墙，走镜像站
  `docker pull docker.1ms.run/emscripten/emsdk:3.1.40 && docker tag … emscripten/emsdk:3.1.40`。
- 内存：Docker Desktop settings `MemoryMiB ≥ 8192`（默认 2048 会拖死编译期 IO）。

## 配方

```bash
# 1) 取源码（钉死 commit——即 @ffmpeg/core 0.12.10 的发布源，ABI 与现有
#    wrapper（assets/ffmpeg.js 0.12.x + 814.ffmpeg.js）完全兼容）
wsl -d Ubuntu
git clone https://github.com/ffmpegwasm/ffmpeg.wasm.git ~/ffmpeg-wasm-build
cd ~/ffmpeg-wasm-build
git checkout f876f907c7e9b9bf51d4ed0b913a855a63ae63fc   # main @ 2026-09

# 2) 唯一功能改动：libvpx 开 10-bit（build/libvpx.sh CONF_FLAGS 数组，
#    --target=generic-gnu 行后插入）
#    --enable-vp9-highbitdepth

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
cp ~/ffmpeg-wasm-build/packages/core/dist/umd/ffmpeg-core.js   assets/ffmpeg-core-st.js
cp ~/ffmpeg-wasm-build/packages/core/dist/umd/ffmpeg-core.wasm assets/ffmpeg-core-st.wasm
#    部署到构建目录（dx 不拷 assets/，且 python http.server 无缓存头，
#    测前浏览器硬刷新/禁缓存）：
cp assets/ffmpeg-core-st.{js,wasm} target/dx/StickerProcess/debug/web/public/
```

## 版本钉与依赖面

- FFmpeg **n5.1.4**（上游有意钉死，Dockerfile 注释：n6 起 CLI 只剩 MT 多线程；
  升 6/7 不在本配方内）；emsdk 3.1.40；libvpx fork **v1.13.1** + highbitdepth。
- ST 链接 flags 里 `-sINITIAL_MEMORY=32MB -sALLOW_MEMORY_GROWTH` 上游自带，无需改。
- 回滚：`git checkout assets/ffmpeg-core-st.js` + 拷回 `.pre-w4` 备份即可；
  引擎矩阵（`Engine::for_web`）不感知 core 版本，回滚无需改代码。

## 换 core 后的验证清单

1. GIF 回归：http://127.0.0.1:8123（或静态 8199）拖 `2.gif` → Run → Done ~35KB。
2. 10-bit 判据：页面 `FFmpeg.exec` 跑 `-i in.mp4 … -pix_fmt yuv420p10 … out.webm`
   → code=0；产物桌面 `ffmpeg -i out.webm -f null -` 报 `yuv420p10le (Profile 2)`。
3. 兜底路由：预 stub `stickerWebcodecsProbeSupport=()=>false` → 拖 mp4 → Run →
   Done ≤256KB（走 ffmpeg-wasm）。真机 Firefox 为自然复现。
