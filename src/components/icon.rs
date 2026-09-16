//! 内联 SVG 图标（16×16 网格，颜色跟随 `currentColor`）。
//!
//! 刻意不用文字符号（⚙ ▶ ■）：同一码位在不同字体下字形差异很大，Windows 上
//! 还常落到 emoji 字体渲成彩色方块；SVG 在 WebView2 与浏览器里完全一致。
//! 图形只用基本图元或短路径，避免抄错长路径渲染成乱码。

use dioxus::prelude::*;

/// 设置：三档滑杆（比齿轮在小尺寸下更易辨认，且只用矩形拼得出来）。
#[component]
pub fn IconSettings() -> Element {
    rsx! {
        svg { class: "ico", view_box: "0 0 16 16",
            // 三条轨道
            rect { x: "2", y: "3.25", width: "12", height: "1.5", rx: "0.75" }
            rect { x: "2", y: "7.25", width: "12", height: "1.5", rx: "0.75" }
            rect { x: "2", y: "11.25", width: "12", height: "1.5", rx: "0.75" }
            // 每档的旋钮
            rect { x: "9.5", y: "1.75", width: "2", height: "4.5", rx: "0.75" }
            rect { x: "4.5", y: "5.75", width: "2", height: "4.5", rx: "0.75" }
            rect { x: "8", y: "9.75", width: "2", height: "4.5", rx: "0.75" }
        }
    }
}

/// 重试 / 重跑：270° 圆弧 + 缺口处指向弧线前进方向的箭头。
#[component]
pub fn IconRetry() -> Element {
    rsx! {
        svg { class: "ico ico-stroke", view_box: "0 0 16 16",
            // 圆心 (8,8)、半径 4.6：12 点 → **逆时针** 270° → 3 点，右上留缺口。
            // sweep 必须为 0：12 点→3 点的"顺时针大弧"在几何上不存在，写 1 会让
            // 渲染器放大椭圆去凑，画出来是个歪块（实测过）。
            path { d: "M8 3.4A4.6 4.6 0 1 0 12.6 8" }
            // 箭头贴在弧线末端 (12.6,8) 上方，指向切线方向（上）
            path { class: "ico-head", d: "M12.6 4.1 14.7 7.7 10.5 7.7Z" }
        }
    }
}

/// 移除任务：交叉线。
#[component]
pub fn IconClose() -> Element {
    rsx! {
        svg { class: "ico ico-stroke", view_box: "0 0 16 16",
            path { d: "M4.2 4.2 11.8 11.8M11.8 4.2 4.2 11.8" }
        }
    }
}
