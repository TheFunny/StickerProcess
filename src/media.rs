use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MediaFile {
    path: PathBuf,
    r#type: Option<MediaType>,
    sticker: Option<StickerType>,
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
            _ => None,
        };
        let sticker: Option<StickerType> = r#type.as_ref().map(|t| match t {
            MediaType::Video(_) => StickerType::Animated,
            MediaType::Image(_) => StickerType::Static,
        });
        Self {
            path,
            r#type,
            sticker,
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

    pub fn sticker(&self) -> Option<StickerType> {
        self.sticker.clone()
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
pub enum StickerType {
    Static,
    Animated,
}

#[derive(Debug, PartialEq, Clone)]
pub enum VideoType {
    Mp4,
    Gif,
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

    #[test]
    fn test_new_media_file_mp4() {
        let path = "test.mp4";
        let media_file = MediaFile::new(Path::new(&path));
        assert_eq!(media_file.r#type, Some(MediaType::Video(VideoType::Mp4)));
        assert_eq!(media_file.sticker, Some(StickerType::Animated));
    }

    #[test]
    fn test_new_media_file_jpg() {
        let path = "test.jpg";
        let media_file = MediaFile::new(Path::new(&path));
        assert_eq!(media_file.r#type, Some(MediaType::Image(ImageType::Jpg)));
        assert_eq!(media_file.sticker, Some(StickerType::Static));
    }

    #[test]
    fn test_new_media_file_gif() {
        let path = "test.gif";
        let media_file = MediaFile::new(Path::new(&path));
        assert_eq!(media_file.r#type, Some(MediaType::Video(VideoType::Gif)));
        assert_eq!(media_file.sticker, Some(StickerType::Animated));
    }

    #[test]
    fn test_new_media_file_empty() {
        let path = "test";
        let media_file = MediaFile::new(Path::new(&path));
        println!("Test with file \"{path}\": {:?}", media_file);
        assert_eq!(media_file.r#type, None);
        assert_eq!(media_file.sticker, None);
    }
}
