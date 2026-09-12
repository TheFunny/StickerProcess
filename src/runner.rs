//! 异步转码循环。
//!
//! 结构（Phase E3/E1）：
//! - [`run_all`]：外层队列循环，逐任务调用 [`run_single_task`]
//! - [`run_single_task`]：单个任务的尝试循环（尺寸重试），返回 [`TaskOutcome`]
//! - 取消：UI 的 cancel 信号经**每任务一个**的 watcher 置位任务内
//!   `Arc<AtomicBool>`，ffmpeg 进程被 kill
//! - 实时进度：`Transcoder::run_with_progress` 经 mpsc 回传（接收端在
//!   `UiState::start_run` 中按 ~10Hz 节流合并）
//! - 错误为 `TranscodeError` 枚举；日志覆盖重试/系数调整/取消/完成

use crate::app::{TaskEntry, UiState};
use crate::components::toast::ToastKind;
use crate::config::Settings;
use crate::media::MediaType;
use crate::transcoder::{Engine, Status, TranscodeError, Transcoder, shrunk_factor};
use dioxus::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 单个任务的实时进度更新（进度接收循环消费）。
pub struct ProgressUpdate {
    pub index: usize,
    pub pct: f32,
}

/// 平台无关的阻塞执行桥：桌面走 tokio spawn_blocking 线程池；
/// wasm 单线程直接执行（阻塞 UI —— 下一阶段换 web worker）。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Result<T, String>> {
    async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn run_blocking<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    Ok(f())
}

/// 引擎解析：设置值 + 可用性兜底，返回实际执行的引擎字符串。
/// - 非法值（parse 失败）→ 回落 inprocess（config.load 已兜底，此处是运行期保险）
/// - webcodecs 且浏览器不支持（`web_supported=false`）→ 回落 inprocess
/// - sidecar 且 ffmpeg/libvpx-vp9 不可用 → 回落 inprocess
/// 桌面端 `web_supported` 恒为 false；wasm 侧由 JS 探测结果传入。
pub(crate) fn resolve_engine(setting: &str, web_supported: bool) -> String {
    let engine = match Engine::parse(setting) {
        Some(e) => e,
        None => {
            log::warn!("engine='{setting}' 非法，回落 inprocess");
            return Engine::Inprocess.as_str().to_string();
        }
    };
    match engine {
        Engine::Webcodecs if !web_supported => {
            log::warn!("engine=webcodecs 但浏览器不支持，回落 inprocess");
            Engine::Inprocess.as_str().to_string()
        }
        Engine::Sidecar if !sidecar_vp9_available() => {
            log::warn!("engine=sidecar 但 ffmpeg/libvpx-vp9 不可用，回落 inprocess");
            Engine::Inprocess.as_str().to_string()
        }
        other => other.as_str().to_string(),
    }
}

/// sidecar 可用性：桌面查探测缓存；wasm 恒 false（无子进程）。
#[cfg(feature = "desktop")]
fn sidecar_vp9_available() -> bool {
    crate::sidecar_probe::SidecarProbe::probe().is_some_and(|p| p.has_vp9)
}

#[cfg(not(feature = "desktop"))]
fn sidecar_vp9_available() -> bool {
    false
}

/// 输出大小相对上限的倍率（>1.0 即超限），无输出时返回 None。
fn size_excess_factor(task: &Transcoder, video_limit: u64, image_limit: u64) -> Option<f64> {
    let size = task.output_size.as_ref()?.size as f64;
    let limit = match task.media_file.r#type() {
        Some(MediaType::Image(_)) => image_limit,
        Some(MediaType::Video(_)) => video_limit,
        None => unreachable!(),
    };
    Some(size / limit as f64)
}

enum Decision {
    /// 未达重试上限，缩小系数后重跑当前任务。
    Retry,
    /// 进入下一个任务（含 Done / Alert / 重试耗尽）。
    Advance,
}

enum TaskOutcome {
    /// 任务结束（Done / Alert / 重试耗尽），继续下一个。
    Advanced,
    /// 用户取消，终止整个队列。
    Cancelled,
}

/// 为尚未设置输出路径的任务分配带时间戳的输出文件。
fn ensure_output_dir_set(task: &Arc<Mutex<Transcoder>>, output_dir: &str) -> Result<(), String> {
    let mut t = task.lock().map_err(|e| e.to_string())?;
    if t.get_output().is_none() {
        t.set_output_dir(output_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

pub async fn run_all(
    mut ctx: UiState,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
) {
    let settings = ctx.settings.peek().clone();
    let mut index = 0usize;

    loop {
        // 动态读取队列长度：允许运行中拖入新文件
        let Some(entry) = ctx.tasks.cloned().get(index).cloned() else {
            break;
        };
        if *ctx.cancel.peek() {
            break;
        }

        if let Err(msg) = ensure_output_dir_set(&entry.transcoder, &settings.output_dir) {
            ctx.touch_entry(index, |e| e.error = Some(msg.clone()));
            ctx.with_task(index, |t| t.status = Status::Alert);
            ctx.push_toast(ToastKind::Error, msg);
            break;
        }

        match run_single_task(&mut ctx, &entry, index, &settings, progress_tx.clone()).await {
            TaskOutcome::Advanced => index += 1,
            TaskOutcome::Cancelled => {
                // 被取消的任务回到 Pending，可再次 Run（cancel_flag 在
                // run_with_progress 入口复位）
                ctx.touch_entry(index, |e| e.progress = None);
                ctx.with_task(index, |t| t.status = Status::Pending);
                log::info!("queue cancelled at '{}'", entry.input_path);
                break;
            }
        }
    }

    ctx.running.set(false);
}

/// 运行单个任务（含尺寸重试循环）。watcher 每任务一个，跨重试存活。
async fn run_single_task(
    ctx: &mut UiState,
    entry: &TaskEntry,
    index: usize,
    settings: &Settings,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
) -> TaskOutcome {
    let task_arc = Arc::clone(&entry.transcoder);
    let name = file_name(&entry.input_path);

    // 取消桥接：watcher 轮询 UI 信号，置位后 run_with_progress 内 kill ffmpeg
    let Some(cancel_flag) = task_arc.lock().ok().map(|t| Arc::clone(&t.cancel_flag)) else {
        return TaskOutcome::Advanced;
    };
    // watcher 闭包 move 一份；转码分支再 clone 一份
    let cancel_flag_watcher = Arc::clone(&cancel_flag);
    let cancel_signal = ctx.cancel; // Signal<bool> is Copy
    let mut watcher = Some(dioxus::prelude::spawn(async move {
        loop {
            if *cancel_signal.read() {
                cancel_flag_watcher.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            crate::timers::sleep(std::time::Duration::from_millis(100)).await;
        }
    }));
    let max_retry = settings.max_retry;
    let (video_limit, image_limit) = (settings.video_max_size(), settings.image_max_size());
    let duration_factors = settings.duration_factors;
    let target_fps = settings.target_fps;
    let retry_shrink = settings.retry_shrink_factor;

    let mut retry: u8 = 0;
    let mut cancelled = false;

    'attempt: loop {
        if *ctx.cancel.peek() {
            cancelled = true;
            break 'attempt;
        }

        ctx.with_task(index, |t| {
            t.duration_factors = duration_factors;
            t.target_fps = target_fps;
            t.status = Status::Processing;
        });
        ctx.touch_entry(index, |e| {
            e.progress = None;
            e.error = None;
        });

        // 后台线程执行转码；不跨 await 持有锁
        // （std::time::Instant 在 wasm 未实现——桌面才有计时）
        #[cfg(not(target_arch = "wasm32"))]
        let started = Instant::now();
        #[cfg(target_arch = "wasm32")]
        let started = ();
        let result = {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let task_arc = Arc::clone(&task_arc);
                let tx = progress_tx.clone();
                // 引擎解析：设置值 + 可用性兜底（详见 resolve_engine）。
                // 桌面端 web_supported 恒 false；wasm 化时改为传 JS 探测结果。
                let engine = resolve_engine(&settings.engine, false);
                let awaited = run_blocking(move || {
                    let mut t = task_arc
                        .lock()
                        .map_err(|e| TranscodeError::Join(e.to_string()))?;
                    t.run_with_progress(&engine, move |pct| {
                        // 发送失败仅意味着接收端已关闭，忽略即可
                        let _ = tx.send(ProgressUpdate { index, pct });
                    })
                })
                .await;
                // 扁平化 run_blocking 的 JoinError 包装：内层就是转码结果
                match awaited {
                    Ok(inner) => inner,
                    Err(e) => Err(TranscodeError::Join(e)),
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                // wasm：两段式（锁内 prepare → await JS 引擎 → 锁内 finish），
                // 锁均不跨 await。重试时 prepare_web_job 以收缩后的因子重算 bitrate。
                let tx = progress_tx.clone();
                let Ok(mut t) = task_arc.lock() else {
                    return TaskOutcome::Advanced; // 锁中毒：跳过
                };
                t.cancel_flag
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                let job_result = t.prepare_web_job();
                drop(t);
                match job_result {
                    Ok(job) => {
                        let engine = job.engine;
                        let on_progress = {
                            let tx = tx.clone();
                            move |pct| {
                                let _ = tx.send(ProgressUpdate { index, pct });
                            }
                        };
                        let awaited = match engine {
                            "webcodecs" => {
                                crate::transcoder::web::exec_webcodecs(
                                    job,
                                    std::sync::Arc::clone(&cancel_flag),
                                    on_progress,
                                )
                                .await
                            }
                            _ => {
                                crate::transcoder::web::exec_ffmpeg_wasm(
                                    job,
                                    std::sync::Arc::clone(&cancel_flag),
                                    on_progress,
                                )
                                .await
                            }
                        };
                        match awaited {
                            Ok(out) => {
                                let result = task_arc.lock().map(|mut t| t.finish_web_job(out));
                                match result {
                                    Ok(inner) => inner,
                                    Err(e) => Err(TranscodeError::Join(e.to_string())),
                                }
                            }
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                }
            }
        };
        ctx.touch_entry(index, |e| {
            e.progress = None;
        });
        let Ok(()) = result else {
            let err = result.unwrap_err();
            if *ctx.cancel.peek() || matches!(err, TranscodeError::Cancelled) {
                // 取消：不标 Alert，错误详情也不写，状态由下方复位
                cancelled = true;
                log::info!("{name}: cancelled");
                ctx.push_toast(ToastKind::Info, format!("Task cancelled: {name}"));
            } else {
                // 与 iced NextProcess(Err) 一致：标记 Alert 后跳到下一个任务
                log::error!("Failed to process {name}: {err}");
                ctx.touch_entry(index, |e| e.error = Some(err.to_string()));
                ctx.with_task(index, |t| t.status = Status::Alert);
                ctx.push_toast(ToastKind::Error, err.to_string());
            }
            break 'attempt;
        };
        // 与 iced NextProcess(Ok) 一致：查尺寸 → 调系数 → 决定重试/前进
        let decision = ctx
            .with_task(index, |t| match t.check_size() {
                Ok(_) => match size_excess_factor(t, video_limit, image_limit) {
                    Some(excess) if excess > 1.0 => {
                        if let Some(factor) = t.size_factor.as_mut() {
                            let old = factor.get();
                            let new = shrunk_factor(old, excess, retry_shrink);
                            factor.set(new);
                            log::warn!(
                                "{name}: output {:.2} KB over limit ({excess:.2}x), \
                                 factor {old:.3} -> {new:.3}",
                                t.output_size
                                    .as_ref()
                                    .map_or(0.0, |s| s.size as f64 / 1024.0),
                            );
                        }
                        t.status = Status::SizeExcess;
                        if retry < max_retry && t.size_factor.is_some() {
                            Decision::Retry
                        } else {
                            Decision::Advance
                        }
                    }
                    Some(_) => {
                        t.status = Status::Done;
                        Decision::Advance
                    }
                    None => {
                        t.status = Status::Alert;
                        Decision::Advance
                    }
                },
                Err(e) => {
                    log::error!("Error check size: {e}");
                    t.status = Status::Alert;
                    Decision::Advance
                }
            })
            .unwrap_or(Decision::Advance);

        match decision {
            Decision::Retry => {
                retry += 1;
                log::info!("{name}: retrying ({retry}/{max_retry})");
                ctx.push_toast(
                    ToastKind::Warn,
                    format!("Output over size limit, retrying ({retry}/{max_retry})"),
                );
                continue 'attempt;
            }
            Decision::Advance => {
                let total = ctx.tasks.peek().len().max(1);
                ctx.overall_progress.set((index + 1) as f32 / total as f32);
                if let Some(done) = ctx
                    .tasks
                    .cloned()
                    .get(index)
                    .filter(|e| e.status == Status::Done)
                {
                    let kb = done
                        .output_size
                        .map_or_else(|| "?".into(), |s| format!("{:.2}KB", s as f64 / 1024.0));
                    #[cfg(not(target_arch = "wasm32"))]
                    log::info!(
                        "{name} -> {kb} in {:.1}s",
                        started.elapsed().as_millis() as f64 / 1000.0
                    );
                    #[cfg(target_arch = "wasm32")]
                    log::info!("{name} -> {kb}");
                    ctx.push_toast(ToastKind::Success, format!("{name} -> {kb}"));
                }
                break 'attempt;
            }
        }
    }

    if let Some(watcher) = watcher.take() {
        watcher.cancel();
    }

    if cancelled {
        // 半成品输出（无 trailer 的 webm 等）直接清理，避免残留坏文件
        if let Ok(t) = task_arc.lock()
            && let Some(out) = t.get_output()
        {
            let _ = std::fs::remove_file(out);
        }
        ctx.touch_entry(index, |e| e.progress = None);
        ctx.with_task(index, |t| t.status = Status::Pending);
        return TaskOutcome::Cancelled;
    }
    TaskOutcome::Advanced
}

#[cfg(test)]
mod tests {
    use super::resolve_engine;

    #[test]
    fn resolve_engine_passthrough_valid() {
        assert_eq!(resolve_engine("inprocess", false), "inprocess");
        // sidecar 直通与否取决于本机 ffmpeg 探测结果，两者都是合法输出
        let sc = resolve_engine("sidecar", false);
        assert!(sc == "sidecar" || sc == "inprocess");
    }

    #[test]
    fn resolve_engine_webcodecs_falls_back_when_unsupported() {
        assert_eq!(resolve_engine("webcodecs", false), "inprocess");
        assert_eq!(resolve_engine("webcodecs", true), "webcodecs");
    }

    #[test]
    fn resolve_engine_invalid_falls_back() {
        assert_eq!(resolve_engine("gpu", false), "inprocess");
        assert_eq!(resolve_engine("", false), "inprocess");
    }

    #[test]
    fn engine_parse_ffmpeg_wasm_roundtrip() {
        use crate::transcoder::Engine;

        let engine = Engine::parse("ffmpeg-wasm").expect("ffmpeg-wasm must parse");
        assert_eq!(engine.as_str(), "ffmpeg-wasm");
        assert_eq!(Engine::parse(engine.as_str()), Some(Engine::FfmpegWasm));
    }
}
