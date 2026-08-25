//! 异步转码循环（Phase A 执行流）。
//!
//! iced 消息链 `SelectRun → CurrentProcess → NextProcess → Done`
//! 改为单个 async 任务：顺序执行 + 尺寸重试 + 取消检查。
//! 重试/系数调整逻辑与 iced 版逐行对应（factor /= excess * 0.96）。

use crate::app::UiState;
use crate::media::MediaType;
use crate::transcoder::{Status, Transcoder};
use dioxus::prelude::*;
use std::sync::{Arc, Mutex};

const IMAGE_MAX_SIZE: u64 = 512 * 1024;
const VIDEO_MAX_SIZE: u64 = 256 * 1024;

/// 与原 main.rs 中 `Transcoder::size_excess_factor` 相同的判定。
fn size_excess_factor(task: &Transcoder) -> Option<f64> {
    let size = task.output_size.as_ref()?.size as f64;
    let limit = match task.media_file.r#type() {
        Some(MediaType::Image(_)) => IMAGE_MAX_SIZE,
        Some(MediaType::Video(_)) => VIDEO_MAX_SIZE,
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

/// 错误弹窗（与 iced 版一致的 rfd 模态框）。
pub async fn show_error_dialog(message: String) {
    rfd::AsyncMessageDialog::new()
        .set_title("Error")
        .set_description(&message)
        .show()
        .await;
}

/// 为尚未设置输出路径的任务分配带时间戳的输出文件。
/// 失败视为全局错误（对应 iced 的 Error(-1) → 终止全部）。
fn ensure_output_dir_set(task: &Arc<Mutex<Transcoder>>, output_dir: &str) -> Result<(), String> {
    let mut t = task.lock().map_err(|e| e.to_string())?;
    if t.get_output().is_none() {
        t.set_output_dir(output_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub async fn run_all(mut ctx: UiState) {
    let max_retry = *ctx.max_retry.peek();
    let mut index = 0usize;
    let mut cancelled = false;

    loop {
        // 动态读取队列长度：与 iced 订阅一样允许运行中拖入新文件
        let Some(task_arc) = ctx.tasks.peek().get(index).map(|e| e.transcoder.clone()) else {
            break;
        };
        if *ctx.cancel.peek() {
            // 已在外层：直接结束整个运行
            break;
        }

        let output_dir = ctx.output_dir.peek().clone();
        if let Err(msg) = ensure_output_dir_set(&task_arc, &output_dir) {
            show_error_dialog(msg).await;
            break;
        }

        let mut retry: u8 = 0;
        loop {
            if *ctx.cancel.peek() {
                cancelled = true;
                break;
            }
            ctx.with_task(index, |t| t.status = Status::Processing);

            // 后台线程执行转码；不跨 await 持有锁
            let result = {
                let task_arc = Arc::clone(&task_arc);
                tokio::task::spawn_blocking(move || {
                    let mut t = task_arc.lock().map_err(|e| e.to_string())?;
                    t.run().map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("join error: {e}")))
            };

            match result {
                Ok(()) => {
                    // 与 iced NextProcess(Ok) 一致：查尺寸 → 调系数 → 决定重试/前进
                    let decision = ctx
                        .with_task(index, |t| match t.check_size() {
                            Ok(_) => match size_excess_factor(t) {
                                Some(excess) if excess > 1.0 => {
                                    if let Some(factor) = t.size_factor.as_mut() {
                                        factor.set(factor.get() / excess * 0.96);
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
                            continue;
                        }
                        Decision::Advance => {
                            let total = ctx.tasks.peek().len().max(1);
                            ctx.overall_progress.set((index + 1) as f32 / total as f32);
                            break;
                        }
                    }
                }
                Err(e) => {
                    // 与 iced NextProcess(Err) 一致：标记 Alert、弹窗、跳到下一个任务（不计入进度）
                    log::error!("Failed to process: {e}");
                    ctx.with_task(index, |t| t.status = Status::Alert);
                    show_error_dialog(e).await;
                    break;
                }
            }
        }
        if cancelled {
            break;
        }
        index += 1;
    }

    ctx.running.set(false);
}
