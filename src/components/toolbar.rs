//! 工具栏：添加文件、清空完成、输出目录、重试次数、运行按钮。
//!
//! 与 iced 版对等：运行期间禁用全部编辑入口；
//! "Add File" 用隐藏的 `<input type="file" multiple>`（label 触发原生对话框）。

use crate::app::UiState;
use crate::components::number_field::U8Input;
use dioxus::prelude::*;
use std::path::PathBuf;

const FILE_ACCEPT: &str = ".mp4,.gif,.apng,.jpg,.jpeg,.png";

#[component]
pub fn Toolbar() -> Element {
    let mut ctx = use_context::<UiState>();
    let running = ctx.running.cloned();

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
                    accept: FILE_ACCEPT,
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
            div { class: "spacer" }
            span { class: "label", "Retry times:" }
            U8Input {
                value: *ctx.max_retry.read(),
                min: 0u8,
                max: 10u8,
                disabled: running,
                on_change: move |v| ctx.max_retry.set(v),
            }
            button {
                class: "btn btn-primary",
                disabled: running || ctx.tasks.cloned().is_empty(),
                onclick: move |_| ctx.start_run(),
                "Run"
            }
            button {
                class: "btn",
                disabled: running,
                onclick: move |_| ctx.theme.set(ctx.theme.cloned().toggled()),
                "{ctx.theme.cloned().label()}"
            }
            button {
                class: "btn btn-danger",
                disabled: !running,
                onclick: move |_| ctx.cancel.set(true),
                "Cancel"
            }
        }

        div { class: "row",
            span { class: "label", "Output Dir:" }
            input {
                class: "input grow",
                r#type: "text",
                placeholder: "Type output directory here",
                value: "{ctx.output_dir.read()}",
                disabled: running,
                oninput: move |evt: Event<FormData>| ctx.output_dir.set(evt.data.value()),
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
