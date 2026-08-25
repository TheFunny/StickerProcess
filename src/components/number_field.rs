//! 数值输入组件（替代 iced_aw 的 `NumberInput`）。
//!
//! 文本框 + 宽松解析：键入过程中解析失败则不回传；
//! 外部值变化（如运行后系数自动缩小）时通过 use_effect 回写显示。

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
    let mut text = use_signal(|| value.to_string());

    use_effect(move || {
        if text.peek().trim().parse::<u8>() != Ok(value) {
            text.set(value.to_string());
        }
    });

    rsx! {
        input {
            class: "input num",
            r#type: "text",
            disabled,
            value: "{text}",
            oninput: move |evt: Event<FormData>| {
                let raw = evt.data.value();
                text.set(raw.clone());
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
    let mut text = use_signal(move || fmt_f64(value));

    use_effect(move || {
        let differs = match text.cloned().trim().parse::<f64>() {
            Ok(current) => (current - value).abs() >= f64::EPSILON * 4.0,
            Err(_) => true,
        };
        if differs {
            text.set(fmt_f64(value));
        }
    });

    rsx! {
        input {
            class: "input num",
            r#type: "text",
            disabled,
            value: "{text}",
            oninput: move |evt: Event<FormData>| {
                let raw = evt.data.value();
                text.set(raw.clone());
                if let Ok(v) = raw.trim().parse::<f64>() {
                    on_change.call(v.clamp(min, max));
                }
            },
        }
    }
}
