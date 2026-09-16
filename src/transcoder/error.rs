//! 转码错误类型（E1）：替代贯穿各层的 `&str` 错误。

#[cfg(feature = "desktop")]
use ffmpeg_the_third as ffmpeg;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TranscodeError {
    #[error("invalid media type")]
    InvalidMediaType,
    #[error("invalid media duration")]
    InvalidDuration,
    #[error("invalid output path")]
    InvalidOutputPath,
    #[error("output not set")]
    OutputNotSet,
    #[error("failed to spawn ffmpeg")]
    Spawn,
    #[error("failed to read ffmpeg output")]
    ReadOutput,
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),
    #[error("cancelled")]
    Cancelled,
    #[error("duration patch failed: {0}")]
    DurationPatch(&'static str),
    #[error("image pipeline failed: {0}")]
    ImagePipe(&'static str),
    #[error("size check failed: {0}")]
    SizeCheck(String),
    #[error("failed to write output: {0}")]
    OutputWrite(String),
    #[error("engine error: {0}")]
    Engine(String),
    #[error("join error: {0}")]
    Join(String),
    /// 打开容器/编解码上下文阶段的 libav 错误（`format::input` 之类——那里的
    /// 失败本来就不是"解码错误"，标成 Decoder 是错的）。帧循环与封装阶段仍走
    /// 下方阶段变体，保留定位信息。
    #[cfg(feature = "desktop")]
    #[error("ffmpeg error: {0}")]
    Ffmpeg(ffmpeg::Error),
    #[error("encoder not found: {0}")]
    EncoderNotFound(&'static str),
    #[error("decoder error: {0}")]
    Decoder(String),
    #[error("encoder error: {0}")]
    Encoder(String),
    #[error("filter graph error: {0}")]
    Filter(String),
    #[error("muxer error: {0}")]
    Muxer(String),
    #[error("unsupported engine: {0}")]
    UnsupportedEngine(&'static str),
}

// `#[from]` 与变体上的 `#[cfg]` 组合不被 thiserror 接受，手写等价实现
#[cfg(feature = "desktop")]
impl From<ffmpeg::Error> for TranscodeError {
    fn from(e: ffmpeg::Error) -> Self {
        Self::Ffmpeg(e)
    }
}
