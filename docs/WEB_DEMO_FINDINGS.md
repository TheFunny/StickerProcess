# Route A Spike: ffmpeg.wasm 网页端可行性 Demo（2026-09-08）

## 结论
**可行，GIF 路径端到端跑通。** 预构建 core 有一个 h264→VP9 的已知级别缺陷，非架构性阻塞。

## 产物
`wasm-demo/`（gitignored 候选，未入库）：
- `index.html` — 单页 demo，复刻 `command.rs` 的码率公式/系数表/GIF×0.75 patch
- `ffmpeg.js` + `814.ffmpeg.js` — @ffmpeg/ffmpeg 0.12.15 UMD wrapper + 其 classic worker shim（本地文件，必须相邻）
- `ffmpeg-core-st.{js,wasm}` — @ffmpeg/core 0.12.10 单线程 core（~32 MB）
- `serve.py` — python 静态服务器（ST core 不需要 COOP/COEP）

## 实测结果（headless Chromium）
| 路径 | 结果 |
|---|---|
| GIF (2.gif 2.4s) → VP9 webm (yuva420p) | ✅ 5.7s，35.4 KB（限额 256 KB），页面内嵌播放+下载 |
| h264 解码 (1.mp4) | ✅ 解码正常 |
| MP4 → VP9 (yuv420p/yuv420p10le/yuva420p 任意 pix_fmt) | ❌ core wasm OOB 崩溃（ST）/ 无限挂起（MT） |
| 崩溃恢复 | ✅ 重置 `ffmpeg=null` 重载 core 后 GIF 再次成功 |

速度：wasm VP9 ≈ 0.4x 实时（无 asm、x86_32）；原生 sidecar 同任务远快于此。

## 关键坑（换人再接必读）
1. **0.12.x 组装矩阵**：UMD core 必须 `importScripts` → 只能在 classic worker 里跑；
   ESM wrapper 默认 spawn module worker（无 importScripts）→ 必须传
   `classWorkerURL` 指向 UMD 的 `814.ffmpeg.js`（或用 UMD wrapper 让它自动找相邻文件）。
   本地部署 wrapper 的 worker 文件缺失时 `load()` 永久挂起、无报错。
2. **MT core**（core-mt 0.12.10，需 COOP/COEP + SharedArrayBuffer）：在 headless 下
   VP9 编码起始即挂死（有无 -row-mt 均挂）。未在真实有头浏览器验证。
3. **MP4 崩溃**：0.12.10 core 内置 ffmpeg 5.0 (Lavf59.27/Lavc59.37)，libvpx wasm
   构建（`--disable-asm` generic-gnu）对 h264/yuv420p 输入编码即 OOB。GIF (bgra) 输入正常。
   修法 = 自建 core：ffmpeg 6/7 + `-sALLOW_MEMORY_GROWTH` + `--enable-vp9-highbitdepth`
   （对齐桌面版 vcpkg `libvpx[highbitdepth]`），工作量约等于 E6 phase 1 的静态构建记录。
4. **10-bit 缺失**：预构建 core 的 libvpx 无 highbitdepth，`yuv420p10le` 被静默回退 8-bit。
5. headless evaluate 有 30s 上限；长任务要把 await 放页面内，异步轮询 DOM。

## 对正式移植的含义
- Dioxus `web` feature 直接换 UI target；`config.rs`→localStorage；`preview://`→object URL；
  `command.rs` 纯函数原样保留（demo 里 JS 复刻版与其逐参一致）。
- 引擎层新增第三引擎 `"wasm"`：桥接 JS `FFmpeg.exec`，`MediaFile` 需字节化抽象。
- 若走自建 core，同一套 build 脚本可产出 ST/MT 两个 flavor；MP4 修复与 10-bit 支持一并解决。
