use crate::media::{ImageType, MediaFile, MediaType, StickerType, VideoType};
use ffmpeg_sidecar::{child::FfmpegChild, command::FfmpegCommand};
use ffmpeg_the_third as ffmpeg;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

impl MediaFile {
    pub fn check_input(&mut self) -> Result<(), ffmpeg::Error> {
        let ictx = ffmpeg::format::input(self.path())?;

        let duration = ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        ffmpeg::format::context::input::dump(&ictx, 0, Some(&*self.path_str()));

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
                            self.path().display()
                        );
                        self.set_type(match id {
                            ffmpeg::codec::id::Id::PNG => MediaType::Image(ImageType::Png),
                            ffmpeg::codec::id::Id::MJPEG => MediaType::Image(ImageType::Jpg),
                            ffmpeg::codec::id::Id::WEBP => MediaType::Image(ImageType::Webp),
                            _ => unreachable!(),
                        })
                    }
                }
            }
            _ => match self.r#type().ok_or(ffmpeg::Error::InvalidData)? {
                MediaType::Image(_) => {
                    log::warn!(
                        "{} is an image file, but it contains a video codec",
                        self.path().display()
                    );
                    self.set_type(match id {
                        ffmpeg::codec::id::Id::GIF => MediaType::Video(VideoType::Gif),
                        ffmpeg::codec::id::Id::APNG => MediaType::Video(VideoType::Apng),
                        _ => MediaType::Video(VideoType::Mp4),
                    })
                }
                MediaType::Video(_) => {}
            },
        };
        if duration > 0f64 {
            self.set_duration(duration);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Factor {
    // default: f64,
    value: f64,
}

impl Factor {
    pub fn new(value: f64) -> Self {
        Self {
            // default: value,
            value,
        }
    }

    pub fn set(&mut self, value: f64) {
        self.value = value;
    }

    pub fn get(&self) -> f64 {
        self.value
    }
}

#[derive(Debug, Clone)]
pub struct FileSize {
    pub size: u64,
}

impl FileSize {
    pub fn new(size: u64) -> Self {
        Self { size }
    }

    pub fn set(&mut self, size: u64) {
        self.size = size;
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Status {
    /// 已入队，等待探测（Phase C: 添加时异步探测时长/编码）。
    Probing,
    Pending,
    Processing,
    Done,
    Alert,
    SizeExcess,
}

#[derive(Debug, Clone)]
pub struct Transcoder {
    pub media_file: MediaFile,
    pub size_factor: Option<Factor>,
    pub output_size: Option<FileSize>,
    pub status: Status,
    /// 置位后中断正在运行的 ffmpeg 进程（runner 经 Arc 桥接 UI 的 cancel 信号）。
    pub cancel_flag: Arc<AtomicBool>,
}

impl Transcoder {
    pub fn new(media_file: MediaFile) -> Self {
        // 构造时不做 IO 探测（仅按扩展名分类），check_input 延后到后台线程
        Self {
            media_file,
            size_factor: None,
            output_size: None,
            status: Status::Probing,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn probe(&mut self) -> Result<(), ffmpeg::Error> {
        self.media_file.check_input()
    }

    // pub fn get_status(&self) -> Status {
    //     self.status.clone()
    // }

    pub fn set_output<P: AsRef<Path> + ?Sized>(&mut self, output: &P) -> Result<(), &str> {
        self.media_file.set_output(output.as_ref().to_path_buf());
        Ok(())
    }

    pub fn get_output(&self) -> Option<&PathBuf> {
        self.media_file.output()
    }

    pub fn set_output_dir<P: AsRef<Path> + ?Sized>(&mut self, output_dir: &P) -> Result<(), &str> {
        let ext = match self.media_file.r#type().ok_or("Invalid media type")? {
            MediaType::Video(_) => "webm",
            MediaType::Image(_) => "png",
        };
        let time = chrono::Local::now();
        let output =
            output_dir
                .as_ref()
                .join(format!("{}.{}", time.format("%Y-%m-%d-%H%M%S%.3f"), ext));
        self.set_output(&output)
    }

    pub fn run_with_progress(&mut self, mut on_progress: impl FnMut(f32)) -> Result<(), &str> {
        // 上次取消遗留的标志必须清掉，否则同一任务再次 Run 会立即被"取消"
        self.cancel_flag.store(false, Ordering::Relaxed);
        let media_type = self.media_file.r#type().ok_or("Invalid media type")?;
        let mut command = self
            .gen_command()
            .map_err(|_e| "Failed to generate command")?;
        let mut process = command.spawn().map_err(|_| "Failed to run transcoder")?;
        match media_type {
            MediaType::Video(_) => {
                // 图片路径用 stdout 传 PNG，视频走 stderr 解析的进度事件
                let duration = self.media_file.duration().unwrap_or(1.0).max(0.001);
                for event in process.iter().map_err(|_| "Error reading process output")? {
                    if self.cancel_flag.load(Ordering::Relaxed) {
                        let _ = process.kill();
                        return Err("Cancelled");
                    }
                    if let ffmpeg_sidecar::event::FfmpegEvent::Progress(p) = event {
                        on_progress((parse_progress_time(&p.time) / duration) as f32);
                    }
                }
                if self.cancel_flag.load(Ordering::Relaxed) {
                    let _ = process.kill();
                    return Err("Cancelled");
                }
                self.run_video()
            }
            MediaType::Image(_) => {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    return Err("Cancelled");
                }
                self.run_image(&mut process)
            }
        }
    }

    fn gen_command(&mut self) -> Result<FfmpegCommand, &str> {
        let mut command = FfmpegCommand::new();
        command
            .input(self.media_file.path_str())
            .filter("scale=512:512:force_original_aspect_ratio=decrease")
            .args(["-sws_flags", "lanczos"])
            .overwrite();
        if let MediaType::Video(v_type) = self.media_file.r#type().ok_or("Invalid media type")? {
            let duration: f64;
            if v_type == VideoType::Apng {
                duration = 1.;
            } else {
                duration = self.media_file.duration().ok_or("Invalid media duration")?;
                if duration <= 0f64 {
                    return Err("Invalid media duration");
                }
            }
            let target_bitrate = (256 * 1024 * 8) as f64 / duration;
            let mut factor = match self.size_factor.as_ref() {
                Some(factor) => factor.get(),
                None => {
                    let factor = match duration {
                        ..1f64 => 1.2,
                        ..2f64 => 1.1,
                        ..3f64 => 1.0,
                        ..5f64 => 0.9,
                        ..8f64 => 0.8,
                        8f64.. => 0.7,
                        _ => 1.0,
                    };
                    self.size_factor = Some(Factor::new(factor));
                    factor
                }
            };
            if let VideoType::Gif = v_type {
                factor *= 0.75;
            }
            let target_bitrate = (target_bitrate * factor) as u32 / 10 * 10;
            command
                .no_audio()
                .codec_video("libvpx-vp9")
                .pix_fmt(match v_type {
                    VideoType::Mp4 => "yuv420p10",
                    VideoType::Gif | VideoType::Apng => "yuva420p",
                })
                // .rate(30.)
                .crf(26)
                .args(["-b:v", &target_bitrate.to_string()])
                .args(["-bufsize", &(target_bitrate as f64 * 1.5).to_string()])
                .args(["-row-mt", "1"])
                // .args(["-deadline", "best"])
                .format("webm")
                .output(
                    self.media_file
                        .output()
                        .and_then(|p| p.to_str())
                        .ok_or("Invalid output path")?,
                );
        } else {
            command.codec_video("png").format("image2").pipe_stdout();
        };
        Ok(command)
    }

    fn run_video(&mut self) -> Result<(), &str> {
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
            .open(self.get_output().ok_or("Output not set")?)
            .map_err(|_e| "Failed to open output file")?;
        let sequence = &[0x44, 0x89, 0x88];
        let position = find_binary_sequence_in_file(&file, sequence)
            .map_err(|_e| "Error finding sequence")?
            .ok_or("Binary sequence not found")?;
        file.seek_write(&100f64.to_be_bytes(), position + sequence.len() as u64)
            .map(|_| Ok(()))
            .map_err(|_e| "Error writing file")?
    }

    fn run_image(&mut self, process: &mut FfmpegChild) -> Result<(), &str> {
        let std_out = process.take_stdout().ok_or("Invalid stdout")?;
        let mut reader = BufReader::new(std_out);
        let mut buffer = Vec::new();
        reader
            .read_to_end(&mut buffer)
            .map_err(|_| "Read stdout failed")?;
        let mut option = oxipng::Options::from_preset(4);
        option.strip = oxipng::StripChunks::Safe;
        option.optimize_alpha = true;
        let buffer =
            oxipng::optimize_from_memory(&buffer, &option).map_err(|_| "Error optimizing image")?;
        let file = File::create(self.get_output().ok_or("Output not set")?)
            .map_err(|_e| "Error creating file")?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&buffer).map_err(|_| "Error writing file")
    }

    pub fn check_size(&mut self) -> Result<&FileSize, &str> {
        let metadata = self
            .get_output()
            .ok_or("Output not set")?
            .metadata()
            .map_err(|_e| "Error getting metadata")?;
        match self.output_size {
            Some(ref mut size) => size.set(metadata.len()),
            None => self.output_size = Some(FileSize::new(metadata.len())),
        }
        Ok(self.output_size.as_ref().unwrap())
    }
}

/// 把 ffmpeg 进度行的 `time`（如 `00:03:29.04`）解析为秒数；解析失败返回 0。
fn parse_progress_time(time: &str) -> f64 {
    let mut seconds = 0f64;
    for part in time.trim().split(':') {
        let value = part.parse::<f64>().unwrap_or(0.0);
        seconds = seconds * 60.0 + value;
    }
    seconds
}
