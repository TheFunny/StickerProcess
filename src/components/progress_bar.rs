//! 总体进度条：沿用 `(index+1)/len` 语义，运行中才显示（与 iced 一致）。

use crate::app::UiState;
use crate::transcoder::Status;
use dioxus::prelude::*;

#[component]
pub fn ProgressBar() -> Element {
    let ctx = use_context::<UiState>();
    if !ctx.running.cloned() {
        return rsx! {};
    }
    let pct = (ctx.overall_progress.cloned().clamp(0.0, 1.0) * 100.0).round();
    let list = ctx.tasks.read();
    let total = list.len();
    let done = list.iter().filter(|t| t.status == Status::Done).count();

    rsx! {
        div { class: "progress-track",
            div { class: "progress-fill", width: "{pct}%" }
            span { class: "progress-label", "{done} / {total}" }
        }
    }
}
