//! 设置面板：模态对话框，改动即时生效并即时落盘。
//!
//! 覆盖设置项：输出目录、重试次数、视频/图片大小上限、重试缩放系数、
//! 时长→系数表、强制帧率、引擎（桌面）。主题三选一在工具栏 ⋯ 菜单（低频动作
//! 归溢出菜单——面板里那份是重复控件，已删）。
//! 码率基准 / ffmpeg 路径 / 语言 / 并行数为规划中的可选项，暂不开放。

use crate::app::UiState;
use crate::components::number_field::NumberInput;
#[cfg(not(target_arch = "wasm32"))]
use crate::config::OutputDirState;
use crate::config::Settings;
use dioxus::prelude::*;

/// 时长→系数表的固定区间标签，与 `Settings::duration_factors` 下标对应。
const FACTOR_BANDS: [&str; 6] = ["<1s", "<2s", "<3s", "<5s", "<8s", "\u{2265}8s"];

#[component]
pub fn SettingsPanel() -> Element {
    let mut ctx = use_context::<UiState>();
    // Reset Defaults 的二次确认态（4s 无第二次点击自动解除）
    let mut reset_armed = use_signal(|| false);
    if !ctx.show_settings.cloned() {
        return rsx! {};
    }
    let factors = ctx.settings.read().duration_factors;

    // sidecar 可用性：启动探测缓存；不可用时下拉项置灰并显示后缀。
    // 可用但缺 libvpx-vp9 编码器（极简 ffmpeg 构建）同样视为不可用于转码。
    #[cfg(not(target_arch = "wasm32"))]
    let (sidecar_available, unavailable_suffix) = {
        let probe = crate::sidecar_probe::SidecarProbe::probe();
        let available = probe.is_some_and(|p| p.has_vp9);
        let suffix = if available {
            ""
        } else {
            match probe {
                Some(_) => " — missing libvpx-vp9 encoder",
                None => " — ffmpeg not detected",
            }
        };
        (available, suffix)
    };
    // 引擎下拉仅桌面有意义：web 的引擎矩阵即策略（Engine::for_web），用户选不了
    // 也不该在这看——路由信息 Compatibility Report 已完整呈现。面板行整块不渲染。
    #[cfg(not(target_arch = "wasm32"))]
    let engine_section = rsx! {
        div { class: "settings-section", "Engine" }
        div { class: "settings-row",
            span { class: "label", "Engine" }
            select {
                class: "input",
                value: "{ctx.settings.read().engine.as_str()}",
                onchange: move |evt: Event<FormData>| {
                    let engine = evt.data.value();
                    // 选项固定合法；parse 失败（手改 DOM）兜底默认值
                    ctx.update_settings(move |s| {
                        s.engine = crate::transcoder::Engine::parse(&engine)
                            .unwrap_or_default()
                    });
                },
                option {
                    value: "sidecar",
                    disabled: !sidecar_available,
                    "Sidecar (ffmpeg.exe){unavailable_suffix}"
                }
                option { value: "inprocess", "In-process (libav)" }
                option {
                    value: "webcodecs",
                    disabled: true,
                    "WebCodecs (web only)"
                }
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    let engine_section = rsx! {};

    // 输出目录仅桌面有意义（web 产物驻内存走下载）；rsx 元素级 #[cfg] 不被
    // 解析，按 engine_select 的 let 双分支先例处理。
    #[cfg(not(target_arch = "wasm32"))]
    let output_dir_row = rsx! {
        div { class: "settings-row",
            span { class: "label", "Save to" }
            // 与工具栏同一套：选择按钮做进输入框内，面板里也保持一致
            div { class: "dir-field",
                input {
                    class: if ctx.settings.read().output_dir_state() == OutputDirState::Missing {
                        "input grow invalid-dir"
                    } else {
                        "input grow"
                    },
                    r#type: "text",
                    "aria-label": "Output folder",
                    value: "{ctx.settings.read().output_dir}",
                    oninput: move |evt: Event<FormData>| {
                        let value = evt.data.value();
                        ctx.update_settings(move |s| s.output_dir = value);
                    },
                }
                button {
                    class: "btn icon dir-pick",
                    title: "Choose output folder",
                    "aria-label": "Choose output folder",
                    onclick: move |_| ctx.pick_output_dir(),
                    crate::components::icon::IconFolder {}
                }
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    let output_dir_row = rsx! {};

    rsx! {
        div {
            class: "modal-backdrop",
            tabindex: 0,
            role: "dialog",
            "aria-label": "Settings",
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
                class: "modal modal-settings",
                onclick: move |evt: Event<MouseData>| evt.stop_propagation(),
                // 标题固定在顶、只有正文滚动；关闭收进右上角的 X（原底部 Close 按钮）
                div { class: "modal-head",
                    h2 { class: "modal-title", "Settings" }
                    button {
                        class: "btn icon",
                        title: "Close",
                        "aria-label": "Close settings",
                        onclick: move |_| ctx.show_settings.set(false),
                        crate::components::icon::IconClose {}
                    }
                }
                div { class: "modal-body",
                    div { class: "settings-section", "Output" }
                    {output_dir_row}

                div { class: "settings-row",
                    span { class: "label", "Keep input file name" }
                    input {
                        class: "input check",
                        r#type: "checkbox",
                        disabled: false,
                        checked: ctx.settings.read().keep_input_name,
                        onchange: move |evt: Event<FormData>| {
                            let on = evt.data.checked();
                            ctx.update_settings(move |s| s.keep_input_name = on);
                        },
                    }
                    span { class: "hint", "Output: input-name-timestamp.webm (off = timestamp only)" }
                }

                div { class: "settings-section", "Size limits & retries" }

                div { class: "settings-row",
                    span { class: "label", "Video max size (KB)" }
                    NumberInput<u32> {
                        value: ctx.settings.read().video_max_size_kb,
                        min: 16u32,
                        max: 102400u32,
                        disabled: false,
                        on_change: move |v| ctx.update_settings(move |s| s.video_max_size_kb = v),
                    }
                    span { class: "hint", "Telegram's own limit is 256 KB" }
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
                    span { class: "hint", "Telegram's own limit is 512 KB" }
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
                    span { class: "hint", "Extra attempts while the result is still over the limit" }
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
                    span { class: "hint", "factor ÷ excess × this, applied per retry" }
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
                    span { class: "hint", "Starting size factor by clip length; GIF gets ×0.75 on top" }
                }

                div { class: "settings-row",
                    span { class: "label", "Patch webm duration" }
                    input {
                        class: "input check",
                        r#type: "checkbox",
                        disabled: false,
                        checked: ctx.settings.read().webm_duration_patch,
                        onchange: move |evt: Event<FormData>| {
                            let on = evt.data.checked();
                            ctx.update_settings(move |s| s.webm_duration_patch = on);
                        },
                    }
                    span { class: "hint", "Reports a 100 ms duration so Telegram's length check passes; off keeps the encoder's value" }
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
                    span { class: "hint", "0 keeps the source frame rate" }
                }

                {engine_section}

                div { class: "row modal-actions",
                    button {
                        class: if reset_armed() { "btn btn-danger" } else { "btn" },
                        // 一键清空全部调参且立即落盘、无撤销——要求二次确认
                        onclick: move |_| {
                            if !reset_armed() {
                                reset_armed.set(true);
                                spawn(async move {
                                    crate::timers::sleep(std::time::Duration::from_secs(4)).await;
                                    reset_armed.set(false);
                                });
                                return;
                            }
                            reset_armed.set(false);
                            // 恢复默认值但保留用户已选择的输出目录
                            ctx.update_settings(|s| {
                                let dir = s.output_dir.clone();
                                *s = Settings::default();
                                s.output_dir = dir;
                            });
                        },
                        if reset_armed() { "Confirm reset?" } else { "Reset Defaults" }
                    }
                }
                }
            }
        }
    }
}
