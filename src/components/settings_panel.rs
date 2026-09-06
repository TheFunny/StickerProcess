//! 设置面板：模态对话框，改动即时生效并即时落盘。
//!
//! 覆盖设置项：输出目录、重试次数、视频/图片大小上限、重试缩放系数、
//! 时长→系数表、强制帧率、主题（System/Light/Dark）。
//! 码率基准 / ffmpeg 路径 / 语言 / 并行数为规划中的可选项，暂不开放。

use crate::app::UiState;
use crate::components::number_field::NumberInput;
use crate::config::Settings;
use dioxus::prelude::*;

/// 时长→系数表的固定区间标签，与 `Settings::duration_factors` 下标对应。
const FACTOR_BANDS: [&str; 6] = ["<1s", "<2s", "<3s", "<5s", "<8s", "\u{2265}8s"];

#[component]
pub fn SettingsPanel() -> Element {
    let mut ctx = use_context::<UiState>();
    if !ctx.show_settings.cloned() {
        return rsx! {};
    }
    let factors = ctx.settings.read().duration_factors;

    // sidecar 可用性：启动探测缓存；不可用时下拉项置灰并显示后缀。
    // 可用但缺 libvpx-vp9 编码器（极简 ffmpeg 构建）同样视为不可用于转码。
    let probe = crate::sidecar_probe::SidecarProbe::probe();
    let sidecar_available = probe.is_some_and(|p| p.has_vp9);
    let unavailable_suffix = if sidecar_available {
        ""
    } else {
        match probe {
            Some(_) => " — 缺少 libvpx-vp9 编码器",
            None => " — 未检测到 ffmpeg",
        }
    };

    rsx! {
        div {
            class: "modal-backdrop",
            tabindex: 0,
            onmounted: move |evt: Event<MountedData>| {
                spawn(async move {
                    let _ = evt.data.set_focus(true).await;
                });
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                if evt.data.key() == dioxus::html::Key::Escape {
                    ctx.show_settings.set(false);
                }
            },
            onclick: move |_| ctx.show_settings.set(false),
            div {
                class: "modal",
                onclick: move |evt: Event<MouseData>| evt.stop_propagation(),
                h2 { class: "modal-title", "Settings" }

                div { class: "settings-row",
                    span { class: "label", "Output Dir" }
                    input {
                        class: if ctx.settings.read().output_dir_valid() {
                            "input grow"
                        } else {
                            "input grow invalid-dir"
                        },
                        r#type: "text",
                        value: "{ctx.settings.read().output_dir}",
                        oninput: move |evt: Event<FormData>| {
                            let value = evt.data.value();
                            ctx.update_settings(move |s| s.output_dir = value);
                        },
                    }
                    button {
                        class: "btn",
                        onclick: move |_| ctx.pick_output_dir(),
                        "Select"
                    }
                }

                div { class: "settings-row",
                    span { class: "label", "Max retry" }
                    NumberInput<u8> {
                        value: ctx.settings.read().max_retry,
                        min: 0u8,
                        max: 10u8,
                        disabled: false,
                        on_change: move |v| ctx.update_settings(move |s| s.max_retry = v),
                    }
                }

                div { class: "settings-row",
                    span { class: "label", "Video max size (KB)" }
                    NumberInput<u32> {
                        value: ctx.settings.read().video_max_size_kb,
                        min: 16u32,
                        max: 102400u32,
                        disabled: false,
                        on_change: move |v| ctx.update_settings(move |s| s.video_max_size_kb = v),
                    }
                }

                div { class: "settings-row",
                    span { class: "label", "Image max size (KB)" }
                    NumberInput<u32> {
                        value: ctx.settings.read().image_max_size_kb,
                        min: 16u32,
                        max: 102400u32,
                        disabled: false,
                        on_change: move |v| ctx.update_settings(move |s| s.image_max_size_kb = v),
                    }
                }

                div { class: "settings-row",
                    span { class: "label", "Retry shrink factor" }
                    NumberInput<f64> {
                        value: ctx.settings.read().retry_shrink_factor,
                        min: 0.05,
                        max: 1.0,
                        disabled: false,
                        on_change: move |v| ctx.update_settings(move |s| s.retry_shrink_factor = v),
                    }
                }

                div { class: "settings-group",
                    span { class: "label", "Duration factors (bitrate patch)" }
                    div { class: "factor-grid",
                        for (index, band) in FACTOR_BANDS.iter().enumerate() {
                            label { class: "factor-cell",
                                span { class: "label", "{band}" }
                                NumberInput<f64> {
                                    key: "{band}",
                                    value: factors[index],
                                    min: 0.05,
                                    max: 10.0,
                                    disabled: false,
                                    on_change: move |v| {
                                        ctx.update_settings(move |s| s.duration_factors[index] = v);
                                    },
                                }
                            }
                        }
                    }
                }

                div { class: "settings-row",
                    span { class: "label", "Force FPS (0 = auto)" }
                    NumberInput<f64> {
                        value: ctx.settings.read().target_fps,
                        min: 0.0,
                        max: 240.0,
                        disabled: false,
                        on_change: move |v| ctx.update_settings(move |s| s.target_fps = v),
                    }
                }

                div { class: "settings-row",
                    span { class: "label", "Transcode engine" }
                    select {
                        class: "input",
                        value: "{ctx.settings.read().engine}",
                        onchange: move |evt: Event<FormData>| {
                            let engine = evt.data.value();
                            ctx.update_settings(move |s| s.engine = engine);
                        },
                        option {
                            value: "sidecar",
                            disabled: !sidecar_available,
                            "Sidecar (ffmpeg.exe){unavailable_suffix}"
                        }
                        option { value: "inprocess", "In-process (libav)" }
                    }
                }
                div { class: "settings-row",
                    span { class: "label", "Theme" }
                    select {
                        class: "input",
                        value: "{ctx.settings.read().theme}",
                        onchange: move |evt: Event<FormData>| {
                            let theme = evt.data.value();
                            ctx.update_settings(move |s| s.theme = theme);
                        },
                        option { value: "system", "System (follow OS)" }
                        option { value: "light", "Light" }
                        option { value: "dark", "Dark" }
                    }
                }

                div { class: "row modal-actions",
                    button {
                        class: "btn",
                        onclick: move |_| {
                            // 恢复默认值但保留用户已选择的输出目录
                            ctx.update_settings(|s| {
                                let dir = s.output_dir.clone();
                                *s = Settings::default();
                                s.output_dir = dir;
                            });
                        },
                        "Reset Defaults"
                    }
                    div { class: "spacer" }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| ctx.show_settings.set(false),
                        "Close"
                    }
                }
            }
        }
    }
}
