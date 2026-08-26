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

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub fn r#type(&self) -> Option<MediaType> {
        self.r#type.clone()
    }

    pub fn set_type(&mut self, r#type: MediaType) {
        self.r#type = Some(r#type);
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn set_duration(&mut self, duration: f64) {
        self.duration = Some(duration);
    }

    pub fn output(&self) -> Option<&PathBuf> {
        self.output.as_ref()
    }

    pub fn set_output(&mut self, output: PathBuf) {
        self.output = Some(output);
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
