# AGENTS.md

Guidance for AI agents working in this repository.

## Project Overview

**StickerProcess** is a desktop tool that converts images and short videos into
**Telegram-style stickers**, implemented in **Rust** with the **iced** GUI:

- Video (mp4 / gif / apng) → animated sticker in **webm** (libvpx-vp9), target ≤ **256 KB**
- Image (jpg / jpeg / png / webp) → static sticker in **png**, target ≤ **512 KB**
- All media is scaled to fit **512×512** with aspect ratio preserved
  (`scale=512:512:force_original_aspect_ratio=decrease`, lanczos)

The former Python/tkinter implementation was superseded by this Rust version and
archived under `archive/` (gitignored). Do not treat Python files as active code.

## Repository Layout

| Path | Purpose |
|---|---|
| `src/main.rs` | iced GUI application: `App`, `Message` enum, task orchestration, views |
| `src/media.rs` | `MediaFile` model: type detection by extension, duration, output path; `MediaType`/`VideoType`/`ImageType`/`StickerType` enums; unit tests |
| `src/transcoder.rs` | `Transcoder`: ffmpeg command generation, image/video processing, size checking; `Factor`, `FileSize`, `Status`; ffmpeg probing via `ffmpeg-the-third`; `MediaFile::infer_duration` |
| `Cargo.toml` | Dependencies + release profile (size-optimized, `lto = "fat"`, `panic = "abort"`, `strip = "symbols"`) |
| `notes.md` | Developer notes (see Gotchas) |
| `examples/` | Small Rust snippets (e.g. `f64tobytes.rs`) |
| `archive/` | Legacy Python implementation and scripts (gitignored) |
| `ico/`, `input/`, `out/`, `output/`, `target/` | App icon, media IO, build/cache dirs (gitignored) |

## Architecture

- **GUI**: iced 0.14 (`iced_aw` `NumberInput` for retry times and size factor).
- **State**: `App` holds `tasks: Vec<Arc<Mutex<Transcoder>>>` — shared with background
  `tokio::task::spawn_blocking` workers via `Arc<Mutex<..>>` (deliberately NOT cloned).
- **Message flow**: `SelectRun` → `CurrentProcess{index, retry}` → `NextProcess(ProcessResult)`
  → either retry (size excess) or advance to the next task → `Done`.
- **Sequential processing**: tasks run one at a time; progress = `(index + 1) / tasks.len()`.
- **Retry loop** (size-based): after a run, `check_size()` compares output size against the
  target; if `size_excess_factor() > 1.0`, the `size_factor` is shrunk
  (`factor = factor / excess * 0.96`) and the task is re-run, up to `max_retry` (GUI, default 3).
- **Transcoding**:
  - Video → webm: `-b:v` computed from target size and duration
    (`256 * 1024 * 8 bits / duration_seconds`), `-bufsize = b:v * 1.5`, `-row-mt 1`,
    `crf 26`, `pix_fmt` yuv420p10 (mp4) / yuva420p (gif, apng), `-an`.
  - Image → png: ffmpeg pipes PNG to stdout (`-f image2`, `-c:v png`), then
    `oxipng::optimize_from_memory` (preset 4, safe strip, alpha optimize) writes the file.
- **Duration inference**: `ffmpeg-the-third` opens the input to read codec id and duration;
  APNG is assigned a fixed duration of 1 s (no probe). Output filenames are timestamps
  `%Y-%m-%d-%H%M%S%.3f` with extension `webm` / `png`.
- **Webm duration patch** (`run_video`): after encoding, the file is scanned for the binary
  marker `44 89 88` and 8 bytes are overwritten with `100f64` (big-endian) to force a
  fixed/fake duration on the sticker.
- **Status**: `Pending`, `Processing`, `Done`, `Alert` (error), `SizeExcess` (retry-able).

### Default size factors by duration (video)

`<1s → 1.2`, `<2s → 1.1`, `<3s → 1.0`, `<5s → 0.9`, `<8s → 0.8`, `≥8s → 0.7`;
GIF additionally × 0.75. The factor is user-editable in the GUI (0.1..=10.0).

## Build & Run

```bash
cargo run            # debug
cargo build --release
cargo test           # media.rs unit tests
```

## Conventions

- Commit messages: short lowercase English summary, e.g. `add APNG support and switch to
  ffmpeg-the-third`. Commits may combine feature + cleanup + minor tweaks.
- Keep the `Arc<Mutex<Transcoder>>` sharing pattern; do not clone task state.
- New media types must be wired in **three** places:
  1. `src/main.rs` — `VIDEO` / `IMAGE` / `SUPPORTED` constants
  2. `src/media.rs` — extension match + `VideoType`/`ImageType` enum + a unit test
  3. `src/transcoder.rs` — codec probing (`infer_duration`), duration handling, pix_fmt,
     and any factor adjustments

## Gotchas

- **ffmpeg must be discoverable**: when updating ffmpeg, update BOTH `PATH` and
  `FFMPEG_DIR`, otherwise "ffmpeg not found" errors occur (see `notes.md`).
- The ffmpeg binding crate was switched from `ffmpeg-next` to `ffmpeg-the-third`
  (see `Cargo.toml`, commented line). Keep that dependency in sync with
  `ffmpeg-sidecar` usage.
- GIF was changed from `yuva420p10` to `yuva420p` — do not "fix" it back; 10-bit
  yuva is invalid for these formats.
- The `run_video` duration patch assumes the marker bytes `44 89 88` exist; if the
  encoded webm lacks them, the task errors (`Binary sequence not found`).
- Output overwrites are allowed (`.overwrite()` / ffmpeg `-y`); output files are
  timestamped to avoid collisions.
