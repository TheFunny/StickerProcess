use ffmpeg_the_third as ffmpeg;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MediaFile {
    path: PathBuf,
    r#type: Option<MediaType>,
    duration: Option<f64>,
    output: Option<PathBuf>,
}

impl MediaFile {
    pub fn new(path: &Path) -> Self {
        let path = PathBuf::from(path);
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_ascii_lowercase());
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
            path,
            r#type,
            duration: None,
            output: None,
        }
    }

    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub fn r#type(&self) -> Option<MediaType> {
        self.r#type.clone()
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn output(&self) -> Option<&PathBuf> {
        self.output.as_ref()
    }

    pub fn set_output(&mut self, output: PathBuf) {
        self.output = Some(output);
    }

    /// 用 ffmpeg 探测真实编码与时长，并纠正按扩展名误判的类型
    /// （如视频容器装着图片编码）。供后台探测线程调用。
    pub fn probe(&mut self) -> Result<(), ffmpeg::Error> {
        let ictx = ffmpeg::format::input(&self.path)?;

        let duration = ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let ist = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;

        let decoder = ffmpeg::codec::context::Context::from_parameters(ist.parameters())?
            .decoder()
            .video()?;

        let id = decoder.id();

        match id {
            ffmpeg::codec::id::Id::PNG
            | ffmpeg::codec::id::Id::MJPEG
            | ffmpeg::codec::id::Id::WEBP => {
                // implement animated webp?
                match self.r#type().ok_or(ffmpeg::Error::InvalidData)? {
                    MediaType::Image(_) => {}
                    MediaType::Video(_) => {
                        log::warn!(
                            "{} is a video file, but it contains an image codec",
                            self.path.display()
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
                        self.path.display()
                    );
                    self.r#type = Some(match id {
                        ffmpeg::codec::id::Id::GIF => MediaType::Video(VideoType::Gif),
                        ffmpeg::codec::id::Id::APNG => MediaType::Video(VideoType::Apng),
                        _ => MediaType::Video(VideoType::Mp4),
                    });
                }
                MediaType::Video(_) => {}
            },
        };
        if duration > 0f64 {
            self.duration = Some(duration);
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
    use crate::app::{IMAGE, SUPPORTED, VIDEO};

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

    #[test]
    fn test_new_media_file_empty() {
        let media_file = MediaFile::new(Path::new("test"));
        assert_eq!(media_file.r#type, None);
    }

    /// AGENTS"四处同步"约定的保证：SUPPORTED 的每一项都必须能被识别，
    /// VIDEO/IMAGE 必须与 SUPPORTED 完全一致。
    #[test]
    fn supported_extensions_are_consistent() {
        assert!(
            VIDEO.iter().chain(IMAGE.iter()).eq(SUPPORTED.iter()),
            "VIDEO+IMAGE must equal SUPPORTED"
        );
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
}
