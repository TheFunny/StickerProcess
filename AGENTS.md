# AGENTS.md

Guidance for AI agents working in this repository.

## Project Overview

**StickerProcess** is a desktop **and web** tool that converts images and short
videos into **Telegram-style stickers**, implemented in **Rust** with the
**Dioxus 0.7** GUI (wry/WebView2 desktop, wasm/web feature in-browser):

- Video (mp4 / gif / apng / animated webp) → animated sticker in **webm**
  (libvpx-vp9), target ≤ **256 KB**
- Image (jpg / jpeg / png / webp) → static sticker in **png**, target ≤ **512 KB**
- All media is scaled to fit **512×512** with aspect ratio preserved
  (`scale=512:512:force_original_aspect_ratio=decrease`, lanczos)
- Web build is live at <https://thefunny.github.io/StickerProcess/> (W series,
  see `docs/WEB_PLAN.md`; dual engine = ffmpeg.wasm + WebCodecs, matrix = policy).
  Animated webp is desktop-only (web outputs first-frame png until dual-stream
  alpha lands — spike verdict: WebCodecs can't, see `docs/WEB_DEMO_FINDINGS_C.md`).

E6 Phase 1 added an in-process transcoding engine (libav via `ffmpeg-the-third`),
selected by the `engine` setting (typed `Engine` enum, default `Sidecar` via
`#[default]` in `transcoder/mod.rs`;
`Inprocess` is always available and is the runtime fallback; `Sidecar` requires a detected ffmpeg.exe). Sidecar
availability is probed once at startup (`src/sidecar_probe.rs`: exe adjacent
to the app, else PATH, plus a `libvpx-vp9` encoder check); the settings
dropdown disables sidecar when unavailable, and the runner falls back to
inprocess with a warning if a saved `sidecar` preference can't be honored. Static ffmpeg linking (vcpkg x64-windows-static) is also
supported — see `docs/E6_INPROCESS_RESEARCH.md` §7.

## Repository Layout

| Path | Purpose |
|---|---|
| `src/main.rs` | Binary entry: desktop-only logger init (`pretty_env_logger`; wasm has no log backend) + Dioxus launch with window config |
|`src/app.rs`|Root component; `UiState` global signals (`settings` is the single source of truth for config); `TaskEntry { transcoder, mirror: TaskMirror }`|
| `src/config.rs` | `Settings` model (serde+toml), persisted to `%APPDATA%/StickerProcess/settings.toml`; `load`/`save`（原子写）+ `sanitize` 数值钳制 + roundtrip tests |
| `src/runner.rs` | Async transcode loop: `run_all` (skips `Done` tasks) → `run_single_task` (size-based retry, per-task cancel watcher, `TranscodeError` handling, retry/factor logging), progress channel, toasts |
| `src/components/` | UI widgets: `toolbar`, `task_list`, `number_field`, `drop_zone`, `progress_bar`, `toast`, `settings_panel`, `preview` |
| `src/app.css` | Stylesheet embedded via `include_str!`; theme variables (`[data-theme="dark"]`), row/modal/toast polish |
| `src/media.rs` | `MediaFile` model: type detection by the single `SUPPORTED` `(ext, MediaType)` table (drives入队过滤/`type_of_ext`/`dotted_exts`/空态文案) + `mime_for_ext`（预览三处共用）, `probe()` (ffmpeg codec/duration check incl. animated-webp → `Video(AnimatedWebP)` correction + packet-summed duration), duration, output path; enums; unit tests |
| `src/transcoder/` | Framework-agnostic core split into `mod.rs` (types + orchestration + engine dispatch + sidecar stderr event pump), `command.rs` (ffmpeg command gen + `SCALE_FILTER` / `DEFAULT_DURATION_FACTORS` + bitrate pure fns + shared `video_bitrate`/`effective_duration`/`resolve_factor`), `inprocess.rs` (libav pipe: decode→filter→encode→mux), `steps.rs` (webm duration patch — desktop `patch_webm_file`、web `patch_webm_bytes`，共享 `find_duration_payload`；sidecar image stdout reader), `web.rs` (wasm32-only: dual-engine bridge — ffmpeg.wasm + WebCodecs, two-phase `prepare_web_job`/`finish_web_job`, `native_probe`, script injection), `error.rs` (`TranscodeError`) |
| `build.rs` | Static-ffmpeg link glue: when `FFMPEG_DIR` points at a static install (vcpkg x64-windows-static), emits extra link libs (vpx, DirectShow/MediaFoundation system libs) and generates `avicap32.lib` from `build/avicap32.def` into `OUT_DIR` |
| `build/avicap32.def` | 2-symbol module definition used by `build.rs` to synthesize the `avicap32` import lib the Windows SDK doesn't ship |
| `build/nsis-hooks.nsh` | NSIS 安装前钩子（`[bundle.windows.nsis] installer_hooks`）：`.onInit` 里结束正在运行的实例 + 拒绝静默降级覆盖。**不 fork dx 的 NSIS 模板**；文件不经 handlebars 渲染（读安装器自身 VERSIONINFO 取新版本号）。见 `docs/RELEASE.md` §升级防护 |
| `src/preview.rs` | `preview://` custom protocol for the preview modal: URL builders, MIME via `media::mime_for_ext`, HTTP Range/206（单次响应封顶 8 MiB 窗口，防 GB 级 `bytes=0-` 一次分配）, percent encode/decode（`percent-encoding` crate，桌面 target 专属依赖）; unit tests |
| `src/timers.rs` | `sleep()` 双实现：桌面 = tokio；wasm32 = `setTimeout` Promise（`std::time`/`tokio::time` 在 wasm panic）|
|`scripts/fetch-ffmpeg-core.sh`|从 `TheFunny/ffmpeg-core-st` 的 Release 取回自建 ffmpeg.wasm core（`assets/ffmpeg-core-st.{js,wasm}`）：core 版本与 sha256 的唯一出处，CI 与本地开发共用 |
|`scripts/fetch-ffmpeg-static.sh`|从 `TheFunny/ffmpeg-static-win` 的 Release 取回静态 ffmpeg（解包到 `ffmpeg-dist/` 当 `FFMPEG_DIR`）：tag 与 sha256 的唯一出处，`release-desktop.yml` / `ci.yml` 与本地开发共用 |
| `docs/RELEASE.md` | NSIS installer upgrade semantics, current gaps, and future updater options |
| `docs/REFACTOR_PLAN.md` | Post-Phase-D refactor checklist (P1–P5) and rejected/deferred decisions with rationale |
| `docs/COMPAT_REPORT_PLAN.md` | Compatibility Report 按钮：开工前可行性报告（数据源盘点）+ 实测差异与验证记录 |
| `docs/MOTION_REVIEW.md` | 弹窗/Toast 动画评审：实测数据（丢帧、时长、重启、reduced-motion）+ 建议与实施记录 |
| `docs/BACKLOG.md` | **待办索引**：各计划文档里尚未落地的项 + 状态 + 优先级，以及"有意排除"清单（先查这里再翻各计划原文） |
| `docs/WEB_DEMO_FINDINGS_B.md` | Route B spike record: WebCodecs pipeline timings, alpha:'keep' unsupported, browser coverage (corrected 2026-09: Firefox 133+ full stack) |
| `docs/WEB_DEMO_FINDINGS_C.md` | Route C spike: WebCodecs in-block alpha webm **not feasible** (encoder accepts I420A but emits no alpha bitstream) — kills dual-track plan |
|`docs/W4_CORE_BUILD.md`|Self-built ffmpeg.wasm core recipe (built by CI in `TheFunny/ffmpeg-core-st`, published as release assets), rollback record |
| `docs/MIGRATION_PLAN.md` | Roadmap and phase checklist (A–E complete; E6 phase 1 in-process transcoding complete) |
| `docs/WEB_PLAN.md` | Web dual-engine roadmap (W1–W5): ffmpeg.wasm for GIF/APNG alpha, WebCodecs for MP4, engine matrix, and deployment |
| `.github/workflows/deploy-web.yml` | CI：push master → **先 windows 跑 `cargo test --locked`（质量门，lint 仍不挡部署但正确性挡）** → dx release build（`--base-path /StickerProcess/`；dx 下载带 sha256 校验，`cargo fetch --locked` 验锁）→ 跑 `scripts/fetch-ffmpeg-core.sh` 从 ffmpeg-core-st Release 取 29.5MB core（sha256 校验） → 组装产物（cp assets 八件套+core，index→404）→ python 注入静态 OG/description 到 head（dx 无自定义模板、爬虫不执行 JS）→ 官方三件套 configure/upload/deploy-pages 发布（Pages 源=Actions）|
| `.github/workflows/release-desktop.yml` | CI：push tag `v*`（或手动 dispatch）→ windows runner 跑 `scripts/fetch-ffmpeg-static.sh` 从 ffmpeg-static-win Release 取预构建静态库当 `FFMPEG_DIR` → cargo test + `dx bundle --release --nsis` **两次**（webview_install_mode 无 CLI 覆盖，sed 改 Dioxus.toml 切 OfflineInstaller/Skip）+ `cargo build --release` 的裸 exe 当便携版 → 产物收进 `dist/`（第二次 bundle 会清空 nsis 目录，原地留文件=丢）改名 `-setup-webview.exe`/`-setup-no-webview.exe`/`-portable.exe` → softprops/action-gh-release 发布。首次正式包 = v0.1.0 |
| `.github/workflows/ci.yml` | CI 门禁：push master / PR 跑 clippy 两个 target（`-D warnings`）+ `cargo test --locked`（windows，此前单测只在打 tag 时跑）。web job 只需 wasm32 target（`build.rs` 在非 desktop 特性下直接返回，不碰 ffmpeg）；desktop/test job 用 release-desktop.yml 那份预构建静态库（同一 `scripts/fetch-ffmpeg-static.sh`，缓存 `ffmpeg-dist`，key 跟着脚本哈希走）——clippy 也要过 `build.rs`。registry 缓存键跨 workflow 共享（`cargo-{os}-`），target 各 job 专属。刻意独立于 deploy-web.yml：一处 lint 不该挡住线上部署 |
| `docs/WEB_DEMO_FINDINGS.md` | Route A spike record: ffmpeg.wasm assembly gotchas (UMD/classic-worker pairing, MP4 OOB in prebuilt cores) |
| `src/sidecar_probe.rs` | Startup probe for sidecar ffmpeg: path resolution (app dir → PATH), libvpx-vp9 encoder check, process-wide cache |
| `Cargo.toml` | Dependencies + release profile (size-optimized, `lto = "fat"`, `panic = "abort"`, `strip = "symbols"`) |
| `docs/notes.md` | Developer notes, gitignored (see Gotchas) |
| `docs/` | Project documentation: migration/refactor plans, E6 research, dev notes |
| `archive/` | Legacy implementations (gitignored) |
| `ico/` | App icon：`icon.ico`（5-image PNG-in-ICO，tracked——CI 打包需要）；其余尺寸 png gitignored。`input/`/`out/`/`output/`/`target/` 全 gitignored |
| `assets/` | Web glue + **自建** ffmpeg.wasm core（**不**由 dx 复制——每次构建后手动 cp 到 `target/dx/.../web/public/` 根，见 Gotchas）：`ffmpeg-engine.js`（glue `stickerFfmpeg*`，pix_fmt 由 Rust 矩阵传入）、`webcodecs-engine.js`（glue `stickerWebcodecs*` + `stickerNativeProbe` + `stickerDownload` + `stickerCompatAssets`）、`webm-muxer.js`（webm-muxer@5 UMD）、ffmpeg.wasm ST core bundle（`ffmpeg.js` UMD wrapper + `814.ffmpeg.js` classic worker 入库；`ffmpeg-core-st.{js,wasm}` 由 `scripts/fetch-ffmpeg-core.sh` 取）、`favicon.png`（tab 图标）与 `og-image.png`（分享卡片图；两者 App rsx `document::Link`/`Meta` 相对路径引用）。core 为自建件（上游 `f876f90`，FFmpeg n5.1.4/emsdk 3.1.40 + libvpx `--enable-vp9-highbitdepth`，修复预构建的 10-bit 回退与 MP4 OOB 崩溃）；两份都 gitignore，远端唯一出处 = `TheFunny/ffmpeg-core-st` 的 Release（CI 构建 + headless Chromium 冒烟 + sha256，见 `docs/W4_CORE_BUILD.md`）|

## Architecture

- **State**: `UiState` bundles Copy-able signals (`tasks`, `settings`,
  `show_settings`, `show_preview`, `running`, `overall_progress`, `cancel`,
  `toasts`) provided to components via context.
- **Settings (Phase B)**: `config::Settings` is the single source of truth for all
  config (output dir, max retry, video/image size limits, retry shrink factor,
  duration factor table, forced FPS, theme, transcode `engine`, webm 时长补丁开关
  `webm_duration_patch` (默认 true；关掉则产物保留编码器原始 Duration，
  即不再谎报 100 ms 骗过 Telegram 的时长检测)). Mutate only via
  `UiState::update_settings(…)` — it applies the closure, then schedules the
  save. Desktop **debounces 300 ms with a single-writer epoch**（逐键同步写的
  toml 序列化 + rename 在 Windows+Defender 下可达数十 ms、直落渲染帧；代数保证
  只有最新任务写盘，快照→写盘间无 await，保序/无撕裂与原同步写等价），关窗尾写
  由 App 的 wry `CloseRequested` 兜底；wasm 同步写 localStorage（~1KB，无窗口
  钩子语义）。写失败弹去重 toast（无日志后端/无控制台的平台上 log 即静默）。
  Invalid output dirs (nonexistent, uncreatable parent) render the toolbar and
  settings inputs with a red border via `Settings::output_dir_valid()`
  (desktop only — on web both rows are not rendered at all).
  Missing/corrupt file falls back to defaults at load; `sanitize` also **clamps
  the numeric fields** (size limits ≥16 KB, shrink factor / `duration_factors`
  0.05..=10, fps ≤240, non-finite → default) because a hand-edited toml bypasses
  every UI clamp — a 0 KB limit turns each result into "over limit" (burns all
  retries) and non-finite floats make toml serialization fail permanently, which
  silently kills every later save. `NumberInput` rejects non-finite input for the
  same reason, and desktop `save` writes a temp file then renames it (atomic
  replace) so an interrupted write can't leave a half-parsed toml. Size limits,
  the duration-factor table and the forced FPS are synced onto each `Transcoder`
  by the runner before every attempt; the retry shrink factor is applied by the
  runner when shrinking on size excess.
  Each queued task is a `TaskEntry { transcoder: Arc<Mutex<Transcoder>>, mirror: TaskMirror }`
  shared with background `tokio::task::spawn_blocking` workers via `Arc<Mutex<..>>`
  (deliberately NOT cloned). `TaskMirror` holds **every** render-time field
  (path/status/output_size/factor/elapsed/error/is_video) so rendering never
  locks the mutex while a worker holds it during transcoding. 行内转码进度
  **刻意不在镜像里**——它是 `UiState.progress: Signal<Option<(usize, f32)>>`
  （顺序执行单槽即可；放镜像会让 10Hz 写入把整表 `tasks` 信号标脏，空转
  TaskList/Summary/Toolbar/Preview）。`TaskMirror` derives
  `PartialEq` on purpose: `TaskEntry`'s manual eq only compares the mirror, so a
  hand-written field list could silently drop a field and make dioxus memo skip
  re-renders (this broke factor display, then status/error badges).
  Mirror writes go through the two write entries —
  `UiState::with_task(index, …)` for Transcoder-derived fields (locks, applies, syncs
  status/factor/output_size/output_path/output_file_name) and
  `UiState::touch_entry(index, …)` (closure gets `&mut TaskMirror`) for UI-only
  mirrors (elapsed/error). Do not mutate mirrors via raw `tasks.with_mut`
  elsewhere. Row actions: `retry_task` (Alert/SizeExcess/Done → Pending, factor
  preserved; renders as "Retry"/"Re-run") and `remove_task` (any status).
- **Async probing**: adding a file creates the `Transcoder` without IO probing
  (`Status::Probing`); `MediaFile::probe` runs on a background thread and flips the
  mirror to `Pending`/`Alert`. Run is blocked while any task is probing.
  `Arc::ptr_eq` guards against stale index writes if the queue shifts. Files may
  also be dropped *while* a run is going, so `run_all` re-checks each entry and
  waits (50 ms poll) while it is still `Probing`, and `write_back_probe` must
  never touch a mirror that is already `Processing` — `with_task` locks
  synchronously on the main thread, so it would block behind the worker's
  transcode lock (frozen UI) and erase the running state; on wasm it would also
  race `prepare_web_job` into reading an empty duration (spurious `Alert`).
- **Execution flow**: Run → `runner::run_all` async loop, which **skips `Done`
  tasks** (re-running one goes through the row's Re-run button → `Pending`).
  Per task: assign a timestamped output path once (timestamp + 进程内单调序号防
  毫秒碰撞), set `Processing`, run
  `Transcoder::run_with_progress(engine, …)` inside `spawn_blocking` — `engine`
  is the typed `Engine` returned by `resolve_engine(Settings.engine)`
  (`Sidecar` → ffmpeg CLI subprocess via `ffmpeg-sidecar`, progress parsed from
  a self-driven stderr event pump; `Inprocess` → `Transcoder::run_inprocess`,
  libav pipe, progress computed from frame pts / duration). Both send progress
  over an mpsc channel (发送侧 50ms 节流——接收端 100ms 才 flush，逐帧 send 全是
  无效唤醒); the ~10Hz receiver loop writes the dedicated `UiState::progress`
  signal only while that index is still `Processing` — stale post-completion
  updates must not resurrect the progress bar on a Done row. Then `check_size()`. If over limit,
  shrink factor (`factor = factor / excess * retry_shrink_factor`) and retry up to
  `max_retry`, else advance.
  Task errors mark `Alert`, push an error toast, and skip to the next task.
  The `cancel` signal is checked between attempts; mid-task cancel bridges to
  `Transcoder.cancel_flag: Arc<AtomicBool>` — sidecar polls it from the event loop
  (so a *silent*, stuck ffmpeg is still killable) and from the image stdout reader,
  inprocess checks the flag each frame and returns `Cancelled`; either way the task
  resets to `Pending` and the run stops.
- **Sequential processing**: tasks run one at a time; overall progress = `(index+1)/len`.
- **File input**: toolbar uses a hidden `<input type="file" multiple>` triggered by a
  styled label; output-folder picking uses `rfd` (error dialogs were replaced by
  toasts in Phase C). Drag & drop works on the whole window: `ondragover` sets
  highlight, `ondrop` reads files and `FileData::path()` returns full paths on
  Windows (verified on dioxus 0.7.x).
- **Preview (Phase D)**: clicking a task row opens the preview modal
  (`show_preview`); input streams via the `preview://` custom protocol
  (`src/preview.rs`, HTTP Range support), output renders from a base64 data URL
  cached per (output_path, output_size) in the component (no re-encode on
  10Hz progress updates). Esc closes modals (backdrop autofocuses via
  `onmounted` + `set_focus`, so no input click needed first).
- **Row output link**: desktop — clicking the output size text runs
  `explorer /select,<path>` to reveal the file in Explorer. Web — the same click
  triggers a browser download of `output_bytes` via `stickerDownload`; the
  toolbar additionally has **Download All** (`UiState::download_all`, sequential
  with 400 ms spacing) and the Output Dir row is not rendered at all on wasm
  (components use the `let`-binding dual-branch pattern; rsx element-level
  `#[cfg]` is NOT parsed).
- **Status**: `Probing` (async probe in flight), `Pending`, `Processing`,
  `Done`, `Alert` (error), `SizeExcess` (retry-able); badge colors map to CSS
  classes in `app.css`, badge text comes from `Status::label()` (never `{:?}` —
  users must not see `SizeExcess`).
- **Theming**: CSS custom properties in `app.css`; `[data-theme="dark"]` on
  `<html>` overrides variables. Theme is part of `Settings` (persisted); the
  three-way toggle lives in the toolbar `⋯` menu (设置面板里的重复控件已删).
- **Compatibility Report** (toolbar `⋯` menu → `Compatibility Report`; UI in
  `components/compat.rs`, data in `compat.rs`): read-only environment checkup —
  engine availability, browser caps, engine-asset reachability, and the routing
  policy for each media type. It reuses `Engine::for_web` for the routing rows
  (never a second copy of the matrix), reads the cached `SidecarProbe`, and on
  wasm calls the same caps probe as `webcodecs_supported()` (`probe_details`;
  the glue returns a per-capability object, the bool is derived from it) plus
  `stickerCompatAssets` (HEAD requests only — **never** loads the 29.5 MB core).
  Asset rows are the sanctioned way to spot the "forgot to copy `assets/` to the
  web root" trap: a missing file gets a 200-HTML SPA fallback on Pages, so the
  check rejects `text/html` and empty `Content-Length` instead of trusting
  `res.ok`. Desktop collection is blocking (spawns `ffmpeg -version`, writes a
  probe file for output-dir writability) and therefore runs through
  `runner::run_blocking`. Re-check keeps the previous rows on screen (only the
  button flips to "Re-checking…" and is disabled while probing) — clearing the
  report first collapses the modal to one line and back, which reads as a flash.
  Opening it must be added to the `native_bridge_js` modal gate (`ctx.show_compat`)
  or Ctrl+Enter would start a run behind the modal.
- **Transcoding**:
  - Video bitrate (both engines): `-b:v` computed from target size and
    duration (`256 * 1024 * 8 bits / duration_seconds`), `-bufsize = b:v * 1.5`,
    `-row-mt 1`, `crf 26`, pix_fmt yuv420p10le (mp4) / yuva420p (gif, apng,
    animated webp), no audio.
  - Image → png: sidecar pipes PNG over stdout; inprocess png-encodes the
    filtered frame in memory; both then run the shared oxipng helper
    (`write_optimized_png`: preset 4, safe strip, alpha optimize).
- **Transcoding engines** (E6 phase 1, dual-track; selected by
  `Settings.engine`, dispatched in `mod.rs::run_with_progress`):
  - *sidecar* (default): `gen_command()` builds the ffmpeg CLI invocation,
    `ffmpeg-sidecar` runs it; image path pipes PNG over stdout.
  - *inprocess*: `inprocess.rs` — one pipeline time base (1/1000 ms):
    decode (`send_packet`/`receive_frame`, frame threading) → filter graph
    (`buffer` → `scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos`
    → `format=<pix_fmt>` → `buffersink`, linked via `Graph::output("in")` /
    `.input("out")` / `.parse`) → encode (libvpx-vp9 opened lazily on the
    first filter output frame so width/height come from the filter, not a
    formula; crf/row-mt/rc_buffer_size/rc_max_rate via options dict) →
    `write_interleaved` into a webm muxer. Encoder/stream tb = 1/1000;
    decoded-frame pts rescaled once from the stream tb; packets NOT
    rescaled (decoder tb == stream tb — double rescaling compressed pts).
    The graph's Source/Sink are re-`get()` per push/pull — the binding's
    borrow model doesn't allow holding them. Image path: decode first frame,
    filter to RGBA, png-encode in memory.
  - Shared helpers live in `command.rs`: **`video_bitrate(v_type)` is the single
    entry point of the bitrate chain** (`effective_duration` (APNG = 1.0) →
    `resolve_factor` (lazy default-factor init + GIF ×0.75 patch) →
    `quantized_bitrate`), returning `(duration, b:v)`; sidecar `gen_command`,
    inprocess and web all call it, so the three engines stay bitrate-identical
    — edit the chain here, never inline a copy in an engine.
    Same rule for `command::SCALE_FILTER` (the `scale=512:512:…:flags=lanczos`
    spec) and `media::VideoType::pix_fmt()` — both were hand-copied three times
    before.
    All engines run the webm duration patch after muxing unless the
    `webm_duration_patch` setting is off (`Transcoder::duration_patch`, synced
    per attempt by the runner like the other settings).
  - *cancel*: sidecar polls `cancel_flag` in its stderr event pump and its stdout
    image reader (100 ms), then kills + reaps the child; inprocess checks
    `cancel_flag` each frame and returns `Cancelled` (Drop chain releases
    libav handles; no orphan processes possible).
  - *failure*: **both** sidecar paths (video and image) check ffmpeg's exit
    status after the pipes close — the header (including Duration) is already on
    disk when an encode dies, so an unchecked partial file would pass the patch
    and be reported `Done`; `Alert` tasks get their half-written output deleted,
    like the cancel path.
- **Duration inference**: `ffmpeg-the-third` opens the input to read codec id
  and duration; APNG is assigned a fixed duration of 1 s (no probe); animated
  webp (codec id `WEBP_ANIM`, container duration N/A) sums packet
  pts+duration in `probe_desktop`. Output filenames are timestamps
  `%Y-%m-%d-%H%M%S%.3f` with extension `webm` / `png`.
- **Webm duration patch** (`transcoder::steps`): after encoding, the Duration
  element (EBML ID `0x4489` + 1-byte size vint `0x88`, i.e. marker bytes
  `44 89 88`) has its 8-byte payload overwritten with `100.0f64` (big-endian).
  The value is in **Segment Ticks**, and `TimestampScale` is 1e6 ns (1 tick =
  1 ms), so the clip claims a **100 ms** duration — the point is to slip past
  Telegram's sticker length check (over-long clips get rejected), not to change
  the actual playback. Toggle: setting `webm_duration_patch` (default true).
  The scan **stops at the first Cluster ID**
  (`1F 43 B6 75`; Duration lives in Info, always before any Cluster) — an
  unbounded search would hit a false `44 89 88` inside VP9 payload and silently
  corrupt 8 bytes of video. Shared locator `find_duration_payload` backs both
  paths: desktop `run_video` patches the file (`patch_webm_file`: read whole file
  → patch → write back; the file is capped at the sticker size, so the old
  "8 KB window + in-place seek + whole-file fallback" arrangement only bought
  three write paths for 8 bytes), web `finish_web_job` uses `patch_webm_bytes`
  on the in-memory bytes. Regression tests:
  `duration_patch_writes_at_marker_offset`, `duration_patch_rejects_marker_near_eof`,
  `duration_patch_refuses_marker_inside_cluster`,
  `duration_patch_in_place_preserves_rest`.

### Default size factors by duration (video)

`<1s → 1.2`, `<2s → 1.1`, `<3s → 1.0`, `<5s → 0.9`, `<8s → 0.8`, `≥8s → 0.7`
(`command::DEFAULT_DURATION_FACTORS` is the single copy — `Settings::default` and
`Transcoder::new` both read it);
the whole table is configurable in Settings (band edges stay fixed). GIF
additionally × 0.75 — a hard-coded bitrate-calculation patch, NOT a setting. The
factor is user-editable per task (0.1..=10.0) and only appears after the first
run lazily initializes it.

## Build & Run

```bash
cargo run            # debug
cargo build --release
cargo test           # 52 unit tests + 5 #[ignore] libav smoke tests
cargo test -- --ignored   # needs ffmpeg static libs (see "ffmpeg environment")

# web (wasm32): engine matrix runs in browser; core assets in assets/ (29.5MB wasm gitignored)
# chrono/pretty_env_logger 是桌面专属 target 依赖；wasm 无日志后端（log 宏直接短路）
cargo check --target wasm32-unknown-unknown --no-default-features --features web
dx serve --platform web
```

Test count: 52 unit tests across media/config/command/preview/app/steps/
inprocess/runner/number_field/compat, plus 5 integration smoke tests (`inprocess_video_smoke`,
`inprocess_gif_smoke`, `inprocess_image_smoke`,
`image_has_no_size_factor_after_transcode`,
`force_fps_frame_count_matches_sidecar`) that require the static
ffmpeg libs (env setup below). `inprocess_video_smoke` accepts a
`SMOKE_INPUT` env var to transcode an arbitrary input.

### wasm 体积（实测，别重复测）

`dx build --release --platform web` **不会**自动过 wasm-opt（日志里没有
"Optimizing WASM" 那行；`Dioxus.toml` 缺 `[web.wasm_opt]` 段即关），但给 app
wasm 开这个开关是白费：当前产物 638KB（rustc `opt-level="s"` + LTO fat + strip，
无 name 段），binaryen 132 `-Oz` 只再省 1.2KB（-0.2%），`-O3`/`-O4` 反而更大
（+0.6%/+0.8%）。**别加 `[web.wasm_opt]`**。真正的体积杠杆是 ffmpeg core：
33MB → 29.5MB（`-Oz`，-11%，gzip -3.6%，export/memory 声明不变，转码实测正常），
产物由 `TheFunny/ffmpeg-core-st` 的 CI 构建并发布为 Release（配方见
`docs/W4_CORE_BUILD.md`）。

### ffmpeg environment

The lib dir is resolved in `build.rs` in this order:
1. `FFMPEG_DIR` (explicit install root), else
2. `VCPKG_ROOT` → `{root}/installed/{triplet}` (triplet from
   `VCPKG_DEFAULT_TRIPLET`, default `x64-windows-static`; `VCPKGRS_TRIPLET`
   controls the `ffmpeg-sys` vcpkg probe), else
3. panic with setup instructions.

Neither set → build fails with clear Chinese instructions. Per-machine setup
lives in gitignored `.cargo/config.toml` `[env]` (template committed empty;
see `.gitignore`) or `setx` user env vars.

Two supported setups (see `docs/E6_INPROCESS_RESEARCH.md` §7 for the full record):

- **Dev (shared ffmpeg)**: `FFMPEG_DIR` → ffmpeg 8.x/9.x shared build root
  (with `include/` + `lib/`), `bin/` on `PATH`. `ffmpeg-the-third` links
  libav at build time (probe + inprocess engine); `ffmpeg-sidecar` calls the
  CLI at runtime (sidecar engine). Update BOTH when updating ffmpeg.
- **Static (vcpkg)**: `FFMPEG_DIR` → the package from
  [TheFunny/ffmpeg-static-win](https://github.com/TheFunny/ffmpeg-static-win)
  releases (unpacked: `lib/` + `include/` + `share/`), fetched by
  `scripts/fetch-ffmpeg-static.sh` — that repo's CI builds it from a pinned
  vcpkg tree with `ffmpeg[avcodec,avdevice,avfilter,avformat,swresample,swscale,vpx,zlib]`
  + `libvpx[highbitdepth]` and smoke tests it. Produces an exe with zero ffmpeg
  DLL imports. Local rebuild recipe (emergency): `docs/E6_INPROCESS_RESEARCH.md` §7.3.
  Requires `ffmpeg-sys` feature `static` (enabled via the `ffmpeg-the-third`
  `static` feature) — see `build.rs` for the extra link libs. `engine`
  setting must be `inprocess` in this mode (no `ffmpeg.exe` on PATH).

The first build fetches the dioxus dependency tree (~300 crates); rebuilds work
offline from the registry cache while `Cargo.lock` stays untouched.


## Conventions

- Commit messages: short lowercase English summary, e.g. `add APNG support and switch to
  ffmpeg-the-third`. Commits may combine feature + cleanup + minor tweaks.
- Keep the `Arc<Mutex<Transcoder>>` sharing pattern; do not clone task state;
  never hold a `MutexGuard` across an `.await`.
- Mirror writes: use `UiState::with_task` (Transcoder-derived fields) or
  `UiState::touch_entry` (closure receives `&mut TaskMirror`); direct
  `tasks.with_mut` on mirror fields elsewhere will desynchronize the UI.
- Engine-specific changes go in their own file (`command.rs` sidecar /
  `inprocess.rs` libav / `web.rs` wasm 桥); shared bitrate logic lives in
  `command.rs` helpers so all engines stay behaviorally identical. The *setting*
  is the typed `Engine` enum (serde kebab-case 序列化值 `"sidecar" | "inprocess" |
  "webcodecs" | "ffmpeg-wasm"` 与原字符串逐字一致；非法值反序列化即失败，
  `load` 整体回退默认——`engine_valid()`/sanitize 引擎分支已随之删除) — the core
  boundary is **typed**: `resolve_engine`
  returns `Engine`, `run_with_progress(Engine, …)` dispatches on it, `WebJob.engine`
  is an `Engine`, and the wasm dispatch matches exhaustively (no `other =>`
  catch-all, so an unknown engine string can no longer silently run sidecar).
  Web (wasm32) ignores the setting: matrix = policy, `Engine::for_web(&media_type,
  webcodecs_ok)` (`mod.rs`) → GIF/APNG = ffmpeg-wasm (yuva420p alpha); MP4 =
  webcodecs, **fallback ffmpeg-wasm (yuva420p10)** when VP9 caps unavailable (W4
  self-built core, real 10-bit verified); image = webcodecs. Animated webp is
  desktop-only (`Video(AnimatedWebP)` never reaches `for_web`; the last arm
  pins it to ffmpeg-wasm harmlessly — web side keeps it as Image(Webp) →
  first-frame png). `webcodecs_ok` is probed once per Run in `runner::run_all`
  (wasm) via `web::probe_details`（glue 返回逐项明细，bool 由明细算——兼容性报告
  消费同一份明细）；the tuple's pix_fmt (`VideoType::pix_fmt`) rides on `WebJob` through
  `stickerFfmpegTranscode`.
- Errors from the transcode core are `transcoder::TranscodeError` (thiserror);
  cancellation is matched via the enum, never by string comparison.
  `MediaFile::probe` / `Transcoder::probe` return `Result<(), String>` on both
  platforms (the consumers only ever stringify the error).
- Numeric inputs (`NumberInput`) commit on every valid keystroke (parse → clamp →
  `on_change`); focus shows a local draft so the cursor doesn't jump, and the
  non-editing display mirrors the external value directly (retry shrink is
  immediately visible). There is no separate enter/blur commit step.
  `TaskMirror` MUST NOT hand-write `PartialEq`: it derives it, and `TaskEntry::eq`
  compares only that struct — a hand-written field list dropped a mirror field
  before and made dioxus memo silently skip re-renders (broke factor display,
  then status/error badges).
- Lint state: run clippy for **both** targets —
  `cargo clippy` and
  `cargo clippy --target wasm32-unknown-unknown --no-default-features --features web`.
  Both are **warning-free**: platform-only API surface is either cfg-gated to the
  platform that uses it (`#[cfg(feature = "desktop")]` / `#[cfg(not(target_arch =
  "wasm32"))]`) or, where the other platform's unit tests need it (e.g.
  `Engine::for_web`, `MediaFile::from_bytes`), explicitly marked
  `#[cfg_attr(<other target>, allow(dead_code))]` with a reason. Don't reintroduce
  a hand-maintained "these warnings are false positives" list — if a new
  cross-platform item trips it, use one of those two forms.
  `cargo clippy --all-targets` is *not* a substitute: it adds a second target
  whose own dead-code view differs (measured: 6 → 7 warnings).
  Both commands are **gated in CI** (`.github/workflows/ci.yml`) with `-- -D warnings`.
- New media types must be wired in **three** places:
  1. `src/media.rs` — `SUPPORTED` 表加一行 `(ext, MediaType)`（入队过滤、
     `type_of_ext`、`dotted_exts`、空态文案全部自动派生）+ 对应 enum variant
     + 一行单测；`media::tests::supported_extensions_are_consistent` 断言表内
     每项可识别
  2. `src/media.rs` `mime_for_ext` — 预览三处（preview:// 协议 / 桌面 data URL /
     web Blob URL）共用的扩展名→MIME；漏加则该类型输入预览 404
  3. `src/media.rs` (`VideoType::pix_fmt`) + `src/transcoder/command.rs`
     — pix_fmt / codec handling, plus `MediaFile::probe` codec correction
  Same-extension dual type (animated webp) skips 1（extension exists）and adds
  a `VideoType` variant + probe detection instead; `for_web` gets a catch-all.

## Gotchas
- **wasm 平台三坑**（W1 实测，全部静默崩上下文/死锁，报错无栈）：
  1. `tokio::time`、`std::time::Instant`、`chrono::Local` 在 wasm32 一律 panic
     （"time not implemented"）——用 `timers::sleep`（cfg 双实现）与 `js_sys::Date`。
  2. dioxus 事件管线的 `prevent_default`/信号写入是异步的：拦不住浏览器同步默认动作
     （拖文件→导航打开），`dioxus::spawn` 在 wasm-bindgen Closure 里永不被轮询，
     `spawn_local` 上下文直接写 Signal 死锁。拖拽方案：原生 window 监听同步
     preventDefault（dragover+drop）+ thread_local 队列 + dioxus 排空任务。
  3. 经 wasm-bindgen 传出的 `&[u8]` 背靠 wasm 内存、**不可 detach**——ffmpeg.wasm
     `writeFile` 会 transfer 所有权，glue 必须先 `new Uint8Array(data).slice()`。
- **全局快捷键 / 弹窗焦点陷阱走 window 级原生监听 + eval 通道**（`app.rs::native_bridge_js`
  的 JS + `Eval::recv` 循环），别改回"根节点 tabindex + onkeydown"：点掉一个按钮或
  关掉弹窗后焦点落到 `body`，事件就不再经过 `.app`，快捷键会**静默失效**（实测：
  关掉设置面板后 Ctrl+Enter 彻底不响应）。同理，键盘默认动作只能靠原生监听里
  **同步** 的 `preventDefault` 拦（dioxus 管线是异步的，见上条）。桥的 JS 末尾必须
  永不 resolve（`await new Promise(() => {})`）：eval 通道在 JS 返回后即关闭，
  `dioxus.send` 会全部丢失；启动日志里的 `key bridge installed` 就是它的心跳。
- **弹窗的滚动容器是弹窗自身**（`.modal-settings`，设置面板 + 兼容性报告共用）：
  头/脚 sticky，正文 `flex: none; overflow: visible; padding: 0 18px`，弹窗
  `padding: 0; gap: 0; overflow-y: auto`。曾经让 `.modal-body` 独占滚动区——那时
  标题+动作行+内边距+间距共 ~127px 是**滚轮死区**，而默认窗口（960×680）下兼容性
  报告只溢出 ~39px，指针自然落在下沿/动作行上，于是"只剩半行却滚不动"（实测死区
  内 deltaY 全丢、正文内正常）。改用原生滚动容器后弹窗内任意位置都能滚，也不必
  用脚本转发滚轮（转发还要猜 deltaMode/缩放，手感与原生不一致）。sticky 背景必须
  不透明——`padding`/`gap` 一旦留在弹窗上，滚上来的内容会从半透明间隙里漏出来。
- **dragover 期间 `dataTransfer.files` 恒空**（规范保护模式，文件仅 drop 时可见）——
  判断“文件拖拽”看 `types` 是否含 `"Files"`。
- **ffmpeg.wasm / webcodecs 资产需手动 cp**：`assets/` 下 glue + core + 图标全套
  （`ffmpeg.js` wrapper、`814.ffmpeg.js` classic worker、`ffmpeg-core-st.{js,wasm}`
  ——这两份不入库，先跑 `scripts/fetch-ffmpeg-core.sh` 取回（29.5MB wasm-opt -Oz 后，
  sha256 校验）、`ffmpeg-engine.js`、`webcodecs-engine.js`、`webm-muxer.js`、
  `favicon.png`、`og-image.png`）。
  **dx（serve 与 build 皆然）不会把项目 `assets/` 拷进 `target/dx/.../web/public/`**——
  构建/跑起来前必须手动 cp 到该目录根，改过任一 JS 再 cp 一次（否则用的是旧副本）。
  glue/注入/favicon 全部用**相对**路径（`web.rs inject_scripts`、glue 自举
  `ffmpeg.js`、`loadCore` 的 `location.href` 拼接、`document::Link href`）——
  根域与 Pages 子路径（`/StickerProcess/`）都能解析；SPA fallback 会让缺失路径
  返回 HTML 伪装 200，警惕。
- **GitHub Pages 部署**：Pages 源 = GitHub Actions（workflow），三件套
  `configure-pages`→`upload-pages-artifact`→`deploy-pages`；push master 即发布，
  无手动发布路径（旧 gh-pages 分支 + build-ghpages.sh 已删）。首次从分支源迁到
  Actions 那次 deploy 报 `environment protection rules`（源被 configure-pages 同
  运行内才切成 workflow，自动建的 `github-pages` environment 带残留分支限制），
  **再跑一次即绿**（Run #1 失败 → #3 成功实证）。用户 PAT 对 Pages/env 设置 API
  是 404——但 workflow 的 `pages: write` 权限够用。core 的 CI 来源 =
  `TheFunny/ffmpeg-core-st` 的 Release（"Build ST core" workflow 产出；
  版本 tag 与两份 sha256 钉在 `scripts/fetch-ffmpeg-core.sh`，发新 core 就改这三处）。
- **python http.server 无 Cache-Control → 浏览器启发式缓存 wasm/loader JS**：改过 Rust
  重 `dx build` 后页面仍"不挂载"（`#main` 空、无 console 错误——`__wbg_init` 的 Promise
  reject 无人 catch），实为吃到旧 `StickerProcess.js` 与新 `StickerProcess_bg.wasm`
  混搭（wasm-bindgen 符号 hash 撕裂，报 `function import requires a callable`）。
  排查：对比磁盘 loader 与 wasm 的 `__wbg_*` hash 是否成对；处置：硬刷新/禁缓存重载，
  不要怀疑构建产物本身。
- **ffmpeg must be discoverable**: when updating ffmpeg, update BOTH `PATH` and
  `FFMPEG_DIR`, otherwise "ffmpeg not found" errors occur (see `docs/notes.md`).
- The ffmpeg binding crate is `ffmpeg-the-third` (see `Cargo.toml`, commented
  `ffmpeg-next` line). Keep that dependency in sync with `ffmpeg-sidecar` usage.
- GIF was changed from `yuva420p10` to `yuva420p` — do not "fix" it back; 10-bit
  yuva is invalid for these formats.
- **Static build gotchas** (see `docs/E6_INPROCESS_RESEARCH.md` §7.3 for details):
  vcpkg checkout must be recent enough (ffmpeg ≥ 5.1, avcodec ≥ 59.37);
  `libvpx[highbitdepth]` is required for yuv420p10le encoding (gif/png smoke
  tests pass without it, mp4 smoke fails with "Invalid argument");
  `avicap32.lib` is synthesized by `build.rs` from `build/avicap32.def`
  (Windows SDK doesn't ship it); debug zlib links as `zsd.lib`.
  `build.rs` resolves the lib dir from `FFMPEG_DIR` or `VCPKG_ROOT` and panics
  with setup instructions when neither resolves; it must point at the static
  install root, not the shared one, for the extra link libs to resolve.
- The `run_video` duration patch assumes the marker bytes `44 89 88` exist; if the
  encoded webm lacks them, the task errors (`Binary sequence not found`).
- Output overwrites are allowed (`.overwrite()` / ffmpeg `-y`); output files are
  timestamped to avoid collisions.
- Dioxus event closures receive `Event<T>` wrappers — use `evt.data.value()` /
  `evt.data.files()`; signal call-syntax needs a binding, so prefer `sig.cloned()` /
  `*sig.read()` / `*sig.peek()` over `state.field()`.
- Hand-edited lockfiles can break offline resolution via target-specific deps
  (e.g. libredox → plain); regenerate locks online when possible.
