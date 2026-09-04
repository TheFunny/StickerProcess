//! 转码的两个收尾步骤：webm 时长补丁与图片管道（oxipng）。

use super::{TranscodeError, Transcoder};
use ffmpeg_sidecar::child::FfmpegChild;
use std::io::BufReader;
use std::io::Read;

impl Transcoder {
    /// webm 时长补丁：定位二进制标记 `44 89 88`，其后 8 字节覆写为
    /// `100f64`（大端）强制贴纸时长。
    pub(super) fn run_video(&mut self) -> Result<(), TranscodeError> {
        let path = self
            .get_output()
            .ok_or(TranscodeError::OutputNotSet)?
            .clone();
        let mut data = std::fs::read(&path)
            .map_err(|_| TranscodeError::DurationPatch("failed to open output file"))?;
        let position = data
            .windows(3)
            .position(|w| w == [0x44, 0x89, 0x88])
            .ok_or(TranscodeError::DurationPatch("binary sequence not found"))?;
        let end = position + 3 + 8;
        if end > data.len() {
            return Err(TranscodeError::DurationPatch(
                "binary sequence too close to EOF",
            ));
        }
        data[position + 3..end].copy_from_slice(&100f64.to_be_bytes());
        std::fs::write(&path, data)
            .map_err(|_| TranscodeError::DurationPatch("error writing file"))?;
        Ok(())
    }

    /// 图片管道（sidecar）：ffmpeg stdout PNG → 共享 oxipng 管道写盘。
    /// oxipng 优化逻辑在 inprocess.rs::write_optimized_png（两引擎共用）。
    pub(super) fn run_image(&mut self, process: &mut FfmpegChild) -> Result<(), TranscodeError> {
        let std_out = process
            .take_stdout()
            .ok_or(TranscodeError::ImagePipe("stdout not piped"))?;
        let mut reader = BufReader::new(std_out);
        let mut buffer = Vec::new();
        reader
            .read_to_end(&mut buffer)
            .map_err(|_| TranscodeError::ImagePipe("read stdout failed"))?;
        self.write_optimized_png(&buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaFile;
    use std::path::Path;

    /// 回归：标记跨 1024 字节块边界时旧实现偏移多算 last_bytes 长度。
    #[test]
    fn duration_patch_writes_at_marker_offset() {
        let dir = std::env::temp_dir().join(format!("stp-patch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.webm");
        let mut data = vec![0u8; 5000];
        data[2046..2049].copy_from_slice(&[0x44, 0x89, 0x88]); // 跨块边界
        std::fs::write(&path, &data).unwrap();

        let mut t = Transcoder::new(MediaFile::new(Path::new("dummy.mp4")));
        t.set_output(&path);
        t.run_video().unwrap();

        let patched = std::fs::read(&path).unwrap();
        assert_eq!(&patched[2049..2057], &100f64.to_be_bytes());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
