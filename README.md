# StickerProcess

把图片和短视频转换为 **Telegram 贴纸**格式。桌面版（Rust + Dioxus，Windows）+
网页版（同一套代码编译 wasm，浏览器内完成全部转换，**不上传任何文件**）。

- 视频（mp4 / gif / apng / 动画 webp）→ 动态贴纸 **webm**（VP9，≤ 256 KB）
- 图片（jpg / jpeg / png / webp 静态）→ 静态贴纸 **png**（≤ 512 KB）
- 等比缩放到 512×512（lanczos）；超限自动缩系数重试；原/成品对比预览

## 获取

- **网页版**：<https://thefunny.github.io/StickerProcess/>（拖入文件 → Run → 下载）
- **桌面版**：<https://github.com/TheFunny/StickerProcess/releases/latest>：
  `*-setup-webview.exe`（安装版，内嵌 WebView2 离线安装器，任何 Win10/11 可装）、
  `*-setup-no-webview.exe`（安装版，体积小，要求系统已装 WebView2 运行时——Win11 自带）、
  `*-portable.exe`（免安装便携版，解压即跑，零 DLL 依赖）

## 使用（桌面版）

1. 拖入文件或点击 **Add File**（自动探测时长/编码）
2. 点击 **Run** 逐任务转码，实时进度；超过大小上限自动调整码率系数重试
3. 点击任务行打开 **Input vs Output** 并排预览；点击输出大小在资源管理器中定位文件
4. **Settings**：输出目录、大小上限、重试次数/系数、时长→系数表、帧率、
   转码引擎（Sidecar = ffmpeg.exe 子进程，默认；探测不到自动回退 In-process =
   内置 libav）、主题。设置即时保存到 `%APPDATA%/StickerProcess/settings.toml`

## 从源码构建（桌面）

前置：Rust stable（edition 2024）。ffmpeg 开发库二选一：

- **shared**：ffmpeg 8.x/9.x 构建，`FFMPEG_DIR` 指向其根目录（含 `include/`+`lib/`），
  `bin/` 加入 `PATH`（更新 ffmpeg 时两者都要更新）
- **vcpkg 静态**（产出自包含 exe，零 DLL）：

```bash
vcpkg install "ffmpeg[avdevice,avformat,avfilter,swscale,swresample,vpx,zlib]" --triplet x64-windows-static
vcpkg install "libvpx[highbitdepth]" --triplet x64-windows-static --recurse
set FFMPEG_DIR=<vcpkg>/installed/x64-windows-static
```

`FFMPEG_DIR` 也可写到项目 `.cargo/config.toml` 的 `[env]`（一次配置）或由
`VCPKG_ROOT` 自动推导；两者皆缺时构建报错并给出指引。

```bash
cargo run                  # 开发运行
cargo test                 # 单元测试
cargo build --release      # 发布构建
dx bundle --release --platform windows --package-types nsis   # NSIS 安装包
```

安装包不内嵌 ffmpeg.exe：Sidecar 引擎需要用户在 PATH 或主程序同目录自备，
否则自动使用 In-process（内置 libav）。

## 网页端（开发者）

同一套代码 `--features web` 编译为 wasm。引擎按媒体类型自动选择（矩阵即策略，
用户不可选）：GIF/APNG → ffmpeg.wasm（ST core，透明双轨 VP9）；MP4 → WebCodecs
（VP9 编码能力缺失的浏览器自动回落 ffmpeg.wasm）；图片 → WebCodecs。动画 webp
在网页端暂输出首帧 PNG（桌面端已支持转 webm）。

```bash
dx serve --platform web                                   # 开发预览
dx build --platform web --release --base-path /<repo>/    # 子路径部署必须带 --base-path
```

⚠ 两个坑：

1. **dx 不拷贝项目 `assets/` 到产物目录**（serve/build 皆然），需手动补齐（core 先取，
   见下条）：

```bash
bash scripts/fetch-ffmpeg-core.sh   # 下载 ffmpeg-core-st.{js,wasm} 到 assets/ 并校验 sha256
cp assets/{ffmpeg.js,814.ffmpeg.js,ffmpeg-core-st.js,ffmpeg-core-st.wasm,\
ffmpeg-engine.js,webcodecs-engine.js,webm-muxer.js,favicon.png,og-image.png} \
   target/dx/StickerProcess/release/web/public/
```

2. **自建 core 不入库**（`.gitignore` 掉 `assets/ffmpeg-core-st.{js,wasm}`，29.5 MB）。
   `scripts/fetch-ffmpeg-core.sh` 从
   [TheFunny/ffmpeg-core-st](https://github.com/TheFunny/ffmpeg-core-st) 的 Release 取件，
   版本与 sha256 钉在该脚本里（唯一出处）；重建配方见
   [docs/W4_CORE_BUILD.md](docs/W4_CORE_BUILD.md)。

线上发布全自动：push `master` → GitHub Actions（`.github/workflows/deploy-web.yml`）
构建并部署到 Pages。手动流程与所有踩坑记录（gh-pages 迁移、OG 注入、缓存策略）
见 [AGENTS.md](AGENTS.md) 与文档目录。

## 文档

|文档|内容|
|---|---|
|[AGENTS.md](AGENTS.md)|架构、约定、Gotchas（贡献者必读）|
|[docs/RELEASE.md](docs/RELEASE.md)|NSIS 安装包的升级语义与缺口|
|[docs/WEB_PLAN.md](docs/WEB_PLAN.md)|网页端路线图（W1–W5，已完成）|
|[docs/MIGRATION_PLAN.md](docs/MIGRATION_PLAN.md)|iced → Dioxus 迁移史|
|[docs/REFACTOR_PLAN.md](docs/REFACTOR_PLAN.md)|重构清单与被否决策|
|[docs/E6_INPROCESS_RESEARCH.md](docs/E6_INPROCESS_RESEARCH.md)|进程内转码与静态构建记录|
|[docs/W4_CORE_BUILD.md](docs/W4_CORE_BUILD.md)|自建 ffmpeg.wasm core 配方|
|docs/WEB_DEMO_FINDINGS{,_B,_C}.md|三次引擎 spike 实测记录|
