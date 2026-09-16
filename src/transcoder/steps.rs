//! 转码的两个收尾步骤：webm 时长补丁与图片管道（oxipng）。

use super::{TranscodeError, Transcoder};
#[cfg(feature = "desktop")]
use ffmpeg_sidecar::child::FfmpegChild;
#[cfg(feature = "desktop")]
use std::io::BufReader;
#[cfg(feature = "desktop")]
use std::io::Read;
#[cfg(feature = "desktop")]
use std::sync::atomic::Ordering;

impl Transcoder {
    /// webm 时长补丁（桌面 Path 源）：只读头部定位 Duration，原地覆写 8 字节。
    /// 旧实现读全文件 + 整写一遍——每次尝试为改 8 字节多一轮全量 I/O。
    #[cfg(feature = "desktop")]
    pub(super) fn run_video(&mut self) -> Result<(), TranscodeError> {
        let path = self
            .get_output()
            .ok_or(TranscodeError::OutputNotSet)?
            .clone();
        patch_webm_file(&path)
    }
}

/// 磁盘补丁（桌面 Path 源）：读全文件 → 定位 → 写回。
/// 文件上限就是贴纸大小（≤512KB），一次全量 I/O 是微秒级；原先的"8KB 头部
/// 窗口 + seek 原地写 + 整文件兜底"为省这点 I/O 维护了三条写路径与三种错误
/// 映射（还多一份重复的定位逻辑）。定位语义全在 `find_duration_payload`。
#[cfg(feature = "desktop")]
fn patch_webm_file(path: &std::path::Path) -> Result<(), TranscodeError> {
    let data = std::fs::read(path)
        .map_err(|_| TranscodeError::DurationPatch("failed to read output file"))?;
    let patched = patch_webm_bytes(data)?;
    std::fs::write(path, &patched).map_err(|_| TranscodeError::DurationPatch("error writing file"))
}

/// Duration 载荷（8 字节 f64）的起始偏移 = 标记位置 + 3。
/// 纯函数：桌面原地补丁与网页端 finish_web_job 共用。
///
/// 扫描止于首个 Cluster（ID `1F 43 B6 75`）：Duration 恒在 Info 内、先于
/// 任何 Cluster。不限界的搜索若 Duration 缺失/编码不同，会误中 VP9 载荷
/// 字节并静默改坏 8 字节视频数据。
fn find_duration_payload(data: &[u8]) -> Result<usize, TranscodeError> {
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
    Ok(position + 3)
}

/// 内存字节补丁（网页端 Bytes 源）。
pub(super) fn patch_webm_bytes(mut data: Vec<u8>) -> Result<Vec<u8>, TranscodeError> {
    let at = find_duration_payload(&data)?;
    data[at..at + 8].copy_from_slice(&100f64.to_be_bytes());
    Ok(data)
}

#[cfg(feature = "desktop")]
impl Transcoder {
    /// 图片管道（sidecar）：ffmpeg stdout PNG → 共享 oxipng 管道写盘。
    /// oxipng 优化逻辑在 inprocess.rs::write_optimized_png（两引擎共用）。
    pub(super) fn run_image(&mut self, process: &mut FfmpegChild) -> Result<(), TranscodeError> {
        use std::sync::mpsc::RecvTimeoutError;

        const POLL: std::time::Duration = std::time::Duration::from_millis(100);
        let std_out = process
            .take_stdout()
            .ok_or(TranscodeError::ImagePipe("stdout not piped"))?;
        // stdout 读到 EOF 可能要等 ffmpeg 退出：放独立线程读，主线程按拍轮询
        // cancel_flag（否则卡住的解码让 Cancel 同样失效）。
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(std_out);
            let mut buffer = Vec::new();
            let ok = reader.read_to_end(&mut buffer).is_ok();
            tx.send((buffer, ok)).ok();
        });
        let buffer = loop {
            match rx.recv_timeout(POLL) {
                Ok((buffer, true)) => break buffer,
                Ok((_, false)) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(TranscodeError::ImagePipe("read stdout failed"));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.cancel_flag.load(Ordering::Relaxed) {
                        let _ = process.kill();
                        let _ = process.wait(); // 回收子进程（crate 无 Drop）
                        return Err(TranscodeError::Cancelled);
                    }
                }
            }
        };
        // 与视频路径同一不变量：退出码才代表成功。解码失败时 stdout 可能只有
        // 半个 PNG，直接交给 oxipng 会把真因报成 "optimize failed"。
        let status = process.wait().map_err(|_| TranscodeError::ReadOutput)?;
        if !status.success() {
            return Err(TranscodeError::FfmpegFailed(status.to_string()));
        }
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

    /// 回归：原地补丁只改 8 字节载荷——文件长度与其余字节必须原封不动。
    #[test]
    #[cfg(feature = "desktop")]
    fn duration_patch_in_place_preserves_rest() {
        let mut data = vec![7u8; 300];
        data[10..13].copy_from_slice(&[0x44, 0x89, 0x88]);
        let path = std::env::temp_dir().join(format!("sp_patch_{}.webm", std::process::id()));
        std::fs::write(&path, &data).unwrap();
        patch_webm_file(&path).unwrap();
        let got = std::fs::read(&path).unwrap();
        assert_eq!(got.len(), data.len());
        assert_eq!(&got[13..21], &100f64.to_be_bytes());
        assert_eq!(&got[..10], &data[..10]);
        assert_eq!(&got[21..], &data[21..]);
        let _ = std::fs::remove_file(&path);
    }
}
