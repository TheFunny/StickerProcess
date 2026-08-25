//! 总体进度条：沿用 `(index+1)/len` 语义，运行中才显示（与 iced 一致）。

use crate::app::UiState;
use dioxus::prelude::*;

#[component]
pub fn ProgressBar() -> Element {
    let ctx = use_context::<UiState>();
    if !ctx.running.cloned() {
        return rsx! {};
    }
    let pct = (ctx.overall_progress.cloned().clamp(0.0, 1.0) * 100.0).round();

    rsx! {
        div { class: "progress-track",
            div { class: "progress-fill", width: "{pct}%" }
        }
    }
}
