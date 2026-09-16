//! 工具栏：添加文件、清空完成、设置入口、主题切换、取消、运行按钮。
//!
//! Phase B：输出目录直接读写 `Settings`（即时同步落盘）；重试次数移入设置面板。
//! web：输出目录行整体不渲染（产物驻内存走下载）；新增 Download All。

use crate::app::{SUPPORTED, UiState};
use dioxus::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

/// 文件选择器 accept 属性，从 SUPPORTED 常量派生（避免手写漂移）。
fn file_accept() -> String {
    SUPPORTED
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[component]
pub fn Toolbar() -> Element {
    let mut ctx = use_context::<UiState>();
    let running = ctx.running.cloned();
    // 主题按钮显示当前档位（与设置面板/实际呈现一致），点击切换到下一档
    let theme_label = match ctx.settings.read().theme.as_str() {
        "system" => "System",
        "light" => "Light",
        _ => "Dark",
    };

    /// 主题三档循环。
    fn next_theme(current: &str) -> String {
        match current {
            "system" => "light",
            "light" => "dark",
            _ => "system",
        }
        .into()
    }

    // web：产物驻内存，逐个点下载太累——一键顺序下载全部 Done（400ms 间隔
    // 防浏览器多文件拦截，首次弹"允许"）。rsx 元素级 #[cfg] 不被解析，
    // 按 settings_panel engine_select 的 let 双分支先例处理。
    #[cfg(target_arch = "wasm32")]
    let extra_btn = rsx! {
        button {
            class: "btn",
            disabled: running
                || !ctx
                    .tasks
                    .read()
                    .iter()
                    .any(|e| e.status == crate::transcoder::Status::Done),
            onclick: move |_| ctx.download_all(),
            "Download All"
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let extra_btn = rsx! {};

    // 输出目录仅桌面有意义（web 无文件系统），整行 let 双分支。
    #[cfg(not(target_arch = "wasm32"))]
    let output_row = rsx! {
        div { class: "row",
            span { class: "label", "Output Dir:" }
            input {
                class: if ctx.settings.read().output_dir_valid() {
                    "input grow"
                } else {
                    "input grow invalid-dir"
                },
                r#type: "text",
                placeholder: "Type output directory here",
                value: "{ctx.settings.read().output_dir}",
                disabled: running,
                oninput: move |evt: Event<FormData>| {
                    let value = evt.data.value();
                    ctx.update_settings(move |s| s.output_dir = value);
                },
            }
            button {
                class: "btn",
                disabled: running,
                onclick: move |_| ctx.pick_output_dir(),
                "Select"
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    let output_row = rsx! {};

    rsx! {
        div { class: "row",
            label {
                class: if running { "btn disabled" } else { "btn" },
                "Add File"
                input {
                    id: "add-file-input",
                    style: "display: none",
                    r#type: "file",
                    multiple: true,
                    accept: "{file_accept()}",
                    onchange: move |evt: Event<FormData>| {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let files: Vec<PathBuf> =
                                evt.data.files().iter().map(|f| f.path()).collect();
                            if !files.is_empty() {
                                ctx.add_files(files);
                            }
                        }
                        #[cfg(target_arch = "wasm32")]
                        {
                            for f in evt.data.files() {
                                spawn(async move {
                                    let name = f.name();
                                    if let Ok(bytes) = f.read_bytes().await {
                                        ctx.add_file_bytes(name, bytes.to_vec());
                                    }
                                });
                            }
                        }
                        // 必须清空 value：input 保留上次选择，浏览器只在选中的
                        // 列表与 value 不同时才再触发 change —— 删掉任务后重选
                        // 同一文件会静默失败
                        let _ = document::eval(
                            "document.getElementById('add-file-input').value = ''",
                        );
                    },
                }
            }
            button {
                class: "btn",
                disabled: running,
                onclick: move |_| ctx.clear_done(),
                "Clear Done"
            }
            {extra_btn}
            button {
                class: "btn",
                disabled: running,
                onclick: move |_| ctx.show_settings.set(true),
                "Settings"
            }
            div { class: "spacer" }
            button {
                class: "btn",
                onclick: move |_| {
                    ctx.update_settings(|s| s.theme = next_theme(&s.theme));
                },
                "{theme_label}"
            }
            button {
                class: "btn btn-danger",
                disabled: running || ctx.tasks.read().is_empty(),
                onclick: move |_| ctx.cancel.set(true),
                "Cancel"
            }
            button {
                class: "btn btn-primary",
                disabled: running || ctx.tasks.cloned().is_empty(),
                onclick: move |_| ctx.start_run(),
                "Run"
            }
        }
        {output_row}
    }
}
