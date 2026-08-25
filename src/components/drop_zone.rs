//! 拖拽区（整窗接收）。
//!
//! 已验证路径（spike/dioxus-dnd-demo, Dioxus 0.7.10）：
//! Windows 下 `ondrop` → `evt.data.files()` → `FileData::path()` 返回完整路径；
//! 悬停高亮挂 `ondragover`（Windows 不触发 dragenter）。

use crate::app::UiState;
use dioxus::html::HasFileData;
use dioxus::prelude::*;
use std::path::PathBuf;

#[component]
pub fn DropZone(children: Element) -> Element {
    let mut dragging = use_signal(|| false);
    let mut ctx = use_context::<UiState>();

    rsx! {
        div {
            class: "app",
            ondragover: move |evt: Event<DragData>| {
                // 文字选择拖动不带文件（files 由 wry 原生文件拖拽合成），不触发覆盖层
                if !dragging() && !evt.data.files().is_empty() {
                    dragging.set(true);
                }
            },
            ondragleave: move |_| dragging.set(false),
            ondrop: move |evt: Event<DragData>| {
                dragging.set(false);
                let files: Vec<PathBuf> = evt.data.files().iter().map(|f| f.path()).collect();
                // 与文件选择一致：运行中也允许追加（沿用 iced 订阅语义）
                if !files.is_empty() {
                    ctx.add_files(files);
                }
            },
            {children}
            if dragging() {
                div { class: "drop-overlay", "Release to add files" }
            }
        }
    }
}
