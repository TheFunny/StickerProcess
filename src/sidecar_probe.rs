//! sidecar（外部 ffmpeg.exe）可用性探测。
//!
//! 探测逻辑与 `ffmpeg-sidecar` 的路径解析一致（`paths::ffmpeg_path`）：
//! 1. 主程序同目录的 `ffmpeg.exe`（随包/手动放置场景）
//! 2. 系统 `PATH` 中的 `ffmpeg`
//!
//! 结果进程内缓存一次；`probe()` 只读缓存（零开销），`check_vp9`
//! 用 `-encoders` 子进程确认 libvpx-vp9（首次探测时调用一次）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static PROBE: OnceLock<SidecarProbe> = OnceLock::new();
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROBE_OUTPUT: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SidecarProbe {
    /// 解析到的 ffmpeg 可执行文件路径（存在性已确认）
    pub exe: PathBuf,
    /// 是否有 libvpx-vp9 编码器（转码必需；极简构建可能没有）
    pub has_vp9: bool,
}

impl SidecarProbe {
    /// 进程内缓存版：首次调用自动触发探测（init 无需显式存在——原先的公开
    /// init() 包装只有 main 在用，返回值还被丢弃）。
    pub fn probe() -> Option<&'static SidecarProbe> {
        let r = PROBE.get_or_init(Self::detect);
        (!r.exe.as_os_str().is_empty()).then_some(r)
    }
    fn detect() -> Self {
        Self::select_candidate(Self::candidates(), Self::check_vp9)
    }

    fn candidates() -> Vec<PathBuf> {
        let mut candidates = vec![
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("ffmpeg.exe")))
                .unwrap_or_default(),
            PathBuf::from("ffmpeg.exe"),
        ];
        candidates.extend(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join("ffmpeg.exe")),
        );
        candidates
    }

    fn select_candidate(candidates: Vec<PathBuf>, check: impl Fn(&Path) -> bool) -> Self {
        for path in candidates.into_iter().filter(|path| path.is_file()) {
            if check(&path) {
                log::info!("sidecar ffmpeg detected: {}", path.display());
                return Self {
                    exe: path,
                    has_vp9: true,
                };
            }
        }
        log::info!("sidecar ffmpeg with libvpx-vp9 not found — inprocess engine only");
        Self {
            exe: PathBuf::new(),
            has_vp9: false,
        }
    }

    fn check_vp9(exe: &Path) -> bool {
        let Ok(child) = Command::new(exe)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return false;
        };
        Self::run_probe(child, PROBE_TIMEOUT)
    }

    /// 并发排空 stdout/stderr；每路最多读取 1 MiB，超时后 kill + wait。
    fn run_probe(mut child: Child, timeout: Duration) -> bool {
        fn reader<R: Read + Send + 'static>(
            pipe: R,
        ) -> std::thread::JoinHandle<Result<Vec<u8>, std::io::Error>> {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.take(MAX_PROBE_OUTPUT)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
        }
        let out_reader = child.stdout.take().map(reader);
        let err_reader = child.stderr.take().map(reader);
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                let _ = child.kill();
                break;
            }
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => {
                    let _ = child.kill();
                    break;
                }
            }
        }
        let status = child.wait().ok();
        let stdout = out_reader
            .and_then(|reader| reader.join().ok())
            .and_then(Result::ok)
            .unwrap_or_default();
        let stderr = err_reader
            .and_then(|reader| reader.join().ok())
            .and_then(Result::ok)
            .unwrap_or_default();
        status.is_some_and(|status| status.success())
            && [stdout, stderr]
                .iter()
                .any(|bytes| String::from_utf8_lossy(bytes).contains("libvpx-vp9"))
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
    fn probe_stands_alone() {
        // 无前置初始化要求，单独调用不 panic
        let _ = SidecarProbe::probe();
    }

    #[test]
    fn skips_candidate_without_vp9() {
        let first = std::env::temp_dir().join(format!("stp_no_vp9_{}.cmd", std::process::id()));
        let second = std::env::temp_dir().join(format!("stp_with_vp9_{}.cmd", std::process::id()));
        std::fs::write(&first, "@exit /b 0\r\n").unwrap();
        std::fs::write(&second, "@echo libvpx-vp9\r\n@exit /b 0\r\n").unwrap();
        let selected =
            SidecarProbe::select_candidate(vec![first.clone(), second.clone()], |path| {
                std::fs::read_to_string(path).is_ok_and(|text| text.contains("libvpx-vp9"))
            });
        assert_eq!(selected.exe, second);
        assert!(selected.has_vp9);
        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn probe_timeout_returns_false() {
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert!(!SidecarProbe::run_probe(child, Duration::from_millis(100)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
