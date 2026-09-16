#[cfg(feature = "desktop")]
use ffmpeg_the_third as ffmpeg;
use std::path::{Path, PathBuf};

/// 媒体来源：桌面为文件路径；网页端为前端读入内存的字节 + 文件名。
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Path(PathBuf),
    /// 网页端：前端已读入内存的文件字节 + 原始文件名（扩展名判定用）
    Bytes {
        data: Vec<u8>,
        name: String,
    },
}

#[derive(Debug, Clone)]
pub struct MediaFile {
    source: Source,
    r#type: Option<MediaType>,
    duration: Option<f64>,
    output: Option<PathBuf>,
}

impl MediaFile {
    pub fn new(path: &Path) -> Self {
        Self::from_source(Source::Path(path.to_path_buf()))
    }

    /// 网页端：前端读入内存的字节 + 原始文件名（扩展名判定类型）。
    pub fn from_bytes(data: Vec<u8>, name: String) -> Self {
        Self::from_source(Source::Bytes { data, name })
    }

    fn from_source(source: Source) -> Self {
        let ext = Self::ext_of(&source).map(|s| s.to_ascii_lowercase());
        let r#type: Option<MediaType> = match ext.as_deref() {
            Some("mp4") => MediaType::Video(VideoType::Mp4).into(),
            Some("gif") => MediaType::Video(VideoType::Gif).into(),
            Some("jpg") | Some("jpeg") => MediaType::Image(ImageType::Jpg).into(),
            Some("png") => MediaType::Image(ImageType::Png).into(),
            Some("webp") => MediaType::Image(ImageType::Webp).into(),
            Some("apng") => MediaType::Video(VideoType::Apng).into(),
            _ => None,
        };
        Self {
            source,
            r#type,
            duration: None,
            output: None,
        }
    }

    /// 扩展名（小写前调用方处理）：Path 用 extension()，Bytes 用 name 的最后一个点后缀
    /// （无点 → None，与 Path 语义一致）。
    fn ext_of(source: &Source) -> Option<&str> {
        match source {
            Source::Path(p) => p.extension().and_then(|e| e.to_str()),
            Source::Bytes { name, .. } => name.rsplit_once('.').map(|(_, ext)| ext),
        }
    }

    /// 桌面专属：文件路径。Bytes 源返回 None（网页端无文件系统路径）。
    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::Path(p) => Some(p.as_path()),
            Source::Bytes { .. } => None,
        }
    }

    /// 展示名：Path 用完整路径，Bytes 用原始文件名。
    pub fn display_name(&self) -> String {
        match &self.source {
            Source::Path(p) => p.to_string_lossy().into_owned(),
            Source::Bytes { name, .. } => name.clone(),
        }
    }

    /// 输入文件名主干（不含扩展名）；取不到或为空返回 None。
    /// 产物命名（"保留输入文件名"设置）用。
    pub fn file_stem(&self) -> Option<String> {
        let stem = match &self.source {
            Source::Path(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()),
            Source::Bytes { name, .. } => name.rsplit_once('.').map(|(stem, _)| stem.to_string()),
        };
        stem.filter(|s| !s.is_empty())
    }

    /// 是否为内存字节源（网页端任务）。
    pub fn is_bytes(&self) -> bool {
        matches!(self.source, Source::Bytes { .. })
    }

    pub fn r#type(&self) -> Option<MediaType> {
        self.r#type.clone()
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    /// Bytes 源的字节引用；Path 源 None。
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.source {
            Source::Bytes { data, .. } => Some(data),
            Source::Path(_) => None,
        }
    }

    /// Bytes 源输入长度（桌面 metadata 等价物）；Path 源 None。
    pub fn input_len(&self) -> Option<u64> {
        match &self.source {
            Source::Bytes { data, .. } => Some(data.len() as u64),
            Source::Path(p) => std::fs::metadata(p).ok().map(|m| m.len()),
        }
    }

    /// 回填探测时长（网页端由 JS 元数据填充；桌面 probe_desktop 自动填）。
    pub fn set_duration(&mut self, duration: f64) {
        if duration > 0.0 {
            self.duration = Some(duration);
        }
    }

    /// 回填探测类型（网页端 APNG 检测：扩展名 png 但 ImageDecoder 报多帧）。
    pub fn set_type(&mut self, media_type: MediaType) {
        self.r#type = Some(media_type);
    }

    pub fn output(&self) -> Option<&PathBuf> {
        self.output.as_ref()
    }

    pub fn set_output(&mut self, output: PathBuf) {
        self.output = Some(output);
    }

    /// 用 ffmpeg 探测真实编码与时长，并纠正按扩展名误判的类型
    /// （如视频容器装着图片编码）。供后台探测线程调用。
    ///
    /// 错误统一成 `String`：两个平台各自的错误类型（`ffmpeg::Error` / 无错误）
    /// 的消费者都只是把它转成 UI 文案，原先的 cfg 双签名只在 app.rs 换来两组
    /// 一模一样语义的 map_err。Bytes 源（网页端）跳过 ffmpeg 探测：时长/类型
    /// 由前端传入，直接 Ok。
    pub fn probe(&mut self) -> Result<(), String> {
        #[cfg(feature = "desktop")]
        {
            if matches!(self.source, Source::Bytes { .. }) {
                return Ok(());
            }
            self.probe_desktop().map_err(|e| e.to_string())
        }
        #[cfg(not(feature = "desktop"))]
        {
            // wasm：无 ffmpeg。Bytes 源类型由前端判定（from_bytes），时长由
            // 前端元数据填充（app.rs 的 native_probe 回填）。
            Ok(())
        }
    }

    #[cfg(feature = "desktop")]
    fn probe_desktop(&mut self) -> Result<(), ffmpeg::Error> {
        let path = self.path().expect("probe on Bytes guarded above");
        let mut ictx = ffmpeg::format::input(path)?;

        let duration = ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let ist = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;

        let decoder = ffmpeg::codec::context::Context::from_parameters(ist.parameters())?
            .decoder()
            .video()?;

        let id = decoder.id();
        let anim_webp = id == ffmpeg::codec::id::Id::WEBP_ANIM;

        match id {
            ffmpeg::codec::id::Id::PNG
            | ffmpeg::codec::id::Id::MJPEG
            | ffmpeg::codec::id::Id::WEBP => {
                match self.r#type().ok_or(ffmpeg::Error::InvalidData)? {
                    MediaType::Image(_) => {}
                    MediaType::Video(_) => {
                        log::warn!(
                            "{} is a video file, but it contains an image codec",
                            self.display_name()
                        );
                        self.r#type = Some(match id {
                            ffmpeg::codec::id::Id::PNG => MediaType::Image(ImageType::Png),
                            ffmpeg::codec::id::Id::MJPEG => MediaType::Image(ImageType::Jpg),
                            ffmpeg::codec::id::Id::WEBP => MediaType::Image(ImageType::Webp),
                            _ => unreachable!(),
                        });
                    }
                }
            }
            _ => match self.r#type().ok_or(ffmpeg::Error::InvalidData)? {
                MediaType::Image(_) => {
                    log::warn!(
                        "{} is an image file, but it contains a video codec",
                        self.display_name()
                    );
                    self.r#type = Some(match id {
                        ffmpeg::codec::id::Id::GIF => MediaType::Video(VideoType::Gif),
                        ffmpeg::codec::id::Id::APNG => MediaType::Video(VideoType::Apng),
                        ffmpeg::codec::id::Id::WEBP_ANIM => {
                            MediaType::Video(VideoType::AnimatedWebP)
                        }
                        _ => MediaType::Video(VideoType::Mp4),
                    });
                }
                MediaType::Video(_) => {}
            },
        };
        if duration > 0f64 {
            self.duration = Some(duration);
        } else if anim_webp {
            // webp_anim 容器无 duration（ffprobe N/A）；累加 packet pts+duration
            // （stream tb 实测 1/1000），取末包上界即总时长。
            let mut end = 0f64;
            for (s, p) in ictx.packets().flatten() {
                let tb = f64::from(s.time_base());
                let d = (p.duration().max(0) as f64) * tb;
                end = end.max(p.pts().unwrap_or(0) as f64 * tb + d);
            }
            if end > 0.0 {
                self.duration = Some(end);
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum MediaType {
    Video(VideoType),
    Image(ImageType),
}

#[derive(Debug, PartialEq, Clone)]
pub enum VideoType {
    Mp4,
    Gif,
    Apng,
    /// 动画 webp（仅桌面：probe 把扩展名阶段的 Image(Webp) 纠正至此；
    /// 网页端无 probe 纠正路径，恒 Image(Webp) 输出首帧 PNG）。
    AnimatedWebP,
}

impl VideoType {
    /// 输出像素格式：三引擎（sidecar CLI / inprocess libav / web glue）唯一出处。
    /// `yuv420p10` 是 CLI 与 wasm 端都接受的写法（`descriptor().name()` 会给
    /// `yuv420p10le`，改名会连带改 JS 胶水）。
    pub fn pix_fmt(&self) -> &'static str {
        match self {
            VideoType::Mp4 => "yuv420p10",
            VideoType::Gif | VideoType::Apng | VideoType::AnimatedWebP => "yuva420p",
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum ImageType {
    Jpg,
    Png,
    Webp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SUPPORTED;

    #[test]
    fn test_new_media_file_mp4() {
        let media_file = MediaFile::new(Path::new("test.mp4"));
        assert_eq!(media_file.r#type, Some(MediaType::Video(VideoType::Mp4)));
    }

    #[test]
    fn test_new_media_file_jpg() {
        let media_file = MediaFile::new(Path::new("test.jpg"));
        assert_eq!(media_file.r#type, Some(MediaType::Image(ImageType::Jpg)));
    }

    #[test]
    fn test_new_media_file_gif() {
        let media_file = MediaFile::new(Path::new("test.gif"));
        assert_eq!(media_file.r#type, Some(MediaType::Video(VideoType::Gif)));
    }

    #[test]
    fn test_new_media_file_apng() {
        let media_file = MediaFile::new(Path::new("test.apng"));
        assert_eq!(media_file.r#type, Some(MediaType::Video(VideoType::Apng)));
    }

    /// 扩展名阶段 webp 恒为静态图：动画 webp 的识别发生在桌面 probe
    /// （codec id WEBP_ANIM → Video(AnimatedWebP)），此处锁住不误判。
    #[test]
    fn anim_webp_extension_stays_image_until_probe() {
        let media_file = MediaFile::new(Path::new("x.webp"));
        assert_eq!(media_file.r#type, Some(MediaType::Image(ImageType::Webp)));
    }

    #[test]
    fn test_new_media_file_empty() {
        let media_file = MediaFile::new(Path::new("test"));
        assert_eq!(media_file.r#type, None);
    }

    /// AGENTS「四处同步」约定的保证：SUPPORTED 的每一项都必须能被识别。
    #[test]
    fn supported_extensions_are_consistent() {
        for ext in SUPPORTED {
            let file = MediaFile::new(Path::new(&format!("file.{ext}")));
            assert!(
                file.r#type.is_some(),
                "SUPPORTED extension '{ext}' not recognized by MediaFile::new"
            );
        }
        // 反向：不在 SUPPORTED 里的已知扩展必须被拒绝
        for ext in ["webm", "bmp", "txt"] {
            let file = MediaFile::new(Path::new(&format!("file.{ext}")));
            assert!(file.r#type.is_none(), "'{ext}' should be rejected");
        }
    }

    #[test]
    fn from_bytes_type_detection() {
        let mp4 = MediaFile::from_bytes(vec![], "clip.mp4".into());
        assert_eq!(mp4.r#type, Some(MediaType::Video(VideoType::Mp4)));
        let gif = MediaFile::from_bytes(vec![], "anim.gif".into());
        assert_eq!(gif.r#type, Some(MediaType::Video(VideoType::Gif)));
        let png = MediaFile::from_bytes(vec![], "pic.png".into());
        assert_eq!(png.r#type, Some(MediaType::Image(ImageType::Png)));
        let unknown = MediaFile::from_bytes(vec![], "file.xyz".into());
        assert_eq!(unknown.r#type, None);
    }

    /// Bytes 契约：probe 直接 Ok（时长/类型由前端传入），path() 为 None。
    #[test]
    fn bytes_source_probe_ok_and_path_none() {
        let mut media = MediaFile::from_bytes(vec![], "a.gif".into());
        assert!(media.path().is_none());
        assert_eq!(media.display_name(), "a.gif");
        media.probe().unwrap();
    }

    #[test]
    fn path_source_still_works() {
        let media = MediaFile::new(Path::new("test.mp4"));
        assert!(media.path().is_some());
        assert_eq!(media.display_name(), "test.mp4");
    }
}
