//! 网页端引擎桥（Route A: ffmpeg.wasm；Route B: WebCodecs）。
//!
//! 两段式执行避免 MutexGuard 跨 await：
//! 1. 锁内 `prepare_web_job`（校验类型/求码率/选引擎/克隆字节）
//! 2. await `exec_ffmpeg_wasm` / `exec_webcodecs`（JS 胶水跑引擎，进度经回调上报）
//! 3. 锁内 `finish_web_job`（webm 时长补丁 → store_output）
//!
//! JS 侧契约：`assets/ffmpeg-engine.js`（stickerFfmpeg*）与
//! `assets/webcodecs-engine.js`（stickerWebcodecs* + stickerNativeProbe）。

use super::{TranscodeError, Transcoder, steps};
use crate::media::{MediaType, VideoType};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = window)]
extern "C" {
    // `catch`：JS 侧异常转 Result 而非 wasm trap（glue 未就绪时调用不炸上下文）
    #[wasm_bindgen(catch, js_name = stickerFfmpegReady)]
    fn sticker_ffmpeg_ready() -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(catch, js_name = stickerFfmpegTranscode)]
    fn sticker_ffmpeg_transcode(
        data: &[u8],
        name: &str,
        bitrate: u32,
        fps: f64,
        pix_fmt: &str,
        on_progress: &Closure<dyn FnMut(f32)>,
    ) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(catch, js_name = stickerWebcodecsProbeSupport)]
    fn sticker_webcodecs_probe_support() -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(catch, js_name = stickerWebcodecsTranscode)]
    fn sticker_webcodecs_transcode(
        data: &[u8],
        name: &str,
        bitrate: u32,
        fps: f64,
        kind: &str,
        on_progress: &Closure<dyn FnMut(f32)>,
    ) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(catch, js_name = stickerNativeProbe)]
    fn sticker_native_probe(name: &str, data: &[u8]) -> Result<js_sys::Promise, JsValue>;
    /// 公开给组件层（task_list 下载）；定义在 webcodecs-engine.js（每任务探测
    /// 必经）。`catch`：glue 缺失/JS 异常转 Err 而非 trap 炸应用上下文。
    #[wasm_bindgen(catch, js_name = stickerDownload)]
    pub fn sticker_download(data: &[u8], filename: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = stickerFfmpegCancel)]
    fn sticker_ffmpeg_cancel();
    #[wasm_bindgen(js_name = stickerWebcodecsCancel)]
    fn sticker_webcodecs_cancel();
}

/// 已解出锁的任务参数（锁不跨 await）。engine/kind 决定运行期分发，
/// pix_fmt 仅 ffmpeg-wasm 分支消费（MP4=10-bit，GIF/APNG=alpha）。
pub struct WebJob {
    pub data: Vec<u8>,
    pub name: String,
    pub bitrate: u32,
    pub fps: f64,
    pub engine: &'static str,
    pub kind: &'static str,
    pub pix_fmt: &'static str,
}

impl Transcoder {
    /// 阶段 1（锁内，同步）：校验类型、按媒体类型 + WebCodecs caps 选引擎
    /// （矩阵见 `Engine::for_web`）、求 duration/因子/bitrate（复用
    /// effective_duration + resolve_factor——因子惰性初始化与 GIF×0.75 patch
    /// 与桌面完全一致）、克隆输入字节。
    // ponytail: 克隆一次输入字节（典型 ≤5MB）换锁不跨 await；内存实测吃紧改 Rc/Bytes 共享
    pub fn prepare_web_job(&mut self, webcodecs_ok: bool) -> Result<WebJob, TranscodeError> {
        let media_type = self
            .media_file
            .r#type()
            .ok_or(TranscodeError::InvalidMediaType)?;
        let (engine, kind, pix_fmt) = super::Engine::for_web(&media_type, webcodecs_ok);
        let engine = engine.as_str();
        let bitrate = match media_type {
            MediaType::Video(v) => self.video_bitrate(&v)?,
            MediaType::Image(_) => 0,
        };
        let data = match self.media_file.bytes() {
            Some(bytes) => bytes.to_vec(),
            None => return Err(TranscodeError::InvalidMediaType),
        };
        let name = self.media_file.display_name();
        Ok(WebJob {
            data,
            name,
            bitrate,
            fps: self.target_fps,
            engine,
            kind,
            pix_fmt,
        })
    }

    /// 视频码率：目标字节 × 8 / 时长 × 因子（与桌面 gen_command 同一公式链）。
    fn video_bitrate(&mut self, v_type: &VideoType) -> Result<u32, TranscodeError> {
        let duration = self.effective_duration(v_type)?;
        let factor = self.resolve_factor(duration, v_type);
        Ok(super::command::quantized_bitrate(
            super::command::target_bitrate_bps(duration),
            factor,
        ))
    }

    /// 阶段 2（锁内，同步）：视频走 webm 时长补丁，图片（PNG）原样
    /// → store_output（Bytes 源写内存）。
    // ponytail: web 图片不跑 oxipng——libdeflate-sys 需 wasm C 工具链（clang）；
    // 浏览器 convertToBlob 的 PNG 已合法且贴纸远小于 512KB 上限。web 图片若超限
    // 再接 wasm 版 oxipng 或后端压缩。
    pub fn finish_web_job(&mut self, out: Vec<u8>) -> Result<(), TranscodeError> {
        let out = match self.media_file.r#type() {
            Some(MediaType::Video(_)) => steps::patch_webm_bytes(out)?,
            _ => out,
        };
        self.store_output(out)
    }
}

/// 注入 ffmpeg.wasm glue（含 UMD wrapper），await core ready → transcode。
/// 进度闭包 → on_progress；回调内轮询 cancel_flag，置位即 terminate 并提前返回。
pub(crate) async fn exec_ffmpeg_wasm(
    job: WebJob,
    cancel_flag: Arc<AtomicBool>,
    mut on_progress: impl FnMut(f32) + 'static,
) -> Result<Vec<u8>, TranscodeError> {
    inject_scripts(&["/ffmpeg.js", "/ffmpeg-engine.js"], "stickerFfmpegReady").await?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscodeError::Cancelled);
    }
    let ready = sticker_ffmpeg_ready().map_err(|e| js_error(e, &cancel_flag))?;
    js_sys::Promise::from(ready)
        .await
        .map_err(|e| js_error(e, &cancel_flag))?;

    let cancel_flag_probe = Arc::clone(&cancel_flag);
    let closure = Closure::new(move |pct: f32| {
        on_progress(pct);
        if cancel_flag_probe.load(Ordering::Relaxed) {
            sticker_ffmpeg_cancel();
        }
    });

    let data = job.data.as_slice();
    let name = job.name.as_str();
    let bitrate = job.bitrate;
    let fps = job.fps;
    let pix_fmt = job.pix_fmt;
    let promise = sticker_ffmpeg_transcode(data, name, bitrate, fps, pix_fmt, &closure)
        .map_err(|e| js_error(e, &cancel_flag))?;
    let out = promise.await.map_err(|e| js_error(e, &cancel_flag))?;
    closure.forget(); // JS 侧仍持有引用（onProgress），实例重建前不再泄漏增长

    js_value_to_bytes(out)
}

/// 内存字节 → base64 data URL（wasm 预览输入/输出共用；≤512KB 编码毫秒级）。
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// WebCodecs 引擎（Route B）：视频路径先查 caps（非 Chromium → 明确报错），
/// 图片路径直接跑（Canvas/ImageDecoder 无编码 caps 依赖）。
pub(crate) async fn exec_webcodecs(
    job: WebJob,
    cancel_flag: Arc<AtomicBool>,
    mut on_progress: impl FnMut(f32) + 'static,
) -> Result<Vec<u8>, TranscodeError> {
    inject_scripts(
        &["/webm-muxer.js", "/webcodecs-engine.js"],
        "stickerNativeProbe",
    )
    .await?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscodeError::Cancelled);
    }
    if job.kind == "video" && !webcodecs_supported().await {
        return Err(TranscodeError::UnsupportedEngine(
            "WebCodecs VP9 not supported in this browser",
        ));
    }

    let cancel_flag_probe = Arc::clone(&cancel_flag);
    let closure = Closure::new(move |pct: f32| {
        on_progress(pct);
        if cancel_flag_probe.load(Ordering::Relaxed) {
            sticker_webcodecs_cancel();
        }
    });

    let data = job.data.as_slice();
    let name = job.name.as_str();
    let bitrate = job.bitrate;
    let fps = job.fps;
    let kind = job.kind;
    let promise = sticker_webcodecs_transcode(data, name, bitrate, fps, kind, &closure)
        .map_err(|e| js_error(e, &cancel_flag))?;
    let out = promise.await.map_err(|e| js_error(e, &cancel_flag))?;
    closure.forget();

    js_value_to_bytes(out)
}

/// WebCodecs VP9 编码能力（注入 B glue 后查 caps；不加载 ffmpeg core）。
/// UI 置灰与 exec_webcodecs 视频路径共用。失败/不支持 → false。
pub async fn webcodecs_supported() -> bool {
    if inject_scripts(
        &["/webm-muxer.js", "/webcodecs-engine.js"],
        "stickerNativeProbe",
    )
    .await
    .is_err()
    {
        return false;
    }
    let Ok(promise) = sticker_webcodecs_probe_support() else {
        return false;
    };
    js_sys::Promise::from(promise)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
/// 原生探测结果（webcodecs glue 的 stickerNativeProbe，不加载任何引擎）。
pub(crate) struct NativeProbe {
    pub duration: f64,
    pub apng: bool,
}
/// 探测时长（秒）与 APNG 标志；注入失败/异常 → 全零：MP4/GIF 保持
/// InvalidDuration Alert，png 按静态图片继续（语义与扩展名判定一致）。
pub(crate) async fn native_probe(name: &str, data: &[u8]) -> NativeProbe {
    let unknown = || NativeProbe {
        duration: 0.0,
        apng: false,
    };
    if inject_scripts(
        &["/webm-muxer.js", "/webcodecs-engine.js"],
        "stickerNativeProbe",
    )
    .await
    .is_err()
    {
        return unknown();
    }
    let Ok(promise) = sticker_native_probe(name, data) else {
        return unknown();
    };
    let Ok(v) = js_sys::Promise::from(promise).await else {
        return unknown();
    };
    NativeProbe {
        duration: js_sys::Reflect::get(&v, &JsValue::from_str("duration"))
            .ok()
            .and_then(|d| d.as_f64())
            .unwrap_or(0.0),
        apng: js_sys::Reflect::get(&v, &JsValue::from_str("apng"))
            .ok()
            .and_then(|a| a.as_bool())
            .unwrap_or(false),
    }
}

fn js_error(e: JsValue, cancel_flag: &AtomicBool) -> TranscodeError {
    if cancel_flag.load(Ordering::Relaxed) {
        TranscodeError::Cancelled
    } else {
        let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
        TranscodeError::SizeCheck(msg)
    }
}

fn js_value_to_bytes(v: JsValue) -> Result<Vec<u8>, TranscodeError> {
    let arr = js_sys::Uint8Array::new(&v);
    Ok(arr.to_vec())
}

fn reflect_has(window: &web_sys::Window, key: &str) -> bool {
    js_sys::Reflect::has(window, &JsValue::from_str(key)).unwrap_or(false)
}

/// 幂等注入 glue 脚本链（按序：依赖在前）；注入后轮询 marker 是否挂上 window
/// （200ms × 50 次 = 10s 上限），避免 onload 回调的 Closure 生命周期管理。
async fn inject_scripts(srcs: &[&str], marker: &str) -> Result<(), TranscodeError> {
    let window = web_sys::window().ok_or(TranscodeError::SizeCheck("no window on wasm".into()))?;
    if reflect_has(&window, marker) {
        return Ok(());
    }
    let document = window
        .document()
        .ok_or(TranscodeError::SizeCheck("no document on wasm".into()))?;
    // 顺序注入：前一脚本是后一的全局依赖（UMD wrapper → glue；muxer → glue）。
    // 重复插入只多执行一次 IIFE，window 函数被覆盖，无副作用。
    for src in srcs {
        let script = document
            .create_element("script")
            .map_err(|_| TranscodeError::SizeCheck("create script element failed".into()))?;
        script
            .set_attribute("src", src)
            .map_err(|_| TranscodeError::SizeCheck("set script src failed".into()))?;
        let head = document
            .head()
            .ok_or(TranscodeError::SizeCheck("no head on wasm".into()))?;
        head.append_child(&script)
            .map_err(|_| TranscodeError::SizeCheck("append script failed".into()))?;
    }

    for _ in 0..50 {
        if reflect_has(&window, marker) {
            return Ok(());
        }
        crate::timers::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(TranscodeError::SizeCheck(format!(
        "{srcs:?} failed to load within 10s"
    )))
}
