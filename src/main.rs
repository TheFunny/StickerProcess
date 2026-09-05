//! StickerProcess — Dioxus 0.7 desktop 引导入口。
//!
//! 迁移自 iced 0.14（见 docs/MIGRATION_PLAN.md Phase A）：
//! 仅重写 UI 层，`media.rs` / `transcoder.rs` 核心逻辑零改动。

// dx bundle 发布产物不弹终端窗口；debug 运行保留控制台看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod components;
mod config;
mod media;
mod preview;
mod runner;
mod transcoder;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

fn main() {
    // 默认 Info（重试/系数调整/完成可见）；RUST_LOG 可覆盖
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();
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
