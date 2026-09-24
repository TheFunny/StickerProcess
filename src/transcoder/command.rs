//! ffmpeg 命令生成（`Transcoder::gen_command`）与码率/系数纯函数。
//!
//! 纯函数独立出来以便脱离 ffmpeg 进程单测（E6）。

use super::{TranscodeError, Transcoder};
#[cfg(feature = "desktop")]
use crate::media::MediaType;
use crate::media::VideoType;
#[cfg(feature = "desktop")]
use ffmpeg_sidecar::command::FfmpegCommand;

/// 视频码率基准（bps）：256 KB 贴纸换算为比特 / 时长。
const BITRATE_BASE_BYTES: f64 = 256.0 * 1024.0;

/// VP9 恒定质量模式的目标质量（越低质量越高、体积越大）。
#[cfg(feature = "desktop")]
pub(super) const VP9_CRF: u32 = 26;

/// `-bufsize` 与 `-b:v` 的比值（解码器缓冲时长，越大码率越平稳）。
#[cfg(feature = "desktop")]
pub(super) const BUFSIZE_RATIO: f64 = 1.5;

/// 缩放滤镜规格：桌面两引擎（sidecar CLI / inprocess 滤镜图）唯一出处。
/// web 端的 scale 规格在 JS 胶水里（浏览器侧没有 ffmpeg 滤镜串）。
#[cfg(feature = "desktop")]
pub(super) const SCALE_FILTER: &str =
    "scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos";

/// 时长→默认系数表（设置面板的出厂值，也是 `Transcoder::new` 的初值）。
pub const DEFAULT_DURATION_FACTORS: [f64; 6] = [1.2, 1.1, 1.0, 0.9, 0.8, 0.7];

/// 目标码率 = 贴纸比特数 / 视频时长。
pub(super) fn target_bitrate_bps(duration: f64) -> f64 {
    BITRATE_BASE_BYTES * 8.0 / duration
}

/// 时长→默认系数查表，区间固定：<1s, <2s, <3s, <5s, <8s, ≥8s。
pub fn default_factor(duration: f64, table: &[f64; 6]) -> f64 {
    let [f1, f2, f3, f5, f8, f8p] = *table;
    match duration {
        ..1f64 => f1,
        ..2f64 => f2,
        ..3f64 => f3,
        ..5f64 => f5,
        ..8f64 => f8,
        8f64.. => f8p,
        _ => 1.0, // NaN 等无效值（与旧实现一致）
    }
}

/// 码率 × 系数后量化到 10 的倍数（ffmpeg `-b:v` 取整数）。
pub(super) fn quantized_bitrate(base_bps: f64, factor: f64) -> u32 {
    (base_bps * factor) as u32 / 10 * 10
}

/// 把 ffmpeg 进度行的 `time`（如 `00:03:29.04`）解析为秒数；解析失败返回 0。
/// 桌面专属：进度行来自 sidecar 的 stderr，web 端进度由 JS 直接给比例。
#[cfg(feature = "desktop")]
pub fn parse_progress_time(time: &str) -> f64 {
    let mut seconds = 0f64;
    for part in time.trim().split(':') {
        let value = part.parse::<f64>().unwrap_or(0.0);
        seconds = seconds * 60.0 + value;
    }
    seconds
}
impl Transcoder {
    /// 实际参与码率计算的时长（秒）：APNG 固定 1.0（无可靠探测值），
    /// 其余视频用探测时长；<=0 视为无效。
    pub(super) fn effective_duration(&self, v_type: &VideoType) -> Result<f64, TranscodeError> {
        if *v_type == VideoType::Apng {
            return Ok(1.0);
        }
        let duration = self
            .media_file
            .duration()
            .ok_or(TranscodeError::InvalidDuration)?;
        if duration <= 0.0 {
            return Err(TranscodeError::InvalidDuration);
        }
        Ok(duration)
    }

    /// 求值码率系数：惰性初始化默认系数（查表）+ GIF 0.75 patch（程序内固定值，非设置项）。
    pub(super) fn resolve_factor(&mut self, duration: f64, v_type: &VideoType) -> f64 {
        let mut factor = match self.size_factor {
            Some(factor) => factor,
            None => {
                let factor = default_factor(duration, &self.duration_factors);
                self.size_factor = Some(factor);
                factor
            }
        };
        if let VideoType::Gif = v_type {
            factor *= 0.75;
        }
        factor
    }

    /// 视频码率链（三引擎唯一出处）：effective_duration → resolve_factor →
    /// quantized_bitrate。返回 (参与计算的时长, b:v)——inprocess 进度也要时长。
    /// AGENTS 不变量：sidecar / inprocess / web 必须码率一致，改这里，别复制。
    pub(super) fn video_bitrate(
        &mut self,
        v_type: &VideoType,
    ) -> Result<(f64, u32), TranscodeError> {
        let duration = self.effective_duration(v_type)?;
        let factor = self.resolve_factor(duration, v_type);
        Ok((
            duration,
            quantized_bitrate(target_bitrate_bps(duration), factor),
        ))
    }
}

#[cfg(feature = "desktop")]
impl Transcoder {
    pub(super) fn gen_command(&mut self) -> Result<FfmpegCommand, TranscodeError> {
        let mut command = FfmpegCommand::new();
        command
            .input(
                self.media_file
                    .path()
                    .ok_or(TranscodeError::InvalidOutputPath)?
                    .to_string_lossy(),
            )
            .filter(SCALE_FILTER)
            .overwrite();
        if let MediaType::Video(v_type) = self
            .media_file
            .r#type()
            .ok_or(TranscodeError::InvalidMediaType)?
        {
            let (_, target_bitrate) = self.video_bitrate(&v_type)?;
            if self.target_fps > 0.0 {
                command.args(["-r", &self.target_fps.to_string()]);
            }
            command
                .no_audio()
                .codec_video("libvpx-vp9")
                .pix_fmt(v_type.pix_fmt())
                .crf(VP9_CRF)
                .args(["-b:v", &target_bitrate.to_string()])
                .args([
                    "-bufsize",
                    &(target_bitrate as f64 * BUFSIZE_RATIO).to_string(),
                ])
                .args(["-row-mt", "1"])
                .format("webm")
                .output(
                    self.media_file
                        .output()
                        .and_then(|p| p.to_str())
                        .ok_or(TranscodeError::InvalidOutputPath)?,
                );
        } else {
            command.codec_video("png").format("image2").pipe_stdout();
        };
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_TABLE: [f64; 6] = [1.2, 1.1, 1.0, 0.9, 0.8, 0.7];

    #[test]
    fn factor_table_band_edges() {
        // 左闭右开区间：边界值落在更高一档
        assert_eq!(default_factor(0.999, &DEFAULT_TABLE), 1.2);
        assert_eq!(default_factor(1.0, &DEFAULT_TABLE), 1.1);
        assert_eq!(default_factor(2.0, &DEFAULT_TABLE), 1.0);
        assert_eq!(default_factor(3.0, &DEFAULT_TABLE), 0.9);
        assert_eq!(default_factor(5.0, &DEFAULT_TABLE), 0.8);
        assert_eq!(default_factor(8.0, &DEFAULT_TABLE), 0.7);
        assert_eq!(default_factor(60.0, &DEFAULT_TABLE), 0.7);
    }

    #[test]
    fn factor_table_uses_custom_values() {
        let table = [2.0, 2.0, 2.0, 2.0, 2.0, 2.0];
        assert_eq!(default_factor(0.5, &table), 2.0);
    }

    #[test]
    fn bitrate_base_matches_sticker_budget() {
        // 256KB*8/1s = 2097152 bps
        assert_eq!(target_bitrate_bps(1.0), 2_097_152.0);
        assert!((target_bitrate_bps(2.0) - 1_048_576.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bitrate_quantized_to_tens() {
        // 2097152 * 1.0 → as u32 截断后再取 10 的倍数
        assert_eq!(quantized_bitrate(2_097_152.0, 1.0), 2_097_150);
        assert_eq!(quantized_bitrate(100.0, 1.0), 100);
        assert_eq!(quantized_bitrate(105.0, 1.0), 100);
    }

    #[test]
    fn progress_time_parses() {
        assert!((parse_progress_time("00:00:01.00") - 1.0).abs() < 1e-9);
        assert!((parse_progress_time("00:03:29.04") - 209.04).abs() < 1e-9);
        assert_eq!(parse_progress_time("garbage"), 0.0);
    }

    #[test]
    fn web_gif_bitrate_math() {
        use crate::media::MediaFile;
        let mut t = Transcoder::new(MediaFile::from_bytes(vec![1, 2, 3], "a.gif".into()));
        t.media_file.set_duration(2.0);
        // 直接断言三引擎唯一出处本身——旧版手工串联 effective_duration →
        // resolve_factor → quantized_bitrate，链的接线/顺序改坏测试照样绿
        let (duration, bitrate) = t.video_bitrate(&VideoType::Gif).unwrap();
        assert!((duration - 2.0).abs() < f64::EPSILON);
        // duration=2.0 落 <3s 档（因子 1.0）→ 256KB*8/2s * 1.0 * 0.75(gif) = 786432
        // 量化到 10 的倍数 → 786430
        assert_eq!(bitrate, 786_430);
    }

    /// 链的出口：真正进 ffmpeg 命令的 -b:v / -bufsize（= b:v × 1.5）必须与
    /// video_bitrate 的结果一致——链算对但没接进命令，旧测试发现不了。
    #[test]
    #[cfg(feature = "desktop")]
    fn gen_command_emits_bitrate_and_bufsize() {
        use crate::media::{MediaFile, MediaType};
        use std::path::Path;
        // Path 源（gen_command 要 input path）+ 已分配输出（要 output path）
        let mut t = Transcoder::new(MediaFile::new(Path::new("a.mp4")));
        t.media_file.set_duration(2.0);
        t.set_output("out/a.webm");
        let MediaType::Video(v_type) = t.media_file.r#type().expect("mp4 types as video") else {
            panic!("mp4 must be a video type");
        };
        // 先跑一次码率链固定 size_factor；gen_command 内部重跑会复用同一因子
        let (_, bv) = t.video_bitrate(&v_type).unwrap();
        let cmd = t.gen_command().unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let pos = args.iter().position(|a| a == "-b:v").expect("-b:v present");
        assert_eq!(args[pos + 1], bv.to_string());
        let pos = args
            .iter()
            .position(|a| a == "-bufsize")
            .expect("-bufsize present");
        assert_eq!(args[pos + 1], (bv as f64 * BUFSIZE_RATIO).to_string());
    }
}
