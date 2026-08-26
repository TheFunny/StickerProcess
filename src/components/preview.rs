//! 预览弹窗（Phase D 双轨制）：
//!
//! - 输入（可能很大）：走 `preview://` 自定义协议，按需流式读盘
//! - 输出（≤512KB 贴纸）：base64 data URL，转码完成后自动刷新
//!
//! 点击任务行打开；并排展示 原始 vs 输出 与大小对比。
//!
//! 注意：所有 hooks 必须在早退之前无条件调用（dioxus hook 顺序约束）；
//! 输出编码的 effect 必须在闭包内读取 `tasks` 信号建立依赖
//! （dioxus 0.7 的 effect 只因内部读取的信号变化而重跑），并以
//! (输出路径, 大小) 为键去重，避免转码期间进度高频更新导致重复编码。

use crate::app::{TaskEntry, UiState};
use crate::preview;
use dioxus::prelude::*;
use std::path::Path;

/// 输入侧按真实容器选择元素：gif/apng 是动画图片，浏览器不支持在
/// `<video>` 中解码它们。
fn input_is_video(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
        == Some("mp4")
}

#[component]
pub fn PreviewModal() -> Element {
    let mut ctx = use_context::<UiState>();

    // ---- 渲染 ----
    let index = ctx.show_preview.cloned();
    let Some(index) = index else {
        return rsx! {};
    };
    let Some(entry) = ctx.tasks.cloned().get(index).cloned() else {
        return rsx! {};
    };

    // 上限来自设置（超限判定与任务行一致）
    let (video_limit, image_limit) = {
        let s = ctx.settings.read();
        (s.video_max_size(), s.image_max_size())
    };
    let ratio = entry.size_excess_ratio(video_limit, image_limit);

    // 输出侧 data URL：输出 ≤512KB，同步读盘+base64 仅需毫秒级；
    // 渲染随 tasks 信号自动重算，转码完成后 (路径, 大小) 变化即自然刷新。
    let output_url: Option<String> = entry.output_size.and_then(|_| {
        entry.transcoder.lock().ok().and_then(|t| {
            t.get_output().cloned().map(|path| {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let mime = if ext == "png" {
                    "image/png"
                } else {
                    "video/webm"
                };
                preview::output_data_url_mime(mime, &std::fs::read(path).unwrap_or_default())
            })
        })
    });

    let input_url = preview::media_url(Path::new(&entry.input_path));
    let input_video = input_is_video(&entry.input_path);

    let input_kb = kb(entry.input_size);
    let output_side_label = entry
        .output_size
        .map(|s| format!("Output ({})", kb(s)))
        .unwrap_or_else(|| "Output".into());
    let compare = compare_text(&entry, ratio);
    // data URL 已同步生成：Done/SizeExcess 时必有值；仅剩极端竞态兜底
    let show_loading = matches!(
        entry.status,
        crate::transcoder::Status::Done | crate::transcoder::Status::SizeExcess
    ) && output_url.is_none();

    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| ctx.show_preview.set(None),
            div {
                class: "modal preview-modal",
                onclick: move |evt: Event<MouseData>| evt.stop_propagation(),
                h2 { class: "modal-title", "Preview" }
                span { class: "preview-path", title: "{entry.input_path}", "{entry.input_path}" }

                div { class: "preview-grid",
                    div { class: "preview-pane",
                        span { class: "label", "Input ({input_kb})" }
                        if input_video {
                            video { src: "{input_url}", controls: true }
                        } else {
                            img { src: "{input_url}" }
                        }
                    }
                    div { class: "preview-pane",
                        span { class: "label", "{output_side_label}" }
                        {render_output(output_url.clone(), output_is_video(&entry), show_loading)}
                    }
                }

                div { class: "row modal-actions",
                    span {
                        class: match ratio {
                            Some(r) if r > 1.0 => "size-excess",
                            Some(_) => "size-ok",
                            None => "label",
                        },
                        "{compare}"
                    }
                    div { class: "spacer" }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| ctx.show_preview.set(None),
                        "Close"
                    }
                }
            }
        }
    }
}

fn kb(bytes: u64) -> String {
    format!("{:.2} KB", bytes as f64 / 1024.0)
}

/// 对比行文案：未转码 / 达标 / 超限。
fn compare_text(entry: &TaskEntry, ratio: Option<f64>) -> String {
    let input = kb(entry.input_size);
    match (entry.output_size, ratio) {
        (Some(out), Some(r)) => format!(
            "{} \u{2192} {}  \u{2713} within limit ({r:.2}x)",
            input,
            kb(out)
        ),
        (Some(out), None) => format!("{input} \u{2192} {}", kb(out)),
        (None, _) => format!("{input} \u{2192} not transcoded"),
    }
}

/// 输出侧内容类型：视频输入 → webm 输出；图片输入 → png 输出。
fn output_is_video(entry: &TaskEntry) -> bool {
    entry.is_video
}

/// 输出侧内容：有 data URL 按类型渲染；已完成但还在编码 → Loading；否则占位。
fn render_output(url: Option<String>, is_video: bool, loading: bool) -> Element {
    match url {
        Some(url) if is_video => rsx! {
            video { src: "{url}", controls: true, loop: true }
        },
        Some(url) => rsx! {
            img { src: "{url}" }
        },
        None if loading => rsx! {
            div { class: "preview-empty", "Loading…" }
        },
        None => rsx! {
            div { class: "preview-empty", "Not transcoded yet" }
        },
    }
}
