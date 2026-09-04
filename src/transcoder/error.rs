//! 转码错误类型（E1）：替代贯穿各层的 `&str` 错误。

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
    #[error("cancelled")]
    Cancelled,
    #[error("duration patch failed: {0}")]
    DurationPatch(&'static str),
    #[error("image pipeline failed: {0}")]
    ImagePipe(&'static str),
    #[error("size check failed: {0}")]
    SizeCheck(String),
    #[error("join error: {0}")]
    Join(String),
    #[error("encoder not found: {0}")]
    EncoderNotFound(&'static str),
    #[error("decoder error: {0}")]
    Decoder(String),
    #[error("filter graph error: {0}")]
    Filter(String),
    #[error("muxer error: {0}")]
    Muxer(String),
}
