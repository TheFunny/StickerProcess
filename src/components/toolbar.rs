//! 工具栏：添加文件、运行/取消、设置入口 + "⋯" 溢出菜单（清空完成、下载全部、主题）。
//!
//! 改动背景（批次二实验）：按钮一多，640px 最小窗口就满了（7 个文字按钮实测只剩
//! ~10px 余量）。低频动作（清空完成 / 下载全部 / 主题三档）收进溢出菜单，主题由
//! "循环切换"改为"三选一 + 当前项打勾"——循环按钮看不出有哪几档，也看不出当前在
//! 哪一档；工具栏只留入口（Add File）、绝对主操作（Run）、以及成对出现的 Cancel。
//!
//! Phase B：输出目录直接读写 `Settings`（即时同步落盘）；重试次数移入设置面板。
//! web：输出目录行整体不渲染（产物驻内存走下载）。

use crate::app::{SUPPORTED, UiState};
#[cfg(not(target_arch = "wasm32"))]
use crate::config::OutputDirState;
use crate::transcoder::Status;
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

/// 主题三档（溢出菜单里的单选项）。
const THEMES: [(&str, &str); 3] = [
    ("system", "System (follow OS)"),
    ("light", "Light"),
    ("dark", "Dark"),
];

#[component]
pub fn Toolbar() -> Element {
    let mut ctx = use_context::<UiState>();
    let running = ctx.running.cloned();
    let mut menu_open = use_signal(|| false);
    let theme = ctx.settings.read().theme.clone();
    // 不 clone 任务列表：只读一遍算两个标志（工具栏会随 tasks 信号重渲染）
    let (has_done, runnable) = {
        let list = ctx.tasks.read();
        (
            list.iter().any(|e| e.mirror.status == Status::Done),
            list.iter().any(|e| e.mirror.status != Status::Done),
        )
    };

    // web：产物驻内存，一键顺序下载全部 Done（400ms 间隔防浏览器多文件拦截）
    #[cfg(target_arch = "wasm32")]
    let extra_btn = rsx! {
        button {
            class: "menu-item",
            disabled: running || !has_done,
            onclick: move |_| {
                menu_open.set(false);
                ctx.download_all();
            },
            "Download All"
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let extra_btn = rsx! {};

    // 输出目录状态（桌面）：红框与提示文案都从它派生；可写性只在失焦时真写一次
    #[cfg(not(target_arch = "wasm32"))]
    let mut dir_write_ok = use_signal(|| None::<bool>);
    #[cfg(not(target_arch = "wasm32"))]
    let dir_state = ctx.settings.read().output_dir_state();
    #[cfg(not(target_arch = "wasm32"))]
    let (dir_bad, dir_hint) = match dir_state {
        OutputDirState::Missing => (true, Some(("Folder not found", " danger"))),
        // 存在但是写不进去（只读/ACL）：以前会一路通过校验、跑到写盘才炸
        OutputDirState::Ok if dir_write_ok() == Some(false) => {
            (true, Some(("Folder is not writable", " danger")))
        }
        OutputDirState::WillCreate => (false, Some(("Will be created when you run", ""))),
        OutputDirState::Ok => (false, None),
    };

    // 输出目录仅桌面有意义（web 无文件系统），整行 let 双分支。
    #[cfg(not(target_arch = "wasm32"))]
    let output_row = rsx! {
        div { class: "row",
            span { class: "label", "Save to" }
            // 选择按钮做成输入框内的后缀图标；"打开目录"放在本行最右
            div { class: "dir-field",
                input {
                    class: if dir_bad {
                        "input grow invalid-dir"
                    } else {
                        "input grow"
                    },
                    r#type: "text",
                    "aria-label": "Output folder",
                    placeholder: "Type output directory here",
                    value: "{ctx.settings.read().output_dir}",
                    disabled: running,
                    oninput: move |evt: Event<FormData>| {
                        let value = evt.data.value();
                        ctx.update_settings(move |s| s.output_dir = value);
                    },
                    onblur: move |_| {
                        // 真写一个临时文件才算数；有副作用，故只挂失焦而不是每键
                        let ok = ctx.settings.peek().output_dir_writable();
                        dir_write_ok.set(Some(ok));
                    },
                }
                button {
                    class: "btn icon dir-pick",
                    title: "Choose output folder",
                    "aria-label": "Choose output folder",
                    disabled: running,
                    onclick: move |_| ctx.pick_output_dir(),
                    crate::components::icon::IconFolder {}
                }
            }
            button {
                class: "btn icon",
                title: "Open output folder in Explorer",
                "aria-label": "Open output folder",
                // 目录还不存在时不可点（Explorer 会报错），与"将自动创建"区分开
                disabled: dir_state != OutputDirState::Ok,
                onclick: move |_| {
                    // UiState 是 Copy：拿一份局部可变绑定去调 &mut 方法
                    let mut ctx = ctx;
                    ctx.open_output_dir();
                },
                crate::components::icon::IconOpenExternal {}
            }
        }
        if let Some((text, cls)) = dir_hint {
            div { class: "hint{cls}", "{text}" }
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
            div { class: "spacer" }
            button {
                class: "btn icon",
                title: "Settings",
                "aria-label": "Settings",
                disabled: running,
                onclick: move |_| ctx.show_settings.set(true),
                crate::components::icon::IconSettings {}
            }
            // Run 与 Cancel 是同一处状态机：进行中显示 Cancel，否则显示 Run
            if running {
                button {
                    class: "btn btn-danger run-slot",
                    title: "Cancel the running job",
                    onclick: move |_| ctx.cancel.set(true),
                    "Cancel"
                }
            } else {
                button {
                    class: "btn btn-primary run-slot",
                    title: "Transcode every task that is not done yet",
                    disabled: !runnable,
                    onclick: move |_| ctx.start_run(),
                    "Run"
                }
            }
            div { class: "menu-wrap",
                button {
                    class: "btn icon",
                    title: "More actions",
                    "aria-label": "More actions",
                    "aria-expanded": if menu_open() { "true" } else { "false" },
                    onclick: move |_| menu_open.set(!menu_open()),
                    "⋯"
                }
                if menu_open() {
                    // 点击外部关闭（透明满屏捕获层）
                    div { class: "menu-catch", onclick: move |_| menu_open.set(false) }
                    div { class: "menu",
                        button {
                            class: "menu-item",
                            disabled: running || !has_done,
                            onclick: move |_| {
                                menu_open.set(false);
                                ctx.clear_done();
                            },
                            "Clear Done"
                        }
                        {extra_btn}
                        div { class: "menu-sep" }
                        // 主题：三选一（原先是一个看不出档位的循环按钮）
                        for (value, label) in THEMES {
                            button {
                                class: "menu-item",
                                onclick: move |_| {
                                    menu_open.set(false);
                                    ctx.update_settings(move |s| s.theme = value.to_string());
                                },
                                span { "{label}" }
                                if theme == value {
                                    span { class: "menu-check", "✓" }
                                }
                            }
                        }
                    }
                }
            }
        }
        {output_row}
    }
}
