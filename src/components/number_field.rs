//! 数值输入组件（泛型统一 u8 / u32 / f64）。
//!
//! 即时提交语义：每次有效键入立即解析并回调 on_change（factor 只在
//! gen_command 时被读取，中间值无害）；聚焦时显示本地草稿避免光标跳动，
//! 失焦/重渲染后非编辑态直接镜像外部值——重试自动缩放等外部变化立即可见。
//!
//! 键入期间的中间值提交只影响 Transcoder，不影响显示（显示走草稿），
//! 因此不存在"提交后跳回旧值"的窗口。

use dioxus::prelude::*;

/// 数值输入支持的类型约束。`pretty` 控制非编辑态的显示格式
/// （浮点去尾零，整数原样）。
pub trait NumEdit: Copy + PartialOrd + std::fmt::Display + std::str::FromStr {
    /// 解析失败时返回 None 的类型化包装（FromStr::Err 不统一，这里抹平）。
    fn parse_num(s: &str) -> Option<Self>;
    fn clamp_to(self, min: Self, max: Self) -> Self;
    fn pretty(self) -> String;
}

macro_rules! impl_num_edit_int {
    ($t:ty) => {
        impl NumEdit for $t {
            fn parse_num(s: &str) -> Option<Self> {
                s.trim().parse::<$t>().ok()
            }
            fn clamp_to(self, min: Self, max: Self) -> Self {
                self.clamp(min, max)
            }
            fn pretty(self) -> String {
                self.to_string()
            }
        }
    };
}

impl_num_edit_int!(u8);
impl_num_edit_int!(u32);

impl NumEdit for f64 {
    fn parse_num(s: &str) -> Option<Self> {
        s.trim().parse::<f64>().ok()
    }
    fn clamp_to(self, min: Self, max: Self) -> Self {
        self.clamp(min, max)
    }
    fn pretty(self) -> String {
        // 最多 3 位小数、去掉尾随 0
        let s = format!("{self:.3}");
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    }
}

#[component]
pub fn NumberInput<T: NumEdit + 'static>(
    value: T,
    min: T,
    max: T,
    disabled: bool,
    on_change: EventHandler<T>,
) -> Element {
    let mut draft = use_signal(String::new);
    let mut editing = use_signal(|| false);

    rsx! {
        input {
            class: "input num",
            r#type: "text",
            disabled,
            value: if editing() { draft.cloned() } else { value.pretty() },
            onfocus: move |_| {
                editing.set(true);
                draft.set(value.pretty());
            },
            onblur: move |_| editing.set(false),
            oninput: move |evt: Event<FormData>| {
                let raw = evt.data.value();
                draft.set(raw.clone());
                // 有效键入立即提交；解析失败（如空串、"0."）不回传
                if let Some(v) = T::parse_num(&raw) {
                    on_change.call(v.clamp_to(min, max));
                }
            },
        }
    }
}
