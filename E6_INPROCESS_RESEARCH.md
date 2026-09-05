# E6 立项草案：进程内转码（替代 ffmpeg-sidecar）

> 状态: 可行性研究完成 (2026-09-02)，待 Phase E 收尾后立项细化。
> 目标: 用 `ffmpeg-the-third` 的 libav API 在进程内完成解码→滤镜→编码→封装，
> 替代 `ffmpeg-sidecar` 子进程方案。

## 1. 动机

- **分发零依赖**: 探测（已在用 the-third）+ 转码都走 libav 后，`ffmpeg.exe`/DLL
  完全不再需要——与 E5 的 static 构建议题合并考虑
- **进度更精确**: 直接数帧/算 pts，替代 stderr 文本解析
  （`parse_progress_time`，含 "00:03:29.04" 文本解析的脆弱性）
- **取消更干净**: 帧循环内直接检查 `cancel_flag`，无需 kill 子进程
- **依赖收敛**: 删除 `ffmpeg-sidecar` 依赖，全库只剩一个 ffmpeg 绑定

## 2. API 可行性查证结论 (ffmpeg-the-third 6.0.0+ffmpeg-9.0)

全部逐个查证存在（本机 ffmpeg 8.0.1 均可用）:

| 步骤 | API | 备注 |
|---|---|---|
| 解复用 | `format::context::input`（probe 已在用） | `duration()` 在 common.rs#87 |
| 解码 | `decoder().video()` → `send_packet` / `receive_frame` / `send_eof` | decoder/opened.rs |
| 缩放滤镜 | `filter::graph::parse("scale=512:512:force_original_aspect_ratio=decrease")` + `source::add` / `sink::frame` | 字符串滤镜规格原样可用，lanczos 参数照搬 |
| VP9 编码 | `encoder().video()` → `send_frame` / `receive_packet` / `send_eof` | 本机 ffmpeg 带 libvpx-vp9 ✓ |
| 封装 | `format::context::output` → `add_stream` / `write_header` / `write_trailer` | webm muxer ✓ |
| 取消 | 帧循环内检查 `cancel_flag`（无需 kill 进程） | 比 sidecar 方案干净 |

## 3. 风险点（立项前需重点验证）

1. **libvpx-vp9 延迟帧**: encoder 带 `delay` capability——`send_eof` 后仍会
   吐包，flush 顺序错了会丢尾帧或卡死。需确认 `receive_packet` 的
   EAGAIN 循环写法
2. **像素格式**: 现行 CLI 参数 `yuva420p`（gif/apng）与 `yuv420p10`（mp4）。
   已核对 libvpx-vp9 支持列表含 `yuva420p` 与 `yuv420p10le` ✓；
   但解码→滤镜→编码三段之间的格式协商（swscale 自动插入 vs 滤镜链内
   format 转换）需要实验
3. **行为保真**: crf/bufsize/row-mt/10bit 等参数要逐个翻译成 API 选项
   （encoder options dict）；编码结果与 CLI 可能有细微差异——
   需用 `input/` 下样例（1.mp4/1.gif/2.gif + png/jpg）做 A/B 对比
4. **webm 时长补丁可能可删**: libav muxer 直接写流时长，
   `44 89 88` 二进制 hack 或许不再必要（需实验确认 Telegram 兼容性）
5. **图片管道重写**: 现在是 PNG over stdout + oxipng 内存优化。
   进程内方案：解码→缩放→编码 PNG（libav 的 png encoder）→ oxipng。
   或保留 oxipng 前直接从帧拿像素

## 4. 实施草案（两阶段）

### 第一阶段：双轨并存（保真优先）
- `Transcoder::run_inprocess(on_progress)` 与现有 sidecar 路径并存
- 设置或编译期开关切换
- 用 `input/` 样例做 A/B 对比（输出大小、时长、可播放性、Telegram 实测）
- 通过后切换默认路径，sidecar 代码降级为后备或删除

### 第二阶段：清理
- 删 `ffmpeg-sidecar` 依赖与 `parse_progress_time`
- 视实验结论删 webm duration patch（steps.rs 可能整个消失）

## 5. 工作量估计

2–4 天（含 A/B 对比验证）。建议排在 Phase E（打包/README）之后单独立项。

## 6. 与既有议题的联动

- **E5 static 构建**: 进程内方案使"static 化"收益更大（单个自包含 exe）；
  若走 `ffmpeg-the-third` 的 `build` feature 从源码编译 libav，需要 MSVC
  工具链，构建复杂度高——建议继续用 FFMPEG_DIR 链接 shared 开发库
- **E4 并行转码**: 进程内方案无子进程开销，多任务并行更轻量
- **日志/进度契约**: `ProgressUpdate` mpsc 结构不变，仅数据来源更精确

## 7. static 构建（2026-09-05 研究并落地，提交 2d78447）

目标：`ffmpeg-the-third` 以静态库链接，产出零 ffmpeg DLL 依赖的自包含
exe，为 E6 第二阶段（删 sidecar）铺路。

### 7.1 build.rs 链接逻辑（`ffmpeg-sys-the-third` 6.0.0+ffmpeg-9.0）

三条路径，按优先级：
1. **`build` feature（源码编译）**：git clone FFmpeg n9.x → `./configure
   --enable-static --disable-shared --disable-autodetect --disable-programs`
   → `make install`。
2. **`FFMPEG_DIR`（预编译库）**：`link_to_libraries()` 按 `static` feature
   发 `cargo::rustc-link-lib=static=avutil…`。
3. vcpkg / pkg-config（Windows 上 pkg-config 不可用）。

### 7.2 路线对比（结论：C）

| 路线 | 评价 |
|---|---|
| A. gyan/BtbN 预编译 static | gyan shared 包的 `.lib` 是 DLL 导入库非静态库；BtbN 产 MinGW `.a`，与 MSVC CRT 不兼容 ✗ |
| B. 源码编译 `--features build` | 需 MSYS2 全套（gcc/make/nasm）；`compile.rs` 无 Windows 分支，未官方测试 ✗ |
| **C. vcpkg x64-windows-static** | MSVC 主机唯一官方支持路径，系统库（bcrypt/ole32/ws2_32/secur32）自动链接 ✓ |

### 7.3 落地实录（踩坑记录）

1. **vcpkg 版本**：旧 checkout（2022-01，ffmpeg 4.4.1）不满足
   the-third 要求的 avcodec ≥ 59.37（FFmpeg 5.1+）。升级到 2026-07-27
   （ffmpeg 9.0.1 port，与 the-third 6.0 的 `ffmpeg_9_0` 特性精确匹配）。
   网络走代理 `http://127.0.0.1:10808`（tarball 替代 git fetch）。
2. **`--toolchain=msvc` 失效**：旧 `vcpkg-cmake` port 的 `cmake_get_vars`
   助手不导出 `VCPKG_DETECTED_MSVC` → portfile 不加 `--toolchain=msvc` →
   ffmpeg configure 在 MSYS 路径（`/c/...`）下做 cl.exe 探测失败
   （`-Fo` 输出路径被误解为 `D:\c\...`）。换新版 vcpkg-cmake-get-vars
   （导出该变量）后 configure 走 `TMPDIR=.` 相对路径，一切正常。
3. **`libvpx[highbitdepth]` 必须**：vpx 默认不带 VP9 10-bit 支持，
   `yuv420p10le` 编码直接报 "Specified pixel format ... not supported"。
   装 `libvpx[highbitdepth]` 后重建 ffmpeg 解决。
4. **png 需要 `ffmpeg[zlib]`**：`--disable-zlib` 时 png 编解码被禁。
   注意 debug 链接的 zlib 名是 `zsd.lib`（release `zs.lib`）。
5. **`avicap32.lib` 不在 Windows SDK**：avdevice 的 vfwcap 依赖它，
   但 SDK 只带 `msvfw32.lib`。用 `lib.exe /def` 从手写 `.def`
   （仅导出 `capCreateCaptureWindowA` / `capGetDriverDescriptionA`）
   生成导入库，存放于 `D:/Tools/ffmpeg-static-extras/`。
6. **系统库补链**：`ffmpeg-sys` 的 `EXTRALIBS` 透传只在 `--features
   build` 路径生效，FFMPEG_DIR 路径不传外部依赖 → 项目 `build.rs`
   在 `FFMPEG_DIR` 设置时补链 16 个库：vpx、strmiids、mfuuid、uuid、
   winmm、ws2_32、secur32、bcrypt、user32、avicap32、msvfw32、gdi32、
   oleaut32、shlwapi、psapi、ncrypt、crypt32、zs，并加
   `/NODEFAULTLIB:{LIBCMT,LIBCMTD,MSVCRTD}` 抑制 CRT 冲突。

### 7.4 CI 适配（待做）

当前 `build.rs` 的 avicap32 导入库路径硬编码 `D:/Tools/ffmpeg-static-extras/`。
CI 化的两个选择：
- **推荐**：把 `avicap32.def`（仅 5 行文本，见提交 2d78447 的 out/ 历史）
  放进仓库，CI 里用 `lib.exe /def:avicap32.def /machine:x64
  /out:<build-dir>/avicap32.lib` 生成——`lib.exe` 随 MSVC 必有，零外部依赖；
  `build.rs` 改为生成到 `OUT_DIR` 并 `rustc-link-search` 指向它。
- 备选：vcpkg manifest 模式（`vcpkg.json`）+ GitHub Actions 的
  `lukka/run-vcpkg` action，binary cache（GitHub Cache backend）后
  二次构建秒级。

### 7.5 验证结果（✅ 全部通过）

- `cargo build` 零警告；26 单测 + 3 个 `#[ignore]` libav 冒烟全绿
- PE 导入表：**零 ffmpeg DLL**（仅剩 `avicap32.dll` 等 Windows 系统库）
- debug exe 43.5 MB（含未裁剪的 debug 信息；release 预计更小）
- 复现命令：
  ```powershell
  $env:FFMPEG_DIR = "D:\Tools\vcpkg\installed\x64-windows-static"
  cargo build
  cargo test -- --ignored
  ```
- license：libvpx BSD 无碍；若加 x264 需 GPL 化

### 7.6 下一步（E6 第二阶段候选项）

- 删 `ffmpeg-sidecar` 依赖与 `parse_progress_time`
- 删 sidecar 分支（`gen_command`、`run_image` 的 stdout 管道、
  `TranscodeError::Spawn/ReadOutput`）
- NSIS 资源表删 `ffmpeg/ffmpeg.exe`，安装包瘦身
- CI：见 7.4
