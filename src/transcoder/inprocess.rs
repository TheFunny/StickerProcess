//! 进程内转码（E6 第一阶段）：libav API 解码→滤镜→编码→封装。
//!
//! 与 sidecar 路径（`mod.rs::run_with_progress`）行为对齐：
//! - 码率/系数/CRF/bufsize 语义与 `gen_command` 逐字一致（共享
//!   `effective_duration` / `resolve_factor`）
//! - 滤镜规格与 CLI 字符串逐字一致（含 `flags=lanczos`）
//! - webm 时长补丁对两种路径同样生效（`run_video`）
//! - 取消经 `cancel_flag`：无需 kill 进程，Drop 链自动释放
//!
//! 时间基决策：整条管道统一 1/1000（毫秒）。解码后帧 pts 一次性重缩放到
//! 1/1000，编码器/输出流/滤镜 buffer 参数均以此为基准，mux 前无需再缩放。

use super::command::{BUFSIZE_RATIO, VP9_CRF, quantized_bitrate, target_bitrate_bps};
use super::{TranscodeError, Transcoder};
use crate::media::{MediaType, VideoType};
use ffmpeg_the_third as ffmpeg;
use ffmpeg_the_third::Rational;
use ffmpeg_the_third::codec::threading;
use ffmpeg_the_third::format::Pixel;
use ffmpeg_the_third::frame::video::Video as VideoFrame;
use ffmpeg_the_third::util::mathematics::Rescale;
use libc::EAGAIN;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;

/// 输出流时间基：webm muxer 惯例毫秒。
const OUT_TB: Rational = Rational(1, 1000);

/// libav 全局初始化（进程一次；失败也只试一次，后续报 Decoder 错误）。
static LIBAV_INIT: LazyLock<()> = LazyLock::new(|| {
    let _ = ffmpeg::init();
});

impl Transcoder {
    /// 进程内转码：解码→滤镜→编码→封装，帧循环内检查 cancel_flag。
    /// on_progress 以帧 pts / 输入时长上报（0..=1）。
    pub fn run_inprocess(
        &mut self,
        mut on_progress: impl FnMut(f32),
    ) -> Result<(), TranscodeError> {
        // 上次取消遗留的标志必须清掉（与 sidecar 入口一致）
        self.cancel_flag.store(false, Ordering::Relaxed);
        LazyLock::force(&LIBAV_INIT);
        let media_type = self
            .media_file
            .r#type()
            .ok_or(TranscodeError::InvalidMediaType)?;
        match media_type {
            MediaType::Video(v_type) => {
                self.run_inprocess_video(v_type, &mut on_progress)?;
                // 与 sidecar 路径一致：写盘后打时长补丁（第一阶段两种路径都生效）
                self.run_video()
            }
            MediaType::Image(_) => {
                let raw = self.encode_image_rgba()?;
                self.write_optimized_png(&raw)
            }
        }
    }

    /// 视频管道：解码 → 滤镜 → 编码（首帧开编码器）→ 写包 → drain → trailer。
    fn run_inprocess_video(
        &mut self,
        v_type: VideoType,
        on_progress: &mut impl FnMut(f32),
    ) -> Result<(), TranscodeError> {
        let duration = self.effective_duration(&v_type)?;
        let factor = self.resolve_factor(duration, &v_type);
        let target_bitrate = quantized_bitrate(target_bitrate_bps(duration), factor);
        let out_pix = match v_type {
            VideoType::Mp4 => Pixel::YUV420P10LE,
            VideoType::Gif | VideoType::Apng => Pixel::YUVA420P,
        };

        let mut ictx = ffmpeg::format::input(&self.media_file.path_str())
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?;
        let istream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(TranscodeError::Decoder("no video stream".into()))?;
        let stream_index = istream.index();

        // 输出流的平均帧率：强制 fps 优先，否则沿用源流（matroska 用它推
        // 导 track 默认块时长——缺失时最后一帧时长记 0，流 DURATION 标签
        // 会比 CLI 少一帧的时长）
        let fps = if self.target_fps > 0.0 {
            Some(fps_rational(self.target_fps))
        } else {
            let src_fps = istream.avg_frame_rate();
            (src_fps.numerator() > 0 && src_fps.denominator() > 0).then_some(src_fps)
        };
        let mut dec_ctx = ffmpeg::codec::Context::from_parameters(istream.parameters())
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?;
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
        dec_ctx.set_threading(threading::Config {
            kind: threading::Type::Frame,
            count: threads,
        });
        let mut decoder = dec_ctx
            .decoder()
            .video()
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?;

        // time_base：未 open 的 decoder 常为 0/1（无效），回退容器流 tb；
        // packet 统一重缩放到该基，解码输出帧 pts 即在此基下。
        let in_tb = {
            let tb = decoder.time_base();
            if tb.numerator() <= 0 || tb.denominator() <= 0 {
                istream.time_base()
            } else {
                tb
            }
        };
        let (in_pix, in_w, in_h) = (decoder.format(), decoder.width(), decoder.height());
        let mut sar = decoder.aspect_ratio();
        if sar.numerator() == 0 {
            sar = Rational(1, 1);
        }

        // 强制 fps 与 sidecar `-r` 语义对齐：fps 滤镜复制/丢帧到目标 CFR
        // （置于 scale 之后，保留 alpha 通道）；未强制时沿用源流帧率仅作元数据。
        let spec = if self.target_fps > 0.0 {
            format!(
                "scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos,fps={}",
                self.target_fps
            )
        } else {
            "scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos".to_string()
        };
        let mut graph = build_graph(in_w, in_h, in_pix, in_tb, sar, out_pix, &spec)?;

        // 编码器（open 推迟到首帧：宽高以滤镜输出为准；setter 都在 Video 包装上）
        let codec = ffmpeg::codec::encoder::find_by_name("libvpx-vp9")
            .ok_or(TranscodeError::EncoderNotFound("libvpx-vp9"))?;
        let enc_ctx = ffmpeg::codec::Context::new_with_codec(codec);
        let mut enc_video_opt = Some(
            enc_ctx
                .encoder()
                .video()
                .map_err(|e| TranscodeError::Decoder(e.to_string()))?,
        );
        // webm muxer 需要全局头（open 时生成 extradata 供 copy_parameters）
        enc_video_opt
            .as_mut()
            .unwrap()
            .set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
        enc_video_opt.as_mut().unwrap().set_time_base(OUT_TB);
        enc_video_opt
            .as_mut()
            .unwrap()
            .set_bit_rate(target_bitrate as usize);
        // CLI 语义对齐：ffmpeg.c 的 -b:v 只设 AVCodecContext.bit_rate（libvpx 的
        // rc_target_bitrate），rc_max_rate 保持 0（VBR）；-bufsize → rc_buffer_size。
        // 之前多设 rc_max_rate=b:v 迫使 libvpx 进入 CBR 式封顶，输出比 CLI 小 ~35%。
        let mut opts = Some(ffmpeg::Dictionary::new());
        opts.as_mut().unwrap().set("crf", VP9_CRF.to_string());
        opts.as_mut().unwrap().set("row-mt", "1");
        opts.as_mut().unwrap().set(
            "rc_buffer_size",
            &(target_bitrate as f64 * BUFSIZE_RATIO).to_string(),
        );

        let out_path = self
            .get_output()
            .ok_or(TranscodeError::OutputNotSet)?
            .clone();
        let mut octx = ffmpeg::format::output_as(&out_path, "webm")
            .map_err(|e| TranscodeError::Muxer(e.to_string()))?;

        let mut packet = ffmpeg::Packet::empty();
        let mut iframe = VideoFrame::empty();
        let mut oframe = VideoFrame::empty();
        let mut last_pts: Option<i64> = None;
        let mut enc: Option<ffmpeg::codec::encoder::video::Encoder> = None;

        // 收编码器包并写盘（enc 已 open）
        macro_rules! write_packets {
            ($enc:expr) => {
                loop {
                    match $enc.receive_packet(&mut packet) {
                        Ok(()) => {
                            packet.set_stream(0);
                            packet
                                .write_interleaved(&mut octx)
                                .map_err(|e| TranscodeError::Muxer(e.to_string()))?;
                        }
                        // VP9 delay：EOF 即收尾；EAGAIN 表示暂无包
                        Err(ffmpeg::Error::Eof) => break,
                        Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => break,
                        Err(e) => return Err(TranscodeError::Encoder(e.to_string())),
                    }
                }
            };
        }

        // 拉滤镜输出直到 EAGAIN/Eof，逐帧送编码器（首帧处 open + write_header）
        macro_rules! drain_filter {
            () => {
                loop {
                    if self.cancel_flag.load(Ordering::Relaxed) {
                        return Err(TranscodeError::Cancelled);
                    }
                    match sink_frame(&mut graph, &mut oframe) {
                        Ok(()) => {
                            if enc.is_none() {
                                let mut enc_video = enc_video_opt.take().unwrap();
                                enc_video.set_width(oframe.width());
                                enc_video.set_height(oframe.height());
                                enc_video.set_format(out_pix);
                                if let Some(fps) = fps {
                                    enc_video.set_frame_rate(Some(fps));
                                }
                                let opened = enc_video
                                    .open_with(opts.take().unwrap())
                                    .map_err(map_open_err)?;
                                // opened: video::Encoder(pub Video(pub Context))；先导出参数
                                let params = ffmpeg::codec::Parameters::from(&opened);
                                let mut ost = octx
                                    .add_stream(codec)
                                    .map_err(|e| TranscodeError::Muxer(e.to_string()))?;
                                ost.set_time_base(OUT_TB);
                                if let Some(fps) = fps {
                                    ost.set_avg_frame_rate(fps);
                                }
                                ost.set_parameters(params);
                                octx.write_header()
                                    .map_err(|e| TranscodeError::Muxer(e.to_string()))?;
                                enc = Some(opened);
                            }
                            // VFR/异常源可能出现 pts=None 或非单调的帧；libvpx
                            // 拿到 NOPTS/回退时间戳会产生损坏输出。统一走
                            // next_pts：重缩放并钳位为 last+1ms，保证严格递增。
                            let pts = next_pts(last_pts, oframe.pts(), in_tb, OUT_TB);
                            last_pts = Some(pts);
                            oframe.set_pts(Some(pts));
                            if duration > 0.0 {
                                let seconds = pts as f64 * OUT_TB.numerator() as f64
                                    / OUT_TB.denominator() as f64;
                                on_progress((seconds / duration).clamp(0.0, 1.0) as f32);
                            }
                            let encoder = enc.as_mut().unwrap();
                            encoder
                                .send_frame(&oframe)
                                .map_err(|e| TranscodeError::Encoder(e.to_string()))?;
                            write_packets!(encoder);
                        }
                        Err(ffmpeg::Error::Eof) => break,
                        Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => break,
                        Err(e) => return Err(TranscodeError::Filter(e.to_string())),
                    }
                }
            };
        }

        for res in ictx.packets() {
            let (stream, ipacket) = res.map_err(|e| TranscodeError::Decoder(e.to_string()))?;
            if stream.index() != stream_index {
                continue;
            }
            if self.cancel_flag.load(Ordering::Relaxed) {
                return Err(TranscodeError::Cancelled);
            }
            // packet pts 已在流 tb（in_tb）下，无需重缩放（decoder tb = stream tb）
            decoder
                .send_packet(&ipacket)
                .map_err(|e| TranscodeError::Decoder(e.to_string()))?;
            loop {
                match decoder.receive_frame(&mut iframe) {
                    Ok(()) => {
                        push_frame(&mut graph, &iframe)
                            .map_err(|e| TranscodeError::Filter(e.to_string()))?;
                        drain_filter!();
                    }
                    Err(ffmpeg::Error::Eof) => break,
                    Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => break,
                    Err(e) => return Err(TranscodeError::Decoder(e.to_string())),
                }
            }
        }

        // 解码器 drain → 滤镜 flush → 编码器 EOF flush（VP9 delay 靠 Eof 收尾）
        decoder
            .send_eof()
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?;
        loop {
            match decoder.receive_frame(&mut iframe) {
                Ok(()) => {
                    push_frame(&mut graph, &iframe)
                        .map_err(|e| TranscodeError::Filter(e.to_string()))?;
                    drain_filter!();
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => continue,
                Err(e) => return Err(TranscodeError::Decoder(e.to_string())),
            }
        }
        push_flush(&mut graph).map_err(|e| TranscodeError::Filter(e.to_string()))?;
        drain_filter!();
        if let Some(encoder) = enc.as_mut() {
            encoder
                .send_eof()
                .map_err(|e| TranscodeError::Encoder(e.to_string()))?;
            write_packets!(encoder);
        }
        octx.write_trailer()
            .map_err(|e| TranscodeError::Muxer(e.to_string()))?;
        Ok(())
    }

    /// 图片管道：解码首帧 → 滤镜(RGBA) → png 编码进内存 → oxipng → 写盘。
    fn encode_image_rgba(&mut self) -> Result<Vec<u8>, TranscodeError> {
        let mut ictx = ffmpeg::format::input(&self.media_file.path_str())
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?;
        let istream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(TranscodeError::Decoder("no video stream".into()))?;
        let stream_index = istream.index();
        let mut decoder = ffmpeg::codec::Context::from_parameters(istream.parameters())
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?
            .decoder()
            .video()
            .map_err(|e| TranscodeError::Decoder(e.to_string()))?;

        let in_tb = {
            let tb = decoder.time_base();
            if tb.numerator() <= 0 || tb.denominator() <= 0 {
                istream.time_base()
            } else {
                tb
            }
        };
        let (in_pix, in_w, in_h) = (decoder.format(), decoder.width(), decoder.height());
        let mut sar = decoder.aspect_ratio();
        if sar.numerator() == 0 {
            sar = Rational(1, 1);
        }
        let mut graph = build_graph(
            in_w,
            in_h,
            in_pix,
            in_tb,
            sar,
            Pixel::RGBA,
            "scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos",
        )?;

        let mut iframe = VideoFrame::empty();
        let mut oframe = VideoFrame::empty();
        // 首个解码帧入滤镜
        let mut got = false;
        'decode: for res in ictx.packets() {
            let (stream, ipacket) = res.map_err(|e| TranscodeError::Decoder(e.to_string()))?;
            if stream.index() != stream_index {
                continue;
            }
            if self.cancel_flag.load(Ordering::Relaxed) {
                return Err(TranscodeError::Cancelled);
            }
            decoder
                .send_packet(&ipacket)
                .map_err(|e| TranscodeError::Decoder(e.to_string()))?;
            loop {
                match decoder.receive_frame(&mut iframe) {
                    Ok(()) => {
                        push_frame(&mut graph, &iframe)
                            .map_err(|e| TranscodeError::Filter(e.to_string()))?;
                        got = true;
                        break 'decode;
                    }
                    Err(ffmpeg::Error::Eof) => break 'decode,
                    Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => break,
                    Err(e) => return Err(TranscodeError::Decoder(e.to_string())),
                }
            }
        }
        if !got {
            return Err(TranscodeError::Decoder("no frame decoded".into()));
        }

        // 滤镜 flush 后取输出帧（静态图单帧）
        push_flush(&mut graph).map_err(|e| TranscodeError::Filter(e.to_string()))?;
        let mut out_frame = None;
        loop {
            match sink_frame(&mut graph, &mut oframe) {
                Ok(()) => out_frame = Some(oframe.clone()),
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => break,
                Err(e) => return Err(TranscodeError::Filter(e.to_string())),
            }
        }
        let out_frame = out_frame.ok_or(TranscodeError::Filter("no filtered frame".into()))?;
        // png 编码进内存（PNG 裸流即文件格式，无需 muxer；alpha 对不透明源无害）
        let codec = ffmpeg::codec::encoder::find_by_name("png")
            .ok_or(TranscodeError::EncoderNotFound("png"))?;
        let enc_ctx = ffmpeg::codec::Context::new_with_codec(codec);
        let mut enc = enc_ctx
            .encoder()
            .video()
            .map_err(|e| TranscodeError::Encoder(e.to_string()))?;
        enc.set_width(out_frame.width());
        enc.set_height(out_frame.height());
        enc.set_format(Pixel::RGBA);
        enc.set_time_base(OUT_TB);
        let mut encoder = enc
            .open()
            .map_err(|e| TranscodeError::Encoder(e.to_string()))?;
        encoder
            .send_frame(&out_frame)
            .map_err(|e| TranscodeError::Encoder(e.to_string()))?;
        encoder
            .send_eof()
            .map_err(|e| TranscodeError::Encoder(e.to_string()))?;
        let mut pkt = ffmpeg::Packet::empty();
        let mut png = Vec::new();
        loop {
            match encoder.receive_packet(&mut pkt) {
                Ok(()) => {
                    if let Some(data) = pkt.data() {
                        png.extend_from_slice(data);
                    }
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => continue,
                Err(e) => return Err(TranscodeError::Encoder(e.to_string())),
            }
        }
        Ok(png)
    }

    /// oxipng 内存优化（preset 4）→ 写盘。sidecar `run_image` 同样复用。
    pub(super) fn write_optimized_png(&mut self, raw: &[u8]) -> Result<(), TranscodeError> {
        let mut option = oxipng::Options::from_preset(4);
        option.strip = oxipng::StripChunks::Safe;
        option.optimize_alpha = true;
        let optimized = oxipng::optimize_from_memory(raw, &option)
            .map_err(|_| TranscodeError::ImagePipe("optimize failed"))?;
        let file = std::fs::File::create(self.get_output().ok_or(TranscodeError::OutputNotSet)?)
            .map_err(|_| TranscodeError::ImagePipe("create file failed"))?;
        let mut writer = std::io::BufWriter::new(file);
        std::io::Write::write_all(&mut writer, &optimized)
            .map_err(|_| TranscodeError::ImagePipe("write failed"))
    }
}

/// 组装 scale 滤镜图：buffer → <spec> → format=<out_pix> → buffersink。
/// 中间滤镜经 Parser 挂接手工 add 的 buffer/buffersink（the-third 惯用法）。
fn build_graph(
    in_w: u32,
    in_h: u32,
    in_pix: Pixel,
    in_tb: Rational,
    in_sar: Rational,
    out_pix: Pixel,
    spec: &str,
) -> Result<ffmpeg::filter::Graph, TranscodeError> {
    let mut graph = ffmpeg::filter::Graph::new();
    let buffer = ffmpeg::filter::find("buffer")
        .ok_or(TranscodeError::Filter("filter 'buffer' not found".into()))?;
    let in_pix_name = in_pix.descriptor().map_or("none", |d| d.name());
    graph
        .add(
            &buffer,
            "in",
            &format!(
                "video_size={in_w}x{in_h}:pix_fmt={in_pix_name}:time_base={}/{}:pixel_aspect={}/{}",
                in_tb.numerator(),
                in_tb.denominator(),
                in_sar.numerator(),
                in_sar.denominator(),
            ),
        )
        .map_err(|e| TranscodeError::Filter(e.to_string()))?;
    let buffersink = ffmpeg::filter::find("buffersink").ok_or(TranscodeError::Filter(
        "filter 'buffersink' not found".into(),
    ))?;
    graph
        .add(&buffersink, "out", "")
        .map_err(|e| TranscodeError::Filter(e.to_string()))?;
    // buffersink 的 pix_fmts 选项限定输出像素格式
    graph.get("out").unwrap().set_pixel_format(out_pix);
    // spec 挂到 "in" 的输出 pad 与 "out" 的输入 pad 之间
    let out_pix_name = out_pix.descriptor().map_or("none", |d| d.name());
    graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(&format!("{spec},format={out_pix_name}")))
        .map_err(|e| TranscodeError::Filter(e.to_string()))?;
    graph
        .validate()
        .map_err(|e| TranscodeError::Filter(e.to_string()))?;
    Ok(graph)
}

/// 送一帧进滤镜 source（每次重新 get，借用模型不允许长期持有 Source）。
fn push_frame(graph: &mut ffmpeg::filter::Graph, frame: &VideoFrame) -> Result<(), ffmpeg::Error> {
    graph.get("in").unwrap().source().add(frame)
}

/// 结束滤镜输入（av_buffersrc_add_frame(null)）。
fn push_flush(graph: &mut ffmpeg::filter::Graph) -> Result<(), ffmpeg::Error> {
    graph.get("in").unwrap().source().flush()
}

/// 从滤镜图拉一帧（每次重新 get）。
fn sink_frame(
    graph: &mut ffmpeg::filter::Graph,
    frame: &mut VideoFrame,
) -> Result<(), ffmpeg::Error> {
    graph.get("out").unwrap().sink().frame(frame)
}

/// 编码器 open 失败映射：EncoderNotFound 单列（本机无 libvpx 时显式报错，
/// 不静默回退 sidecar），其余归 Encoder。
fn map_open_err(e: ffmpeg::Error) -> TranscodeError {
    if matches!(e, ffmpeg::Error::EncoderNotFound) {
        TranscodeError::EncoderNotFound("libvpx-vp9")
    } else {
        TranscodeError::Encoder(e.to_string())
    }
}

/// fps → Rational（毫秒精度）。
fn fps_rational(fps: f64) -> Rational {
    Rational((fps * 1000.0).round() as i32, 1000)
}

/// 下一帧输出 pts：重缩放到输出时间基，并钳位为严格递增（last+1ms）。
/// None-pts 帧回退 last+1（首帧 0）。VFR/异常源的时间戳损坏防线。
fn next_pts(last: Option<i64>, pts: Option<i64>, from: Rational, to: Rational) -> i64 {
    match pts {
        Some(p) => {
            let scaled = p.rescale(from, to);
            last.map_or(scaled, |lp| scaled.max(lp + 1))
        }
        None => last.map_or(0, |lp| lp + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_rational_millisecond_precision() {
        assert_eq!(fps_rational(25.0), Rational(25_000, 1000));
        assert_eq!(fps_rational(29.97), Rational(29_970, 1000));
    }
    #[test]
    fn next_pts_monotonic_and_rescale() {
        let tb_in = Rational(1, 90000);
        // 正常重缩放：90000 基下 9000 → 100ms
        assert_eq!(next_pts(None, Some(9000), tb_in, OUT_TB), 100);
        // 回退 pts：None → last+1
        assert_eq!(next_pts(Some(100), None, tb_in, OUT_TB), 101);
        // 首帧 None-pts → 0
        assert_eq!(next_pts(None, None, tb_in, OUT_TB), 0);
        // 回退/重复帧钳位为 last+1（严格递增）
        assert_eq!(next_pts(Some(100), Some(90), tb_in, OUT_TB), 101);
        assert_eq!(next_pts(Some(100), Some(100), tb_in, OUT_TB), 101);
    }

    /// libav 集成冒烟：需 FFMPEG_DIR/PATH 环境，手动 `cargo test -- --ignored`。
    /// 不做大小/时长断言——A/B 对比由人工完成。
    #[test]
    #[ignore]
    fn inprocess_video_smoke() {
        let input = std::env::var("SMOKE_INPUT").unwrap_or_else(|_| "input/1.mp4".into());
        let mut t = Transcoder::new(crate::media::MediaFile::new(std::path::Path::new(&input)));
        t.probe().unwrap();
        t.set_output(&format!("out/_e6_smoke_{}.webm", std::process::id()));
        t.run_inprocess(|_| {}).unwrap();
        let size = std::fs::metadata(t.get_output().unwrap()).unwrap().len();
        assert!(size > 0);
    }

    #[test]
    #[ignore]
    fn inprocess_gif_smoke() {
        let mut t = Transcoder::new(crate::media::MediaFile::new(std::path::Path::new(
            "input/1.gif",
        )));
        t.probe().unwrap();
        t.set_output(&format!("out/_e6_gif_{}.webm", std::process::id()));
        t.run_inprocess(|_| {}).unwrap();
        let size = std::fs::metadata(t.get_output().unwrap()).unwrap().len();
        assert!(size > 0);
    }

    #[test]
    #[ignore]
    fn inprocess_image_smoke() {
        let mut t = Transcoder::new(crate::media::MediaFile::new(std::path::Path::new(
            "input/2024-04-10-315.png",
        )));
        t.probe().unwrap();
        t.set_output(&format!("out/_e6_img_{}.png", std::process::id()));
        t.run_inprocess(|_| {}).unwrap();
        let size = std::fs::metadata(t.get_output().unwrap()).unwrap().len();
        assert!(size > 0);
    }

    /// 图片任务永远没有 size_factor（resolve_factor 只对视频惰性初始化）；
    /// runner 依赖该不变量跳过图片超限的空转重试。
    #[test]
    #[ignore]
    fn image_has_no_size_factor_after_transcode() {
        let mut t = Transcoder::new(crate::media::MediaFile::new(std::path::Path::new(
            "input/2024-04-10-315.png",
        )));
        t.probe().unwrap();
        t.set_output(&format!("out/_e6_imgf_{}.png", std::process::id()));
        t.run_inprocess(|_| {}).unwrap();
        t.check_size().unwrap();
        assert!(t.size_factor.is_none());
    }

    /// Force FPS 双引擎帧数对齐：sidecar `-r` 与 inprocess `fps` 滤镜应产出
    /// 相同帧数（用 ffprobe 计数；无 ffprobe 时跳过）。需 ffmpeg CLI 在 PATH。
    #[test]
    #[ignore]
    fn force_fps_frame_count_matches_sidecar() {
        let ffprobe = |path: &str| -> Option<u64> {
            let out = std::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "csv=p=0",
                    path,
                ])
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        };
        let run = |engine: &str, out: String| -> String {
            let mut t = Transcoder::new(crate::media::MediaFile::new(std::path::Path::new(
                "input/1.mp4",
            )));
            t.probe().unwrap();
            t.target_fps = 15.0;
            t.set_output(&out);
            t.run_with_progress(engine, |_| {}).unwrap();
            out
        };
        let a = run(
            "inprocess",
            format!("out/_fps_inproc_{}.webm", std::process::id()),
        );
        let b = run(
            "sidecar",
            format!("out/_fps_side_{}.webm", std::process::id()),
        );
        let (fa, fb) = (ffprobe(&a), ffprobe(&b));
        let _ = (std::fs::remove_file(&a), std::fs::remove_file(&b));
        match (fa, fb) {
            (Some(fa), Some(fb)) => {
                println!("force_fps frames: inprocess={fa} sidecar={fb}");
                assert_eq!(fa, fb, "frame count mismatch: inprocess={fa} sidecar={fb}")
            }
            _ => eprintln!("ffprobe unavailable, skipped frame count assert"),
        }
    }
}
