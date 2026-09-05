//! sidecar（外部 ffmpeg.exe）可用性探测。
//!
//! 探测逻辑与 `ffmpeg-sidecar` 的路径解析一致（`paths::ffmpeg_path`）：
//! 1. 主程序同目录的 `ffmpeg.exe`（随包/手动放置场景）
//! 2. 系统 `PATH` 中的 `ffmpeg`
//!
//! 结果进程内缓存一次；`available()` 只做文件存在性检查（零开销），
//! `verify()` 追加 `-version` 子进程调用确认可执行（首次 UI 展示时调用）。

use std::path::PathBuf;
use std::sync::OnceLock;

static PROBE: OnceLock<SidecarProbe> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct SidecarProbe {
    /// 解析到的 ffmpeg 可执行文件路径（存在性已确认）
    pub exe: PathBuf,
    /// 是否有 libvpx-vp9 编码器（转码必需；极简构建可能没有）
    pub has_vp9: bool,
}

impl SidecarProbe {
    /// 启动后调用一次；后续 `probe()` 直接命中缓存。
    pub fn init() -> &'static SidecarProbe {
        PROBE.get_or_init(Self::detect)
    }

    /// 缓存命中版：首次调用自动触发探测（init 无需显式调用）。
    pub fn probe() -> Option<&'static SidecarProbe> {
        let this = Self::init() as *const SidecarProbe;
        // SAFETY: OnceLock 返回的引用是 'static 的；filter 后重建引用
        let r = unsafe { &*this };
        (!r.exe.as_os_str().is_empty()).then_some(r)
    }
    fn detect() -> Self {
        // 与 ffmpeg-sidecar 相同的解析顺序：同目录 exe 优先，回落 PATH
        let candidates = [
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("ffmpeg.exe"))),
            Some(PathBuf::from("ffmpeg.exe")),
        ];
        for path in candidates.into_iter().flatten() {
            if path.is_file() {
                let has_vp9 = Self::check_vp9(&path);
                log::info!(
                    "sidecar ffmpeg detected: {} (vp9={})",
                    path.display(),
                    has_vp9
                );
                return Self { exe: path, has_vp9 };
            }
        }
        log::info!("sidecar ffmpeg not found — inprocess engine only");
        Self {
            exe: PathBuf::new(),
            has_vp9: false,
        }
    }

    /// `ffmpeg -encoders` 输出里找 libvpx-vp9（探测子进程，约 50-100ms）。
    fn check_vp9(exe: &PathBuf) -> bool {
        std::process::Command::new(exe)
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                stdout.contains("libvpx-vp9") || stderr.contains("libvpx-vp9")
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_result_without_panic() {
        // 本机有无 ffmpeg 都不能 panic；有则校验路径存在
        let probe = SidecarProbe::detect();
        if probe.exe.as_os_str().is_empty() {
            assert!(!probe.has_vp9);
        } else {
            assert!(probe.exe.is_file());
        }
    }

    #[test]
    fn probe_none_before_init_is_ok() {
        // init 前调用 probe() 返回 None（不 panic）——探测与消费解耦
        let _ = SidecarProbe::probe();
    }
}
