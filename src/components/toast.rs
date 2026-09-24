//! Toast 通知：右下角堆叠，自动消失。
//!
//! 由 `UiState::push_toast` 入队（含 4 秒自动移除），
//! `ToastContainer` 只负责渲染。

use crate::app::UiState;
use dioxus::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: String,
    /// 消失前的渐出状态（由 push_toast 的定时器置位）。
    pub leaving: bool,
    /// 带撤销按钮（删除任务的回执）。
    pub undo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToastKind {
    Success,
    Error,
    Warn,
    Info,
}

impl ToastKind {
    fn class(self) -> &'static str {
        match self {
            ToastKind::Success => "toast success",
            ToastKind::Error => "toast error",
            ToastKind::Warn => "toast warn",
            ToastKind::Info => "toast info",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ToastKind::Success => "Done",
            ToastKind::Error => "Error",
            ToastKind::Warn => "Warning",
            ToastKind::Info => "Info",
        }
    }
}

#[component]
pub fn ToastContainer() -> Element {
    let mut ctx = use_context::<UiState>();
    let toasts = ctx.toasts.cloned();

    rsx! {
        div { class: "toast-container", "aria-live": "polite",
            for toast in toasts {
                div {
                    key: "{toast.id}",
                    class: if toast.leaving { "{toast.kind.class()} leaving" } else { "{toast.kind.class()}" },
                    span { class: "toast-kind", "[{toast.kind.label()}]" }
                    span { class: "toast-text", "{toast.text}" }
                    if toast.undo {
                        button {
                            class: "btn btn-mini toast-undo",
                            // 运行中禁用：undo_remove 同一守卫的可见反馈（保槽，跑完可再点）
                            disabled: ctx.running.cloned(),
                            onclick: move |_| ctx.undo_remove(toast.id),
                            "Undo"
                        }
                    }
                }
            }
        }
    }
}
