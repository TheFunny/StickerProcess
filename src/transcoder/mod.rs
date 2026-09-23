//! 框架无关的转码核心（Phase E3 拆分为子模块）：
//!
//! - [`error`]：`TranscodeError` 错误枚举
//! - [`command`]：ffmpeg 命令生成 + 码率/系数纯函数
//! - [`steps`]：webm 时长补丁、图片 oxipng 管道
//!
//! 对外暴露 `Transcoder` / `Status` / `shrunk_factor`。

mod command;
mod error;
#[cfg(feature = "desktop")]
mod inprocess;
mod steps;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// 时长→系数表的出厂值（config 与 Transcoder 共用一份，避免两处漂移）。
pub use command::DEFAULT_DURATION_FACTORS;
pub use error::TranscodeError;

/// 产物文件名里输入名主干的最大字符数（Windows 路径长度上限的粗保护）。
const MAX_NAME_STEM: usize = 64;

/// 产物路径唯一序号：时间戳只有毫秒精度，两个"秒败"任务（spawn 前即报错的
/// 无 duration mp4）背靠背分配输出路径会落进同一毫秒 → 同路径 → 一方的失败
/// 清理删掉另一方的成品。时间戳后拼进程内单调序号消除该窗口。
static OUTPUT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "desktop")]
use ffmpeg_sidecar::child::FfmpegChild;
#[cfg(feature = "desktop")]
use ffmpeg_sidecar::event::FfmpegEvent;

use crate::media::{MediaFile, MediaType};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "desktop")]
use std::sync::atomic::Ordering;

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

impl Status {
    /// 行内徽标文案：不把 Rust 变体名（`SizeExcess` 等）直接抛给用户。
    pub fn label(&self) -> &'static str {
        match self {
            Status::Probing => "Probing",
            Status::Pending => "Pending",
            Status::Processing => "Transcoding",
            Status::Done => "Done",
            Status::Alert => "Failed",
            Status::SizeExcess => "Over limit",
        }
    }
}

/// 转码引擎：sidecar（ffmpeg 子进程）/ inprocess（libav 进程内）为桌面双轨；
/// webcodecs（浏览器原生编解码）与 ffmpeg-wasm（Route A）为网页端。
/// 设置直接存本类型（kebab-case 序列化值与原字符串逐字一致），不再是
/// "字符串 + 四层校验"——非法值在反序列化期就进不来。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    Sidecar,
    Inprocess,
    Webcodecs,
    FfmpegWasm,
}

impl Default for Engine {
    fn default() -> Self {
        Self::Sidecar
    }
}

impl Engine {
    /// 桌面设置下拉的 String → Engine 映射（选项固定，非法即兜底）。
    /// wasm 无引擎 UI（矩阵即策略），此函数仅桌面与测试消费。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sidecar" => Some(Self::Sidecar),
            "inprocess" => Some(Self::Inprocess),
            "webcodecs" => Some(Self::Webcodecs),
            "ffmpeg-wasm" => Some(Self::FfmpegWasm),
            _ => None,
        }
    }

    /// wasm 分发表按字符串匹配引擎（`job.engine`）时用；桌面已全程类型化。
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
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
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // 桌面单测覆盖矩阵
    pub fn for_web(
        media_type: &MediaType,
        webcodecs_ok: bool,
    ) -> (Engine, &'static str, &'static str) {
        use crate::media::{MediaType::*, VideoType::*};
        match media_type {
            Video(v) if matches!(v, Gif | Apng) => (Engine::FfmpegWasm, "video", v.pix_fmt()),
            Video(v) if webcodecs_ok && matches!(v, Mp4) => {
                (Engine::Webcodecs, "video", v.pix_fmt())
            }
            Video(v) => (Engine::FfmpegWasm, "video", v.pix_fmt()),
            Image(_) => (Engine::Webcodecs, "image", "yuva420p"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Transcoder {
    pub media_file: MediaFile,
    pub size_factor: Option<f64>,
    pub output_size: Option<u64>,
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
    /// 是否在封装后给 webm 打时长补丁（设置 `webm_duration_patch`，默认 true）。
    pub duration_patch: bool,
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
            duration_factors: command::DEFAULT_DURATION_FACTORS,
            target_fps: 0.0,
            output_bytes: None,
            duration_patch: true,
        }
    }

    /// 同步执行探测（读编码/时长），供后台线程调用。
    /// wasm：media_file.probe() 直接 Ok（见 media.rs 平台分支）。
    pub fn probe(&mut self) -> Result<(), String> {
        self.media_file.probe()
    }

    pub fn set_output<P: AsRef<Path> + ?Sized>(&mut self, output: &P) {
        self.media_file.set_output(output.as_ref().to_path_buf());
    }

    pub fn get_output(&self) -> Option<&PathBuf> {
        self.media_file.output()
    }

    /// 为任务分配输出文件（视频 .webm / 图片 .png）。
    /// `keep_input_name` 为真时前缀输入文件名主干：`输入名-时间戳.webm`（默认
    /// 只有时间戳，与历史行为一致）。
    pub fn set_output_dir<P: AsRef<Path> + ?Sized>(
        &mut self,
        output_dir: &P,
        keep_input_name: bool,
    ) -> Result<(), TranscodeError> {
        let ext = match self
            .media_file
            .r#type()
            .ok_or(TranscodeError::InvalidMediaType)?
        {
            MediaType::Video(_) => "webm",
            MediaType::Image(_) => "png",
        };
        // 输入名可能很长（Windows MAX_PATH 260）：截断，唯一性交给时间戳
        let prefix = if keep_input_name {
            self.media_file
                .file_stem()
                .map(|stem| {
                    let stem: String = stem.chars().take(MAX_NAME_STEM).collect();
                    format!("{stem}-")
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        // chrono::Local 依赖 std::time（wasm 未实现）——网页端用 JS Date 的 ISO 时间戳
        #[cfg(not(target_arch = "wasm32"))]
        let time = chrono::Local::now();
        let seq = OUTPUT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let output = {
            #[cfg(not(target_arch = "wasm32"))]
            {
                output_dir.as_ref().join(format!(
                    "{prefix}{}-{seq}.{ext}",
                    time.format("%Y-%m-%d-%H%M%S%.3f")
                ))
            }
            #[cfg(target_arch = "wasm32")]
            {
                // ISO: "2026-09-11T12:34:56.789Z" → "2026-09-11-12-34-56.789"
                // （尾 Z 不参与替换：替成 "-" 会留下悬空分隔符）
                let d = js_sys::Date::new(&js_sys::Date::now().into());
                let iso = d.to_iso_string().as_string().unwrap_or_default();
                let mut s = iso.trim_end_matches('Z').replace(['T', ':'], "-");
                if s.is_empty() {
                    // ISO 串异常时不能用 ".webm" 这种隐藏文件名——退回毫秒时间戳
                    s = (js_sys::Date::now() as u64).to_string();
                }
                output_dir.as_ref().join(format!("{prefix}{s}-{seq}.{ext}"))
            }
        };
        self.set_output(&output);
        Ok(())
    }

    /// 读 ffmpeg stderr 事件直到 EOF，回报进度，并保证取消可达。
    ///
    /// 刻意不用 `child.iter()`：那个迭代器把 `&mut FfmpegChild` 借走整个循环，
    /// 于是卡住的 ffmpeg（无声输入、无进度行）就永远 kill 不到——Cancel 变成
    /// 空操作，worker 还持着 Transcoder 锁不放。这里自持 stderr，事件经 crate
    /// 公开的 `spawn_stderr_thread` 以有界 sync channel 回传（见下方容量注释），
    /// 按 100ms 节拍轮询 cancel_flag。
    #[cfg(feature = "desktop")]
    fn pump_events(
        &self,
        process: &mut FfmpegChild,
        duration: f64,
        on_progress: &mut impl FnMut(f32),
    ) -> Result<(), TranscodeError> {
        use std::sync::mpsc::RecvTimeoutError;

        const POLL: std::time::Duration = std::time::Duration::from_millis(100);
        let stderr = process.take_stderr().ok_or(TranscodeError::ReadOutput)?;
        // 有界(8)而非 0：反压不需要零缓冲——bound=0 时解析线程每产一行事件
        // （log 行远多于进度行）都要与本循环完成一次阻塞式线程会合，纯属
        // 上下文切换开销。8 行的余量吸收突发，满了才退回会合。
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let reader = ffmpeg_sidecar::iter::spawn_stderr_thread(stderr, tx);
        let mut outcome = Ok(());
        loop {
            match rx.recv_timeout(POLL) {
                Ok(FfmpegEvent::LogEOF) => break,
                Ok(FfmpegEvent::Progress(p)) => {
                    let pct = command::parse_progress_time(&p.time) / duration;
                    on_progress(pct.clamp(0.0, 1.0) as f32);
                }
                Ok(_) => {}
                // 解析线程消失 == stderr 到底
                Err(RecvTimeoutError::Disconnected) => break,
                // 100ms 没输出也照样查取消（引擎卡住时唯一能中断的时刻）
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.cancel_flag.load(Ordering::Relaxed) {
                let _ = process.kill();
                let _ = process.wait(); // 回收子进程（crate 无 Drop）
                outcome = Err(TranscodeError::Cancelled);
                break;
            }
        }
        drop(rx); // 解除 reader 线程在 send 上的阻塞
        let _ = reader.join();
        outcome
    }

    /// 等子进程退出：100ms 轮询 `try_wait`，取消经 `cancel_flag` 可达。
    /// 裸 `wait()` 在"管道已关、进程仍活"的异常 ffmpeg 上会永久阻塞——
    /// 没人再消费 cancel_flag，Cancel 变空操作、转码锁被 worker 长期持有。
    /// stderr 若还没被 `pump_events` 取走（图片路径），先放线程排入 sink：
    /// 管满会让 ffmpeg 卡在写端，退出码永远等不到。
    #[cfg(feature = "desktop")]
    pub(super) fn wait_cancellable(
        &self,
        process: &mut FfmpegChild,
    ) -> Result<std::process::ExitStatus, TranscodeError> {
        const POLL: std::time::Duration = std::time::Duration::from_millis(100);
        if let Some(mut stderr) = process.take_stderr() {
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            });
        }
        loop {
            if self.cancel_flag.load(Ordering::Relaxed) {
                let _ = process.kill();
                let _ = process.wait(); // kill 后立即返回；回收子进程（crate 无 Drop）
                return Err(TranscodeError::Cancelled);
            }
            match process.as_inner_mut().try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => return Err(TranscodeError::ReadOutput),
            }
        }
    }

    /// 执行转码（按 engine 设置分发 sidecar / inprocess）；进度经回调上报（0..=1）。
    /// 取消经 `cancel_flag` 中断。
    ///
    /// 桌面专属：web 端两段式路径（`prepare_web_job` → JS 引擎 → `finish_web_job`）
    /// 在 runner 的 wasm 分支里分发，不经过本函数。
    #[cfg(feature = "desktop")]
    pub fn run_with_progress(
        &mut self,
        engine: Engine,
        mut on_progress: impl FnMut(f32),
    ) -> Result<(), TranscodeError> {
        if matches!(engine, Engine::Webcodecs | Engine::FfmpegWasm) {
            // 网页端引擎（分发在 runner.rs wasm 两段式路径）；桌面设置这两个值
            // → 明确 Alert（resolve_engine 只兜底 webcodecs，ffmpeg-wasm 漏到这）
            let _ = &mut on_progress;
            return Err(TranscodeError::UnsupportedEngine(
                "web engines (webcodecs / ffmpeg-wasm) require the web build",
            ));
        }
        // 函数整体已按 desktop feature gate，体内不再重复 cfg
        if engine == Engine::Inprocess {
            return self.run_inprocess(on_progress);
        }
        // sidecar 分支（ffmpeg 子进程）
        // 上次取消遗留的标志必须清掉，否则同一任务再次 Run 会立即被"取消"
        self.cancel_flag.store(false, Ordering::Relaxed);
        let media_type = self
            .media_file
            .r#type()
            .ok_or(TranscodeError::InvalidMediaType)?;
        let mut command = self.gen_command()?;
        let mut process = command.spawn().map_err(|_| TranscodeError::Spawn)?;
        match media_type {
            MediaType::Video(v_type) => {
                // 图片路径用 stdout 传 PNG，视频走 stderr 解析的进度事件。
                // 分母与码率同一时长源（APNG=1.0）；pct 钳到 [0,1] 与
                // inprocess 一致——0.001s 地板会把进度条冲出千位
                let duration = self.effective_duration(&v_type)?;
                self.pump_events(&mut process, duration, &mut on_progress)?;
                // stderr EOF ≠ 成功：中途死掉的 ffmpeg 头部已含 Duration，
                // 不查退出码会把无 trailer 坏文件判成 Done。
                let status = self.wait_cancellable(&mut process)?;
                if !status.success() {
                    return Err(TranscodeError::FfmpegFailed(status.to_string()));
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

    /// 收尾产物落位：Path 源写盘（桌面现状），Bytes 源写内存（网页端契约）。
    /// 两条路径都同步 output_size 供尺寸重试判定。
    pub fn store_output(&mut self, bytes: Vec<u8>) -> Result<(), TranscodeError> {
        if self.media_file.is_bytes() {
            self.output_size = Some(bytes.len() as u64);
            self.output_bytes = Some(bytes);
            return Ok(());
        }
        #[cfg(feature = "desktop")]
        {
            let path = self.get_output().ok_or(TranscodeError::OutputNotSet)?;
            std::fs::write(path, &bytes).map_err(|e| TranscodeError::OutputWrite(e.to_string()))?;
            Ok(())
        }
        #[cfg(not(feature = "desktop"))]
        {
            Err(TranscodeError::OutputNotSet)
        }
    }

    /// 转码完成后读取输出文件大小（供尺寸重试判定）。
    pub fn check_size(&mut self) -> Result<(), TranscodeError> {
        let size = if let Some(bytes) = &self.output_bytes {
            bytes.len() as u64
        } else {
            self.get_output()
                .ok_or(TranscodeError::OutputNotSet)?
                .metadata()
                .map_err(|e| TranscodeError::SizeCheck(e.to_string()))?
                .len()
        };
        self.output_size = Some(size);
        Ok(())
    }
}

/// 超限重试的系数缩放：factor / excess * shrink（runner 在重试决策时使用）。
pub fn shrunk_factor(current: f64, excess: f64, shrink: f64) -> f64 {
    current / excess * shrink
}

/// 输出大小相对上限的倍率（>1.0 即超限）。镜像侧（`TaskEntry`）与 Transcoder 侧
/// （runner 重试决策）共用这一条公式——两边各写一遍曾出现过分母漂移。
pub fn excess_ratio(output_size: Option<u64>, limit: u64) -> Option<f64> {
    Some(output_size? as f64 / limit as f64)
}

#[cfg(test)]
mod tests {
    use super::{Transcoder, shrunk_factor};

    /// 产物命名契约（"保留输入文件名"设置）：默认纯时间戳，开启后前缀输入名主干。
    #[test]
    fn output_name_keeps_input_stem_only_when_enabled() {
        use crate::media::MediaFile;

        let mut t = Transcoder::new(MediaFile::new(std::path::Path::new("input/clip.mp4")));
        t.set_output_dir(std::path::Path::new("out"), false)
            .unwrap();
        let plain = t
            .get_output()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(plain.ends_with(".webm"), "{plain}");
        assert!(!plain.starts_with("clip-"), "默认不该带输入名: {plain}");

        let mut t = Transcoder::new(MediaFile::new(std::path::Path::new("input/clip.mp4")));
        t.set_output_dir(std::path::Path::new("out"), true).unwrap();
        let kept = t
            .get_output()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(kept.starts_with("clip-"), "应保留输入名: {kept}");
        assert!(kept.ends_with(".webm"), "{kept}");
    }

    /// 输入名主干截到 64 字符（Windows 路径上限的粗保护）；图片产物是 .png。
    #[test]
    fn output_name_truncates_long_input_stem() {
        use crate::media::MediaFile;

        let long = format!("input/{}.png", "x".repeat(200));
        let mut t = Transcoder::new(MediaFile::new(std::path::Path::new(&long)));
        t.set_output_dir(std::path::Path::new("out"), true).unwrap();
        let name = t
            .get_output()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(name.matches('x').count(), 64, "{name}");
        assert!(name.ends_with(".png"), "{name}");
    }

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
        // 动画 webp 不会走到 web（桌面 probe 专属变体）——pix_fmt 与 gen_command
        // 同一出处，故为 alpha 格式而非旧的硬编码 yuv420p10
        assert_eq!(
            Engine::for_web(&MediaType::Video(VideoType::AnimatedWebP), true),
            (Engine::FfmpegWasm, "video", "yuva420p")
        );
    }
}
