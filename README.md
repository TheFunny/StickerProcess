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
# 前置（三选一，配置一次后无需再设）：
#   a) 项目 .cargo/config.toml [env] 里写 FFMPEG_DIR（模板已建好，填路径即可）
#   b) setx FFMPEG_DIR <ffmpeg 安装根目录>        （shared 或 vcpkg 静态）
#   c) setx VCPKG_ROOT <vcpkg 根目录>             （自动推导 installed/<triplet>）
cargo run
```

设置面板中可选择转码引擎，启动时自动探测 ffmpeg 可执行文件（主程序
同目录或 PATH）：**In-process**（libav，默认——零依赖、进度精确、取消干净）
与 **Sidecar**（ffmpeg.exe 子进程，探测到才可选）。选择持久化到 settings.toml。

### 构建安装包（NSIS，内嵌 WebView2 离线安装器；不含 ffmpeg）

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

## 网页端

同一套代码编译为 wasm 在浏览器运行（`--no-default-features --features web`）：

- **GIF/APNG** → ffmpeg.wasm（ST core，透明双轨 VP9）
- **MP4 / 图片** → WebCodecs（浏览器原生解码 + VP9 编码 + webm-muxer；图片走
  Canvas→PNG）。引擎按媒体类型自动选择（矩阵即策略），无 ffmpeg 下载。
- 拖拽/选择文件、进度、超限重试、预览、下载、设置持久化（localStorage）全通。

### 本地运行 / 部署

```bash
# 开发预览
dx serve --platform web

# 发布构建（产物是纯静态文件，任意静态托管即可；无 SharedArrayBuffer，
# 因此不需要 COOP/COEP 跨域隔离头）
dx build --platform web --release

# ⚠ dx（serve 与 build 皆然）不会把项目 assets/ 拷进产物目录，跑起来/部署前手动补齐：
cp assets/{ffmpeg.js,814.ffmpeg.js,ffmpeg-core-st.js,ffmpeg-core-st.wasm,\
ffmpeg-engine.js,webcodecs-engine.js,webm-muxer.js,favicon.png} \
   target/dx/StickerProcess/debug/web/public/    # release 换 release/
```

改过 assets/ 下任何 JS 后同样要重新 cp（构建目录里的副本不会自动同步）。

`ffmpeg-core-st.wasm`（32 MB）不入库（`.gitignore`），构建网页端前需本地存在
（获取方式见 docs/W4_CORE_BUILD.md）。glue 加载路径是**相对**页面的（随页面
base 解析，根域/子路径托管皆可），静态服务器仍不要对未知路径做 SPA fallback
返回 HTML（会伪装成 200 导致加载失败）。

**缓存**：托管给 `ffmpeg-core-st.*` 配 `Cache-Control: public, max-age=31536000,
immutable`（首载 32 MB，回访秒开）。托管平台改不了头（如 GitHub Pages）时，重建
core 后改文件名（如 `ffmpeg-core-st-v2.wasm`）并同步 `ffmpeg-engine.js` 里的
`loadCore` URL + 本 README cp 清单，防止用户吃到旧缓存 core 配新 glue。

### GitHub Pages（自动部署）

线上地址 https://thefunny.github.io/StickerProcess/ 。push 到 `master`（改动命中
`src/` `assets/` `Cargo.*` `Dioxus.toml`）即由 `.github/workflows/deploy-web.yml`
构建，官方三件套 `configure-pages` → `upload-pages-artifact` → `deploy-pages`
发布（Pages 源 = GitHub Actions）。

workflow 关键点（踩过的坑都在此固化）：

- **core 来源**：32 MB `ffmpeg-core-st.wasm` 不入库（`.gitignore`），CI 从
  `wasm-core` 孤儿分支 `git show` 取回。该分支由 `build-wasmcore-branch.sh` 在本地
  重建（改了自建 core 后跑一遍再 `git push -f origin wasm-core`）——这是 core 在
  远端的唯一出处，配方本体见 `docs/W4_CORE_BUILD.md`。
- **子路径**：`dx build --platform web --release --base-path /StickerProcess/`
  必须带 `--base-path`（Dioxus.toml 无 base_url 键，0.7.10 实测只认命令行）。
  漏掉的表现：本地正常、Pages 白屏（`/assets/*.js` 404）。Rust 注入的 glue 与
  core 用**相对**路径，根域/子路径托管都能解析。
- **assets 手动补齐**：dx 不把项目 `assets/` 拷进产物目录，workflow 里显式 cp
  七个 JS + core wasm + favicon.png，并复制 `index.html` 为 `404.html`（Pages 无
  SPA fallback，避免刷新 404）。
- **免装 wasm-bindgen**：dx 自带桥接版本；CI 只装 stable 工具链 + `wasm32-unknown-unknown`
  target + dx 预编译二进制。build.rs 对 web（非 desktop feature）提前 return，
  CI 无 `FFMPEG_DIR` 也不 panic。

**首次迁移坑（已踩实）**：Pages 源从旧的 gh-pages 分支切到 Actions 的那一次运行，
deploy 报 `Branch "master" is not allowed to deploy to github-pages due to
environment protection rules`——`configure-pages` 同一次运行内才把源切成 workflow，
自动建的 `github-pages` environment 初始带分支限制，**重跑一次即通过**（#1 失败、
#3 成功实证）。gh-pages 分支与 `build-ghpages.sh` 脚本现已删除，发布只走 push。

## 构建

```bash
cargo build            # debug
cargo build --release  # release（size-optimized: lto=fat, panic=abort, strip）
cargo test             # 40 个单元测试
cargo test -- --ignored  # libav 集成冒烟测试（需静态 ffmpeg，见下）

# 网页端（wasm）
cargo check --target wasm32-unknown-unknown --no-default-features --features web
dx serve --platform web
dx build --platform web --release   # 部署前手动补 assets/，见"网页端"节

dx bundle …            # 桌面安装包（见上）
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
# 之后配置环境（三选一，见"从源码运行"）：
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
│   ├── web.rs           wasm32 专属：双引擎桥（ffmpeg.wasm / WebCodecs），
│   │                    两段式 prepare/finish 不跨 await 持锁、glue 注入、原生探测
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
