# StickerProcess

把图片和短视频转换为 **Telegram 贴纸**格式。桌面版（Rust + Dioxus，Windows）+
网页版（同一套代码编译 wasm，浏览器内完成全部转换，**不上传任何文件**）。

- 视频（mp4 / gif / apng / 动画 webp，以及带透明的 webm）→ 动态贴纸 **webm**（VP9，≤ 256 KB）
- 图片（jpg / jpeg / png / 静态 webp）→ 静态贴纸 **png**（≤ 512 KB）
- 等比缩放到 512×512（lanczos）；超限自动缩小码率系数重试；原/成品并排预览

> 动画 webp 仅桌面版支持转 webm；网页版输出首帧 PNG。

## 获取

- **网页版**：<https://thefunny.github.io/StickerProcess/>（拖入文件 → Run → 下载）
- **桌面版**：<https://github.com/TheFunny/StickerProcess/releases/latest>
  - `*-setup-webview.exe`：安装版，内嵌 WebView2 离线安装器，Win10/11 均可安装
  - `*-setup-no-webview.exe`：安装版，体积小，要求系统已有 WebView2 运行时（Win11 自带）
  - `*-portable.exe`：免安装便携版，解压即运行

## 使用（桌面版）

1. 拖入文件，或点击 **Add File**（Ctrl+O 同样可用）；添加时自动探测时长与编码
2. 点击 **Run**（Ctrl+Enter 开始/取消）逐个转码，实时显示进度；超限自动调整系数重试
3. 点击任务行打开原/成品并排预览；点击输出大小可在资源管理器中定位文件
4. 已完成的任务不会被 Run 重跑，需要重转时点行内 **Re-run**
5. **Compatibility Report**（工具栏 `⋯` 菜单）：查看转码引擎、浏览器能力与各媒体类型的路由
6. **Settings**：
   - 输出目录、是否保留原文件名
   - 视频/图片大小上限、重试次数与系数、时长→系数表、强制帧率
   - 转码引擎：`Sidecar (ffmpeg.exe)`（默认）或 `In-process (libav)`；Sidecar 探测不到时不可选，已保存的 Sidecar 偏好无法满足时自动回退 In-process 并提示
   - Patch webm duration：默认开启，向 Telegram 报告 100 ms 时长以通过时长检查；关闭则保留编码器真实时长
   - 设置即时保存到 `%APPDATA%/StickerProcess/settings.toml`
7. 主题（浅色 / 深色 / 跟随）在工具栏 `⋯` 菜单中切换，不在 Settings 里

## 从源码构建（桌面）

前置：Rust stable（edition 2024）。ffmpeg 开发库二选一：

- **shared**：ffmpeg 8.x/9.x 共享构建，`FFMPEG_DIR` 指向其根目录（含 `include/` 与 `lib/`），
  `bin/` 加入 `PATH`。更新 ffmpeg 时两者都要更新。
- **vcpkg 静态**（产出零 DLL 的自包含 exe，发布版使用此方式）：

```bash
bash scripts/fetch-ffmpeg-static.sh          # 推荐：取 CI 构建好的静态包到 ffmpeg-dist/
set FFMPEG_DIR=%CD%\ffmpeg-dist              # …或本地自建（见下）
vcpkg install "ffmpeg[avcodec,avdevice,avfilter,avformat,swresample,swscale,vpx,zlib]" --triplet x64-windows-static
vcpkg install "libvpx[highbitdepth]" --triplet x64-windows-static --recurse
set FFMPEG_DIR=<vcpkg>/installed/x64-windows-static
```

静态包由 [TheFunny/ffmpeg-static-win](https://github.com/TheFunny/ffmpeg-static-win)
的 CI 构建并发布（tag 与 sha256 钉在取件脚本里）；本地 vcpkg 配方作为离线应急保留。

`FFMPEG_DIR` 也可写入项目 `.cargo/config.toml` 的 `[env]`（一次配置），或由 `VCPKG_ROOT`
自动推导；两者皆缺时构建报错并给出指引。

```bash
cargo run                  # 开发运行
cargo test                 # 单元测试
cargo test -- --ignored    # 集成冒烟测试，需要静态 ffmpeg 库
cargo build --release      # 发布构建
dx bundle --release --platform windows --package-types nsis   # NSIS 安装包
```

安装包不内嵌 `ffmpeg.exe`：Sidecar 引擎需要用户自备（PATH 或主程序同目录），
否则自动使用 In-process（内置 libav）。

贡献者请先读 [AGENTS.md](AGENTS.md)（架构、约定、Gotchas）。

## 网页端（开发者）

同一套代码以 `--features web` 编译为 wasm。引擎由媒体类型自动选择，用户不可选：

| 输入 | 引擎 |
|---|---|
| GIF / APNG / webm（带透明） | ffmpeg.wasm（ST core，yuva420p 透明 VP9） |
| MP4 | WebCodecs；VP9 编码能力缺失的浏览器回落 ffmpeg.wasm（yuva420p10） |
| 图片 | WebCodecs |

```bash
dx serve --platform web                                   # 开发预览
dx build --platform web --release --base-path /<repo>/    # 子路径部署必须带 --base-path
```

⚠ 注意两点：

1. **dx 不会把项目 `assets/` 拷到产物目录**（serve 与 build 皆然），需手动补齐。
   先取回自建 core，再拷贝：

```bash
bash scripts/fetch-ffmpeg-core.sh   # 下载 ffmpeg-core-st.{js,wasm} 到 assets/ 并校验 sha256
cp assets/{ffmpeg.js,814.ffmpeg.js,ffmpeg-core-st.js,ffmpeg-core-st.wasm,\
ffmpeg-engine.js,webcodecs-engine.js,webm-muxer.js,favicon.png,og-image.png} \
   target/dx/StickerProcess/release/web/public/
```

2. **自建 core 不入库**（`.gitignore` 掉 `assets/ffmpeg-core-st.{js,wasm}`，约 29.5 MB）。
   `scripts/fetch-ffmpeg-core.sh` 从 [TheFunny/ffmpeg-core-st](https://github.com/TheFunny/ffmpeg-core-st)
   的 Release 取件，版本与 sha256 钉在该脚本里（唯一出处）；重建配方见
   [docs/W4_CORE_BUILD.md](docs/W4_CORE_BUILD.md)。

线上发布全自动：push `master` → GitHub Actions（`.github/workflows/deploy-web.yml`）
先跑测试，再构建并部署到 GitHub Pages。部署后可用 `scripts/smoke-web.sh` 在无头浏览器中挂载验证。
踩坑记录（wasm strip、缓存、OG 注入等）见 [AGENTS.md](AGENTS.md) 的 Gotchas。

## 文档

| 文档 | 内容 |
|---|---|
| [AGENTS.md](AGENTS.md) | 架构、约定、Gotchas（贡献者必读） |
| [docs/RELEASE.md](docs/RELEASE.md) | NSIS 安装包的升级语义与缺口 |
| [docs/WEB_PLAN.md](docs/WEB_PLAN.md) | 网页端路线图（W1–W5，已落地） |
| [docs/W4_CORE_BUILD.md](docs/W4_CORE_BUILD.md) | 自建 ffmpeg.wasm core 配方 |
| [docs/WEB_DEMO_FINDINGS.md](docs/WEB_DEMO_FINDINGS.md) · [_B](docs/WEB_DEMO_FINDINGS_B.md) · [_C](docs/WEB_DEMO_FINDINGS_C.md) | 三次引擎 spike 的实测记录 |
| [docs/E6_INPROCESS_RESEARCH.md](docs/E6_INPROCESS_RESEARCH.md) | 进程内转码与静态构建记录 |
| [docs/MIGRATION_PLAN.md](docs/MIGRATION_PLAN.md) | iced → Dioxus 迁移史与路线 |
| [docs/REFACTOR_PLAN.md](docs/REFACTOR_PLAN.md) | 重构清单与被否决的方案 |
| [docs/COMPAT_REPORT_PLAN.md](docs/COMPAT_REPORT_PLAN.md) | Compatibility Report 的设计与验证记录 |
| [docs/MOTION_REVIEW.md](docs/MOTION_REVIEW.md) | 弹窗与 Toast 动画评审 |
| [docs/BACKLOG.md](docs/BACKLOG.md) | 尚未落地的待办索引（各项状态与优先级） |
