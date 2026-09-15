//! 框架无关的转码核心（Phase E3 拆分为子模块）：
//!
//! - [`error`]：`TranscodeError` 错误枚举
//! - [`command`]：ffmpeg 命令生成 + 码率/系数纯函数
//! - [`steps`]：webm 时长补丁、图片 oxipng 管道
//!
//! 对外暴露 `Transcoder` / `Factor` / `FileSize` / `Status` / `shrunk_factor`。

mod command;
mod error;
#[cfg(feature = "desktop")]
mod inprocess;
mod steps;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use error::TranscodeError;

#[cfg(feature = "desktop")]
use ffmpeg_the_third as ffmpeg;

#[cfg(feature = "desktop")]
use ffmpeg_sidecar::event::FfmpegEvent;

use crate::media::{MediaFile, MediaType};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct Factor {
    value: f64,
}

impl Factor {
    pub fn new(value: f64) -> Self {
        Self { value }
    }

    pub fn set(&mut self, value: f64) {
        self.value = value;
    }

    pub fn get(&self) -> f64 {
        self.value
    }
}

#[derive(Debug, Clone)]
pub struct FileSize {
    pub size: u64,
}

impl FileSize {
    pub fn new(size: u64) -> Self {
        Self { size }
    }

    pub fn set(&mut self, size: u64) {
        self.size = size;
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Status {
    /// 已入队，等待探测（添加时异步探测时长/编码）。
    Probing,
    Pending,
    Processing,
    Done,
    Alert,
    SizeExcess,
}

/// 转码引擎：sidecar（ffmpeg 子进程）/ inprocess（libav 进程内）为桌面双轨；
/// webcodecs（浏览器原生编解码）与 ffmpeg-wasm（Route A）为网页端。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Engine {
    Sidecar,
    Inprocess,
    Webcodecs,
    FfmpegWasm,
}

impl Engine {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sidecar" => Some(Self::Sidecar),
            "inprocess" => Some(Self::Inprocess),
            "webcodecs" => Some(Self::Webcodecs),
            "ffmpeg-wasm" => Some(Self::FfmpegWasm),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sidecar => "sidecar",
            Self::Inprocess => "inprocess",
            Self::Webcodecs => "webcodecs",
            Self::FfmpegWasm => "ffmpeg-wasm",
        }
    }

    /// 网页端引擎矩阵（矩阵即策略，设置值不参与）：
    /// Gif/Apng → ffmpeg-wasm（alpha 双轨）；Mp4 → WebCodecs，其 VP9 caps
    /// 不可用时兜底 ffmpeg-wasm（W4 自建 core，10-bit 已验证）；图片 → webcodecs。
    /// 返回 (engine, glue kind, ffmpeg-wasm 分支的 pix_fmt)。
    /// `Video(AnimatedWebP)` 仅桌面存在（probe 纠正产物）；web 端动画 webp
    /// 恒为 Image(Webp)（输出首帧 PNG），兜底臂纯防编译期不穷尽。
    pub fn for_web(
        media_type: &MediaType,
        webcodecs_ok: bool,
    ) -> (Engine, &'static str, &'static str) {
        use crate::media::{MediaType::*, VideoType::*};
        match media_type {
            Video(Gif | Apng) => (Engine::FfmpegWasm, "video", "yuva420p"),
            Video(Mp4) if webcodecs_ok => (Engine::Webcodecs, "video", "yuv420p10"),
            Video(_) => (Engine::FfmpegWasm, "video", "yuv420p10"),
            Image(_) => (Engine::Webcodecs, "image", "yuva420p"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Transcoder {
    pub media_file: MediaFile,
    pub size_factor: Option<Factor>,
    pub output_size: Option<FileSize>,
    pub status: Status,
    /// 置位后中断正在运行的 ffmpeg 进程（runner 经 Arc 桥接 UI 的 cancel 信号）。
    pub cancel_flag: Arc<AtomicBool>,
    /// 时长→默认系数表，区间固定：<1s, <2s, <3s, <5s, <8s, ≥8s（来自设置）。
    pub duration_factors: [f64; 6],
    /// 强制输出帧率（fps）；<=0 表示不强制。
    pub target_fps: f64,
    /// 内存输出（网页端契约）：Bytes 源任务转码产物写这里而非磁盘。
    /// 桌面 Path 源恒为 None（走 output 路径）。
    pub output_bytes: Option<Vec<u8>>,
}

impl Transcoder {
    pub fn new(media_file: MediaFile) -> Self {
        // 构造时不做 IO 探测（仅按扩展名分类），probe 延后到后台线程
        Self {
            media_file,
            size_factor: None,
            output_size: None,
            status: Status::Probing,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            duration_factors: [1.2, 1.1, 1.0, 0.9, 0.8, 0.7],
            target_fps: 0.0,
            output_bytes: None,
        }
    }

    /// 同步执行探测（读编码/时长），供后台线程调用。
    /// wasm：media_file.probe() 直接 Ok（见 media.rs 平台分支）。
    #[cfg(feature = "desktop")]
    pub fn probe(&mut self) -> Result<(), ffmpeg::Error> {
        self.media_file.probe()
    }

    #[cfg(not(feature = "desktop"))]
    pub fn probe(&mut self) -> Result<(), ()> {
        self.media_file.probe()
    }

    pub fn set_output<P: AsRef<Path> + ?Sized>(&mut self, output: &P) {
        self.media_file.set_output(output.as_ref().to_path_buf());
    }

    pub fn get_output(&self) -> Option<&PathBuf> {
        self.media_file.output()
    }

    /// 为任务分配带时间戳的输出文件（视频 .webm / 图片 .png）。
    pub fn set_output_dir<P: AsRef<Path> + ?Sized>(
        &mut self,
        output_dir: &P,
    ) -> Result<(), TranscodeError> {
        let ext = match self
            .media_file
            .r#type()
            .ok_or(TranscodeError::InvalidMediaType)?
        {
            MediaType::Video(_) => "webm",
            MediaType::Image(_) => "png",
        };
        // chrono::Local 依赖 std::time（wasm 未实现）——网页端用 JS Date 的 ISO 时间戳
        #[cfg(not(target_arch = "wasm32"))]
        let time = chrono::Local::now();
        let output = {
            #[cfg(not(target_arch = "wasm32"))]
            {
                output_dir
                    .as_ref()
                    .join(format!("{}.{}", time.format("%Y-%m-%d-%H%M%S%.3f"), ext))
            }
            #[cfg(target_arch = "wasm32")]
            {
                // ISO: "2026-09-11T12:34:56.789Z" → "2026-09-11-123456.789"（唯一性即可）
                let d = js_sys::Date::new(&js_sys::Date::now().into());
                let s = d.to_iso_string().as_string().unwrap_or_default();
                let s = s.replace(['T', ':', 'Z'], "-");
                output_dir.as_ref().join(format!("{s}.{ext}"))
            }
        };
        self.set_output(&output);
        Ok(())
    }

    /// 执行转码（按 engine 设置分发 sidecar / inprocess / webcodecs）；
    /// 进度经回调上报（0..=1）。取消经 `cancel_flag` 中断。
    pub fn run_with_progress(
        &mut self,
        engine: &str,
        mut on_progress: impl FnMut(f32),
    ) -> Result<(), TranscodeError> {
        if engine == "webcodecs" || engine == "ffmpeg-wasm" {
            // 网页端引擎（分发在 runner.rs wasm 两段式路径）；桌面设置这两个值
            // → 明确 Alert（resolve_engine 只兜底 webcodecs，ffmpeg-wasm 漏到这）
            let _ = &mut on_progress;
            return Err(TranscodeError::UnsupportedEngine(
                "web engines (webcodecs / ffmpeg-wasm) require the web build",
            ));
        }
        #[cfg(feature = "desktop")]
        if engine == "inprocess" {
            return self.run_inprocess(on_progress);
        }
        // sidecar 分支（ffmpeg 子进程）桌面专属；wasm 无子进程
        #[cfg(feature = "desktop")]
        {
            // 上次取消遗留的标志必须清掉，否则同一任务再次 Run 会立即被"取消"
            self.cancel_flag.store(false, Ordering::Relaxed);
            let media_type = self
                .media_file
                .r#type()
                .ok_or(TranscodeError::InvalidMediaType)?;
            let mut command = self.gen_command()?;
            let mut process = command.spawn().map_err(|_| TranscodeError::Spawn)?;
            match media_type {
                MediaType::Video(_) => {
                    // 图片路径用 stdout 传 PNG，视频走 stderr 解析的进度事件
                    let duration = self.media_file.duration().unwrap_or(1.0).max(0.001);
                    for event in process.iter().map_err(|_| TranscodeError::ReadOutput)? {
                        if self.cancel_flag.load(Ordering::Relaxed) {
                            let _ = process.kill();
                            return Err(TranscodeError::Cancelled);
                        }
                        if let FfmpegEvent::Progress(p) = event {
                            on_progress((command::parse_progress_time(&p.time) / duration) as f32);
                        }
                    }
                    self.run_video()
                }
                MediaType::Image(_) => {
                    if self.cancel_flag.load(Ordering::Relaxed) {
                        return Err(TranscodeError::Cancelled);
                    }
                    self.run_image(&mut process)
                }
            }
        }
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (&engine, &mut on_progress);
            Err(TranscodeError::UnsupportedEngine(
                "sidecar requires desktop",
            ))
        }
    }

    /// 收尾产物落位：Path 源写盘（桌面现状），Bytes 源写内存（网页端契约）。
    /// 两条路径都同步 output_size 供尺寸重试判定。
    pub fn store_output(&mut self, bytes: Vec<u8>) -> Result<(), TranscodeError> {
        if self.media_file.is_bytes() {
            let size = bytes.len() as u64;
            self.output_bytes = Some(bytes);
            match &mut self.output_size {
                Some(s) => s.set(size),
                None => self.output_size = Some(FileSize::new(size)),
            }
            return Ok(());
        }
        #[cfg(feature = "desktop")]
        {
            let path = self.get_output().ok_or(TranscodeError::OutputNotSet)?;
            std::fs::write(path, &bytes).map_err(|e| TranscodeError::SizeCheck(e.to_string()))?;
            Ok(())
        }
        #[cfg(not(feature = "desktop"))]
        {
            Err(TranscodeError::OutputNotSet)
        }
    }

    /// 转码完成后读取输出文件大小（供尺寸重试判定）。
    pub fn check_size(&mut self) -> Result<&FileSize, TranscodeError> {
        let size = if let Some(bytes) = &self.output_bytes {
            bytes.len() as u64
        } else {
            self.get_output()
                .ok_or(TranscodeError::OutputNotSet)?
                .metadata()
                .map_err(|e| TranscodeError::SizeCheck(e.to_string()))?
                .len()
        };
        match &mut self.output_size {
            Some(size_slot) => size_slot.set(size),
            None => self.output_size = Some(FileSize::new(size)),
        }
        Ok(self.output_size.as_ref().unwrap())
    }
}

/// 超限重试的系数缩放：factor / excess * shrink（runner 在重试决策时使用）。
pub fn shrunk_factor(current: f64, excess: f64, shrink: f64) -> f64 {
    current / excess * shrink
}

#[cfg(test)]
mod tests {
    use super::shrunk_factor;

    #[test]
    fn shrunk_factor_formula() {
        // 与旧行为逐字对应：factor / excess * 0.96
        assert!((shrunk_factor(0.9, 1.5, 0.96) - 0.576).abs() < 1e-12);
        assert!((shrunk_factor(1.0, 1.0, 0.96) - 0.96).abs() < 1e-12);
    }

    #[test]
    fn for_web_matrix() {
        use super::Engine;
        use crate::media::{ImageType, MediaType, VideoType};
        // GIF/APNG → ffmpeg-wasm（alpha 路径），pix_fmt 恒 yuva420p
        assert_eq!(
            Engine::for_web(&MediaType::Video(VideoType::Gif), true),
            (Engine::FfmpegWasm, "video", "yuva420p")
        );
        assert_eq!(
            Engine::for_web(&MediaType::Video(VideoType::Apng), false),
            (Engine::FfmpegWasm, "video", "yuva420p")
        );
        // MP4：WebCodecs caps 可用走 webcodecs，否则兜底 ffmpeg-wasm（10-bit）
        assert_eq!(
            Engine::for_web(&MediaType::Video(VideoType::Mp4), true),
            (Engine::Webcodecs, "video", "yuv420p10")
        );
        assert_eq!(
            Engine::for_web(&MediaType::Video(VideoType::Mp4), false),
            (Engine::FfmpegWasm, "video", "yuv420p10")
        );
        // 图片 → webcodecs image
        for img in [ImageType::Png, ImageType::Jpg, ImageType::Webp] {
            assert_eq!(
                Engine::for_web(&MediaType::Image(img), true),
                (Engine::Webcodecs, "image", "yuva420p")
            );
        }
        // AnimatedWebP 仅桌面出现；若真到 web 侧，兜底 ffmpeg-wasm 无害（web 恒 Image(Webp)）
        assert_eq!(
            Engine::for_web(&MediaType::Video(VideoType::AnimatedWebP), true),
            (Engine::FfmpegWasm, "video", "yuv420p10")
        );
    }
}
