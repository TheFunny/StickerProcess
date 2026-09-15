# Web 双端实现规划（W 系列）

> 状态: 规划 (2026-09-09) · 前置: 阶段 1 地基已落地（`06dab36`，wasm 可编译 + Engine 三态 + Bytes 契约）
> 调研依据: `WEB_DEMO_FINDINGS.md`（Route A: ffmpeg.wasm）、`WEB_DEMO_FINDINGS_B.md`（Route B: WebCodecs）

## 0. 结论与引擎矩阵

网页端双引擎：**B（WebCodecs）优先，A（ffmpeg.wasm）回落**。真机实测（2026-09-09，Chrome 152）
**`alpha:'keep'` 不受支持** → GIF/APNG 透明通道在 B 上被拍扁黑底，而透明是贴纸核心诉求。
因此矩阵修正为（A 先行，见 W1）：

| 任务类型 | Chrome/Edge 系 | Firefox | Safari |
|---|---|---|---|
| GIF / APNG → webm（需 alpha） | **A**（yuva420p，已验证带透明） | A | A |
| MP4 → webm | **B**（~1s 级，demo 7.07s/6s 视频）；B caps（VP9 isConfigSupported + rVFC）不可用时 → A 兜底（W4：yuv420p10 已验证） | 133+ 三件齐，**真机实测走 B Done（2026-09-14）**；130/131 无 rVFC → A | 由 caps 探针判定（VP9 编码支持不稳定，A 兜底恒可用） |
| 图片 → png | B（Canvas convertToBlob，无 oxipng——见 W2.4 ponytail） | 同左 | 同左 |

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

- [x] **W2.1 契约固定**（实际落地面：`stickerWebcodecsProbeSupport/Transcode/Cancel`
  + `stickerNativeProbe`；`assets/webcodecs-engine.js`）
  ```js
  // web/engine.js（wasm-bindgen 桥接面）
  probeSupport() -> { webcodecs: bool, alphaKeep: bool, tenBit: bool }
  transcode(bytes, {name, duration, isGif, bitrate, fps, onProgress}) -> Promise<Uint8Array>
  ```
- [x] **W2.2 JS 模块 `assets/webcodecs-engine.js`**（唯一新依赖 webm-muxer@5 入
  `assets/`；MP4/图片路径全带看门狗超时：探测 10s、播放 30s+20s/秒——无 H.264
  解码器的浏览器不报错而是永久挂起，缺它任务卡死）
  - 移植 `wasm-demo/webcodecs-demo/index.html` 管线:
    `ImageDecoder`(GIF/APNG) / `<video>`+rVFC(MP4) → OffscreenCanvas 512 fit →
    `VideoEncoder(vp09.00.10.08)` → `webm-muxer@5`（30KB，唯一依赖，需传 width/height）
  - MP4 时长/帧数: HTMLMediaElement metadata；GIF: `frames×Σdelay`
  - **v1 仅接 MP4**（alpha 不支持，GIF 归 A）`ponytail: B 引擎 v1 仅 MP4；双流 alpha 封装是后续项`
- [x] **W2.3 Rust 桥**（并入 `transcoder/web.rs`：`exec_webcodecs` +
  `prepare_web_job` 的 engine/kind 分发，两段式不跨 await 持锁）
- [x] **W2.4 图片 → png**（Canvas 重绘 512 fit → `convertToBlob(png)`；
  `ponytail:` oxipng 在 wasm 需 clang 工具链，浏览器 PNG 直出足够 <512KB，超限再接）
- [x] **W2.5 验收**：拖 `input/1.mp4` → Run → Done ≤256KB（真机 Firefox 实测通过，
  走 B 快路径）。原预期"Firefox → 提示浏览器不支持"已被证伪——Firefox 133+ 三件齐
  （WebCodecs/rVFC/ImageDecoder），caps 探针返真、MP4 走 WebCodecs 正常；非 Chromium
  的 MP4 兜底 ffmpeg-wasm 仅对 caps 不可用的浏览器（Firefox 130/131 无 rVFC、
  Safari VP9 preview）生效（见 W4 与 WEB_DEMO_FINDINGS_B 修正）

## W3. 网页端 UI/IO 收尾（两引擎共用）

- [x] **W3.1 文件输入**（W1/W2 期间已完成：拖拽 + `<input type=file>` 双通道 →
  `add_file_bytes`）
  （替换 `input_size: 0` 占位；拖拽 + `<input type=file>` 双通道）
- [x] **W3.2 探测**（W2 已完成：`stickerNativeProbe` 统一 GIF/PNG(APNG 纠正)/MP4 元数据）
  `probe()` Bytes 已直接 Ok）；显示逻辑复用现有 `Probing → Pending` 流
- [x] **W3.3 输出预览**（输入=Blob objectURL（按 名字+大小 缓存/ revoke）、
  输出=锁内克隆 `output_bytes` → data URL（复用 (路径,大小) 缓存键）；真机布局
  无头实测：blob:http… + data:video/webm;base64… 双 pane 加载成功）
- [x] **W3.4 输出获取**（W2 期间已完成：`stickerDownload` 点击输出大小下载）
- [x] **W3.5 设置面板**（wasm 下拉列 ffmpeg-wasm / WebCodecs 两项，WebCodecs 按
  caps（`isConfigSupported` vp9）置灰，探测在面板打开时做一次并缓存；矩阵即策略，
  手选弹提示回弹 Auto。桌面反向维持原样）
- [x] **W3.6 验收**（无头完成：混合队列逐个 Done/预期 Alert、APNG 纠正→webm、
  预览双 pane 加载、主题跨刷新读回；MP4 真机项见 W2.5）

## W4. 自建 wasm core（解锁 A 的 MP4 + 10-bit）

**目标**: 解决预构建 core 两缺陷（MP4 OOB、无 highbitdepth）。独立工作项，不阻塞 W1–W3。

- [x] 自建 core 构建（上游 `f876f90` main，FFmpeg **n5.1.4**——上游有意钉死，n6
  仅 MT 可行；emsdk 3.1.40 自带 `-sALLOW_MEMORY_GROWTH`、bind.js 导出 ABI，两项无需改动；
  唯一功能编辑 = libvpx.sh 加 `--enable-vp9-highbitdepth`）
- [x] 产出替换 core；MP4 接入 ffmpeg-wasm 兜底（WebCodecs caps 不可用时，
  `Engine::for_web` 三元组 + WebJob.pix_fmt → glue）；桌面 yuv420p10 对齐实测：
  产物 `vp9 (Profile 2), yuv420p10le`（真 10-bit，预构建回退 8-bit 的缺陷已修）
- [x] 回归：GIF 基线 35.51KB/10s 无退化；MP4 端到端通过（无头模拟 Firefox：caps→false
  + 时长 stub 6.0s → Run → Done 208.44KB；OOB 崩溃修复，同实例二次 exec 正常）

## W5. 收尾与发布

- [x] **W5.1 部署**（release 构建实测：产物纯静态、无 COOP/COEP 要求；
  ⚠ dx release 不拷项目 assets/——部署前需手动补 7 个 glue/core 文件（已写进
  README"网页端"节）；core 32MB 首载 loading 态 / IndexedDB 缓存留 v2）
- [x] **W5.2 文档**（AGENTS.md：引擎矩阵/web.rs/assets/ 布局 + wasm 三坑 Gotchas；
  README："网页端"节 + 构建/部署命令 + 架构图补 web.rs）
- [x] **W5.3 测试**（矩阵抽成 `Engine::for_web` 纯函数 + `for_web_matrix` 桌面可跑，
  取代 wasm-only 的 resolve_web_engine 记录面；码率/系数/时长补丁均已有）
- [x] **W5.4 里程碑**：W1–W3+W5 完成 → 可发布网页端 v1（Chromium 系全类型；
  GIF/APNG→ffmpeg.wasm alpha、MP4/图片→WebCodecs）。W4 自建 core 为可选增强，
  解锁非 Chromium 的 MP4 与 10-bit，不阻塞发布。

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
  双流 in-block 封装已 spike 判死（WebCodecs 收下 I420A 但不产 alpha 位流，产物是
  骗过探测的坏文件，见 WEB_DEMO_FINDINGS_C）
- **10-bit**: ✅ W4 已解决——自建 core（libvpx highbitdepth）实测 `yuv420p10le`
- **core 体积**: 32MB 首载（本地 ~3s，CI/CDN 视网络）；✅ v2 已做流式进度映射 +
  immutable 缓存头配方（README）；IndexedDB 判定不做（HTTP 缓存等价）
- **MP4 OOB（预构建 core）**: ✅ W4 已解决——自建 core 上 h264→VP9 code=0，
  同实例二次 exec 不挂；MP4 经 `Engine::for_web` 在 WebCodecs caps 不可用时兜底 ffmpeg-wasm
- **run_blocking wasm 分支**: 当前直接执行阻塞 UI；W1 接桥时转 Web Worker 或保持
  （转码在 JS worker 内，Rust 侧只是 await Promise，实际不阻塞——实现时确认）
