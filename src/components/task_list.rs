//! 任务列表与任务行。
//!
//! 渲染只读显示镜像，不锁 Transcoder（worker 持锁期间 UI 不卡顿）。
//! Phase C：行内进度条 + 转码耗时 + 错误详情悬停提示 + Probing 状态。

use crate::app::{TaskEntry, UiState};
use crate::components::number_field::NumberInput;
use crate::transcoder::Status;
use dioxus::prelude::*;

#[component]
pub fn TaskList() -> Element {
    let ctx = use_context::<UiState>();
    let count = ctx.tasks.read().len();

    rsx! {
        div { class: "task-list grow", role: "list",
            if count == 0 {
                div { class: "empty-hint",
                    "No tasks yet. Click \"Add File\" or drop files here."
                }
            }
            for index in 0..count {
                TaskRow { key: "{index}", index }
            }
        }
    }
}

#[component]
fn TaskRow(index: usize) -> Element {
    let ctx = use_context::<UiState>();
    let entry = ctx.tasks.read().get(index).cloned();
    let Some(entry) = entry else { return rsx! {} };

    rsx! { TaskRowView { entry, index, running: ctx.running.cloned() } }
}

/// 拆出纯展示子组件：entry 变化时才重渲染本行。
#[component]
fn TaskRowView(entry: TaskEntry, index: usize, running: bool) -> Element {
    let mut ctx = use_context::<UiState>();
    // 拖选文字/数字时按下与松开位置不同——click 会落在行上，需与普通点击区分
    let mut mouse_down_at = use_signal(|| None::<(f64, f64)>);

    let status_class = match entry.mirror.status {
        Status::Probing => "badge probing",
        Status::Processing => "badge processing",
        Status::Done => "badge done",
        Status::Alert => "badge alert",
        Status::SizeExcess => "badge size-excess",
        _ => "badge",
    };
    // 与 iced 一致：输出大小颜色区分是否超限（上限来自设置）
    let settings = ctx.settings.read();
    let (video_limit, image_limit) = (settings.video_max_size(), settings.image_max_size());
    let is_excess = entry
        .size_excess_ratio(video_limit, image_limit)
        .is_some_and(|ratio| ratio > 1.0);
    // 输出产物：名字降为元信息（12px 弱化），大小才是结果本身。
    // 达标与否除颜色外必须有文字标记（色觉障碍用户 + WCAG 1.4.1 不只是颜色）
    let output = entry.mirror.output_size.map(|size| {
        let (mark, kb) = (if is_excess { "!" } else { "✓" }, size as f64 / 1024.0);
        (
            entry.mirror.output_file_name.clone().unwrap_or_default(),
            format!("{mark} {kb:.2}KB"),
        )
    });
    let size_class = if is_excess { "size-excess" } else { "size-ok" };
    // 同一个链接的动作随平台不同：桌面在资源管理器定位，web 触发下载
    let size_action = if cfg!(target_arch = "wasm32") {
        "Download output"
    } else {
        "Reveal in Explorer"
    };
    // wasm 下载用：锁内克隆输出字节（仅 Done/SizeExcess 任务有）
    #[cfg(target_arch = "wasm32")]
    let entry_bytes: Option<Vec<u8>> = {
        let t = entry.transcoder.lock().ok();
        t.and_then(|t| t.output_bytes.clone())
    };
    // 选中输出文件用（onclick 闭包捕获，桌面专属）
    #[cfg(not(target_arch = "wasm32"))]
    let select_path = entry.mirror.output_path.clone();
    // 与 iced 一致：系数仅在首次转码（自动初始化）后出现
    let factor_value = entry.mirror.factor;
    // 转码耗时（成功后保留展示）
    let elapsed_text = entry.mirror.elapsed_ms.map(format_elapsed);
    // 悬停提示：优先错误详情，否则完整路径
    let tooltip = match &entry.mirror.error {
        Some(err) => format!("{}\n{}", entry.mirror.input_path, err),
        None => entry.mirror.input_path.clone(),
    };

    rsx! {
        div {
            class: "task-row",
            title: "{tooltip}",
            role: "listitem",
            tabindex: 0,
            "aria-label": "{entry.mirror.status.label()}: {entry.mirror.input_path} — open preview",
            onmousedown: move |evt: Event<MouseData>| {
                let p = evt.data.client_coordinates();
                mouse_down_at.set(Some((p.x, p.y)));
            },
            onclick: move |evt: Event<MouseData>| {
                // 按下与松开位移超过 4px 视为拖选，不打开预览
                let p = evt.data.client_coordinates();
                if let Some((dx, dy)) = mouse_down_at.cloned() {
                    let moved = ((p.x - dx).powi(2) + (p.y - dy).powi(2)).sqrt();
                    if moved > 4.0 {
                        return;
                    }
                }
                ctx.show_preview.set(Some(index));
            },
            // 键盘等价物：行可 Tab 到，Enter/Space 打开预览（子控件自行吞掉按键）
            onkeydown: move |evt: Event<KeyboardData>| {
                let key = evt.data.key();
                if key == dioxus::html::Key::Enter
                    || matches!(&key, dioxus::html::Key::Character(s) if s.as_str() == " ")
                {
                    evt.prevent_default();
                    ctx.show_preview.set(Some(index));
                }
            },
            div { class: "row-main",
                span { class: "{status_class}", "[{entry.mirror.status.label()}]" }
                span { class: "path", "{entry.mirror.input_path}" }
                if let Some(pct) = entry.mirror.progress {
                    span { class: "pct", "{(pct * 100.0).round()}%" }
                } else if let Some((name, size)) = output {
                    button {
                        class: "btn-link out-file",
                        title: "{size_action}",
                        onkeydown: move |evt: Event<KeyboardData>| evt.stop_propagation(),
                        onclick: move |evt: Event<MouseData>| {
                            evt.stop_propagation();
                            // /select 打开资源管理器并选中输出文件
                            #[cfg(not(target_arch = "wasm32"))]
                            if let Some(path) = select_path.as_ref() {
                                // /select 打开资源管理器并选中输出文件
                                let _ = std::process::Command::new("explorer")
                                    .arg(format!("/select,{}", path.display()))
                                    .spawn();
                            }
                            #[cfg(target_arch = "wasm32")]
                            {
                                // web：从内存 output_bytes 触发浏览器下载
                                if let Some(bytes) = entry_bytes.clone() {
                                    let name = entry
                                        .mirror
                                        .output_file_name
                                        .clone()
                                        .unwrap_or_else(|| "sticker.webm".into());
                                    spawn(async move {
                                        if crate::transcoder::web::sticker_download(
                                            &bytes, &name,
                                        ).is_err() {
                                            log::error!("download failed: glue missing");
                                        }
                                    });
                                }
                            }
                        },
                        span { class: "out-name", "{name}" }
                        span { class: "out-size {size_class}", "{size}" }
                    }
                }
                if let Some(elapsed) = elapsed_text {
                    span { class: "elapsed", "{elapsed}" }
                }
                if let Some(factor) = factor_value {
                    span {
                        onclick: move |evt: Event<MouseData>| evt.stop_propagation(),
                        onkeydown: move |evt: Event<KeyboardData>| evt.stop_propagation(),
                        NumberInput<f64> {
                            value: factor,
                            min: 0.1,
                            max: 10.0,
                            disabled: running,
                            on_change: move |v| {
                                ctx.with_task(index, move |t| {
                                    if let Some(f) = t.size_factor.as_mut() {
                                        *f = v;
                                    }
                                });
                            },
                        }
                    }
                }
                // Done 也可重跑：Run 只处理未完成任务，重转需先把它退回 Pending
                if matches!(entry.mirror.status, Status::Alert | Status::SizeExcess | Status::Done) {
                    button {
                        class: "btn btn-mini",
                        disabled: running,
                        onkeydown: move |evt: Event<KeyboardData>| evt.stop_propagation(),
                        onclick: move |evt: Event<MouseData>| {
                            evt.stop_propagation();
                            ctx.retry_task(index);
                        },
                        if entry.mirror.status == Status::Done { "Re-run" } else { "Retry" }
                    }
                }
                button {
                    class: "btn btn-mini btn-remove",
                    disabled: running,
                    "aria-label": "Remove task",
                    onkeydown: move |evt: Event<KeyboardData>| evt.stop_propagation(),
                    onclick: move |evt: Event<MouseData>| {
                        evt.stop_propagation();
                        ctx.remove_task(index);
                    },
                    "✕"
                }
            }
            if let Some(pct) = entry.mirror.progress {
                div { class: "row-progress",
                    div { class: "row-progress-fill", width: "{(pct * 100.0).round()}%" }
                }
            }
        }
    }
}

/// 毫秒 → `12.3s` / `1m02s`。
fn format_elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}.{:01}s", secs, (ms % 1000) / 100)
    }
}
