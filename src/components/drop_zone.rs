//! 拖拽区（整窗接收）。
//!
//! 已验证路径（spike/dioxus-dnd-demo, Dioxus 0.7.10）：
//! Windows 下 `ondrop` → `evt.data.files()` → `FileData::path()` 返回完整路径；
//! 悬停高亮挂 `ondragover`（Windows 不触发 dragenter）。

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use crate::app::UiState;
use dioxus::html::HasFileData;
use dioxus::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;

/// 原生 drop 监听（'static 闭包）与应用状态之间用待处理队列桥接：
/// 守卫只推队列；DropZone 挂载后启动的 dioxus 任务排空队列并调用
/// add_file_bytes（dioxus 执行器上下文内写信号，避免调度重入挂起）。
#[cfg(target_arch = "wasm32")]
mod drop_bridge {
    use super::*;
    // wasm32 单线程：thread_local RefCell 足够（push 来自 JS 回调、drain 在 dioxus
    // 任务，均主线程）
    thread_local! {
        pub static PENDING: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
    }

    pub fn push(name: String, data: Vec<u8>) {
        PENDING.with(|q| q.borrow_mut().push((name, data)));
        // 唤醒排空任务：写一个普通 JS 全局标志，排空任务轮询它
        js_sys::Reflect::set(
            &js_sys::global(),
            &"_dropPending".into(),
            &wasm_bindgen::JsValue::from_f64(js_sys::Math::random()),
        )
        .ok();
    }
}

/// FileReader 异步读取文件字节（绕开 dioxus FileData，直接走 web-sys）。
#[cfg(target_arch = "wasm32")]
async fn js_read_file_bytes(file: web_sys::File) -> Result<Vec<u8>, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let mut init = |resolve: js_sys::Function, reject: js_sys::Function| {
        let reader = web_sys::FileReader::new().expect("FileReader");
        let onload = Closure::<dyn FnMut()>::new({
            let reader = reader.clone();
            let resolve = resolve.clone();
            let reject = reject.clone();
            move || match reader.result() {
                Ok(result) => {
                    resolve.call1(&wasm_bindgen::JsValue::NULL, &result).ok();
                }
                Err(e) => {
                    reject.call1(&wasm_bindgen::JsValue::NULL, &e).ok();
                }
            }
        });
        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
        // 闭包随 reader 存活；reader 由 JS 持有——forget 保守正确
        onload.forget();
        reader.read_as_array_buffer(&file).expect("read failed");
    };
    let promise = js_sys::Promise::new(&mut init);
    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&result).to_vec())
}

#[cfg(target_arch = "wasm32")]
use drop_bridge::PENDING;

#[component]
pub fn DropZone(children: Element) -> Element {
    let mut dragging = use_signal(|| false);
    let mut ctx = use_context::<UiState>();

    #[cfg(target_arch = "wasm32")]
    {
        // 排空任务：dioxus 执行器内运行（信号写入安全）。同时按 _dropHover
        // 时间戳新鲜度维护覆盖层高亮（拖出窗口 500ms 无打点即熄灭）。
        spawn(async move {
            let mut last_seen = 0f64;
            loop {
                crate::timers::sleep(std::time::Duration::from_millis(200)).await;
                let hover = js_sys::Reflect::get(&js_sys::global(), &"_dropHover".into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let fresh = js_sys::Date::now() - hover < 500.0;
                if dragging() != fresh {
                    dragging.set(fresh);
                }
                let pending_now = js_sys::Reflect::get(&js_sys::global(), &"_dropPending".into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                if pending_now == last_seen {
                    continue;
                }
                last_seen = pending_now;
                let batch: Vec<(String, Vec<u8>)> =
                    PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
                for (name, data) in batch {
                    log::info!("[drop] queue drain: {name}");
                    ctx.add_file_bytes(name, data);
                }
            }
        });
    }

    #[cfg(target_arch = "wasm32")]
    use_effect(move || {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        // 悬停高亮：dioxus ondragover 同样走异步管线不可靠——原生 dragover 给
        // 全局时间戳打点（仅文件拖拽），排空任务按新鲜度翻转 dragging 信号。
        // 注意：dragover 期间 dataTransfer.files 恒空（规范保护模式，文件仅
        // drop 时刻可见），判断文件拖拽要看 types 是否含 "Files"。
        let hover_cb = wasm_bindgen::closure::Closure::wrap(Box::new(|e: web_sys::DragEvent| {
            let has_files = e
                .data_transfer()
                .and_then(|dt| js_sys::Reflect::get(&dt, &"types".into()).ok())
                .is_some_and(|types| js_sys::Array::from(&types).includes(&"Files".into(), 0));
            if has_files {
                let _ = js_sys::Reflect::set(
                    &js_sys::global(),
                    &"_dropHover".into(),
                    &wasm_bindgen::JsValue::from_f64(js_sys::Date::now()),
                );
            }
        })
            as Box<dyn FnMut(web_sys::DragEvent)>);
        let _ =
            window.add_event_listener_with_callback("dragover", hover_cb.as_ref().unchecked_ref());
        hover_cb.forget();
        let cb = wasm_bindgen::closure::Closure::wrap(Box::new(move |e: web_sys::DragEvent| {
            e.prevent_default();
            // 清除悬停戳：drop 后覆盖层立即消失（不等 500ms 过期）
            let _ = js_sys::Reflect::set(
                &js_sys::global(),
                &"_dropHover".into(),
                &wasm_bindgen::JsValue::from_f64(0.0),
            );
            let Some(dt) = e.data_transfer() else {
                return;
            };
            let Some(files) = dt.files() else {
                return;
            };
            for i in 0..files.length() {
                let Some(file) = files.item(i) else {
                    continue;
                };
                let name = file.name();
                wasm_bindgen_futures::spawn_local(async move {
                    match js_read_file_bytes(file).await {
                        Ok(bytes) => {
                            log::info!("[drop] read ok: {name} ({}B)", bytes.len());
                            drop_bridge::push(name, bytes);
                        }
                        Err(e) => log::warn!("[drop] read failed: {name}: {e}"),
                    }
                });
            }
        })
            as Box<dyn FnMut(web_sys::DragEvent)>);
        let _ = window.add_event_listener_with_callback("drop", cb.as_ref().unchecked_ref());
        // 泄漏：全局守卫与应用同生命周期
        cb.forget();
    });

    rsx! {
        div {
            class: "app",
            ondragover: move |evt: Event<DragData>| {
                // 文字选择拖动不带文件（files 由 wry 原生文件拖拽合成），不触发覆盖层
                // web 端高亮由原生 dragover 时间戳 + 排空任务维护，此处仅桌面。
                #[cfg(not(target_arch = "wasm32"))]
                if !dragging() && !evt.data.files().is_empty() {
                    dragging.set(true);
                }
                #[cfg(target_arch = "wasm32")]
                let _ = evt;
            },
            ondragleave: move |_| {
                #[cfg(not(target_arch = "wasm32"))]
                dragging.set(false);
            },
            ondrop: move |evt: Event<DragData>| {
                let _ = &evt; // web 端 evt 不用（原生守卫摄取）
                #[cfg(not(target_arch = "wasm32"))]
                dragging.set(false);
                // 与文件选择一致：运行中也允许追加（沿用 iced 订阅语义）
                // web 端摄取由原生 drop 守卫完成（本监听 prevent_default 异步生效
                // 不可靠），此处仅桌面路径。
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let files: Vec<PathBuf> =
                        evt.data.files().iter().map(|f| f.path()).collect();
                    if !files.is_empty() {
                        ctx.add_files(files);
                    }
                }
            },
            {children}
            if dragging() {
                div { class: "drop-overlay", "Release to add files" }
            }
        }
    }
}
