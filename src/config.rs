//! 设置持久化（Phase B）。
//!
//! `Settings` 是全部可配置项的唯一事实源，序列化为
//! `%APPDATA%/StickerProcess/settings.toml`。
//! 读取失败/文件缺失一律回退默认值并记录日志；保存由 UI 层同步触发。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 输出目录可用性：红框与提示文案都从它派生（替代原来的 bool）。
/// 桌面专属——web 没有输出目录这一行。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputDirState {
    /// 已存在（是否可写另算，见 `Settings::output_dir_writable`）。
    Ok,
    /// 不存在，但父目录在——开跑时自动创建。
    WillCreate,
    /// 父目录不存在，或路径上已经是个文件（建不出来）。
    Missing,
}

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
    /// 转码引擎："sidecar"（ffmpeg 子进程）或 "inprocess"（libav 进程内）。
    #[serde(default = "default_engine")]
    pub engine: String,
    /// 产物文件名是否带上输入文件名（`输入名-时间戳.webm`）；默认 false = 只有时间戳。
    #[serde(default)]
    pub keep_input_name: bool,
}

fn default_engine() -> String {
    "sidecar".into()
}

fn default_retry_shrink_factor() -> f64 {
    0.96
}

fn default_duration_factors() -> [f64; 6] {
    // 与 Transcoder::new / command.rs 常量同一份（原先三处各写一遍）
    crate::transcoder::DEFAULT_DURATION_FACTORS
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            max_retry: 3,
            video_max_size_kb: 256,
            image_max_size_kb: 512,
            retry_shrink_factor: default_retry_shrink_factor(),
            engine: default_engine(),
            duration_factors: default_duration_factors(),
            target_fps: 0.0,
            theme: "system".into(),
            keep_input_name: false,
        }
    }
}

impl Settings {
    /// 转码引擎设置是否为合法值。
    pub fn engine_valid(&self) -> bool {
        crate::transcoder::Engine::parse(self.engine.as_str()).is_some()
    }

    /// 输出目录状态（桌面）：UI 用它决定红框与提示文案。
    /// 只看存在性，不碰磁盘写——可写性要真正建文件才知道，见 `output_dir_writable`。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn output_dir_state(&self) -> OutputDirState {
        if self.output_dir.is_empty() {
            return OutputDirState::Missing;
        }
        let path = std::path::Path::new(&self.output_dir);
        if path.is_dir() {
            return OutputDirState::Ok;
        }
        // 父目录在、且路径本身没被别的东西占着 → 开跑时 create_dir 会成功
        match path.parent() {
            Some(parent) if parent.is_dir() && !path.exists() => OutputDirState::WillCreate,
            _ => OutputDirState::Missing,
        }
    }

    /// 真正建/删一个临时文件确认可写。
    /// 只读目录与 ACL 拒绝在 `is_dir()` 上都是"正常"，只有写一次才知道——但这是有
    /// 副作用的 syscall，**别放进每键一次的渲染路径**：仅失焦与开跑前调用。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn output_dir_writable(&self) -> bool {
        let dir = std::path::Path::new(&self.output_dir);
        if !dir.is_dir() {
            return false;
        }
        let probe = dir.join(".stickerprocess_write_test");
        match std::fs::write(&probe, b"") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }

    pub fn video_max_size(&self) -> u64 {
        u64::from(self.video_max_size_kb) * 1024
    }

    pub fn image_max_size(&self) -> u64 {
        u64::from(self.image_max_size_kb) * 1024
    }
}

// ---- 持久化（按平台拆分，签名一致）----
// 桌面：%APPDATA%/StickerProcess/settings.toml（toml 格式，用户已有文件）
// wasm：localStorage key "StickerProcess.settings"（JSON 格式，体积小无转义负担）

#[cfg(not(target_arch = "wasm32"))]
fn config_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(|base| {
            PathBuf::from(base)
                .join("StickerProcess")
                .join("settings.toml")
        })
        .unwrap_or_else(|| PathBuf::from("settings.toml"))
}

#[cfg(target_arch = "wasm32")]
const LOCAL_STORAGE_KEY: &str = "StickerProcess.settings";

/// 启动时读取设置；缺失/损坏时返回默认值。
#[cfg(not(target_arch = "wasm32"))]
pub fn load() -> Settings {
    let mut settings = match std::fs::read_to_string(config_path()) {
        Ok(raw) => toml::from_str(&raw).unwrap_or_else(|e| {
            log::warn!("Failed to parse settings.toml, using defaults: {e}");
            Settings::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(e) => {
            log::warn!("Failed to read settings.toml, using defaults: {e}");
            Settings::default()
        }
    };
    sanitize(&mut settings);
    settings
}

#[cfg(target_arch = "wasm32")]
pub fn load() -> Settings {
    let mut settings = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(LOCAL_STORAGE_KEY).ok().flatten())
        .and_then(|raw| match serde_json::from_str::<Settings>(&raw) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("Failed to parse localStorage settings, using defaults: {e}");
                None
            }
        })
        .unwrap_or_default();
    sanitize(&mut settings);
    settings
}

/// 公共兜底：引擎值 / 输出目录 / 数值项（两平台一致）。手改的
/// settings.toml / localStorage 绕过 NumberInput 的 UI 钳制：0 KB 上限
/// → inf 超限比烧满重试；非有限值 → toml 序列化永久失败。范围与设置
/// 面板各 NumberInput 的 min/max 对齐。
fn sanitize(settings: &mut Settings) {
    if !settings.engine_valid() {
        log::warn!(
            "Invalid engine '{}' in settings, falling back to 'sidecar'",
            settings.engine
        );
        settings.engine = "sidecar".into();
    }
    if settings.output_dir.is_empty() {
        settings.output_dir = default_output_dir();
    }
    let max_kb = settings.video_max_size_kb.clamp(16, 102_400);
    if settings.video_max_size_kb != max_kb {
        log::warn!("video_max_size_kb out of range, clamped to {max_kb}");
        settings.video_max_size_kb = max_kb;
    }
    let max_kb = settings.image_max_size_kb.clamp(16, 102_400);
    if settings.image_max_size_kb != max_kb {
        log::warn!("image_max_size_kb out of range, clamped to {max_kb}");
        settings.image_max_size_kb = max_kb;
    }
    fn clamp_f64(v: &mut f64, min: f64, max: f64, name: &str) {
        let c = if v.is_finite() {
            v.clamp(min, max)
        } else {
            min
        };
        if *v != c {
            log::warn!("{name} out of range ({v}), clamped to {c}");
            *v = c;
        }
    }
    clamp_f64(
        &mut settings.retry_shrink_factor,
        0.05,
        1.0,
        "retry_shrink_factor",
    );
    clamp_f64(&mut settings.target_fps, 0.0, 240.0, "target_fps");
    for (i, f) in settings.duration_factors.iter_mut().enumerate() {
        clamp_f64(f, 0.05, 10.0, &format!("duration_factors[{i}]"));
    }
}

/// 阻塞写盘（调用方放在 spawn_blocking 中）。
#[cfg(not(target_arch = "wasm32"))]
pub fn save(settings: &Settings) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = toml::to_string_pretty(settings).map_err(|e| e.to_string())?;
    // 先写临时文件再改名：fs::write 截断后写，中断窗口留下半截 toml
    // → load 判损坏，全部设置回默认。rename 在同一卷上是原子的。
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, raw).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// 阻塞写 localStorage（wasm 无独立线程，直接同步写，量级 ~1KB）。
#[cfg(target_arch = "wasm32")]
pub fn save(settings: &Settings) -> Result<(), String> {
    let raw = serde_json::to_string(settings).map_err(|e| e.to_string())?;
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .ok_or_else(|| "localStorage unavailable".to_string())?
        .set_item(LOCAL_STORAGE_KEY, &raw)
        .map_err(|e| format!("localStorage set_item failed: {e:?}"))
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
        s.engine = "inprocess".into();
        s.keep_input_name = true;
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
    fn missing_engine_falls_back_to_sidecar() {
        // 兼容旧版 settings.toml：缺 engine 键时用默认 sidecar
        let legacy = "output_dir = 'X'\nmax_retry = 2\nvideo_max_size_kb = 256\nimage_max_size_kb = 512\ntheme = 'light'\n";
        let parsed: Settings = toml::from_str(legacy).unwrap();
        assert_eq!(parsed.engine, "sidecar");
    }

    #[test]
    fn engine_valid_rejects_unknown_values() {
        let mut s = Settings::default();
        assert!(s.engine_valid());
        s.engine = "inprocess".into();
        assert!(s.engine_valid());
        s.engine = "gpu".into();
        assert!(!s.engine_valid());
    }

    #[test]
    fn sanitize_clamps_out_of_range_values() {
        let mut s = Settings::default();
        s.video_max_size_kb = 0; // inf 超限比的源头
        s.retry_shrink_factor = f64::NAN; // toml 序列化失败源
        s.duration_factors = [99.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        s.target_fps = 1e9;
        sanitize(&mut s);
        assert_eq!(s.video_max_size_kb, 16);
        assert_eq!(s.retry_shrink_factor, 0.05);
        assert_eq!(s.duration_factors[0], 10.0);
        assert_eq!(s.duration_factors[1], 0.05);
        assert_eq!(s.target_fps, 240.0);
        // 序列化必须恢复可用
        assert!(toml::to_string_pretty(&s).is_ok());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn output_dir_state_covers_the_four_cases() {
        let base = std::env::temp_dir().join(format!("sp_cfg_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        let mut s = Settings::default();

        // 已存在的目录
        s.output_dir = base.to_string_lossy().into_owned();
        assert_eq!(s.output_dir_state(), OutputDirState::Ok);
        assert!(s.output_dir_writable(), "temp dir should be writable");

        // 不存在、但父目录在 → 开跑时 create_dir 会成功
        s.output_dir = base.join("child").to_string_lossy().into_owned();
        assert_eq!(s.output_dir_state(), OutputDirState::WillCreate);
        // 还没建出来时不能声称可写（探测不建目录）
        assert!(!s.output_dir_writable());

        // 路径已被一个文件占着 → 建不出来
        let file = base.join("occupied");
        std::fs::write(&file, b"x").unwrap();
        s.output_dir = file.to_string_lossy().into_owned();
        assert_eq!(s.output_dir_state(), OutputDirState::Missing);

        // 父目录本身就不存在
        s.output_dir = base
            .join("nope")
            .join("deeper")
            .to_string_lossy()
            .into_owned();
        assert_eq!(s.output_dir_state(), OutputDirState::Missing);

        // 空串 = 未设置
        s.output_dir = String::new();
        assert_eq!(s.output_dir_state(), OutputDirState::Missing);

        let _ = std::fs::remove_dir_all(&base);
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
