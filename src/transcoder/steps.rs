//! 转码的两个收尾步骤：webm 时长补丁与图片管道（oxipng）。

use super::{TranscodeError, Transcoder};
#[cfg(feature = "desktop")]
use ffmpeg_sidecar::child::FfmpegChild;
#[cfg(feature = "desktop")]
use std::io::BufReader;
#[cfg(feature = "desktop")]
use std::io::Read;

impl Transcoder {
    /// webm 时长补丁：读盘 → patch_webm_bytes → store_output。
    pub(super) fn run_video(&mut self) -> Result<(), TranscodeError> {
        let path = self
            .get_output()
            .ok_or(TranscodeError::OutputNotSet)?
            .clone();
        let data = std::fs::read(&path)
            .map_err(|_| TranscodeError::DurationPatch("failed to open output file"))?;
        let patched = patch_webm_bytes(data)?;
        self.store_output(patched)
            .map_err(|_| TranscodeError::DurationPatch("error writing file"))?;
        Ok(())
    }
}

/// 定位 Duration 元素 `44 89 88`（EBML ID 0x4489 + 1 字节 size vint 0x88），
/// 其后 8 字节覆写为 `100f64`（大端）强制贴纸时长。
/// 纯函数：桌面 run_video 与网页端 finish_web_job 共用。
///
/// 扫描止于首个 Cluster（ID `1F 43 B6 75`）：Duration 恒在 Info 内、先于
/// 任何 Cluster。不限界的搜索若 Duration 缺失/编码不同，会误中 VP9 载荷
/// 字节并静默改坏 8 字节视频数据。
pub(super) fn patch_webm_bytes(mut data: Vec<u8>) -> Result<Vec<u8>, TranscodeError> {
    const MARKER: [u8; 3] = [0x44, 0x89, 0x88];
    const CLUSTER: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];
    let cluster_at = data
        .windows(4)
        .position(|w| w == CLUSTER)
        .unwrap_or(data.len());
    let position = data[..cluster_at]
        .windows(3)
        .position(|w| w == MARKER)
        .ok_or(TranscodeError::DurationPatch("binary sequence not found"))?;
    let end = position + 3 + 8;
    if end > cluster_at {
        return Err(TranscodeError::DurationPatch(
            "binary sequence too close to EOF",
        ));
    }
    data[position + 3..end].copy_from_slice(&100f64.to_be_bytes());
    Ok(data)
}

#[cfg(feature = "desktop")]
impl Transcoder {
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

    /// 回归：标记跨块边界时 patch_webm_bytes 的偏移正确（原 run_video 回归用例改指纯函数）。
    #[test]
    fn duration_patch_writes_at_marker_offset() {
        let mut data = vec![0u8; 5000];
        data[2046..2049].copy_from_slice(&[0x44, 0x89, 0x88]); // 跨 1024 块边界
        let patched = patch_webm_bytes(data).unwrap();
        assert_eq!(&patched[2049..2057], &100f64.to_be_bytes());
        // 前置内容不受影响
        assert_eq!(&patched[..3], &[0u8, 0, 0]);
    }

    #[test]
    fn duration_patch_rejects_marker_near_eof() {
        let mut data = vec![0u8; 10];
        data[7..10].copy_from_slice(&[0x44, 0x89, 0x88]); // 3+8 字节超出 EOF
        let err = patch_webm_bytes(data).unwrap_err();
        assert!(err.to_string().contains("too close to EOF"));
    }

    #[test]
    fn duration_patch_rejects_missing_marker() {
        let data = vec![0u8; 64];
        let err = patch_webm_bytes(data).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    /// 回归：Cluster 载荷里的伪标记不得被改写（误中会静默坏 8 字节视频数据）。
    #[test]
    fn duration_patch_refuses_marker_inside_cluster() {
        let mut data = vec![0u8; 200];
        data[10..14].copy_from_slice(&[0x1F, 0x43, 0xB6, 0x75]); // Cluster 在前
        data[50..53].copy_from_slice(&[0x44, 0x89, 0x88]); // 载荷里的伪标记
        let err = patch_webm_bytes(data).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
