//! 兼容性报告（"Compatibility Report" 弹窗的数据层）。
//!
//! 把散在各处的可用性事实收拢成分组行：sidecar 探测缓存、libav 版本、
//! 浏览器 caps、引擎资产可达性、以及 `Engine::for_web` 的路由矩阵。
//! **只读**：不改转码路径、不复刻矩阵（路由行直接调 `Engine::for_web`）。
//!
//! 桌面探测是阻塞的（`ffmpeg -version` 子进程 + 写临时文件测目录可写），统一经
//! `runner::run_blocking` 落到线程池；`collect` 两平台同名同签名，组件侧无分支。

use crate::config::Settings;

/// 单行结论。颜色类名见 `components/compat.rs` 的 `.compat-state.<css>`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatState {
    Ok,
    /// 可用但降级（例如引擎设置会被兜底、输出目录待自动创建）。
    Warn,
    Fail,
    Info,
}

impl CompatState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Info => "INFO",
        }
    }

    pub fn css(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompatRow {
    pub label: String,
    pub state: CompatState,
    pub detail: String,
}

impl CompatRow {
    pub fn new(label: impl Into<String>, state: CompatState, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            state,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompatSection {
    pub title: &'static str,
    pub rows: Vec<CompatRow>,
}

impl CompatSection {
    pub fn new(title: &'static str, rows: Vec<CompatRow>) -> Self {
        Self { title, rows }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompatReport {
    pub sections: Vec<CompatSection>,
}

/// 探测整体失败（线程池 join 失败等）时的兜底报告——不能给用户一个空白弹窗。
/// 桌面专属：web 侧的失败各自降级成 Fail 行，没有"整份报告拿不到"的路径。
#[cfg(not(target_arch = "wasm32"))]
fn failed(detail: impl Into<String>) -> CompatReport {
    CompatReport {
        sections: vec![CompatSection::new(
            "Probe",
            vec![CompatRow::new(
                "Compatibility probe",
                CompatState::Fail,
                detail,
            )],
        )],
    }
}

/// 采集兼容性报告（两平台同名同签名：组件 `spawn` 后直接 await）。
#[cfg(not(target_arch = "wasm32"))]
pub async fn collect(settings: Settings) -> CompatReport {
    match crate::runner::run_blocking(move || desktop::collect_blocking(&settings)).await {
        Ok(report) => report,
        Err(e) => failed(format!("probe thread failed: {e}")),
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn collect(settings: Settings) -> CompatReport {
    web::collect(settings).await
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use super::{CompatReport, CompatRow, CompatSection, CompatState};
    use crate::config::{OutputDirState, Settings};
    use ffmpeg_the_third as ffmpeg;
    use std::path::Path;

    pub fn collect_blocking(settings: &Settings) -> CompatReport {
        let resolved = crate::runner::resolve_engine(settings.engine, false);
        CompatReport {
            sections: vec![
                CompatSection::new("Platform", platform_rows(settings, resolved)),
                CompatSection::new("Engines", engine_rows()),
                CompatSection::new("Output", vec![output_row(settings)]),
                CompatSection::new(
                    "Routing (policy)",
                    vec![CompatRow::new(
                        "All media types",
                        CompatState::Info,
                        format!("{} (from the engine setting)", resolved.as_str()),
                    )],
                ),
            ],
        }
    }

    /// 设置值 vs 实际解析结果：保存了 sidecar 但机器上没有 ffmpeg 时，
    /// 这里能立刻说清"为什么实际跑的是 inprocess"。
    fn platform_rows(
        settings: &Settings,
        resolved: crate::transcoder::Engine,
    ) -> Vec<CompatRow> {
        let (state, detail) = if resolved == settings.engine {
            (
                CompatState::Info,
                format!("'{}'", settings.engine.as_str()),
            )
        } else {
            (
                CompatState::Warn,
                format!(
                    "'{}' unavailable → falls back to '{}'",
                    settings.engine.as_str(),
                    resolved.as_str()
                ),
            )
        };
        vec![
            CompatRow::new(
                "Build",
                CompatState::Info,
                format!(
                    "{} · {}/{}",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            ),
            CompatRow::new("Engine setting", state, detail),
        ]
    }

    fn engine_rows() -> Vec<CompatRow> {
        let mut rows = Vec::new();
        match crate::sidecar_probe::SidecarProbe::probe() {
            Some(probe) => {
                rows.push(CompatRow::new(
                    "ffmpeg.exe (sidecar)",
                    CompatState::Ok,
                    probe.exe.display().to_string(),
                ));
                rows.push(CompatRow::new(
                    "libvpx-vp9 (sidecar)",
                    if probe.has_vp9 {
                        CompatState::Ok
                    } else {
                        CompatState::Fail
                    },
                    if probe.has_vp9 {
                        "encoder found"
                    } else {
                        "encoder missing — sidecar cannot transcode VP9"
                    },
                ));
                let (state, detail) = match ffmpeg_version(&probe.exe) {
                    Some(v) => (CompatState::Info, v),
                    None => (CompatState::Warn, "`ffmpeg -version` gave no output".into()),
                };
                rows.push(CompatRow::new("ffmpeg version (sidecar)", state, detail));
            }
            None => {
                rows.push(CompatRow::new(
                    "ffmpeg.exe (sidecar)",
                    CompatState::Fail,
                    "not found (app folder or PATH)",
                ));
                rows.push(CompatRow::new(
                    "libvpx-vp9 (sidecar)",
                    CompatState::Info,
                    "n/a — no ffmpeg.exe",
                ));
            }
        }
        // 与 inprocess.rs 同一初始化入口（LazyLock 只跑一次）
        let _ = ffmpeg::init();
        rows.push(CompatRow::new(
            "libav (inprocess)",
            CompatState::Ok,
            format!("avutil {}", version_string(ffmpeg::util::version())),
        ));
        let vp9 = ffmpeg::codec::encoder::find_by_name("libvpx-vp9").is_some();
        rows.push(CompatRow::new(
            "libvpx-vp9 (inprocess)",
            if vp9 {
                CompatState::Ok
            } else {
                CompatState::Fail
            },
            if vp9 {
                "encoder compiled in"
            } else {
                "encoder not compiled in"
            },
        ));
        rows
    }

    fn output_row(settings: &Settings) -> CompatRow {
        let path = settings.output_dir.clone();
        // 空串是可达状态（工具条的 output_dir_state 也为它画红框）：按"未设置"报，
        // 别报成父目录缺失——那是两回事。
        if path.trim().is_empty() {
            return CompatRow::new(
                "Output folder",
                CompatState::Fail,
                "not set — pick a folder in the toolbar or settings",
            );
        }
        match settings.output_dir_state() {
            OutputDirState::Ok => {
                let writable = settings.output_dir_writable();
                CompatRow::new(
                    "Output folder",
                    if writable {
                        CompatState::Ok
                    } else {
                        CompatState::Fail
                    },
                    if writable {
                        format!("{path} · writable")
                    } else {
                        format!("{path} · not writable")
                    },
                )
            }
            OutputDirState::WillCreate => CompatRow::new(
                "Output folder",
                CompatState::Warn,
                format!("{path} · will be created on Run"),
            ),
            OutputDirState::Missing => CompatRow::new(
                "Output folder",
                CompatState::Fail,
                format!("{path} · parent folder missing"),
            ),
        }
    }

    /// libav 版本整数 → "major.minor.micro"（C 宏的位布局 16/8/8）。
    pub(super) fn version_string(v: u32) -> String {
        format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
    }

    /// `ffmpeg -version` 首行；失败/空输出 → None（报告降级为 Warn，不报错中断）。
    /// 阻塞子进程——只在 `collect_blocking` 里调用，已经在 `spawn_blocking` 上。
    fn ffmpeg_version(exe: &Path) -> Option<String> {
        let out = std::process::Command::new(exe).arg("-version").output().ok()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let first = stdout.lines().next()?.trim();
        (!first.is_empty()).then(|| first.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::{CompatReport, CompatRow, CompatSection, CompatState};
    use crate::config::Settings;
    use crate::media::{ImageType, MediaType, VideoType};
    use crate::transcoder::Engine;
    use crate::transcoder::web::{ProbeDetails, compat_assets, probe_details};

    pub async fn collect(settings: Settings) -> CompatReport {
        let details = probe_details().await;
        let webcodecs_ok = details.is_some_and(|d| d.supported());
        CompatReport {
            sections: vec![
                CompatSection::new("Platform", platform_rows(&settings)),
                CompatSection::new("Engines", engine_rows(details)),
                CompatSection::new("Assets", asset_rows(compat_assets().await)),
                CompatSection::new("Routing (policy)", routing_rows(webcodecs_ok)),
            ],
        }
    }

    fn platform_rows(settings: &Settings) -> Vec<CompatRow> {
        let window = web_sys::window();
        let secure = window
            .as_ref()
            .is_some_and(|w| w.is_secure_context());
        let user_agent = window
            .as_ref()
            .and_then(|w| w.navigator().user_agent().ok())
            .unwrap_or_default();
        vec![
            CompatRow::new(
                "Build",
                CompatState::Info,
                format!("{} · wasm32", env!("CARGO_PKG_VERSION")),
            ),
            CompatRow::new(
                "Engine setting",
                CompatState::Info,
                format!(
                    "'{}' (ignored on web — the media type picks the engine)",
                    settings.engine.as_str()
                ),
            ),
            CompatRow::new(
                "Secure context",
                if secure {
                    CompatState::Ok
                } else {
                    CompatState::Fail
                },
                if secure {
                    "yes (WebCodecs requires it)".to_string()
                } else {
                    "no — WebCodecs is unavailable outside https/localhost".to_string()
                },
            ),
            CompatRow::new("Browser", CompatState::Info, truncate(&user_agent, 120)),
        ]
    }

    /// MP4 的 WebCodecs 快路径任一条件缺失都只是**降级**（兜底 ffmpeg.wasm 仍能出
    /// 贴纸，代价是 29.5MB core 下载 + 慢得多），故标 Warn 而不是 Fail；图片路径
    /// 没有兜底，缺 ImageDecoder/OffscreenCanvas 就是 Fail。
    fn engine_rows(details: Option<ProbeDetails>) -> Vec<CompatRow> {
        let Some(d) = details else {
            return vec![CompatRow::new(
                "WebCodecs glue",
                CompatState::Fail,
                "webcodecs-engine.js did not load or did not answer",
            )];
        };
        let degraded = |ok: bool| {
            if ok {
                CompatState::Ok
            } else {
                CompatState::Warn
            }
        };
        vec![
            CompatRow::new(
                "WebCodecs VideoEncoder",
                degraded(d.video_encoder),
                if d.video_encoder {
                    "available"
                } else {
                    "missing — MP4 falls back to ffmpeg.wasm (29.5 MB core)"
                },
            ),
            CompatRow::new(
                "requestVideoFrameCallback",
                degraded(d.rvfc),
                if d.rvfc {
                    "available"
                } else {
                    "missing — MP4 falls back to ffmpeg.wasm (Firefox < 132)"
                },
            ),
            CompatRow::new(
                "WebM muxer",
                degraded(d.muxer),
                if d.muxer {
                    "WebMMuxer loaded"
                } else {
                    "missing — webm-muxer.js did not load, MP4 falls back to ffmpeg.wasm"
                },
            ),
            CompatRow::new(
                "VP9 8-bit encode",
                degraded(d.vp9_8bit),
                if d.vp9_8bit {
                    "vp09.00.10.08 supported"
                } else {
                    "unsupported — MP4 falls back to ffmpeg.wasm"
                },
            ),
            CompatRow::new(
                "ImageDecoder",
                flag(d.image_decoder),
                if d.image_decoder {
                    "available"
                } else {
                    "missing — images and GIF/APNG probing fail (Firefox < 133, Safari)"
                },
            ),
            CompatRow::new(
                "OffscreenCanvas.convertToBlob",
                flag(d.offscreen_canvas),
                if d.offscreen_canvas {
                    "available"
                } else {
                    "missing — the image path is unavailable"
                },
            ),
        ]
    }

    fn asset_rows(assets: Option<(Vec<String>, Vec<String>)>) -> Vec<CompatRow> {
        let Some((webcodecs_missing, ffmpeg_missing)) = assets else {
            return vec![CompatRow::new(
                "Engine assets",
                CompatState::Fail,
                "asset check failed (glue not loaded)",
            )];
        };
        vec![
            missing_row(
                "WebCodecs assets (webm-muxer.js, webcodecs-engine.js)",
                &webcodecs_missing,
            ),
            missing_row(
                "ffmpeg.wasm assets (glue + 29.5 MB core)",
                &ffmpeg_missing,
            ),
        ]
    }

    /// 资产缺失：最可能的成因就是"忘了把 assets/ 拷到 web 根"（dx 不会拷，
    /// 见 AGENTS 的 Gotchas），故提示直接写出来。
    fn missing_row(label: &str, missing: &[String]) -> CompatRow {
        if missing.is_empty() {
            CompatRow::new(label, CompatState::Ok, "reachable")
        } else {
            CompatRow::new(
                label,
                CompatState::Fail,
                format!(
                    "missing: {} — copy assets/ to the web root (scripts/fetch-ffmpeg-core.sh fetches the core)",
                    missing.join(", ")
                ),
            )
        }
    }

    /// 路由矩阵：直接问 `Engine::for_web`（唯一出处），报告只负责展示。
    fn routing_rows(webcodecs_ok: bool) -> Vec<CompatRow> {
        let cases: [(&str, MediaType); 4] = [
            ("mp4", MediaType::Video(VideoType::Mp4)),
            ("gif", MediaType::Video(VideoType::Gif)),
            ("apng", MediaType::Video(VideoType::Apng)),
            ("images (jpg/png/webp)", MediaType::Image(ImageType::Jpg)),
        ];
        cases
            .into_iter()
            .map(|(label, media_type)| {
                let (engine, _kind, pix_fmt) = Engine::for_web(&media_type, webcodecs_ok);
                let detail = match media_type {
                    MediaType::Image(_) => {
                        format!("{} · 512px PNG (animated webp: first frame only)", engine.as_str())
                    }
                    MediaType::Video(_) => format!("{} · {pix_fmt}", engine.as_str()),
                };
                CompatRow::new(label, CompatState::Info, detail)
            })
            .collect()
    }

    fn flag(ok: bool) -> CompatState {
        if ok { CompatState::Ok } else { CompatState::Fail }
    }

    /// 长度截断（按字符，不切坏 UTF-8）。
    fn truncate(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            return s.to_string();
        }
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn version_string_unpacks_avutil_bits() {
        assert_eq!(desktop::version_string((59 << 16) | (8 << 8) | 100), "59.8.100");
        assert_eq!(desktop::version_string(0), "0.0.0");
    }

    /// 报告必须跟着探测缓存走：探测不到 ffmpeg.exe 时不许写成可用（反之亦然）。
    /// `--nocapture` 下打印整份报告，作为桌面端的实测证据。
    #[test]
    fn sidecar_row_follows_probe() {
        let report = desktop::collect_blocking(&Settings::default());
        let row = report
            .sections
            .iter()
            .flat_map(|s| &s.rows)
            .find(|r| r.label == "ffmpeg.exe (sidecar)")
            .expect("sidecar row present");
        let expected_fail = crate::sidecar_probe::SidecarProbe::probe().is_none();
        assert_eq!(row.state == CompatState::Fail, expected_fail, "{}", row.detail);
        assert!(!row.detail.is_empty());
        for section in &report.sections {
            assert!(!section.rows.is_empty(), "empty section {}", section.title);
            println!("[{}]", section.title);
            for r in &section.rows {
                println!("  {} {} — {}", r.state.label(), r.label, r.detail);
            }
        }
    }
}
