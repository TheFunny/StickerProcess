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
    #[cfg(feature = "desktop")]
    #[error("invalid output path")]
    InvalidOutputPath,
    #[error("output not set")]
    OutputNotSet,
    #[error("cancelled")]
    Cancelled,
    #[error("duration patch failed: {0}")]
    DurationPatch(&'static str),
    #[error("size check failed: {0}")]
    SizeCheck(String),
    #[error("join error: {0}")]
    Join(String),
    #[error("unsupported engine: {0}")]
    UnsupportedEngine(&'static str),
    // ---- 桌面引擎专属（子进程 / libav / 写盘）：wasm 桥不会产生这些错误，
    //      按平台裁掉变体，两侧的 lint 才都是干净的（Host 侧单测仍覆盖它们）----
    #[cfg(feature = "desktop")]
    #[error("failed to spawn ffmpeg")]
    Spawn,
    #[cfg(feature = "desktop")]
    #[error("failed to read ffmpeg output")]
    ReadOutput,
    #[cfg(feature = "desktop")]
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),
    #[cfg(feature = "desktop")]
    #[error("image pipeline failed: {0}")]
    ImagePipe(&'static str),
    #[cfg(feature = "desktop")]
    #[error("failed to write output: {0}")]
    OutputWrite(String),
    #[cfg(feature = "desktop")]
    #[error("encoder not found: {0}")]
    EncoderNotFound(&'static str),
    #[cfg(feature = "desktop")]
    #[error("decoder error: {0}")]
    Decoder(String),
    #[cfg(feature = "desktop")]
    #[error("encoder error: {0}")]
    Encoder(String),
    #[cfg(feature = "desktop")]
    #[error("filter graph error: {0}")]
    Filter(String),
    #[cfg(feature = "desktop")]
    #[error("muxer error: {0}")]
    Muxer(String),
    /// 打开容器/编解码上下文阶段的 libav 错误（`format::input` 之类——那里的
    /// 失败本来就不是"解码错误"，标成 Decoder 是错的）。帧循环与封装阶段仍走
    /// 上方阶段变体，保留定位信息。
    #[cfg(feature = "desktop")]
    #[error("ffmpeg error: {0}")]
    Ffmpeg(ffmpeg::Error),
    /// 桌面上不会构造（只有 wasm 桥 `web.rs` 产生）：保住两端同一个枚举形状
    /// 比按平台裁剪变体更省事，故这里用 cfg。
    #[cfg(target_arch = "wasm32")]
    #[error("engine error: {0}")]
    Engine(String),
}

// `#[from]` 与变体上的 `#[cfg]` 组合不被 thiserror 接受，手写等价实现
#[cfg(feature = "desktop")]
impl From<ffmpeg::Error> for TranscodeError {
    fn from(e: ffmpeg::Error) -> Self {
        Self::Ffmpeg(e)
    }
}
