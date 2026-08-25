//! 任务列表与任务行。
//!
//! 与 iced 版对等：`[Status] 路径 … 输出大小(超限红色) 系数输入`；
//! 渲染只读显示镜像，不锁 Transcoder（worker 持锁期间 UI 不卡顿）。

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

    rsx! {
        div { class: "task-row",
            span { class: "{status_class}", "[{entry.status:?}]" }
            span { class: "path", title: "{entry.input_path}", "{entry.input_path}" }
            if let Some(size) = size_text {
                span {
                    class: if is_excess { "size-excess" } else { "size-ok" },
                    "{size}"
                }
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
    }
}
