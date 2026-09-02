# StickerProcess

桌面工具：把图片和短视频转换为 **Telegram 贴纸**格式。

- 视频（mp4 / gif / apng）→ 动态贴纸 **webm**（VP9，目标 ≤ 256 KB）
- 图片（jpg / jpeg / png / webp）→ 静态贴纸 **png**（≤ 512 KB）
- 所有媒体等比缩放到 **512×512**（lanczos）
- 每任务实时进度、超限自动缩系数重试、随时取消、原/成品对比预览
- 浅色 / 深色 / 跟随系统主题；设置持久化

基于 **Rust + Dioxus 0.7**（wry/WebView2）。

## 使用

### 从源码运行

```bash
# 前置：ffmpeg 可执行文件在 PATH 中（或设置 FFMPEG_DIR 指向 shared 构建）
cargo run
```

### 构建安装包（NSIS，内嵌 WebView2 离线安装器 + 随包 ffmpeg）

```bash
dx bundle --release --platform windows --package-types nsis
# 产物: target/dx/StickerProcess/bundle/windows/nsis/StickerProcess_0.1.0_x64-setup.exe
```

安装包包含 `ffmpeg.exe`（转码子进程，自动在主程序同目录查找）。
WebView2 运行时安装策略可在 `Dioxus.toml` 的
`[bundle.windows].webview_install_mode` 调整（OfflineInstaller / Skip 等）。

### 工作流

1. 拖入文件或点击 **Add File**（自动探测时长/编码，显示 Probing…）
2. 点击 **Run**——逐任务转码，实时进度条；超过大小上限自动按
   `factor = factor / excess × retry_shrink_factor` 重试（上限次数可配置）
3. 点击任务行查看 **Input vs Output** 并排预览与大小对比
4. 顶部进度条显示整体进度；**Cancel** 随时终止（任务回到 Pending 可重跑）

### 设置

右上角 **Settings**：输出目录（持久化）、最大重试次数、视频/图片大小上限、
重试缩放系数、时长→系数表（区间固定）、强制帧率（0=自动）、主题。
所有修改即时落盘到 `%APPDATA%/StickerProcess/settings.toml`。

## 构建

```bash
cargo build            # debug
cargo build --release  # release（size-optimized: lto=fat, panic=abort, strip）
cargo test             # 21 个单元测试
dx bundle …            # 安装包（见上）
```

开发前置：本机需有 **ffmpeg 8.x shared 构建**，`FFMPEG_DIR` 指向其根目录
（含 `include/` 与 `lib/`），`bin/` 加入 `PATH`——`ffmpeg-the-third`
编译期链接 libav（用于探测），`ffmpeg-sidecar` 运行期调用 CLI（用于转码）。
更新 ffmpeg 时**两者都要**更新，否则报 "ffmpeg not found"。

## 架构概览

```
src/
├── main.rs              入口：日志 + 窗口配置 + preview:// 协议注册
├── app.rs               根组件；UiState 全局信号；TaskEntry（逻辑层 + 显示镜像）
├── config.rs            Settings（serde+toml → %APPDATA%/StickerProcess/settings.toml）
├── runner.rs            异步转码循环：run_all → run_single_task（重试/取消/进度/日志）
├── media.rs             MediaFile：扩展名分类、probe()（ffmpeg 探测真实编码/时长）
├── preview.rs           preview:// 自定义协议（输入侧大文件流式读取，HTTP Range）
├── transcoder/          框架无关转码核心
│   ├── mod.rs           Transcoder 类型与编排（run_with_progress / check_size）
│   ├── command.rs       ffmpeg 命令生成 + 码率/系数纯函数（含单测）
│   ├── steps.rs         webm 时长补丁、oxipng 图片管道
│   └── error.rs         TranscodeError（thiserror）
└── components/          UI 组件：toolbar / task_list / settings_panel / preview /
                         number_field / drop_zone / progress_bar / toast
```

详细约定（镜像写入口、NumberInput 契约、新增媒体类型的四处同步点等）
见 [AGENTS.md](AGENTS.md)；重构历史与待办见 [REFACTOR_PLAN.md](REFACTOR_PLAN.md)；
进程内转码（去 sidecar）立项草案见 [E6_INPROCESS_RESEARCH.md](E6_INPROCESS_RESEARCH.md)。
