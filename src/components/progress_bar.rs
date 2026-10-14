//! 总体进度条：沿用 `(index+1)/len` 语义，运行中才显示（与 iced 一致）。

use crate::app::UiState;
use dioxus::prelude::*;

#[component]
pub fn ProgressBar() -> Element {
    let ctx = use_context::<UiState>();
    if !ctx.running.cloned() {
        return rsx! {};
    }
    let fraction = ctx.overall_progress.cloned().clamp(0.0, 1.0);
    let pct = (fraction * 100.0).round();
    let total = ctx.tasks.read().len();
    // 文字与条共用 overall_progress（按位次推进，失败/超限任务也算走过），
    // 不另数 Status::Done——否则失败任务会让条走了、数字不动
    let finished = (fraction * total as f32).round() as usize;

    rsx! {
        div { class: "row",
            div { class: "progress-track grow",
                div { class: "progress-fill", width: "{pct}%" }
            }
            // 原先绝对定位在 track 上方 + track 的 overflow:hidden → 从未显示过
            span { class: "progress-label", "{finished} / {total}" }
        }
    }
}
