# Route C spike 记录：WebCodecs 编 in-block alpha webm（#4 双流 alpha）

日期 2026-09-15，无头 Chrome（同 W2 环境）。目标：B 引擎吃 GIF/APNG——
WebCodecs 编码带 alpha 的 VP9，产物与桌面 ffmpeg `yuva420p` 输出同格式
（WebM AlphaMode=1 in-block），解码端（Chrome/ffmpeg/Telegram）认。

## 结论：❌ 不可行（判据失败于"解码端还原透明"）

| 判据 | 结果 |
|---|---|
| webm-muxer@5 支持 alpha | ✅ 原生：`video.alpha:true` → 写 AlphaMode(21440)=1，零改动 |
| VideoEncoder 接受带 alpha 的输入 | ✅ `format:"I420A"`（Y/U/V/A 四平面）configure+encode+flush 全过，24 帧 121ms |
| 产物容器带 alpha 标志 | ✅ ffprobe 报 `alpha_mode : 1`，24 帧完整解码 |
| **解码端还原透明（硬判据）** | ❌ Chrome `<video>`+canvas：`transparentPx=0`（全不透明）；ffmpeg：`-pix_fmt rgba` 导 PNG 得 **rgb24**（同命令打桌面参考件得 rgba 带透明） |

即：Chrome 的 VP9 编码器**收下 I420A 但不产出 libvpx 式 in-block alpha 位流**
（大概率丢弃 A 平面按普通 I420 编）。解码器按块内约定找 alpha 子帧，找不到 →
整帧不透明。容器标志是写了，语义没跟上——产物是"骗过探测的坏文件"。

试过两版编码侧方案，全灭：
1. **上下堆叠 W×2H 单帧**（WhatsApp 网页版思路）：ffmpeg/Chrome 都当 240×480
   普通视频，无拆帧。
2. **I420A 四平面直喂**（Chrome 内部可能自动拆双帧）：编码器不拆（上述）。
3. BlockAdditional 双轨封装（AlphaMode=2）：WebCodecs chunk 是黑盒码流，
   无法在块内塞第二路 VP9 灰度帧 + BlockMore 映射——API 层不可达。

## 对 v2 菜单的含义

- **#4（B 引擎吃 GIF/APNG）判死**：提速 GIF 转码这条路在 WebCodecs 上封死，
  除非 Chrome 未来支持 `alpha:'keep'`/显式双帧 API（FINDINGS_B 当年实测
  alpha:'keep' 不支持，今天依然）。
- GIF 提速已另路解决：realtime+cpu-used 4 → exec 1.2s（10s→1.2s），
  A 引擎"慢"基本消除；首访 32MB core 属下载问题（HTTP 缓存一次），非 CPU 问题。
- 遗留可玩项只剩 MT core（-sUSE_PTHREADS，需托管加 COOP/COEP——Pages 不行，
  Cloudflare 行）；在 exec 已 1.2s 的现状下收益微小，**不建议做**。

spike 页与产物：`wasm-demo/alpha-spike/`（gitignored），参考件
`ref-gif.webm`（ffmpeg 产的正版 alpha_mode:1 可对照）。
