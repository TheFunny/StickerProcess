# Route B Spike: WebCodecs + webm-muxer 网页端 Demo（2026-09-08）

## 结论
**全路径跑通，无需任何下载，速度碾压 Route A。** 唯一硬伤是 alpha 通道在部分环境不可用。

## 产物
`wasm-demo/webcodecs-demo/`：
- `index.html` — 单页 demo：`ImageDecoder`（GIF/APNG）/ `<video>`+rVFC（MP4）解码 →
  `OffscreenCanvas` 缩放 512 fit → `VideoEncoder`（VP9 8-bit）→ `webm-muxer` 封装
- `webm-muxer.js` — webm-muxer@5.0.3（30 KB，唯一依赖）

## 实测结果（真实 Chrome 152 headless，localhost）
| 路径 | 结果 | 耗时 | 输出 |
|---|---|---|---|
| GIF (2.gif 2.4s) | ✅ | **1.02s** | 37.8 KB |
| MP4 (1.mp4 6.0s) | ✅ | 7.07s | 102.5 KB |
| 长视频 (512×512, 2.8s) | ✅ | ~4s | 240.6 KB |
| 输出元数据 | duration=2.733s 320×306，可播放 | | |

对比 Route A（ffmpeg.wasm）：同 GIF 5.7s 且要下载 32 MB core；**Route B 快 5.6×、零下载**。
MP4 路径直接可用（Chrome 原生 h264 解码）——恰是 Route A 预构建 core 崩溃的路径。

## 与桌面版的差异（关键限制）
1. **CRF 不暴露**：WebCodecs 只有 bitrate 控制；demo 直接用 `b:v = base×factor`（与
   `quantized_bitrate()` 一致），质量波动比 crf26+bitrate 双约束差一点。
2. **alpha:'keep'**：headless 软件 VP9 编码器不支持 `alpha:'keep'`（isConfigSupported=false），
   **GIF 透明通道被拍扁到黑底**。桌面 Chrome 的 libvpx 是否支持需真机验证；10-bit 同样不支持
   （`vp09.00.10.10`=false，headless 无高比特深度软编）。Telegram 贴纸 GIF 源通常带透明，
   这是 Route B 的主要产品风险。
3. **缩放质量**：Canvas `imageSmoothingQuality='high'`（近似 bilinear/lanczos 级别可调），
   不是 ffmpeg lanczos，但肉眼差别小。
4. **浏览器兼容**（2026-09 按 MDN BCD 修正）：WebCodecs VideoEncoder Chrome/Edge 94+、
   **Firefox 130+、Safari 16.4+**；rVFC Chrome 83+/Firefox 132+/Safari 15.4+；
   ImageDecoder Chrome 94+/Firefox 133+/Safari 仅 preview。即 Firefox 133+ 三件齐、
   理论上 MP4 可走 B 快路径（真机验证中）；VP9 编码 isConfigSupported 在老 Firefox
   130/131（无 rVFC）会虚报可用——caps 探针已加 rVFC 门控，该窗口回落 A 引擎。
   原"实际覆盖 = Chromium 系"结论过时。
5. MP4 解码走 `requestVideoFrameCallback` 抓帧，时长/帧率来自页面探测（demo 硬编码 30fps 槽），
   正式版用 WebCodecs `VideoDecoder` + demuxer（或保持 rVFC）更精确。

## 对正式移植的含义
- 引擎层加 `"webcodecs"` 引擎：桥接 JS；`command.rs` 码率纯函数复用（b:v 计算已一致）。
- `duration` 探测：GIF 用 `frames × Σdelay`（ImageDecoder 免费给出），MP4 用 HTMLMediaElement。
- webm duration patch（`steps.rs` 44 89 88）在 Route B 不需要——muxer 写正确 Duration。
- 透明度方案：a) 接受黑底（多数 TG 表情包可接受）；b) 检测真机 `alpha:'keep'` 支持后启用；
  c) 回退 Route A（yuva420p）处理带 alpha 的 GIF。三选一见产品取舍。
