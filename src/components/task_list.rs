//! 任务列表与任务行。
//!
//! 渲染只读显示镜像，不锁 Transcoder（worker 持锁期间 UI 不卡顿）。
//! Phase C：行内进度条 + 转码耗时 + 错误详情悬停提示 + Probing 状态。

use crate::app::{TaskEntry, UiState};
use crate::components::number_field::F64Input;
use crate::transcoder::Status;
use dioxus::prelude::*;

#[component]
pub fn TaskList() -> Element {
    let ctx = use_context::<UiState>();
    let count = ctx.tasks.read().len();

    rsx! {
        div { class: "task-list grow",
            if count == 0 {
                div { class: "empty-hint",
                    "No tasks yet. Click \"Add File\" or drop files here."
                }
            }
            for index in 0..count {
                TaskRow { key: "{index}", index }
            }
        }
    }
}

#[component]
fn TaskRow(index: usize) -> Element {
    let ctx = use_context::<UiState>();
    let entry = ctx.tasks.read().get(index).cloned();
    let Some(entry) = entry else { return rsx! {} };

    rsx! { TaskRowView { entry, index, running: ctx.running.cloned() } }
}

/// 拆出纯展示子组件：entry 变化时才重渲染本行。
#[component]
fn TaskRowView(entry: TaskEntry, index: usize, running: bool) -> Element {
    let mut ctx = use_context::<UiState>();

    let status_class = match entry.status {
        Status::Probing => "badge probing",
        Status::Processing => "badge processing",
        Status::Done => "badge done",
        Status::Alert => "badge alert",
        Status::SizeExcess => "badge size-excess",
        _ => "badge",
    };

    // 与 iced 一致：输出大小颜色区分是否超限
    let is_excess = entry
        .size_excess_factor()
        .is_some_and(|excess| excess > 1.0);
    let size_text = entry
        .output_size
        .map(|size| format!("{:.2}KB", size as f64 / 1024.0));

    // 与 iced 一致：系数仅在首次转码（自动初始化）后出现
    let factor_value = entry.factor;

    // 转码耗时（成功后保留展示）
    let elapsed_text = entry.elapsed_ms.map(format_elapsed);
    // 悬停提示：优先错误详情，否则完整路径
    let tooltip = match &entry.error {
        Some(err) => format!("{}\n{}", entry.input_path, err),
        None => entry.input_path.clone(),
    };

    rsx! {
        div { class: "task-row", title: "{tooltip}",
            div { class: "row-main",
                span { class: "{status_class}", "[{entry.status:?}]" }
                span { class: "path", "{entry.input_path}" }
                if let Some(pct) = entry.progress {
                    span { class: "pct", "{(pct * 100.0).round()}%" }
                } else if let Some(size) = size_text {
                    span {
                        class: if is_excess { "size-excess" } else { "size-ok" },
                        "{size}"
                    }
                }
                if let Some(elapsed) = elapsed_text {
                    span { class: "elapsed", "{elapsed}" }
                }
                if let Some(factor) = factor_value {
                    F64Input {
                        value: factor,
                        min: 0.1,
                        max: 10.0,
                        disabled: running,
                        on_change: move |v| {
                            ctx.with_task(index, move |t| {
                                if let Some(f) = t.size_factor.as_mut() {
                                    f.set(v);
                                }
                            });
                        },
                    }
                }
            }
            if let Some(pct) = entry.progress {
                div { class: "row-progress",
                    div { class: "row-progress-fill", width: "{(pct * 100.0).round()}%" }
                }
            }
        }
    }
}

/// 毫秒 → `12.3s` / `1m02s`。
fn format_elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}.{:01}s", secs, (ms % 1000) / 100)
    }
}
