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
#[cfg(not(target_arch = "wasm32"))]
use crate::preview;
use dioxus::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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
    // 输出 data URL 缓存：以 (输出路径, 大小) 为键，键不变不重读盘/重编码
    // （hook 必须在早退之前调用；Rc<RefCell> 非信号，绝不触发重渲染）。
    let output_cache = use_hook(|| Rc::new(RefCell::new(None::<(PathBuf, u64, String)>)));
    // wasm 输入侧 object URL 缓存：以 (输入名, 大小) 为键；键变时 revoke 旧 URL
    // （Blob 已在构造时拷贝字节，revoke 不影响已渲染元素）。非信号，不触发重渲染。
    #[cfg(target_arch = "wasm32")]
    let input_cache = use_hook(|| Rc::new(RefCell::new(None::<((String, u64), String)>)));

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

    // 输出侧 data URL：输出 ≤512KB，编码仅需毫秒级；缓存键为
    // (输出路径, 大小)——转码期间 tasks 信号 10Hz 更新不会重复编码。
    // 桌面从镜像路径读盘（免锁）；wasm 锁内克隆 output_bytes（单线程，
    // 锁从不跨 await 持有，渲染期同步取锁与 task_list 下载同一模式）。
    let output_url: Option<String> = {
        let key = match (&entry.output_path, &entry.output_size) {
            (Some(path), Some(size)) => Some((path.clone(), *size)),
            _ => None,
        };
        let mut slot = output_cache.borrow_mut();
        match (&key, slot.as_ref()) {
            (Some((p, s)), Some((lp, ls, url))) if lp == p && ls == s => Some(url.clone()),
            _ => {
                let url = key.as_ref().and_then(|(path, size)| {
                    let mime = output_mime(path);
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        std::fs::read(path)
                            .ok()
                            .map(|bytes| preview::output_data_url_mime(mime, &bytes))
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        entry
                            .transcoder
                            .lock()
                            .ok()
                            .and_then(|t| t.output_bytes.clone())
                            // 镜像大小与内存不一致（竞态）→ None，走 Loading 兜底
                            .filter(|b| b.len() as u64 == *size)
                            .map(|bytes| crate::transcoder::web::data_url(mime, &bytes))
                    }
                });
                // 键为 None 时也清空缓存，避免复用上一个任务的 URL
                *slot = key.map(|(p, s)| (p, s, url.clone().unwrap_or_default()));
                url
            }
        }
    };

    #[cfg(not(target_arch = "wasm32"))]
    let input_url = preview::media_url(Path::new(&entry.input_path));
    #[cfg(target_arch = "wasm32")]
    let input_url: String = {
        // web：输入字节 → Blob object URL（几 MB 转 data URL 太浪费）；
        // (名字, 大小) 为键缓存，键变 revoke 旧 URL（Blob 构造时已拷贝字节）。
        let key = (entry.input_path.clone(), entry.input_size);
        let mut slot = input_cache.borrow_mut();
        match slot.as_ref() {
            Some((k, url)) if *k == key => url.clone(),
            _ => {
                let bytes = entry
                    .transcoder
                    .lock()
                    .ok()
                    .and_then(|t| t.media_file.bytes().map(|b| b.to_vec()))
                    .unwrap_or_default();
                let mime = input_mime(&key.0);
                let url = object_url_for(mime, &bytes);
                if let Some((_, old)) = slot.replace((key, url.clone())) {
                    web_sys::Url::revoke_object_url(&old).ok();
                }
                url
            }
        }
    };
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
            tabindex: 0,
            onmounted: move |evt: Event<MountedData>| {
                spawn(async move {
                    let _ = evt.data.set_focus(true).await;
                });
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                if evt.data.key() == dioxus::html::Key::Escape {
                    ctx.show_preview.set(None);
                }
            },
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

/// 输出文件扩展名（小写）。
fn path_ext_str(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn output_mime(path: &Path) -> &'static str {
    if path_ext_str(path) == "png" {
        "image/png"
    } else {
        "video/webm"
    }
}

/// 输入侧 MIME（按扩展名；与 src/preview.rs 的映射一致，wasm 无该模块）。
fn input_mime(name: &str) -> &'static str {
    match path_ext_str(Path::new(name)).as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" | "apng" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// 字节 → Blob object URL（wasm 输入预览；Blob 构造即拷贝字节）。
#[cfg(target_arch = "wasm32")]
fn object_url_for(mime: &str, bytes: &[u8]) -> String {
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array.into());
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &opts) else {
        return String::new();
    };
    web_sys::Url::create_object_url_with_blob(&blob)
        .map(String::from)
        .unwrap_or_default()
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
