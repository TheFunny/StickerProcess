//! 工具栏：添加文件、清空完成、设置入口、主题切换、取消、运行按钮。
//!
//! Phase B：输出目录直接读写 `Settings`（防抖落盘）；重试次数移入设置面板。

use crate::app::{SUPPORTED, UiState};
use dioxus::prelude::*;
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
    let theme_label = if ctx.settings.read().theme == "dark" {
        "Light"
    } else {
        "Dark"
    };

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
                        let files: Vec<PathBuf> = evt.data.files().iter().map(|f| f.path()).collect();
                        if !files.is_empty() {
                            ctx.add_files(files);
                        }
                    },
                }
            }
            button {
                class: "btn",
                disabled: running,
                onclick: move |_| ctx.clear_done(),
                "Clear Done"
            }
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
                    ctx.update_settings(|s| s.theme = if s.theme == "dark" { "light".into() } else { "dark".into() });
                },
                "{theme_label}"
            }
            button {
                class: "btn btn-danger",
                disabled: !running,
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

        div { class: "row",
            span { class: "label", "Output Dir:" }
            input {
                class: "input grow",
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
    }
}
