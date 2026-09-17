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
use crate::media::MediaType;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = window)]
extern "C" {
    // `catch`：JS 侧异常转 Result 而非 wasm trap（glue 未就绪时调用不炸上下文）
    #[wasm_bindgen(catch, js_name = stickerFfmpegReady)]
    fn sticker_ffmpeg_ready(
        on_core_progress: &Closure<dyn FnMut(f32)>,
    ) -> Result<js_sys::Promise, JsValue>;
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
    /// `catch`：glue 尚未注入时 `window.sticker*Cancel` 还不存在，直接调用会抛
    /// TypeError 并丢掉整个 wasm 上下文——转成 Err 由调用方忽略。
    #[wasm_bindgen(catch, js_name = stickerFfmpegCancel)]
    fn sticker_ffmpeg_cancel() -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = stickerWebcodecsCancel)]
    fn sticker_webcodecs_cancel() -> Result<(), JsValue>;
}

/// 主动取消当前网页端引擎（runner 的取消 watcher 直接调用，不经进度回调）。
/// 引擎卡住时不会再有进度回调——取消必须能主动送达 JS 侧，否则"取消"只是把
/// 标志置位，任务要等引擎自己退出。wasm 单线程、队列串行，全局取消即当前任务。
pub fn cancel_active() {
    let _ = sticker_ffmpeg_cancel();
    let _ = sticker_webcodecs_cancel();
}

/// 已解出锁的任务参数（锁不跨 await）。engine/kind 决定运行期分发，
/// pix_fmt 仅 ffmpeg-wasm 分支消费（MP4=10-bit，GIF/APNG=alpha）。
pub struct WebJob {
    pub data: Vec<u8>,
    pub name: String,
    pub bitrate: u32,
    pub fps: f64,
    /// 类型化引擎（矩阵来自 `Engine::for_web`）；分发表靠它穷尽匹配。
    pub engine: super::Engine,
    /// glue 侧契约值（"video" / "image"），与 pix_fmt 同类：JS 按字符串取值。
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
        let bitrate = match media_type {
            MediaType::Video(v) => self.video_bitrate(&v)?.1,
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

    // 码率链共享 command.rs::video_bitrate（三引擎唯一出处）

    /// 阶段 2（锁内，同步）：视频走 webm 时长补丁（设置关掉则原样），
    /// 图片（PNG）原样 → store_output（Bytes 源写内存）。
    // ponytail: web 图片不跑 oxipng——libdeflate-sys 需 wasm C 工具链（clang）；
    // 浏览器 convertToBlob 的 PNG 已合法且贴纸远小于 512KB 上限。web 图片若超限
    // 再接 wasm 版 oxipng 或后端压缩。
    pub fn finish_web_job(&mut self, out: Vec<u8>) -> Result<(), TranscodeError> {
        let out = match self.media_file.r#type() {
            Some(MediaType::Video(_)) if self.duration_patch => steps::patch_webm_bytes(out)?,
            _ => out,
        };
        self.store_output(out)
    }
}

/// 注入 ffmpeg.wasm glue（含 UMD wrapper），await core ready → transcode。
/// 进度两段拼接：core 首载（32MB 流式下载）映射 0–30%，转码映射 30–100%；
/// 回调内轮询 cancel_flag，置位即 terminate 并提前返回。
pub(crate) async fn exec_ffmpeg_wasm(
    job: WebJob,
    cancel_flag: Arc<AtomicBool>,
    on_progress: impl FnMut(f32) + 'static,
) -> Result<Vec<u8>, TranscodeError> {
    inject_scripts(&["ffmpeg.js", "ffmpeg-engine.js"], "stickerFfmpegReady").await?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscodeError::Cancelled);
    }
    // 两段闭包共享同一 on_progress（wasm 单线程，Rc<RefCell> 足够）。
    // core 下载进度（0–30%）：闭包被 JS 侧在 load 期间持有，await 返回后
    // 不再被调用，随作用域 drop。
    let on_progress = std::rc::Rc::new(std::cell::RefCell::new(on_progress));
    let op_core = std::rc::Rc::clone(&on_progress);
    let core_closure = Closure::new(move |pct: f32| {
        (*op_core.borrow_mut())(pct * 0.3);
    }); // ponytail: core 下载期不可取消，ready resolve 后统一查 cancel_flag
    let ready = sticker_ffmpeg_ready(&core_closure).map_err(|e| js_error(e, &cancel_flag))?;
    ready.await.map_err(|e| js_error(e, &cancel_flag))?;
    drop(core_closure); // JS 已 resolve，闭包不再可达

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscodeError::Cancelled);
    }
    let cancel_flag_probe = Arc::clone(&cancel_flag);
    let op_xfer = std::rc::Rc::clone(&on_progress);
    let closure = Closure::new(move |pct: f32| {
        (*op_xfer.borrow_mut())(0.3 + pct * 0.7);
        if cancel_flag_probe.load(Ordering::Relaxed) {
            let _ = sticker_ffmpeg_cancel();
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
    // glue 的 finally 已 ff.off("progress")——promise settle 后 JS 不再引用
    // 本闭包，随作用域 drop（此前 forget() 使监听器链随任务数无界增长）。

    js_value_to_bytes(out)
}

/// WebCodecs 引擎（Route B）：视频路径先查 caps（非 Chromium → 明确报错），
/// 图片路径直接跑（Canvas/ImageDecoder 无编码 caps 依赖）。
pub(crate) async fn exec_webcodecs(
    job: WebJob,
    cancel_flag: Arc<AtomicBool>,
    mut on_progress: impl FnMut(f32) + 'static,
) -> Result<Vec<u8>, TranscodeError> {
    inject_scripts(
        &["webm-muxer.js", "webcodecs-engine.js"],
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
            let _ = sticker_webcodecs_cancel();
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

    js_value_to_bytes(out)
}

/// WebCodecs VP9 编码能力（注入 B glue 后查 caps；不加载 ffmpeg core）。
/// UI 置灰与 exec_webcodecs 视频路径共用。失败/不支持 → false。
pub async fn webcodecs_supported() -> bool {
    if inject_scripts(
        &["webm-muxer.js", "webcodecs-engine.js"],
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
    promise
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
        &["webm-muxer.js", "webcodecs-engine.js"],
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
    let Ok(v) = promise.await else {
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
        TranscodeError::Engine(msg)
    }
}

fn js_value_to_bytes(v: JsValue) -> Result<Vec<u8>, TranscodeError> {
    // glue 契约违约（返回非 typed-array）→ 0 字节而不是报错，
    // 会伪装成"成功且小于上限"的 Done
    let Some(arr) = v.dyn_ref::<js_sys::Uint8Array>() else {
        return Err(TranscodeError::Engine("engine returned no bytes".into()));
    };
    if arr.length() == 0 {
        return Err(TranscodeError::Engine(
            "engine returned empty output".into(),
        ));
    }
    Ok(arr.to_vec())
}

fn reflect_has(window: &web_sys::Window, key: &str) -> bool {
    js_sys::Reflect::has(window, &JsValue::from_str(key)).unwrap_or(false)
}

/// 幂等注入 glue 脚本链（按序：依赖在前）；注入后轮询 marker 是否挂上 window
/// （200ms × 50 次 = 10s 上限），避免 onload 回调的 Closure 生命周期管理。
async fn inject_scripts(srcs: &[&str], marker: &str) -> Result<(), TranscodeError> {
    let window =
        web_sys::window().ok_or_else(|| TranscodeError::Engine("no window on wasm".into()))?;
    if reflect_has(&window, marker) {
        return Ok(());
    }
    let document = window
        .document()
        .ok_or_else(|| TranscodeError::Engine("no document on wasm".into()))?;
    // 顺序注入：前一脚本是后一的全局依赖（UMD wrapper → glue；muxer → glue）。
    // 重复插入只多执行一次 IIFE，window 函数被覆盖，无副作用。
    for src in srcs {
        let script = document
            .create_element("script")
            .map_err(|_| TranscodeError::Engine("create script element failed".into()))?;
        script
            .set_attribute("src", src)
            .map_err(|_| TranscodeError::Engine("set script src failed".into()))?;
        let head = document
            .head()
            .ok_or_else(|| TranscodeError::Engine("no head on wasm".into()))?;
        head.append_child(&script)
            .map_err(|_| TranscodeError::Engine("append script failed".into()))?;
    }

    for _ in 0..50 {
        if reflect_has(&window, marker) {
            return Ok(());
        }
        crate::timers::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(TranscodeError::Engine(format!(
        "{srcs:?} failed to load within 10s"
    )))
}
