# 兼容性报告按钮 — 开工前可行性报告

> **状态：已实施（2026-09-20）**。与本文的差异（实施时按实测收敛）：
> - 路由行的状态语义定为 `INFO`；MP4 的 WebCodecs 快路径缺条件改为 **WARN**（兜底
>   ffmpeg.wasm 仍能出贴纸，只是慢 + 29.5MB core），只有图片路径缺
>   `ImageDecoder`/`OffscreenCanvas` 才是 FAIL。
> - **VP9 10-bit caps 行没做**：浏览器的 10-bit VP9 与 ffmpeg-wasm 兜底路径无关
>   （那是 wasm 里的 libvpx），报它只是噪音。
> - CSS 实际新增 18 行（`.compat-row/-state/-detail`），没有复用 `.badge`——它定宽
>   108px 且带 Processing 的 spinner 伪元素。
> - web 侧的 `collect()` 落在 `src/compat.rs` 的 `mod web`（`webcodecs_supported`
>   拆出的 `probe_details`/`compat_assets` 在 `transcoder/web.rs`）。
> - Re-check 最初清空报告再重探，弹窗高度从满屏塌成一行再撑开 = 肉眼明显闪烁；
>   改为**保留旧行**、按钮切 `Re-checking…`（探测期间禁用），高度全程不变
>   （实测 web 570px / 桌面 558px 恒定）。
> - 弹窗滚轮死区（标题/动作行/内边距 ~127px）：把滚动容器从 `.modal-body` 换成
>   弹窗自身 + 头脚 sticky（见 app.css 的 `.modal-settings`）。默认窗口 960×680 下
>   报告只溢出 ~39px，指针自然落在下沿/动作行 → 旧结构"只剩半行却滚不动"。

**结论**：可行，且绝大部分数据**已经存在**（探测代码都已落地，只是散在设置面板/runner
里、只输出一个 bool）。新增成本 ≈ 1 个弹窗组件 + 1 个桌面采集模块 + 1 个 JS 汇总函数，
**不改动转码路径、不改引擎矩阵**。

---

## 1. 需求拆解

界面上一个入口 → 弹出"当前环境的兼容性报告"，回答三件事：

1. 这台机器 / 这个浏览器上，**哪些引擎真的可用**（决定任务能不能转、会走哪条路）。
2. 不可用时**缺的到底是哪一项**——现在要等任务跑完变 Alert 才知道（例如 Firefox
   130/131：`isConfigSupported` 为真但缺 rVFC，MP4 会误路由）。
3. 报 bug / 换机时可复述的环境信息。

---

## 2. 数据源盘点（关键：哪些现成、哪些要新写）

### 桌面

| 检查项 | 来源（现状） | 新代码成本 |
|---|---|---|
| sidecar ffmpeg 是否找到 | `sidecar_probe.rs:20-31` 已缓存的 `SidecarProbe{exe, has_vp9}` | 0（读缓存） |
| sidecar 是否有 libvpx-vp9 | 同上 `has_vp9` | 0 |
| sidecar 版本串 | 新跑一次 `ffmpeg -version` | ~50-100ms 子进程，必须 `spawn_blocking` |
| inprocess libav 版本 | `ffmpeg::util::version()`（`avutil_version()` u32，位解 major/minor/micro）+ `util::configuration()` | 0 |
| inprocess 是否有 libvpx-vp9 | `codec::encoder::find_by_name("libvpx-vp9")`（`inprocess.rs:125` 同一调用） | 0 |
| 当前 engine 设置 + 实际解析结果 | `Settings.engine` + `runner.rs:48 resolve_engine` | 0 |
| 输出目录状态 / 可写 | `config.rs:105-123`（`output_dir_state` / `output_dir_writable`） | 0（写一次临时文件，只在开弹窗时） |
| app 版本 / 目标平台 | `env!("CARGO_PKG_VERSION")` + `cfg!` | 0 |
| WebView2 版本 | **不可得**（wry 无 API）。可选退路：弹窗内 `document::eval` 读 `navigator.userAgent` | — |

### 网页端

| 检查项 | 来源（现状） | 新代码成本 |
|---|---|---|
| WebCodecs `VideoEncoder` | `webcodecs-engine.js:88-100`（`stickerWebcodecsProbeSupport`，现**只返回一个 bool**） | 拆成明细对象（必须——报告要知道是哪一项挂） |
| `requestVideoFrameCallback`（rVFC） | 同上 | 已查，纳入明细 |
| `WebMMuxer` 全局（webm-muxer.js） | 同上 | 已查，纳入明细 |
| VP9 8-bit `isConfigSupported` | 同上（`vp09.00.10.08`） | 已查 |
| VP9 10-bit（`vp09.00.10.10`） | 未查。ffmpeg-wasm 兜底路径用 `yuv420p10le`，不是路由依据 | 加一行 info（廉价） |
| `ImageDecoder` | **未查，必须加**：GIF/APNG 时长探测（`native_probe`）与图片转码路径的硬依赖，Firefox <133 / Safari 缺失 → GIF/APNG 任务直接 `InvalidDuration` Alert（`web.rs:247` 注释已写明） | 1 行判断 |
| `OffscreenCanvas.convertToBlob` | 未查，图片路径硬依赖（`transcodeImage`） | 1 行判断 |
| 引擎资产可达性 | 未查。`ffmpeg-core-st.wasm`(29.5MB) / `ffmpeg-core-st.js` / `ffmpeg.js` / `814.ffmpeg.js` / `webm-muxer.js` / `webcodecs-engine.js`（部署清单见 `deploy-web.yml:72-75`） | HEAD 请求 ×6（几个 KB）——**直接命中 AGENTS 那条"dx 不拷 assets、必须手动 cp"的坑** |
| 每种媒体类型的实际路由 | 现成：`Engine::for_web`（`mod.rs:99-113`） | 0，白送一张矩阵表 |
| secure context / UA / 内存 | `navigator.isSecureContext` / `userAgent` / `deviceMemory`、`performance.memory`（Chromium） | 免费，info 行 |
| ffmpeg core 是否已加载 | **不做**：要真加载才知道 = 32MB 下载。只能用"资产可达"近似，文案上写清 | — |
| `crossOriginIsolated` / SharedArrayBuffer | **不需要**：ST core 无 pthreads（`WEB_DEMO_FINDINGS.md:11`） | — |

---

## 3. 明确不做的部分

- WebView2 版本号（无 API），GPU/硬件解码清单，真实编码压力测试（不值这个复杂度）。
- 任何自动上报/遥测。
- 弹窗内加载 ffmpeg core（会把"打开一个弹窗"变成 32MB 下载）。

---

## 4. 交互方案

- **入口**：工具栏 `⋯` 溢出菜单新增一项 `Compatibility Report`。工具栏在 640px 最小窗口
  已只剩 ~10px 余量（`toolbar.rs` 头注释实测），不再塞图标按钮；菜单里已有同类的低频动作
  （Clear Done / Download All / 主题）。
- **弹窗**：复用 `.modal-backdrop` / `.modal` / `.modal-head` / `.modal-body` 与
  `preview.rs:144-159` 的完整套路（backdrop `tabindex=0` + `onmounted set_focus` +
  `Escape` 关闭 + 点背景关闭 + 内容 `stop_propagation`）。行复用 `.settings-row` /
  `.label` / `.hint` + 现有 `.badge` 色（`.done`/`.alert`/`.size-excess`）——**新增 CSS = 0 行**，
  真出现配色不搭再补 6 行。
- **异步探测**：信号 `Option<CompatReport>`，打开时 None → `detecting…`，探测完成填 Some
  （照 `settings_panel.rs:25-38` 的 `webcodecs_ok` 模式）；桌面探测走 `run_blocking`。
- **行状态**：`ok` / `warn`（可用但降级，如 sidecar 缺 vp9 会 fallback）/ `fail`（当前路径
  不可用）/ `info`。文案英文（现有 UI 全英文）。

报告形态（示意）：

```
Compatibility Report
Engine setting: sidecar  →  resolved: sidecar          [info]

Platform
  StickerProcess 0.1.0 · windows-x86_64                [info]
  WebView2 UA: Chrome/…                                [info]

Engines
  sidecar ffmpeg        ✓  C:\…\ffmpeg.exe             [ok]
  sidecar libvpx-vp9    ✓                              [ok]
  inprocess libav       ✓  avutil 59.8.100 (ffmpeg 8.x)[ok]
  inprocess libvpx-vp9  ✓                              [ok]

Output
  Output folder         ✓  D:\…\output (writable)      [ok]

Routing (policy)
  mp4          → sidecar    gif → sidecar    apng → sidecar
  jpg/png/webp → sidecar    webp(anim) → sidecar
```

---

## 5. 实现清单

| 文件 | 改动 | 规模 |
|---|---|---|
| `src/components/compat.rs` | **新增**：`CompatModal` 组件 + 行渲染 + 每平台 `collect()` | ~120 行 |
| `src/components/mod.rs` | `pub mod compat;` | +1 |
| `src/app.rs` | `UiState.show_compat: Signal<bool>`；渲染 `<CompatModal/>`；**`native_bridge_js` 弹窗门禁（`app.rs:821`）加 `show_compat`** | ~6 行 |
| `src/components/toolbar.rs` | `⋯` 菜单加一项 | ~8 行 |
| `src/compat.rs` | 新增：`CompatReport`/`CompatRow`/`CompatState` 类型 + 桌面采集（cfg 非 wasm） | ~90 行 |
| `src/transcoder/web.rs` | `extern sticker_compat_report`；`webcodecs_supported()` 改为读明细对象（保持 bool 语义，**唯一实现仍在 JS**）；新增 `compat_report()` | ~40 行 |
| `assets/webcodecs-engine.js` | `stickerWebcodecsProbeSupport` 改返回明细 `{videoEncoder, rvfc, muxer, imageDecoder, offscreenCanvas, vp9_8bit, vp9_10bit}`；Rust 由明细算 `supported`；新增 `stickerCompatReport`（含资产 HEAD 检查） | ~40 行 |
| `src/app.css` | 0 行（复用现有类） | 0 |

合计 ≈ 300 行（含 JS），1 个新组件文件 + 1 个新采集模块。**转码路径、引擎矩阵、`Engine::for_web`
一概不动。**

---

## 6. 必须处理的坑

1. **快捷键门禁**：`app.rs:821` 只挡了 `show_settings`/`show_preview`；新弹窗不加进去，
   Ctrl+Enter 会在弹窗后面照样开跑。
2. **clippy 双 target `-D warnings`**（CI 门禁）：wasm-only / desktop-only 的字段与函数按
   `mod.rs:83` 的先例用 `#[cfg]` 或 `#[cfg_attr(<另一平台>, allow(dead_code))]`，别引入
   手写"这些警告是假阳性"清单。
3. **wasm 禁 `std::time`/`Instant`/`chrono`** → 用 `timers::sleep`；探测全程 async，
   不在渲染路径里 await。
4. **桌面阻塞探测**：`ffmpeg -version` 与"写临时文件测可写"都必须 `spawn_blocking`；
   可写性只在弹窗打开时跑一次（`config.rs` 已注明别放进逐键渲染路径）。
5. **资产 HEAD 的 SPA 兜底陷阱**：缺失路径会返回 HTML 200（AGENTS 记录过）→ 校验
   `content-type`/`content-length`，不能只看 `res.ok`。
6. **别在弹窗里加载 ffmpeg core**（32MB）；"ffmpeg.wasm 就绪"一栏只能叫"资产可达"。
7. **探测结果要缓存进信号**（None → Some），不要每次渲染重跑。

---

## 7. 验证计划

- **web**：手动 cp `assets/` 八件套 → `dx serve --platform web` → 打开弹窗截图；与设置面板
  里的 VP9 状态核对一致。
- **web 负例**：临时把 `assets/ffmpeg-core-st.wasm` 改名再打开报告 → 资产行必须显示 ✗
  （同时验证 SPA 兜底 200 陷阱被正确识别）。
- **桌面**：`cargo run` → 报告显示 sidecar 路径 + libvpx-vp9 + libav 版本 + 输出目录可写。
- `cargo clippy` 与 `cargo clippy --target wasm32-unknown-unknown --no-default-features
  --features web`（均 `-- -D warnings`）。
- 可选单测：把"明细 → 行状态/文案"的映射做成纯函数，测 vp9 缺失 → warn + 回退说明
  （JS 侧探测本身不进单测，成本高于价值）。

---

## 8. 已定决策

1. **入口**：`⋯` 菜单的 `Compatibility Report`（已实施）。
2. **Copy report**：不做（v1 只展示）。
3. **路由矩阵**：包含（直接调 `Engine::for_web`）。

## 9. 验证记录（实测）

- 桌面 `cargo test compat -- --nocapture`：真实报告（sidecar exe + libvpx-vp9 + `ffmpeg
  version 9.0.1` + `avutil 61.1.101` + output 可写）。
- 桌面 WebView2（CDP 9222 截图）：报告弹窗逐行正确。
- Chromium 网页端（`dx build --platform web` + 手动 cp assets + `python -m http.server`）：
  全 OK；路由行 `mp4 → webcodecs · yuv420p10` / `gif,apng → ffmpeg-wasm · yuva420p`。
- 负例（资产缺失 + SPA 兜底 200 HTML）：隐藏 `ffmpeg-core-st.wasm` 后该行报
  `FAIL missing: ffmpeg-core-st.wasm…`——用一个"缺失路径返回 index.html 200"的临时
  服务器复现了 AGENTS 记录过的兜底陷阱，证明只看 `res.ok` 会误报。
- 快捷键门禁：弹窗打开时 Ctrl+Enter 不开跑（用 Cancel 按钮出现与否判定），关闭后照常。

