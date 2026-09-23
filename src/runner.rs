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
use crate::media::MediaType;
#[cfg(not(target_arch = "wasm32"))]
use crate::transcoder::Engine;
use crate::transcoder::{Status, TranscodeError, Transcoder, excess_ratio, shrunk_factor};
use dioxus::prelude::*;
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// 单个任务的实时进度更新（进度接收循环消费）。
pub struct ProgressUpdate {
    pub index: usize,
    pub pct: f32,
}

/// 平台无关的阻塞执行桥：桌面走 tokio spawn_blocking 线程池。
/// wasm 侧没有独立线程（直接同步执行即可），故本函数只在桌面存在——
/// 网页端的探测/转码路径各自同步调用，不经过这里。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// 引擎解析：设置值 + 可用性兜底，返回实际执行的引擎。
/// - 非法值（parse 失败）→ 回落 inprocess（config.load 已兜底，此处是运行期保险）
/// - webcodecs 且浏览器不支持（`web_supported=false`）→ 回落 inprocess
/// - sidecar 且 ffmpeg/libvpx-vp9 不可用 → 回落 inprocess
///
/// 桌面专属：web 端忽略 `Settings.engine`（矩阵即策略，`Engine::for_web` 决定）。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_engine(setting: &str, web_supported: bool) -> Engine {
    let engine = match Engine::parse(setting) {
        Some(e) => e,
        None => {
            log::warn!("engine='{setting}' 非法，回落 inprocess");
            return Engine::Inprocess;
        }
    };
    match engine {
        Engine::Webcodecs if !web_supported => {
            log::warn!("engine=webcodecs 但浏览器不支持，回落 inprocess");
            Engine::Inprocess
        }
        Engine::Sidecar if !sidecar_vp9_available() => {
            log::warn!("engine=sidecar 但 ffmpeg/libvpx-vp9 不可用，回落 inprocess");
            Engine::Inprocess
        }
        other => other,
    }
}

/// sidecar 可用性：查启动时的探测缓存（仅桌面有子进程）。
#[cfg(not(target_arch = "wasm32"))]
fn sidecar_vp9_available() -> bool {
    crate::sidecar_probe::SidecarProbe::probe().is_some_and(|p| p.has_vp9)
}

/// 输出大小相对上限的倍率（>1.0 即超限），无输出时返回 None。
/// 类型缺失（不会发生：入队已按扩展名过滤）按图片上限处理——原先 `unreachable!()`
/// 会直接 panic 掉整个进程，不值得为一个已排除的状态付这个代价。
fn size_excess_factor(task: &Transcoder, video_limit: u64, image_limit: u64) -> Option<f64> {
    let limit = match task.media_file.r#type() {
        Some(MediaType::Video(_)) => video_limit,
        _ => image_limit,
    };
    excess_ratio(task.output_size, limit)
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

/// 为尚未设置输出路径的任务分配输出文件（文件名规则见 `set_output_dir`）。
fn ensure_output_dir_set(
    task: &Arc<Mutex<Transcoder>>,
    output_dir: &str,
    keep_input_name: bool,
) -> Result<(), String> {
    let mut t = task.lock().map_err(|e| e.to_string())?;
    if t.get_output().is_none() {
        t.set_output_dir(output_dir, keep_input_name)
            .map_err(|e| e.to_string())?;
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
    let mut index = 0usize;
    // WebCodecs VP9 caps：每次 Run 探一次（glue 已由探测阶段加载，毫秒级）；
    // 桌面恒 true（wasm 分发不执行，值无消费方）
    #[cfg(target_arch = "wasm32")]
    let webcodecs_ok = crate::transcoder::web::webcodecs_supported().await;
    #[cfg(not(target_arch = "wasm32"))]
    let webcodecs_ok = true;
    // 动态读取队列长度：允许运行中拖入新文件
    while let Some(entry) = ctx.tasks.cloned().get(index).cloned() {
        if *ctx.cancel.peek() {
            break;
        }
        // 运行中拖入的新文件可能仍在探测：等镜像出结果再起——否则
        // write_back_probe 在主线程撞 worker 锁（UI 冻结）、wasm 端
        // prepare_web_job 读到空时长误 Alert。
        if matches!(entry.mirror.status, Status::Probing) {
            crate::timers::sleep(std::time::Duration::from_millis(50)).await;
            continue;
        }
        // Done 不重跑（用户要重转走行内 Re-run）；整体进度按位次照常推进。
        if entry.mirror.status == Status::Done {
            let total = ctx.tasks.peek().len().max(1);
            ctx.overall_progress.set((index + 1) as f32 / total as f32);
            index += 1;
            continue;
        }
        // 每任务取当前设置（output_dir 等）
        let settings = ctx.settings.peek().clone();

        if let Err(msg) = ensure_output_dir_set(
            &entry.transcoder,
            &settings.output_dir,
            settings.keep_input_name,
        ) {
            ctx.touch_entry(index, |e| e.error = Some(msg.clone()));
            ctx.with_task(index, |t| t.status = Status::Alert);
            ctx.push_toast(ToastKind::Error, msg);
            break;
        }

        match run_single_task(&mut ctx, &entry, index, progress_tx.clone(), webcodecs_ok).await {
            TaskOutcome::Advanced => index += 1,
            TaskOutcome::Cancelled => {
                // 复位（progress/状态）由 run_single_task 完成，这里只记日志
                //（cancel_flag 在 run_with_progress 入口复位）
                log::info!("queue cancelled at '{}'", entry.mirror.input_path);
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
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
    webcodecs_ok: bool,
) -> TaskOutcome {
    // 桌面：caps 仅 wasm 分发消费，此处无引用（bool 是 Copy，不影响 wasm 分支使用）
    #[cfg(not(target_arch = "wasm32"))]
    let _ = webcodecs_ok;
    let task_arc = Arc::clone(&entry.transcoder);
    let name = file_name(&entry.mirror.input_path);

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
                // wasm：引擎卡住时不再有进度回调，取消必须主动送达 JS 侧
                // （桌面 sidecar/inprocess 各自在循环里轮询该标志）
                #[cfg(target_arch = "wasm32")]
                crate::transcoder::web::cancel_active();
                return;
            }
            crate::timers::sleep(std::time::Duration::from_millis(100)).await;
        }
    }));

    let mut retry: u8 = 0;
    let mut cancelled = false;

    'attempt: loop {
        if *ctx.cancel.peek() {
            cancelled = true;
            break 'attempt;
        }
        // 每 attempt 重读设置：入口快照会让运行中改的上限/重试数失效，
        // UI 按实时值判超限颜色而 runner 按旧值判 Done —— 显示互相矛盾
        let settings = ctx.settings.peek().clone();
        let max_retry = settings.max_retry;
        let (video_limit, image_limit) = (settings.video_max_size(), settings.image_max_size());
        let duration_factors = settings.duration_factors;
        let target_fps = settings.target_fps;
        let retry_shrink = settings.retry_shrink_factor;
        let duration_patch = settings.webm_duration_patch;

        ctx.with_task(index, |t| {
            t.duration_factors = duration_factors;
            t.target_fps = target_fps;
            t.duration_patch = duration_patch;
            t.status = Status::Processing;
        });
        ctx.progress.set(None);
        ctx.touch_entry(index, |e| e.error = None);

        // 后台线程执行转码；不跨 await 持有锁
        // （std::time::Instant 在 wasm 未实现——桌面才有计时）
        #[cfg(not(target_arch = "wasm32"))]
        let started = Instant::now();
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
                    t.run_with_progress(engine, move |pct| {
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
                // 块作用域收锁：guard 在块尾释放（显式 drop(t) 会让 clippy
                // 误报 await_holding_lock，形状也像真有锁跨 await 的 bug）
                let job_result = {
                    let Ok(mut t) = task_arc.lock() else {
                        // 锁中毒跳过前先收 watcher：否则轮询任务与其 clone 的
                        // cancel_flag 泄漏，该任务下次 Run 秒"取消"
                        if let Some(w) = watcher.take() {
                            w.cancel();
                        }
                        return TaskOutcome::Advanced;
                    };
                    t.cancel_flag
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    t.prepare_web_job(webcodecs_ok)
                };
                match job_result {
                    Ok(job) => {
                        let engine = job.engine;
                        let on_progress = {
                            let tx = tx.clone();
                            move |pct| {
                                let _ = tx.send(ProgressUpdate { index, pct });
                            }
                        };
                        // 穷尽匹配：Engine 类型一路保留，未知引擎串已不可能出现
                        let awaited = match engine {
                            crate::transcoder::Engine::Webcodecs => {
                                crate::transcoder::web::exec_webcodecs(
                                    job,
                                    std::sync::Arc::clone(&cancel_flag),
                                    on_progress,
                                )
                                .await
                            }
                            crate::transcoder::Engine::FfmpegWasm => {
                                crate::transcoder::web::exec_ffmpeg_wasm(
                                    job,
                                    std::sync::Arc::clone(&cancel_flag),
                                    on_progress,
                                )
                                .await
                            }
                            crate::transcoder::Engine::Sidecar
                            | crate::transcoder::Engine::Inprocess => {
                                Err(TranscodeError::UnsupportedEngine(engine.as_str()))
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
        ctx.progress.set(None);
        let Ok(()) = result else {
            let err = result.unwrap_err();
            if *ctx.cancel.peek() || matches!(err, TranscodeError::Cancelled) {
                // 取消：不标 Alert，错误详情也不写，状态由下方复位
                cancelled = true;
                log::info!("{name}: cancelled");
                ctx.push_toast(ToastKind::Info, format!("Task cancelled: {name}"));
            } else {
                // 与 iced NextProcess(Err) 一致：标记 Alert 后跳到下一个任务。
                // 半成品（无 trailer 的 webm 等）与 cancel 路径同法清理，
                // 不留在输出目录冒充成品。Re-run 时被删的可能是上一次成功
                // 的产物——size 必须一并清掉，否则镜像仍显示 ✓/KB、Reveal 与
                // 预览入口保留，而文件已不存在（with_task 随后同步镜像）。
                log::error!("Failed to process {name}: {err}");
                if let Ok(mut t) = task_arc.lock()
                    && let Some(out) = t.get_output()
                {
                    let _ = std::fs::remove_file(out);
                    t.output_size = None;
                }
                ctx.touch_entry(index, |e| e.error = Some(err.to_string()));
                ctx.with_task(index, |t| t.status = Status::Alert);
                ctx.push_toast(ToastKind::Error, err.to_string());
            }
            break 'attempt;
        };
        // 与 iced NextProcess(Ok) 一致：查尺寸 → 调系数 → 决定重试/前进
        // check_size 失败（写盘后文件被删/杀软隔离）也要把原因带到行内：
        // 只置 Alert 的话徽标显示 Failed 却无任何错误详情（对齐转码失败路径）
        let mut size_err: Option<String> = None;
        let decision = ctx
            .with_task(index, |t| match t.check_size() {
                Ok(_) => match size_excess_factor(t, video_limit, image_limit) {
                    Some(excess) if excess > 1.0 => {
                        if let Some(factor) = t.size_factor.as_mut() {
                            let old = *factor;
                            let new = shrunk_factor(old, excess, retry_shrink);
                            *factor = new;
                            log::warn!(
                                "{name}: output {:.2} KB over limit ({excess:.2}x), \
                                 factor {old:.3} -> {new:.3}",
                                t.output_size.map_or(0.0, |s| s as f64 / 1024.0),
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
                    size_err = Some(e.to_string());
                    t.status = Status::Alert;
                    Decision::Advance
                }
            })
            .unwrap_or(Decision::Advance);
        if let Some(msg) = size_err {
            ctx.touch_entry(index, |e| e.error = Some(msg));
        }

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
                    .filter(|e| e.mirror.status == Status::Done)
                {
                    let kb = done
                        .mirror
                        .output_size
                        .map_or_else(|| "?".into(), |s| format!("{:.2}KB", s as f64 / 1024.0));
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let ms = started.elapsed().as_millis() as u64;
                        ctx.touch_entry(index, |e| e.elapsed_ms = Some(ms));
                        log::info!("{name} -> {kb} in {:.1}s", ms as f64 / 1000.0);
                    }
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
        // 半成品输出（无 trailer 的 webm 等）直接清理，避免残留坏文件；
        // 同上：attempt 顶部取消时删掉的可能是 Re-run 前的旧成品，size 一并清
        if let Ok(mut t) = task_arc.lock()
            && let Some(out) = t.get_output()
        {
            let _ = std::fs::remove_file(out);
            t.output_size = None;
        }
        ctx.progress.set(None);
        ctx.with_task(index, |t| t.status = Status::Pending);
        return TaskOutcome::Cancelled;
    }
    TaskOutcome::Advanced
}

#[cfg(test)]
mod tests {
    use super::resolve_engine;
    use crate::transcoder::Engine;

    #[test]
    fn resolve_engine_passthrough_valid() {
        assert_eq!(resolve_engine("inprocess", false), Engine::Inprocess);
        // sidecar 直通与否取决于本机 ffmpeg 探测结果，两者都是合法输出
        let sc = resolve_engine("sidecar", false);
        assert!(sc == Engine::Sidecar || sc == Engine::Inprocess);
    }

    #[test]
    fn resolve_engine_webcodecs_falls_back_when_unsupported() {
        assert_eq!(resolve_engine("webcodecs", false), Engine::Inprocess);
        assert_eq!(resolve_engine("webcodecs", true), Engine::Webcodecs);
    }

    #[test]
    fn resolve_engine_invalid_falls_back() {
        assert_eq!(resolve_engine("gpu", false), Engine::Inprocess);
        assert_eq!(resolve_engine("", false), Engine::Inprocess);
    }

    #[test]
    fn engine_parse_ffmpeg_wasm_roundtrip() {
        use crate::transcoder::Engine;

        let engine = Engine::parse("ffmpeg-wasm").expect("ffmpeg-wasm must parse");
        assert_eq!(engine.as_str(), "ffmpeg-wasm");
        assert_eq!(Engine::parse(engine.as_str()), Some(Engine::FfmpegWasm));
    }
}
