//! 网页端引擎桥（Route A：ffmpeg.wasm）。
//!
//! 两段式执行避免 MutexGuard 跨 await：
//! 1. 锁内 `prepare_web_job`（校验类型/求码率/克隆字节）
//! 2. await `exec_ffmpeg_wasm`（JS 胶水跑 ffmpeg.wasm，进度经回调上报）
//! 3. 锁内 `finish_web_job`（时长补丁 → store_output）
//!
//! JS 侧契约在 `public/ffmpeg-engine.js`（stickerFfmpeg* 系列）。

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
        on_progress: &Closure<dyn FnMut(f32)>,
    ) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(catch, js_name = stickerFfmpegProbe)]
    fn sticker_ffmpeg_probe(name: &str, data: &[u8]) -> Result<js_sys::Promise, JsValue>;
    /// 公开给组件层（task_list 下载）。终止 ffmpeg.wasm 用 stickerFfmpegCancel。
    #[wasm_bindgen(js_name = stickerDownload)]
    pub fn sticker_download(data: &[u8], filename: &str);
    #[wasm_bindgen(js_name = stickerFfmpegCancel)]
    fn sticker_ffmpeg_cancel();
}

/// 已解出锁的任务参数（锁不跨 await）。
pub struct WebJob {
    pub data: Vec<u8>,
    pub name: String,
    pub bitrate: u32,
    pub fps: f64,
}

impl Transcoder {
    /// 阶段 1（锁内，同步）：校验类型（W1 仅 Gif/Apng）、求 duration/因子/bitrate
    /// （复用 effective_duration + resolve_factor——因子惰性初始化与 GIF×0.75 patch
    /// 与桌面完全一致）、克隆输入字节。
    // ponytail: 克隆一次输入字节（典型 ≤5MB）换锁不跨 await；内存实测吃紧改 Rc/Bytes 共享
    pub fn prepare_web_job(&mut self) -> Result<WebJob, TranscodeError> {
        let v_type = match self.media_file.r#type() {
            Some(MediaType::Video(v @ (VideoType::Gif | VideoType::Apng))) => v,
            Some(MediaType::Video(VideoType::Mp4)) | Some(MediaType::Image(_)) => {
                return Err(TranscodeError::UnsupportedEngine(
                    "mp4 & image web engine lands in W2",
                ));
            }
            _ => return Err(TranscodeError::InvalidMediaType),
        };
        let duration = self.effective_duration(&v_type)?;
        let factor = self.resolve_factor(duration, &v_type);
        let bitrate =
            super::command::quantized_bitrate(super::command::target_bitrate_bps(duration), factor);
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
        })
    }

    /// 阶段 2（锁内，同步）：webm 时长补丁 → store_output（Bytes 源写内存）。
    pub fn finish_web_job(&mut self, out: Vec<u8>) -> Result<(), TranscodeError> {
        let patched = steps::patch_webm_bytes(out)?;
        self.store_output(patched)
    }
}

/// 注入 <script src="/ffmpeg-engine.js">（幂等），await ready（首次拉起 32MB core 加载），
/// await transcode。进度闭包 → on_progress；回调内轮询 cancel_flag，置位即 terminate
/// 并提前返回。Promise reject：cancel_flag 已置位 → Cancelled，否则 SizeCheck 承载
/// JS 错误文本。exec 前若 cancel_flag 已置位 → 直接 Cancelled。
pub(crate) async fn exec_ffmpeg_wasm(
    job: WebJob,
    cancel_flag: Arc<AtomicBool>,
    mut on_progress: impl FnMut(f32) + 'static,
) -> Result<Vec<u8>, TranscodeError> {
    inject_engine_script().await?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscodeError::Cancelled);
    }
    let ready = sticker_ffmpeg_ready().map_err(|e| js_error(e, &cancel_flag))?;
    js_sys::Promise::from(ready)
        .await
        .map_err(|e| js_error(e, &cancel_flag))?;

    // 进度闭包：上报进度 + 轮询取消（ffmpeg.wasm 无 seek 式取消，只能 terminate）
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
    let promise = sticker_ffmpeg_transcode(data, name, bitrate, fps, &closure)
        .map_err(|e| js_error(e, &cancel_flag))?;
    let out = promise.await.map_err(|e| js_error(e, &cancel_flag))?;
    closure.forget(); // JS 侧仍持有引用（onProgress），实例重建前不再泄漏增长

    js_value_to_bytes(out)
}

/// 探测时长（秒）：Promise<f64>，0 视为未知。不依赖 core 加载。
pub(crate) async fn probe_duration(name: &str, data: &[u8]) -> f64 {
    if inject_engine_script().await.is_err() {
        return 0.0;
    }
    let promise = match sticker_ffmpeg_probe(name, data) {
        Ok(p) => p,
        Err(_) => return 0.0, // glue 未加载（或 JS 异常）：时长未知
    };
    match js_sys::Promise::from(promise).await {
        Ok(v) => v.as_f64().unwrap_or(0.0),
        Err(_) => 0.0,
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

/// 幂等注入 glue 脚本；注入后轮询 `stickerFfmpegReady` 是否挂上 window
/// （200ms × 50 次 = 10s 上限），避免 onload 回调的 Closure 生命周期管理。
async fn inject_engine_script() -> Result<(), TranscodeError> {
    let window = web_sys::window().ok_or(TranscodeError::SizeCheck("no window on wasm".into()))?;
    let document = window
        .document()
        .ok_or(TranscodeError::SizeCheck("no document on wasm".into()))?;
    if reflect_has(&window, "stickerFfmpegReady") {
        return Ok(());
    }
    // 两段注入：ffmpeg.js（UMD wrapper，定义 FFmpegWASM）先于 glue（依赖该全局）。
    // 均为幂等注入——重复插入只多执行一次 IIFE，window 函数被覆盖，无副作用。
    for src in ["/ffmpeg.js", "/ffmpeg-engine.js"] {
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
        if reflect_has(&window, "stickerFfmpegReady") {
            return Ok(());
        }
        crate::timers::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(TranscodeError::SizeCheck(
        "ffmpeg-engine.js failed to load within 10s".into(),
    ))
}
