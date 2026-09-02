# AGENTS.md

Guidance for AI agents working in this repository.

## Project Overview

**StickerProcess** is a desktop tool that converts images and short videos into
**Telegram-style stickers**, implemented in **Rust** with the **Dioxus 0.7** desktop
GUI (wry/WebView2):

- Video (mp4 / gif / apng) → animated sticker in **webm** (libvpx-vp9), target ≤ **256 KB**
- Image (jpg / jpeg / png / webp) → static sticker in **png**, target ≤ **512 KB**
- All media is scaled to fit **512×512** with aspect ratio preserved
  (`scale=512:512:force_original_aspect_ratio=decrease`, lanczos)

Legacy implementations are archived under `archive/` (gitignored). The former
iced 0.14 GUI was migrated to Dioxus in Phase A of `MIGRATION_PLAN.md`; later
phases add settings persistence (B), live progress/visuals (C), preview (D),
refactor/packaging (E).

## Repository Layout

| Path | Purpose |
|---|---|
| `src/main.rs` | Binary entry: logger init + Dioxus launch with window config |
| `src/app.rs` | Root component; `UiState` global signals (`settings` is the single source of truth for config); `TaskEntry`; `SUPPORTED`/`VIDEO`/`IMAGE` constants |
| `src/config.rs` | `Settings` model (serde+toml), persisted to `%APPDATA%/StickerProcess/settings.toml`; `load`/`save` + roundtrip tests |
| `src/runner.rs` | Async transcode loop: `run_all` → `run_single_task` (size-based retry, per-task cancel watcher, `TranscodeError` handling, retry/factor logging), progress channel, toasts |
| `src/components/` | UI widgets: `toolbar`, `task_list`, `number_field`, `drop_zone`, `progress_bar`, `toast`, `settings_panel`, `preview` |
| `src/app.css` | Stylesheet embedded via `include_str!`; theme variables (`[data-theme="dark"]`) landed in Phase C |
| `src/media.rs` | `MediaFile` model: type detection by extension, `probe()` (ffmpeg codec/duration check), duration, output path; enums; unit tests |
| `src/transcoder/` | Framework-agnostic core split into `mod.rs` (types + orchestration), `command.rs` (ffmpeg command gen + bitrate pure fns), `steps.rs` (webm duration patch, oxipng image pipe), `error.rs` (`TranscodeError`) |
| `MIGRATION_PLAN.md` | Roadmap and phase checklist (A done; B–E pending) |
| `Cargo.toml` | Dependencies + release profile (size-optimized, `lto = "fat"`, `panic = "abort"`, `strip = "symbols"`) |
| `notes.md` | Developer notes (see Gotchas) |
| `archive/` | Legacy implementations (gitignored) |
| `ico/`, `input/`, `out/`, `output/`, `target/` | App icon, media IO, build/cache dirs (gitignored) |

## Architecture

- **State**: `UiState` bundles Copy-able signals (`tasks`, `settings`, `show_settings`,
  `running`, `overall_progress`, `cancel`, `toasts`) provided to components
  via context.
- **Settings (Phase B)**: `config::Settings` is the single source of truth for all
  config (output dir, max retry, video/image size limits, retry shrink factor,
  duration factor table, forced FPS, theme). Mutate only via
  `UiState::update_settings(…)` — it applies the closure,
  then debounce-saves to `%APPDATA%/StickerProcess/settings.toml` (500 ms, latest
  write wins). Missing/corrupt file falls back to defaults at load. Size limits,
  the duration-factor table and the forced FPS are synced onto each `Transcoder`
  by the runner before every attempt; the retry shrink factor is applied by the
  runner when shrinking on size excess.
  Each queued task is a `TaskEntry { transcoder: Arc<Mutex<Transcoder>>, ... }`
  shared with background `tokio::task::spawn_blocking` workers via `Arc<Mutex<..>>`
  (deliberately NOT cloned). `TaskEntry` also carries *display mirror* fields
  (path/status/output_size/factor/progress/elapsed/error) so rendering never
  locks the mutex while a worker holds it during transcoding. Mirror writes go
  through the two write entries — `UiState::with_task(index, …)` for
  Transcoder-derived fields (locks, applies, syncs status/factor/output_size)
  and `UiState::touch_entry(index, …)` for UI-only mirrors
  (progress/elapsed/error/input_duration). Do not mutate mirrors via raw
  `tasks.with_mut` elsewhere.
- **Async probing**: adding a file creates the `Transcoder` without IO probing
  (`Status::Probing`); `MediaFile::probe` runs on a background thread and flips the
  mirror to `Pending`/`Alert`. Run is blocked while any task is probing.
  `Arc::ptr_eq` guards against stale index writes if the queue shifts.
- **Execution flow**: Run → `runner::run_all` async loop. Per task: assign a
  timestamped output path once, set `Processing`, run
  `Transcoder::run_with_progress()` inside `spawn_blocking` (progress parsed from
  ffmpeg stderr by `ffmpeg-sidecar`'s `iter()`, sent over an mpsc channel to the
  UI mirror), then `check_size()`. If over limit, shrink factor
  (`factor = factor / excess * retry_shrink_factor`) and retry up to `max_retry`, else advance.
  Task errors mark `Alert`, push an error toast, and skip to the next task.
  The `cancel` signal is checked between attempts; mid-task cancel bridges to
  `Transcoder.cancel_flag: Arc<AtomicBool>` via a watcher task that kills the
  running ffmpeg process, resets the task to `Pending`, and stops the whole run.
- **Sequential processing**: tasks run one at a time; overall progress = `(index+1)/len`.
- **File input**: toolbar uses a hidden `<input type="file" multiple>` triggered by a
  styled label; output-folder picking uses `rfd` (error dialogs were replaced by
  toasts in Phase C). Drag & drop works on the whole window: `ondragover` sets
  highlight, `ondrop` reads files and `FileData::path()` returns full paths on
  Windows (verified on dioxus 0.7.x).
- **Status**: `Probing` (async probe in flight), `Pending`, `Processing`,
  `Done`, `Alert` (error), `SizeExcess` (retry-able); badge colors map to CSS
  classes in `app.css`.
- **Theming**: CSS custom properties in `app.css`; `[data-theme="dark"]` on
  `<html>` overrides variables. Theme is part of `Settings` (persisted); toggles
  live in the toolbar and the settings panel.
- **Transcoding**:
  - Video → webm: `-b:v` computed from target size and duration
    (`256 * 1024 * 8 bits / duration_seconds`), `-bufsize = b:v * 1.5`, `-row-mt 1`,
    `crf 26`, pix_fmt yuv420p10 (mp4) / yuva420p (gif, apng), `-an`.
  - Image → png: ffmpeg pipes PNG to stdout (`-f image2`, `-c:v png`), then
    `oxipng::optimize_from_memory` (preset 4, safe strip, alpha optimize) writes the file.
- **Duration inference**: `ffmpeg-the-third` opens the input to read codec id and
  duration; APNG is assigned a fixed duration of 1 s (no probe). Output filenames are
  timestamps `%Y-%m-%d-%H%M%S%.3f` with extension `webm` / `png`.
- **Webm duration patch** (`transcoder::steps`): after encoding, the file is scanned
  for the binary marker `44 89 88` and 8 bytes are overwritten with `100f64`
  (big-endian) to force a fixed/fake duration on the sticker.
- **Status**: `Probing`, `Pending`, `Processing`, `Done`, `Alert` (error),
  `SizeExcess` (retry-able); badge colors map to CSS classes in `app.css`.
### Default size factors by duration (video)

`<1s → 1.2`, `<2s → 1.1`, `<3s → 1.0`, `<5s → 0.9`, `<8s → 0.8`, `≥8s → 0.7`;
the whole table is configurable in Settings (band edges stay fixed). GIF
additionally × 0.75 — a hard-coded bitrate-calculation patch, NOT a setting. The
factor is user-editable per task (0.1..=10.0) and only appears after the first
run lazily initializes it.

## Build & Run

```bash
cargo run            # debug
cargo build --release
cargo test           # media.rs unit tests
```

The first build fetches the dioxus dependency tree (~300 crates); rebuilds work
offline from the registry cache while `Cargo.lock` stays untouched.

## Conventions

- Commit messages: short lowercase English summary, e.g. `add APNG support and switch to
  ffmpeg-the-third`. Commits may combine feature + cleanup + minor tweaks.
- Keep the `Arc<Mutex<Transcoder>>` sharing pattern; do not clone task state;
  never hold a `MutexGuard` across an `.await`.
- Mirror writes: use `UiState::with_task` (Transcoder-derived fields) or
  `UiState::touch_entry` (UI-only fields); direct `tasks.with_mut` on mirror
  fields elsewhere will desynchronize the UI.
- Errors from the transcode core are `transcoder::TranscodeError` (thiserror);
  cancellation is matched via the enum, never by string comparison.
- Numeric inputs (`NumberInput`) use commit-on-enter/blur plus
  `key: "{value}"` remounting — the key guarantees the field shows the new
  value after any external change (manual commit or retry shrink).
  `TaskEntry::eq` MUST include every mirror field a component depends on:
  a missing field silently memo-skips re-renders (this broke factor display).
- New media types must be wired in **four** places:
  1. `src/app.rs` — `VIDEO` / `IMAGE` constants (`SUPPORTED` is derived)
  2. `src/components/toolbar.rs` — accept string derives from `SUPPORTED`
     automatically; extend only for special cases
  3. `src/media.rs` — extension match + enum variant + a unit test
  4. `src/transcoder/command.rs` — pix_fmt / codec handling, plus
     `MediaFile::probe` codec correction in `src/media.rs`

## Gotchas

- **ffmpeg must be discoverable**: when updating ffmpeg, update BOTH `PATH` and
  `FFMPEG_DIR`, otherwise "ffmpeg not found" errors occur (see `notes.md`).
- The ffmpeg binding crate is `ffmpeg-the-third` (see `Cargo.toml`, commented
  `ffmpeg-next` line). Keep that dependency in sync with `ffmpeg-sidecar` usage.
- GIF was changed from `yuva420p10` to `yuva420p` — do not "fix" it back; 10-bit
  yuva is invalid for these formats.
- The `run_video` duration patch assumes the marker bytes `44 89 88` exist; if the
  encoded webm lacks them, the task errors (`Binary sequence not found`).
- Output overwrites are allowed (`.overwrite()` / ffmpeg `-y`); output files are
  timestamped to avoid collisions.
- Dioxus event closures receive `Event<T>` wrappers — use `evt.data.value()` /
  `evt.data.files()`; signal call-syntax needs a binding, so prefer `sig.cloned()` /
  `*sig.read()` / `*sig.peek()` over `state.field()`.
- Hand-edited lockfiles can break offline resolution via target-specific deps
  (e.g. libredox → plain); regenerate locks online when possible.
