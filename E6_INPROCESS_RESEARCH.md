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

## 7. static 构建研究（2026-09-05）

目标：让 `ffmpeg-the-third` 以静态库链接（零 DLL 依赖的自包含 exe），
为 E6 第二阶段（删 sidecar）铺路。

### 7.1 build.rs 链接逻辑（`ffmpeg-sys-the-third` 6.0.0+ffmpeg-9.0）

三条路径，按优先级：
1. **`build` feature（源码编译）**：git clone FFmpeg n9.x → `./configure
   --enable-static --disable-shared --disable-autodetect --disable-programs`
   → `make install`。`EXTRALIBS` 从 `ffbuild/config.mak` 读取后透传给 rustc。
2. **`FFMPEG_DIR`（预编译库）**：`link_to_libraries()` 按 `static` feature
   发 `cargo::rustc-link-lib=static=avutil…`。**无需 `--features build`**——
   只要 `FFMPEG_DIR/lib` 下有 `avutil.lib` 等 MSVC 静态导入库即可。
3. vcpkg（未装）/ pkg-config（Windows 不可用）。

### 7.2 本机工具链盘点

| 工具 | 状态 |
|---|---|
| MSVC 2022 Community + cl.exe | ✓（14.42）|
| Windows SDK | ✓（10.0.22000，含 um/ucrt）|
| `make` | ✗（Git for Windows 不带；choco 未装 msys2/mingw）|
| nasm/yasm | ✗（源码编 x86 asm 需要；`--disable-x86asm` 可绕过但慢）|
| vcpkg | ✗ |

### 7.3 三条可行路线对比

**A. 预编译 static 库 + FFMPEG_DIR（推荐）**
gyan.dev 提供 `ffmpeg-*-full_build-shared` 也无 `.lib` 静态库？——其
`lib/*.lib` 是 DLL 导入库（链接到 DLL），**不是**静态库。需要另找
BtbN/FFmpeg-Builds 的 `win64-lgpl`/`win64-gpl` **static** 包（产出
`lib/*.a` + `lib/*.dll.a`，配 MSVC 也能链 `.a` 吗？——MSVC 链接器不吃
MinGW `libavutil.a`（COFF vs ELF 差异实际是 MinGW 也产 COFF，但
CRT/异常模型不兼容，链接大概率失败））。

**B. 源码编译（`--features ffmpeg-the-third/build`）**
需要 MSYS2（gcc/make/nasm）一套工具链 + `build-lib-vpx`（libvpx 也要
先编好，configure 会去系统找）。构建脚本在 Windows/MSVC 下**未经
官方测试**（`compile.rs` 无 windows 分支，`make` 调用假设 unix PATH）。
估计首次打通 0.5–1 天 + 每次全量重建 10–20 分钟。

**C. vcpkg 静态 ffmpeg（`vcpkg install ffmpeg[core] --triplet x64-windows-static`）**
build.rs 有原生 `try_vcpkg` 分支（含 bcrypt/ole32/ws2_32/secur32 系统库
自动链接）。最贴近 crate 作者预期路径；vcpkg 构建约 30–60 分钟（一次性，
binary caching 后秒级）。libvpx 是 ffmpeg 的依赖自动带上。

### 7.4 结论与建议

- **推荐 C（vcpkg x64-windows-static）**：唯一在 MSVC 主机工具链下
  官方支持的路径；`FFMPEG_DIR` 留空即可走 try_vcpkg。
- 路线 B（MSYS2 源码编译）为备选：灵活性最高（自定义 encoder 集），
  但 Windows 打通成本高。
- 无论哪条：删掉 sidecar 后 exe 体积预计 +15–25MB（静态 libav 剪裁后；
  gyan 全量 DLL 235MB 是含全部 98 个外部库的，`--disable-autodetect`
  的源码/vcpkg 默认集小得多——vcpkg ffmpeg[core] 无 libvpx 时需
  `ffmpeg[core,vpx]`）。
- license 注意：ffmpeg 静态链接（L）GPL 传染——libvpx 是 BSD 无碍，
  若启用 x264 需 `build-license-gpl` 并整体 GPL 化发布。

### 7.5 验证步骤（待执行）

1. `winget install vcpkg`（或 git clone 到 `D:/Tools/vcpkg`）+
   `vcpkg install ffmpeg[vpx] --triplet x64-windows-static`
2. `set VCPKG_ROOT=…` 后 `cargo build --release`（无需改 Cargo.toml；
   若 vcpkg 不在默认查找路径，用 `FFMPEG_DIR` 指向
   `vcpkg/installed/x64-windows-static` 亦可，build.rs 两路等价）
3. `cargo test -- --ignored` 验证 inprocess 冒烟；确认无 DLL 依赖
   （`dumpbin /dependents StickerProcess.exe` 不含 av*.dll）
4. NSIS 打包资源表中删除 `ffmpeg/ffmpeg.exe`
