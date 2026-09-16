//! 列表底部的结果汇总 + 输出目录入口。
//!
//! 批量跑完之后，"几个成了 / 几个超限 / 几个失败 / 一共多大"是用户立刻要看的，
//! 而"打开输出目录"是紧接着的动作（桌面端）。此前两者都没有：只能逐行数。

use crate::app::UiState;
use crate::transcoder::Status;
use dioxus::prelude::*;

/// 人类可读的大小（KB 进位到 MB）。
fn format_size(bytes: u64) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb >= 1024.0 {
        format!("{:.1} MB", kb / 1024.0)
    } else {
        format!("{kb:.1} KB")
    }
}

#[component]
pub fn SummaryBar() -> Element {
    let ctx = use_context::<UiState>();
    let (total, done, over, failed, bytes) = {
        let list = ctx.tasks.read();
        let (mut done, mut over, mut failed, mut bytes) = (0usize, 0usize, 0usize, 0u64);
        for task in list.iter() {
            match task.mirror.status {
                Status::Done => {
                    done += 1;
                    bytes += task.mirror.output_size.unwrap_or(0);
                }
                // 超限也是"跑完了"，产物同样计入总量（用户要按它决定要不要调小）
                Status::SizeExcess => {
                    over += 1;
                    bytes += task.mirror.output_size.unwrap_or(0);
                }
                Status::Alert => failed += 1,
                _ => {}
            }
        }
        (list.len(), done, over, failed, bytes)
    };
    if total == 0 {
        return rsx! {};
    }

    // 桌面专属按钮（"打开输出目录"）已挪到工具栏的输出目录行——那里才是它的位置
    rsx! {
        div { class: "row summary",
            span { class: "summary-item", "{done} done" }
            if over > 0 {
                span { class: "summary-item summary-over", "{over} over limit" }
            }
            if failed > 0 {
                span { class: "summary-item summary-failed", "{failed} failed" }
            }
            if bytes > 0 {
                span { class: "summary-item", "{format_size(bytes)} total" }
            }
        }
    }
}
