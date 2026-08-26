//! 异步转码循环。
//!
//! 顺序执行 + 尺寸重试 + 取消检查。Phase C 新增：
//! - 实时进度：`Transcoder::run_with_progress` 经 mpsc 回传百分比
//! - 取消：UI 的 cancel 信号经 watcher 置位任务内 `Arc<AtomicBool>`，ffmpeg 进程被 kill
//! - Toast 通知替代 rfd 模态错误弹窗；错误详情写入任务镜像供悬停提示

use crate::app::{TaskEntry, UiState};
use crate::components::toast::ToastKind;
use crate::media::MediaType;
use crate::transcoder::{Status, Transcoder};
use dioxus::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 单个任务的实时进度更新（进度接收循环消费）。
pub struct ProgressUpdate {
    pub index: usize,
    pub pct: f32,
}

/// 与原 main.rs 中 `Transcoder::size_excess_factor` 相同的判定。
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

/// 为尚未设置输出路径的任务分配带时间戳的输出文件。
fn ensure_output_dir_set(task: &Arc<Mutex<Transcoder>>, output_dir: &str) -> Result<(), String> {
    let mut t = task.lock().map_err(|e| e.to_string())?;
    if t.get_output().is_none() {
        t.set_output_dir(output_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn set_mirror(ctx: &mut UiState, index: usize, f: impl FnOnce(&mut TaskEntry)) {
    ctx.tasks.with_mut(|list| {
        if let Some(entry) = list.get_mut(index) {
            f(entry);
        }
    });
}

pub async fn run_all(
    mut ctx: UiState,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
) {
    let settings = ctx.settings.peek().clone();
    let max_retry = settings.max_retry;
    let (video_limit, image_limit) = (settings.video_max_size(), settings.image_max_size());
    let retry_shrink = settings.retry_shrink_factor;
    let duration_factors = settings.duration_factors;
    let target_fps = settings.target_fps;
    let mut index = 0usize;
    let mut cancelled = false;
    loop {
        // 动态读取队列长度：与 iced 订阅一样允许运行中拖入新文件
        let Some(entry) = ctx.tasks.cloned().get(index).cloned() else {
            break;
        };
        let task_arc = Arc::clone(&entry.transcoder);
        if *ctx.cancel.peek() {
            break;
        }

        let output_dir = ctx.settings.peek().output_dir.clone();
        if let Err(msg) = ensure_output_dir_set(&task_arc, &output_dir) {
            set_mirror(&mut ctx, index, |e| e.error = Some(msg.clone()));
            ctx.with_task(index, |t| t.status = Status::Alert);
            ctx.push_toast(ToastKind::Error, msg);
            break;
        }

        let mut retry: u8 = 0;
        'attempt: loop {
            if *ctx.cancel.peek() {
                cancelled = true;
                break;
            }
            // 取消桥接：watcher 轮询 UI 信号，置位后 run_with_progress 内 kill ffmpeg
            let cancel_flag = match task_arc.lock().ok().map(|t| Arc::clone(&t.cancel_flag)) {
                Some(flag) => flag,
                None => break,
            };
            let cancel_watcher = ctx.cancel; // Signal<bool> is Copy
            let watcher = spawn(async move {
                loop {
                    if *cancel_watcher.read() {
                        cancel_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            });

            ctx.with_task(index, |t| {
                t.duration_factors = duration_factors;
                t.target_fps = target_fps;
                t.status = Status::Processing;
            });
            set_mirror(&mut ctx, index, |e| {
                e.progress = None;
                e.error = None;
            });

            // 后台线程执行转码；不跨 await 持有锁
            let started = Instant::now();
            let result = {
                let task_arc = Arc::clone(&task_arc);
                let tx = progress_tx.clone();
                tokio::task::spawn_blocking(move || {
                    let mut t = task_arc.lock().map_err(|e| e.to_string())?;
                    t.run_with_progress(move |pct| {
                        // 发送失败仅意味着接收端已关闭，忽略即可
                        let _ = tx.send(ProgressUpdate { index, pct });
                    })
                    .map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("join error: {e}")))
            };
            watcher.cancel();

            match result {
                Ok(()) => {
                    let elapsed = started.elapsed().as_millis() as u64;
                    set_mirror(&mut ctx, index, |e| {
                        e.elapsed_ms = Some(elapsed);
                        e.progress = None;
                    });
                    // 与 iced NextProcess(Ok) 一致：查尺寸 → 调系数 → 决定重试/前进
                    let decision = ctx
                        .with_task(index, |t| match t.check_size() {
                            Ok(_) => match size_excess_factor(t, video_limit, image_limit) {
                                Some(excess) if excess > 1.0 => {
                                    if let Some(factor) = t.size_factor.as_mut() {
                                        factor.set(crate::transcoder::shrunk_factor(
                                            factor.get(),
                                            excess,
                                            retry_shrink,
                                        ));
                                    }
                                    t.status = Status::SizeExcess;
                                    if retry < max_retry {
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
                                log::error!("Error check size: {:?}", e);
                                t.status = Status::Alert;
                                Decision::Advance
                            }
                        })
                        .unwrap_or(Decision::Advance);

                    match decision {
                        Decision::Retry => {
                            retry += 1;
                            ctx.push_toast(
                                ToastKind::Warn,
                                format!("Output over size limit, retrying ({retry}/{max_retry})"),
                            );
                            continue;
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
                                let kb = done.output_size.map_or_else(
                                    || "?".into(),
                                    |s| format!("{:.2}KB", s as f64 / 1024.0),
                                );
                                let name = done
                                    .input_path
                                    .rsplit(['\\', '/'])
                                    .next()
                                    .unwrap_or(&done.input_path)
                                    .to_string();
                                ctx.push_toast(ToastKind::Success, format!("{name} -> {kb}"));
                            }
                            break 'attempt;
                        }
                    }
                }
                Err(e) => {
                    if *ctx.cancel.peek() || e == "Cancelled" {
                        // 取消：不标 Alert，错误详情也不写，保持 Processing 由下方复位
                        cancelled = true;
                        ctx.push_toast(
                            ToastKind::Info,
                            format!("Task cancelled: {}", entry.input_path),
                        );
                        break 'attempt;
                    }
                    // 与 iced NextProcess(Err) 一致：标记 Alert 后跳到下一个任务（不计入进度）
                    log::error!("Failed to process: {e}");
                    set_mirror(&mut ctx, index, |entry| entry.error = Some(e.clone()));
                    ctx.with_task(index, |t| t.status = Status::Alert);
                    ctx.push_toast(ToastKind::Error, e);
                    break 'attempt;
                }
            }
        }
        if cancelled {
            // 被取消的任务回到 Pending，可再次 Run（cancel_flag 已在 run_with_progress 入口复位）
            set_mirror(&mut ctx, index, |e| e.progress = None);
            ctx.with_task(index, |t| t.status = Status::Pending);
            break;
        }
        index += 1;
    }

    ctx.running.set(false);
}
