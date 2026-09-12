//! 平台无关 sleep：桌面走 tokio::time；wasm 用 setTimeout Promise
//! （tokio time driver 在 wasm32 不可用，任何调用会 panic）。

#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: std::time::Duration) {
    use wasm_bindgen::JsCast;
    let millis = duration.as_millis().min(i32::MAX as u128) as i32;
    let mut init = |resolve: js_sys::Function, _: js_sys::Function| {
        web_sys::window()
            .expect("no window on wasm")
            .set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(), millis)
            .expect("set_timeout failed");
    };
    let promise = js_sys::Promise::new(&mut init);
    let _ = promise.await;
}

/// wasm 专用：固定 200ms 轮询步进（web.rs 注入等待用）。
#[cfg(target_arch = "wasm32")]
pub async fn poll_step() {
    sleep(std::time::Duration::from_millis(200)).await;
}
