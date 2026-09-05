# StickerProcess

桌面工具：把图片和短视频转换为 **Telegram 贴纸**格式。

- 视频（mp4 / gif / apng）→ 动态贴纸 **webm**（VP9，目标 ≤ 256 KB）
- 图片（jpg / jpeg / png / webp）→ 静态贴纸 **png**（≤ 512 KB）
- 所有媒体等比缩放到 **512×512**（lanczos）
- 每任务实时进度、超限自动缩系数重试、随时取消、原/成品对比预览
- 双转码引擎：Sidecar（ffmpeg 子进程，默认）/ In-process（libav，零子进程）
- 可选静态构建：自包含 exe，零 ffmpeg DLL 依赖

基于 **Rust + Dioxus 0.7**（wry/WebView2）。

## 使用

### 从源码运行

```bash
# 前置：FFMPEG_DIR 指向 ffmpeg 8.x/9.x 安装根目录（含 include/ 与 lib/），
# bin/ 加入 PATH；或指向 vcpkg 静态安装（见下"静态构建"）
cargo run
```

设置面板中可选择转码引擎：**Sidecar**（默认，调用 ffmpeg.exe 子进程）或
**In-process**（libav 进程内转码，无子进程、进度更精确、取消更干净）。
选择持久化到 settings.toml。

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
重试缩放系数、时长→系数表（区间固定）、强制帧率（0=自动）、转码引擎
（Sidecar / In-process）、主题。所有修改即时落盘到
`%APPDATA%/StickerProcess/settings.toml`。

## 构建

```bash
cargo build            # debug
cargo build --release  # release（size-optimized: lto=fat, panic=abort, strip）
cargo test             # 26 个单元测试
cargo test -- --ignored  # libav 集成冒烟测试（需静态 ffmpeg，见下）
dx bundle …            # 安装包（见上）
```

### ffmpeg 环境（两种）

**开发（shared 构建）**：本机有 ffmpeg 8.x/9.x shared 构建，`FFMPEG_DIR`
指向其根目录（含 `include/` 与 `lib/`），`bin/` 加入 `PATH`。
`ffmpeg-the-third` 编译期链接 libav（探测 + inprocess 引擎），
`ffmpeg-sidecar` 运行期调用 CLI（sidecar 引擎）。更新 ffmpeg 时**两者都要**更新。

**静态构建（vcpkg，零 DLL 依赖）**：

```bash
vcpkg install "ffmpeg[avdevice,avformat,avfilter,swscale,swresample,vpx,zlib]" ^
              --triplet x64-windows-static
vcpkg install "libvpx[highbitdepth]" --triplet x64-windows-static --recurse
set FFMPEG_DIR=D:\Tools\vcpkg\installed\x64-windows-static
cargo build --release
```

产出自包含 exe（无 av*.dll 依赖，release 约 27 MB）。注意：
- 需要 `libvpx[highbitdepth]`（否则 mp4 的 yuv420p10le 编码报错）
- 工程的 `build.rs` 自动补齐 vpx/DirectShow 等系统库链接并生成
  `avicap32.lib`（详见 `docs/E6_INPROCESS_RESEARCH.md` §7）
- 静态模式下使用 In-process 引擎（无 ffmpeg.exe）

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
│   ├── mod.rs           Transcoder 类型与编排（engine 分发 run_with_progress / check_size）
│   ├── command.rs       ffmpeg 命令生成 + 码率/系数纯函数 + 两引擎共享的
│   │                    effective_duration / resolve_factor（含单测）
│   ├── inprocess.rs     libav 进程内管道：解码→滤镜→编码→封装（E6）
│   ├── steps.rs         webm 时长补丁、sidecar 图片 stdout 读取
│   └── error.rs         TranscodeError（thiserror）
└── components/          UI 组件：toolbar / task_list / settings_panel / preview /
                         number_field / drop_zone / progress_bar / toast
build.rs                静态 ffmpeg 链接适配（vcpkg）：补系统库、生成 avicap32.lib
```
详细约定（镜像写入口、NumberInput 契约、新增媒体类型的四处同步点等）
见 [AGENTS.md](AGENTS.md)；发布与升级说明见
[docs/RELEASE.md](docs/RELEASE.md)；重构历史与待办见
[REFACTOR_PLAN.md](docs/REFACTOR_PLAN.md)；进程内转码（E6 第一阶段，
双引擎并存）与静态构建记录见
[E6_INPROCESS_RESEARCH.md](docs/E6_INPROCESS_RESEARCH.md)。
