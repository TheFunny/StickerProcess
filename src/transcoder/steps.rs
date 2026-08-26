//! 转码的两个收尾步骤：webm 时长补丁与图片管道（oxipng）。

use super::{TranscodeError, Transcoder};
use ffmpeg_sidecar::child::FfmpegChild;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::windows::fs::FileExt;

impl Transcoder {
    /// webm 时长补丁：定位二进制标记 `44 89 88`，其后 8 字节覆写为
    /// `100f64`（大端）强制贴纸时长。
    pub(super) fn run_video(&mut self) -> Result<(), TranscodeError> {
        fn find_binary_sequence_in_file(
            mut file: &File,
            sequence: &[u8],
        ) -> std::io::Result<Option<u64>> {
            let mut buffer = [0u8; 1024];
            let mut position = 0;
            let mut last_bytes = Vec::new();

            loop {
                let bytes_read = file.read(&mut buffer)?;

                if bytes_read == 0 {
                    break; // 读取结束
                }

                // 将上次未完成的字节和当前缓冲区拼接
                let mut combined = last_bytes.clone();
                combined.extend_from_slice(&buffer[..bytes_read]);

                // 查找二进制序列
                if let Some(index) = combined
                    .windows(sequence.len())
                    .position(|window| window == sequence)
                {
                    return Ok(Some(position + index as u64));
                }

                // 保存当前缓冲区的最后部分以处理跨块情况
                if combined.len() > sequence.len() {
                    last_bytes = combined.split_off(combined.len() - sequence.len());
                } else {
                    last_bytes.clear();
                }

                position += bytes_read as u64;
            }

            Ok(None)
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.get_output().ok_or(TranscodeError::OutputNotSet)?)
            .map_err(|_| TranscodeError::DurationPatch("failed to open output file"))?;
        let sequence = &[0x44, 0x89, 0x88];
        let position = find_binary_sequence_in_file(&file, sequence)
            .map_err(|_| TranscodeError::DurationPatch("error finding sequence"))?
            .ok_or(TranscodeError::DurationPatch("binary sequence not found"))?;
        file.seek_write(&100f64.to_be_bytes(), position + sequence.len() as u64)
            .map_err(|_| TranscodeError::DurationPatch("error writing file"))?;
        Ok(())
    }

    /// 图片管道：ffmpeg 从 stdout 输出 PNG → oxipng 内存优化（preset 4）→ 写盘。
    pub(super) fn run_image(&mut self, process: &mut FfmpegChild) -> Result<(), TranscodeError> {
        let std_out = process
            .take_stdout()
            .ok_or(TranscodeError::ImagePipe("stdout not piped"))?;
        let mut reader = BufReader::new(std_out);
        let mut buffer = Vec::new();
        reader
            .read_to_end(&mut buffer)
            .map_err(|_| TranscodeError::ImagePipe("read stdout failed"))?;
        let mut option = oxipng::Options::from_preset(4);
        option.strip = oxipng::StripChunks::Safe;
        option.optimize_alpha = true;
        let buffer = oxipng::optimize_from_memory(&buffer, &option)
            .map_err(|_| TranscodeError::ImagePipe("optimize failed"))?;
        let file = File::create(self.get_output().ok_or(TranscodeError::OutputNotSet)?)
            .map_err(|_| TranscodeError::ImagePipe("create file failed"))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&buffer)
            .map_err(|_| TranscodeError::ImagePipe("write failed"))
    }
}
