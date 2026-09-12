# Web 双端实现规划（W 系列）

> 状态: 规划 (2026-09-09) · 前置: 阶段 1 地基已落地（`06dab36`，wasm 可编译 + Engine 三态 + Bytes 契约）
> 调研依据: `WEB_DEMO_FINDINGS.md`（Route A: ffmpeg.wasm）、`WEB_DEMO_FINDINGS_B.md`（Route B: WebCodecs）

## 0. 结论与引擎矩阵

网页端双引擎：**B（WebCodecs）优先，A（ffmpeg.wasm）回落**。真机实测（2026-09-09，Chrome 152）
**`alpha:'keep'` 不受支持** → GIF/APNG 透明通道在 B 上被拍扁黑底，而透明是贴纸核心诉求。
因此矩阵修正为（A 先行，见 W1）：

| 任务类型 | Chrome/Edge 系 | Firefox/Safari |
|---|---|---|
| GIF / APNG → webm（需 alpha） | **A**（yuva420p，已验证带透明） | A |
| MP4 → webm | **B**（~1s 级，demo 7.07s/6s 视频） | ❌ v1 不支持（见 W4） |
| 图片 → png | B 路径直接可行（Canvas + oxipng wasm），v1 顺带 | 同左 |

非引擎层双端契约（阶段 1 已就绪，无需再动）：
`MediaFile::from_bytes` / `Transcoder::output_bytes` / `store_output` / `Engine::parse` /
`resolve_engine(setting, web_supported)` / config localStorage 持久化。

## W1. ffmpeg.wasm 引擎（A）— GIF/APNG alpha 关键路径

**目标**: 网页端 GIF/APNG → 带 alpha 的 webm，端到端可用。

- [x] **W1.1 JS 模块 `assets/ffmpeg-engine.js`**（实际落地：自举加载 `ffmpeg.js`
  UMD wrapper；导出 `stickerFfmpegReady/Transcode/Probe/Cancel` + `stickerDownload`）
  - ST core（`@ffmpeg/core 0.12.10`，32MB）加载器 + Promise 化封装；
    UMD core 必须 classic worker：wrapper UMD `ffmpeg.js` + 相邻 `814.ffmpeg.js` shim
    （见 FINDINGS.md 坑 1，worker 文件缺失时 load() 静默挂起）
  - 导出（与 B 引擎同一契约，见 W2.1）:
    - `probeSupport() -> {ffmpegOk: true}`（A 恒可用，加载成功即 true）
    - `transcode(bytes, {name, duration, isGif, bitrate, fps, onProgress}) -> Promise<Uint8Array>`
  - 参数照 `Transcoder::gen_command`（`command.rs`）: `-vf scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos`
    `-c:v libvpx-vp9 -pix_fmt yuva420p(GIF/APNG) -crf 26 -b:v <bitrate> -bufsize 1.5x -f webm`
  - 码率公式/系数表/GIF×0.75 patch 复用 Rust 侧计算结果（JS 只收 bitrate，不重复实现）
  - **v1 范围**: 仅 GIF/APNG（MP4 在预构建 core 上 OOB 崩溃，见 FINDINGS.md；MP4 由 B 承接）
- [x] **W1.2 core 资产进仓**（`assets/` 四件套；32MB `.wasm` gitignore，
  其余入库；dx 把 asset 目录复制到 web 根，glue 从根路径加载）
  - `ffmpeg-core-st.{js,wasm}` + `ffmpeg.js` + `814.ffmpeg.js` 入 `web/vendor/`（或 dioxus asset 目录）
  - gitignore `wasm-demo/` 保持（spike 不入库）；core ~32MB 入库前与用户确认（或改 CDN+版本锁定）
- [x] **W1.3 Rust 桥 `transcoder/web.rs`**（`#[cfg(target_arch = "wasm32")]`）
  - wasm-bindgen 调 JS：`transcode()` 接 `MediaFile::from_bytes` 的字节；
    进度经 `Closure` 回调 → mpsc → 现有 10Hz UI 节流（`runner.rs` 接收端零改动）
  - 实现 `run_webcodecs` 同级的 `run_ffmpeg_wasm(on_progress)`，`run_with_progress` 分发接入
  - 完成字节走 `store_output()`（`output_bytes` 已就绪）
- [x] **W1.4 引擎选择**（`resolve_web_engine(media_type)`：GIF/APNG → ffmpeg-wasm，
  MP4/图片 → webcodecs 占位错误；wasm 两段式 prepare/finish 不跨 await 持锁）
  - 签名扩展: `resolve_engine(setting, media_type, caps)`；caps 来自 JS `probeSupport()`
  - 矩阵: GIF/APNG → ffmpeg-wasm；MP4 → webcodecs(支持时)/不支持则报错提示；
    图片 → webcodecs（见 W2.4）
- [x] **W1.5 验收**（真机通过：拖拽/覆盖层高亮/Run/进度/Done ≤256KB/透明输出/下载；
  无头复验 glue 转码 36.1KB 双轨 VP9 alpha）

## W2. WebCodecs 引擎（B）— MP4 主路径

**目标**: Chromium 系 MP4 → webm 走 B；契约与 W1 相同。

- [ ] **W2.1 契约固定（两引擎共用，先写死再实现）**
  ```js
  // web/engine.js（wasm-bindgen 桥接面）
  probeSupport() -> { webcodecs: bool, alphaKeep: bool, tenBit: bool }
  transcode(bytes, {name, duration, isGif, bitrate, fps, onProgress}) -> Promise<Uint8Array>
  ```
- [ ] **W2.2 JS 模块 `web/webcodecs-engine.js`**
  - 移植 `wasm-demo/webcodecs-demo/index.html` 管线:
    `ImageDecoder`(GIF/APNG) / `<video>`+rVFC(MP4) → OffscreenCanvas 512 fit →
    `VideoEncoder(vp09.00.10.08)` → `webm-muxer@5`（30KB，唯一依赖，需传 width/height）
  - MP4 时长/帧数: HTMLMediaElement metadata；GIF: `frames×Σdelay`
  - **v1 仅接 MP4**（alpha 不支持，GIF 归 A）`ponytail: B 引擎 v1 仅 MP4；双流 alpha 封装是后续项`
- [ ] **W2.3 Rust 桥 `transcoder/web/webcodecs.rs`**: 同 W1.3 模式（wasm-bindgen + mpsc 进度）
- [ ] **W2.4 图片 → png**（顺带）: Canvas 重绘 512 fit → `ImageEncoder` → oxipng wasm
  （`oxipng` crate 有 wasm 构建；`store_output` 收优化后字节）
- [ ] **W2.5 验收**: `dx serve` 拖 `input/1.mp4` → Run → Done ≤256KB
  （基线: demo 7.07s/102.5KB）；Firefox 打开 → MP4 任务提示"浏览器不支持"

## W3. 网页端 UI/IO 收尾（两引擎共用）

- [ ] **W3.1 文件输入**: `add_files` wasm 分支接前端 `File` 字节 → `MediaFile::from_bytes`
  （替换 `input_size: 0` 占位；拖拽 + `<input type=file>` 双通道）
- [ ] **W3.2 探测**: wasm 分支 `spawn_probe` 改走 JS 元数据（duration/type 由前端给，
  `probe()` Bytes 已直接 Ok）；显示逻辑复用现有 `Probing → Pending` 流
- [ ] **W3.3 输出预览**: `components/preview.rs` wasm 分支从 `output_bytes` 生成
  objectURL（替换 `None` 占位）；`input` 预览用 objectURL（替换空串占位）
- [ ] **W3.4 输出获取**: 任务行输出大小点击 → 下载（`<a download>`，替换 explorer 占位）；
  输出目录概念在网页端隐藏（`toolbar.rs` Select 按钮已 cfg）
- [ ] **W3.5 设置面板**: 引擎下拉 wasm 侧显示实际可用引擎（sidecar/webcodecs 桌面项置灰逻辑已有，
  wasm 反向：sidecar 置灰、inprocess 置灰、webcodecs/ffmpeg-wasm 按 caps）
- [ ] **W3.6 验收**: 完整用户流——拖入混合队列（gif+mp4+png）→ Run → 逐个完成 →
  预览/下载；设置持久化跨刷新（localStorage）

## W4. 自建 wasm core（解锁 A 的 MP4 + 10-bit）

**目标**: 解决预构建 core 两缺陷（MP4 OOB、无 highbitdepth）。独立工作项，不阻塞 W1–W3。

- [ ] ffmpeg 6/7 emscripten 构建: `--enable-vp9-highbitdepth` + `-sALLOW_MEMORY_GROWTH`
  + `-sEXPORTED_FUNCTIONS` 对齐 wrapper 期望（参照 E6 静态构建记录 §7 的 vcpkg 配方映射）
- [ ] 产出替换 `web/vendor/` core；A 引擎范围扩到 MP4（yuv420p10le 桌面对齐）
- [ ] 回归: A 的 GIF 基线不退化；MP4 端到端通过

## W5. 收尾与发布

- [ ] **W5.1 部署**: 静态托管（`dx build --platform web` 产物）；COOP/COEP 不需要
  （ST core 无 SharedArrayBuffer）；core 32MB 首载加 loading 态 + 可选 IndexedDB 缓存（v2）
- [ ] **W5.2 文档**: AGENTS.md 增 web 架构段（引擎矩阵/桥接层/资产目录）；README 部署说明
- [ ] **W5.3 测试**: 码率/系数纯函数已有；新增 resolve_engine 矩阵测试（media_type × caps 全组合）
- [ ] **W5.4 里程碑**

| 阶段 | 内容 | 预估 |
|---|---|---|
| W1 | ffmpeg.wasm 引擎 + GIF alpha | 2–3 天 |
| W2 | WebCodecs 引擎 + 图片 | 2 天 |
| W3 | UI/IO 收尾 | 1–2 天 |
| W4 | 自建 core（可选） | 2–3 天 |
| W5 | 部署文档 | 1 天 |

## 落地顺序

W1 → W2 → W3（一个可发布的网页端 v1）→ W5 → W4（独立增强，随时可插）。
W1/W2 内部：JS 模块先行（demo 已验证，纯搬运），Rust 桥随后，引擎选择最后接线。
每步可独立验证：W1 完成即 GIF 端到端可用，W2 完成即 MP4 可用。

## 风险与既知限制

- **alpha**: B 引擎所有 Chrome 均不支持 `alpha:'keep'`（真机 2026-09-09 实测）——v1 靠矩阵规避；
  双流封装（color+alpha 第二轨，WhatsApp 网页同款）列为 v2 候选
- **10-bit**: 预构建 core 无 highbitdepth（静默回退 8-bit）——W4 解决
- **core 体积**: 32MB 首载（demo 实测本地 ~2s）；CDN/缓存策略 W5 定
- **MP4 OOB（预构建 core）**: A 引擎 v1 不接 MP4，W4 后放开
- **run_blocking wasm 分支**: 当前直接执行阻塞 UI；W1 接桥时转 Web Worker 或保持
  （转码在 JS worker 内，Rust 侧只是 await Promise，实际不阻塞——实现时确认）
