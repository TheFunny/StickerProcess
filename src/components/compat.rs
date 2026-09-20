//! 兼容性报告弹窗：引擎 / 资产 / 路由的只读体检。
//!
//! 打开时探测一次（结果缓存进信号；Re-check 重探）。重探**不清空**已有报告，
//! 只把按钮切成 "Re-checking…"：清空会让弹窗高度从满屏塌成一行再撑开，肉眼就是
//! 明显闪烁（桌面 WebView2 上尤其刺眼）。首次打开仍显示 "Detecting…"。
//! 数据层在 `crate::compat`（两平台同名 `collect`），本文件只管渲染。

use crate::app::UiState;
use crate::compat::CompatReport;
use dioxus::prelude::*;

#[component]
pub fn CompatModal() -> Element {
    let mut ctx = use_context::<UiState>();
    // 探测结果缓存；None = 尚未探测（仅首次打开时出现）
    let mut report = use_signal(|| None::<CompatReport>);
    // Re-check 的触发器：仅凭 report 置 None 不会重跑 effect（依赖没变）
    let mut nonce = use_signal(|| 0u32);
    // 已完成的批次号：首次打开是 0，每次 Re-check 递增
    let mut probed = use_signal(|| None::<u32>);
    let mut probing = use_signal(|| false);
    let open = ctx.show_compat;

    use_effect(move || {
        let want = nonce(); // 依赖：每次 Re-check 递增
        if !open.cloned() || *probed.peek() == Some(want) {
            return;
        }
        probing.set(true);
        let settings = ctx.settings.peek().clone();
        spawn(async move {
            let collected = crate::compat::collect(settings).await;
            report.set(Some(collected));
            probed.set(Some(want));
            probing.set(false);
        });
    });

    if !open.cloned() {
        return rsx! {};
    }
    let collected = report.read().clone();

    rsx! {
        div {
            class: "modal-backdrop",
            tabindex: 0,
            role: "dialog",
            "aria-label": "Compatibility report",
            onmounted: move |evt: Event<MountedData>| {
                spawn(async move {
                    let _ = evt.data.set_focus(true).await;
                });
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                if evt.data.key() == dioxus::html::Key::Escape {
                    ctx.show_compat.set(false);
                }
            },
            onclick: move |_| ctx.show_compat.set(false),
            div {
                class: "modal modal-settings",
                onclick: move |evt: Event<MouseData>| evt.stop_propagation(),
                div { class: "modal-head",
                    h2 { class: "modal-title", "Compatibility Report" }
                    button {
                        class: "btn icon",
                        title: "Close",
                        "aria-label": "Close compatibility report",
                        onclick: move |_| ctx.show_compat.set(false),
                        crate::components::icon::IconClose {}
                    }
                }
                div { class: "modal-body",
                    match collected {
                        None => rsx! {
                            span { class: "hint", "Detecting…" }
                        },
                        Some(report) => rsx! {
                            for section in report.sections.iter() {
                                div { class: "settings-section", "{section.title}" }
                                for row in section.rows.iter() {
                                    div { class: "compat-row",
                                        span { class: "label", "{row.label}" }
                                        span { class: "compat-state {row.state.css()}", "{row.state.label()}" }
                                        span { class: "compat-detail", "{row.detail}" }
                                    }
                                }
                            }
                        },
                    }
                }
                div { class: "row modal-actions",
                    span { class: "hint", "Read-only probe — nothing is transcoded or uploaded" }
                    button {
                        class: "btn",
                        // 探测期间禁用：连点会让两批探测并发（结果顺序不定）
                        disabled: probing(),
                        onclick: move |_| {
                            if !probing() {
                                nonce += 1;
                            }
                        },
                        if probing() { "Re-checking…" } else { "Re-check" }
                    }
                }
            }
        }
    }
}
