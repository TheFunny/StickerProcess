//! 设置持久化（Phase B）。
//!
//! `Settings` 是全部可配置项的唯一事实源，序列化为
//! `%APPDATA%/StickerProcess/settings.toml`。
//! 读取失败/文件缺失一律回退默认值并记录日志；保存由 UI 层防抖触发。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 输出目录；空串表示未设置（load 时回填为 ./output）。
    pub output_dir: String,
    /// 超限重试次数上限。
    pub max_retry: u8,
    /// 视频贴纸大小上限（KB）。
    pub video_max_size_kb: u32,
    /// 图片贴纸大小上限（KB）。
    pub image_max_size_kb: u32,
    /// 超限重试时的系数缩放：factor = factor / excess * retry_shrink_factor。
    #[serde(default = "default_retry_shrink_factor")]
    pub retry_shrink_factor: f64,
    /// 时长→默认系数表，区间固定：<1s, <2s, <3s, <5s, <8s, ≥8s。
    #[serde(default = "default_duration_factors")]
    pub duration_factors: [f64; 6],
    /// 强制输出帧率（fps）；0 表示跟随源/编码器自动。
    #[serde(default)]
    pub target_fps: f64,
    /// 主题："light" / "dark"。
    pub theme: String,
}

fn default_retry_shrink_factor() -> f64 {
    0.96
}

fn default_duration_factors() -> [f64; 6] {
    [1.2, 1.1, 1.0, 0.9, 0.8, 0.7]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            max_retry: 3,
            video_max_size_kb: 256,
            image_max_size_kb: 512,
            retry_shrink_factor: default_retry_shrink_factor(),
            duration_factors: default_duration_factors(),
            target_fps: 0.0,
            theme: "system".into(),
        }
    }
}

impl Settings {
    /// 输出目录是否可用（已存在，或父目录存在可创建）。
    pub fn output_dir_valid(&self) -> bool {
        let path = std::path::Path::new(&self.output_dir);
        path.is_dir()
            || (!self.output_dir.is_empty() && path.parent().is_some_and(|p| p.is_dir()))
    }

    pub fn video_max_size(&self) -> u64 {
        u64::from(self.video_max_size_kb) * 1024
    }

    pub fn image_max_size(&self) -> u64 {
        u64::from(self.image_max_size_kb) * 1024
    }
}

fn config_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(|base| {
            PathBuf::from(base)
                .join("StickerProcess")
                .join("settings.toml")
        })
        .unwrap_or_else(|| PathBuf::from("settings.toml"))
}

/// 启动时读取设置；缺失/损坏时返回默认值。
pub fn load() -> Settings {
    let mut settings = match std::fs::read_to_string(config_path()) {
        Ok(raw) => toml::from_str(&raw).unwrap_or_else(|e| {
            log::warn!("Failed to parse settings.toml, using defaults: {e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    if settings.output_dir.is_empty() {
        settings.output_dir = default_output_dir();
    }
    settings
}

/// 阻塞写盘（调用方放在 spawn_blocking 中）。
pub fn save(settings: &Settings) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = toml::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

fn default_output_dir() -> String {
    let mut output = std::env::current_dir().unwrap_or_else(|e| {
        log::error!("Failed to get current directory: {}", e);
        PathBuf::from(".")
    });
    output.push("output");
    output.to_string_lossy().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default() {
        let raw = toml::to_string_pretty(&Settings::default()).unwrap();
        let parsed: Settings = toml::from_str(&raw).unwrap();
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn roundtrip_custom() {
        let mut s = Settings::default();
        s.output_dir = r"D:\out put\目录".into();
        s.max_retry = 7;
        s.video_max_size_kb = 128;
        s.image_max_size_kb = 1024;
        s.retry_shrink_factor = 0.9;
        s.duration_factors = [1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        s.target_fps = 30.0;
        s.theme = "dark".into();
        let raw = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&raw).unwrap(), s);
    }

    #[test]
    fn missing_new_fields_fall_back_to_defaults() {
        // 兼容旧版 settings.toml：缺新字段时用默认值（未知键 gif_factor_multiplier 被忽略）
        let legacy = "output_dir = 'X'\nmax_retry = 2\nvideo_max_size_kb = 256\nimage_max_size_kb = 512\ngif_factor_multiplier = 0.75\ntheme = 'light'\n";
        let parsed: Settings = toml::from_str(legacy).unwrap();
        assert_eq!(parsed.duration_factors, default_duration_factors());
        assert_eq!(parsed.target_fps, 0.0);
        assert_eq!(parsed.retry_shrink_factor, default_retry_shrink_factor());
    }

    #[test]
    fn load_missing_file_uses_defaults_with_output_backfill() {
        // 不落盘：直接验证默认值的回填逻辑等价性
        let mut s = Settings::default();
        assert!(s.output_dir.is_empty());
        s.output_dir = default_output_dir();
        assert!(s.output_dir.ends_with("output"));
    }
}
