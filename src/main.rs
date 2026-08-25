//! StickerProcess — Dioxus 0.7 desktop 引导入口。
//!
//! 迁移自 iced 0.14（见 MIGRATION_PLAN.md Phase A）：
//! 仅重写 UI 层，`media.rs` / `transcoder.rs` 核心逻辑零改动。

mod app;
mod components;
mod media;
mod runner;
mod transcoder;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

fn main() {
    pretty_env_logger::init();
    LaunchBuilder::new()
        .with_cfg(desktop! {
            Config::new().with_window(
                WindowBuilder::new()
                    .with_title("Sticker Process")
                    .with_inner_size(LogicalSize::new(960.0, 680.0))
                    .with_min_inner_size(LogicalSize::new(640.0, 480.0)),
            )
        })
        .launch(app::App);
}
