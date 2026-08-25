//! 数值输入组件（替代 iced_aw 的 `NumberInput`）。
//!
//! 文本框 + 宽松解析：键入过程中解析失败则不回传。
//! 显示采用"编辑态"模式：聚焦时显示本地草稿（避免光标跳动），
//! 失焦时直接镜像外部值——外部变化（如超限重试自动缩小系数）立即反映到 UI。

use dioxus::prelude::*;

/// 把 f64 格式化为最多 3 位小数、去掉尾随 0 的字符串。
fn fmt_f64(v: f64) -> String {
    let s = format!("{v:.3}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[component]
pub fn U8Input(
    value: u8,
    min: u8,
    max: u8,
    disabled: bool,
    on_change: EventHandler<u8>,
) -> Element {
    let mut draft = use_signal(String::new);
    let mut editing = use_signal(|| false);

    rsx! {
        input {
            class: "input num",
            r#type: "text",
            disabled,
            value: if editing() { draft.cloned() } else { value.to_string() },
            onfocus: move |_| {
                editing.set(true);
                draft.set(value.to_string());
            },
            onblur: move |_| editing.set(false),
            oninput: move |evt: Event<FormData>| {
                let raw = evt.data.value();
                draft.set(raw.clone());
                if let Ok(v) = raw.trim().parse::<u8>() {
                    on_change.call(v.clamp(min, max));
                }
            },
        }
    }
}

#[component]
pub fn F64Input(
    value: f64,
    min: f64,
    max: f64,
    disabled: bool,
    on_change: EventHandler<f64>,
) -> Element {
    let mut draft = use_signal(String::new);
    let mut editing = use_signal(|| false);

    rsx! {
        input {
            class: "input num",
            r#type: "text",
            disabled,
            value: if editing() { draft.cloned() } else { fmt_f64(value) },
            onfocus: move |_| {
                editing.set(true);
                draft.set(fmt_f64(value));
            },
            onblur: move |_| editing.set(false),
            oninput: move |evt: Event<FormData>| {
                let raw = evt.data.value();
                draft.set(raw.clone());
                if let Ok(v) = raw.trim().parse::<f64>() {
                    on_change.call(v.clamp(min, max));
                }
            },
        }
    }
}
