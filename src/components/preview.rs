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
    // 存 Rc<str>：data URL 可达 ~699KB，跑批期间 10Hz 重渲染时 String 克隆
    // 是 ~14MB/s 的无谓 memcpy，Rc 克隆是 O(1)。
    let output_cache = use_hook(|| Rc::new(RefCell::new(None::<(PathBuf, u64, Rc<str>)>)));
    // wasm 输入侧 object URL 缓存：以 (输入名, 大小) 为键；键变时 revoke 旧 URL
    // （Blob 已在构造时拷贝字节，revoke 不影响已渲染元素）。非信号，不触发重渲染。
    #[cfg(target_arch = "wasm32")]
    let input_cache = use_hook(|| Rc::new(RefCell::new(None::<((String, u64), String)>)));

    // ---- 渲染 ----
    let index = ctx.show_preview.cloned();
    let Some(index) = index else {
        return rsx! {};
    };
    let Some(entry) = ctx.tasks.read().get(index).cloned() else {
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
    let output_url: Option<Rc<str>> = {
        let key = match (&entry.mirror.output_path, &entry.mirror.output_size) {
            (Some(path), Some(size)) => Some((path.clone(), *size)),
            _ => None,
        };
        let mut slot = output_cache.borrow_mut();
        match (&key, slot.as_ref()) {
            (Some((p, s)), Some((lp, ls, url))) if lp == p && ls == s => Some(Rc::clone(url)),
            _ => {
                let url = key.as_ref().and_then(|(path, _size)| {
                    let mime = output_mime(path);
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        std::fs::read(path).ok().map(|bytes| data_url(mime, &bytes))
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        entry
                            .transcoder
                            .lock()
                            .ok()
                            .and_then(|t| t.output_bytes.clone())
                            // 镜像大小与内存不一致（竞态）→ None，走 Loading 兜底
                            .filter(|b| b.len() as u64 == *_size)
                            .map(|bytes| data_url(mime, &bytes))
                    }
                });
                // 只缓存成功值：失败（文件被删/改名、杀软瞬时锁住）不入缓存，
                // 否则空串被当有效 URL 渲染成空元素且永不重试（key 不变）。
                // key 为 None 时清空，避免复用上一个任务的 URL。
                let url = url.map(Rc::from);
                *slot = match (&key, &url) {
                    (Some((p, s)), Some(u)) => Some((p.clone(), *s, Rc::clone(u))),
                    _ => None,
                };
                url
            }
        }
    };

    #[cfg(not(target_arch = "wasm32"))]
    let input_url = preview::media_url(Path::new(&entry.mirror.input_path));
    #[cfg(target_arch = "wasm32")]
    let input_url: String = {
        // web：输入字节 → Blob object URL（几 MB 转 data URL 太浪费）；
        // (名字, 大小) 为键缓存，键变 revoke 旧 URL（Blob 构造时已拷贝字节）。
        let key = (entry.mirror.input_path.clone(), entry.mirror.input_size);
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
                // 只缓存成功值：失败（空串）不入缓存，下次渲染重试
                if url.is_empty() {
                    if let Some((_, old)) = slot.take() {
                        web_sys::Url::revoke_object_url(&old).ok();
                    }
                } else if let Some((_, old)) = slot.replace((key, url.clone())) {
                    web_sys::Url::revoke_object_url(&old).ok();
                }
                url
            }
        }
    };
    let input_video = input_is_video(&entry.mirror.input_path);

    let input_kb = kb(entry.mirror.input_size);
    let output_side_label = entry
        .mirror
        .output_size
        .map(|s| format!("Output ({})", kb(s)))
        .unwrap_or_else(|| "Output".into());
    let compare = compare_text(&entry, ratio);
    // 产物动作的平台差异（桌面定位 / web 下载）
    let output_action = if cfg!(target_arch = "wasm32") {
        "Download"
    } else {
        "Reveal in Explorer"
    };
    // data URL 已同步生成：Done/SizeExcess 时必有值；仅剩极端竞态兜底
    let show_loading = matches!(
        entry.mirror.status,
        crate::transcoder::Status::Done | crate::transcoder::Status::SizeExcess
    ) && output_url.is_none();
    rsx! {
        div {
            class: "modal-backdrop",
            tabindex: 0,
            role: "dialog",
            "aria-label": "Preview",
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
                span { class: "preview-path", title: "{entry.mirror.input_path}", "{entry.mirror.input_path}" }
                // 行内只放一行省略版，完整原因在这里（可换行/滚动）
                if let Some(err) = entry.mirror.error.as_deref() {
                    div { class: "preview-error", "{err}" }
                }

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
                        {render_output(output_url.as_deref(), entry.mirror.is_video, show_loading)}
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
                    // 产物动作：行内标记只读后，这里是"拿到结果"的唯一入口
                    if entry.mirror.output_path.is_some() && entry.mirror.output_size.is_some() {
                        button {
                            class: "btn",
                            title: "{output_action}",
                            onclick: move |_| {
                                #[cfg(not(target_arch = "wasm32"))]
                                if let Some(path) = entry.mirror.output_path.as_ref() {
                                    // /select 打开资源管理器并选中输出文件
                                    let _ = std::process::Command::new("explorer")
                                        .arg(format!("/select,{}", path.display()))
                                        .spawn();
                                }
                                #[cfg(target_arch = "wasm32")]
                                {
                                    // web：从内存 output_bytes 触发浏览器下载
                                    let name = entry
                                        .mirror
                                        .output_file_name
                                        .clone()
                                        .unwrap_or_else(|| "sticker.webm".into());
                                    let bytes = entry
                                        .transcoder
                                        .lock()
                                        .ok()
                                        .and_then(|t| t.output_bytes.clone());
                                    if let Some(bytes) = bytes {
                                        spawn(async move {
                                            if crate::transcoder::web::sticker_download(
                                                &bytes, &name,
                                            )
                                            .is_err()
                                            {
                                                // wasm 无日志后端：失败必须给用户看得见的反馈
                                                ctx.push_toast(
                                                    crate::components::toast::ToastKind::Error,
                                                    "Download failed",
                                                );
                                            }
                                        });
                                    }
                                }
                            },
                            "{output_action}"
                        }
                    }
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

/// 内存字节 → base64 data URL（输出轨 ≤512KB，编码毫秒级）。
/// 唯一消费者是本组件——原先挂在 transcoder 上只是因为预览先落地在那边。
fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// 输出文件扩展名（小写）。
fn path_ext_str(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn output_mime(path: &Path) -> &'static str {
    // 输出只可能是 .png / .webm；兜底沿用旧默认
    crate::media::mime_for_ext(&path_ext_str(path)).unwrap_or("video/webm")
}

/// 输入侧 MIME（web 预览 Blob URL；桌面走 preview:// 协议）。
#[cfg(target_arch = "wasm32")]
fn input_mime(name: &str) -> &'static str {
    crate::media::mime_for_ext(&path_ext_str(Path::new(name)))
        .unwrap_or("application/octet-stream")
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
    web_sys::Url::create_object_url_with_blob(&blob).unwrap_or_default()
}

/// 对比行文案：未转码 / 达标 / 超限。
fn compare_text(entry: &TaskEntry, ratio: Option<f64>) -> String {
    let input = kb(entry.mirror.input_size);
    let Some(out) = entry.mirror.output_size else {
        return format!("{input} \u{2192} not transcoded");
    };
    let arrow = format!("{input} \u{2192} {}", kb(out));
    match ratio {
        // 超限时不能再说 "within limit"（原先只有颜色变红，文字照旧）
        Some(r) if r > 1.0 => format!("{arrow}  \u{2717} over limit ({r:.2}x)"),
        Some(r) => format!("{arrow}  \u{2713} within limit ({r:.2}x)"),
        None => arrow,
    }
}

/// 输出侧内容：有 data URL 按类型渲染；已完成但还在编码 → Loading；否则占位。
/// 收 &str：data URL 可达 ~699KB，调用方正处在渲染路径上，不为传参再拷一次。
fn render_output(url: Option<&str>, is_video: bool, loading: bool) -> Element {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// data URL 是浏览器侧的输入契约：mime 前缀 + 标准 base64（写错则预览整片空白）。
    #[test]
    fn data_url_formats_prefix_and_payload() {
        use base64::Engine as _;
        assert_eq!(
            data_url("image/png", &[1, 2, 3]),
            format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3])
            )
        );
    }
}
