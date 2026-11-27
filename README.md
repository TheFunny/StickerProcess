# StickerProcess

把图片和短视频转换为 **Telegram 贴纸**格式。基于 Rust + Dioxus，提供 Windows 桌面版与纯浏览器网页版（WASM，**不上传任何文件**）。

- **视频**（mp4 / gif / apng / webm / 动画 webp）→ 动态贴纸 **webm**（VP9，默认 ≤ 256 KB）
- **图片**（jpg / jpeg / png / webp）→ 静态贴纸 **png**（默认 ≤ 512 KB）
- 等比缩放（Lanczos，默认 512×512；视频与图像分辨率可独立配置，支持 `0` 保持原尺寸）
- 超限自动按系数收缩重试、原/成品并排预览、浅色/深色主题

> 动画 webp 仅桌面版支持转 webm，网页版输出首帧 PNG。

## 获取

- **网页版**：<https://thefunny.github.io/StickerProcess/>
- **桌面版**：<https://github.com/TheFunny/StickerProcess/releases/latest>
  - `*-setup-webview.exe`：安装版（含 WebView2 离线安装器，Win10/11 通用）
  - `*-setup-no-webview.exe`：轻量安装版（需系统已装 WebView2，Win11 自带）
  - `*-portable.exe`：免安装便携版

## 使用

1. 拖入文件或点击 **Add File**（`Ctrl+O`），点击 **Run**（`Ctrl+Enter`）开始/取消转码。
2. 点击任务行可并排预览原图与成品；点击输出大小可在资源管理器定位文件（网页版直接下载）。
3. 常用参数位于 **Settings**（视频/图像分辨率、体积上限、重试与码率系数、强制帧率、webm 时长补丁，以及桌面端输出目录与引擎选择）；主题切换与 **Compatibility Report** 位于工具栏 `⋯` 菜单。

## 开发与构建

前置：Rust stable（edition 2024）。架构说明、静态库自建与踩坑记录见 [AGENTS.md](AGENTS.md) 与 `docs/`。

### 桌面端（Windows）

```bash
bash scripts/fetch-ffmpeg-static.sh          # 获取预构建静态 ffmpeg 到 ffmpeg-dist/
set FFMPEG_DIR=%CD%\ffmpeg-dist
cargo run                                    # 开发运行
cargo test                                   # 单元测试
cargo build --release                        # 发布构建（自包含 exe，无 DLL 依赖）
```

> 也可使用本地共享 ffmpeg（配置 `FFMPEG_DIR` 与 `PATH`）或 `VCPKG_ROOT`。桌面端默认优先 `Sidecar (ffmpeg.exe)`，未检测到时自动回退内置 `In-process (libav)`。

### 网页端（WASM）

网页端按媒体类型自动路由引擎：GIF / APNG / webm 走 `ffmpeg.wasm`（`yuva420p` 保留透明），MP4 与静态图片走 `WebCodecs`（不支持 VP9 的浏览器自动回退 `ffmpeg.wasm` `yuv420p10`）。

```bash
bash scripts/fetch-ffmpeg-core.sh            # 获取 ffmpeg-core-st.{js,wasm} 到 assets/
dx build --platform web --release            # 子路径部署加 --base-path /<repo>/
# dx 不会复制 assets/，需手动拷入产物目录：
cp assets/{ffmpeg.js,814.ffmpeg.js,ffmpeg-core-st.js,ffmpeg-core-st.wasm,ffmpeg-engine.js,webcodecs-engine.js,webm-muxer.js,favicon.png,og-image.png} \
   target/dx/StickerProcess/release/web/public/
```
