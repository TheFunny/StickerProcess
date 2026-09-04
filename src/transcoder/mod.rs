//! 框架无关的转码核心（Phase E3 拆分为子模块）：
//!
//! - [`error`]：`TranscodeError` 错误枚举
//! - [`command`]：ffmpeg 命令生成 + 码率/系数纯函数
//! - [`steps`]：webm 时长补丁、图片 oxipng 管道
//!
//! 对外暴露 `Transcoder` / `Factor` / `FileSize` / `Status` / `shrunk_factor`。

mod command;
mod error;
mod inprocess;
mod steps;

pub use error::TranscodeError;

use ffmpeg_the_third as ffmpeg;

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
        }
    }

    /// 同步执行探测（读编码/时长），供后台线程调用。
    pub fn probe(&mut self) -> Result<(), ffmpeg::Error> {
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
        let time = chrono::Local::now();
        let output =
            output_dir
                .as_ref()
                .join(format!("{}.{}", time.format("%Y-%m-%d-%H%M%S%.3f"), ext));
        self.set_output(&output);
        Ok(())
    }

    /// 执行转码（按 engine 设置分发 sidecar / inprocess）；
    /// 进度经回调上报（0..=1）。取消经 `cancel_flag` 中断。
    pub fn run_with_progress(
        &mut self,
        engine: &str,
        mut on_progress: impl FnMut(f32),
    ) -> Result<(), TranscodeError> {
        if engine == "inprocess" {
            return self.run_inprocess(on_progress);
        }
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

    /// 转码完成后读取输出文件大小（供尺寸重试判定）。
    pub fn check_size(&mut self) -> Result<&FileSize, TranscodeError> {
        let metadata = self
            .get_output()
            .ok_or(TranscodeError::OutputNotSet)?
            .metadata()
            .map_err(|e| TranscodeError::SizeCheck(e.to_string()))?;
        match &mut self.output_size {
            Some(size) => size.set(metadata.len()),
            None => self.output_size = Some(FileSize::new(metadata.len())),
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
}
