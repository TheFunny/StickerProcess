//! StickerProcess — Dioxus 0.7 引导入口（desktop / web 双平台）。
//!
//! 迁移自 iced 0.14（见 docs/MIGRATION_PLAN.md Phase A）：
//! 仅重写 UI 层，`media.rs` / `transcoder.rs` 核心逻辑零改动。

// dx bundle 发布产物不弹终端窗口；debug 运行保留控制台看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod components;
mod config;
mod media;
mod runner;
mod transcoder;

#[cfg(not(target_arch = "wasm32"))]
mod preview; // wry 自定义协议，桌面专属
#[cfg(not(target_arch = "wasm32"))]
mod sidecar_probe; // ffmpeg.exe 子进程探测，桌面专属

// 桌面入口：wry 窗口 + preview:// 协议 + sidecar 探测
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
    use dioxus::prelude::*;

    // 默认 Info（重试/系数调整/完成可见）；RUST_LOG 可覆盖
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();
    // sidecar 可用性探测（进程内缓存一次；日志记录结果）
    crate::sidecar_probe::SidecarProbe::init();
    let cfg = preview::register(
        // with_menu(None) 关闭 dioxus 默认菜单栏（File/View 等）
        Config::new()
            .with_menu(None::<dioxus::desktop::muda::Menu>)
            .with_window(
                WindowBuilder::new()
                    .with_title("Sticker Process")
                    .with_inner_size(LogicalSize::new(960.0, 680.0))
                    .with_min_inner_size(LogicalSize::new(640.0, 480.0)),
            ),
    );
    LaunchBuilder::new()
        .with_cfg(desktop! { cfg })
        .launch(app::App);
}

// 网页入口：无 preview 协议 / sidecar；转码引擎下一阶段接 WebCodecs 桥
#[cfg(target_arch = "wasm32")]
fn main() {
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();
    dioxus::launch(app::App);
}
